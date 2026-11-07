use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use jc_core::error::ProblemType;
use serde::Serialize;
use utoipa::ToSchema;

/// RFC 7807 Problem Details representation.
#[derive(Debug, Clone, Serialize, serde::Deserialize, ToSchema, PartialEq, Eq)]
pub struct ProblemDetails {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// What the caller does about it, one English sentence: the catalogue's hint of the slug
    /// (API/00 §4, API/09); the Portal UI shows its own translation by the slug instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Every violation of one request, when there is more than one thing to say. A form marks
    /// all its bad fields from this in one pass instead of one round trip per mistake (CC-24),
    /// and a model corrects its input by `path` (AG-146, T-5949).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<FieldError>>,
}

/// One violation: the member it is about and what is wrong with it (AG-146). `path` is the
/// member's dotted path (`spec.history[1]`), or empty when the finding is about the whole request.
#[derive(Debug, Clone, Serialize, serde::Deserialize, ToSchema, PartialEq, Eq)]
pub struct FieldError {
    pub path: String,
    pub message: String,
}

impl FieldError {
    /// A violation line as the Portal writes it inside: `path: message`, a bare `path` (the
    /// problem's `detail` says what is wrong), or a sentence about the whole request.
    pub fn from_line(line: &str, detail: Option<&str>) -> Self {
        let is_path = |text: &str| {
            !text.is_empty()
                && text.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '_' | '.' | '[' | ']' | '-' | '/' | '$' | '@' | '*')
                })
        };
        match line.split_once(": ") {
            Some((path, message)) if is_path(path) && !message.trim().is_empty() => Self {
                path: path.to_owned(),
                message: message.to_owned(),
            },
            _ if is_path(line) => Self {
                path: line.to_owned(),
                message: detail.unwrap_or("this member is not accepted").to_owned(),
            },
            _ => Self {
                path: String::new(),
                message: line.to_owned(),
            },
        }
    }
}

/// The problem types only the Portal writes (T-4485, API/09); every other `type` it answers with is
/// one of jc-core's `PROBLEM_TYPES`. The test below holds every slug the Portal's source writes to
/// the two lists, and the docs check holds API/09 to them: one section per slug, no section without.
#[rustfmt::skip]
pub const PORTAL_PROBLEM_TYPES: &[ProblemType] = &[
    ProblemType { slug: "invalid-request", status: 400, title: "Invalid Request", hint: "Correct the request as the detail says; errors lists every violation at once." },
    ProblemType { slug: "second-factor-required", status: 403, title: "Second Factor Required", hint: "Sign in again with your one-time code (GET /api/v1/auth/login?step_up=true), then repeat the decision." },
    ProblemType { slug: "self-approval", status: 403, title: "Self Approval Forbidden", hint: "Ask another person who may approve changes in this project to approve it." },
    ProblemType { slug: "service-off", status: 403, title: "Service Off", hint: "Ask an administrator to switch the service on at the layer the answer names." },
    ProblemType { slug: "recipient-refused", status: 403, title: "Recipient Refused", hint: "Send only to people of the organization with a verified address." },
    ProblemType { slug: "draft-conflict", status: 409, title: "Draft Conflict", hint: "Someone saved the draft since you opened it: read current, then make your change on it." },
    ProblemType { slug: "verdict-required", status: 409, title: "Verdict Required", hint: "Run the check the answer names and send the change again with its verdict." },
    ProblemType { slug: "file-too-large", status: 413, title: "File Too Large", hint: "Store a smaller object; the answer names the limit in bytes." },
    ProblemType { slug: "unprocessable", status: 422, title: "Unprocessable Content", hint: "Correct each member errors names; the dry run lists the same findings before you send." },
    ProblemType { slug: "secret-in-argument", status: 422, title: "Secret In Argument", hint: "Enter the credential in the Portal's secret form, never in a chat or a manifest, and name it by secretRef." },
    ProblemType { slug: "quota", status: 429, title: "Quota Used", hint: "Wait until resetAt, delete what the App no longer needs, or ask an administrator to raise the quota." },
    ProblemType { slug: "not-implemented", status: 501, title: "Not Implemented", hint: "This Portal does not do this yet; the detail names what it does instead." },
    ProblemType { slug: "no-json", status: 502, title: "No JSON", hint: "The model answered with something other than JSON of the schema; ask again." },
    ProblemType { slug: "no-model-key", status: 503, title: "No Model Key", hint: "Ask an administrator to set or top up the organization's model key." },
];

/// The hint of a slug, from jc-core's catalogue or the Portal's own; nothing for an unknown one.
pub fn hint_of(slug: &str) -> Option<String> {
    jc_core::error::problem_type(slug)
        .or_else(|| PORTAL_PROBLEM_TYPES.iter().find(|known| known.slug == slug))
        .map(|known| known.hint.to_owned())
}

/// A document a client reads by its own members (`error`, `check`, `reason`), made a problem
/// document by adding the members it lacks (RFC 9457 §3.2 extension members), so every refusal
/// is one of the catalogued types and its old readers still find what they read (T-5949).
pub fn with_problem_members(
    body: serde_json::Value,
    status: StatusCode,
    slug: &str,
) -> serde_json::Value {
    let mut document = match body {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("error".to_owned(), other);
            map
        }
    };
    let known = jc_core::error::problem_type(slug)
        .or_else(|| PORTAL_PROBLEM_TYPES.iter().find(|known| known.slug == slug));
    let defaults = [
        ("type", serde_json::json!(type_uri(slug))),
        (
            "title",
            serde_json::json!(known.map_or("Error", |known| known.title)),
        ),
        ("status", serde_json::json!(status.as_u16())),
        ("hint", serde_json::json!(known.map(|known| known.hint))),
    ];
    for (member, value) in defaults {
        if !value.is_null() {
            document.entry(member).or_insert(value);
        }
    }
    serde_json::Value::Object(document)
}

/// The media type of every problem answer (RFC 9457 §3).
const PROBLEM_JSON: &str = "application/problem+json";

/// The problem answer of [`with_problem_members`].
pub fn problem_response(body: serde_json::Value, status: StatusCode, slug: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, PROBLEM_JSON)],
        axum::Json(with_problem_members(body, status, slug)),
    )
        .into_response()
}

/// Portal API error variants conforming to RFC 7807.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A bad request with one entry per violation, so the caller gets them all at once (CC-24).
    #[error("bad request: {detail}")]
    Invalid { detail: String, errors: Vec<String> },
    /// 422: the manifest parses, and a member of it names what the model or the contract
    /// refuses, one entry per member with its path (API/11 EP-159, the UI DONE rule).
    #[error("unprocessable: {detail}")]
    Unprocessable { detail: String, errors: Vec<String> },
    /// 422: the input holds a value that reads like a credential, at the JSON pointers given and
    /// never echoed; a credential goes in the Portal's secret form (MF-24, T-5971).
    #[error("secret in argument")]
    SecretInArgument(Vec<String>),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    /// 403 with the reason: the verb no role grants, or the constraint the manifest violates (PF-50).
    #[error("forbidden: {0}")]
    Denied(String),
    #[error("self-approval: {0}")]
    SelfApproval(String),
    /// 403 at a decision door: the session proved no second factor (T-5567, AUT-2).
    #[error("second factor required")]
    SecondFactorRequired,
    #[error("conflict: {0}")]
    Conflict(String),
    /// 412: the `If-Match` a write sent is not the manifest's current `ETag`, which the answer
    /// carries so the caller can read again and retry (API/13 section 3).
    #[error("precondition failed: {detail}")]
    PreconditionFailed { detail: String, etag: String },
    /// 409 with the verdict gate's own document (PF-57): `error: verdict_required`, the check to
    /// run and why; the body the operations registry answers, so every door reads alike (T-0956).
    #[error("verdict required")]
    VerdictRequired(serde_json::Value),
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),
    #[error("payload too large: {0}")]
    TooLarge(String),
    #[error("too many requests: {0}")]
    TooManyRequests(String),
    #[error("service unavailable: {0}")]
    Unavailable(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let etag = match &self {
            Self::PreconditionFailed { etag, .. } => Some(etag.clone()),
            _ => None,
        };
        let (status, body) = self.into_document();
        let body = serde_json::to_string(&body).unwrap_or_else(|_| {
            r#"{"type":"https://joinedcontext.com/errors/internal-error","title":"Internal Server Error","status":500}"#.to_string()
        });
        let mut builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, PROBLEM_JSON);
        if let Some(etag) = etag {
            builder = builder.header(header::ETAG, etag);
        }
        builder
            .body(Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
    }
}

impl ApiError {
    /// A failed database call as an answer. No connection in time, a lost connection or a server
    /// restarting is a dependency down, 503 with a next step; the rest is our bug (T-6520). The
    /// caller logs `err`: the DSN, the SQL and the driver's wording stay out of the answer (TS-09).
    pub fn from_db(err: &sqlx::Error, database: &str) -> Self {
        let down = match err {
            sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::WorkerCrashed
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_) => true,
            // Class 57P: shutting down, starting up. 53300: too many connections.
            sqlx::Error::Database(db) => db
                .code()
                .is_some_and(|code| code.starts_with("57P") || code == "53300"),
            _ => false,
        };
        if down {
            Self::Unavailable(format!(
                "the {database} database did not answer; try again in a minute"
            ))
        } else {
            Self::Internal(format!("the {database} database call failed"))
        }
    }

    /// The answer's status and document: the problem a REST door answers and an
    /// MCP tool error carries as its structured content, so both read alike (AG-146, T-5949).
    pub fn into_document(self) -> (StatusCode, serde_json::Value) {
        let this = match self {
            Self::VerdictRequired(body) => {
                let status = StatusCode::CONFLICT;
                return (
                    status,
                    with_problem_members(body, status, "verdict-required"),
                );
            }
            other => other,
        };
        let mut errors = None;
        let (status, slug, title, detail) = match this {
            Self::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "resource-not-found",
                "Resource Not Found",
                Some(msg),
            ),
            Self::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "invalid-request",
                "Invalid Request",
                Some(msg),
            ),
            Self::Invalid {
                detail,
                errors: violations,
            } => {
                errors = Some(violations);
                (
                    StatusCode::BAD_REQUEST,
                    "invalid-request",
                    "Invalid Request",
                    Some(detail),
                )
            }
            Self::Unprocessable {
                detail,
                errors: violations,
            } => {
                errors = Some(violations);
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "unprocessable",
                    "Unprocessable Content",
                    Some(detail),
                )
            }
            Self::SecretInArgument(paths) => {
                errors = Some(
                    paths
                        .iter()
                        .map(|path| format!("{path}: reads like a credential"))
                        .collect(),
                );
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "secret-in-argument",
                    "Secret In Argument",
                    Some("a value of this input reads like a credential, so nothing was kept or sent on; enter it in the Portal's secret form (needsSecret of jc_change_plan names the link) and name it by secretRef".into()),
                )
            }
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized",
                None,
            ),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "Forbidden", None),
            Self::Denied(msg) => (StatusCode::FORBIDDEN, "forbidden", "Forbidden", Some(msg)),
            Self::SecondFactorRequired => (
                StatusCode::FORBIDDEN,
                "second-factor-required",
                "Second Factor Required",
                Some("This decision needs a second factor in this session: sign in again with your one-time code, then repeat it.".into()),
            ),
            Self::SelfApproval(msg) => (
                StatusCode::FORBIDDEN,
                "self-approval",
                "Self Approval Forbidden",
                Some(msg),
            ),
            Self::Conflict(msg) => (StatusCode::CONFLICT, "conflict", "Conflict", Some(msg)),
            Self::PreconditionFailed { detail, .. } => (
                StatusCode::PRECONDITION_FAILED,
                "precondition-failed",
                "Precondition Failed",
                Some(detail),
            ),
            // Answered above with its own document; kept here so the match stays exhaustive.
            Self::VerdictRequired(body) => (
                StatusCode::CONFLICT,
                "verdict-required",
                "Verdict Required",
                Some(body.to_string()),
            ),
            Self::UnsupportedMediaType(msg) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported-media-type",
                "Unsupported Media Type",
                Some(msg),
            ),
            Self::TooLarge(msg) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload-too-large",
                "Payload Too Large",
                Some(msg),
            ),
            Self::TooManyRequests(msg) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too-many-requests",
                "Too Many Requests",
                Some(msg),
            ),
            Self::Unavailable(msg) => {
                // A dependency failed (the realm, the forge, the broker): the person sees the
                // detail once, and the operator needs it in the log to find out why (T-6086).
                tracing::warn!(detail = %msg, "answered 503: a dependency is unavailable");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "service-unavailable",
                    "Service Unavailable",
                    Some(msg),
                )
            }
            Self::NotImplemented(msg) => (
                StatusCode::NOT_IMPLEMENTED,
                "not-implemented",
                "Not Implemented",
                Some(msg),
            ),
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal-error",
                    "Internal Server Error",
                    Some("Unexpected internal error".to_string()),
                )
            }
        };

        let errors = errors.map(|lines: Vec<String>| {
            lines
                .iter()
                .map(|line| FieldError::from_line(line, detail.as_deref()))
                .collect()
        });
        let problem = ProblemDetails {
            r#type: type_uri(slug),
            title: title.to_string(),
            status: status.as_u16(),
            detail,
            instance: None,
            hint: hint_of(slug),
            errors,
        };
        let body = serde_json::to_value(&problem).unwrap_or_else(|_| {
            serde_json::json!({"type": "https://joinedcontext.com/errors/internal-error", "title": "Internal Server Error", "status": 500})
        });
        (status, body)
    }
}

/// The `type` of a problem named `slug`, the one place it is spelled (an MCP tool error at the
/// connector door names the same type as the route's answer).
pub fn type_uri(slug: &str) -> String {
    format!("https://joinedcontext.com/errors/{slug}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn a_database_that_is_down_is_503_and_a_broken_call_500() {
        for down in [
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
            sqlx::Error::WorkerCrashed,
            sqlx::Error::Io(std::io::ErrorKind::ConnectionReset.into()),
        ] {
            // Matched, not rendered: a rendered 503 logs, and a_503_logs_its_detail captures that.
            let answer = ApiError::from_db(&down, "preferences");
            assert!(
                matches!(&answer, ApiError::Unavailable(m) if m.contains("try again")),
                "{down}: {answer:?}"
            );
        }
        let (status, doc) =
            ApiError::from_db(&sqlx::Error::RowNotFound, "preferences").into_document();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!doc.to_string().contains("no rows"), "{doc}");
    }

    #[tokio::test]
    async fn status_codes_and_content_type() {
        let cases = [
            (ApiError::NotFound("test".into()), StatusCode::NOT_FOUND),
            (ApiError::BadRequest("test".into()), StatusCode::BAD_REQUEST),
            (ApiError::Unauthorized, StatusCode::UNAUTHORIZED),
            (ApiError::Forbidden, StatusCode::FORBIDDEN),
            (ApiError::SelfApproval("test".into()), StatusCode::FORBIDDEN),
            (ApiError::SecondFactorRequired, StatusCode::FORBIDDEN),
            (ApiError::Conflict("test".into()), StatusCode::CONFLICT),
            (
                ApiError::PreconditionFailed {
                    detail: "test".into(),
                    etag: "\"e\"".into(),
                },
                StatusCode::PRECONDITION_FAILED,
            ),
            (
                ApiError::TooManyRequests("test".into()),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                ApiError::UnsupportedMediaType("test".into()),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                ApiError::Unavailable("test".into()),
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                ApiError::NotImplemented("test".into()),
                StatusCode::NOT_IMPLEMENTED,
            ),
            (
                ApiError::Internal("test".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ];

        for (err, expected_status) in cases {
            let res = err.into_response();
            assert_eq!(res.status(), expected_status);
            assert_eq!(
                res.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/problem+json"
            );
        }
    }

    #[tokio::test]
    async fn every_problem_carries_its_catalogued_hint() {
        for (err, slug) in [
            (ApiError::SelfApproval("mine".into()), "self-approval"),
            (ApiError::SecondFactorRequired, "second-factor-required"),
            (ApiError::Forbidden, "forbidden"),
        ] {
            let body = err
                .into_response()
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes();
            let problem: ProblemDetails = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                problem.r#type,
                format!("https://joinedcontext.com/errors/{slug}")
            );
            assert_eq!(problem.hint, hint_of(slug));
            assert!(problem.hint.is_some_and(|hint| !hint.is_empty()), "{slug}");
        }
        assert_eq!(hint_of("no-such-slug"), None);
    }

    #[test]
    fn every_violation_line_becomes_a_path_and_a_message() {
        let detail = Some("the request names 2 problems");
        assert_eq!(
            FieldError::from_line("spec.history[1]: names no space", detail),
            FieldError {
                path: "spec.history[1]".into(),
                message: "names no space".into()
            }
        );
        assert_eq!(
            FieldError::from_line("subject", detail),
            FieldError {
                path: "subject".into(),
                message: "the request names 2 problems".into()
            }
        );
        assert_eq!(
            FieldError::from_line("slot 'temperature' has no range", detail),
            FieldError {
                path: String::new(),
                message: "slot 'temperature' has no range".into()
            }
        );
        assert_eq!(FieldError::from_line("ids:", None).path, "");
    }

    #[tokio::test]
    async fn an_invalid_request_answers_every_field_with_its_path() {
        let (status, body) = ApiError::Invalid {
            detail: "2 members are wrong".into(),
            errors: vec!["spec.name: too long".into(), "spec.space".into()],
        }
        .into_document();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body["errors"],
            serde_json::json!([
                {"path": "spec.name", "message": "too long"},
                {"path": "spec.space", "message": "2 members are wrong"}
            ])
        );
    }

    #[test]
    fn a_document_of_its_own_becomes_a_problem_and_keeps_its_members() {
        let verdict = serde_json::json!({ "error": "verdict_required", "check": "jc_manifest_dry_run", "detail": "check it" });
        let (status, body) = ApiError::VerdictRequired(verdict).into_document();
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["type"],
            "https://joinedcontext.com/errors/verdict-required"
        );
        assert_eq!(body["status"], 409);
        assert_eq!(body["error"], "verdict_required");
        assert_eq!(body["check"], "jc_manifest_dry_run");
        assert_eq!(body["detail"], "check it", "its own detail stays");
        let failed = with_problem_members(
            serde_json::json!({ "error": { "message": "boom" } }),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal-error",
        );
        assert_eq!(failed["title"], "Internal Server Error");
        assert_eq!(failed["error"]["message"], "boom");
    }

    #[tokio::test]
    async fn internal_error_hides_message() {
        let secret_msg = "database password leaked";
        let res = ApiError::Internal(secret_msg.to_string()).into_response();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
        let problem: ProblemDetails = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            problem.detail,
            Some("Unexpected internal error".to_string())
        );
        assert!(!String::from_utf8_lossy(&body_bytes).contains(secret_msg));
        assert_eq!(
            problem.r#type,
            "https://joinedcontext.com/errors/internal-error"
        );
        assert_eq!(problem.title, "Internal Server Error");
    }

    /// Every slug the Portal writes is catalogued (T-4485): the slug that follows a status in the
    /// source (`ApiError`'s table, `services::problem`, the basemap's problems) and every type
    /// written whole. A slug outside both lists would be a `type` with no page in API/09.
    #[test]
    fn every_slug_the_portal_writes_is_catalogued() {
        let known = |slug: &str| {
            jc_core::error::problem_type(slug).is_some()
                || PORTAL_PROBLEM_TYPES.iter().any(|known| known.slug == slug)
        };
        let after_status = regex::Regex::new(r#"StatusCode::[A-Z_]+,\s*"([a-z][a-z-]*)""#).unwrap();
        let whole =
            regex::Regex::new(r"https://joinedcontext\.com/(?:errors|problems)/([a-z][a-z-]*)")
                .unwrap();
        let mut seen = std::collections::BTreeSet::new();
        let mut stack = vec![std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src"
        ))];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    // a module's tests make up slugs to prove a refusal; they are not written
                    let text = text.split("#[cfg(test)]").next().unwrap_or_default();
                    for found in after_status
                        .captures_iter(text)
                        .chain(whole.captures_iter(text))
                    {
                        seen.insert((found[1].to_owned(), path.display().to_string()));
                    }
                }
            }
        }
        assert!(
            seen.iter().any(|(slug, _)| slug == "self-approval"),
            "the scan found nothing"
        );
        let unknown: Vec<_> = seen.iter().filter(|(slug, _)| !known(slug)).collect();
        assert!(
            unknown.is_empty(),
            "no PORTAL_PROBLEM_TYPES entry for {unknown:?}"
        );
        for portal in PORTAL_PROBLEM_TYPES {
            assert!(
                jc_core::error::problem_type(portal.slug).is_none(),
                "{} is jc-core's",
                portal.slug
            );
            assert!(
                seen.iter().any(|(slug, _)| slug == portal.slug),
                "nothing writes {}",
                portal.slug
            );
        }
    }

    #[derive(Clone, Default)]
    struct Buffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// T-6086: a 503 names its cause in the log, not only in the answer: the walker met three
    /// realm 503s on dev and the Portal had logged nothing an operator could read.
    #[test]
    fn a_503_logs_its_detail() {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        tracing::subscriber::with_default(
            crate::telemetry::log_subscriber(
                move || writer.clone(),
                tracing_subscriber::EnvFilter::new("warn"),
            ),
            || {
                ApiError::Unavailable("the realm could not be reached: timed out".into())
                    .into_document()
            },
        );
        let written = String::from_utf8(buffer.0.lock().expect("buffer").clone()).expect("utf-8");
        let line: serde_json::Value = serde_json::from_str(written.trim()).expect(&written);
        assert_eq!(line["level"], "WARN");
        assert_eq!(line["detail"], "the realm could not be reached: timed out");
    }
}
