// SPDX-License-Identifier: EUPL-1.2
//! The person's decision on a write, recorded by the deployment's decision
//! page (T-5816): with [`CONFIRM_URL_ENV`] set, a write tool asks by a URL
//! (MCP URL-mode elicitation) instead of a form, and its retry runs only on
//! the decision the page recorded through the admin route
//! `PUT /q/mcp-confirmations/{id}`. A client's own `accept` counts for
//! nothing: an AI client cannot approve its own write. The question and the
//! decision live in the store (T-7507), so any replica serves either and a
//! restart loses neither.

use crate::confirm::{self, TTL};
use antares_ops::{
    ngsi_problem, Caps, Confirmation, Confirmations, Confirming, NgsiError, Taken,
    TenantConfirmations,
};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};
use std::sync::LazyLock;
use std::time::Duration;

/// The decision page the person is sent to, `{id}` standing for the
/// question's id: `https://portal.example.org/mcp-confirmations/{id}`.
pub const CONFIRM_URL_ENV: &str = "ANTARES_MCP_CONFIRM_URL";

/// The placeholder of [`CONFIRM_URL_ENV`].
const ID: &str = "{id}";

/// The most open questions in a Tenant, past which a write is refused until
/// some expire, and of one subject in it, so one caller cannot fill them.
const OPEN: Caps = Caps {
    tenant: 10_000,
    subject: 100,
};

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
pub(crate) async fn opens(
    kept: &TenantConfirmations,
    asking: &Asking,
    id: &str,
    subject: &[(String, Vec<u8>)],
    about: (&str, &str, &str),
) -> Result<bool, NgsiError> {
    match asking {
        Asking::Form => Ok(true),
        Asking::Broken => Ok(false),
        Asking::Page(_) => kept.ask(id, &question(subject, about, TTL), OPEN).await,
    }
}

fn question(
    subject: &[(String, Vec<u8>)],
    (view, tool, message): (&str, &str, &str),
    left: Duration,
) -> Confirmation {
    Confirmation {
        view: view.to_owned(),
        tool: tool.to_owned(),
        message: message.to_owned(),
        subject: subject.to_vec(),
        left,
        decision: None,
    }
}

/// Why the retry carrying `state` may not run, or `Ok` once: with a page
/// the decision it recorded is taken, which spends it; with a form the
/// state is spent. A decline or an open question spends nothing more.
pub(crate) async fn answered(
    kept: &TenantConfirmations,
    asking: &Asking,
    (state, bound): (&str, &[u8]),
    subject: &[(String, Vec<u8>)],
    form_accepted: bool,
) -> Result<(), &'static str> {
    let found = match (asking, confirm::opened(state, bound)) {
        (Asking::Page(_), Some(id)) => Some(
            kept.take(&id, &subject.to_vec())
                .await
                .map_err(|e| unavailable(&e))?,
        ),
        _ => None,
    };
    confirms(asking, found, form_accepted)?;
    if *asking != Asking::Form {
        return Ok(());
    }
    match confirm::redeem(kept, state, bound, subject).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(STALE),
        Err(e) => Err(unavailable(&e)),
    }
}

/// The refusal while the store cannot say what was asked or decided.
pub(crate) fn unavailable(e: &NgsiError) -> &'static str {
    tracing::error!(error = %e, "the store of mcp write confirmations failed");
    "the confirmation could not be read or kept now; nothing was written, call again shortly"
}

/// Whether the retry of a write is confirmed: by the decision a page
/// recorded for `found` (the question its `requestState` seals, `None` when
/// it seals none), else by the client's form answer `form_accepted`; a
/// broken page confirms nothing.
pub(crate) fn confirms(
    asking: &Asking,
    found: Option<Taken>,
    form_accepted: bool,
) -> Result<(), &'static str> {
    const DECLINED: &str = "the person did not confirm; nothing was written";
    match asking {
        Asking::Form if form_accepted => Ok(()),
        Asking::Form => Err(DECLINED),
        Asking::Broken => Err(BROKEN),
        Asking::Page(_) => match found {
            Some(Taken::Accepted) => Ok(()),
            Some(Taken::Waiting) => Err("the person has not decided on the page yet; call again with the same requestState once they have"),
            Some(Taken::Declined) => Err(DECLINED),
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

/// The word of a decision.
fn word(accept: bool) -> &'static str {
    if accept {
        "accept"
    } else {
        "decline"
    }
}

/// The admin routes of the decision page, relative to `/q`, over the store
/// `all` reads: `GET /mcp-confirmations/{id}` reads a question, `PUT`
/// records the person's decision on it.
pub fn routes(all: Confirmations) -> Router {
    let deciding = all.clone();
    Router::new().route(
        "/mcp-confirmations/{id}",
        get(move |Path(id): Path<String>| async move { read(&all, &id).await })
            .put(
                move |Path(id): Path<String>, body: axum::body::Bytes| async move {
                    decide(&deciding, &id, &body).await
                },
            )
            .layer(axum::extract::DefaultBodyLimit::max(MAX_DECISION_BYTES)),
    )
}

/// The question's JSON: what the page shows the person, and the subject
/// headers it checks against the person signed in.
fn shown(id: &str, q: &Confirmation) -> Value {
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
        "expiresInSeconds": q.left.as_secs(),
        "decision": q.decision.map(word),
    })
}

fn not_open() -> Response {
    ngsi_problem(StatusCode::NOT_FOUND, "no such open question")
}

/// A store that cannot answer is never "no such question".
fn failed(e: &NgsiError) -> Response {
    tracing::error!(error = %e, "the store of mcp write confirmations failed");
    ngsi_problem(
        StatusCode::INTERNAL_SERVER_ERROR,
        "the questions cannot be read or kept now; try again shortly",
    )
}

async fn read(all: &Confirmations, id: &str) -> Response {
    let Some(tenant) = confirm::tenant_of(id) else {
        return not_open();
    };
    match all.of(tenant).find(id).await {
        Ok(Some(q)) => axum::Json(shown(id, &q)).into_response(),
        Ok(None) => not_open(),
        Err(e) => failed(&e),
    }
}

/// Record the decision in `body`, `{"decision": "accept" | "decline",
/// "subject": {"<header>": "<value>"}}`: the page names who decided by the
/// subject headers it vouches for, and each must be one of the asking
/// call's. Anyone else's decision is the same 404 as an unknown id.
async fn decide(all: &Confirmations, id: &str, body: &[u8]) -> Response {
    let (accept, by) = match parse(body) {
        Ok(parsed) => parsed,
        Err(why) => return ngsi_problem(StatusCode::BAD_REQUEST, why),
    };
    // the person, never only a header many callers share
    if !by.iter().any(|(name, _)| name == "x-jc-subject") {
        return not_open();
    }
    let Some(kept) = confirm::tenant_of(id).map(|tenant| all.of(tenant)) else {
        return not_open();
    };
    let asked = match kept.find(id).await {
        Ok(asked) => asked,
        Err(e) => return failed(&e),
    };
    // a question's subject never changes, so it may be checked before the write
    if !asked.is_some_and(|q| by.iter().all(|pair| q.subject.contains(pair))) {
        return not_open();
    }
    match kept.decide(id, accept).await {
        Ok(Some(q)) => {
            tracing::info!(view = %q.view, tool = %q.tool, decision = word(accept), "mcp write decided");
            axum::Json(shown(id, &q)).into_response()
        }
        Ok(None) => not_open(),
        Err(e) => failed(&e),
    }
}

/// The decision and the subject pairs of a decision body.
fn parse(body: &[u8]) -> Result<(bool, Confirming), &'static str> {
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(body) else {
        return Err("the body is no JSON object");
    };
    if body.keys().any(|k| k != "decision" && k != "subject") {
        return Err("the body holds only decision and subject");
    }
    let accept = match body.get("decision").and_then(Value::as_str) {
        Some("accept") => true,
        Some("decline") => false,
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
    Ok((accept, by))
}

#[cfg(test)]
mod tests {
    use super::*;
    use antares_ops::test_kit::TestBroker;
    use antares_ops::TenantId;

    const ABOUT: (&str, &str, &str) = ("v", "delete_entity", "Allow?");

    fn person(sub: &str) -> Confirming {
        vec![("x-jc-subject".to_owned(), sub.as_bytes().to_vec())]
    }

    /// The questions of one replica, and those of Tenant `t` on it.
    struct Replica {
        all: Confirmations,
        kept: TenantConfirmations,
    }

    impl Replica {
        fn of(broker: &TestBroker) -> Self {
            let all = broker.confirmations();
            let kept = all.of(TenantId::new("t").unwrap_or_default());
            Self { all, kept }
        }

        async fn new() -> Result<Self, String> {
            Ok(Self::of(&TestBroker::new().await?))
        }

        async fn asked(&self, subject: &Confirming, left: Duration, caps: Caps) -> (String, bool) {
            let (_, id) = confirm::seal(self.kept.tenant(), b"call").unwrap_or_default();
            let opened = self
                .kept
                .ask(&id, &question(subject, ABOUT, left), caps)
                .await;
            (id, matches!(opened, Ok(true)))
        }

        async fn open(&self, sub: &str) -> String {
            let (id, opened) = self.asked(&person(sub), TTL, OPEN).await;
            assert!(opened);
            id
        }

        async fn decide(&self, id: &str, body: &[u8]) -> StatusCode {
            decide(&self.all, id, body).await.status()
        }

        async fn take(&self, id: &str, subject: &Confirming) -> Option<Taken> {
            self.kept.take(id, subject).await.ok()
        }
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

    #[tokio::test]
    async fn only_the_asking_person_decides_and_the_retry_takes_it_once() -> Result<(), String> {
        let r = Replica::new().await?;
        let id = r.open("alice").await;
        let alice = person("alice");
        assert_eq!(r.take(&id, &alice).await, Some(Taken::Waiting));
        let other = r.decide(&id, &body("accept", "mallory")).await;
        assert_eq!(other, StatusCode::NOT_FOUND);
        assert_eq!(r.take(&id, &alice).await, Some(Taken::Waiting));
        assert_eq!(
            r.decide(&id, &body("accept", "alice")).await,
            StatusCode::OK
        );
        assert_eq!(r.take(&id, &person("mallory")).await, Some(Taken::Unknown));
        assert_eq!(r.take(&id, &alice).await, Some(Taken::Accepted));
        assert_eq!(r.take(&id, &alice).await, Some(Taken::Unknown));
        Ok(())
    }

    /// T-7507: a question asked on one replica is decided and taken on the
    /// other, also once the asking one is gone.
    #[tokio::test]
    async fn a_question_asked_on_one_replica_is_decided_on_the_other() -> Result<(), String> {
        let one = TestBroker::new().await?;
        let id = Replica::of(&one).open("alice").await;
        let other = Replica::of(&one.sibling().await?);
        drop(one);
        assert_eq!(
            (
                other.decide(&id, &body("accept", "alice")).await,
                other.take(&id, &person("alice")).await,
            ),
            (StatusCode::OK, Some(Taken::Accepted))
        );
        Ok(())
    }

    /// T-7507: an id naming another Tenant reaches no question of this one.
    #[tokio::test]
    async fn a_question_answers_in_its_own_tenant_alone() -> Result<(), String> {
        let r = Replica::new().await?;
        let id = r.open("alice").await;
        let nonce = id.split('-').next().unwrap_or_default();
        let forged = format!("{nonce}-75"); // Tenant "u"
        assert_eq!(read(&r.all, &forged).await.status(), StatusCode::NOT_FOUND);
        let decided = r.decide(&forged, &body("accept", "alice")).await;
        assert_eq!(decided, StatusCode::NOT_FOUND);
        let u = r.all.of(TenantId::new("u").map_err(|e| e.to_string())?);
        assert_eq!(
            u.take(&id, &person("alice")).await.ok(),
            Some(Taken::Unknown)
        );
        assert_eq!(r.take(&id, &person("alice")).await, Some(Taken::Waiting));
        Ok(())
    }

    #[tokio::test]
    async fn a_decline_ends_the_question_and_an_expired_one_is_gone() -> Result<(), String> {
        let r = Replica::new().await?;
        let alice = person("alice");
        let id = r.open("alice").await;
        assert_eq!(
            r.decide(&id, &body("decline", "alice")).await,
            StatusCode::OK
        );
        assert_eq!(r.take(&id, &alice).await, Some(Taken::Declined));
        let (late, opened) = r.asked(&alice, Duration::ZERO, OPEN).await;
        assert!(opened);
        assert_eq!(read(&r.all, &late).await.status(), StatusCode::NOT_FOUND);
        let decided = r.decide(&late, &body("accept", "alice")).await;
        assert_eq!(decided, StatusCode::NOT_FOUND);
        assert_eq!(r.take(&late, &alice).await, Some(Taken::Unknown));
        assert_eq!(read(&r.all, "n3").await.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn a_decision_body_names_a_decision_and_the_person() -> Result<(), String> {
        let r = Replica::new().await?;
        let id = r.open("alice").await;
        for bad in [
            json!([]),
            json!({"decision": "maybe", "subject": {"x-jc-subject": "alice"}}),
            json!({"decision": "accept"}),
            json!({"decision": "accept", "subject": {}}),
            json!({"decision": "accept", "subject": {"x-jc-subject": 1}}),
            json!({"decision": "accept", "subject": {"x-jc-subject": "alice"}, "by": "x"}),
        ] {
            let answer = r.decide(&id, bad.to_string().as_bytes()).await;
            assert_eq!(answer, StatusCode::BAD_REQUEST, "{bad}");
        }
        assert_eq!(r.take(&id, &person("alice")).await, Some(Taken::Waiting));
        Ok(())
    }

    #[tokio::test]
    async fn every_subject_pair_the_page_names_must_be_the_askers() -> Result<(), String> {
        let r = Replica::new().await?;
        let id = r.open("alice").await;
        let wider = json!({"decision": "accept",
            "subject": {"x-jc-subject": "alice", "x-jc-group": "admins"}});
        let answer = r.decide(&id, wider.to_string().as_bytes()).await;
        assert_eq!(answer, StatusCode::NOT_FOUND);
        assert_eq!(r.take(&id, &person("alice")).await, Some(Taken::Waiting));
        Ok(())
    }

    #[tokio::test]
    async fn a_decision_names_the_person_and_not_only_a_shared_header() -> Result<(), String> {
        let r = Replica::new().await?;
        let mut subject = person("alice");
        subject.push(("x-jc-org".to_owned(), b"city".to_vec()));
        let (id, opened) = r.asked(&subject, TTL, OPEN).await;
        assert!(opened);
        let shared = json!({"decision": "accept", "subject": {"x-jc-org": "city"}});
        let answer = r.decide(&id, shared.to_string().as_bytes()).await;
        assert_eq!(answer, StatusCode::NOT_FOUND);
        assert_eq!(r.take(&id, &subject).await, Some(Taken::Waiting));
        Ok(())
    }

    #[tokio::test]
    async fn the_open_questions_are_bounded_per_subject() -> Result<(), String> {
        let r = Replica::new().await?;
        for _ in 0..OPEN.subject {
            r.open("alice").await;
        }
        assert!(!r.asked(&person("alice"), TTL, OPEN).await.1);
        r.open("bob").await;
        Ok(())
    }

    #[tokio::test]
    async fn only_a_recorded_accept_confirms_where_a_page_decides() -> Result<(), String> {
        let page = Asking::Page("https://p/{id}".to_owned());
        assert_eq!(confirms(&page, Some(Taken::Accepted), false), Ok(()));
        for found in [
            Some(Taken::Waiting),
            Some(Taken::Declined),
            Some(Taken::Unknown),
            None,
        ] {
            assert!(confirms(&page, found, true).is_err());
        }
        assert_eq!(confirms(&Asking::Form, None, true), Ok(()));
        assert!(confirms(&Asking::Form, Some(Taken::Accepted), false).is_err());
        assert_eq!(
            confirms(&Asking::Broken, Some(Taken::Accepted), true),
            Err(BROKEN)
        );
        let r = Replica::new().await?;
        let who = person("alice");
        let opens = |asking, id| opens(&r.kept, asking, id, &who, ABOUT);
        assert_eq!(opens(&Asking::Form, "f1").await.ok(), Some(true));
        assert_eq!(opens(&Asking::Broken, "b1").await.ok(), Some(false));
        assert_eq!(opens(&page, "p1").await.ok(), Some(true));
        assert_eq!(r.take("p1", &who).await, Some(Taken::Waiting));
        Ok(())
    }

    #[tokio::test]
    async fn every_open_question_counts_toward_the_whole_bound() -> Result<(), String> {
        assert_eq!((OPEN.tenant, OPEN.subject), (10_000, 100));
        let r = Replica::new().await?;
        let three = Caps {
            tenant: 3,
            subject: 2,
        };
        for sub in ["a", "a", "b"] {
            assert!(r.asked(&person(sub), TTL, three).await.1);
        }
        assert!(!r.asked(&person("new"), TTL, three).await.1);
        Ok(())
    }

    #[test]
    fn the_page_reads_the_question_and_who_asked() {
        let shown = shown("n1", &question(&person("alice"), ABOUT, TTL));
        assert_eq!(
            shown.get("subject"),
            Some(&json!({"x-jc-subject": ["alice"]}))
        );
        assert_eq!(shown.get("message"), Some(&json!("Allow?")));
        assert_eq!(shown.get("decision"), Some(&Value::Null));
        assert_eq!(shown.get("expiresInSeconds"), Some(&json!(TTL.as_secs())));
    }
}
