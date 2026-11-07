// SPDX-License-Identifier: EUPL-1.2
//! The person's decision on a write, recorded by the deployment's decision
//! page (T-5816): with [`CONFIRM_URL_ENV`] set, a write tool asks by a URL
//! (MCP URL-mode elicitation) instead of a form, and its retry runs only on
//! the decision the page recorded through the admin route
//! `PUT /q/mcp-confirmations/{id}`. A client's own `accept` counts for
//! nothing: an AI client cannot approve its own write.

use crate::confirm::TTL;
use antares_ops::ngsi_problem;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// The decision page the person is sent to, `{id}` standing for the
/// question's id: `https://portal.example.org/mcp-confirmations/{id}`.
pub const CONFIRM_URL_ENV: &str = "ANTARES_MCP_CONFIRM_URL";

/// The placeholder of [`CONFIRM_URL_ENV`].
const ID: &str = "{id}";

/// The most open questions; past it a write is refused until some expire.
const MAX_OPEN: usize = 10_000;

/// The most open questions of one subject, so one caller cannot fill
/// [`MAX_OPEN`].
const MAX_OPEN_PER_SUBJECT: usize = 100;

/// The largest decision body the admin route reads.
const MAX_DECISION_BYTES: usize = 4096;

/// How this process asks the person to confirm a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Asking {
    /// No decision page: the client host asks with a form (MCP form mode).
    Form,
    /// The page at this template records the decision.
    Page(String),
    /// [`CONFIRM_URL_ENV`] is set and unusable: every write is refused.
    Broken,
}

/// This process's [`Asking`], read once.
pub(crate) fn asking() -> &'static Asking {
    static ASKING: LazyLock<Asking> =
        LazyLock::new(|| asking_of(std::env::var(CONFIRM_URL_ENV).ok().as_deref()));
    &ASKING
}

/// The [`Asking`] of the configured `raw` template.
pub(crate) fn asking_of(raw: Option<&str>) -> Asking {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Asking::Form;
    };
    let usable = (raw.starts_with("https://") || raw.starts_with("http://"))
        && raw.matches(ID).count() == 1
        && !raw.contains(char::is_whitespace);
    if usable {
        Asking::Page(raw.to_owned())
    } else {
        let why =
            "mcp decision page is no http(s) URL with one {id}; write tools refuse every call";
        tracing::error!(env = CONFIRM_URL_ENV, "{why}");
        Asking::Broken
    }
}

/// Whether the call of `subject` may be asked now, question `id` about
/// its view, tool and message opened when `asking` is a page: `false` for
/// a broken page, or when the open questions or the subject's share of
/// them are full.
pub(crate) fn opens(
    asking: &Asking,
    id: &str,
    subject: &[(String, Vec<u8>)],
    about: (&str, &str, &str),
) -> bool {
    match asking {
        Asking::Form => true,
        Asking::Page(_) => ask_in(&QUESTIONS, id, subject, about, Instant::now()),
        Asking::Broken => false,
    }
}

/// Whether the retry of a write is confirmed: by the decision a page
/// recorded for `found` (the question its `requestState` seals, `None` when
/// it seals none), else by the client's form answer `form_accepted`; a
/// broken page confirms nothing.
pub(crate) fn confirms(
    asking: &Asking,
    found: Option<Found>,
    form_accepted: bool,
) -> Result<(), &'static str> {
    const DECLINED: &str = "the person did not confirm; nothing was written";
    match asking {
        Asking::Form if form_accepted => Ok(()),
        Asking::Form => Err(DECLINED),
        Asking::Broken => Err(BROKEN),
        Asking::Page(_) => match found {
            Some(Found::Accepted) => Ok(()),
            Some(Found::Waiting) => Err("the person has not decided on the page yet; call again with the same requestState once they have"),
            Some(Found::Declined) => Err(DECLINED),
            _ => Err(STALE),
        },
    }
}

/// The refusal of a write this endpoint cannot ask about.
pub(crate) const BROKEN: &str = "this endpoint cannot ask the person now; nothing was written";

/// The refusal of a `requestState` that answers no open call.
pub(crate) const STALE: &str =
    "the requestState does not answer this call; call the tool again without it";

/// The page of question `id` under `template`.
pub(crate) fn page_of(template: &str, id: &str) -> String {
    template.replacen(ID, id, 1)
}

/// What the person decided on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    Accept,
    Decline,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
        }
    }
}

/// One open question.
struct Question {
    /// The subject headers of the call that asked.
    subject: Vec<(String, Vec<u8>)>,
    view: String,
    tool: String,
    message: String,
    until: Instant,
    decision: Option<Decision>,
}

/// What the retry of a write finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    Accepted,
    Declined,
    /// Open, and the person has not decided yet.
    Waiting,
    /// Expired, answered before, or never asked.
    Unknown,
}

/// Subject headers and their values, as [`antares_ops::Caller::subject`] lists them.
type Subject = Vec<(String, Vec<u8>)>;

/// The open questions, by id.
type Questions = Mutex<HashMap<String, Question>>;

static QUESTIONS: LazyLock<Questions> = LazyLock::new(Questions::default);

fn locked(q: &Questions) -> std::sync::MutexGuard<'_, HashMap<String, Question>> {
    // a holder that panicked left the map whole: each change is one step
    q.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn ask_in(
    questions: &Questions,
    id: &str,
    subject: &[(String, Vec<u8>)],
    (view, tool, message): (&str, &str, &str),
    now: Instant,
) -> bool {
    let mut map = locked(questions);
    map.retain(|_, q| q.until > now);
    let held = map.values().filter(|q| q.subject == subject).count();
    if map.len() >= MAX_OPEN || held >= MAX_OPEN_PER_SUBJECT {
        return false;
    }
    map.insert(
        id.to_owned(),
        Question {
            subject: subject.to_vec(),
            view: view.to_owned(),
            tool: tool.to_owned(),
            message: message.to_owned(),
            until: now.checked_add(TTL).unwrap_or(now),
            decision: None,
        },
    );
    true
}

/// What the retry of `subject` finds for question `id`. A decision ends
/// the question; a waiting one stays open for the next retry.
pub(crate) fn take(id: &str, subject: &[(String, Vec<u8>)]) -> Found {
    take_in(&QUESTIONS, id, subject, Instant::now())
}

fn take_in(questions: &Questions, id: &str, subject: &[(String, Vec<u8>)], now: Instant) -> Found {
    let mut map = locked(questions);
    let Some(q) = map
        .get(id)
        .filter(|q| q.until > now && q.subject == subject)
    else {
        return Found::Unknown;
    };
    let found = match q.decision {
        None => return Found::Waiting,
        Some(Decision::Accept) => Found::Accepted,
        Some(Decision::Decline) => Found::Declined,
    };
    map.remove(id);
    found
}

/// The admin routes of the decision page, relative to `/q`:
/// `GET /mcp-confirmations/{id}` reads a question, `PUT` records the
/// person's decision on it.
pub fn routes() -> Router {
    Router::new().route(
        "/mcp-confirmations/{id}",
        get(|Path(id): Path<String>| async move { read(&QUESTIONS, &id, Instant::now()) })
            .put(
                |Path(id): Path<String>, body: axum::body::Bytes| async move {
                    decide(&QUESTIONS, &id, &body, Instant::now())
                },
            )
            .layer(axum::extract::DefaultBodyLimit::max(MAX_DECISION_BYTES)),
    )
}

/// The question's JSON: what the page shows the person, and the subject
/// headers it checks against the person signed in.
fn shown(id: &str, q: &Question, now: Instant) -> Value {
    let mut subject: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::default();
    for (name, value) in &q.subject {
        subject
            .entry(name)
            .or_default()
            .push(String::from_utf8_lossy(value).into_owned());
    }
    json!({
        "id": id,
        "view": q.view,
        "tool": q.tool,
        "message": q.message,
        "subject": subject,
        "expiresInSeconds": q.until.saturating_duration_since(now).as_secs(),
        "decision": q.decision.map(Decision::as_str),
    })
}

fn read(questions: &Questions, id: &str, now: Instant) -> Response {
    let map = locked(questions);
    match map.get(id).filter(|q| q.until > now) {
        Some(q) => axum::Json(shown(id, q, now)).into_response(),
        None => ngsi_problem(StatusCode::NOT_FOUND, "no such open question"),
    }
}

/// Record the decision in `body`, `{"decision": "accept" | "decline",
/// "subject": {"<header>": "<value>"}}`: the page names who decided by the
/// subject headers it vouches for, and each must be one of the asking
/// call's. Anyone else's decision is the same 404 as an unknown id.
fn decide(questions: &Questions, id: &str, body: &[u8], now: Instant) -> Response {
    let (decision, by) = match parse(body) {
        Ok(parsed) => parsed,
        Err(why) => return ngsi_problem(StatusCode::BAD_REQUEST, why),
    };
    let mut map = locked(questions);
    let Some(q) = map
        .get_mut(id)
        .filter(|q| q.until > now && by.iter().all(|pair| q.subject.contains(pair)))
    else {
        return ngsi_problem(StatusCode::NOT_FOUND, "no such open question");
    };
    q.decision = Some(decision);
    tracing::info!(view = %q.view, tool = %q.tool, decision = decision.as_str(), "mcp write decided");
    axum::Json(shown(id, q, now)).into_response()
}

/// The decision and the subject pairs of a decision body.
fn parse(body: &[u8]) -> Result<(Decision, Subject), &'static str> {
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(body) else {
        return Err("the body is no JSON object");
    };
    if body.keys().any(|k| k != "decision" && k != "subject") {
        return Err("the body holds only decision and subject");
    }
    let decision = match body.get("decision").and_then(Value::as_str) {
        Some("accept") => Decision::Accept,
        Some("decline") => Decision::Decline,
        _ => return Err("decision: accept or decline"),
    };
    let Some(Value::Object(subject)) = body.get("subject") else {
        return Err("subject: an object of header names and values");
    };
    let mut by = Vec::new();
    for (name, value) in subject {
        let Some(value) = value.as_str() else {
            return Err("subject: each value is a string");
        };
        by.push((name.to_ascii_lowercase(), value.as_bytes().to_vec()));
    }
    if by.is_empty() {
        return Err("subject: name the person who decided");
    }
    Ok((decision, by))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn person(sub: &str) -> Vec<(String, Vec<u8>)> {
        vec![("x-jc-subject".to_owned(), sub.as_bytes().to_vec())]
    }

    fn asked(q: &Questions, id: &str, sub: &str, now: Instant) {
        assert!(ask_in(
            q,
            id,
            &person(sub),
            ("v", "delete_entity", "Allow?"),
            now
        ));
    }

    fn body(decision: &str, sub: &str) -> Vec<u8> {
        json!({"decision": decision, "subject": {"X-JC-Subject": sub}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn the_template_needs_a_web_url_with_one_id() {
        assert_eq!(asking_of(None), Asking::Form);
        assert_eq!(asking_of(Some(" ")), Asking::Form);
        let good = "https://portal.example.org/mcp-confirmations/{id}";
        assert_eq!(asking_of(Some(good)), Asking::Page(good.to_owned()));
        for bad in [
            "https://portal.example.org/mcp-confirmations",
            "https://p/{id}/{id}",
            "javascript:alert(1)//{id}",
            "https://p/ {id}",
        ] {
            assert_eq!(asking_of(Some(bad)), Asking::Broken, "{bad}");
        }
        assert_eq!(
            page_of(good, "abc"),
            "https://portal.example.org/mcp-confirmations/abc"
        );
    }

    #[test]
    fn a_plain_http_page_serves_a_cluster_inside() {
        let plain = "http://portal.local/c/{id}";
        assert_eq!(asking_of(Some(plain)), Asking::Page(plain.to_owned()));
    }

    #[test]
    fn only_the_asking_person_decides_and_the_retry_takes_it_once() {
        let q = Questions::default();
        let now = Instant::now();
        asked(&q, "n1", "alice", now);
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Waiting);
        let other = decide(&q, "n1", &body("accept", "mallory"), now);
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Waiting);
        let ok = decide(&q, "n1", &body("accept", "alice"), now);
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(take_in(&q, "n1", &person("mallory"), now), Found::Unknown);
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Accepted);
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Unknown);
    }

    #[test]
    fn a_decline_ends_the_question_and_an_expired_one_is_gone() {
        let q = Questions::default();
        let now = Instant::now();
        asked(&q, "n1", "alice", now);
        assert_eq!(
            decide(&q, "n1", &body("decline", "alice"), now).status(),
            StatusCode::OK
        );
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Declined);
        asked(&q, "n2", "alice", now);
        let later = now + TTL + Duration::from_secs(1);
        assert_eq!(read(&q, "n2", later).status(), StatusCode::NOT_FOUND);
        assert_eq!(
            decide(&q, "n2", &body("accept", "alice"), later).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(take_in(&q, "n2", &person("alice"), later), Found::Unknown);
        assert_eq!(read(&q, "n3", now).status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_decision_body_names_a_decision_and_the_person() {
        let q = Questions::default();
        let now = Instant::now();
        asked(&q, "n1", "alice", now);
        for bad in [
            json!([]),
            json!({"decision": "maybe", "subject": {"x-jc-subject": "alice"}}),
            json!({"decision": "accept"}),
            json!({"decision": "accept", "subject": {}}),
            json!({"decision": "accept", "subject": {"x-jc-subject": 1}}),
            json!({"decision": "accept", "subject": {"x-jc-subject": "alice"}, "by": "x"}),
        ] {
            let answer = decide(&q, "n1", bad.to_string().as_bytes(), now);
            assert_eq!(answer.status(), StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Waiting);
    }

    #[test]
    fn every_subject_pair_the_page_names_must_be_the_askers() {
        let q = Questions::default();
        let now = Instant::now();
        asked(&q, "n1", "alice", now);
        let wider = json!({"decision": "accept",
            "subject": {"x-jc-subject": "alice", "x-jc-group": "admins"}});
        let answer = decide(&q, "n1", wider.to_string().as_bytes(), now);
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(take_in(&q, "n1", &person("alice"), now), Found::Waiting);
    }

    #[test]
    fn the_open_questions_are_bounded_per_subject() {
        let q = Questions::default();
        let now = Instant::now();
        for n in 0..MAX_OPEN_PER_SUBJECT {
            asked(&q, &format!("a{n}"), "alice", now);
        }
        assert!(!ask_in(
            &q,
            "a-more",
            &person("alice"),
            ("v", "t", "m"),
            now
        ));
        asked(&q, "b0", "bob", now);
        let later = now + TTL + Duration::from_secs(1);
        assert!(ask_in(
            &q,
            "a-later",
            &person("alice"),
            ("v", "t", "m"),
            later
        ));
    }

    #[test]
    fn only_a_recorded_accept_confirms_where_a_page_decides() {
        let page = Asking::Page("https://p/{id}".to_owned());
        assert_eq!(confirms(&page, Some(Found::Accepted), false), Ok(()));
        for found in [
            Some(Found::Waiting),
            Some(Found::Declined),
            Some(Found::Unknown),
            None,
        ] {
            assert!(confirms(&page, found, true).is_err());
        }
        assert_eq!(confirms(&Asking::Form, None, true), Ok(()));
        assert!(confirms(&Asking::Form, Some(Found::Accepted), false).is_err());
        assert_eq!(
            confirms(&Asking::Broken, Some(Found::Accepted), true),
            Err(BROKEN)
        );
        let who = person("alice");
        assert!(opens(&Asking::Form, "f1", &who, ("v", "t", "m")));
        assert!(!opens(&Asking::Broken, "b1", &who, ("v", "t", "m")));
        assert!(opens(&page, "p1", &who, ("v", "t", "m")));
        assert_eq!(take("p1", &who), Found::Waiting);
    }

    #[test]
    fn every_open_question_counts_toward_the_whole_bound() {
        let q = Questions::default();
        let now = Instant::now();
        for n in 0..MAX_OPEN {
            let sub = format!("s{}", n / MAX_OPEN_PER_SUBJECT);
            assert!(ask_in(
                &q,
                &format!("q{n}"),
                &person(&sub),
                ("v", "t", "m"),
                now
            ));
        }
        assert!(!ask_in(
            &q,
            "one-more",
            &person("new"),
            ("v", "t", "m"),
            now
        ));
    }

    #[test]
    fn the_page_reads_the_question_and_who_asked() {
        let q = Questions::default();
        let now = Instant::now();
        asked(&q, "n1", "alice", now);
        let map = locked(&q);
        let shown = map
            .get("n1")
            .map(|one| shown("n1", one, now))
            .unwrap_or_default();
        assert_eq!(
            shown.get("subject"),
            Some(&json!({"x-jc-subject": ["alice"]}))
        );
        assert_eq!(shown.get("message"), Some(&json!("Allow?")));
        assert_eq!(shown.get("decision"), Some(&Value::Null));
        assert_eq!(shown.get("expiresInSeconds"), Some(&json!(TTL.as_secs())));
    }
}
