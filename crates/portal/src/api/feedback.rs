//! Feedback from any page (T-3272, API/01 §38): a signed-in person's words, scrubbed of personal
//! data and credentials, kept without an author; the administrators' list the board's proposed
//! tasks are made from.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::feedback::{Feedback, NewFeedback};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// The longest text a feedback carries, in characters once trimmed.
pub const MAX_TEXT: usize = 2_000;
/// The longest page address kept.
pub const MAX_PAGE: usize = 500;
/// The largest screenshot, decoded.
pub const MAX_SCREENSHOT: usize = 2 * 1024 * 1024;
/// How many a person sends in an hour.
pub const PER_HOUR: usize = 10;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackBody {
    pub text: String,
    pub page: String,
    /// A PNG `data:` URL, only when the person ticked it.
    #[serde(default)]
    pub screenshot: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackAccepted {
    pub id: i64,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct FeedbackPage {
    /// The id the previous page ended at; `0` for the first.
    pub after: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackList {
    pub items: Vec<Feedback>,
}

/// The text with every e-mail address, phone number and credential-shaped word replaced, so the
/// board's task carries no personal data and no secret pasted by mistake (API/01 §38).
pub fn scrub(text: &str) -> String {
    static EMAIL: OnceLock<Option<regex::Regex>> = OnceLock::new();
    static PHONE: OnceLock<Option<regex::Regex>> = OnceLock::new();
    let email = EMAIL
        .get_or_init(|| regex::Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").ok());
    // Nine digits or more, with the spaces, dashes, dots and parentheses a number is written with.
    let phone = PHONE.get_or_init(|| regex::Regex::new(r"\+?\d[\d ().-]{7,}\d").ok());
    // Both patterns are literals the tests compile; were one ever refused, nothing is kept rather
    // than a text that may still hold what it should not.
    let (Some(email), Some(phone)) = (email, phone) else {
        return "[removed]".to_owned();
    };
    let text = email.replace_all(text, "[removed]");
    let text = phone.replace_all(&text, |found: &regex::Captures<'_>| {
        let digits = found[0].chars().filter(char::is_ascii_digit).count();
        if digits >= 9 {
            "[removed]".to_owned()
        } else {
            found[0].to_owned()
        }
    });
    text.split(' ')
        .map(|word| {
            if crate::pipeline_outcomes::credential_shaped(word) {
                "[removed]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The page without its query and fragment, which can carry a name or a filter value, and as a
/// path alone: a part holding anything but `A-Z a-z 0-9 / _ . ~ % -`, or an address, is kept as
/// `_`, so the page is one line wherever it is written (the board's task file, T-3272).
fn page_of(page: &str) -> Result<String, ApiError> {
    let page = page.split(['?', '#']).next().unwrap_or_default().trim();
    if !page.starts_with('/') || page.starts_with("//") || page.len() > MAX_PAGE {
        return Err(ApiError::Invalid {
            detail: "page must be the Portal address the person was on, starting with /".into(),
            errors: vec!["page".into()],
        });
    }
    let plain = |part: &str| {
        part.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.~%-".contains(&b))
            && !part.to_ascii_lowercase().contains("%40")
    };
    let parts: Vec<&str> = page
        .split('/')
        .map(|part| if plain(part) { part } else { "_" })
        .collect();
    Ok(parts.join("/"))
}

fn screenshot_of(data: &str) -> Result<Vec<u8>, ApiError> {
    let invalid = |why: &str| ApiError::Invalid {
        detail: format!("screenshot {why}"),
        errors: vec!["screenshot".into()],
    };
    let encoded = data
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| invalid("must be a PNG data: URL"))?;
    if encoded.len() > MAX_SCREENSHOT.div_ceil(3) * 4 {
        return Err(invalid("is larger than 2 MB"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| invalid("is not base64"))?;
    if !bytes.starts_with(PNG) {
        return Err(invalid("is not a PNG"));
    }
    Ok(bytes)
}

/// Whether `subject` may send one more now; the seconds to wait when not.
// ponytail: one window per replica, in memory; a shared counter if the Portal runs several.
fn admit(subject: &str, now: Instant) -> Result<(), u64> {
    static SENT: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    let hour = Duration::from_secs(3600);
    let mut sent = SENT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let times = crate::api::knowledge::within_span(&mut sent, subject, now, hour);
    if times.len() >= PER_HOUR {
        let oldest = times.first().copied().unwrap_or(now);
        return Err((hour - now.duration_since(oldest)).as_secs().max(1));
    }
    times.push(now);
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/feedback",
    summary = "Send Feedback",
    description = "A signed-in person's feedback from a page: scrubbed of e-mail addresses, phone numbers and credentials, kept without an author.",
    tag = "feedback",
    request_body(
        content = FeedbackBody,
        content_type = "application/json",
        example = json!({ "text": "The Approve button stays grey after I type the name", "page": "/projects/helsinki/approvals" })
    ),
    responses(
        (status = 202, description = "Kept", body = FeedbackAccepted),
        (status = 400, description = "An unknown member, or a field out of bounds", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 429, description = "Ten in the last hour already", body = ProblemDetails),
    )
)]
pub async fn send_feedback(
    user: CurrentUser,
    State(state): State<AppState>,
    body: Result<Json<FeedbackBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let text = body.text.trim();
    if text.is_empty() || text.chars().count() > MAX_TEXT {
        return Err(ApiError::Invalid {
            detail: format!("text must be 1 to {MAX_TEXT} characters"),
            errors: vec!["text".into()],
        });
    }
    let page = page_of(&body.page)?;
    let screenshot = body.screenshot.as_deref().map(screenshot_of).transpose()?;
    if let Err(wait) = admit(&user.0.identity.subject, Instant::now()) {
        let mut refused = ApiError::TooManyRequests(format!(
            "{PER_HOUR} feedbacks in an hour already; send this one in {} minutes",
            wait.div_ceil(60)
        ))
        .into_response();
        if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
            refused.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return Ok(refused);
    }
    let id = state
        .feedback
        .add(NewFeedback {
            page: &page,
            version: crate::APP_VERSION,
            text: &scrub(text),
            screenshot: screenshot.as_deref(),
        })
        .await
        .map_err(|err| ApiError::Internal(format!("the feedback was not kept: {err}")))?;
    Ok((StatusCode::ACCEPTED, Json(FeedbackAccepted { id })).into_response())
}

fn administrator(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    if crate::permissions::for_request(state, identity, ORG_NAMESPACE).administers_organization() {
        Ok(())
    } else {
        Err(ApiError::Denied(
            "reading feedback needs an administrator of the organization (PF-03)".into(),
        ))
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/feedback",
    summary = "List Feedback",
    description = "The feedback after an id, oldest first, at most 100: what the board's proposed tasks are made from.",
    tag = "feedback",
    params(FeedbackPage),
    responses(
        (status = 200, description = "One page", body = FeedbackList),
        (status = 400, description = "An unknown parameter", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
    )
)]
pub async fn list_feedback(
    user: CurrentUser,
    State(state): State<AppState>,
    query: Result<Query<FeedbackPage>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<FeedbackList>, ApiError> {
    administrator(&state, &user.0.identity)?;
    let Query(query) = query.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let items = state
        .feedback
        .after(query.after.unwrap_or(0))
        .await
        .map_err(|err| ApiError::Internal(format!("the feedback could not be read: {err}")))?;
    Ok(Json(FeedbackList { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/feedback/{id}/screenshot",
    summary = "Read A Feedback's Screenshot",
    description = "The PNG sent with one feedback; 404 when it has none.",
    tag = "feedback",
    params(("id" = i64, Path, description = "The feedback's id")),
    responses(
        (status = 200, description = "The screenshot", content_type = "image/png"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
        (status = 404, description = "No such feedback, or none sent with it", body = ProblemDetails),
    )
)]
pub async fn feedback_screenshot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    administrator(&state, &user.0.identity)?;
    let png = state
        .feedback
        .screenshot(id)
        .await
        .map_err(|err| ApiError::Internal(format!("the screenshot could not be read: {err}")))?
        .ok_or_else(|| ApiError::NotFound(format!("feedback {id} has no screenshot")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            // A screenshot of a person's page is nobody's to keep in a shared cache.
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        png,
    )
        .into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/feedback", post(send_feedback))
        .route("/organization/feedback", get(list_feedback))
        .route(
            "/organization/feedback/{id}/screenshot",
            get(feedback_screenshot),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_data_and_credentials_leave_the_text() {
        // The token is put together here so the repository holds no token-shaped literal for
        // a secret scanner to report (trivy, gitleaks).
        let token = format!("{}-{}", "glpat", "0123456789abcdefghij");
        let said = scrub(&format!(
            "Mail jana.kovacova@banskabystrica.sk or call +421 900 123 456; my token is {token} and step 12 of 3000 failed"
        ));
        assert!(!said.contains("jana"), "{said}");
        assert!(!said.contains("900 123"), "{said}");
        assert!(!said.contains("glpat"), "{said}");
        // Short numbers are what a person describes a problem with.
        assert!(said.contains("step 12 of 3000 failed"), "{said}");
    }

    #[test]
    fn a_page_keeps_its_path_alone() {
        assert_eq!(
            page_of("/projects/x/spaces?edit=jana#top").ok(),
            Some("/projects/x/spaces".into())
        );
        for bad in [
            "projects/x",
            "//evil.example/x",
            "https://evil.example/",
            "",
        ] {
            assert!(page_of(bad).is_err(), "{bad}");
        }
        assert!(page_of(&format!("/{}", "a".repeat(MAX_PAGE))).is_err());
    }

    #[test]
    fn a_page_is_a_path_and_nothing_else() {
        // A line break, a quote or a space would end the line it is written on (T-3272): the part
        // of the path that holds one is kept as `_`, and so is a part that holds an address.
        assert_eq!(
            page_of("/x\nstatus: todo\nowner:\n").ok(),
            Some("/_".into())
        );
        assert_eq!(
            page_of("/projects/x/people/jana@hel.fi/edit").ok(),
            Some("/projects/x/people/_/edit".into())
        );
        assert_eq!(
            page_of("/projects/x/people/jana%40hel.fi").ok(),
            Some("/projects/x/people/_".into())
        );
        assert_eq!(
            page_of("/projects/x/spaces/\"air\" quality").ok(),
            Some("/projects/x/spaces/_".into())
        );
        assert_eq!(
            page_of("/projects/air-2_x.v1/~view/%C3%A1").ok(),
            Some("/projects/air-2_x.v1/~view/%C3%A1".into())
        );
    }

    #[test]
    fn a_screenshot_is_a_png_data_url_of_at_most_two_megabytes() {
        let png = base64::engine::general_purpose::STANDARD.encode([PNG, b"rest"].concat());
        assert!(screenshot_of(&format!("data:image/png;base64,{png}")).is_ok());
        let jpeg = base64::engine::general_purpose::STANDARD.encode(b"\xff\xd8\xff rest");
        assert!(screenshot_of(&format!("data:image/png;base64,{jpeg}")).is_err());
        assert!(screenshot_of(&format!("data:image/jpeg;base64,{png}")).is_err());
        assert!(screenshot_of("data:image/png;base64,***").is_err());
        let huge = "A".repeat(MAX_SCREENSHOT.div_ceil(3) * 4 + 4);
        assert!(screenshot_of(&format!("data:image/png;base64,{huge}")).is_err());
    }

    #[test]
    fn the_eleventh_in_an_hour_waits() {
        let start = Instant::now();
        let who = "rate-test-subject";
        for _ in 0..PER_HOUR {
            assert!(admit(who, start).is_ok());
        }
        let wait = admit(who, start).expect_err("the eleventh");
        assert!(wait > 3500 && wait <= 3600, "{wait}");
        assert!(admit(who, start + Duration::from_secs(3601)).is_ok());
        assert!(admit("someone-else", start).is_ok());
    }
}
