//! Who may do what, as the Policies page, the Access explorer and the publish flow answer it,
//! through the registry (JC2-29, JC2-33).
//!
//! The caller's own rules, a Policy tried on another subject, what a subject reaches, why one
//! attribute is refused, one page under a draft filter and the catalogue draft of an Endpoint had
//! browser routes and no operations. Each operation calls the route's own function, so the
//! grants, the organization-administrator check of the two questions about someone else and the
//! answers stay the route's. Nothing here writes.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::bounds::{text, texts, ID, NAME, QUERY, TERM};
use super::runs::as_user;
use super::{annotations, parse_input, OpError, Operation};
use crate::api::access_attributes::Asked;
use crate::api::access_explore::ExploreRequest;
use crate::api::access_simulate::{SimulateRequest, Who};
use crate::api::catalogue_draft::CatalogueDraftRequest;
use crate::api::endpoint_preview::PreviewRequest;
use crate::change::Lane;

pub const PERMISSIONS: &str = "jc_permissions_me";
pub const SIMULATE: &str = "jc_access_simulate";
pub const EXPLORE: &str = "jc_access_explore";
pub const EXPLAIN: &str = "jc_attribute_explain";
pub const PREVIEW: &str = "jc_endpoint_preview";
pub const CATALOGUE_DRAFT: &str = "jc_catalogue_draft";

/// `endpoint` is the Endpoint, beside the route's own body: a Policy question, held to the
/// Endpoints a narrowed connector lists by that member.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimulateInput {
    endpoint: String,
    subject: Who,
    action: String,
    #[serde(default, rename = "type")]
    entity_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainInput {
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    attribute: String,
    #[serde(default)]
    action: Option<String>,
}

/// The route's own type refuses an unknown member of the draft (`preview_request`).
#[derive(Debug, Deserialize)]
struct PreviewInput {
    name: String,
    #[serde(flatten)]
    draft: Value,
}

fn subject_schema() -> Value {
    json!({
        "type": "object",
        "description": "person takes id; group, role, serviceAccount take name",
        "properties": {
            "kind": { "type": "string", "enum": ["person", "group", "role", "serviceAccount", "public"], "description": "Subject kind" },
            "id": text("Keycloak user id", ID),
            "name": text("e.g. city-stewards", NAME)
        },
        "required": ["kind"],
        "additionalProperties": false
    })
}

fn endpoint_name() -> Value {
    text("Endpoint name, e.g. air-public", NAME)
}

fn permissions_input_schema() -> Value {
    super::empty_input_schema()
}

fn simulate_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "endpoint": endpoint_name(),
            "subject": subject_schema(),
            "action": text("A CIM 009 operation, e.g. retrieveEntity", TERM),
            "type": text("Entity type; absent: any type", TERM)
        },
        "required": ["endpoint", "subject", "action"],
        "additionalProperties": false
    })
}

fn explore_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "subject": subject_schema() },
        "required": ["subject"],
        "additionalProperties": false
    })
}

fn explain_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": endpoint_name(),
            "type": text("e.g. AirQualityObserved", TERM),
            "attribute": text("e.g. pm10", TERM),
            "action": { "type": "string", "enum": ["read", "write", "delete"], "description": "default write" }
        },
        "required": ["name", "type", "attribute"],
        "additionalProperties": false
    })
}

fn preview_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": endpoint_name(),
            "type": text("e.g. AirQualityObserved", TERM),
            "projection": {
                "type": "object",
                "description": "Draft spec.projection",
                "properties": {
                    "classes": {
                        "type": "array",
                        "maxItems": 64,
                        "description": "Classes served",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": text("Class", TERM),
                                "slots": texts("e.g. [pm10]", TERM)
                            },
                            "required": ["name"],
                            "additionalProperties": false
                        }
                    },
                    "filter": {
                        "type": "object",
                        "description": "NGSI-LD query parts",
                        "properties": {
                            "q": text("e.g. pm10>40", QUERY),
                            "scopeQ": text("e.g. /city/#", QUERY),
                            "geoQ": text("e.g. near;maxDistance==500;Point;[19.1,48.7]", QUERY),
                            "temporalQ": text("e.g. after;2026-10-01T00:00:00Z", QUERY)
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["classes"],
                "additionalProperties": false
            },
            "hiddenAttributes": texts("Draft hidden attributes", TERM),
            "id": { "type": "array", "maxItems": 100, "items": { "type": "string", "maxLength": ID }, "description": "Only these entity ids" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "default 50" },
            "offset": { "type": "integer", "minimum": 0, "description": "Entities skipped" }
        },
        "required": ["name", "type"],
        "additionalProperties": false
    })
}

fn catalogue_draft_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "endpoint": endpoint_name() },
        "required": ["endpoint"],
        "additionalProperties": false
    })
}

fn permissions_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string" },
            "bootstrap": { "type": "boolean" },
            "grants": { "type": "array", "description": "Verbs on kinds and the bindings they come from", "items": { "type": "object" } },
            "projects": { "type": "object", "description": "Whether you may open projects" }
        },
        "required": ["project", "bootstrap", "grants"]
    })
}

fn simulate_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "decision": { "type": "boolean" },
            "reason": { "type": "string" },
            "policy": { "type": "string", "description": "The Policy that decided" },
            "assigner": { "type": "string" },
            "subject": { "type": "object", "description": "The subject as the gateway resolved it" }
        },
        "required": ["decision", "reason", "subject"]
    })
}

fn explore_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "subject": { "type": "object" },
            "endpoints": {
                "type": "array",
                "description": "Each Endpoint the subject reaches with its Policies and grant document",
                "items": { "type": "object" }
            },
            "mcpServers": { "type": "array", "items": { "type": "string" } },
            "consents": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["subject", "endpoints", "mcpServers", "consents"]
    })
}

fn explain_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "type": "string" },
            "attribute": { "type": "string" },
            "action": { "type": "string" },
            "decision": { "type": ["boolean", "null"] },
            "reason": { "type": ["string", "null"] },
            "policy": { "type": ["string", "null"], "description": "The deciding Policy, where you may read it" },
            "policyTitle": { "type": ["string", "null"] }
        },
        "required": ["type", "attribute", "action"]
    })
}

fn preview_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "count": { "type": "integer" },
            "entities": { "type": "array", "description": "One page under the draft", "items": { "type": "object" } }
        },
        "required": ["count", "entities"]
    })
}

fn catalogue_draft_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "endpoint": { "type": "string" },
            "makesPublic": { "type": "boolean", "description": "Publishing makes the Endpoint public (red lane)" },
            "catalog": { "type": "object", "description": "The drafted spec.catalog" },
            "publish": { "type": "object", "description": "The drafted spec.publish" },
            "missing": { "type": "array", "items": { "type": "string" } },
            "attributes": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["endpoint", "makesPublic", "catalog", "publish", "missing"]
    })
}

fn validate_as<T: serde::de::DeserializeOwned>(val: &Value) -> Result<(), OpError> {
    parse_input::<T>(val.clone()).map(|_| ())
}

/// The route's preview body: the draft members as the route reads them, checked by the route's
/// own type before anything is asked of the broker.
fn preview_request(draft: Value) -> Result<PreviewRequest, OpError> {
    parse_input(draft)
}

fn validate_preview(val: &Value) -> Result<(), OpError> {
    let input: PreviewInput = parse_input(val.clone())?;
    preview_request(input.draft).map(|_| ())
}

async fn simulate(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: SimulateInput,
) -> Result<Value, OpError> {
    let Json(simulated) = crate::api::access_simulate::simulate_access(
        as_user(caller),
        State(state.clone()),
        Path((project.to_owned(), input.endpoint)),
        Ok(Json(SimulateRequest {
            subject: input.subject,
            action: input.action,
            entity_type: input.entity_type,
        })),
    )
    .await?;
    Ok(serde_json::to_value(simulated)?)
}

async fn explain(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: ExplainInput,
) -> Result<Value, OpError> {
    let Json(explained) = crate::api::access_attributes::explain_attribute(
        as_user(caller),
        State(state.clone()),
        Path((project.to_owned(), input.name)),
        Ok(Query(Asked {
            entity_type: input.entity_type,
            attribute: input.attribute,
            action: input.action,
        })),
    )
    .await?;
    Ok(serde_json::to_value(explained)?)
}

async fn preview(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: PreviewInput,
) -> Result<Value, OpError> {
    let request = preview_request(input.draft)?;
    let Json(page) = crate::api::endpoint_preview::preview_endpoint(
        as_user(caller),
        State(state.clone()),
        Path((project.to_owned(), input.name)),
        Ok(Json(request)),
    )
    .await?;
    Ok(serde_json::to_value(page)?)
}

/// An operation's input and output schemas.
type Schemas = (fn() -> Value, fn() -> Value);

/// A read about access: green, idempotent, gated by the route.
fn read(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    kind: &'static str,
    schemas: Schemas,
    validate: fn(&Value) -> Result<(), OpError>,
    run: super::RunFn,
) -> Operation {
    Operation {
        name,
        title,
        description,
        input: schemas.0,
        output: schemas.1,
        annotations: annotations(true, false, true),
        kind,
        verb: None,
        lane: Lane::Green,
        validate,
        run,
    }
}

pub fn operations() -> Vec<Operation> {
    vec![
        read(
            PERMISSIONS,
            "Read My Permissions",
            "Your effective rules in the project: which verbs on which kinds, from which bindings",
            "*",
            (permissions_input_schema, permissions_output_schema),
            validate_as::<super::EmptyInput>,
            |caller, state, project, _val| {
                Box::pin(async move {
                    let Json(effective) = crate::api::permissions::permissions_me(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                    )
                    .await?;
                    Ok(serde_json::to_value(effective)?)
                })
            },
        ),
        read(
            SIMULATE,
            "Try A Policy",
            "What the gateway decides for a person, group, role, ServiceAccount or the public doing one action on an Endpoint, and the Policy that decides; organization administrators",
            "Policy",
            (simulate_input_schema, simulate_output_schema),
            validate_as::<SimulateInput>,
            |caller, state, project, val| {
                Box::pin(async move { simulate(caller, state, project, parse_input(val)?).await })
            },
        ),
        read(
            EXPLORE,
            "Explore Access",
            "Every Endpoint of the project a subject reaches, with its Policies and the gateway's grant document; organization administrators",
            "Policy",
            (explore_input_schema, explore_output_schema),
            validate_as::<ExploreRequest>,
            |caller, state, project, val| {
                Box::pin(async move {
                    let Json(explored) = crate::api::access_explore::explore_access(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Ok(Json(parse_input(val)?)),
                    )
                    .await?;
                    Ok(serde_json::to_value(explored)?)
                })
            },
        ),
        read(
            EXPLAIN,
            "Why An Attribute Is Refused",
            "What decided one attribute of an Endpoint for you and, if you may read it, the Policy that decided",
            "Endpoint",
            (explain_input_schema, explain_output_schema),
            validate_as::<ExplainInput>,
            |caller, state, project, val| {
                Box::pin(async move { explain(caller, state, project, parse_input(val)?).await })
            },
        ),
        read(
            PREVIEW,
            "Preview An Endpoint Filter",
            "The count and one page of an Endpoint's entities under a draft projection and hidden attributes, before proposing them; writes nothing",
            "Endpoint",
            (preview_input_schema, preview_output_schema),
            validate_preview,
            |caller, state, project, val| {
                Box::pin(async move { preview(caller, state, project, parse_input(val)?).await })
            },
        ),
        read(
            CATALOGUE_DRAFT,
            "Draft A Catalogue Entry",
            "The spec.catalog and spec.publish the publish flow proposes for an Endpoint, and whether publishing makes it public; writes nothing",
            "Endpoint",
            (catalogue_draft_input_schema, catalogue_draft_output_schema),
            validate_as::<CatalogueDraftRequest>,
            |caller, state, project, val| {
                Box::pin(async move {
                    let Json(draft) = crate::api::catalogue_draft::draft_publication(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Json(parse_input(val)?),
                    )
                    .await?;
                    Ok(serde_json::to_value(draft)?)
                })
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_input_is_the_routes_own_body_beside_the_endpoint() {
        assert!(validate_as::<SimulateInput>(&json!({
            "endpoint": "air", "subject": { "kind": "group", "name": "city-stewards" }, "action": "retrieveEntity"
        }))
        .is_ok());
        assert!(validate_as::<SimulateInput>(&json!({
            "endpoint": "air", "subject": { "kind": "public" }, "action": "retrieveEntity", "type": "Bike"
        }))
        .is_ok());
        // A subject of no known kind, or one carrying another kind's member, is refused.
        for subject in [
            json!({ "kind": "admin" }),
            json!({ "kind": "public", "id": "x" }),
        ] {
            assert!(validate_as::<SimulateInput>(&json!({
                "endpoint": "air", "subject": subject, "action": "retrieveEntity"
            }))
            .is_err());
        }
        assert!(validate_as::<ExploreRequest>(
            &json!({ "subject": { "kind": "person", "id": "u-1" } })
        )
        .is_ok());
        assert!(validate_as::<ExplainInput>(
            &json!({ "name": "air", "type": "T", "attribute": "pm10" })
        )
        .is_ok());
        assert!(validate_as::<CatalogueDraftRequest>(&json!({ "endpoint": "air" })).is_ok());
        assert!(
            validate_as::<CatalogueDraftRequest>(&json!({ "endpoint": "air", "x": 1 })).is_err()
        );
    }

    #[test]
    fn a_preview_keeps_the_routes_draft_checks() {
        assert!(validate_preview(&json!({ "name": "air", "type": "AirQualityObserved" })).is_ok());
        assert!(validate_preview(&json!({
            "name": "air", "type": "T", "hiddenAttributes": ["owner"], "limit": 10,
            "projection": { "classes": [{ "name": "T", "slots": ["pm10"] }], "filter": { "q": "pm10>40" } }
        }))
        .is_ok());
        assert!(validate_preview(&json!({ "name": "air" })).is_err());
        assert!(
            validate_preview(&json!({ "name": "air", "type": "T", "url": "http://x" })).is_err()
        );
    }
}
