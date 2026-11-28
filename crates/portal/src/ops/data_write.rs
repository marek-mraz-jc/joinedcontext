//! `jc_data_write`, the entity write of the connector tools (T-6389): create, upsert, merge or
//! delete up to [`MAX_ENTITIES`] entities through one Endpoint of the project, as the caller.
//!
//! As `jc_data_query`, the Portal writes with no token of its own: it exchanges the token the
//! caller presented for one the gateway takes for that Endpoint alone, so the Endpoint's Policy
//! decides each entity on the person's live grants. A public Endpoint of another project is never
//! a target: an anonymous caller writes nothing. A connector gets the tool only with its `writes`
//! switch and its policy's, beside the plan's apply (`connectors::written`).

use serde::Deserialize;
use serde_json::{json, Value};

use super::bounds::{self, text, ID, RECORD_MEMBERS};
use super::data_query::{capped_page, detail_of, readable_slug, Target, GATEWAY_TIMEOUT};
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::error::ApiError;
use crate::pipeline_source_deletes::{reason_header, MAX_REASON};
use crate::state::AppState;

pub const NAME: &str = "jc_data_write";
/// Entities one call writes: one batch of the broker's entity operations, small enough that the
/// person confirming it can read what goes.
pub const MAX_ENTITIES: usize = 100;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Create,
    Upsert,
    Merge,
    Delete,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataWriteInput {
    pub endpoint: String,
    pub action: Action,
    #[serde(default)]
    pub entities: Vec<Value>,
    #[serde(default)]
    pub ids: Vec<String>,
    /// Why the person deletes, in their words (DM-113); a delete needs it, other actions ignore it.
    #[serde(default)]
    pub reason: Option<String>,
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "endpoint": text("Endpoint name, e.g. bike-stations", bounds::NAME),
            "action": {
                "type": "string",
                "enum": ["create", "upsert", "merge", "delete"],
                "description": "create: new entities; upsert: create or replace attributes; merge: patch existing ones"
            },
            "entities": {
                "type": "array",
                "maxItems": MAX_ENTITIES,
                "description": "create, upsert, merge: NGSI-LD entities, e.g. {id: urn:ngsi-ld:Bike:s1, type: Bike, name: {type: Property, value: Main}}",
                "items": { "type": "object", "maxProperties": RECORD_MEMBERS }
            },
            "ids": {
                "type": "array",
                "maxItems": MAX_ENTITIES,
                "description": "delete: entity ids",
                "items": { "type": "string", "maxLength": ID }
            },
            // Twice the gateway's 500: whitespace around a reason is trimmed, `checked` counts the rest.
            "reason": text("delete: why, in the person's words", 2 * MAX_REASON as u64)
        },
        "required": ["endpoint", "action"],
        "additionalProperties": false
    })
}

fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "written": { "type": "array", "items": { "type": "string" }, "description": "Ids written" },
            "errors": {
                "type": "array",
                "description": "Refused entities, each with its id and why",
                "items": { "type": "object" }
            }
        },
        "required": ["written", "errors"]
    })
}

fn invalid(path: &str, message: impl Into<String>) -> OpError {
    OpError::InvalidInput {
        path: path.into(),
        message: message.into(),
    }
}

/// The input of one call, refused where it is wrong before the person is asked anything: a
/// delete names ids and no entities, any other action entities with an id and a type and no ids.
pub fn checked(input: &DataWriteInput) -> Result<(), OpError> {
    if input.action == Action::Delete {
        if input.ids.is_empty() || !input.entities.is_empty() {
            return Err(invalid(
                "/ids",
                "a delete names the entity ids, and no entities",
            ));
        }
        if let Some(at) = input.ids.iter().position(|id| id.trim().is_empty()) {
            return Err(invalid(
                &format!("/ids/{at}"),
                "an id is an entity URN, e.g. urn:ngsi-ld:T:1",
            ));
        }
        if delete_reason(input).is_none() {
            return Err(invalid(
                "/reason",
                format!(
                    "a delete names why in the person's own words, 1 to {MAX_REASON} characters: \
                     ask them, never write one yourself"
                ),
            ));
        }
        if delete_reason(input).is_some_and(crate::secret_scan::quoted_secret) {
            return Err(invalid(
                "/reason",
                "the reason reads like a credential: it would be kept with the delete, so leave the credential out",
            ));
        }
        return Ok(());
    }
    if input.entities.is_empty() || !input.ids.is_empty() {
        return Err(invalid(
            "/entities",
            "create, upsert and merge name the entities, each with id and type, and no ids",
        ));
    }
    for (at, entity) in input.entities.iter().enumerate() {
        for member in ["id", "type"] {
            if !entity
                .get(member)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.trim().is_empty())
            {
                return Err(invalid(
                    &format!("/entities/{at}/{member}"),
                    format!("each entity names its {member}"),
                ));
            }
        }
    }
    if input.entities.len() > MAX_ENTITIES || input.ids.len() > MAX_ENTITIES {
        return Err(invalid(
            "/entities",
            format!("at most {MAX_ENTITIES} per call"),
        ));
    }
    Ok(())
}

fn validate_write(val: &Value) -> Result<(), OpError> {
    checked(&parse_input(val.clone())?)
}

/// The reason of a delete as the gateway counts it (DM-113): trimmed, 1 to [`MAX_REASON`]
/// characters; `None` for any other length, which is refused, never cut.
fn delete_reason(input: &DataWriteInput) -> Option<&str> {
    let reason = input.reason.as_deref()?.trim();
    (1..=MAX_REASON)
        .contains(&reason.chars().count())
        .then_some(reason)
}

/// What the person confirms for a delete before it runs (AG-63): the Endpoint, the ids and the
/// reason that goes to the gateway, so a reason the model made up is the person's to decline.
pub fn question(input: &Value) -> Option<String> {
    let input: DataWriteInput = parse_input(input.clone()).ok()?;
    let reason = delete_reason(&input).filter(|_| input.action == Action::Delete)?;
    let shown: Vec<&str> = input.ids.iter().take(3).map(String::as_str).collect();
    let more = input.ids.len().saturating_sub(shown.len());
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Some(format!(
        "Deletes {}{more} through Endpoint {}, with the reason \"{reason}\".",
        shown.join(", "),
        input.endpoint
    ))
}

/// The broker's entity operation of `action`, and its body.
fn request_of(input: &DataWriteInput) -> (&'static str, Value) {
    match input.action {
        Action::Create => ("ngsi-ld/v1/entityOperations/create", json!(input.entities)),
        Action::Upsert => (
            "ngsi-ld/v1/entityOperations/upsert?options=update",
            json!(input.entities),
        ),
        Action::Merge => ("ngsi-ld/v1/entityOperations/merge", json!(input.entities)),
        Action::Delete => ("ngsi-ld/v1/entityOperations/delete", json!(input.ids)),
    }
}

/// The ids a call named, in its order.
fn named_ids(input: &DataWriteInput) -> Vec<String> {
    if input.action == Action::Delete {
        return input.ids.clone();
    }
    input
        .entities
        .iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// The operation a Policy must grant for `action`: the operation group `updateOps` holds none of
/// these, so a refusal names it (T-6451).
fn operation_of(action: Action) -> &'static str {
    match action {
        Action::Create => "createBatch",
        Action::Upsert => "upsertBatch",
        Action::Merge => "mergeBatch",
        Action::Delete => "deleteBatch",
    }
}

/// The answer from the gateway's: every id written on 201/204, the broker's own split on 207,
/// and a refusal the caller can act on for everything else.
fn answer_of(
    name: &str,
    status: u16,
    body: &Value,
    action: Action,
    ids: Vec<String>,
) -> Result<Value, OpError> {
    match status {
        201 | 204 => Ok(json!({ "written": ids, "errors": [] })),
        207 => Ok(json!({
            "written": body.get("success").cloned().unwrap_or_else(|| json!([])),
            "errors": body.get("errors").cloned().unwrap_or_else(|| json!([])),
        })),
        400 => Err(OpError::Api(ApiError::BadRequest(format!(
            "Endpoint {name} refused the entities: {}",
            detail_of(body)
        )))),
        401 | 403 => Err(OpError::Api(ApiError::Denied(format!(
            "Endpoint {name}'s Policy does not let you write this: {}; this write needs a Policy \
             assigned to you that grants the operation {} on these types",
            detail_of(body),
            operation_of(action)
        )))),
        404 => Err(OpError::Api(ApiError::NotFound(format!(
            "the gateway does not serve writes on Endpoint {name}: {}",
            detail_of(body)
        )))),
        409 => Err(OpError::Api(ApiError::Conflict(format!(
            "Endpoint {name}: {}; upsert writes an entity that exists",
            detail_of(body)
        )))),
        _ => {
            tracing::warn!(status, endpoint = name, "the gateway failed a data write");
            Err(OpError::Api(ApiError::Unavailable(format!(
                "the gateway answered {status} for Endpoint {name}; nothing is known written, read before writing again"
            ))))
        }
    }
}

async fn run(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: DataWriteInput,
) -> Result<Value, OpError> {
    checked(&input)?;
    let name = &input.endpoint;
    let slug = match readable_slug(caller, state, project, name)? {
        Target::Own(slug) => slug,
        // Shared with the caller to read, never theirs to write: as missing as it was before.
        Target::Shared(_) => return Err(super::data_query::unshared(project, name)),
        Target::Public(_) => {
            return Err(OpError::Api(ApiError::NotFound(format!(
                "no Endpoint {name} in project {project}: a public Endpoint of another project is read, never written"
            ))))
        }
    };
    let bearer = caller.bearer.as_ref().ok_or_else(|| {
        ApiError::Denied(
            "jc_data_write writes as you with the token your MCP client presented: call it \
             through /api/v1/mcp or a connector"
                .into(),
        )
    })?;
    let base = state.config.gateway_url.as_deref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no gateway address (JC_PORTAL_GATEWAY_URL)".into())
    })?;
    let oidc = crate::auth::realms::exchanger(state, &bearer.0).await?;
    let token = oidc.exchange_for_endpoint(&bearer.0, &slug).await?;
    let (path, body) = request_of(&input);
    let mut request = crate::api::pipelines::http()
        .post(format!(
            "{}/api/endpoint/{slug}/{path}",
            base.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("accept", "application/json");
    // Encoded once, so a line break in it is `%0A` and never a header of its own.
    if let Some(reason) = delete_reason(&input).filter(|_| input.action == Action::Delete) {
        request = request.header("JC-Reason", reason_header(reason));
    }
    let answer = request
        .json(&body)
        .timeout(GATEWAY_TIMEOUT)
        .send()
        .await
        .map_err(|err| {
            tracing::warn!(error = %err.without_url(), endpoint = slug, "the gateway did not answer a data write");
            ApiError::Unavailable(
                "the gateway did not answer; nothing is known written, read before writing again"
                    .into(),
            )
        })?;
    let status = answer.status().as_u16();
    let bytes = capped_page(answer).await?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    super::data_query::not_admitted(status, caller, state, project, name)?;
    answer_of(name, status, &body, input.action, named_ids(&input))
}

pub fn operations() -> Vec<Operation> {
    vec![Operation {
        name: NAME,
        title: "Write Data",
        description: "Creates, upserts, merges or deletes up to 100 entities through one Endpoint, as you; its Policy decides each",
        input: input_schema,
        output: output_schema,
        annotations: OperationAnnotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: false,
        },
        // The Portal's grant is reading the Endpoint (its manifest, in its space); what may be
        // written is the Endpoint Policy's, decided by the gateway per entity.
        kind: "Endpoint",
        verb: Some(jc_core::kinds::Verb::Read),
        lane: Lane::Yellow,
        validate: validate_write,
        run: |caller, state, project, val| {
            Box::pin(async move { run(caller, state, project, parse_input(val)?).await })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(args: Value) -> DataWriteInput {
        parse_input(args).expect("input")
    }

    #[test]
    #[should_panic(expected = "\"bikes\"")]
    fn a_valid_write_is_no_refusal() {
        refused_at(json!({ "endpoint": "bikes", "action": "delete",
                           "ids": ["urn:ngsi-ld:T:1"], "reason": "a duplicate" }));
    }

    fn refused_at(args: Value) -> String {
        let result = validate_write(&args);
        let Err(OpError::InvalidInput { path, .. }) = result else {
            panic!("{args}: {result:?}")
        };
        path
    }

    #[test]
    fn each_action_names_what_it_writes() {
        let station = json!({ "id": "urn:ngsi-ld:BikeHireDockingStation:s1", "type": "BikeHireDockingStation" });
        assert!(validate_write(
            &json!({ "endpoint": "bikes", "action": "create", "entities": [station] })
        )
        .is_ok());
        assert!(
            validate_write(&json!({ "endpoint": "bikes", "action": "delete",
                                        "ids": ["urn:ngsi-ld:T:1"], "reason": "a duplicate" }))
            .is_ok()
        );
        assert_eq!(
            refused_at(json!({ "endpoint": "bikes", "action": "create" })),
            "/entities"
        );
        assert_eq!(
            refused_at(json!({ "endpoint": "bikes", "action": "delete", "entities": [station] })),
            "/ids"
        );
        assert_eq!(
            refused_at(
                json!({ "endpoint": "bikes", "action": "merge", "entities": [{ "id": "x" }] })
            ),
            "/entities/0/type"
        );
        assert_eq!(
            refused_at(json!({ "endpoint": "bikes", "action": "delete", "ids": [" "] })),
            "/ids/0"
        );
        let many: Vec<Value> = (0..=MAX_ENTITIES)
            .map(|n| json!({ "id": format!("urn:ngsi-ld:T:{n}"), "type": "T" }))
            .collect();
        assert_eq!(
            refused_at(json!({ "endpoint": "bikes", "action": "upsert", "entities": many })),
            "/entities"
        );
        assert!(
            validate_write(&json!({ "endpoint": "bikes", "action": "replace", "ids": ["a"] }))
                .is_err()
        );
    }

    #[test]
    fn a_delete_names_the_persons_reason_and_nothing_cuts_it() {
        let delete = |reason: Value| json!({ "endpoint": "bikes", "action": "delete", "ids": ["urn:ngsi-ld:T:1"], "reason": reason });
        let bare = json!({ "endpoint": "bikes", "action": "delete", "ids": ["urn:ngsi-ld:T:1"] });
        assert_eq!(refused_at(bare), "/reason");
        assert_eq!(refused_at(delete(json!(""))), "/reason");
        assert_eq!(refused_at(delete(json!(" \n\t "))), "/reason");
        // Counted as the gateway counts: characters once trimmed, not bytes.
        assert_eq!(
            refused_at(delete(json!("ž".repeat(MAX_REASON + 1)))),
            "/reason"
        );
        assert!(validate_write(&delete(json!(format!("  {}\n", "ž".repeat(MAX_REASON))))).is_ok());
        // A credential quoted in the reason would be kept with the delete.
        let token = format!("ghp_{}", "0123456789abcdef".repeat(3));
        assert_eq!(
            refused_at(delete(json!(format!("rotated {token}")))),
            "/reason"
        );
        // Other actions ignore it.
        let station = json!({ "id": "urn:ngsi-ld:T:1", "type": "T" });
        assert!(validate_write(
            &json!({ "endpoint": "bikes", "action": "create", "entities": [station] })
        )
        .is_ok());

        let call = input(delete(json!("  line one\r\nJC-Record: version  ")));
        let reason = delete_reason(&call).expect("reason");
        assert_eq!(reason, "line one\r\nJC-Record: version");
        let header = reason_header(reason);
        assert!(!header.contains(['\r', '\n', ' ', ':']), "{header}");
        assert_eq!(header, "line%20one%0D%0AJC-Record%3A%20version");
        assert!(reqwest::header::HeaderValue::from_str(&header).is_ok());
    }

    #[test]
    fn the_person_confirms_the_reason_a_delete_sends() {
        let ids: Vec<String> = (1..=5).map(|n| format!("urn:ngsi-ld:T:{n}")).collect();
        let asked = question(
            &json!({ "endpoint": "bikes", "action": "delete", "ids": ids,
                                      "reason": " duplicate of station 042 " }),
        )
        .expect("a question");
        assert!(asked.contains("urn:ngsi-ld:T:3 and 2 more"), "{asked}");
        assert!(asked.contains("Endpoint bikes"), "{asked}");
        assert!(asked.contains("\"duplicate of station 042\""), "{asked}");
        let station = json!({ "id": "urn:ngsi-ld:T:1", "type": "T" });
        assert_eq!(
            question(
                &json!({ "endpoint": "bikes", "action": "create", "entities": [station], "reason": "x" })
            ),
            None
        );
        assert_eq!(
            question(
                &json!({ "endpoint": "bikes", "action": "delete", "ids": ["urn:ngsi-ld:T:1"] })
            ),
            None
        );
    }

    #[test]
    fn each_action_is_one_broker_entity_operation() {
        let entities = json!([{ "id": "urn:ngsi-ld:T:1", "type": "T" }]);
        for (action, path) in [
            ("create", "ngsi-ld/v1/entityOperations/create"),
            (
                "upsert",
                "ngsi-ld/v1/entityOperations/upsert?options=update",
            ),
            ("merge", "ngsi-ld/v1/entityOperations/merge"),
        ] {
            let call = input(json!({ "endpoint": "e", "action": action, "entities": entities }));
            assert_eq!(request_of(&call), (path, entities.clone()));
        }
        let delete =
            input(json!({ "endpoint": "e", "action": "delete", "ids": ["urn:ngsi-ld:T:1"] }));
        assert_eq!(
            request_of(&delete),
            (
                "ngsi-ld/v1/entityOperations/delete",
                json!(["urn:ngsi-ld:T:1"])
            )
        );
    }

    #[test]
    fn the_gateways_answer_becomes_what_was_written_or_why_not() {
        let ids = vec!["a".to_owned()];
        assert_eq!(
            answer_of("e", 201, &Value::Null, Action::Upsert, ids.clone()).expect("ok")["written"],
            json!(["a"])
        );
        let split = json!({ "success": ["a"], "errors": [{ "entityId": "b", "error": { "title": "Forbidden" } }] });
        let answer = answer_of("e", 207, &split, Action::Upsert, ids.clone()).expect("ok");
        assert_eq!(answer["errors"][0]["entityId"], "b");
        let detail = json!({ "detail": "the Policy grants no write of type T" });
        let got = answer_of("e", 403, &detail, Action::Upsert, ids.clone());
        assert!(
            matches!(&got, Err(OpError::Api(ApiError::Denied(why)))
                if why.contains("no write of type T") && why.contains("the operation upsertBatch")),
            "{got:?}"
        );
        assert!(matches!(
            answer_of("e", 409, &detail, Action::Upsert, ids.clone()),
            Err(OpError::Api(ApiError::Conflict(_)))
        ));
        assert!(matches!(
            answer_of("e", 502, &Value::Null, Action::Upsert, ids),
            Err(OpError::Api(ApiError::Unavailable(_)))
        ));
    }

    /// Each action is its broker operation with its body: entities, or the ids a delete names.
    #[test]
    fn each_action_is_its_broker_operation_with_its_ids() {
        let entity = json!({ "id": "urn:ngsi-ld:Bus:1", "type": "Bus" });
        for (action, path) in [
            ("create", "ngsi-ld/v1/entityOperations/create"),
            (
                "upsert",
                "ngsi-ld/v1/entityOperations/upsert?options=update",
            ),
            ("merge", "ngsi-ld/v1/entityOperations/merge"),
        ] {
            let call = input(
                json!({ "endpoint": "bus", "action": action, "entities": [entity, { "type": "Bus" }] }),
            );
            assert_eq!(
                request_of(&call),
                (path, json!([entity, { "type": "Bus" }]))
            );
            assert_eq!(
                named_ids(&call),
                ["urn:ngsi-ld:Bus:1"],
                "an entity without an id names none"
            );
        }
        let delete = input(
            json!({ "endpoint": "bus", "action": "delete", "ids": ["urn:ngsi-ld:Bus:2"], "reason": "sold" }),
        );
        assert_eq!(
            request_of(&delete),
            (
                "ngsi-ld/v1/entityOperations/delete",
                json!(["urn:ngsi-ld:Bus:2"])
            )
        );
        assert_eq!(named_ids(&delete), ["urn:ngsi-ld:Bus:2"]);
    }

    /// The gateway's answer: every id on 201/204, the broker's own split on 207 (empty lists
    /// when it names none), and a refusal naming the Endpoint and what to do for the rest.
    #[test]
    fn the_gateways_answer_is_the_written_ids_or_a_refusal_to_act_on() {
        let ids = || vec!["urn:ngsi-ld:Bus:1".to_owned()];
        for status in [201, 204] {
            assert_eq!(
                answer_of("bus", status, &Value::Null, Action::Create, ids()).expect("written"),
                json!({ "written": ["urn:ngsi-ld:Bus:1"], "errors": [] })
            );
        }
        let split = json!({ "success": ["urn:ngsi-ld:Bus:1"], "errors": [{ "entityId": "urn:ngsi-ld:Bus:2" }] });
        assert_eq!(
            answer_of("bus", 207, &split, Action::Upsert, ids()).expect("split"),
            json!({ "written": ["urn:ngsi-ld:Bus:1"], "errors": [{ "entityId": "urn:ngsi-ld:Bus:2" }] })
        );
        assert_eq!(
            answer_of("bus", 207, &Value::Null, Action::Upsert, ids()).expect("split"),
            json!({ "written": [], "errors": [] })
        );
        let why = json!({ "detail": "speed is no number" });
        let answer = answer_of("bus", 400, &why, Action::Create, ids());
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::BadRequest(m))) if *m == "Endpoint bus refused the entities: speed is no number"),
            "{answer:?}"
        );
        for (status, action, operation) in [
            (401, Action::Merge, "mergeBatch"),
            (403, Action::Delete, "deleteBatch"),
            (403, Action::Create, "createBatch"),
            (403, Action::Upsert, "upsertBatch"),
        ] {
            let answer = answer_of(
                "bus",
                status,
                &json!({ "title": "Forbidden" }),
                action,
                ids(),
            );
            assert!(
                matches!(&answer, Err(OpError::Api(ApiError::Denied(m))) if m.starts_with(
                            "Endpoint bus's Policy does not let you write this: Forbidden;"
                        ) && m.contains(&format!("grants the operation {operation} "))),
                "{answer:?}"
            );
        }
        let answer = answer_of("bus", 404, &Value::Null, Action::Create, ids());
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::NotFound(m))) if *m == "the gateway does not serve writes on Endpoint bus: no reason given"),
            "{answer:?}"
        );
        let answer = answer_of(
            "bus",
            409,
            &json!({ "detail": "exists" }),
            Action::Create,
            ids(),
        );
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Conflict(m))) if *m == "Endpoint bus: exists; upsert writes an entity that exists"),
            "{answer:?}"
        );
        let answer = answer_of("bus", 502, &Value::Null, Action::Create, ids());
        assert!(
            matches!(&answer, Err(OpError::Api(ApiError::Unavailable(m))) if m.starts_with(
                "the gateway answered 502 for Endpoint bus; nothing is known written"
            )),
            "{answer:?}"
        );
    }

    /// A delete through the caller's own Endpoint goes to the gateway as the person, with the
    /// reason they gave as `JC-Reason`, and answers the ids written; a gateway that does not
    /// answer is a 503 that says nothing is known written.
    #[tokio::test]
    async fn an_own_delete_sends_its_reason_and_a_silent_gateway_is_unavailable() {
        use wiremock::matchers::{header_exists, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let slug = "w7m2kd9x4wpz8nhr3vbt6cfy5b";
        let realm = crate::auth::oidc::tests::mock_realm().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "endpoint-token", "token_type": "Bearer", "expires_in": 300
            })))
            .mount(&realm)
            .await;
        let gateway = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/delete"
            )))
            .and(header_exists("JC-Reason"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&gateway)
            .await;
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::auth::oidc::tests::config_for(&realm),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");
        let client = std::sync::Arc::new(client);
        let on = |gateway: String| {
            let mut config = crate::config::Config::for_tests();
            config.gateway_url = Some(gateway);
            let mut state = AppState::new(config, None);
            state.oidc = Some(client.clone());
            state.mirror.upsert(crate::ops::testkit::envelope(
                "Endpoint",
                "counters",
                "doprava",
                json!({ "slug": slug, "audience": "project" }),
            ));
            state
        };
        let state = on(gateway.uri());
        let mut caller = crate::ops::testkit::admin(&state);
        caller.bearer = Some(super::super::Bearer("person-token".into()));
        let delete = || {
            input(json!({ "endpoint": "counters", "action": "delete",
                          "ids": ["urn:ngsi-ld:T:1"], "reason": "a duplicate\nof T:2" }))
        };
        let written = run(&caller, &state, "doprava", delete())
            .await
            .expect("written");
        assert_eq!(written["written"], json!(["urn:ngsi-ld:T:1"]), "{written}");
        gateway.verify().await;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = closed.local_addr().expect("addr").port();
        drop(closed);
        let silent = on(format!("http://127.0.0.1:{port}"));
        let err = run(&caller, &silent, "doprava", delete())
            .await
            .expect_err("nobody answers");
        assert!(
            matches!(&err, OpError::Api(ApiError::Unavailable(why)) if why.contains("nothing is known written")),
            "{err:?}"
        );
        // A merge carries no JC-Reason, and the silent gateway is the same 503 for it.
        let merge = input(json!({ "endpoint": "counters", "action": "merge",
                                  "entities": [{ "id": "urn:ngsi-ld:T:1", "type": "T" }] }));
        let err = run(&caller, &silent, "doprava", merge)
            .await
            .expect_err("nobody answers a merge");
        assert!(
            matches!(&err, OpError::Api(ApiError::Unavailable(why)) if why.contains("nothing is known written")),
            "{err:?}"
        );
    }

    /// A public Endpoint of another project is read, never written, and one the caller may only
    /// read as shared answers as missing: neither write reaches the gateway.
    #[tokio::test]
    async fn a_public_or_shared_endpoint_is_never_written() {
        let state = crate::ops::testkit::state();
        let public = "zt4qm7ge2xdv6ksb3ncf5arw2y";
        let shared = "q7m2kd9x4wpz8nhr3vbt6cfy5a";
        state.mirror.upsert(crate::ops::testkit::envelope(
            "Endpoint",
            "stops",
            "helsinki",
            json!({ "slug": public, "audience": "public" }),
        ));
        state.mirror.upsert(crate::ops::testkit::envelope(
            "Endpoint",
            "staff",
            "espoo",
            json!({ "slug": shared, "audience": "project" }),
        ));
        let caller = crate::ops::testkit::caller(
            crate::ops::testkit::person("ana", &[]),
            crate::ops::Via::Mcp,
        );
        let write = |endpoint: &str| {
            input(json!({ "endpoint": endpoint, "action": "delete",
                          "ids": ["urn:ngsi-ld:T:1"], "reason": "a duplicate" }))
        };
        let err = run(&caller, &state, "espoo", write(public))
            .await
            .expect_err("public");
        assert!(err.to_string().contains("is read, never written"), "{err}");
        let err = run(&caller, &state, "espoo", write("staff"))
            .await
            .expect_err("shared");
        assert!(
            matches!(&err, OpError::Api(ApiError::NotFound(why)) if why.starts_with("no Endpoint staff in project espoo")),
            "{err:?}"
        );
    }
}
