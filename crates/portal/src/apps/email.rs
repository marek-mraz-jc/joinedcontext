//! `POST /apps/{name}/api/services/email/send`: an App's message to people of the organization,
//! through the relay the deployment names, and the signed link that stops it (AP-168, AP-165,
//! API/06 §3, T-3583).
//!
//! The App holds no address, relay or password (AP-147): it names people by id, or `me`, and the
//! Portal looks their verified address up in the realm, so the route relays to nobody outside the
//! organization. Each recipient gets a message of their own with a one-click unsubscribe
//! (RFC 8058) signed with the Portal's cookie key. Logs carry the App, the project, the message id
//! and a count, never an address, a subject or the relay URL.

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use hmac::{Hmac, Mac};
use jc_core::kinds::AppService;
use lettre::message::header::{ContentType, HeaderName, HeaderValue as MailHeader};
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use sha2::Sha256;

use super::services::{self, Quota};
use crate::error::ApiError;
use crate::state::AppState;

/// The largest body the route takes: a text and an HTML part at their limits, with room.
pub const MAX_BODY_BYTES: usize = 512 * 1024;
const MAX_SUBJECT_CHARS: usize = 200;
const MAX_TEXT_BYTES: usize = 100 * 1024;
const MAX_HTML_BYTES: usize = 200 * 1024;
const MAX_RECIPIENTS: usize = 50;
/// Messages one person receives from one App a day, whatever the App's own quota.
pub const PER_RECIPIENT_PER_DAY: u32 = 20;
const RELAY_TIMEOUT: Duration = Duration::from_secs(20);

/// `to` is a list of person ids or the literal `"me"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum To {
    People(Vec<String>),
    Me(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Send {
    to: To,
    subject: String,
    text: String,
    #[serde(default)]
    html: Option<String>,
}

/// One recipient: the realm's id and their verified address.
struct Recipient {
    id: String,
    address: Address,
}

fn bad(detail: impl Into<String>) -> Response {
    ApiError::BadRequest(detail.into()).into_response()
}

/// The refusal of what a header could be smuggled through, of what is too long to be a subject,
/// and of a recipient that is no person id; `None` when the message may go on.
fn refusal(send: &Send) -> Option<Response> {
    let subject = send.subject.trim();
    let too_large = |part: &str, max: usize| format!("{part} is larger than {max} bytes");
    let problem = if subject.is_empty() {
        "subject is empty".to_owned()
    } else if send.subject.chars().any(char::is_control) {
        "subject holds a line break or another control character, which no header may carry"
            .to_owned()
    } else if subject.chars().count() > MAX_SUBJECT_CHARS {
        format!("subject is longer than {MAX_SUBJECT_CHARS} characters")
    } else if send.text.trim().is_empty() {
        "text is empty".to_owned()
    } else if send.text.len() > MAX_TEXT_BYTES {
        too_large("text", MAX_TEXT_BYTES)
    } else if send
        .html
        .as_ref()
        .is_some_and(|html| html.len() > MAX_HTML_BYTES)
    {
        too_large("html", MAX_HTML_BYTES)
    } else {
        match &send.to {
            To::Me(me) if me == "me" => return None,
            To::Me(_) => "to is a list of person ids or \"me\"".to_owned(),
            To::People(ids) if ids.is_empty() => "to names nobody".to_owned(),
            To::People(ids) if ids.len() > MAX_RECIPIENTS => {
                format!("to names more than {MAX_RECIPIENTS} people")
            }
            To::People(ids) => {
                return ids
                    .iter()
                    .find(|id| !is_person_id(id))
                    .map(|id| services::recipient_refused(id))
            }
        }
    };
    Some(bad(problem))
}

/// A realm id: letters, digits and dashes, never an address.
fn is_person_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(super) async fn send(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let (project, spec, person) = match services::gate(
        &state,
        &headers,
        &uri,
        &name,
        (AppService::Email, &Method::POST),
    )
    .await
    {
        Ok(gated) => gated,
        Err(refused) => return *refused,
    };
    // A cross-site form can post `text/plain` without a preflight, never `application/json`.
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().starts_with("application/json"));
    if !json {
        return ApiError::UnsupportedMediaType("send the message as application/json".into())
            .into_response();
    }
    let send: Send = match serde_json::from_slice(&body) {
        Ok(send) => send,
        Err(err) => return bad(format!("the body is not a message: {err}")),
    };
    if let Some(refused) = refusal(&send) {
        return refused;
    }
    let (Some(relay), Some(from), Some(db)) = (
        state.config.smtp_url.as_deref(),
        state.config.mail_from.clone(),
        state.db.as_ref(),
    ) else {
        return ApiError::Unavailable("this installation sends no mail".into()).into_response();
    };

    let recipients = match recipients(&state, &project, &send.to, &person.identity).await {
        Ok(recipients) => recipients,
        Err(refused) => return *refused,
    };
    let recipients = match not_unsubscribed(db, &project, &name, recipients).await {
        Ok(recipients) => recipients,
        Err(err) => return ApiError::Internal(err.to_string()).into_response(),
    };
    let limit = services::quota(&state.mirror, &project, &spec, Quota::EmailsPerDay);
    match counted(db, (&project, &name), &recipients, limit).await {
        Ok(None) => {}
        Ok(Some(quota)) => {
            return services::quota_used(
                AppService::Email,
                quota,
                Some(time::OffsetDateTime::now_utc()),
            )
        }
        Err(err) => return ApiError::Internal(err.to_string()).into_response(),
    }

    let id = crate::auth::csrf::new_token();
    let sender = Mailbox::new(Some(display_name(&state, &project, &name)), from);
    let transport = match AsyncSmtpTransport::<Tokio1Executor>::from_url(relay) {
        Ok(builder) => builder.timeout(Some(RELAY_TIMEOUT)).build(),
        Err(_) => {
            // The URL holds the relay's password: its parse error is not logged.
            tracing::error!("JC_PORTAL_SMTP_URL is not a relay URL");
            return ApiError::Unavailable("the mail relay is misconfigured".into()).into_response();
        }
    };
    for recipient in &recipients {
        let link = unsubscribe_link(&state, &project, &name, &recipient.id);
        let message = match message(&sender, recipient, &send, &link, &id) {
            Ok(message) => message,
            Err(err) => return bad(format!("the message cannot be built: {err}")),
        };
        if let Err(err) = transport.send(message).await {
            // The relay's reply may quote the address: only its code is logged.
            let code = err.status().map(|code| code.to_string());
            tracing::warn!(app = %name, %project, %id, code = ?code, "the mail relay refused a message");
            return ApiError::Unavailable("the mail relay did not take the message".into())
                .into_response();
        }
    }
    tracing::info!(app = %name, %project, %id, recipients = recipients.len(), "app mail sent");
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "id": id }))).into_response()
}

/// The App's title, else its name; never a control character, so a title cannot add a header.
fn display_name(state: &AppState, project: &str, name: &str) -> String {
    let title = state
        .mirror
        .get(project, "App", name)
        .and_then(|envelope| envelope.metadata.title)
        .map(|title| title.resolve(&[], "en").to_owned())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| name.to_owned());
    title.chars().filter(|c| !c.is_control()).collect()
}

/// The people `to` names with their verified addresses, each once. `403 …/recipient-refused`
/// for an id the realm does not hold, holds disabled or without a verified address, or, with
/// several Organizations, outside the group subtree of the one that owns `project` (AP-168).
async fn recipients(
    state: &AppState,
    project: &str,
    to: &To,
    me: &crate::auth::session::Identity,
) -> Result<Vec<Recipient>, Box<Response>> {
    let ids: BTreeSet<&str> = match to {
        To::Me(_) => {
            let address = me
                .email
                .as_deref()
                .and_then(|email| email.parse::<Address>().ok())
                .ok_or_else(|| Box::new(services::recipient_refused("me")))?;
            return Ok(vec![Recipient {
                id: me.subject.clone(),
                address,
            }]);
        }
        To::People(ids) => ids.iter().map(String::as_str).collect(),
    };
    let unavailable = |reason: &str| {
        Box::new(ApiError::Unavailable(format!("the realm: {reason}")).into_response())
    };
    let people = state
        .people
        .as_ref()
        .ok_or_else(|| unavailable("no admin client"))?;
    let admin = people
        .admin()
        .await
        .map_err(|_| unavailable("unreachable"))?;
    // with one Organization the whole realm is its people; with several, only its subtree,
    // and a project no Organization owns mails nobody
    let organization = if crate::permissions::several_organizations(&state.mirror) {
        Some(
            crate::permissions::organization_of(&state.mirror, project)
                .ok_or_else(|| Box::new(services::recipient_refused("organization")))?,
        )
    } else {
        None
    };
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        let user = match admin.get(id).await {
            Ok(user) => user,
            Err(crate::people::PeopleError::Unreachable(_)) => {
                return Err(unavailable("unreachable"))
            }
            Err(_) => return Err(Box::new(services::recipient_refused(id))),
        };
        let address = user
            .email
            .as_deref()
            .filter(|_| user.enabled && user.email_verified)
            .and_then(|email| email.parse::<Address>().ok())
            .ok_or_else(|| Box::new(services::recipient_refused(id)))?;
        if let Some(organization) = &organization {
            let paths = match admin.group_paths(&user.id).await {
                Ok(paths) => paths,
                Err(crate::people::PeopleError::Unreachable(_)) => {
                    return Err(unavailable("unreachable"))
                }
                Err(_) => return Err(Box::new(services::recipient_refused(id))),
            };
            let root = format!("/{organization}");
            let inside = paths.iter().any(|path| {
                path.strip_prefix(&root)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            });
            if !inside {
                return Err(Box::new(services::recipient_refused(id)));
            }
        }
        found.push(Recipient {
            id: user.id,
            address,
        });
    }
    Ok(found)
}

/// `recipients` without those who stopped this App's mail.
async fn not_unsubscribed(
    db: &sqlx::PgPool,
    project: &str,
    app: &str,
    recipients: Vec<Recipient>,
) -> Result<Vec<Recipient>, sqlx::Error> {
    let ids: Vec<String> = recipients.iter().map(|r| r.id.clone()).collect();
    let stopped: Vec<String> = sqlx::query_scalar(
        "SELECT subject FROM email_unsubscribes WHERE project = $1 AND app = $2 AND subject = ANY($3)",
    )
    .bind(project)
    .bind(app)
    .bind(&ids)
    .fetch_all(db)
    .await?;
    Ok(recipients
        .into_iter()
        .filter(|r| !stopped.contains(&r.id))
        .collect())
}

/// Counts one message per recipient against each recipient's day and the App's, all or nothing;
/// the quota that ran out when one did.
async fn counted(
    db: &sqlx::PgPool,
    app: (&str, &str),
    recipients: &[Recipient],
    limit: u32,
) -> Result<Option<&'static str>, sqlx::Error> {
    let mut tx = db.begin().await?;
    for recipient in recipients {
        if !services::take(
            &mut *tx,
            app,
            "email",
            &recipient.id,
            1,
            PER_RECIPIENT_PER_DAY,
        )
        .await?
        {
            return Ok(Some("emailsPerRecipientPerDay"));
        }
    }
    let count = u32::try_from(recipients.len()).unwrap_or(u32::MAX);
    if count > 0 && !services::take(&mut *tx, app, "email", "", count, limit).await? {
        return Ok(Some("emailsPerDay"));
    }
    tx.commit().await?;
    Ok(None)
}

fn message(
    sender: &Mailbox,
    recipient: &Recipient,
    send: &Send,
    link: &str,
    id: &str,
) -> Result<Message, lettre::error::Error> {
    let footer = format!("\n\n--\nTo stop these messages: {link}\n");
    let text = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(format!("{}{footer}", send.text));
    let builder = Message::builder()
        .from(sender.clone())
        .to(Mailbox::new(None, recipient.address.clone()))
        .subject(send.subject.trim())
        .message_id(Some(format!("<{id}.{}@joinedcontext>", recipient.id)));
    let mut message = match &send.html {
        Some(html) => builder.multipart(
            MultiPart::alternative().singlepart(text).singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(format!(
                        "{html}<p><a href=\"{}\">To stop these messages</a></p>",
                        link.replace('&', "&amp;").replace('"', "&quot;")
                    )),
            ),
        )?,
        None => builder.singlepart(text)?,
    };
    let headers = message.headers_mut();
    headers.insert_raw(MailHeader::new(
        HeaderName::new_from_ascii_str("List-Unsubscribe"),
        format!("<{link}>"),
    ));
    headers.insert_raw(MailHeader::new(
        HeaderName::new_from_ascii_str("List-Unsubscribe-Post"),
        "List-Unsubscribe=One-Click".to_owned(),
    ));
    Ok(message)
}

// --- the unsubscribe link -----------------------------------------------------------------------

/// The signature of one person's link for one App: only this Portal's cookie key makes it.
/// `None` never happens, since HMAC takes a key of any length.
fn signature(state: &AppState, project: &str, app: &str, subject: &str) -> Option<Hmac<Sha256>> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(state.config.cookie_key.signing()).ok()?;
    for part in ["jc-unsubscribe", project, app, subject] {
        mac.update(part.as_bytes());
        mac.update(&[0]);
    }
    Some(mac)
}

const KEY: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn unsubscribe_link(state: &AppState, project: &str, app: &str, subject: &str) -> String {
    let key = signature(state, project, app, subject)
        .map(|mac| KEY.encode(mac.finalize().into_bytes()))
        .unwrap_or_default();
    let mut url = state.config.public_base_url.clone();
    url.set_path("/mail/unsubscribe");
    url.query_pairs_mut()
        .clear()
        .append_pair("p", project)
        .append_pair("a", app)
        .append_pair("s", subject)
        .append_pair("k", &key);
    url.to_string()
}

/// The link's query; a member left out is empty, and an empty one verifies nothing.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Unsubscribe {
    p: String,
    a: String,
    s: String,
    k: String,
}

impl Unsubscribe {
    fn verified(&self, state: &AppState) -> bool {
        crate::resource::is_dns1123(&self.p)
            && crate::resource::is_dns1123(&self.a)
            && is_person_id(&self.s)
            && KEY.decode(&self.k).is_ok_and(|key| {
                signature(state, &self.p, &self.a, &self.s)
                    .is_some_and(|mac| mac.verify_slice(&key).is_ok())
            })
    }
}

fn page(status: StatusCode, body: String) -> Response {
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        format!("<!doctype html><meta charset=utf-8><title>Mail</title><main>{body}</main>"),
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; form-action 'self'; frame-ancestors 'none'"),
    );
    response
}

/// `GET /mail/unsubscribe`: asks before it stops anything, so a scanner that opens every link
/// in a message unsubscribes nobody.
pub async fn unsubscribe_page(
    State(state): State<AppState>,
    Query(link): Query<Unsubscribe>,
) -> Response {
    if !link.verified(&state) {
        return page(
            StatusCode::NOT_FOUND,
            "<p>This link is not valid.</p>".into(),
        );
    }
    page(
        StatusCode::OK,
        format!(
            "<form method=post><p>Stop mail from the App {}?</p><button>Stop</button></form>",
            link.a
        ),
    )
}

/// `POST /mail/unsubscribe`: the page's button and the mail client's one-click (RFC 8058).
pub async fn unsubscribe(
    State(state): State<AppState>,
    Query(link): Query<Unsubscribe>,
) -> Response {
    if !link.verified(&state) {
        return page(
            StatusCode::NOT_FOUND,
            "<p>This link is not valid.</p>".into(),
        );
    }
    let Some(db) = state.db.as_ref() else {
        return ApiError::Unavailable("no database".into()).into_response();
    };
    let stored = sqlx::query(
        "INSERT INTO email_unsubscribes (project, app, subject) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&link.p)
    .bind(&link.a)
    .bind(&link.s)
    .execute(db)
    .await;
    if let Err(err) = stored {
        return ApiError::Internal(err.to_string()).into_response();
    }
    tracing::info!(app = %link.a, project = %link.p, "a person stopped an app's mail");
    page(
        StatusCode::OK,
        format!("<p>You will get no more mail from the App {}.</p>", link.a),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(to: serde_json::Value, subject: &str) -> Send {
        serde_json::from_value(serde_json::json!({ "to": to, "subject": subject, "text": "t" }))
            .expect("a message")
    }

    #[test]
    fn a_line_break_in_the_subject_or_an_address_as_recipient_is_refused() {
        assert!(refusal(&send(serde_json::json!("me"), "Road defect")).is_none());
        assert!(refusal(&send(serde_json::json!(["4f1c-9a"]), "ok")).is_none());
        for subject in ["x\r\nBcc: all@example.org", "x\nBcc: a@b", "x\u{0}", " "] {
            assert!(
                refusal(&send(serde_json::json!("me"), subject)).is_some(),
                "{subject:?}"
            );
        }
        for to in [
            serde_json::json!(["someone@example.org"]),
            serde_json::json!([]),
            serde_json::json!("everyone"),
        ] {
            assert!(refusal(&send(to.clone(), "ok")).is_some(), "{to}");
        }
    }
}
