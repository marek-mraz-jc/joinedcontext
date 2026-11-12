//! The registry's operations on the Organization's OData service (API/13 §2b, T-6815): every
//! operation that is no entity set is a function or an action bound to a project, generated from
//! the registry, so no operation has a handler of its own here.
//!
//! - A read-only operation with arguments is a bound function, called with `GET`, and so is an
//!   entity read whose set has not landed yet ([`SERVED_AS_SETS`]):
//!   `projects('air')/JoinedContext.Portal.jc_schema_get(kind='Endpoint')`. A value holding `/`
//!   or a JSON object goes in a parameter alias:
//!   `…/JoinedContext.Portal.jc_catalog_search(query=@q)?@q='air/quality'`.
//! - Every other operation is a bound action, called with `POST` and its input as the JSON body:
//!   `projects('air')/JoinedContext.Portal.jc_run_create`. A Change-proposing one (API/13 §2b)
//!   needs `If-Match` with the project's `@odata.etag`; a run, a draft or a dry run does not.
//!   `jc_resource_propose`, `jc_resource_delete`, `jc_change_approve` and `jc_change_reject` are
//!   the entity-bound `propose_*`, `approve` and `reject` of [`crate::api::odata_org`].
//!
//! Each call runs [`ops::call`] as the REST door `POST …/ops/{name}` runs it: the registry's
//! permission check, its lane and its answer, for the project the caller may read in this
//! Organization (any other is not there, 404). `$metadata` lists only what the caller may run in
//! one of those projects ([`ops::listing`]).

use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};

use crate::api::odata::{edm_type, identifier, NAMESPACE};
use crate::api::ops::{caller_of, respond};
use crate::auth::session::{CurrentUser, Front};
use crate::ops::{self, Operation};
use crate::state::AppState;

/// API/13 §2b, entity set, singleton or navigation: read as sets, never called here.
pub const ENTITY_READS: [&str; 27] = [
    "jc_resource_list",
    "jc_resource_get",
    "jc_project_get",
    "jc_project_revisions",
    "jc_endpoint_list_all",
    "jc_change_list",
    "jc_change_get",
    "jc_run_list",
    "jc_run_get",
    "jc_draft_list",
    "jc_draft_get",
    "jc_workspace_list",
    "jc_workspace_get",
    "jc_activity_list",
    "jc_member_list",
    "jc_person_list",
    "jc_person_get",
    "jc_access_request_list",
    "jc_organization_read",
    "jc_permissions_me",
    "jc_repository_list",
    "jc_repository_get",
    "jc_repository_commits",
    "jc_repository_commit",
    "jc_repository_refs",
    "jc_repository_pulls",
    "jc_repository_pull",
];

/// The entity reads the Organization's sets, keys and singletons answer today: `projects`, a set
/// per kind, `changes`, `workspaces`, `runs`, `revisions`, `repositories` and `me`. Every other
/// one of [`ENTITY_READS`] stays a bound function (T-6828): `jc_activity_list` and
/// `jc_access_request_list` rows carry no key, a draft is keyed by kind and name together,
/// `jc_repository_get` answers more than its row (readme, last commit) and the commits, refs and
/// pulls take their repository as an argument; `jc_member_list` and `jc_person_*` answer a
/// project's people and the realm's, which the Organization's `members` set is neither of; and
/// `jc_organization_read` reads aspects, not a row.
pub const SERVED_AS_SETS: [&str; 13] = [
    "jc_run_list",
    "jc_run_get",
    "jc_project_revisions",
    "jc_repository_list",
    "jc_resource_list",
    "jc_resource_get",
    "jc_project_get",
    "jc_endpoint_list_all",
    "jc_change_list",
    "jc_change_get",
    "jc_workspace_list",
    "jc_workspace_get",
    "jc_permissions_me",
];

/// API/13 §2b, function: read-only, with arguments.
pub const FUNCTIONS: [&str; 31] = [
    "jc_change_history",
    "jc_catalog_search",
    "jc_knowledge_search",
    "jc_knowledge_status",
    "jc_model_sdm_search",
    "jc_kpi_compute",
    "jc_federation_graph",
    "jc_model_source_get",
    "jc_schema_get",
    "jc_repository_tree",
    "jc_repository_file",
    "jc_repository_settings",
    "jc_ckan_status",
    "jc_syncsource_status",
    "jc_pipeline_metrics",
    "jc_pipeline_logs",
    "jc_pipeline_inspect",
    "jc_pipeline_validate",
    "jc_app_services",
    "jc_space_governance",
    "jc_serviceaccount_access",
    "jc_access_explore",
    "jc_access_simulate",
    "jc_attribute_explain",
    "jc_endpoint_preview",
    "jc_catalogue_draft",
    "jc_workspace_compare",
    "jc_workspace_preview_get",
    "jc_mqtt_captured",
    "jc_project_export",
    "jc_data_query",
];

/// API/13 §2b, Change-proposing action: opens, plans or decides a Change.
pub const PROPOSING: [&str; 15] = [
    "jc_resource_propose",
    "jc_resource_delete",
    "jc_endpoint_propose",
    "jc_model_source_put",
    "jc_project_create",
    "jc_project_delete",
    "jc_project_import",
    "jc_space_complete",
    "jc_flow_start",
    "jc_workspace_propose",
    "jc_access_request_decide",
    "jc_syncsource_detach",
    "jc_change_approve",
    "jc_change_reject",
    "jc_change_apply",
];

/// The Change-proposing operations an entity-bound action of [`crate::api::odata_org`] serves
/// with that entity's `If-Match`; offering them on a project too would skip it.
pub const ENTITY_BOUND: [&str; 4] = [
    "jc_resource_propose",
    "jc_resource_delete",
    "jc_change_approve",
    "jc_change_reject",
];

/// API/13 §2b, other action: no Change; keeps its registry permission and lane.
pub const OTHER_ACTIONS: [&str; 32] = [
    "jc_run_create",
    "jc_run_cancel",
    "jc_run_publish",
    "jc_run_answer",
    "jc_run_message",
    "jc_pipeline_run",
    "jc_syncsource_sync",
    "jc_syncsource_pause",
    "jc_draft_put",
    "jc_draft_drop",
    "jc_workspace_open",
    "jc_workspace_update_from_main",
    "jc_workspace_discard",
    "jc_workspace_preview_start",
    "jc_workspace_preview_stop",
    "jc_change_plan",
    "jc_change_revert_plan",
    "jc_manifest_dry_run",
    "jc_datasource_check",
    "jc_pipeline_test",
    "jc_pipeline_sample_source",
    "jc_pipeline_try_mapping",
    "jc_mqtt_test_publish",
    "jc_model_infer",
    "jc_model_generate",
    "jc_model_sdm_import",
    "jc_person_create",
    "jc_person_edit",
    "jc_person_disable",
    "jc_person_enable",
    "jc_person_sign_out",
    "jc_data_write",
];

/// How a registry operation is called on a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Function,
    Action,
    /// A Change-proposing action: `If-Match` with the project's `@odata.etag`.
    ProposingAction,
}

/// The form `name` takes on a project, or `None` for a read a set answers or an entity-bound
/// action.
pub fn form(name: &str) -> Option<Form> {
    if FUNCTIONS.contains(&name)
        || (ENTITY_READS.contains(&name) && !SERVED_AS_SETS.contains(&name))
    {
        Some(Form::Function)
    } else if OTHER_ACTIONS.contains(&name) {
        Some(Form::Action)
    } else if PROPOSING.contains(&name) && !ENTITY_BOUND.contains(&name) {
        Some(Form::ProposingAction)
    } else {
        None
    }
}

/// The operations the caller may run in a project of `org` they read: what their `$metadata`
/// offers, by [`ops::listing`] (the profile and the route's cheap check, AG-63).
pub(crate) fn offered(
    state: &AppState,
    user: &CurrentUser,
    front: Front,
    projects: &[String],
) -> Vec<&'static Operation> {
    let caller = caller_of(user.clone(), front);
    let mut names = std::collections::BTreeSet::new();
    for project in projects {
        names.extend(
            ops::listing(&caller, state, project)
                .into_iter()
                .map(|summary| summary.name),
        );
    }
    ops::registry()
        .iter()
        .filter(|op| form(op.name).is_some() && identifier(op.name))
        .filter(|op| names.contains(op.name))
        .collect()
}

/// Each input member of `op` with its EDM type and whether it is required.
fn parameters_of(op: &Operation) -> Vec<(String, &'static str, bool)> {
    let input = (op.input)();
    let required: Vec<&str> = input
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    input
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(member, _)| identifier(member))
        .map(|(member, schema)| {
            (
                member.clone(),
                edm_type(schema),
                required.contains(&member.as_str()),
            )
        })
        .collect()
}

/// The `<Function>` and `<Action>` elements of `offered`, each bound to a project. Every name is
/// an identifier and every type one of [`edm_type`]'s, so nothing here needs escaping.
pub(crate) fn xml(offered: &[&'static Operation]) -> String {
    let mut out = String::new();
    for op in offered {
        let element = match form(op.name) {
            Some(Form::Function) => "Function",
            Some(_) => "Action",
            None => continue,
        };
        out.push_str(&format!(
            r#"<{element} Name="{}" IsBound="true"><Parameter Name="bindingParameter" Type="{NAMESPACE}.Project" Nullable="false"/>"#,
            op.name
        ));
        for (member, ty, required) in parameters_of(op) {
            let nullable = if required { r#" Nullable="false""# } else { "" };
            out.push_str(&format!(
                r#"<Parameter Name="{member}" Type="{ty}"{nullable}/>"#
            ));
        }
        out.push_str(&format!(r#"<ReturnType Type="Edm.Untyped"/></{element}>"#));
    }
    out
}

/// The CSDL JSON overload of each of `offered`, by name.
pub(crate) fn json(offered: &[&'static Operation]) -> Vec<(&'static str, Value)> {
    offered
        .iter()
        .filter_map(|op| {
            let kind = match form(op.name)? {
                Form::Function => "Function",
                _ => "Action",
            };
            let mut params = vec![json!({ "$Name": "bindingParameter",
                "$Type": format!("{NAMESPACE}.Project"), "$Nullable": false })];
            params.extend(parameters_of(op).into_iter().map(|(member, ty, required)| {
                json!({ "$Name": member, "$Type": ty, "$Nullable": !required })
            }));
            Some((
                op.name,
                json!({ "$Kind": kind, "$IsBound": true, "$Parameter": params,
                        "$ReturnType": { "$Type": "Edm.Untyped" } }),
            ))
        })
        .collect()
}

/// A refusal before the operation runs: status, problem slug, sentence.
pub(crate) type Refusal = (StatusCode, &'static str, String);

fn bad(message: impl Into<String>) -> Refusal {
    (StatusCode::BAD_REQUEST, "invalid-request", message.into())
}

/// `name(a=1,b=@x)` split into the operation's name and its parameters, aliases resolved from
/// `query` (`@x=...`). A literal is OData's (`'text'` with `''` for a quote, a number, `true`,
/// `false`, `null`); an alias's value may also be JSON (`{"a":1}`, `["x"]`, `"text"`).
pub(crate) fn function_call(
    call: &str,
    query: &str,
) -> Result<(String, Map<String, Value>), Refusal> {
    let call = odata_url::percent_decoded(call)
        .ok_or_else(|| bad("the function call is not valid percent-encoding"))?;
    let (name, args) = match call.split_once('(') {
        Some((name, rest)) => (
            name,
            rest.strip_suffix(')').ok_or_else(|| {
                bad("a function call ends with ')', e.g. jc_schema_get(kind='Endpoint')")
            })?,
        ),
        None => (call.as_str(), ""),
    };
    let mut aliases = Map::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let k = odata_url::percent_decoded(k).unwrap_or_default();
        let Some(alias) = k.strip_prefix('@') else {
            return Err(bad(format!(
                "a function takes its parameters in the call, not {k}; e.g. jc_schema_get(kind=@k)?@k='Endpoint'"
            )));
        };
        let v = odata_url::percent_decoded(v)
            .ok_or_else(|| bad(format!("@{alias} is not valid percent-encoding")))?;
        aliases.insert(alias.to_owned(), alias_value(&v)?);
    }
    let mut params = Map::new();
    for part in split_args(args)? {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| bad(format!("a parameter is name=value, not {part}")))?;
        let k = k.trim();
        if !identifier(k) {
            return Err(bad(format!("{k} is no parameter name")));
        }
        let v = v.trim();
        let value = match v.strip_prefix('@') {
            Some(alias) => aliases
                .get(alias)
                .cloned()
                .ok_or_else(|| bad(format!("the alias @{alias} has no value in the query")))?,
            None => literal(v)?,
        };
        if params.insert(k.to_owned(), value).is_some() {
            return Err(bad(format!("{k} is given twice")));
        }
    }
    Ok((name.to_owned(), params))
}

/// The arguments split on the commas outside quotes.
fn split_args(args: &str) -> Result<Vec<&str>, Refusal> {
    let mut out = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in args.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            ',' if !quoted => {
                out.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err(bad("a text value is closed with ', e.g. kind='Endpoint'"));
    }
    if !args.trim().is_empty() {
        out.push(&args[start..]);
    }
    Ok(out)
}

/// One OData literal as JSON.
fn literal(text: &str) -> Result<Value, Refusal> {
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        if inner.replace("''", "").contains('\'') {
            return Err(bad(format!("a quote inside {text} is written ''")));
        }
        return Ok(Value::String(inner.replace("''", "'")));
    }
    match text {
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "null" => Ok(Value::Null),
        _ => text
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| {
                text.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(Value::from)
                    .ok_or(())
            })
            .map_err(|()| {
                bad(format!(
                    "{text} is no value: text is quoted ('air'), or a number, true, false or null"
                ))
            }),
    }
}

/// An alias's value: an OData literal, else JSON.
fn alias_value(text: &str) -> Result<Value, Refusal> {
    literal(text).or_else(|refused| serde_json::from_str(text).map_err(|_| refused))
}

/// The answer of a function or action, as the OData JSON of an `Edm.Untyped` result.
pub(crate) async fn answer(
    state: &AppState,
    user: CurrentUser,
    front: Front,
    project: &str,
    op: &'static Operation,
    input: Value,
    context: &str,
) -> Response {
    let caller = caller_of(user, front);
    let response = match respond(op, ops::call(op, &caller, state, project, input).await) {
        Ok(response) => response,
        Err(refused) => return refused.into_response(),
    };
    if !response.status().is_success() {
        return response;
    }
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let body =
        json!({ "@odata.context": format!("{context}$metadata#Edm.Untyped"), "value": value });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json;odata.metadata=minimal"),
    );
    headers.insert("odata-version", HeaderValue::from_static("4.01"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    (status, headers, body.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_of_the_registry_has_one_class() {
        let classes: Vec<&str> = ENTITY_READS
            .iter()
            .chain(&FUNCTIONS)
            .chain(&PROPOSING)
            .chain(&OTHER_ACTIONS)
            .copied()
            .collect();
        let wrong: Vec<String> = ops::registry()
            .iter()
            .filter_map(|op| {
                let n = classes.iter().filter(|name| **name == op.name).count();
                (n != 1).then(|| format!("{} ({n} classes)", op.name))
            })
            .chain(
                classes
                    .iter()
                    .filter(|name| ops::find(name).is_none())
                    .map(|name| format!("{name} (not in the registry)")),
            )
            .collect();
        assert!(
            wrong.is_empty(),
            "API/13 §2b and the registry differ: {wrong:?}"
        );
        // Every operation is reachable: a set answers it or it is a function or action here.
        let unreached: Vec<&str> = ops::registry()
            .iter()
            .map(|op| op.name)
            .filter(|name| form(name).is_none())
            .filter(|name| !SERVED_AS_SETS.contains(name) && !ENTITY_BOUND.contains(name))
            .collect();
        assert!(unreached.is_empty(), "no OData form: {unreached:?}");
        for name in FUNCTIONS {
            let op = ops::find(name).expect("found above");
            assert!(op.annotations.read_only_hint, "the function {name} writes");
        }
        // An action may only read (a dry run, a computation): its input is a body, not a URL.
        for name in PROPOSING {
            let op = ops::find(name).expect("found above");
            assert!(!op.annotations.read_only_hint, "{name} proposes nothing");
        }
    }

    #[test]
    fn a_function_call_reads_literals_and_aliases() {
        let (name, params) = function_call(
            "jc_catalog_search(query=@q,limit=5,exact=false,note='it''s')",
            "@q=%27air%2Fquality%27",
        )
        .expect("parsed");
        assert_eq!(name, "jc_catalog_search");
        assert_eq!(
            Value::Object(params),
            json!({ "query": "air/quality", "limit": 5, "exact": false, "note": "it's" })
        );
        let (_, params) =
            function_call("jc_kpi_compute(spec=@s)", "@s=%7B%22a%22%3A1%7D").expect("json alias");
        assert_eq!(params["spec"], json!({ "a": 1 }));
        let (name, params) = function_call("jc_knowledge_status()", "").expect("empty");
        assert_eq!((name.as_str(), params.len()), ("jc_knowledge_status", 0));
        for (call, query) in [
            ("jc_schema_get(kind=Endpoint)", ""),
            ("jc_schema_get(kind='Endpoint'", ""),
            ("jc_schema_get(kind='a,kind='b')", ""),
            ("jc_schema_get(kind='a',kind='b')", ""),
            ("jc_schema_get(kind=@k)", ""),
            ("jc_schema_get(kind='x')", "$top=1"),
            ("jc_schema_get(k-ind='x')", ""),
        ] {
            let refused = function_call(call, query).expect_err(call);
            assert_eq!(refused.0, StatusCode::BAD_REQUEST, "{call}");
        }
    }
}
