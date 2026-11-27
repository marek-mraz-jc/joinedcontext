//! A write tool's arguments under the guards the NGSI-LD surface runs on the same write
//! (T-4590): a tool is the NGSI-LD request it stands for, so its body argument passes the core
//! context check, a subscription's notification target check, a form's minted id and one entity
//! per create, the model's units and relationships, and a form's daily create quota, exactly as
//! that request would at `/ngsi-ld/v1`. What the guards change (a minted id, stripped
//! quarantine marks) goes on to the broker in place of what the caller sent.

use crate::app::{Gateway, MAX_BODY};
use crate::auth::subject::principal_of;
use crate::auth::subject::Subject;
use crate::handlers::ngsi_ld::{reserve_create, view_payload};
use crate::handlers::writes::{apply_planned, Planned};
use crate::mcp::tools::{Shape, TOOLS};
use crate::resolver::Endpoint;
use crate::{mcp, operations, query};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, Method};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// A create the call holds of a form's daily quota, released when the broker does not take it.
pub(crate) struct Reserved {
    caller: String,
    at: u64,
}

impl Reserved {
    pub(crate) fn release(self, gateway: &Gateway, endpoint: &Endpoint) {
        gateway
            .creates
            .release(&endpoint.slug, &self.caller, self.at);
    }
}

/// What a guarded call holds until the broker answers it.
#[derive(Default)]
pub(crate) struct Guarded {
    /// The create it holds of a form's daily quota.
    pub(crate) reserved: Option<Reserved>,
    /// A confirmed delete's planned relationship changes and the headers they are made with,
    /// made only once the broker took the call ([`carry_out`]).
    pub(crate) deletes: Option<(HeaderMap, Planned)>,
}

/// Makes a confirmed delete's planned changes when the broker `taken` the call: the broker
/// alone knows whether the `requestState` is one it sealed, so nothing changes before it
/// deleted the entity (DM-71). `Err` is the text the call answers instead.
pub(crate) async fn carry_out(
    gateway: &Gateway,
    deletes: Option<(HeaderMap, Planned)>,
    taken: bool,
) -> Result<(), String> {
    let Some((headers, planned)) = deletes.filter(|_| taken) else {
        return Ok(());
    };
    // ponytail: a batch_delete result's per-id errors are not read, so the plan of an id the
    // broker kept inside a taken batch is still made; read them once the tool's result shape is
    // shared with the broker.
    apply_planned(gateway, &headers, &planned, |_| true)
        .await
        .map_err(|problem| problem.detail.unwrap_or(problem.title))
}

/// Whether a tool call's answer, JSON or the last event of a stream, is a result that is not a
/// tool error.
pub(crate) fn taken(status: axum::http::StatusCode, held: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(held) else {
        return false;
    };
    let json = text
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("data:"))
        .unwrap_or(text);
    status.is_success()
        && serde_json::from_str::<Value>(json.trim())
            .is_ok_and(|answer| answer["result"].is_object() && answer["result"]["isError"] != true)
}

/// Runs the guards on a `tools/call` of a write tool and rewrites its body argument with what
/// they kept; a read tool passes untouched. `Err` is the JSON-RPC answer that refuses the call.
pub(crate) async fn guarded(
    gateway: &Gateway,
    endpoint: &Endpoint,
    subject: &Subject,
    caller: &str,
    message: &mut Value,
) -> Result<Guarded, Value> {
    let id = message["id"].clone();
    // A call that names no tool runs none: the broker answers it with its invalid-params error.
    let Some(tool) = message["params"]["name"].as_str().map(str::to_owned) else {
        return Ok(Guarded::default());
    };
    if is_read_tool(&tool) {
        return Ok(Guarded::default());
    }
    let refuse =
        |why: &str| mcp::jsonrpc::result(id.clone(), mcp::jsonrpc::refused(why, &Value::Null));
    // The broker runs a write only on the retry that carries the `requestState` it sealed when
    // it asked the person (antares-mcp `Server::write`); the call before it creates nothing.
    let confirmed = message["params"]["requestState"]
        .as_str()
        .is_some_and(|state| !state.is_empty());
    let empty = Map::new();
    let arguments = message["params"]["arguments"].as_object().unwrap_or(&empty);
    let asked = asked_key(&endpoint.slug, caller, &tool, arguments);
    // A write the gateway cannot name as an NGSI-LD request is one it cannot guard: refused,
    // as an unknown tool is to an anonymous caller.
    let Some((method, path, body)) = request_of(&tool, arguments) else {
        tracing::info!(slug = %endpoint.slug, %tool, "MCP write refused: no NGSI-LD request it stands for");
        return Err(refuse(
            "this write tool is not offered here, or its arguments are incomplete",
        ));
    };
    let Some(operation) = operations::operation_of(&method, &path, false) else {
        return Err(refuse("this write names no resource the Endpoint serves"));
    };
    let mut sent = match body.and_then(|name| arguments.get(name)) {
        Some(value) => serde_json::to_vec(value).unwrap_or_default(),
        None => Vec::new(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    // The relationship guards read and unlink in the Endpoint's space, as the NGSI-LD surface's
    // pinned request does; without it they ran in the broker's default tenant (T-4755).
    let Some(tenant) = endpoint.tenant.as_ref() else {
        return Err(refuse(
            "this Endpoint's space has no tenant yet: an operator names the project's Organization",
        ));
    };
    crate::middleware::tenancy::pin_tenant(&mut headers, tenant);
    let mut divided = None;
    let mut planned = Planned::new();
    if let Err(refusal) = view_payload(
        gateway,
        (endpoint, subject),
        (&path, &[], operation),
        &mut headers,
        &mut sent,
        &mut divided,
        &mut planned,
    )
    .await
    {
        let held = axum::body::to_bytes(refusal.into_body(), MAX_BODY)
            .await
            .unwrap_or_default();
        tracing::info!(slug = %endpoint.slug, %tool, principal = %principal_of(subject), "MCP write refused by the Endpoint's write guards");
        return Err(refuse(&refusal_text(&held)));
    }
    // A batch the NGSI-LD surface would split (207) is refused whole: a tool call has one result.
    if let Some((_, refused)) = divided {
        let (first, problem) = &refused[0];
        let why = problem.detail.as_deref().unwrap_or(problem.title.as_str());
        return Err(refuse(&format!(
            "{} of the entities were refused, {first} first: {why}; send the others without them",
            refused.len()
        )));
    }
    if operation == jc_core::kinds::Operation::CreateEntity
        && endpoint
            .creates
            .as_ref()
            .is_some_and(|creates| creates.mint_ids)
    {
        same_minted_id(asked, confirmed, &mut sent, Instant::now());
    }
    if let (Some(name), Ok(kept)) = (body, serde_json::from_slice::<Value>(&sent)) {
        // A body argument was read out of `arguments` above, so it is an object.
        let _ = message["params"]["arguments"]
            .as_object_mut()
            .map(|arguments| arguments.insert(name.to_owned(), kept));
    }
    // The call before the person's answer only plans the changes, so a refusal is told at once.
    let deletes = (confirmed && !planned.is_empty()).then_some((headers, planned));
    match reserve_create(gateway, endpoint, caller, operation) {
        Ok(at) => {
            let reserved = at.map(|at| Reserved {
                caller: caller.to_owned(),
                at,
            });
            // The call before the person's answer only learns a slot is free; the confirmed
            // retry takes it.
            if confirmed {
                return Ok(Guarded { reserved, deletes });
            }
            if let Some(reserved) = reserved {
                reserved.release(gateway, endpoint);
            }
            Ok(Guarded::default())
        }
        Err(refusal) => {
            let held = axum::body::to_bytes(refusal.into_body(), MAX_BODY)
                .await
                .unwrap_or_default();
            Err(refuse(&refusal_text(&held)))
        }
    }
}

/// How long a form's minted id waits for the confirmed retry: the broker's question lifetime
/// (antares-mcp `confirm::TTL`).
const MINTED_FOR: Duration = Duration::from_secs(600);

/// The most questions whose minted ids are kept at once; past it a retry gets a fresh id, which
/// the broker refuses as a stale confirmation.
const MINTED_MAX: usize = 4_096;

/// Per question, the id minted for it and until when it waits.
type Minted = HashMap<Vec<u8>, (String, Instant)>;

/// The id a form minted for a create the person is asked about, by [`asked_key`]: the broker
/// binds its question to the arguments, so the confirmed retry must send the same id.
static MINTED: LazyLock<Mutex<Minted>> = LazyLock::new(Mutex::default);

/// One caller's call of one tool with these arguments on one Endpoint.
fn asked_key(slug: &str, caller: &str, tool: &str, arguments: &Map<String, Value>) -> Vec<u8> {
    let asked = serde_json::to_string(&(slug, caller, tool, arguments)).unwrap_or_default();
    ring::digest::digest(&ring::digest::SHA256, asked.as_bytes())
        .as_ref()
        .to_vec()
}

/// Keeps the id minted into `sent` for the question `asked`, or on the confirmed retry puts the
/// one kept for it back.
fn same_minted_id(asked: Vec<u8>, confirmed: bool, sent: &mut Vec<u8>, now: Instant) {
    let Ok(mut entity) = serde_json::from_slice::<Value>(sent) else {
        return;
    };
    let mut minted = MINTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if confirmed {
        // Kept until it expires: a decision page answers a retry made before the person decided
        // with "still open", and the next retry needs the same id.
        if let Some((id, _)) = minted.get(&asked).filter(|(_, until)| *until > now) {
            entity["id"] = Value::String(id.clone());
            *sent = entity.to_string().into_bytes();
        }
        return;
    }
    let Some(id) = entity["id"].as_str() else {
        return;
    };
    if minted.len() >= MINTED_MAX {
        minted.retain(|_, (_, until)| *until > now);
    }
    if minted.len() < MINTED_MAX || minted.contains_key(&asked) {
        minted.insert(asked, (id.to_owned(), now + MINTED_FOR));
    }
}

fn word<'a>(arguments: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

/// The sentence of a Problem Details body, or a plain refusal when it has none.
fn refusal_text(body: &[u8]) -> String {
    problem_text(body).unwrap_or_else(|| "this Endpoint refused the write".to_owned())
}

/// The NGSI-LD request a broker write tool stands for (Antares `docs/src/mcp.md` section 2): its
/// method, its path under `/ngsi-ld/v1` and the argument that is its body. `None` for a tool
/// this table does not know, or one whose path argument is missing.
fn request_of(
    tool: &str,
    arguments: &Map<String, Value>,
) -> Option<(Method, String, Option<&'static str>)> {
    let (_, method, template, body) = REQUESTS.iter().find(|(name, ..)| *name == tool)?;
    let mut path = (*template).to_owned();
    if path.contains("{id}") {
        path = path.replace("{id}", &query::encode(word(arguments, "id")?));
    }
    if path.contains("{attrId}") {
        path = path.replace("{attrId}", &query::encode(word(arguments, "attrId")?));
    }
    Some((method.clone(), path, *body))
}

/// Tool, method, path (`{id}` percent-encoded, `{attrId}` as given) and body argument.
const REQUESTS: &[(&str, Method, &str, Option<&str>)] = &[
    ("create_entity", Method::POST, "/entities", Some("entity")),
    (
        "replace_entity",
        Method::PUT,
        "/entities/{id}",
        Some("entity"),
    ),
    (
        "merge_entity",
        Method::PATCH,
        "/entities/{id}",
        Some("fragment"),
    ),
    ("delete_entity", Method::DELETE, "/entities/{id}", None),
    (
        "append_attributes",
        Method::POST,
        "/entities/{id}/attrs",
        Some("attributes"),
    ),
    (
        "update_attributes",
        Method::PATCH,
        "/entities/{id}/attrs",
        Some("attributes"),
    ),
    (
        "update_attribute",
        Method::PATCH,
        "/entities/{id}/attrs/{attrId}",
        Some("attribute"),
    ),
    (
        "replace_attribute",
        Method::PUT,
        "/entities/{id}/attrs/{attrId}",
        Some("attribute"),
    ),
    (
        "delete_attribute",
        Method::DELETE,
        "/entities/{id}/attrs/{attrId}",
        None,
    ),
    (
        "batch_create",
        Method::POST,
        "/entityOperations/create",
        Some("entities"),
    ),
    (
        "batch_upsert",
        Method::POST,
        "/entityOperations/upsert",
        Some("entities"),
    ),
    (
        "batch_update",
        Method::POST,
        "/entityOperations/update",
        Some("entities"),
    ),
    (
        "batch_merge",
        Method::POST,
        "/entityOperations/merge",
        Some("entities"),
    ),
    (
        "batch_delete",
        Method::POST,
        "/entityOperations/delete",
        Some("ids"),
    ),
    (
        "create_subscription",
        Method::POST,
        "/subscriptions",
        Some("subscription"),
    ),
    (
        "update_subscription",
        Method::PATCH,
        "/subscriptions/{id}",
        Some("fragment"),
    ),
    (
        "delete_subscription",
        Method::DELETE,
        "/subscriptions/{id}",
        None,
    ),
];

/// The sentence of a Problem Details body: its `detail`, else its `title`.
pub(crate) fn problem_text(body: &[u8]) -> Option<String> {
    let problem = serde_json::from_slice::<Value>(body).ok()?;
    problem["detail"]
        .as_str()
        .or_else(|| problem["title"].as_str())
        .map(str::to_owned)
}

/// The broker's second name of `get_type`.
pub(crate) const DESCRIBE_TYPE: &str = "describe_type";

/// Whether the broker tool only reads: a broker tool of [`TOOLS`] none of whose operations writes
/// (Antares `docs/src/mcp.md` section 2); any other name is a write.
pub(crate) fn is_read_tool(tool: &str) -> bool {
    // The broker lists `get_type` answered with its guide as `describe_type` too (antares-mcp).
    let tool = if tool == DESCRIBE_TYPE {
        "get_type"
    } else {
        tool
    };
    TOOLS.iter().any(|known| {
        known.name == tool && matches!(known.shape, Shape::Broker(_)) && known.read_only()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::ngsi_ld::create_key;
    use serde_json::json;

    /// The read tools are the broker tools that write nothing, read off [`TOOLS`] (F-R2PLT2-012):
    /// a gateway tool and an unknown name are no broker read.
    #[test]
    fn a_read_tool_is_a_broker_tool_that_writes_nothing() {
        for read in [
            "batch_query",
            "query_entities",
            "summarize",
            "list_subscriptions",
            DESCRIBE_TYPE,
        ] {
            assert!(is_read_tool(read), "{read}");
        }
        for other in [
            "delete_entity",
            "batch_upsert",
            "describe_schema",
            "no_such_tool",
        ] {
            assert!(!is_read_tool(other), "{other}");
        }
    }

    /// An attribute name stays one path segment: `../subscriptions` cannot turn an attribute
    /// write into a request on another resource the guards would classify instead.
    #[test]
    fn an_attribute_name_cannot_leave_its_segment() {
        let arguments = json!({"id": "urn:ngsi-ld:Thing:a", "attrId": "../../../subscriptions"});
        let (_, path, _) = request_of("update_attribute", arguments.as_object().unwrap()).unwrap();
        assert!(!path.contains("/../"), "{path}");
        assert_eq!(path.matches('/').count(), 4, "{path}");
    }

    /// Every tool the gateway lists is a read or a write it can name as an NGSI-LD request, and
    /// that request is the operation the tool's grant names: a tool added to one list and not the
    /// other fails here, not as an unguarded write.
    #[test]
    fn every_listed_write_tool_is_the_ngsi_ld_request_its_grant_names() {
        let arguments = json!({"id": "urn:ngsi-ld:Thing:a b", "attrId": "speed"});
        let arguments = arguments.as_object().unwrap();
        for tool in mcp::tools::TOOLS
            .iter()
            .filter(|tool| matches!(tool.shape, mcp::tools::Shape::Broker(_)))
        {
            if is_read_tool(tool.name) {
                assert!(request_of(tool.name, arguments).is_none(), "{}", tool.name);
                continue;
            }
            let request = request_of(tool.name, arguments);
            assert!(
                request.is_some(),
                "{} is a write with no NGSI-LD request",
                tool.name
            );
            let (method, path, _) = request.expect("a write's NGSI-LD request");
            assert_eq!(
                operations::operation_of(&method, &path, false).as_ref(),
                tool.operations.first(),
                "{}",
                tool.name
            );
        }
    }

    #[test]
    fn an_id_is_one_path_segment_and_a_missing_one_names_no_request() {
        let arguments = json!({"id": "urn:ngsi-ld:Thing:x/../../subscriptions"});
        let (_, path, _) = request_of("delete_entity", arguments.as_object().unwrap()).unwrap();
        assert_eq!(
            path,
            "/entities/urn%3Angsi-ld%3AThing%3Ax%2F..%2F..%2Fsubscriptions"
        );
        assert!(request_of("replace_entity", &Map::new()).is_none());
        assert!(request_of("a_tool_added_later", &Map::new()).is_none());
    }

    #[test]
    fn a_signed_in_caller_spends_their_own_creates_and_an_anonymous_one_their_address() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.7"));
        let jana = Subject {
            id: Some("0f5a".to_owned()),
            user: Some("jana".to_owned()),
            ..Subject::default()
        };
        assert_eq!(create_key(&jana, &headers), "principal:user:0f5a");
        assert_eq!(
            create_key(&Subject::anonymous(), &headers),
            "address:10.0.0.7"
        );
    }

    #[test]
    fn a_body_that_is_no_text_is_no_taken_call_and_a_refusal_has_a_sentence() {
        assert!(!taken(axum::http::StatusCode::OK, &[0xff, 0xfe]));
        assert!(taken(
            axum::http::StatusCode::OK,
            br#"{"result":{"content":[]}}"#
        ));
        assert!(!taken(
            axum::http::StatusCode::OK,
            br#"{"result":{"isError":true}}"#
        ));
        assert_eq!(refusal_text(b"not json"), "this Endpoint refused the write");
        assert_eq!(refusal_text(br#"{"title":"Forbidden"}"#), "Forbidden");
        assert_eq!(
            refusal_text(br#"{"title":"Forbidden","detail":"not yours"}"#),
            "not yours"
        );
    }

    /// One test, since the kept ids are one process-wide map: a retry gets the id the question
    /// was minted with, a body that is no JSON or has no id is left alone, an expired entry
    /// makes room, and a full map of live entries takes no new question.
    #[test]
    fn a_minted_id_is_kept_for_the_retry_and_the_map_stays_bounded() {
        let now = Instant::now();
        let key = |n: usize| {
            asked_key(
                "s",
                "jana",
                "create_entity",
                &Map::from_iter([("n".to_owned(), json!(n))]),
            )
        };
        let id_of =
            |sent: &[u8]| serde_json::from_slice::<Value>(sent).expect("json")["id"].clone();
        let mut garbage = b"not json".to_vec();
        same_minted_id(key(0), false, &mut garbage, now);
        assert_eq!(garbage, b"not json");

        let mut first = json!({"id": "urn:x:minted"}).to_string().into_bytes();
        same_minted_id(key(0), false, &mut first, now);
        let mut retry = json!({"id": "urn:x:fresh"}).to_string().into_bytes();
        same_minted_id(key(0), true, &mut retry, now);
        assert_eq!(id_of(&retry), "urn:x:minted");
        let mut late = json!({"id": "urn:x:fresh"}).to_string().into_bytes();
        same_minted_id(key(0), true, &mut late, now + MINTED_FOR * 2);
        assert_eq!(id_of(&late), "urn:x:fresh");

        let mut nameless = json!({"type": "T"}).to_string().into_bytes();
        same_minted_id(key(1), false, &mut nameless, now);
        assert!(!MINTED.lock().expect("map").contains_key(&key(1)));

        let fill = |until: Instant| {
            let mut map = MINTED.lock().expect("map");
            map.clear();
            for n in 0..MINTED_MAX {
                map.insert(vec![1, n as u8, (n >> 8) as u8], ("urn:x:o".into(), until));
            }
        };
        fill(now);
        let mut sent = json!({"id": "urn:x:new"}).to_string().into_bytes();
        same_minted_id(key(2), false, &mut sent, now + MINTED_FOR);
        assert_eq!(
            MINTED.lock().expect("map").len(),
            1,
            "expired entries made room"
        );

        fill(now + MINTED_FOR);
        same_minted_id(key(3), false, &mut sent, now);
        assert!(
            !MINTED.lock().expect("map").contains_key(&key(3)),
            "a full map takes no key"
        );
        MINTED.lock().expect("map").clear();
    }

    /// A confirmed delete whose planned changes cannot be made answers with the reason instead
    /// of the delete's result; one the broker did not take changes nothing.
    #[tokio::test]
    async fn a_planned_change_that_fails_is_the_answer_of_the_call() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let gateway = Gateway::new(
            crate::proxy::Broker::new(&url),
            Box::new(crate::pdp::PolicyPdp),
            "domain",
        );
        let planned = || -> Planned {
            vec![(
                "urn:x".to_owned(),
                vec![crate::handlers::writes::Unlinking::Delete(
                    "urn:y".to_owned(),
                )],
            )]
        };
        let kept = carry_out(&gateway, Some((HeaderMap::new(), planned())), false).await;
        assert_eq!(
            kept,
            Ok(()),
            "nothing changes before the broker took the delete"
        );
        let failed = carry_out(&gateway, Some((HeaderMap::new(), planned())), true).await;
        let text = failed.expect_err("the broker is unreachable");
        assert!(!text.is_empty(), "the call answers with a reason");
    }

    /// An Endpoint whose space has no tenant yet refuses a write before it plans anything: the
    /// route's own tenancy check answers first, so this guard is run directly.
    #[tokio::test]
    async fn a_write_on_an_endpoint_without_a_tenant_is_refused_before_it_is_planned() {
        let gateway = Gateway::new(
            crate::proxy::Broker::new("http://127.0.0.1:9"),
            Box::new(crate::pdp::PolicyPdp),
            "domain",
        );
        let mut endpoint = (*crate::fanout::hub::tests_support::endpoint()).clone();
        endpoint.tenant = None;
        let mut message = json!({"id": 3, "params": {"name": "create_entity",
            "arguments": {"entity": {"id": "urn:ngsi-ld:T:a", "type": "T"}}}});
        let refused = guarded(
            &gateway,
            &endpoint,
            &Subject::anonymous(),
            "caller",
            &mut message,
        )
        .await
        .err()
        .expect("a refusal");
        let text = refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        assert!(text.contains("has no tenant yet"), "{refused}");
        assert_eq!(refused["id"], 3);
    }
}
