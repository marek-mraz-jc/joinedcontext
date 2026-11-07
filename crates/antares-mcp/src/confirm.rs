// SPDX-License-Identifier: EUPL-1.2
//! The person's confirmation of a write (MCP 3.4, D-11, MC4): the first
//! call of a write tool answers an `InputRequiredResult` whose
//! `requestState` is sealed with HMAC-SHA256 over the call (view, subject,
//! tool and arguments) and expires after [`TTL`]; the retry that carries it
//! back is run once, and a second retry with the same state is refused.

use crate::REQUEST_STATE_KEY_FILE_ENV;
use rmcp::model::{RequestStateCodec, SealOptions};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// How long the person has to answer.
pub(crate) const TTL: Duration = Duration::from_secs(600);

/// The most confirmations spent within one [`TTL`]; past it a retry is
/// refused until the oldest expire, so the spent set stays bounded.
const MAX_SPENT: usize = 100_000;

/// The most confirmations one subject spends within one [`TTL`], so one
/// caller cannot fill [`MAX_SPENT`] and lock every other caller out.
const MAX_SPENT_PER_SUBJECT: usize = MAX_SPENT / 100;

/// The codec of this process: the deployment's key, else one drawn at
/// start. `None` when a key file is named and cannot be used: then no write
/// is confirmed at all.
fn codec() -> Option<&'static RequestStateCodec> {
    static CODEC: LazyLock<Option<RequestStateCodec>> =
        LazyLock::new(|| codec_of(key_file(std::env::var_os(REQUEST_STATE_KEY_FILE_ENV))));
    CODEC.as_ref()
}

/// The bytes of the key file at `path`, if one is named; a failure is its
/// kind, never the path's contents.
fn key_file(path: Option<std::ffi::OsString>) -> Option<Result<Vec<u8>, String>> {
    Some(std::fs::read(path?).map_err(|e| e.kind().to_string()))
}

/// The codec of the key `read` from the deployment's file, or of a key
/// drawn now when no file is named.
fn codec_of(read: Option<Result<Vec<u8>, String>>) -> Option<RequestStateCodec> {
    let key = match read {
        // two v4 UUIDs: 244 bits from the operating system's generator
        None => [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()]
            .iter()
            .flat_map(|u| u.into_bytes())
            .collect(),
        Some(Ok(key)) => key,
        Some(Err(why)) => {
            // the path names a secret's mount, never the secret
            tracing::error!(env = REQUEST_STATE_KEY_FILE_ENV, error = %why, "mcp confirmation key unreadable; write tools refuse every call");
            return None;
        }
    };
    let codec = RequestStateCodec::try_new(key).ok();
    if codec.is_none() {
        // shorter than 32 bytes: write tools refuse every call
        let env = REQUEST_STATE_KEY_FILE_ENV;
        tracing::error!(env, "mcp confirmation key too short");
    }
    codec
}

/// The confirmations already spent, per subject, with when each expires.
#[derive(Default)]
struct Spent {
    by_subject: HashMap<String, HashMap<String, Instant>>,
    total: usize,
}

impl Spent {
    /// Forget the nonces expired at `now`, and the subjects left without one.
    fn forget_expired(&mut self, now: Instant) {
        self.by_subject
            .values_mut()
            .for_each(|n| n.retain(|_, until| *until > now));
        self.by_subject.retain(|_, n| !n.is_empty());
        self.total = self.by_subject.values().map(HashMap::len).sum();
    }
}

static SPENT: LazyLock<Mutex<Spent>> = LazyLock::new(Mutex::default);

/// What a call is bound to: its view, the caller's subject, the tool and
/// its arguments as sent (a `Map` serializes its members sorted).
pub(crate) fn bound_to(
    view: &str,
    subject: &[(String, Vec<u8>)],
    tool: &str,
    given: &Map<String, Value>,
) -> Vec<u8> {
    json!([view, subject, tool, given]).to_string().into_bytes()
}

/// A fresh `requestState` for the call `bound` names and the nonce it
/// seals, or `None` when the process has no usable key.
pub(crate) fn seal(bound: &[u8]) -> Option<(String, String)> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let state = codec()?.seal_with(
        nonce.as_bytes(),
        &SealOptions::new().associated_data(bound).ttl(TTL),
    );
    Some((state, nonce))
}

/// The nonce `state` seals when this process sealed it for the call
/// `bound` names and it has not expired; spent or not.
pub(crate) fn opened(state: &str, bound: &[u8]) -> Option<String> {
    codec()
        .and_then(|c| c.open_with(state, bound).ok())
        .and_then(|n| String::from_utf8(n).ok())
}

/// Whether `state` was sealed by this process for the call `bound` names,
/// has not expired, and was not spent before; it is spent now. Every
/// failure is the same `false` (rmcp: "map all token-opening failures to a
/// single client-facing error").
pub(crate) fn redeem(state: &str, bound: &[u8], subject: &[(String, Vec<u8>)]) -> bool {
    let Some(nonce) = opened(state, bound) else {
        return false;
    };
    spend(&SPENT, json!(subject).to_string(), nonce, Instant::now())
}

/// Record `nonce` as spent by `subject` at `now`: `false` when it was spent
/// before, or the subject's share or the whole set is full of unexpired ones.
fn spend(spent: &Mutex<Spent>, subject: String, nonce: String, now: Instant) -> bool {
    // a holder that panicked left the map whole: an insert is one step
    let mut spent = spent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let held = spent.by_subject.get(&subject);
    if held.is_some_and(|n| n.contains_key(&nonce)) {
        return false;
    }
    if spent.total >= MAX_SPENT || held.is_some_and(|n| n.len() >= MAX_SPENT_PER_SUBJECT) {
        spent.forget_expired(now);
    }
    let held = spent.by_subject.get(&subject).map_or(0, HashMap::len);
    if spent.total >= MAX_SPENT || held >= MAX_SPENT_PER_SUBJECT {
        return false;
    }
    let until = now.checked_add(TTL).unwrap_or(now);
    spent
        .by_subject
        .entry(subject)
        .or_default()
        .insert(nonce, until);
    spent.total = spent.total.saturating_add(1);
    true
}

/// Whether the person accepted: the `confirm` answer is an accepted
/// elicitation whose `confirm` is `true`.
pub(crate) fn accepted(responses: Option<&std::collections::BTreeMap<String, Value>>) -> bool {
    let Some(answer) = responses.and_then(|r| r.get(ANSWER)) else {
        return false;
    };
    serde_json::from_value::<rmcp::model::ElicitResult>(answer.clone()).is_ok_and(|r| {
        r.action == rmcp::model::ElicitationAction::Accept
            && r.content.as_ref().and_then(|c| c.get(ANSWER)) == Some(&Value::Bool(true))
    })
}

/// The key of the one input request, and of the field the person sets.
pub(crate) const ANSWER: &str = "confirm";

/// The most ids or names one question lists before it counts the rest.
const MAX_LISTED: usize = 20;

/// What the person is asked: the tool, its operation, the Entity ids and
/// the Attribute names the call touches and, for a Subscription, the
/// endpoint it notifies; never an Attribute value (MCP 3.4, section 10
/// rule 1). Each is a checked argument: an id is a URN without spaces, a
/// name is one the view shows, an endpoint one the deployment allows.
pub(crate) fn question(tool: &str, operation: &str, given: &Map<String, Value>) -> String {
    let subscription = tool.ends_with("_subscription");
    let (mut ids, mut names, mut endpoints) = (Vec::new(), Vec::new(), Vec::new());
    for (arg, v) in given {
        match (arg.as_str(), v) {
            ("id", Value::String(id)) => ids.push(id.clone()),
            ("ids", Value::Array(list)) => {
                ids.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
            }
            ("attrId", Value::String(name)) => names.push(name.clone()),
            ("entity" | "attributes" | "fragment", Value::Object(o)) if !subscription => {
                members(o, &mut ids, &mut names);
            }
            ("entities", Value::Array(list)) => {
                for o in list.iter().filter_map(Value::as_object) {
                    members(o, &mut ids, &mut names);
                }
            }
            ("subscription" | "fragment", _) => {
                endpoints.extend(
                    v.pointer("/notification/endpoint/uri")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                );
            }
            _ => {}
        }
    }
    let mut parts = vec![format!("Allow {tool} ({operation})")];
    for (what, list) in [("on", ids), ("Attributes", names), ("notifying", endpoints)] {
        if !list.is_empty() {
            parts.push(format!("{what}: {}", listed(&list)));
        }
    }
    format!("{}?", parts.join("; "))
}

/// The ids and Attribute names of an Entity body or fragment.
fn members(o: &Map<String, Value>, ids: &mut Vec<String>, names: &mut Vec<String>) {
    for (k, v) in o {
        match k.as_str() {
            "id" => ids.extend(v.as_str().map(str::to_owned)),
            "type" => {}
            name => names.push(name.to_owned()),
        }
    }
}

/// `list` deduplicated, at most [`MAX_LISTED`] of it, and how many more.
fn listed(list: &[String]) -> String {
    let unique: std::collections::BTreeSet<&str> = list.iter().map(String::as_str).collect();
    let shown: Vec<&str> = unique.iter().take(MAX_LISTED).copied().collect();
    let more = unique.len().saturating_sub(shown.len());
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{} and {more} more", shown.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_opens_for_its_own_call_once() {
        let call = bound_to("v", &[], "delete_entity", &Map::new());
        let (state, _) = seal(&call).unwrap_or_default();
        assert_ne!(state, "");
        let other = bound_to("w", &[], "delete_entity", &Map::new());
        assert!(!redeem(&state, &other, &[]));
        assert!(redeem(&state, &call, &[]));
        assert!(!redeem(&state, &call, &[]));
        assert!(!redeem("rs1.x.y", &call, &[]));
    }

    #[test]
    fn the_spent_set_is_bounded_and_forgets_expired_nonces() {
        let spent = Mutex::default();
        let start = Instant::now();
        for n in 0..MAX_SPENT {
            let who = (n / MAX_SPENT_PER_SUBJECT).to_string();
            assert!(spend(&spent, who, n.to_string(), start));
        }
        assert!(!spend(&spent, "new".into(), "full".into(), start));
        assert!(!spend(&spent, "0".into(), "0".into(), start));
        let later = start.checked_add(TTL.saturating_mul(2)).unwrap_or(start);
        assert!(spend(&spent, "new".into(), "full".into(), later));
    }

    #[test]
    fn one_subject_cannot_spend_the_share_of_another() {
        let spent = Mutex::default();
        let start = Instant::now();
        for n in 0..MAX_SPENT_PER_SUBJECT {
            assert!(spend(&spent, "a".into(), n.to_string(), start));
        }
        assert!(!spend(&spent, "a".into(), "more".into(), start));
        assert!(spend(&spent, "b".into(), "more".into(), start));
        let later = start.checked_add(TTL.saturating_mul(2)).unwrap_or(start);
        assert!(spend(&spent, "a".into(), "more".into(), later));
    }

    #[test]
    fn a_key_file_is_read_when_one_is_named() {
        assert!(key_file(None).is_none());
        let missing = key_file(Some("/nonexistent/antares-mcp-key".into()));
        assert_eq!(missing, Some(Err("entity not found".to_owned())));
        let here = key_file(Some(
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml").into(),
        ));
        assert!(here.is_some_and(|k| k.is_ok_and(|k| k.len() > 32)));
    }

    #[test]
    fn a_key_file_must_be_readable_and_long_enough() {
        assert!(codec_of(None).is_some());
        assert!(codec_of(Some(Ok(vec![7; 32]))).is_some());
        assert!(codec_of(Some(Ok(vec![7; 31]))).is_none());
        assert!(codec_of(Some(Err("not found".into()))).is_none());
    }

    #[test]
    fn the_question_names_ids_attributes_and_endpoints_never_values() {
        let given = |v: Value| v.as_object().cloned().unwrap_or_default();
        let q = question(
            "update_attributes",
            "update_attributes",
            &given(
                json!({"id": "urn:ngsi-ld:Car:1", "attributes": {"speed": {"value": "drop the table"}}}),
            ),
        );
        assert_eq!(q, "Allow update_attributes (update_attributes); on: urn:ngsi-ld:Car:1; Attributes: speed?");
        let many: Vec<Value> = (0..25)
            .map(|n| json!({"id": format!("urn:ngsi-ld:Car:{n:02}"), "type": "Car", "speed": 1}))
            .collect();
        let q = question(
            "batch_create",
            "batch_create",
            &given(json!({"entities": many})),
        );
        assert!(
            q.contains("urn:ngsi-ld:Car:19 and 5 more; Attributes: speed?"),
            "{q}"
        );
        let q = question(
            "delete_attribute",
            "delete_attribute",
            &given(json!({"id": "urn:ngsi-ld:Car:1", "attrId": "speed", "deleteAll": true})),
        );
        assert_eq!(
            q,
            "Allow delete_attribute (delete_attribute); on: urn:ngsi-ld:Car:1; Attributes: speed?"
        );
        let q = question(
            "batch_delete",
            "batch_delete",
            &given(json!({"ids": ["urn:ngsi-ld:Car:1"]})),
        );
        assert_eq!(
            q,
            "Allow batch_delete (batch_delete); on: urn:ngsi-ld:Car:1?"
        );
        let sub = json!({"notification": {"endpoint": {"uri": "https://hooks.example/a"}}});
        let q = question(
            "create_subscription",
            "create_subscription",
            &given(json!({"subscription": sub})),
        );
        assert_eq!(
            q,
            "Allow create_subscription (create_subscription); notifying: https://hooks.example/a?"
        );
        let q = question(
            "update_subscription",
            "update_subscription",
            &given(json!({"id": "urn:ngsi-ld:Subscription:s", "fragment": sub})),
        );
        assert_eq!(q, "Allow update_subscription (update_subscription); on: urn:ngsi-ld:Subscription:s; notifying: https://hooks.example/a?");
        let q = question(
            "delete_subscription",
            "delete_subscription",
            &given(json!({"id": "urn:ngsi-ld:Subscription:s"})),
        );
        assert_eq!(
            q,
            "Allow delete_subscription (delete_subscription); on: urn:ngsi-ld:Subscription:s?"
        );
    }

    #[test]
    fn only_an_accepted_true_confirms() {
        let answer = |v: Value| std::collections::BTreeMap::from([(ANSWER.to_owned(), v)]);
        let yes = answer(json!({"action": "accept", "content": {"confirm": true}}));
        assert!(accepted(Some(&yes)));
        for no in [
            json!({"action": "accept", "content": {"confirm": false}}),
            json!({"action": "accept"}),
            json!({"action": "decline"}),
            json!({"action": "cancel", "content": {"confirm": true}}),
            json!("yes"),
        ] {
            assert!(!accepted(Some(&answer(no.clone()))), "{no}");
        }
        assert!(!accepted(None));
        assert!(!accepted(Some(&std::collections::BTreeMap::new())));
    }
}
