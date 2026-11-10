#![forbid(unsafe_code)]
//! Transactional e-mail outbox dispatcher and `MailSender` trait (W7, K9, K10).
//!
//! No plaintext token or password ever sits in `kaisatsu.email_outbox`: the row stores
//! `action_jti`, `user_id`, `actions`, `client_id`, `redirect_uri`, and `lifespan_secs`.
//! At send time the dispatcher mints the signed `SignedActionToken` with that `action_jti`,
//! builds the localized message, and delivers it through a [`MailSender`] implementation
//! ([`EmailSender`] over TLS SMTP in production, or [`RecordingSender`] in tests).

use crate::email::{EmailSender, SignedActionToken};
use crate::i18n::Bundle;
use chrono::{Duration, Utc};
use futures_core::future::BoxFuture;
use jc_router::Mount;
use kaisatsu_core::{EmailOutboxRecord, KaisatsuError, Result, Store};
use kaisatsu_crypto::{Kek, sign_jwt};
use std::sync::Arc;
use std::sync::Mutex;

pub const DEFAULT_OUTBOX_MAX_ATTEMPTS: u32 = 5;
pub const MAX_OUTBOX_BATCH: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMail {
    pub realm_id: String,
    pub to_address: String,
    pub subject: String,
    pub body: String,
    pub action_link: String,
}

/// Trait abstracting e-mail delivery so tests can inspect sent messages without network calls,
/// and production uses `lettre` over TLS SMTP (K9).
pub trait MailSender: Send + Sync {
    fn send_mail<'a>(
        &'a self,
        realm_id: &'a str,
        to_address: &'a str,
        subject: &'a str,
        body: &'a str,
        action_link: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
}

impl MailSender for EmailSender {
    fn send_mail<'a>(
        &'a self,
        _realm_id: &'a str,
        to_address: &'a str,
        subject: &'a str,
        body: &'a str,
        _action_link: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let msg = self.build_message(to_address, subject, body.to_owned())?;
            self.send(msg).await
        })
    }
}

/// In-memory test sink implementing [`MailSender`] that records delivered messages and can be
/// configured to fail a set number of times to test retry/backoff and failure transitions (K9).
#[derive(Clone, Default)]
pub struct RecordingSender {
    sent: Arc<Mutex<Vec<RecordedMail>>>,
    fail_remaining: Arc<Mutex<u32>>,
}

impl RecordingSender {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_failures(&self, count: u32) {
        *self
            .fail_remaining
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = count;
    }

    #[must_use]
    pub fn drain(&self) -> Vec<RecordedMail> {
        std::mem::take(
            &mut *self
                .sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    #[must_use]
    pub fn messages(&self) -> Vec<RecordedMail> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl MailSender for RecordingSender {
    fn send_mail<'a>(
        &'a self,
        realm_id: &'a str,
        to_address: &'a str,
        subject: &'a str,
        body: &'a str,
        action_link: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            {
                let mut rem = self
                    .fail_remaining
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if *rem > 0 {
                    *rem -= 1;
                    return Err(KaisatsuError::Storage("simulated SMTP failure".into()));
                }
            }
            self.sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(RecordedMail {
                    realm_id: realm_id.to_owned(),
                    to_address: to_address.to_owned(),
                    subject: subject.to_owned(),
                    body: body.to_owned(),
                    action_link: action_link.to_owned(),
                });
            Ok(())
        })
    }
}

/// Dispatcher that claims pending [`EmailOutboxRecord`] rows with `FOR UPDATE SKIP LOCKED`,
/// mints their signed action links at send time, delivers them via [`MailSender`], and records
/// completion or exponential backoff (K9).
#[derive(Clone)]
pub struct OutboxDispatcher {
    store: Arc<dyn Store>,
    kek: Kek,
    sender: Arc<dyn MailSender>,
    max_attempts: u32,
}

impl OutboxDispatcher {
    #[must_use]
    pub fn new(store: Arc<dyn Store>, kek: Kek, sender: Arc<dyn MailSender>) -> Self {
        Self {
            store,
            kek,
            sender,
            max_attempts: DEFAULT_OUTBOX_MAX_ATTEMPTS,
        }
    }

    #[must_use]
    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }

    /// Mints the signed action link for `row` using its stored `action_jti`.
    pub async fn mint_link_for_row(
        &self,
        mount: &Mount,
        row: &EmailOutboxRecord,
    ) -> Result<String> {
        let keys = self.store.list_signing_keys(&row.realm_id).await?;
        let key = keys
            .iter()
            .find(|k| k.status == "ACTIVE")
            .ok_or_else(|| KaisatsuError::Crypto("no active signing key".into()))?;
        let now = Utc::now().timestamp();
        let ttl = row.lifespan_secs.clamp(60, 43_200);
        let claims = SignedActionToken {
            iss: row.issuer.clone(),
            sub: row.user_id.clone(),
            aud: row.issuer.clone(),
            jti: row.action_jti.clone(),
            typ: "Action".into(),
            actions: row.actions.clone(),
            client_id: row.client_id.clone(),
            redirect_uri: row.redirect_uri.clone(),
            iat: now,
            exp: now + ttl,
        };
        let token = sign_jwt(key, &self.kek, Some("JWT"), &claims)?;
        let realm = self
            .store
            .get_realm(&row.realm_id)
            .await?
            .map(|r| r.realm)
            .unwrap_or_else(|| row.realm_id.clone());
        Ok(format!(
            "{}/realms/{}/login-actions/action-token?key={}",
            mount.public_base(),
            realm,
            url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>()
        ))
    }

    /// Processes up to `batch_size` (capped at 50) due outbox rows for `realm_id`.
    pub async fn dispatch_realm_batch(
        &self,
        mount: &Mount,
        realm_id: &str,
        batch_size: usize,
    ) -> Result<usize> {
        let claimed = self
            .store
            .claim_outbox_batch(realm_id, batch_size.clamp(1, MAX_OUTBOX_BATCH))
            .await?;
        let mut sent_count = 0usize;
        // Keycloak's `{2}`: the realm's display name, else its name.
        let realm_name = if claimed.is_empty() {
            String::new()
        } else {
            self.store
                .list_realms()
                .await?
                .into_iter()
                .find(|r| r.id == realm_id)
                .map(|r| {
                    if r.display_name.trim().is_empty() {
                        r.realm
                    } else {
                        r.display_name
                    }
                })
                .unwrap_or_default()
        };
        for row in claimed {
            let link = match self.mint_link_for_row(mount, &row).await {
                Ok(l) => l,
                Err(e) => {
                    let _ = self
                        .store
                        .complete_outbox_row(
                            &row.realm_id,
                            &row.id,
                            false,
                            Some(e.public_detail()),
                            Some(Utc::now() + Duration::seconds(10)),
                            self.max_attempts,
                        )
                        .await;
                    continue;
                }
            };
            let (subject, body) =
                action_mail(&Bundle::for_locale(&row.locale), &row, &realm_name, &link);
            match self
                .sender
                .send_mail(&row.realm_id, &row.to_address, &subject, &body, &link)
                .await
            {
                Ok(()) => {
                    self.store
                        .complete_outbox_row(
                            &row.realm_id,
                            &row.id,
                            true,
                            None,
                            None,
                            self.max_attempts,
                        )
                        .await?;
                    sent_count += 1;
                }
                Err(e) => {
                    let backoff_secs = i64::from(row.attempts.min(6)) * 5;
                    self.store
                        .complete_outbox_row(
                            &row.realm_id,
                            &row.id,
                            false,
                            Some(e.public_detail()),
                            Some(Utc::now() + Duration::seconds(backoff_secs)),
                            self.max_attempts,
                        )
                        .await?;
                }
            }
        }
        Ok(sent_count)
    }

    /// One dispatch pass over every realm: up to [`MAX_OUTBOX_BATCH`] due rows each (K9). A
    /// failing realm is logged and does not stop the others.
    pub async fn dispatch_all_realms(&self, mount: &Mount) -> Result<usize> {
        let mut sent = 0usize;
        for realm in self.store.list_realms().await? {
            match self
                .dispatch_realm_batch(mount, &realm.id, MAX_OUTBOX_BATCH)
                .await
            {
                Ok(n) => sent += n,
                Err(e) => {
                    tracing::warn!(realm = %realm.realm, error = %e.public_detail(), "kaisatsu outbox pass")
                }
            }
        }
        Ok(sent)
    }
}

/// Production [`MailSender`]: each realm's own SMTP settings (`smtpServer`, TLS required) with
/// the SMTP password read from one file in the secrets mount at send time, never stored or
/// logged (K9).
pub struct RealmSmtpSender {
    store: Arc<dyn Store>,
    password_file: std::path::PathBuf,
}

impl RealmSmtpSender {
    #[must_use]
    pub fn new(store: Arc<dyn Store>, password_file: std::path::PathBuf) -> Self {
        Self {
            store,
            password_file,
        }
    }
}

impl MailSender for RealmSmtpSender {
    fn send_mail<'a>(
        &'a self,
        realm_id: &'a str,
        to_address: &'a str,
        subject: &'a str,
        body: &'a str,
        action_link: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let smtp = self
                .store
                .get_realm(realm_id)
                .await?
                .and_then(|r| r.smtp_server)
                .ok_or_else(|| KaisatsuError::BadRequest("realm has no smtpServer".into()))?;
            let sender = EmailSender::from_secret_mount(smtp, &self.password_file)?;
            sender
                .send_mail(realm_id, to_address, subject, body, action_link)
                .await
        })
    }
}

/// The `(subject, body)` of an action e-mail as Keycloak's e-mail theme words it
/// (emailVerification, passwordReset or executeActions): `{0}` the link, `{1}` its lifetime in
/// minutes, `{2}` the realm's name, then the actions (executeActions) and the lifetime in words.
fn action_mail(
    bundle: &Bundle,
    row: &EmailOutboxRecord,
    realm_name: &str,
    link: &str,
) -> (String, String) {
    let minutes = (row.lifespan_secs / 60).to_string();
    let expires = link_expiration(bundle, row.lifespan_secs);
    let actions = row
        .actions
        .iter()
        .map(|a| bundle.get(&format!("email.requiredAction.{a}")).to_owned())
        .collect::<Vec<_>>()
        .join(", ");
    let (key, args) = match row.template_kind.as_str() {
        "VERIFY_EMAIL" => (
            "emailVerification",
            vec![link, &minutes, realm_name, &expires],
        ),
        "RESET_CREDENTIALS" => ("passwordReset", vec![link, &minutes, realm_name, &expires]),
        _ => (
            "executeActions",
            vec![link, &minutes, realm_name, &actions, &expires],
        ),
    };
    let subject = bundle.get(&format!("email.{key}Subject")).to_owned();
    let body = bundle
        .format(&format!("email.{key}Body"), &args)
        .replace("\\n", "\n");
    (subject, body)
}

/// Keycloak's `linkExpirationFormatter`: the lifetime in the largest whole unit ("5 minutes",
/// "12 hours"), the unit's word picked by its `{0,choice,...}` pattern.
fn link_expiration(bundle: &Bundle, secs: i64) -> String {
    let (n, unit) = [(86_400, "days"), (3_600, "hours"), (60, "minutes")]
        .into_iter()
        .find(|(size, _)| secs >= *size && secs % size == 0)
        .map_or((secs, "seconds"), |(size, unit)| (secs / size, unit));
    let word = choice(
        bundle.get(&format!(
            "email.linkExpirationFormatter.timePeriodUnit.{unit}"
        )),
        n,
    );
    format!("{n} {word}")
}

/// Java `ChoiceFormat` for `{0,choice,0#minutes|1#minute|1<minutes}`: the last option whose limit
/// `n` reaches (`#` at least, `<` above); a plain word stays as it is.
fn choice(pattern: &str, n: i64) -> String {
    let Some(options) = pattern
        .strip_prefix("{0,choice,")
        .and_then(|p| p.strip_suffix('}'))
    else {
        return pattern.to_owned();
    };
    let mut picked = "";
    for option in options.split('|') {
        let (limit, word, strict) = match option.split_once('#') {
            Some((l, w)) => (l, w, false),
            None => match option.split_once('<') {
                Some((l, w)) => (l, w, true),
                None => continue,
            },
        };
        if let Ok(limit) = limit.trim().parse::<i64>()
            && ((strict && n > limit) || (!strict && n >= limit))
        {
            picked = word;
        }
    }
    picked.to_owned()
}

#[cfg(test)]
mod tests {
    use super::choice;

    #[test]
    fn choice_picks_the_form_of_the_number() {
        let sk = "{0,choice,0#minút|1#minúta|2#minúty|4<minút}";
        assert_eq!(choice(sk, 1), "minúta");
        assert_eq!(choice(sk, 3), "minúty");
        assert_eq!(choice(sk, 5), "minút");
        assert_eq!(
            choice("{0,choice,0#minutes|1#minute|1<minutes}", 2),
            "minutes"
        );
        assert_eq!(choice("perc", 5), "perc");
    }
}
