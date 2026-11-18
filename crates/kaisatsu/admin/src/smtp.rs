#![forbid(unsafe_code)]
//! The realm's SMTP server, set without the rest of the realm (T-7152): the Portal's operators
//! save it through an approved operator Change and the Portal's reconciler writes it here.
//!
//! - `GET /admin/realms/{realm}/smtp-server`: the settings and whether a password is stored, never
//!   the password; `404` while the realm has none.
//! - `PUT /admin/realms/{realm}/smtp-server`: replaces them. A body without `password` keeps the
//!   stored one, which [`kaisatsu_core::Store::upsert_realm`] keeps sealed under the KEK
//!   (`realms.smtp_password_enc`); the admin event never carries it.
//! - `POST /admin/realms/{realm}/smtp-server/test`: sends one test e-mail to `to` through the stored
//!   settings, the host judged by the outbound guard (W12), and says whether the server took it.
//!
//! Each door needs a realm applier or `realm-management` `manage-smtp` (`Principal::may_set_smtp`),
//! in the realm itself, with no Organization pinned.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use jc_router::Mount;
use kaisatsu_core::{KaisatsuError, RequestCtx, Result, SmtpServerConfig};
use kaisatsu_login::{MailSender as _, StoredSmtpSender};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::compat::{AdminState, extract_admin_ctx, record_admin_write};

/// How the connection to the SMTP server is protected: `starttls` (usually port 587) or `ssl`,
/// implicit TLS (usually 465). Plain SMTP is never offered (W7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    Starttls,
    Ssl,
}

/// The settings as they are read: no password, only whether one is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmtpView {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub from: String,
    pub user: String,
    pub password_set: bool,
}

/// The settings as they are written. `password` absent keeps the stored one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SmtpWrite {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub from: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpTest {
    pub to: String,
}

/// Whether the SMTP server took the test e-mail, and the reason a person can act on when not.
#[derive(Debug, Serialize, Deserialize)]
pub struct SmtpTestResult {
    pub sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn invalid(field: &str, message: &str) -> KaisatsuError {
    KaisatsuError::field_error(field, message)
}

/// A host name of letters, digits, `-` and `.`, as SMTP servers are named (`smtp.example.org`):
/// no address literal, scheme, user, port or path.
fn host_name(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            (1..=63).contains(&l.len())
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels
            .last()
            .is_some_and(|tld| tld.bytes().any(|b| b.is_ascii_alphabetic()))
}

impl SmtpWrite {
    /// The settings checked field by field, the stored password kept when none is given.
    fn into_config(self, stored: Option<&SmtpServerConfig>) -> Result<SmtpServerConfig> {
        let host = self.host.trim().trim_end_matches('.').to_ascii_lowercase();
        if !host_name(&host) {
            return Err(invalid(
                "host",
                "give the SMTP server's host name, e.g. smtp.example.org",
            ));
        }
        if self.port == 0 {
            return Err(invalid("port", "give a port from 1 to 65535, e.g. 587"));
        }
        let from = self.from.trim().to_owned();
        if from.len() > 254 || from.parse::<lettre::message::Mailbox>().is_err() {
            return Err(invalid(
                "from",
                "give the address e-mails come from, e.g. noreply@example.org",
            ));
        }
        let user = self.user.trim().to_owned();
        if user.len() > 256 || user.chars().any(char::is_control) {
            return Err(invalid(
                "user",
                "give the SMTP user name, at most 256 characters",
            ));
        }
        let password = match self.password {
            Some(p) if p.len() > 1024 || p.chars().any(|c| c == '\r' || c == '\n') => {
                return Err(invalid(
                    "password",
                    "the password is at most 1024 characters on one line",
                ));
            }
            Some(p) if !p.is_empty() => p,
            // The stored password goes only to the server it was given for: a new host or user
            // needs it typed again.
            _ => stored
                .filter(|s| s.host == host && s.user == user)
                .map(|s| s.password.clone())
                .unwrap_or_default(),
        };
        if !user.is_empty() && password.is_empty() {
            return Err(invalid("password", "a user needs its password"));
        }
        let tls = self.tls;
        Ok(SmtpServerConfig {
            host,
            port: self.port.to_string(),
            from,
            auth: (!user.is_empty()).to_string(),
            user,
            password,
            starttls: (tls == SmtpTls::Starttls).to_string(),
            ssl: (tls == SmtpTls::Ssl).to_string(),
        })
    }
}

fn view(smtp: &SmtpServerConfig) -> SmtpView {
    SmtpView {
        host: smtp.host.clone(),
        port: smtp.port.parse().unwrap_or_default(),
        tls: if smtp.ssl == "true" {
            SmtpTls::Ssl
        } else {
            SmtpTls::Starttls
        },
        from: smtp.from.clone(),
        user: smtp.user.clone(),
        password_set: !smtp.password.is_empty(),
    }
}

async fn caller(
    state: &AdminState,
    mount: Option<Extension<Mount>>,
    realm: &str,
    headers: &HeaderMap,
) -> Result<RequestCtx> {
    let mount = state.oidc.mount_from(mount.as_ref().map(|e| &e.0));
    let ctx = extract_admin_ctx(state, &mount, realm, headers).await?;
    if ctx.active_org_id.is_none() && ctx.principal.may_set_smtp() {
        Ok(ctx)
    } else {
        Err(KaisatsuError::Forbidden(
            "the realm's SMTP server needs manage-smtp or a realm applier".into(),
        ))
    }
}

/// The realm the caller is in: its own (`extract_admin_ctx` checked the token's issuer).
async fn realm_of(state: &AdminState, ctx: &RequestCtx) -> Result<kaisatsu_core::RealmConfig> {
    state
        .oidc
        .store
        .get_realm(&ctx.realm_id)
        .await?
        .ok_or_else(|| KaisatsuError::NotFound("realm not found".into()))
}

pub(crate) async fn get_smtp(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount: Option<Extension<Mount>>,
) -> Result<Json<SmtpView>> {
    let ctx = caller(&state, mount, &realm, &headers).await?;
    realm_of(&state, &ctx)
        .await?
        .smtp_server
        .filter(|s| !s.host.trim().is_empty())
        .map(|s| Json(view(&s)))
        .ok_or_else(|| KaisatsuError::NotFound("the realm has no SMTP server".into()))
}

pub(crate) async fn put_smtp(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount: Option<Extension<Mount>>,
    Json(body): Json<SmtpWrite>,
) -> Result<StatusCode> {
    let ctx = caller(&state, mount, &realm, &headers).await?;
    let mut current = realm_of(&state, &ctx).await?;
    let smtp = body.into_config(current.smtp_server.as_ref())?;
    let shown = serde_json::to_value(view(&smtp)).ok();
    current.smtp_server = Some(smtp);
    state.oidc.store.upsert_realm(current).await?;
    record_admin_write(&state, &ctx, "UPDATE", "REALM_SMTP", &realm, shown).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn test_smtp(
    State(state): State<AdminState>,
    Path(realm): Path<String>,
    headers: HeaderMap,
    mount: Option<Extension<Mount>>,
    Json(body): Json<SmtpTest>,
) -> Result<Json<SmtpTestResult>> {
    let ctx = caller(&state, mount, &realm, &headers).await?;
    let to = body.to.trim();
    if to.parse::<lettre::message::Mailbox>().is_err() {
        return Err(invalid(
            "to",
            "give the address the test goes to, e.g. ops@example.org",
        ));
    }
    let sender = StoredSmtpSender::new(state.oidc.store.clone(), state.oidc.guard.clone());
    let sent = sender
        .send_mail(
            &ctx.realm_id,
            to,
            "Test e-mail from the sign-in service",
            "This is a test e-mail. The sign-in service reached the SMTP server with the saved \
             settings, so invitations and password resets go out the same way.",
            "",
        )
        .await;
    record_admin_write(
        &state,
        &ctx,
        "ACTION",
        "REALM_SMTP_TEST",
        &realm,
        Some(json!({ "sent": sent.is_ok() })),
    )
    .await?;
    Ok(Json(match sent {
        Ok(()) => SmtpTestResult {
            sent: true,
            reason: None,
        },
        Err(e) => {
            tracing::warn!(realm = %ctx.realm_id, error = %e, "kaisatsu: SMTP test e-mail not sent");
            SmtpTestResult {
                sent: false,
                reason: Some(
                    match e {
                        KaisatsuError::OutboundRefused(_) => {
                            "the SMTP host did not resolve, or resolves to a private, loopback or \
                             metadata address, where the sign-in service never sends"
                        }
                        KaisatsuError::BadRequest(_) => {
                            "the saved settings are incomplete: save the host, port, TLS and from \
                             address first"
                        }
                        _ => {
                            "the SMTP server did not take the e-mail: check the host, port, TLS, \
                             user and password"
                        }
                    }
                    .into(),
                ),
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(host: &str) -> SmtpWrite {
        SmtpWrite {
            host: host.into(),
            port: 587,
            tls: SmtpTls::Starttls,
            from: "noreply@example.org".into(),
            user: "mailer".into(),
            password: Some("s3cret".into()),
        }
    }

    #[test]
    fn each_field_is_checked_and_named() {
        for (host, ok) in [
            ("smtp.example.org", true),
            ("SMTP.Example.org.", true),
            ("10.0.0.1", false),
            ("localhost", false),
            ("smtp://x.org", false),
            ("a@b.org", false),
            ("x.org:25", false),
        ] {
            assert_eq!(write(host).into_config(None).is_ok(), ok, "{host}");
        }
        let field = |w: SmtpWrite| match w.into_config(None) {
            Err(KaisatsuError::Validation { fields, .. }) => {
                fields.into_keys().next().unwrap_or_default()
            }
            other => format!("{other:?}"),
        };
        assert_eq!(
            field(SmtpWrite {
                port: 0,
                ..write("a.org")
            }),
            "port"
        );
        assert_eq!(
            field(SmtpWrite {
                from: "nobody".into(),
                ..write("a.org")
            }),
            "from"
        );
        assert_eq!(
            field(SmtpWrite {
                password: None,
                ..write("a.org")
            }),
            "password"
        );
        assert_eq!(
            field(SmtpWrite {
                password: Some("a\nb".into()),
                ..write("a.org")
            }),
            "password"
        );
    }

    #[test]
    fn no_password_keeps_the_stored_one_and_the_view_never_shows_it() {
        let stored = write("smtp.example.org").into_config(None).unwrap();
        let kept = SmtpWrite {
            password: None,
            tls: SmtpTls::Ssl,
            port: 465,
            ..write("smtp.example.org")
        }
        .into_config(Some(&stored))
        .unwrap();
        assert_eq!(kept.password, "s3cret");
        assert_eq!(
            (kept.ssl.as_str(), kept.starttls.as_str()),
            ("true", "false")
        );
        let shown = serde_json::to_string(&view(&kept)).unwrap();
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(shown.contains("\"passwordSet\":true"), "{shown}");
        let open = SmtpWrite {
            user: String::new(),
            password: None,
            ..write("smtp.example.org")
        }
        .into_config(None)
        .unwrap();
        assert_eq!(open.auth, "false");
    }
}
