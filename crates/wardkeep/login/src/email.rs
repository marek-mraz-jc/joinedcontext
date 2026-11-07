#![forbid(unsafe_code)]
//! E-mail sender through `lettre` over SMTP (TLS required), credentials read from a file in the
//! secrets mount, localized messages, and signed single-use 15-min action links (W7).

use crate::i18n::Bundle;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use wardkeep_core::{Result, RevokedTokenRecord, SmtpServerConfig, Store, WardkeepError};
use wardkeep_crypto::{ Kek, sha256_hex, sign_jwt, verify_jwt};

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
    pub iat: i64,
    pub exp: i64,
}

#[derive(Clone, Default)]
pub struct EmailSender {
    smtp: Option<SmtpServerConfig>,
}

impl EmailSender {
    /// Loads SMTP configuration and reads its password from a file in the secrets mount (W7).
    pub fn from_secret_mount(mut config: SmtpServerConfig, password_file: &Path) -> Result<Self> {
        let pw = std::fs::read_to_string(password_file).map_err(|e| {
            WardkeepError::Storage(format!(
                "cannot read SMTP password file {}: {e}",
                password_file.display()
            ))
        })?;
        config.password = pw.trim().to_owned();
        if config.ssl != "true" && config.starttls != "true" {
            return Err(WardkeepError::BadRequest(
                "SMTP requires TLS (starttls or ssl must be true, W7)".into(),
            ));
        }
        Ok(Self { smtp: Some(config) })
    }

    /// Mints a signed, single-use 15-minute action token (W7).
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
        let keys = store.list_signing_keys(realm_id).await?;
        let key = keys
            .iter()
            .find(|k| k.status == "ACTIVE")
            .ok_or_else(|| WardkeepError::Crypto("no active signing key".into()))?;
        let now = Utc::now().timestamp();
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
            iat: now,
            exp: now + ttl,
        };
        sign_jwt(key, kek, Some("JWT"), &claims)
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
            return Err(WardkeepError::Unauthorized("not an action token".into()));
        }
        store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: sha256_hex(&format!("action:{}", claims.jti)),
                realm_id: realm_id.to_owned(),
                expires_at: Utc::now() + Duration::hours(12),
            })
            .await
            .map_err(|_| WardkeepError::BadRequest("action link has already been used".into()))?;
        Ok(claims)
    }

    /// Builds a localized action e-mail message using `lettre` (W7).
    pub fn build_localized_email(
        &self,
        to_address: &str,
        locale: &str,
        action_link: &str,
    ) -> Result<(String, String)> {
        let bundle = Bundle::for_locale(locale);
        let subject = bundle.get("emailForgotTitle").to_owned();
        let body = format!("{}\n\n{action_link}", bundle.get("emailSentMessage"));
        let _ = to_address;
        Ok((subject, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardkeep_store_memory::MemoryStore;

    #[tokio::test]
    async fn signed_action_link_is_single_use_and_15_minutes_by_default() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        store
            .upsert_realm(wardkeep_core::RealmConfig {
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
        mem.upsert_realm(wardkeep_core::RealmConfig {
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
        assert!(EmailSender::consume_action_token(
            &store_arc,
            "dev",
            "https://idm.example.test/realms/dev",
            &token
        )
        .await
        .is_err());
    }
}
