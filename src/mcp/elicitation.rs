//! The question a Yellow or Red MCP call asks before it runs (AG-63, T-0836).
//!
//! An agent may propose and may ask to remove, but it may not decide alone: a call whose
//! operation takes the Yellow or the Red lane, or carries `destructiveHint`, answers an
//! elicitation first. The host shows it to the person, the person opens the Portal page the
//! question names and decides there with their own session, and the client repeats the call
//! naming the question. Only then does the operation run, and the answer is written to the
//! project's activity, so what a person allowed is on the record (AG-56).
//!
//! The decision is the Portal's record, never the client's word (T-5699): a client holding the
//! person's token could otherwise accept its own question without anyone seeing it. The second
//! call's `action` can only decline; it never accepts.
//!
//! The route is one stateless POST, so the elicitation cannot be a server→client request on an
//! open stream: it is the answer to the first call and an argument of the second. What that
//! costs is one extra round trip; what it buys is that the words the person saw and the call
//! that runs are the same call, bound by id, subject and arguments.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::auth::session::now_unix;

/// How long a person has to answer before the id is no longer accepted.
pub const EXPIRES_IN_SECONDS: i64 = 600;

/// What the person decided on the Portal page.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationDecision {
    Accept,
    Decline,
}

/// One question waiting for its answer.
struct Pending {
    /// The subject of the token that asked: nobody else may answer it.
    owner: String,
    operation: String,
    project: String,
    /// The arguments the question was asked about: a different call needs a new answer.
    digest: String,
    /// The words the client was handed, shown again on the page.
    message: String,
    /// The Portal page of what the call would change, a path under the Portal.
    resource: String,
    asked_at: i64,
    /// The person's decision, once they made it on the page.
    decision: Option<ElicitationDecision>,
}

/// One open question as its page shows it to the person who asked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    /// The operation the call runs, `jc_resource_delete`.
    pub operation: String,
    pub message: String,
    /// The Portal path of what the call would change, `/projects/helsinki/endpoints/air`.
    pub resource: String,
    /// Unix seconds after which the question is no longer answered.
    pub expires_at: i64,
    /// `None` until the person decided.
    pub decision: Option<ElicitationDecision>,
}

/// What the caller sent back with the second call.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// The person allowed it on the Portal page: the operation runs.
    Accepted,
    /// The person refused, or the client declined: nothing runs.
    Declined,
    /// The question is open and the person has not decided yet. Nothing runs; the same call may
    /// be sent again once they have.
    Waiting,
    /// No such question for this caller, or it was answered, expired, or asked about something
    /// else. Nothing runs, and the caller asks again.
    Unknown,
}

/// Every question this replica is waiting on.
///
/// ponytail: per-replica map, like the Portal's other in-process state (one Portal replica,
/// owner 2026-10-23); a question lives ten minutes, so a restart costs one repeated ask, never a
/// lost decision.
#[derive(Clone, Default)]
pub struct McpElicitations {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl std::fmt::Debug for McpElicitations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpElicitations")
    }
}

impl McpElicitations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a question and returns its id: random, so it names nothing of the call.
    pub fn ask(
        &self,
        owner: &str,
        operation: &str,
        project: &str,
        digest: &str,
        message: &str,
        resource: &str,
    ) -> String {
        use argon2::password_hash::rand_core::{OsRng, RngCore};
        self.forget_expired();
        let id = format!("eli-{:016x}", OsRng.next_u64());
        if let Ok(mut map) = self.pending.lock() {
            map.insert(
                id.clone(),
                Pending {
                    owner: owner.to_owned(),
                    operation: operation.to_owned(),
                    project: project.to_owned(),
                    digest: digest.to_owned(),
                    message: message.to_owned(),
                    resource: resource.to_owned(),
                    asked_at: now_unix(),
                    decision: None,
                },
            );
        }
        id
    }

    /// The open question `id` of `owner` in `project`, for its page; `None` for anyone else's,
    /// an answered one or an expired one.
    pub fn question(&self, owner: &str, project: &str, id: &str) -> Option<Question> {
        let map = self.pending.lock().ok()?;
        let pending = map
            .get(id)
            .filter(|pending| open_for(pending, owner, project))?;
        Some(question_of(id, pending))
    }

    /// Records the person's decision on the page. `None` when the question is not theirs, was
    /// spent or expired. A second decision before the client's call replaces the first.
    pub fn decide(
        &self,
        owner: &str,
        project: &str,
        id: &str,
        decision: ElicitationDecision,
    ) -> Option<Question> {
        let mut map = self.pending.lock().ok()?;
        let pending = map
            .get_mut(id)
            .filter(|pending| open_for(pending, owner, project))?;
        pending.decision = Some(decision);
        Some(question_of(id, pending))
    }

    /// Reads the answer to the client's second call, and spends the question once it is decided:
    /// an id answers once. Only the decision the person recorded accepts; a client's `action`
    /// may decline and nothing more. `workload` is a ServiceAccount's own token: no person stands
    /// behind it to ask, so its own `accept` is its answer, and the Change it opens is still a
    /// person's to approve in its lane.
    pub fn answer(
        &self,
        owner: &str,
        operation: &str,
        project: &str,
        digest: &str,
        sent: &Value,
        workload: bool,
    ) -> Answer {
        let Some(id) = sent
            .get("elicitationId")
            .or_else(|| sent.get("id"))
            .and_then(Value::as_str)
        else {
            return Answer::Unknown;
        };
        let Ok(mut map) = self.pending.lock() else {
            return Answer::Unknown;
        };
        let Some(pending) = map.get(id) else {
            return Answer::Unknown;
        };
        // The question, the caller and the call all have to be the one that was asked about.
        if !open_for(pending, owner, project)
            || pending.operation != operation
            || pending.digest != digest
        {
            return Answer::Unknown;
        }
        let action = sent.get("action").and_then(Value::as_str);
        let client_declined = matches!(
            action,
            Some("decline" | "declined" | "cancel" | "cancelled")
        );
        let decision = match (pending.decision, action) {
            (None, Some("accept" | "accepted")) if workload => Some(ElicitationDecision::Accept),
            (decision, _) => decision,
        };
        match (decision, client_declined) {
            (Some(ElicitationDecision::Accept), false) => {
                map.remove(id);
                Answer::Accepted
            }
            (Some(ElicitationDecision::Decline), _) | (_, true) => {
                map.remove(id);
                Answer::Declined
            }
            (None, false) => Answer::Waiting,
        }
    }

    fn forget_expired(&self) {
        let now = now_unix();
        if let Ok(mut map) = self.pending.lock() {
            map.retain(|_, pending| now - pending.asked_at <= EXPIRES_IN_SECONDS);
        }
    }
}

/// Whether `pending` is still open and asked by `owner` in `project`.
fn open_for(pending: &Pending, owner: &str, project: &str) -> bool {
    pending.owner == owner
        && pending.project == project
        && now_unix() - pending.asked_at <= EXPIRES_IN_SECONDS
}

fn question_of(id: &str, pending: &Pending) -> Question {
    Question {
        id: id.to_owned(),
        operation: pending.operation.clone(),
        message: pending.message.clone(),
        resource: pending.resource.clone(),
        expires_at: pending.asked_at + EXPIRES_IN_SECONDS,
        decision: pending.decision,
    }
}

/// The `_meta` key of an `input_required` answer's machine-readable question (T-6361).
pub const ELICITATION_META: &str = "joinedcontext.com/elicitation";

/// The question as the client receives it: what would happen, and where the person decides.
///
/// The text carries everything a model needs to go on, the Portal URL and the `elicitationId`
/// included: a real MCP client hands its model the content text only (T-6042, T-6361). The
/// answer carries no `structuredContent`, because every tool declares an `outputSchema` and a
/// question is not the tool's output (CC-47); the same fields sit in `_meta` under
/// [`ELICITATION_META`] for a client that reads them by name.
///
/// `refusal` is the gate's own answer when one already holds — a proposal whose check has not
/// run, for instance. It is carried beside the question rather than instead of it: the three
/// fields the REST route answers (`error`, `check`, `reason`) sit at the top of that `_meta`
/// object, so a client that reads `verdict_required` on the route reads it here too, and the URL
/// the person needs is still there (PF-57, ADR-N-021, T-0947).
pub fn document(id: &str, message: &str, url: &str, refusal: Option<Value>) -> Value {
    let mut fields = serde_json::Map::new();
    if let Some(Value::Object(refused)) = refusal {
        fields.extend(refused);
    }
    fields.insert(
        "elicitation".to_owned(),
        json!({
            "elicitationId": id,
            "mode": "url",
            "url": url,
            "message": message,
            "expiresIn": EXPIRES_IN_SECONDS,
        }),
    );
    let minutes = EXPIRES_IN_SECONDS / 60;
    let text = format!(
        "{message}\n\nThe person decides here: {url}\nThen send this same call again with \
         `\"elicitation\": {{\"elicitationId\": \"{id}\"}}` added to its arguments (or to its \
         params); `\"action\": \"decline\"` beside the id withdraws it. The question expires in \
         {minutes} minutes."
    );
    json!({
        "isError": false,
        "status": "input_required",
        "content": [{ "type": "text", "text": text }],
        "_meta": { ELICITATION_META: Value::Object(fields) },
    })
}

/// A stable short digest of the arguments a question was asked about.
pub fn digest_of(value: &Value) -> String {
    digest_short(&serde_json::to_string(&canonical(value)).unwrap_or_default())
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> =
                map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn digest_short(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))[..16].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> Value {
        json!({ "kind": "Endpoint", "name": "helsinki-bikes-ops", "confirm": "helsinki-bikes-ops" })
    }

    fn ask(asks: &McpElicitations, digest: &str) -> String {
        asks.ask(
            "sub-1",
            "jc_resource_delete",
            "helsinki",
            digest,
            "Delete in project 'helsinki' (red lane).",
            "/projects/helsinki/endpoints/helsinki-bikes-ops",
        )
    }

    /// T-5699, F-POR4-001: the client's own `accept` runs nothing; only the decision the person
    /// recorded on the Portal page does, and it is spent once.
    #[test]
    fn a_client_accept_runs_nothing_and_the_recorded_decision_does() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = ask(&asks, &digest);
        let sent = json!({ "elicitationId": id, "action": "accept" });
        let answer = |sent: &Value| {
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &digest,
                sent,
                false,
            )
        };

        assert_eq!(answer(&sent), Answer::Waiting);
        assert_eq!(answer(&sent), Answer::Waiting, "a wait spends nothing");
        let decided = asks.decide("sub-1", "helsinki", &id, ElicitationDecision::Accept);
        assert_eq!(
            decided.map(|q| q.decision),
            Some(Some(ElicitationDecision::Accept))
        );
        assert_eq!(answer(&json!({ "elicitationId": id })), Answer::Accepted);
        assert_eq!(answer(&sent), Answer::Unknown, "an answer is spent once");
    }

    #[test]
    fn the_answer_belongs_to_the_caller_the_operation_and_the_arguments_asked_about() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = ask(&asks, &digest);
        // Another subject or project neither reads nor decides the question.
        assert!(asks.question("sub-2", "helsinki", &id).is_none());
        assert!(asks.question("sub-1", "espoo", &id).is_none());
        assert!(asks
            .decide("sub-2", "helsinki", &id, ElicitationDecision::Accept)
            .is_none());
        assert!(asks
            .decide("sub-1", "helsinki", &id, ElicitationDecision::Accept)
            .is_some());
        let sent = json!({ "elicitationId": id });

        assert_eq!(
            asks.answer(
                "sub-2",
                "jc_resource_delete",
                "helsinki",
                &digest,
                &sent,
                false
            ),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_resource_propose",
                "helsinki",
                &digest,
                &sent,
                false
            ),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "espoo",
                &digest,
                &sent,
                false
            ),
            Answer::Unknown
        );
        let other = digest_of(&json!({ "kind": "Endpoint", "name": "something-else" }));
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &other,
                &sent,
                false
            ),
            Answer::Unknown
        );
        assert_eq!(
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &digest,
                &sent,
                false
            ),
            Answer::Accepted
        );
    }

    #[test]
    fn a_decline_on_the_page_or_from_the_client_runs_nothing() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let declined = ask(&asks, &digest);
        assert!(asks
            .decide("sub-1", "helsinki", &declined, ElicitationDecision::Decline)
            .is_some());
        let answer = |sent: Value| {
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &digest,
                &sent,
                false,
            )
        };
        assert_eq!(
            answer(json!({ "elicitationId": declined, "action": "accept" })),
            Answer::Declined
        );

        // The client may refuse what the person accepted; a refusal wins.
        let accepted = ask(&asks, &digest);
        assert!(asks
            .decide("sub-1", "helsinki", &accepted, ElicitationDecision::Accept)
            .is_some());
        assert_eq!(
            answer(json!({ "elicitationId": accepted, "action": "decline" })),
            Answer::Declined
        );

        let undecided = ask(&asks, &digest);
        assert_eq!(
            answer(json!({ "elicitationId": undecided, "action": "cancel" })),
            Answer::Declined
        );
        assert_eq!(answer(json!({ "action": "accept" })), Answer::Unknown);
        assert_eq!(answer(json!({ "elicitationId": 7 })), Answer::Unknown);
    }

    #[test]
    fn the_page_reads_what_was_asked_and_what_was_decided() {
        let asks = McpElicitations::new();
        let id = ask(&asks, &digest_of(&arguments()));
        let open = asks.question("sub-1", "helsinki", &id);
        assert_eq!(
            open.as_ref()
                .map(|q| (q.operation.as_str(), q.resource.as_str(), q.decision)),
            Some((
                "jc_resource_delete",
                "/projects/helsinki/endpoints/helsinki-bikes-ops",
                None
            ))
        );
        assert!(open.is_some_and(|q| q.message.contains("red lane") && q.expires_at > now_unix()));
        // A decision may be changed until the client's call spends it.
        assert!(asks
            .decide("sub-1", "helsinki", &id, ElicitationDecision::Accept)
            .is_some());
        let changed = asks.decide("sub-1", "helsinki", &id, ElicitationDecision::Decline);
        assert_eq!(
            changed.and_then(|q| q.decision),
            Some(ElicitationDecision::Decline)
        );
        assert!(asks
            .question("sub-1", "helsinki", "eli-0000000000000000")
            .is_none());
    }

    /// A ServiceAccount's own token has no person to ask: its `accept` answers, a person's does
    /// not.
    #[test]
    fn a_workloads_own_accept_answers_and_a_persons_never_does() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = ask(&asks, &digest);
        let sent = json!({ "elicitationId": id, "action": "accept" });
        let answer = |workload| {
            asks.answer(
                "sub-1",
                "jc_resource_delete",
                "helsinki",
                &digest,
                &sent,
                workload,
            )
        };
        assert_eq!(answer(false), Answer::Waiting);
        assert_eq!(answer(true), Answer::Accepted);
    }

    /// T-6361: the model reads the text alone, so the text names where to decide and the id to
    /// send back; the answer carries no `structuredContent` a tool's `outputSchema` would refuse.
    #[test]
    fn the_text_names_the_url_and_the_id_and_no_structured_content_is_sent() {
        let url = "https://portal.example/projects/helsinki/mcp-elicitations/eli-00000000000000aa";
        let refusal = json!({ "error": "verdict_required", "check": "jc_change_plan" });
        let answer = document("eli-00000000000000aa", "Delete it.", url, Some(refusal));
        let text = answer["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.starts_with("Delete it."), "{text}");
        assert!(text.contains(url), "{text}");
        assert!(
            text.contains("\"elicitationId\": \"eli-00000000000000aa\""),
            "{text}"
        );
        assert!(text.contains("arguments"), "{text}");
        assert!(answer.get("structuredContent").is_none(), "{answer}");
        let meta = &answer["_meta"][ELICITATION_META];
        assert_eq!(meta["elicitation"]["url"], url);
        assert_eq!(meta["elicitation"]["elicitationId"], "eli-00000000000000aa");
        assert_eq!(meta["error"], "verdict_required");
        assert_eq!(answer["status"], "input_required");
    }

    #[test]
    fn the_same_arguments_digest_the_same_however_they_were_written() {
        let a = json!({ "b": [1, { "y": 2, "x": 1 }], "a": "z" });
        let b = json!({ "a": "z", "b": [1, { "x": 1, "y": 2 }] });
        assert_eq!(digest_of(&a), digest_of(&b));
        assert_ne!(digest_of(&a), digest_of(&json!({ "a": "z" })));
    }

    #[test]
    fn the_id_a_client_is_handed_carries_nothing_of_the_call_it_is_about() {
        let asks = McpElicitations::new();
        let digest = digest_of(&arguments());
        let id = asks.ask(
            "sub-secret",
            "jc_resource_delete",
            "tajny-projekt",
            &digest,
            "m",
            "/",
        );
        assert!(id.starts_with("eli-"), "{id}");
        let body = id.trim_start_matches("eli-");
        assert_eq!(body.len(), 16, "{id}");
        assert!(body.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        for leaked in [
            "sub-secret",
            "tajny-projekt",
            "jc_resource_delete",
            digest.as_str(),
        ] {
            assert!(!id.contains(leaked), "the id carried {leaked:?}: {id}");
        }
        assert_ne!(
            id,
            asks.ask(
                "sub-secret",
                "jc_resource_delete",
                "tajny-projekt",
                &digest,
                "m",
                "/"
            )
        );
    }
}
