// SPDX-License-Identifier: EUPL-1.2
//! The person's confirmation of a write (MCP 3.4, D-11, MC4): the first
//! call of a write tool answers an `InputRequiredResult` whose
//! `requestState` is sealed with HMAC-SHA256 over the call (view, subject,
//! tool and arguments) and expires after [`TTL`]; the retry that carries it
//! back is run once on whichever replica (T-7507: the store holds what was
//! spent), and a second retry with the same state is refused.

use crate::REQUEST_STATE_KEY_FILE_ENV;
use antares_ops::{Caps, NgsiError, TenantConfirmations, TenantId};
use rmcp::model::{RequestStateCodec, SealOptions};
use serde_json::{json, Map, Value};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

/// How long the person has to answer.
pub(crate) const TTL: Duration = Duration::from_secs(600);

/// The most confirmations spent within one [`TTL`] in a Tenant, and by one
/// subject in it, so one caller cannot lock every other caller out.
const SPENT: Caps = Caps {
    tenant: 100_000,
    subject: 1000,
};

/// The key a host hands over where the deployment names no key file.
static HOSTED_KEY: OnceLock<Vec<u8>> = OnceLock::new();

/// See [`crate::default_request_state_key`].
pub(crate) fn host_key(key: &[u8]) {
    let _first_wins = HOSTED_KEY.set(key.to_vec());
}

/// The codec of this process: the deployment's key file, else the host's
/// key, else one drawn at start. `None` when a key file is named and cannot
/// be used: then no write is confirmed at all.
fn codec() -> Option<&'static RequestStateCodec> {
    static CODEC: LazyLock<Option<RequestStateCodec>> = LazyLock::new(|| {
        codec_of(
            key_file(std::env::var_os(REQUEST_STATE_KEY_FILE_ENV))
                .or_else(|| HOSTED_KEY.get().cloned().map(Ok)),
        )
    });
    CODEC.as_ref()
}

/// The bytes of the key file at `path`, if one is named; a failure is its
/// kind, never the path's contents.
fn key_file(path: Option<std::ffi::OsString>) -> Option<Result<Vec<u8>, String>> {
    Some(std::fs::read(path?).map_err(|e| e.kind().to_string()))
}

/// The codec of the key `read` from the deployment's file, or of a key
/// drawn now when neither a file nor a host names one.
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

/// What a call is bound to: its Tenant, view, the caller's subject, the
/// tool and its arguments as sent (a `Map` serializes its members sorted).
pub(crate) fn bound_to(
    tenant: &str,
    view: &str,
    subject: &[(String, Vec<u8>)],
    tool: &str,
    given: &Map<String, Value>,
) -> Vec<u8> {
    json!([tenant, view, subject, tool, given])
        .to_string()
        .into_bytes()
}

/// A fresh `requestState` for the call `bound` names and the question id it
/// seals, or `None` when the process has no usable key.
pub(crate) fn seal(tenant: &str, bound: &[u8]) -> Option<(String, String)> {
    let id = id_of(&uuid::Uuid::new_v4().simple().to_string(), tenant);
    Some((sealed(codec()?, &id, bound), id))
}

fn sealed(codec: &RequestStateCodec, id: &str, bound: &[u8]) -> String {
    codec.seal_with(
        id.as_bytes(),
        &SealOptions::new().associated_data(bound).ttl(TTL),
    )
}

/// A question's id: the nonce, a hyphen and its Tenant in hex, so the admin
/// route that gets the id alone works inside that Tenant and never reads
/// across them.
fn id_of(nonce: &str, tenant: &str) -> String {
    use std::fmt::Write as _;
    let hex = tenant.bytes().fold(String::new(), |mut hex, byte| {
        // writing to a String cannot fail
        let _written = write!(hex, "{byte:02x}");
        hex
    });
    format!("{nonce}-{hex}")
}

/// The Tenant an id of [`id_of`] names; `None` for any other string.
pub(crate) fn tenant_of(id: &str) -> Option<TenantId> {
    let (nonce, hex) = id.split_once('-')?;
    let lower_hex = |s: &str| s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let sized = nonce.len() == 32
        && !hex.is_empty()
        && hex.len().is_multiple_of(2)
        && hex.len() <= TenantId::MAX_LEN.saturating_mul(2);
    if !sized || !lower_hex(nonce) || !lower_hex(hex) {
        return None;
    }
    let name: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect::<Option<_>>()?;
    TenantId::new(std::str::from_utf8(&name).ok()?).ok()
}

/// The id `state` seals when it was sealed under this deployment's key for
/// the call `bound` names and it has not expired; spent or not.
pub(crate) fn opened(state: &str, bound: &[u8]) -> Option<String> {
    codec().and_then(|c| opened_with(c, state, bound))
}

fn opened_with(codec: &RequestStateCodec, state: &str, bound: &[u8]) -> Option<String> {
    let id = codec.open_with(state, bound).ok()?;
    String::from_utf8(id).ok()
}

/// Whether `state` was sealed for the call `bound` names, has not expired,
/// and was not spent on any replica; it is spent now. Every refusal is the
/// same `false` (rmcp: "map all token-opening failures to a single
/// client-facing error").
pub(crate) async fn redeem(
    kept: &TenantConfirmations,
    state: &str,
    bound: &[u8],
    subject: &[(String, Vec<u8>)],
) -> Result<bool, NgsiError> {
    match opened(state, bound) {
        Some(id) => kept.spend(&id, &subject.to_vec(), TTL, SPENT).await,
        None => Ok(false),
    }
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
    use antares_ops::test_kit::TestBroker;

    fn tenant(name: &str) -> TenantId {
        TenantId::new(name).unwrap_or_default()
    }

    #[tokio::test]
    async fn a_state_opens_for_its_own_call_once() -> Result<(), String> {
        let kept = TestBroker::new().await?.confirmations().of(tenant("t"));
        let bound = |tenant, view| bound_to(tenant, view, &[], "delete_entity", &Map::new());
        let call = bound("t", "v");
        let (state, id) = seal("t", &call).unwrap_or_default();
        assert_eq!(opened(&state, &call), Some(id));
        for other in [bound("t", "w"), bound("u", "v")] {
            assert_eq!(redeem(&kept, &state, &other, &[]).await.ok(), Some(false));
        }
        assert_eq!(redeem(&kept, &state, &call, &[]).await.ok(), Some(true));
        assert_eq!(redeem(&kept, &state, &call, &[]).await.ok(), Some(false));
        assert_eq!(redeem(&kept, "rs1.x.y", &call, &[]).await.ok(), Some(false));
        Ok(())
    }

    /// T-7507: a nonce spent on one replica is spent on the other.
    #[tokio::test]
    async fn a_nonce_redeems_once_across_replicas() -> Result<(), String> {
        let one = TestBroker::new().await?;
        let other = one.sibling().await?;
        let (one, other) = (
            one.confirmations().of(tenant("t")),
            other.confirmations().of(tenant("t")),
        );
        let call = bound_to("t", "v", &[], "delete_entity", &Map::new());
        let (state, _) = seal("t", &call).unwrap_or_default();
        assert_eq!(
            (
                redeem(&one, &state, &call, &[]).await.ok(),
                redeem(&other, &state, &call, &[]).await.ok(),
            ),
            (Some(true), Some(false))
        );
        Ok(())
    }

    /// T-7507: the same key opens after a restart, another key never does.
    #[test]
    fn a_restart_opens_what_the_same_key_sealed() {
        let codec = |byte| codec_of(Some(Ok(vec![byte; 32])));
        let (Some(before), Some(after), Some(other)) = (codec(7), codec(7), codec(8)) else {
            unreachable!("32 bytes are a key");
        };
        let state = sealed(&before, "id", b"call");
        assert_eq!(opened_with(&after, &state, b"call").as_deref(), Some("id"));
        assert_eq!(opened_with(&other, &state, b"call"), None);
    }

    #[test]
    fn an_id_names_its_tenant_and_nothing_else_parses() {
        let nonce = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
        for name in ["default", "w", "city_a-1", &"t".repeat(TenantId::MAX_LEN)] {
            let id = id_of(nonce, name);
            assert_eq!(
                tenant_of(&id).map(|t| t.as_str().to_owned()).as_deref(),
                Some(name)
            );
        }
        for bad in [
            "q1".to_owned(),
            nonce.to_owned(),
            format!("{nonce}-"),
            format!("{nonce}-7"),
            format!("{nonce}-7A"),
            format!("{nonce}-2a"),
            format!("{nonce}-ff"),
            format!("short-{}", "77"),
            id_of(nonce, "antares-views"),
            id_of(nonce, &"t".repeat(TenantId::MAX_LEN + 1)),
        ] {
            assert!(tenant_of(&bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_spent_set_is_bounded_and_forgets_expired_nonces() -> Result<(), String> {
        assert_eq!((SPENT.tenant, SPENT.subject), (100_000, 1000));
        let kept = TestBroker::new().await?.confirmations().of(tenant("t"));
        let caps = Caps {
            tenant: 3,
            subject: 2,
        };
        let who = |n: u8| vec![("x-user".to_owned(), vec![n])];
        for (id, n, ttl) in [("a", 1, Duration::ZERO), ("b", 1, TTL), ("c", 1, TTL)] {
            assert_eq!(
                kept.spend(id, &who(n), ttl, caps).await.ok(),
                Some(true),
                "{id}"
            );
        }
        assert_eq!(kept.spend("b", &who(2), TTL, caps).await.ok(), Some(false));
        assert_eq!(kept.spend("d", &who(1), TTL, caps).await.ok(), Some(false));
        assert_eq!(kept.spend("d", &who(2), TTL, caps).await.ok(), Some(true));
        assert_eq!(kept.spend("e", &who(3), TTL, caps).await.ok(), Some(false));
        Ok(())
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
