#![forbid(unsafe_code)]
//! E-mail sender through `lettre` over SMTP (TLS required), credentials read from a file in the
//! secrets mount, localized messages, and signed single-use 15-min action links (W7).

use crate::i18n::Bundle;
use jc_time::TimestampTz;
use jiff::SignedDuration;
use kaisatsu_core::{KaisatsuError, Result, RevokedTokenRecord, SmtpServerConfig, Store};
use kaisatsu_crypto::{Kek, sha256_hex, sign_jwt, verify_jwt, verify_jwt_allowing_expired};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Action token lifespan: 15 minutes, single-use (W7).
pub const ACTION_TOKEN_TTL_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedActionToken {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub jti: String,
    pub typ: String,
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// [`email_binding`] of the address the link was sent to: `VERIFY_EMAIL` verifies only that
    /// address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub iat: i64,
    pub exp: i64,
}

/// What an action token stores of `address`: its hash, compared case-blind.
#[must_use]
pub fn email_binding(address: &str) -> String {
    sha256_hex(&format!("action-email:{}", address.trim().to_lowercase()))
}

/// Signs `claims` with the realm's active key. The memory store keeps one active key per realm;
/// a store that does not finds none here and mints no link.
pub(crate) async fn sign_action_token(
    store: &dyn Store,
    kek: &Kek,
    realm_id: &str,
    claims: &SignedActionToken,
) -> Result<String> {
    let keys = store.list_signing_keys(realm_id).await?;
    let Some(key) = keys.iter().find(|k| k.status == "ACTIVE") else {
        return Err(KaisatsuError::Crypto("no active signing key".into()));
    };
    sign_jwt(key, kek, Some("JWT"), claims)
}

#[derive(Clone, Default)]
pub struct EmailSender {
    smtp: Option<SmtpServerConfig>,
}

impl EmailSender {
    /// Loads SMTP configuration and reads its password from a file in the secrets mount (W7).
    pub fn from_secret_mount(mut config: SmtpServerConfig, password_file: &Path) -> Result<Self> {
        let pw = std::fs::read_to_string(password_file).map_err(|e| {
            KaisatsuError::Storage(format!(
                "cannot read SMTP password file {}: {e}",
                password_file.display()
            ))
        })?;
        config.password = pw.trim().to_owned();
        Self::from_stored(config)
    }

    /// The realm's SMTP settings with the password kaisatsu keeps sealed under its KEK
    /// (`realms.smtp_password_enc`), as the Portal's operators save them (T-7152). TLS required.
    pub fn from_stored(config: SmtpServerConfig) -> Result<Self> {
        if config.ssl != "true" && config.starttls != "true" {
            return Err(KaisatsuError::BadRequest(
                "SMTP requires TLS (starttls or ssl must be true, W7)".into(),
            ));
        }
        Ok(Self { smtp: Some(config) })
    }

    /// The SMTP port the settings name.
    pub fn port(&self) -> Result<u16> {
        self.smtp
            .as_ref()
            .ok_or_else(|| KaisatsuError::BadRequest("SMTP is not configured".into()))?
            .port
            .parse()
            .map_err(|_| KaisatsuError::BadRequest("invalid SMTP port".into()))
    }

    /// Mints a signed, single-use 15-minute action token (W7).
    #[allow(clippy::too_many_arguments)]
    pub async fn mint_action_token(
        store: &Arc<dyn Store>,
        kek: &Kek,
        realm_id: &str,
        issuer: &str,
        user_id: &str,
        actions: Vec<String>,
        client_id: Option<String>,
        redirect_uri: Option<String>,
        lifespan_secs: Option<i64>,
    ) -> Result<String> {
        let email = store
            .get_user_by_id(realm_id, None, user_id)
            .await?
            .and_then(|u| u.email)
            .map(|e| email_binding(&e));
        let now = TimestampTz::now().as_second();
        let ttl = lifespan_secs.unwrap_or(ACTION_TOKEN_TTL_SECS).min(43_200);
        let claims = SignedActionToken {
            iss: issuer.to_owned(),
            sub: user_id.to_owned(),
            aud: issuer.to_owned(),
            jti: uuid::Uuid::now_v7().to_string(),
            typ: "Action".into(),
            actions,
            client_id,
            redirect_uri,
            email,
            iat: now,
            exp: now + ttl,
        };
        sign_action_token(store.as_ref(), kek, realm_id, &claims).await
    }

    /// Verifies an action token without using it up: a link opened (or prefetched by a mail
    /// scanner) is only applied by the confirming POST.
    pub async fn check_action_token(
        store: &Arc<dyn Store>,
        realm_id: &str,
        issuer: &str,
        token: &str,
    ) -> Result<SignedActionToken> {
        let keys = store.list_signing_keys(realm_id).await?;
        let (_, claims): (_, SignedActionToken) =
            verify_jwt(token, &keys, Some(issuer), Some(issuer))?;
        if claims.typ != "Action" {
            return Err(KaisatsuError::Unauthorized("not an action token".into()));
        }
        if store
            .is_token_revoked(realm_id, &sha256_hex(&format!("action:{}", claims.jti)))
            .await?
        {
            return Err(KaisatsuError::BadRequest(
                "action link has already been used".into(),
            ));
        }
        Ok(claims)
    }

    /// The claims of an action link that [`Self::check_action_token`] or
    /// [`Self::consume_action_token`] refused, read only to tell its holder why: the signature and
    /// issuer are checked and the expiry is not, so the page can say "expired" and offer the next
    /// step that fits the link's action. A link that does not verify yields `None`.
    pub async fn inspect_refused_action_token(
        store: &Arc<dyn Store>,
        realm_id: &str,
        issuer: &str,
        token: &str,
    ) -> Option<SignedActionToken> {
        let keys = store.list_signing_keys(realm_id).await.ok()?;
        let (_, claims): (_, SignedActionToken) =
            verify_jwt_allowing_expired(token, &keys, Some(issuer)).ok()?;
        (claims.typ == "Action" && claims.aud == issuer).then_some(claims)
    }

    /// Verifies and consumes a single-use action token (second use is refused via `revoked_token`, W7).
    pub async fn consume_action_token(
        store: &Arc<dyn Store>,
        realm_id: &str,
        issuer: &str,
        token: &str,
    ) -> Result<SignedActionToken> {
        let keys = store.list_signing_keys(realm_id).await?;
        let (_, claims): (_, SignedActionToken) =
            verify_jwt(token, &keys, Some(issuer), Some(issuer))?;
        if claims.typ != "Action" {
            return Err(KaisatsuError::Unauthorized("not an action token".into()));
        }
        store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: sha256_hex(&format!("action:{}", claims.jti)),
                realm_id: realm_id.to_owned(),
                expires_at: TimestampTz::now() + SignedDuration::from_hours(12),
            })
            .await
            .map_err(|_| KaisatsuError::BadRequest("action link has already been used".into()))?;
        Ok(claims)
    }

    /// Builds a localized action e-mail as `(subject, body)` (W7).
    pub fn build_localized_email(
        &self,
        to_address: &str,
        locale: &str,
        action_link: &str,
    ) -> Result<(String, String)> {
        to_address
            .parse::<lettre::Address>()
            .map_err(|_| KaisatsuError::BadRequest("invalid recipient address".into()))?;
        let bundle = Bundle::for_locale(locale);
        let subject = bundle.get("emailForgotTitle").to_owned();
        let body = format!("{}\n\n{action_link}", bundle.get("emailSentMessage"));
        Ok((subject, body))
    }

    /// Builds the `lettre` message for `to_address` from the realm `from` address.
    pub fn build_message(
        &self,
        to_address: &str,
        subject: &str,
        body: String,
    ) -> Result<lettre::Message> {
        let smtp = self
            .smtp
            .as_ref()
            .ok_or_else(|| KaisatsuError::BadRequest("SMTP is not configured".into()))?;
        let from = smtp
            .from
            .parse::<lettre::message::Mailbox>()
            .map_err(|_| KaisatsuError::BadRequest("invalid SMTP from address".into()))?;
        let to = to_address
            .parse::<lettre::message::Mailbox>()
            .map_err(|_| KaisatsuError::BadRequest("invalid recipient address".into()))?;
        // One sender and one recipient, both parsed above: lettre refuses only a message
        // missing either.
        let Ok(message) = lettre::Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(body)
        else {
            return Err(KaisatsuError::BadRequest("cannot build e-mail".into()));
        };
        Ok(message)
    }

    /// Sends `message` over SMTP with TLS (implicit TLS when `ssl`, otherwise STARTTLS) (W7).
    /// The error never carries the SMTP password or the server's reply text.
    pub async fn send(&self, message: lettre::Message) -> Result<()> {
        self.send_via(message, None, &[]).await
    }

    /// [`Self::send`] to the address `pinned` (one the outbound guard judged, W12) instead of
    /// the host's name: the TLS certificate is still checked against the host's name. `roots`
    /// are DER certificates trusted beside the public roots
    /// ([`kaisatsu_oidc::OutboundPolicy::test_smtp_roots`], empty outside tests).
    pub async fn send_via(
        &self,
        message: lettre::Message,
        pinned: Option<std::net::SocketAddr>,
        roots: &[Vec<u8>],
    ) -> Result<()> {
        use lettre::AsyncTransport as _;
        use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
        type Transport = lettre::AsyncSmtpTransport<lettre::Tokio1Executor>;
        let smtp = self
            .smtp
            .as_ref()
            .ok_or_else(|| KaisatsuError::BadRequest("SMTP is not configured".into()))?;
        let port = self.port()?;
        // rustls checks the name when it connects; building the parameters fails only for a TLS
        // version rustls lacks, and the minimum here is TLS 1.2.
        let tls = roots
            .iter()
            .filter_map(|der| Certificate::from_der(der.clone()).ok())
            .fold(
                TlsParameters::builder(smtp.host.trim().to_owned()),
                lettre::transport::smtp::client::TlsParametersBuilder::add_root_certificate,
            );
        let Ok(tls) = tls.build() else {
            return Err(KaisatsuError::BadRequest("invalid SMTP host".into()));
        };
        let target = match pinned {
            Some(addr) => addr.ip().to_string(),
            None => smtp.host.clone(),
        };
        let builder = Transport::builder_dangerous(target)
            .tls(if smtp.ssl == "true" {
                Tls::Wrapper(tls)
            } else {
                Tls::Required(tls)
            })
            .port(port)
            .timeout(Some(std::time::Duration::from_secs(10)));
        let builder = if smtp.auth == "true" {
            builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                smtp.user.clone(),
                smtp.password.clone(),
            ))
        } else {
            builder
        };
        builder
            .build()
            .send(message)
            .await
            .map(|_| ())
            .map_err(|_| KaisatsuError::Storage("SMTP delivery failed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaisatsu_store_memory::MemoryStore;

    /// The invariants send_via and build_message rely on: building the TLS parameters never
    /// fails for any host text (rustls checks the name only when it connects), and a message
    /// with one parsed sender and one parsed recipient always builds.
    #[test]
    fn tls_parameters_and_a_two_party_message_always_build() {
        use lettre::transport::smtp::client::TlsParameters;
        for host in [
            "mail.example",
            "",
            "a b",
            "10.0.0.1",
            "\u{fc}n\u{ef}.example",
            "x".repeat(300).as_str(),
        ] {
            assert!(TlsParameters::new(host.to_owned()).is_ok(), "{host}");
        }
        for (from, to) in [
            ("noreply@example.test", "person@example.test"),
            ("Name <a@b.test>", "\"Q, R\" <c@d.test>"),
        ] {
            let built = lettre::Message::builder()
                .from(from.parse().unwrap())
                .to(to.parse().unwrap())
                .subject("")
                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                .body(String::new());
            assert!(built.is_ok(), "{from} {to}");
        }
    }

    #[test]
    fn from_secret_mount_reads_password_file_and_requires_tls() {
        let dir = std::env::temp_dir().join(format!("wk-smtp-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let pw = dir.join("password");
        std::fs::write(&pw, "s3cret\n").unwrap();
        let cfg = SmtpServerConfig {
            host: "smtp.example.org".into(),
            from: "idm@example.org".into(),
            ..SmtpServerConfig::default()
        };
        let sender = EmailSender::from_secret_mount(cfg.clone(), &pw).unwrap();
        assert_eq!(sender.smtp.as_ref().unwrap().password, "s3cret");
        let msg = sender
            .build_message("anna@example.org", "subject", "body".into())
            .unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("To: anna@example.org"));
        assert!(!raw.contains("s3cret"));

        let plain = SmtpServerConfig {
            starttls: "false".into(),
            ssl: "false".into(),
            ..cfg
        };
        assert!(EmailSender::from_secret_mount(plain, &pw).is_err());
        let missing = dir.join("absent");
        let err = EmailSender::from_secret_mount(SmtpServerConfig::default(), &missing)
            .err()
            .unwrap();
        assert!(!err.to_string().contains("s3cret"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn unconfigured_sender_refuses_to_send_and_rejects_bad_addresses() {
        let sender = EmailSender::default();
        assert!(
            sender
                .build_message("a@example.org", "s", "b".into())
                .is_err()
        );
        assert!(
            sender
                .build_localized_email("not an address", "en", "https://x")
                .is_err()
        );
        let (subject, body) = sender
            .build_localized_email("a@example.org", "en", "https://idm/link")
            .unwrap();
        assert!(!subject.is_empty());
        assert!(body.ends_with("https://idm/link"));
    }

    /// Without SMTP settings neither the port nor a send goes anywhere; a send pinned to an
    /// address that refuses the connection fails with the cause and without the password.
    #[tokio::test]
    async fn sends_need_settings_a_nameable_host_and_a_server_that_answers() {
        let unconfigured = EmailSender::default();
        assert_eq!(
            unconfigured.port().unwrap_err().to_string(),
            KaisatsuError::BadRequest("SMTP is not configured".into()).to_string()
        );
        let cfg = SmtpServerConfig {
            host: "localhost".into(),
            from: "idm@example.org".into(),
            ssl: "true".into(),
            auth: "true".into(),
            user: "idm".into(),
            password: "smtp-word-for-tests".into(),
            ..SmtpServerConfig::default()
        };
        let sender = EmailSender::from_stored(cfg.clone()).unwrap();
        let msg = || {
            sender
                .build_message("anna@example.org", "s", "b".into())
                .unwrap()
        };
        let err = unconfigured.send_via(msg(), None, &[]).await.unwrap_err();
        assert!(err.to_string().contains("SMTP is not configured"), "{err}");

        // Port 1 on loopback: nothing listens and no unprivileged test can bind it, so the
        // connection is refused at once.
        let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        for (ssl, pin) in [("true", Some(addr)), ("false", Some(addr)), ("true", None)] {
            let pinned = EmailSender::from_stored(SmtpServerConfig {
                ssl: ssl.into(),
                port: addr.port().to_string(),
                ..cfg.clone()
            })
            .unwrap();
            let err = pinned.send_via(msg(), pin, &[]).await.unwrap_err();
            assert!(
                err.to_string().contains("SMTP delivery failed"),
                "{ssl} {pin:?}: {err}"
            );
            assert!(!err.to_string().contains("smtp-word-for-tests"));
        }
    }

    /// A token of the realm's key that is not an action token is refused as one.
    #[tokio::test]
    async fn a_token_of_another_type_is_no_action_link() {
        let kek = Kek::generate_for_tests();
        let mem = MemoryStore::new(kek.clone());
        mem.upsert_realm(kaisatsu_core::RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let store: Arc<dyn Store> = Arc::new(mem);
        let issuer = "https://idm.example.test/realms/dev";
        let keys = store.list_signing_keys("dev").await.unwrap();
        let active = keys.iter().find(|k| k.status == "ACTIVE").unwrap().clone();
        let now = TimestampTz::now().as_second();
        let refresh = SignedActionToken {
            iss: issuer.into(),
            sub: "u1".into(),
            aud: issuer.into(),
            jti: "j1".into(),
            typ: "Refresh".into(),
            actions: vec!["UPDATE_PASSWORD".into()],
            client_id: None,
            redirect_uri: None,
            email: None,
            iat: now,
            exp: now + 60,
        };
        let token = sign_jwt(&active, &kek, Some("JWT"), &refresh).unwrap();
        let err = EmailSender::check_action_token(&store, "dev", issuer, &token)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not an action token"), "{err}");
    }

    #[tokio::test]
    async fn signed_action_link_is_single_use_and_15_minutes_by_default() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        store
            .upsert_realm(kaisatsu_core::RealmConfig {
                id: "dev".into(),
                realm: "dev".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let kek = store
            .list_signing_keys("dev")
            .await
            .ok()
            .and_then(|_| None)
            .unwrap_or_else(Kek::generate_for_tests);
        let mem = MemoryStore::new(kek.clone());
        mem.upsert_realm(kaisatsu_core::RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let store_arc: Arc<dyn Store> = Arc::new(mem);

        let token = EmailSender::mint_action_token(
            &store_arc,
            &kek,
            "dev",
            "https://idm.example.test/realms/dev",
            "u1",
            vec!["UPDATE_PASSWORD".into()],
            Some("portal-ui".into()),
            Some("https://portal.example.test/".into()),
            None,
        )
        .await
        .expect("mint");

        let claims = EmailSender::consume_action_token(
            &store_arc,
            "dev",
            "https://idm.example.test/realms/dev",
            &token,
        )
        .await
        .expect("first use succeeds");
        assert_eq!(claims.sub, "u1");
        assert_eq!(claims.exp - claims.iat, 900);

        // Second use is refused
        assert!(
            EmailSender::consume_action_token(
                &store_arc,
                "dev",
                "https://idm.example.test/realms/dev",
                &token
            )
            .await
            .is_err()
        );
    }
}
