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
use std::sync::Mutex;
use std::sync::Arc;

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
        *self.fail_remaining.lock().unwrap() = count;
    }

    #[must_use]
    pub fn drain(&self) -> Vec<RecordedMail> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    #[must_use]
    pub fn messages(&self) -> Vec<RecordedMail> {
        self.sent.lock().unwrap().clone()
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
                let mut rem = self.fail_remaining.lock().unwrap();
                if *rem > 0 {
                    *rem -= 1;
                    return Err(KaisatsuError::Storage("simulated SMTP failure".into()));
                }
            }
            self.sent.lock().unwrap().push(RecordedMail {
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
    pub async fn mint_link_for_row(&self, mount: &Mount, row: &EmailOutboxRecord) -> Result<String> {
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
            let bundle = Bundle::for_locale(&row.locale);
            let subject_key = if row.template_kind == "VERIFY_EMAIL" {
                "emailVerifyTitle"
            } else {
                "emailForgotTitle"
            };
            let subject = bundle.get(subject_key);
            let body = format!("{}\n\n{link}", bundle.get("emailSentMessage"));
            match self
                .sender
                .send_mail(&row.realm_id, &row.to_address, subject, &body, &link)
                .await
            {
                Ok(()) => {
                    self.store
                        .complete_outbox_row(&row.realm_id, &row.id, true, None, None, self.max_attempts)
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
}
