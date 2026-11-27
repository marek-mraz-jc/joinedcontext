//! One connector's MCP server at `/c/{id}` (AG-143..AG-147, ADR-N-064 §3.3, API/14 §3).
//!
//! The same JSON-RPC as `/api/v1/mcp`, answered as the connector's owner narrowed by the
//! connector. Each call is judged in the order API/14 §3 lists: the token (signature, issuer,
//! expiry, audience = every connector's, one `connector:{id}` scope = this one), the owner, the state, the client, then the tool
//! and its target. Nothing is cached across calls: the record is read and the owner's bindings
//! decide again every time, so a revoke, an expiry or a role the owner lost holds from the next
//! call. Every call past the token writes one audit row (connector, person, client, tool, target,
//! verdict, duration) and never a token, a header or an input value.

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use super::{
    audited, call_result, dispatch, elicitation, error, failure_result, json_response, result,
    too_large, tool_definition, GROUNDING_META, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    WITHHELD_META,
};
use crate::agents::oneshot::ui_actions::{self, UI_TOOLS};
use crate::connectors::{self, Connector};
use crate::error::ApiError;
use crate::personal_data::{self, Marks};
use crate::state::AppState;

/// The methods a connector serves. Resources, prompts and tasks read the platform past the
/// connector's scope check, so they stay the Portal MCP's (deny by default); the one resource a
/// connector reads is the plan view, a static page that holds no data (T-5960).
const METHODS: &[&str] = &["initialize", "ping", "tools/list", "tools/call"];

pub fn router(config: &crate::config::Config) -> Router<AppState> {
    Router::new()
        .route(
            "/c/{id}",
            post(handle_connector).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route(
            "/c/{id}/.well-known/oauth-protected-resource",
            get(connector_metadata),
        )
        .layer(super::browser_clients(config))
}

fn base(state: &AppState) -> String {
    state
        .config
        .public_base()
        .as_str()
        .trim_end_matches('/')
        .to_owned()
}

/// One answer for an id that does not exist and for another person's connector (AG-143).
fn unknown() -> Response {
    ApiError::NotFound("no such connector".into()).into_response()
}

/// 401 naming the metadata and, for a well-formed id, the scope a client asks a token for
/// (MCP authorization, scope selection).
fn unauthorized(address: &str, id: &str) -> Response {
    let mut response = ApiError::Unauthorized.into_response();
    let mut challenge =
        format!("Bearer resource_metadata=\"{address}/.well-known/oauth-protected-resource\"");
    if connectors::is_id(id) {
        challenge.push_str(&format!(", scope=\"{}\"", connectors::scope(id)));
    }
    if let Ok(value) = challenge.parse() {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

/// RFC 9728 metadata naming the connector as the resource a client asks a token for (RFC 8707).
pub async fn connector_metadata(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !connectors::is_id(&id) {
        return unknown();
    }
    let authorization_servers = super::authorization_servers(&state);
    Json(json!({
        "resource": connectors::address(&base(&state), &id),
        "authorization_servers": authorization_servers,
        "scopes_supported": [connectors::scope(&id)],
        "bearer_methods_supported": ["header"],
    }))
    .into_response()
}

/// The MCP door of one connector.
pub async fn handle_connector(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let address = connectors::address(&base(&state), &id);
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        return unauthorized(&address, &id);
    };
    // The Organization realm of this host checks its own tokens, the installation's the rest
    // (T-7142).
    let realm = crate::auth::realms::of_token(&state, token);
    let Some(verifier) = crate::auth::realms::verifier(&state, realm.as_deref()) else {
        return unauthorized(&address, &id);
    };
    // Every connector token names the connectors' audience and exactly one connector's scope:
    // a Portal token, or one for another connector, is not a token for this one (AG-143, T-5954).
    let Ok((mut session, _)) = verifier
        .verify_for_scope(token, connectors::AUDIENCE, &connectors::scope(&id))
        .await
    else {
        return unauthorized(&address, &id);
    };
    if realm.is_some() {
        crate::auth::realms::confine(&mut session.identity, &state.config.bootstrap_admins);
    }
    if state.is_revoked(&session) {
        return unauthorized(&address, &id);
    }
    if !connectors::is_id(&id) {
        return unknown();
    }
    let connector = match state.connectors.get(&id, &base(&state)).await {
        Ok(Some(c)) => c,
        Ok(None) => return unknown(),
        Err(err) => return err.into_response(),
    };
    let mut message: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    // A one-project connector's calls may leave `project` out (T-6064); the audit, the scope
    // check and the operation then read the project the stored scope names.
    let body = if defaults_project(&connector, &mut message) {
        serde_json::to_vec(&message).map_or(body, Bytes::from)
    } else {
        body
    };
    let (tool, target) = audited(&message);
    let identity = session.identity;
    let audit = super::Audit {
        trail: connectors::Trail::new(Some(connector.id.clone()), None),
        subject: identity.subject.clone(),
        client: identity.client.clone(),
        tool,
        target,
    };
    let call = async {
        if connector.owner != identity.subject {
            (unknown(), "refused: not the owner".to_owned())
        } else {
            judge(&state, &connector, identity.clone(), token, &message, body).await
        }
    };
    super::on_record(&state, audit, call).await
}

/// A call past one of the connector's or the person's limits: 429 problem+json with
/// `Retry-After` in seconds (T-5968).
fn limited(why: &str, wait: i64) -> Response {
    let mut response = ApiError::TooManyRequests(why.to_owned()).into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, header::HeaderValue::from(wait));
    response
}

fn refused_call(id: Value, why: &str) -> Value {
    tool_problem(id, 403, "forbidden", "Forbidden", why)
}

/// The tools one project of the connector's scope offers its caller: the registry's listing
/// filtered by the connector's segments and the ceiling, the resource kinds narrowed to those
/// the caller reads there.
fn listed(
    state: &AppState,
    connector: &connectors::Connector,
    caller: &crate::ops::Caller,
    project: &str,
    within_ceiling: impl Fn(&crate::ops::Operation) -> bool,
) -> Vec<Value> {
    let tools: Vec<Value> = crate::ops::listing(caller, state, project)
        .iter()
        .filter(|summary| {
            crate::ops::find(&summary.name)
                .is_some_and(|op| connector.lists(op, project) && within_ceiling(op))
        })
        .map(|summary| {
            // Answered inline at this door, never as a task (T-6028).
            let mut tool = tool_definition(summary);
            tool["execution"]["taskSupport"] = json!("forbidden");
            // The Endpoints a narrowed scope names, so the client offers exactly those (T-6413).
            if let (Some(names), Some(endpoint)) = (
                connector.endpoints_named(project),
                tool.pointer_mut("/inputSchema/properties/endpoint")
                    .and_then(Value::as_object_mut),
            ) {
                endpoint.insert("enum".into(), json!(names));
            }
            tool
        })
        .collect();
    let (_, kinds) = connector.reach();
    crate::ops::resources::with_readable_kinds(tools, caller, state, project, Some(&kinds))
}

/// One list of tools from several projects' lists: each tool once, a resource tool's kinds
/// the union of what each project lets the caller read.
fn merged(lists: impl Iterator<Item = Vec<Value>>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for tool in lists.flatten() {
        let Some(seen) = out.iter_mut().find(|t| t["name"] == tool["name"]) else {
            out.push(tool);
            continue;
        };
        for field in ["kind", "endpoint"] {
            let more = &tool["inputSchema"]["properties"][field]["enum"];
            // `pointer_mut`: an index writes `null` into a tool without this field (T-6428).
            let Some(have) = seen.pointer_mut(&format!("/inputSchema/properties/{field}")) else {
                continue;
            };
            match (
                have.get_mut("enum").and_then(Value::as_array_mut),
                more.as_array(),
            ) {
                (Some(have), Some(more)) => {
                    for value in more {
                        if !have.contains(value) {
                            have.push(value.clone());
                        }
                    }
                }
                // One project reaches every Endpoint: the list holds no bound.
                (Some(_), None) if field == "endpoint" => {
                    have.as_object_mut().and_then(|have| have.remove("enum"));
                }
                _ => {}
            }
        }
    }
    out
}

/// A tool error the model reads and can act on, its problem in `structuredContent`.
fn tool_problem(id: Value, status: u16, slug: &str, title: &str, why: &str) -> Value {
    result(
        id,
        json!({
            "isError": true,
            "content": [{ "type": "text", "text": why }],
            "structuredContent": {
                "type": crate::error::type_uri(slug),
                "title": title,
                "status": status,
                "detail": why,
            }
        }),
    )
}

/// What the connector answers itself: a read of the plan view, or the refusal of a method it
/// does not serve; `None` for a method the owner's call answers.
fn answered_here(
    state: &AppState,
    method: &str,
    id: &Value,
    message: &Value,
) -> Option<(Response, String)> {
    let uri = message["params"]["uri"].as_str().unwrap_or_default();
    if let Some(contents) = (method == "resources/read")
        .then(|| super::app_view::contents(uri, &state.config.public_base()))
        .flatten()
    {
        let answer = result(id.clone(), json!({ "contents": contents }));
        return Some((json_response(StatusCode::OK, &answer), "allowed".to_owned()));
    }
    if !METHODS.contains(&method) {
        let answer = error(
            id.clone(),
            -32601,
            "a connector serves initialize, ping, tools/list, tools/call and its plan and chart views",
        );
        return Some((
            json_response(StatusCode::OK, &answer),
            "refused: method".to_owned(),
        ));
    }
    None
}

/// The owner's call, narrowed by the connector: the answer and its verdict.
async fn judge(
    state: &AppState,
    connector: &Connector,
    identity: crate::auth::session::Identity,
    token: &str,
    message: &Value,
    body: Bytes,
) -> (Response, String) {
    if !connector.is_active() {
        let why = format!(
            "this connector is {}; create a new one on the My connectors page",
            connector.state
        );
        return (
            ApiError::Denied(why).into_response(),
            format!("refused: {}", connector.state),
        );
    }
    let client = identity.client.as_deref().unwrap_or("");
    if !connector.allowed_clients.is_empty()
        && !connector.allowed_clients.iter().any(|c| c == client)
    {
        let why = format!(
            "this connector answers only {}; the token was obtained by {}",
            connector.allowed_clients.join(", "),
            if client.is_empty() {
                "no named client"
            } else {
                client
            }
        );
        return (
            ApiError::Denied(why).into_response(),
            "refused: client".to_owned(),
        );
    }
    // The owner's DelegationPolicy ceiling, read again on this call (AG-150).
    let ceiling = connectors::Ceiling::of(&state.mirror, &identity).with_approved(connector);
    if let Err(why) = ceiling.door(connector, client, jc_time::TimestampTz::now()) {
        return (
            ApiError::Denied(why).into_response(),
            "refused: ceiling".to_owned(),
        );
    }
    if body.len() > MAX_REQUEST_BYTES {
        let why = format!(
            "the request is {} bytes; this route reads at most {MAX_REQUEST_BYTES}",
            body.len()
        );
        return (
            too_large(&why, &Value::Null),
            "refused: too large".to_owned(),
        );
    }
    // The person's slots first: they are the ones their other connectors and `/api/v1/mcp` share.
    let (person, own, limits) = (
        identity.subject.clone(),
        format!("\u{0}c/{}", connector.id),
        connector.limits,
    );
    let person_limits = connectors::PERSON_LIMITS;
    let _in_flight = match state.mcp_enter(&[
        (&person, person_limits.parallel_calls),
        (&own, limits.parallel_calls),
    ]) {
        Ok(held) => held,
        Err(which) => {
            let why = if which == 0 {
                format!(
                    "{} of your calls are running now, the most at once over all your \
                     connectors; send this one when one of them has answered",
                    person_limits.parallel_calls
                )
            } else {
                format!(
                    "{} calls of this connector are running now, its most at once; send this \
                     one when one of them has answered",
                    limits.parallel_calls
                )
            };
            return (limited(&why, 1), "refused: parallel calls".to_owned());
        }
    };
    let method = message["method"].as_str().unwrap_or("");
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if !message.is_object() || method == "notifications/initialized" || message.get("id").is_none()
    {
        return (
            dispatch(state.clone(), caller(connector, identity, token), body).await,
            "allowed".to_owned(),
        );
    }
    if let Some(answered) = answered_here(state, method, &id, message) {
        return answered;
    }
    let params = &message["params"];
    let project = params["arguments"]["project"]
        .as_str()
        .or_else(|| params["project"].as_str())
        .map(str::to_owned);
    if method == "tools/call" {
        if let Some(name) = params["name"].as_str().filter(|n| UI_TOOLS.contains(n)) {
            return tab_action(state, connector, &identity, name, &params["arguments"], id);
        }
    }
    let caller = caller(connector, identity, token);
    match method {
        "tools/list" => {
            // Without a project, every project of the connector's scope offers its tools, not
            // only the first (JC1-22).
            let projects: Vec<String> = match project {
                Some(project) => vec![project],
                None => connector
                    .projects()
                    .iter()
                    .map(|p| (*p).to_owned())
                    .collect(),
            };
            let mut tools = merged(projects.iter().map(|project| {
                listed(state, connector, &caller, project, |op| {
                    ceiling.lists(connector, op, project)
                })
            }));
            // The UI tools reach the person's paired tab only, so they are offered only while
            // one is paired (T-5967).
            let now = crate::auth::session::now_unix();
            if state
                .paired_tabs
                .is_paired(&connector.id, &connector.owner, now)
            {
                tools.extend(ui_actions::mcp_tools(&connector.projects()));
            }
            if connector.projects().len() == 1 {
                optional_project(&mut tools);
            }
            let answer = result(id, json!({ "tools": tools }));
            (json_response(StatusCode::OK, &answer), "allowed".to_owned())
        }
        "initialize" => {
            let response = dispatch(state.clone(), caller, body).await;
            (
                with_projects_named(
                    response,
                    &connector.projects(),
                    &state.config.public_base(),
                    state.paired_tabs.is_paired(
                        &connector.id,
                        &connector.owner,
                        crate::auth::session::now_unix(),
                    ),
                )
                .await,
                "allowed".to_owned(),
            )
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            // A saved widget's read is checked, not the reference to it (T-6864).
            let resolved = match project.as_deref() {
                Some(p) if name == crate::ops::render::NAME => {
                    crate::ops::render::door_args(&caller, state, p, &params["arguments"])
                }
                _ => None,
            };
            let arguments = resolved.as_ref().unwrap_or(&params["arguments"]);
            let client = caller.identity.client.clone();
            let shows = match checks(
                state,
                connector,
                &ceiling,
                project.as_deref(),
                name,
                arguments,
            )
            .await
            {
                Ok(shows) => shows,
                Err((verdict, why)) => {
                    let answer = refused_call(id, &why);
                    return (json_response(StatusCode::OK, &answer), verdict.to_owned());
                }
            };
            let response = dispatch(state.clone(), caller, body).await;
            let response = if shows {
                response
            } else {
                without_personal(state, project.as_deref(), response).await
            };
            let (response, verdict) = verdict_of(response, id, limits.result_bytes).await;
            let followed = Followed {
                connector,
                client,
                project: project.as_deref(),
                tool: name,
            };
            (
                follow_in_tab(state, &followed, response, &verdict).await,
                verdict,
            )
        }
        _ => (
            dispatch(state.clone(), caller, body).await,
            "allowed".to_owned(),
        ),
    }
}

/// What `initialize` adds while the person's tab is paired (T-6501).
const PAIRED_TAB_NOTE: &str = " The person watches a Portal tab connected to this connector: \
    it opens each approval and confirmation you start by itself, and re-reads its page after each \
    call. Once a change is merged, open what you work on next with jc_ui_navigate.";

/// One answered connector call, for the person's paired tab.
struct Followed<'a> {
    connector: &'a Connector,
    client: Option<String>,
    project: Option<&'a str>,
    tool: &'a str,
}

/// The page that needs the person after `tool` answered `envelope`: a confirmation it asks, or
/// the change it opened (several open the approvals list). The ids come from the Portal's own
/// answer, and the page is one the person could open by hand.
fn page_needing_the_person(project: &str, tool: &str, envelope: &Value) -> Option<String> {
    let result = &envelope["result"];
    if let Some(id) =
        result["_meta"][elicitation::ELICITATION_META]["elicitation"]["elicitationId"].as_str()
    {
        return Some(format!("/projects/{project}/mcp-elicitations/{id}"));
    }
    if tool != "jc_change_apply" {
        return None;
    }
    match result["structuredContent"]["changeRequests"]
        .as_array()?
        .as_slice()
    {
        [] => None,
        [one] => Some(format!(
            "/projects/{project}/approvals/{}",
            one["id"].as_str()?
        )),
        _ => Some(format!("/projects/{project}/approvals")),
    }
}

/// Keeps the person's paired tab beside the work (T-6501): a call that needs them opens that page,
/// any other answered call tells the tab to re-read its page. The tab hears no value of the call,
/// only its tool name, and a client that never calls a UI tool is followed all the same.
async fn follow_in_tab(
    state: &AppState,
    call: &Followed<'_>,
    response: Response,
    verdict: &str,
) -> Response {
    let connector = call.connector;
    let now = crate::auth::session::now_unix();
    let Some(project) = call.project.filter(|_| verdict == "allowed") else {
        return response;
    };
    if !state
        .paired_tabs
        .is_paired(&connector.id, &connector.owner, now)
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_RESPONSE_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let envelope: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let (frame, payload) = match page_needing_the_person(project, call.tool, &envelope) {
        Some(route) => ("navigate", json!({ "route": route })),
        None => ("refresh", json!({})),
    };
    let action = json!({
        "frame": frame,
        "payload": payload,
        "tool": call.tool,
        "project": project,
        "from": { "connector": connector.name, "client": call.client },
        "at": jc_time::TimestampTz::now(),
    });
    // A tab closed or reloading misses this one; the next call tells it again.
    let _ = state
        .paired_tabs
        .deliver(&connector.id, &connector.owner, action, now);
    Response::from_parts(parts, Body::from(bytes))
}

/// A UI action for the person's paired tab (T-5967): checked as the assistant checks it, held to
/// the connector's scope, then handed to that tab only. The answer says it was shown and carries
/// nothing of the tab or the session.
fn tab_action(
    state: &AppState,
    connector: &Connector,
    identity: &crate::auth::session::Identity,
    name: &str,
    arguments: &Value,
    id: Value,
) -> (Response, String) {
    let refused = |status: u16, slug: &str, title: &str, why: &str, verdict: &str| {
        let answer = tool_problem(id.clone(), status, slug, title, why);
        (json_response(StatusCode::OK, &answer), verdict.to_owned())
    };
    let projects = connector.projects();
    let Some(project) = arguments["project"]
        .as_str()
        .filter(|p| projects.contains(p))
    else {
        let why = format!(
            "name the project of the page as `project`, one of this connector's: {}",
            projects.join(", ")
        );
        return refused(403, "forbidden", "Forbidden", &why, "refused: target");
    };
    // `project` was read off the arguments above, so they are an object.
    let mut checked = arguments.clone();
    checked
        .as_object_mut()
        .and_then(|fields| fields.remove("project"));
    let asker = ui_actions::Asker {
        state,
        identity,
        project,
        connector: Some(connector),
    };
    let (frame, payload) = match ui_actions::for_tab(&asker, name, &checked) {
        Ok(done) => done,
        Err(why) => {
            return refused(422, "unprocessable", "Unprocessable", &why, "refused: tool");
        }
    };
    let action = json!({
        "frame": frame,
        "payload": payload,
        "tool": name,
        "project": project,
        "from": { "connector": connector.name, "client": identity.client },
        "at": jc_time::TimestampTz::now(),
    });
    let now = crate::auth::session::now_unix();
    match state
        .paired_tabs
        .deliver(&connector.id, &connector.owner, action, now)
    {
        Ok(()) => {
            let shown = json!({
                "shown": payload.get("route").or_else(|| payload.get("action")),
                "note": "The person's Portal tab shows this; nothing is saved until they propose it there.",
            });
            let answer = result(id.clone(), call_result(&shown));
            (json_response(StatusCode::OK, &answer), "allowed".to_owned())
        }
        Err(undelivered) => {
            // The page exists whether a tab listens or not: the person can open it by hand, and
            // the address holds nothing they could not type (T-6442).
            let why = match payload["route"].as_str() {
                Some(route) => format!(
                    "{}. Or give the person this address to open: {}",
                    undelivered.reason(),
                    state
                        .config
                        .public_base()
                        .join(route)
                        .map_or_else(|_| route.to_owned(), |address| address.to_string())
                ),
                None => undelivered.reason().to_owned(),
            };
            refused(409, "conflict", "Conflict", &why, "refused: no tab")
        }
    }
}

/// Puts the connector's one project into a `tools/call` that names none (T-6064). The project
/// comes from the stored scope only; a call that names a project keeps it, so a foreign one is
/// still refused by `admits`. `true` when the message changed. Public for the fuzz target
/// `mcp_connector_defaults`: the message is outside input.
pub fn defaults_project(connector: &Connector, message: &mut Value) -> bool {
    let [only] = connector.projects()[..] else {
        return false;
    };
    if message["method"] != "tools/call" || !message["params"].is_object() {
        return false;
    }
    let params = &mut message["params"];
    if params.get("arguments").is_none() {
        params["arguments"] = json!({});
    }
    match params["arguments"].as_object_mut() {
        Some(args) if !args.contains_key("project") => {
            args.insert("project".to_owned(), json!(only));
            true
        }
        _ => false,
    }
}

/// A one-project connector's tools take `project` as optional; `initialize` says it defaults to
/// that project, once rather than in every tool a model re-reads each turn (T-6058).
fn optional_project(tools: &mut [Value]) {
    for schema in tools.iter_mut().map(|t| &mut t["inputSchema"]) {
        if let Some(required) = schema["required"].as_array_mut() {
            required.retain(|r| r != "project");
        }
    }
}

/// The `initialize` answer with a sentence naming the projects the connector reaches, so a
/// client never has to guess or ask the person (T-6064), the address that draws a map, since
/// nothing on a connector draws one (T-6442), and where an App's code comes from (T-6487).
async fn with_projects_named(
    response: Response,
    projects: &[&str],
    portal: &url::Url,
    tab_paired: bool,
) -> Response {
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_RESPONSE_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(instructions) = answer["result"]["instructions"].as_str() else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let reach = match projects {
        [only] => {
            format!("This connector reaches project {only}; every tool's `project` defaults to it.")
        }
        many => format!(
            "This connector reaches projects {}; name one as `project` in each call.",
            many.join(", ")
        ),
    };
    let map = format!(
        "To show entities on a map, give the person the Portal address {portal}projects/<project>/\
         explore?endpoint=<endpoint>&type=<type>&colorBy=<number attribute>&view=map; it draws \
         what they may read, and a Layer alone draws nothing."
    );
    let build = format!(
        "An App manifest holds no code: jc_run_create starts the builder, where this connector \
         lists it; otherwise give the person {portal}projects/<project>/apps and name the App."
    );
    let tab = if tab_paired { PAIRED_TAB_NOTE } else { "" };
    answer["result"]["instructions"] = json!(format!("{instructions} {reach} {map} {build}{tab}"));
    // Only what [`METHODS`] and the plan view serve: the Portal MCP's prompts and tasks are
    // refused here, so a client is never told it may call them (JC2-21).
    answer["result"]["capabilities"] = json!({ "tools": {}, "resources": {} });
    json_response(parts.status, &answer)
}

fn caller(
    connector: &Connector,
    identity: crate::auth::session::Identity,
    token: &str,
) -> crate::ops::Caller {
    crate::ops::Caller {
        bearer: Some(crate::ops::Bearer(token.to_owned())),
        identity,
        via: crate::ops::Via::Mcp,
        access: Some(connector.access()),
    }
}

/// The arguments the door's scope, ceiling and personal-data checks read: `jc_render` names its
/// Endpoint read in `data.query` (T-6860), every other tool at the top of its arguments.
/// The door's scope, ceiling and personal-data checks of one call's `arguments`: whether its
/// answer may show personal attributes, or the refusal and its audit verdict.
async fn checks(
    state: &AppState,
    connector: &Connector,
    ceiling: &connectors::Ceiling,
    project: Option<&str>,
    name: &str,
    arguments: &Value,
) -> Result<bool, (&'static str, String)> {
    let args = target_args(name, arguments);
    if let (Some(op), Some(project)) = (crate::ops::find(name), project) {
        connector
            .admits(op, project, args, &|kind, name| {
                connectors::space_now(&state.mirror, project, kind, name)
            })
            .and_then(|()| ceiling.admits(connector, op, project, args))
            .map_err(|why| ("refused: target", why))?;
    }
    let shows = project.is_some_and(|p| connector.shows_personal(p, args));
    if !shows {
        indicator_stays_open(state, project, name, arguments)
            .await
            .map_err(|why| ("refused: personal data", why))?;
    }
    Ok(shows)
}

/// The door's checks again on a read a `jc_render` call makes past the door (T-6867): a whole
/// Dashboard's widget names its Endpoint, channels and source in the saved manifest, which the
/// door never saw. `Ok` for a call that comes through no connector.
pub(crate) async fn admits_read(
    state: &AppState,
    caller: &crate::ops::Caller,
    project: &str,
    args: &Value,
) -> Result<(), String> {
    let Some(connector) = caller.access.as_ref().and_then(|a| a.connector()) else {
        return Ok(());
    };
    let ceiling = connectors::Ceiling::of(&state.mirror, &caller.identity).with_approved(connector);
    checks(
        state,
        connector,
        &ceiling,
        Some(project),
        crate::ops::render::NAME,
        args,
    )
    .await
    .map(|_| ())
    .map_err(|(_, why)| why)
}

fn target_args<'a>(tool: &str, args: &'a Value) -> &'a Value {
    match tool {
        crate::ops::render::NAME => args.pointer("/data/query").unwrap_or(args),
        _ => args,
    }
}

/// An indicator or a chart read from an Endpoint folds entities the answer never carries, so
/// the answer's filter sees nothing: one built on a personal attribute, by its fold, a chart
/// channel or its `q`, is refused before the read (AG-158). Rows a client sends are already
/// its own.
async fn indicator_stays_open(
    state: &AppState,
    project: Option<&str>,
    tool: &str,
    args: &Value,
) -> Result<(), String> {
    let text = |at: &str| args.pointer(at).and_then(Value::as_str).unwrap_or("");
    let texts: Vec<&str> = match tool {
        "jc_kpi_compute" if args.get("endpoint").is_some() => vec![text("/attribute"), text("/q")],
        crate::ops::render::NAME => {
            let channels = args["encoding"].as_object().into_iter().flatten();
            // The order and the attributes read rank and pick entities as the filter does.
            let attrs = match args.pointer("/data/query/attrs") {
                Some(Value::Array(attrs)) => attrs.iter().filter_map(Value::as_str).collect(),
                _ => vec![text("/data/query/attrs")],
            };
            channels
                .filter_map(|(_, channel)| channel["field"].as_str())
                .chain([text("/data/query/q"), text("/data/query/orderBy")])
                .chain(attrs)
                .collect()
        }
        _ => return Ok(()),
    };
    let marks = match project {
        Some(project) => personal_data::of_project(state, project).await?,
        None => return Err("a call that reads an Endpoint names its project".to_owned()),
    };
    match personal_data::mentioned(&marks, &texts) {
        Some(name) => Err(format!(
            "the models mark {name} personal, so this connector computes nothing from it; a scope item with personalData ticked reaches it"
        )),
        None => Ok(()),
    }
}

/// A call's answer without the attributes the project's models mark personal (AG-158). Only an
/// answer that carries an entity reads the models; one whose models cannot be read is refused,
/// since nothing personal could be withheld from it.
async fn without_personal(state: &AppState, project: Option<&str>, response: Response) -> Response {
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_RESPONSE_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut envelope: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !personal_data::carries_entities(&envelope["result"]["structuredContent"]) {
        return Response::from_parts(parts, Body::from(bytes));
    }
    let marks = match project {
        Some(project) => personal_data::of_project(state, project).await,
        None => Err("a call that answers entities names its project".to_owned()),
    };
    envelope["result"] = without(envelope["result"].take(), marks);
    json_response(parts.status, &envelope)
}

/// The `tools/call` `result` without what `marks` names, or the refusal when the models could not
/// be read. Either keeps the grounding the call was evaluated against (AG-12).
fn without(mut result: Value, marks: Result<Marks, String>) -> Value {
    let grounding = result
        .get_mut("_meta")
        .and_then(|meta| meta.get_mut(GROUNDING_META))
        .map(Value::take);
    let mut answer = match marks {
        Ok(marks) => answered_without(result["structuredContent"].take(), &marks),
        Err(why) => failure_result(&why),
    };
    if let Some(grounding) = grounding {
        answer["_meta"][GROUNDING_META] = grounding;
    }
    answer
}

/// The `tools/call` result of `output` without what `marks` names, saying which attributes left
/// so the client's model can tell the person why they are missing.
fn answered_without(mut output: Value, marks: &Marks) -> Value {
    let removed = personal_data::withhold(&mut output, marks);
    let mut answer = call_result(&output);
    if !removed.is_empty() {
        // Beside what a sample answer named of its source (AG-158).
        let mut all: Vec<Value> = answer["_meta"][WITHHELD_META]
            .as_array()
            .cloned()
            .unwrap_or_default();
        all.extend(removed.into_iter().map(Value::String));
        answer["_meta"][WITHHELD_META] = json!(all);
    }
    answer
}

/// The verdict of a dispatched call, read off its envelope; the answer goes back unchanged
/// unless it is larger than the connector's `result_bytes` (T-5968).
async fn verdict_of(response: Response, id: Value, result_bytes: usize) -> (Response, String) {
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_RESPONSE_BYTES).await else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            "error".to_owned(),
        );
    };
    if bytes.len() > result_bytes {
        let why = format!(
            "the answer is {} bytes; this connector returns at most {result_bytes}. Ask for \
             fewer items (limit), fewer attributes (attrs) or one resource by name",
            bytes.len()
        );
        let answer = tool_problem(id, 413, "payload-too-large", "Payload Too Large", &why);
        return (
            json_response(StatusCode::OK, &answer),
            "refused: result size".to_owned(),
        );
    }
    let envelope: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let verdict = if !parts.status.is_success() || envelope.get("error").is_some() {
        "error"
    } else if envelope["result"]["isError"] == json!(true) {
        "refused: tool"
    } else {
        "allowed"
    };
    (
        Response::from_parts(parts, Body::from(bytes)),
        verdict.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JC1-22: a tool only the second project of a connector offers is still listed, and a
    /// resource tool reads the kinds of both projects.
    #[test]
    fn tools_of_every_project_are_listed_once() {
        let first = vec![
            json!({ "name": "jc_resource_list", "inputSchema": { "properties": { "kind": { "enum": ["Endpoint"] } } } }),
        ];
        let second = vec![
            json!({ "name": "jc_resource_list", "inputSchema": { "properties": { "kind": { "enum": ["Endpoint", "Pipeline"] } } } }),
            json!({ "name": "jc_pipeline_logs" }),
        ];
        let tools = merged([first, second].into_iter());
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["jc_resource_list", "jc_pipeline_logs"]);
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["kind"]["enum"],
            json!(["Endpoint", "Pipeline"])
        );
    }

    /// T-6413: the Endpoints two narrowed projects name are one list; a project that reaches
    /// every Endpoint drops the bound.
    #[test]
    fn the_named_endpoints_of_every_project_bound_one_list() {
        let tool = |names: Option<&[&str]>| {
            let mut endpoint = json!({ "type": "string" });
            if let Some(names) = names {
                endpoint["enum"] = json!(names);
            }
            vec![
                json!({ "name": "jc_data_query", "inputSchema": { "properties": { "endpoint": endpoint } } }),
            ]
        };
        let both = merged([tool(Some(&["air"])), tool(Some(&["air", "noise"]))].into_iter());
        assert_eq!(
            both[0]["inputSchema"]["properties"]["endpoint"]["enum"],
            json!(["air", "noise"])
        );
        let open = merged([tool(Some(&["air"])), tool(None)].into_iter());
        assert!(open[0]["inputSchema"]["properties"]["endpoint"]
            .get("enum")
            .is_none());
    }

    /// T-6428: a tool without `kind` or `endpoint` stays without them when two lists merge.
    #[test]
    fn merging_adds_no_field_a_tool_does_not_declare() {
        let tool =
            || vec![json!({ "name": "jc_change_list", "inputSchema": { "properties": {} } })];
        let both = merged([tool(), tool()].into_iter());
        assert_eq!(both[0]["inputSchema"]["properties"], json!({}));
    }

    /// JC2-41: a refused call names the same problem type base as every Portal error.
    #[test]
    fn a_refused_call_names_the_portal_error_type() {
        let answer = refused_call(json!(1), "no");
        assert_eq!(
            answer["result"]["structuredContent"]["type"],
            "https://joinedcontext.com/errors/forbidden"
        );
    }

    #[tokio::test]
    async fn an_answer_past_the_connector_result_size_is_a_413_tool_error() {
        let answer = result(
            json!(7),
            json!({ "content": [{ "type": "text", "text": "x".repeat(100) }] }),
        );
        let response = || json_response(StatusCode::OK, &answer);
        let (kept, verdict) = verdict_of(response(), json!(7), 1_000).await;
        assert_eq!(
            (kept.status(), verdict.as_str()),
            (StatusCode::OK, "allowed")
        );
        let (refused, verdict) = verdict_of(response(), json!(7), 50).await;
        assert_eq!(verdict, "refused: result size");
        let bytes = axum::body::to_bytes(refused.into_body(), 10_000)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["id"], 7);
        assert_eq!(body["result"]["isError"], true);
        assert_eq!(body["result"]["structuredContent"]["status"], 413);
        let detail = body["result"]["structuredContent"]["detail"]
            .as_str()
            .expect("detail");
        assert!(
            detail.contains("at most 50") && detail.contains("(limit)"),
            "{detail}"
        );
    }

    #[test]
    fn the_audit_keeps_identifiers_and_drops_input_values() {
        let (tool, target) = audited(&json!({
            "method": "tools/call",
            "params": { "name": "jc_resource_get", "arguments": {
                "project": "doprava", "kind": "Endpoint", "name": "counters",
                "q": "secret-looking value" } }
        }));
        assert_eq!(tool, "jc_resource_get");
        assert_eq!(target.as_deref(), Some("doprava/Endpoint/counters"));
        // A name that is not an identifier is not kept: it could be anything the model typed.
        let (_, target) = audited(&json!({
            "method": "tools/call",
            "params": { "name": "jc_resource_get", "arguments": {
                "project": "doprava", "name": "Bearer eyJhbGciOi..." } }
        }));
        assert_eq!(target.as_deref(), Some("doprava"));
        let (tool, target) = audited(&json!({ "method": "tools/list", "params": {} }));
        assert_eq!((tool.as_str(), target), ("tools/list", None));
    }

    #[test]
    fn an_answer_loses_its_personal_attributes_and_says_which_in_text_and_structure() {
        let marks = personal_data::marks(
            "classes:\n  PotholeReport:\n    slots: [description, contactEmail]\nslots:\n  contactEmail:\n    annotations: { personalData: true }\n",
        );
        let output = json!({ "entities": [{
            "id": "urn:ngsi-ld:PotholeReport:1", "type": "PotholeReport",
            "description": "Hlboká jama", "contactEmail": "jana@example.org"
        }] });
        let answer = answered_without(output, &marks);
        let text = answer.to_string();
        assert!(!text.contains("jana@example.org"), "{text}");
        assert!(answer["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Hlboká jama"));
        assert_eq!(
            answer["_meta"][WITHHELD_META],
            json!(["PotholeReport.contactEmail"])
        );
        assert_eq!(answer["_meta"][super::super::UNTRUSTED_META], json!(true));
        let plain = answered_without(json!({ "items": [] }), &marks);
        assert!(plain["_meta"].get(WITHHELD_META).is_none());
    }

    #[tokio::test]
    async fn a_one_project_connector_defaults_project_from_its_scope_and_names_it() {
        let connector = connectors::of_conversation("r1", "f:1:ana", "doprava", "pipeline");
        let call = |args: Value| {
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "jc_resource_list", "arguments": args } })
        };

        // No project: the stored scope's one project is put in, so the call reads doprava.
        let mut message = call(json!({ "kind": "Endpoint" }));
        assert!(defaults_project(&connector, &mut message));
        assert_eq!(message["params"]["arguments"]["project"], "doprava");
        let mut bare = json!({ "method": "tools/call", "params": { "name": "jc_resource_list" } });
        assert!(defaults_project(&connector, &mut bare));
        assert_eq!(bare["params"]["arguments"]["project"], "doprava");

        // A project the call names is kept: a foreign one is still refused with today's words.
        let mut foreign = call(json!({ "project": "espoo", "kind": "Endpoint" }));
        assert!(!defaults_project(&connector, &mut foreign));
        assert_eq!(foreign["params"]["arguments"]["project"], "espoo");
        let op = crate::ops::find("jc_resource_list").expect("op");
        let refused = connector
            .admits(op, "espoo", &foreign["params"]["arguments"], &|_, _| None)
            .expect_err("foreign project");
        assert!(
            refused.contains("does not reach project espoo"),
            "{refused}"
        );

        // Another method is never touched.
        let mut list = json!({ "method": "tools/list", "params": {} });
        assert!(!defaults_project(&connector, &mut list));
        assert_eq!(list, json!({ "method": "tools/list", "params": {} }));

        // tools/list of a one-project connector: no tool requires project.
        let mut tools = vec![tool_definition(&crate::ops::summary(op))];
        tools.extend(ui_actions::mcp_tools(&connector.projects()));
        optional_project(&mut tools);
        assert!(tools.iter().all(|t| !t["inputSchema"]["required"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("project")))));
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["project"]["maxLength"],
            63
        );

        // initialize names the project.
        let init = json_response(
            StatusCode::OK,
            &result(
                json!(1),
                json!({ "instructions": "Tool answers carry text.",
                    "capabilities": { "tools": {}, "resources": {}, "prompts": {}, "tasks": {} } }),
            ),
        );
        let portal = url::Url::parse("https://portal.example/").expect("url");
        let answer =
            answer_of(with_projects_named(init, &connector.projects(), &portal, false).await).await;
        // JC2-21: no capability a connector refuses is offered.
        assert_eq!(
            answer["result"]["capabilities"],
            json!({ "tools": {}, "resources": {} })
        );
        assert_eq!(
            answer["result"]["instructions"],
            "Tool answers carry text. This connector reaches project doprava; every tool's \
             `project` defaults to it. To show entities on a map, give the person the Portal \
             address https://portal.example/projects/<project>/explore?endpoint=<endpoint>\
             &type=<type>&colorBy=<number attribute>&view=map; it draws what they may read, and a \
             Layer alone draws nothing. An App manifest holds no code: jc_run_create starts the \
             builder, where this connector lists it; otherwise give the person \
             https://portal.example/projects/<project>/apps and name the App."
        );
    }

    #[test]
    fn a_withheld_answer_keeps_the_grounding_it_was_evaluated_against() {
        let marks = || {
            Ok(personal_data::marks(
                "classes:\n  PotholeReport:\n    slots: [contactEmail]\nslots:\n  contactEmail:\n    annotations: { personalData: true }\n",
            ))
        };
        let grounding = json!({ "revision": "abc123", "drift": "none",
            "syncedAt": null, "evaluatedAt": "2026-10-29T13:00:00Z" });
        let grounded = |output: Value| {
            let mut result = call_result(&output);
            result["_meta"][GROUNDING_META] = grounding.clone();
            result
        };
        let withheld = grounded(json!({ "entities": [{ "id": "urn:ngsi-ld:PotholeReport:1",
            "type": "PotholeReport", "contactEmail": "jana@example.org" }] }));
        let answer = without(withheld, marks());
        assert_eq!(answer["_meta"][GROUNDING_META], grounding);
        assert_eq!(
            answer["_meta"][WITHHELD_META],
            json!(["PotholeReport.contactEmail"])
        );
        assert_eq!(answer["_meta"][super::super::UNTRUSTED_META], json!(true));
        assert!(!answer.to_string().contains("jana@example.org"));
        let kept = without(
            grounded(json!({ "entities": [{ "id": "urn:ngsi-ld:PotholeReport:2",
            "type": "PotholeReport" }] })),
            marks(),
        );
        assert_eq!(kept["_meta"][GROUNDING_META], grounding);
        assert!(kept["_meta"].get(WITHHELD_META).is_none());
        let refused = without(
            grounded(json!({ "entities": [] })),
            Err("no models".to_owned()),
        );
        assert_eq!(refused["_meta"][GROUNDING_META], grounding);
        assert_eq!(refused["isError"], true);
        assert!(refused["_meta"].get(super::super::UNTRUSTED_META).is_none());
    }

    fn ana() -> crate::auth::session::Identity {
        crate::auth::session::Identity {
            subject: "f:1:ana".to_owned(),
            username: "ana".to_owned(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            client: Some("mcp-local".to_owned()),
            second_factor: false,
        }
    }

    async fn answer_of(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn a_call_needing_the_person_opens_their_paired_tab_and_any_other_rereads_it() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector = connectors::of_conversation("r1", "f:1:ana", "doprava", "pipeline");
        let now = crate::auth::session::now_unix();
        let answered = |result: Value| {
            json_response(
                StatusCode::OK,
                &json!({ "jsonrpc": "2.0", "id": 1, "result": result }),
            )
        };
        let follow = |tool: &'static str, result: Value, verdict: &'static str| {
            let call = Followed {
                connector: &connector,
                client: Some("claude-code".to_owned()),
                project: Some("doprava"),
                tool,
            };
            let state = &state;
            let response = answered(result);
            async move { answer_of(follow_in_tab(state, &call, response, verdict).await).await }
        };
        let opened = json!({ "structuredContent": { "changeRequests": [{ "id": "chg-1" }] } });

        // No tab paired: the answer passes untouched.
        let body = follow("jc_change_apply", opened.clone(), "allowed").await;
        assert_eq!(
            body["result"]["structuredContent"]["changeRequests"][0]["id"],
            "chg-1"
        );

        let tab = state
            .paired_tabs
            .pair(&connector.id, "f:1:ana", 5, now + 3600);
        let (mut heard, _) = state
            .paired_tabs
            .listen(&connector.id, &tab, "f:1:ana", 5, now + 3600, now)
            .expect("paired");

        follow("jc_change_apply", opened.clone(), "allowed").await;
        let action = heard.try_recv().expect("the tab opens the change");
        assert_eq!(action["frame"], "navigate");
        assert_eq!(
            action["payload"]["route"],
            "/projects/doprava/approvals/chg-1"
        );

        let two =
            json!({ "structuredContent": { "changeRequests": [{ "id": "a" }, { "id": "b" }] } });
        follow("jc_change_apply", two, "allowed").await;
        let action = heard.try_recv().expect("the tab opens the list");
        assert_eq!(action["payload"]["route"], "/projects/doprava/approvals");

        let question = elicitation::document("eli-1", "Run it?", "https://portal/x", None);
        follow("jc_pipeline_run", question, "allowed").await;
        let action = heard.try_recv().expect("the tab opens the question");
        assert_eq!(
            action["payload"]["route"],
            "/projects/doprava/mcp-elicitations/eli-1"
        );

        let secret = json!({ "structuredContent": { "value": "a-private-value" } });
        follow("jc_data_query", secret, "allowed").await;
        let action = heard.try_recv().expect("the tab reads its page again");
        assert_eq!(action["frame"], "refresh");
        assert!(!action.to_string().contains("a-private-value"), "{action}");

        // A refused call moves nothing.
        follow("jc_change_apply", opened, "refused: tool").await;
        assert!(heard.try_recv().is_err());
    }

    #[tokio::test]
    async fn initialize_names_the_paired_tab_only_while_one_is_paired() {
        let portal = url::Url::parse("https://portal.example/").expect("url");
        let answer = || {
            json_response(
                StatusCode::OK,
                &json!({ "result": { "instructions": "Fenced data." } }),
            )
        };
        let told =
            answer_of(with_projects_named(answer(), &["doprava"], &portal, true).await).await;
        assert!(told["result"]["instructions"]
            .to_string()
            .contains("jc_ui_navigate"));
        let bare =
            answer_of(with_projects_named(answer(), &["doprava"], &portal, false).await).await;
        assert!(!bare["result"]["instructions"]
            .to_string()
            .contains("jc_ui_navigate"));
    }

    #[tokio::test]
    async fn a_ui_action_reaches_only_the_owners_paired_tab_and_never_submits() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector = connectors::of_conversation("r1", "f:1:ana", "doprava", "pipeline");
        let now = crate::auth::session::now_unix();
        let navigate = json!({ "project": "doprava", "page": "spaces" });
        let call =
            |name: &str, args: &Value| tab_action(&state, &connector, &ana(), name, args, json!(1));

        // No tab paired: refused, and the model is told what the person does.
        let (response, verdict) = call("jc_ui_navigate", &navigate);
        assert_eq!(verdict, "refused: no tab");
        let body = answer_of(response).await;
        assert_eq!(body["result"]["isError"], true);
        assert!(body["result"]["structuredContent"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("Connect this tab")));

        // T-6442: the page still exists, so the refusal hands the model its address to pass on.
        let map = json!({ "project": "doprava", "page": "map", "endpoint": "bikes",
            "type": "Station", "color": "free" });
        let (response, verdict) = call("jc_ui_navigate", &map);
        assert_eq!(verdict, "refused: no tab");
        let body = answer_of(response).await;
        let detail = body["result"]["structuredContent"]["detail"].to_string();
        assert!(
            detail.contains(
                "/projects/doprava/explore?endpoint=bikes&type=Station&colorBy=free&view=map"
            ),
            "{detail}"
        );
        assert!(detail.contains("this address to open: http"), "{detail}");

        // A tab paired by another person with this connector's id hears nothing of ana's.
        state
            .paired_tabs
            .pair(&connector.id, "f:1:bob", 5, now + 3600);
        assert_eq!(call("jc_ui_navigate", &navigate).1, "refused: no tab");

        let tab = state
            .paired_tabs
            .pair(&connector.id, "f:1:ana", 5, now + 3600);
        let (mut heard, _) = state
            .paired_tabs
            .listen(&connector.id, &tab, "f:1:ana", 5, now + 3600, now)
            .expect("paired");
        // A project outside the connector's scope, a page the Portal lacks: refused, nothing sent.
        let other = json!({ "project": "espoo", "page": "spaces" });
        assert_eq!(call("jc_ui_navigate", &other).1, "refused: target");
        let nowhere = json!({ "project": "doprava", "page": "admin-secrets" });
        assert_eq!(call("jc_ui_navigate", &nowhere).1, "refused: tool");
        assert!(heard.try_recv().is_err());

        let (response, verdict) = call("jc_ui_navigate", &navigate);
        assert_eq!(verdict, "allowed");
        let text = answer_of(response).await.to_string();
        assert!(
            !text.contains(&tab),
            "the tab's id never reaches the agent: {text}"
        );
        let action = heard.try_recv().expect("the paired tab hears it");
        assert_eq!(action["frame"], "navigate");
        assert_eq!(action["payload"]["route"], "/projects/doprava/spaces");
        assert_eq!(action["from"]["client"], "mcp-local");
        assert_eq!(action["from"]["connector"], json!(connector.name));

        // The tools a paired connector lists are the five actions, none of which submits; a
        // submit is no UI tool, so it never reaches the tab.
        let tools = ui_actions::mcp_tools(&connector.projects());
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, UI_TOOLS);
        assert!(tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true
                && t["inputSchema"]["required"]
                    .as_array()
                    .is_some_and(|r| r.contains(&json!("project")))));
        assert!(!UI_TOOLS.contains(&"jc_ui_submit"));
    }

    /// One call through `judge`, as the door hands it on past the token: the status, the verdict
    /// and the JSON answer (`null` for an empty one).
    async fn judged(
        state: &AppState,
        connector: &Connector,
        identity: crate::auth::session::Identity,
        message: Value,
    ) -> (StatusCode, String, Value) {
        let body = Bytes::from(message.to_string());
        let (response, verdict) =
            judge(state, connector, identity, "a-token", &message, body).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            verdict,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn doprava() -> Connector {
        connectors::of_conversation("r1", "f:1:ana", "doprava", "pipeline")
    }

    fn rpc(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
    }

    /// AG-153: a connector bound to named clients refuses a token that names none, and says so.
    #[tokio::test]
    async fn a_token_of_no_named_client_is_refused_by_a_bound_connector() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut connector = doprava();
        connector.allowed_clients = vec!["claude-ai".to_owned()];
        let mut nobody = ana();
        nobody.client = None;
        let (status, verdict, body) =
            judged(&state, &connector, nobody, rpc("ping", json!({}))).await;
        assert_eq!(
            (status, verdict.as_str()),
            (StatusCode::FORBIDDEN, "refused: client")
        );
        let detail = body["detail"].as_str().expect("detail");
        assert!(
            detail.contains("answers only claude-ai") && detail.contains("no named client"),
            "{detail}"
        );
        let (_, verdict, body) = judged(&state, &connector, ana(), rpc("ping", json!({}))).await;
        assert_eq!(verdict, "refused: client");
        assert!(body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("obtained by mcp-local")));
    }

    /// AG-150: a connector older than the ceiling's longest life is refused at the door.
    #[tokio::test]
    async fn a_connector_older_than_the_ceiling_allows_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut connector = doprava();
        connector.granted_at =
            jc_time::TimestampTz::now() - jiff::SignedDuration::from_hours(24 * 3650);
        let (status, verdict, body) =
            judged(&state, &connector, ana(), rpc("ping", json!({}))).await;
        assert_eq!(
            (status, verdict.as_str()),
            (StatusCode::FORBIDDEN, "refused: ceiling")
        );
        assert!(
            body["detail"].as_str().is_some_and(|d| d.contains("days")),
            "{body}"
        );
    }

    /// AG-60: a request past the byte limit is a 413 naming the limit, and nothing is dispatched.
    #[tokio::test]
    async fn a_request_past_the_limit_is_refused_before_anything_runs() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let message = rpc("tools/list", json!({}));
        let body = Bytes::from(vec![b' '; MAX_REQUEST_BYTES + 1]);
        let (response, verdict) = judge(&state, &doprava(), ana(), "t", &message, body).await;
        assert_eq!(verdict, "refused: too large");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let answer = answer_of(response).await;
        assert!(answer["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains(&format!("at most {MAX_REQUEST_BYTES}"))));
    }

    /// A notification, or a message without an id, is the Portal MCP's answer: 202, allowed.
    #[tokio::test]
    async fn a_notification_through_a_connector_is_accepted() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        for message in [
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "method": "ping" }),
            json!([1, 2]),
        ] {
            let (status, verdict, _) = judged(&state, &doprava(), ana(), message.clone()).await;
            assert_eq!(verdict, "allowed", "{message}");
            assert!(
                status == StatusCode::ACCEPTED || status == StatusCode::OK,
                "{message}: {status}"
            );
        }
    }

    /// T-5967, T-6501: the UI tools are offered only while a tab is paired, a UI call goes to the
    /// tab, and `initialize` says the tab follows the work.
    #[tokio::test]
    async fn a_paired_tab_adds_the_ui_tools_and_the_note_and_takes_ui_calls() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector = doprava();
        let now = crate::auth::session::now_unix();
        let names = |body: &Value| -> Vec<String> {
            body["result"]["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t["name"].as_str().map(str::to_owned))
                .collect()
        };
        let (_, _, unpaired) =
            judged(&state, &connector, ana(), rpc("tools/list", json!({}))).await;
        assert!(
            // ana reads nothing and no tab is paired: no tool at all, the UI tools included.
            names(&unpaired).is_empty(),
            "{unpaired}"
        );
        let call = rpc(
            "tools/call",
            json!({ "name": "jc_ui_filter_table",
            "arguments": { "project": "doprava", "query": "air" } }),
        );
        let (_, verdict, refused) = judged(&state, &connector, ana(), call.clone()).await;
        assert_eq!(verdict, "refused: no tab");
        let detail = refused["result"]["structuredContent"]["detail"]
            .as_str()
            .expect("detail");
        assert!(
            !detail.contains("address to open"),
            "no page to name: {detail}"
        );

        let tab = state
            .paired_tabs
            .pair(&connector.id, "f:1:ana", 5, now + 3600);
        let (mut heard, _) = state
            .paired_tabs
            .listen(&connector.id, &tab, "f:1:ana", 5, now + 3600, now)
            .expect("paired");
        let (_, verdict, listed) =
            judged(&state, &connector, ana(), rpc("tools/list", json!({}))).await;
        assert_eq!(verdict, "allowed");
        let tools = names(&listed);
        assert!(
            UI_TOOLS.iter().all(|ui| tools.iter().any(|t| t == ui)),
            "{tools:?}"
        );
        // One project: no tool requires `project`.
        assert!(listed["result"]["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|t| !t["inputSchema"]["required"]
                .as_array()
                .is_some_and(|r| r.contains(&json!("project")))));

        let (_, verdict, shown) = judged(&state, &connector, ana(), call).await;
        assert_eq!(verdict, "allowed");
        assert_eq!(shown["result"]["structuredContent"]["shown"], "filter");
        let action = heard.try_recv().expect("the tab hears the filter");
        assert_eq!(action["frame"], "ui");

        let (status, verdict, init) = judged(
            &state,
            &connector,
            ana(),
            rpc("initialize", json!({ "protocolVersion": "2025-06-18" })),
        )
        .await;
        assert_eq!((status, verdict.as_str()), (StatusCode::OK, "allowed"));
        let instructions = init["result"]["instructions"]
            .as_str()
            .expect("instructions");
        assert!(
            instructions.contains("This connector reaches project doprava;"),
            "{instructions}"
        );
        assert!(instructions.ends_with(PAIRED_TAB_NOTE), "{instructions}");
        assert_eq!(
            init["result"]["capabilities"],
            json!({ "tools": {}, "resources": {} })
        );
    }

    /// AG-158: a scope item with personal data ticked passes the answer as the call made it.
    #[tokio::test]
    async fn a_scope_item_with_personal_data_passes_the_answer_unchanged() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        // The organization default ceiling carries data-read alone.
        let mut connector = connectors::of_conversation("r1", "f:1:ana", "doprava", "data-read");
        connector.scope[0].personal_data = true;
        // An administrator reads every project: the default ceiling reaches doprava.
        let mut admin = ana();
        admin.groups = vec![state.config.bootstrap_admins.clone()];
        let call = rpc(
            "tools/call",
            json!({ "name": "jc_resource_list",
            "arguments": { "project": "doprava", "kind": "Endpoint" } }),
        );
        let (status, verdict, body) = judged(&state, &connector, admin, call).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !verdict.starts_with("refused: target") && !verdict.starts_with("refused: personal"),
            "{verdict} {body}"
        );
        assert_eq!(body["id"], 1);
    }

    #[tokio::test]
    async fn the_challenge_names_the_scope_only_of_a_connector_id() {
        let id = "abcdefghijklmnopqrstuvwxyz";
        assert!(connectors::is_id(id));
        let challenge = |id: &str| {
            unauthorized("https://p/c/x", id)
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .map(|v| v.to_str().expect("text").to_owned())
        };
        assert_eq!(
            challenge(id).as_deref(),
            Some("Bearer resource_metadata=\"https://p/c/x/.well-known/oauth-protected-resource\", scope=\"connector:abcdefghijklmnopqrstuvwxyz\"")
        );
        assert_eq!(
            challenge("x").as_deref(),
            Some("Bearer resource_metadata=\"https://p/c/x/.well-known/oauth-protected-resource\"")
        );
        // A path that decodes to a control character is still a 401, without a challenge.
        let response = unauthorized("https://p/c/a\nb", "a\nb");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let metadata = connector_metadata(State(state), Path("not-an-id".to_owned())).await;
        assert_eq!(metadata.status(), StatusCode::NOT_FOUND);
    }

    /// The door answers 401 without a bearer and when no realm checks the token, before it reads
    /// the connector or the body.
    #[tokio::test]
    async fn no_bearer_and_no_verifier_are_both_401() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let id = "abcdefghijklmnopqrstuvwxyz".to_owned();
        let call = |headers: HeaderMap| {
            handle_connector(
                State(state.clone()),
                Path(id.clone()),
                headers,
                Bytes::from_static(b"{}"),
            )
        };
        assert_eq!(
            call(HeaderMap::new()).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let mut empty = HeaderMap::new();
        empty.insert(header::AUTHORIZATION, "Bearer   ".parse().expect("header"));
        assert_eq!(call(empty).await.status(), StatusCode::UNAUTHORIZED);
        let mut bearer = HeaderMap::new();
        bearer.insert(
            header::AUTHORIZATION,
            "Bearer a.b.c".parse().expect("header"),
        );
        let response = call(bearer).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .is_ok_and(|c| c.contains("scope=\"connector:abcdefghijklmnopqrstuvwxyz\"")));
    }

    /// A connector token signed by `sign` for `issuer`, naming connector `id`'s scope.
    fn connector_token(sign: &dyn Fn(&Value) -> String, issuer: &str, id: &str) -> String {
        let now = crate::auth::session::now_unix();
        sign(&json!({
            "iss": issuer, "typ": "Bearer", "aud": connectors::AUDIENCE, "sub": "f:1:ana",
            "azp": "mcp-local", "exp": now + 300, "iat": now, "scope": connectors::scope(id),
        }))
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().expect("header"),
        );
        headers
    }

    /// Past a token of its own scope: an id that is no connector id is the 404 of no connector,
    /// a connector store that does not answer is its error, and a one-project connector's call
    /// without `project` is handed on with that project filled in.
    #[tokio::test]
    async fn past_the_token_the_door_reads_the_id_the_store_and_the_one_project() {
        let issuer = "https://idm.example.test/realms/dev";
        let (verifier, sign) =
            crate::auth::bearer::test_verifier(issuer, connectors::AUDIENCE, "k1");
        let mut state = AppState::new(crate::config::Config::for_tests(), None);
        state.bearer = Some(std::sync::Arc::new(verifier));
        let call = |state: AppState, id: String, token: String, body: Value| async move {
            handle_connector(
                State(state),
                Path(id),
                bearer(&token),
                Bytes::from(body.to_string()),
            )
            .await
        };
        let odd = "Not-A-Connector";
        let response = call(
            state.clone(),
            odd.to_owned(),
            connector_token(&sign, issuer, odd),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let new = connectors::NewConnector {
            name: "doprava-in-claude".into(),
            segments: vec!["data-read".into()],
            scope: vec![connectors::ScopeItem {
                project: "doprava".into(),
                space: None,
                endpoint: None,
                pipeline: None,
                access: connectors::ItemAccess::Read,
                personal_data: false,
            }],
            kinds: None,
            writes: false,
            autonomous: false,
            expires_at: jc_time::TimestampTz::now() + jiff::SignedDuration::from_hours(24),
            allowed_clients: vec![],
        };
        let held = state
            .connectors
            .create("f:1:ana", new, connectors::Above::default(), &base(&state))
            .await
            .expect("a connector");
        let ping = rpc(
            "tools/call",
            json!({ "name": "jc_resource_list", "arguments": { "kind": "Endpoint" } }),
        );
        let token = connector_token(&sign, issuer, &held.id);
        let response = call(state.clone(), held.id.clone(), token.clone(), ping).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_ne!(response.status(), StatusCode::NOT_FOUND);

        let dead = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool");
        let mut broken = state.clone();
        broken.connectors = std::sync::Arc::new(connectors::ConnectorStore::new(
            Some((dead, broken.mirror.clone())),
            Some("helsinki".into()),
        ));
        let response = call(broken, held.id.clone(), token, json!({})).await;
        assert!(response.status().is_server_error(), "{}", response.status());
    }

    /// A token of the request's Organization realm is checked by that realm, and the
    /// installation's administrator group it names is dropped before anything is read (T-7001).
    #[tokio::test]
    async fn an_organization_realms_token_is_checked_there_and_confined() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let certs = MockServer::start().await;
        let (signer, set) = crate::auth::bearer::tests::keypair("org-key");
        Mock::given(method("GET"))
            .and(path("/realms/hel/protocol/openid-connect/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&set))
            .mount(&certs)
            .await;
        let mut config = crate::config::Config::for_tests();
        config.oidc = Some(crate::config::OidcConfig {
            issuer: "https://idm.example.test/realms/jc".parse().expect("url"),
            client_id: "portal-api".into(),
            auth: crate::auth::client_auth::ClientAuth::Secret("s".into()),
            extra_ca_pem: None,
            admin_url: None,
        });
        let hosts = crate::organization_host::parse("hel=https://portal.hel.test").expect("hosts");
        config.organization_hosts = hosts.clone();
        let state = AppState::new(config, None);
        let issuer = "https://idm-conn-7931.hel.test/realms/hel";
        let port = certs.address().port();
        let raw = format!(
            r#"[{{"organization":"hel","issuer":"{issuer}","jwks_url":"http://idm-conn-7931.hel.test:{port}/realms/hel/protocol/openid-connect/certs"}}]"#
        );
        assert_eq!(
            state.replace_organization_realms(&raw, Some(*certs.address())),
            Ok(1)
        );
        let hel = state.organization_realms.get("hel").expect("served");
        assert_eq!(hel.bearer.refresh().await.ok(), Some(1));
        let sign = |claims: &Value| {
            let mut head = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
            head.kid = Some("org-key".into());
            jsonwebtoken::encode(&head, claims, &signer).expect("sign")
        };
        let odd = "Not-A-Connector";
        let token = connector_token(&sign, issuer, odd);
        let host = hosts.into_iter().next().expect("hel");
        let response = crate::organization_host::scope(
            host,
            handle_connector(
                State(state.clone()),
                Path(odd.to_owned()),
                bearer(&token),
                Bytes::from_static(b"{}"),
            ),
        )
        .await;
        // Checked by the realm (not the 401 of a token nobody here checks), then no connector.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_connector_door_takes_post_alone_and_a_limit_says_when_to_retry() {
        use tower::ServiceExt;
        let config = crate::config::Config::for_tests();
        let state = AppState::new(config.clone(), None);
        let app = router(&config).with_state(state);
        let get = axum::http::Request::builder()
            .uri("/c/abcdefghijklmnopqrstuvwxyz")
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(get).await.expect("answer");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let limited = limited("too many", 7);
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(limited.headers()[header::RETRY_AFTER], "7");
    }

    /// A kind bound in one list and none in another keeps the bound; only `endpoint` opens.
    #[test]
    fn a_kind_list_without_a_bound_keeps_the_other_lists_kinds() {
        let tool = |kinds: Option<&[&str]>| {
            let mut kind = json!({ "type": "string" });
            if let Some(kinds) = kinds {
                kind["enum"] = json!(kinds);
            }
            vec![
                json!({ "name": "jc_resource_list", "inputSchema": { "properties": { "kind": kind } } }),
            ]
        };
        let both = merged([tool(Some(&["Endpoint"])), tool(None)].into_iter());
        assert_eq!(
            both[0]["inputSchema"]["properties"]["kind"]["enum"],
            json!(["Endpoint"])
        );
        let none = merged([tool(None), tool(Some(&["Pipeline"]))].into_iter());
        assert!(none[0]["inputSchema"]["properties"]["kind"]
            .get("enum")
            .is_none());
    }

    #[test]
    fn only_a_question_or_an_applied_change_needs_the_person() {
        let apply = |requests: Value| json!({ "result": { "structuredContent": { "changeRequests": requests } } });
        assert_eq!(
            page_needing_the_person("p", "jc_change_apply", &apply(json!([]))),
            None
        );
        assert_eq!(
            page_needing_the_person("p", "jc_change_apply", &apply(json!([{ "id": 7 }]))),
            None
        );
        assert_eq!(
            page_needing_the_person("p", "jc_change_apply", &apply(json!("x"))),
            None
        );
        assert_eq!(
            page_needing_the_person("p", "jc_data_query", &apply(json!([{ "id": "a" }]))),
            None
        );
    }

    /// An answer larger than the door reads is a 500 at each step that re-reads it, never a
    /// truncated answer passed on.
    #[tokio::test]
    async fn an_answer_past_the_door_limit_is_never_passed_on_cut() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let connector = doprava();
        let now = crate::auth::session::now_unix();
        let huge = || Response::new(Body::from(vec![b'x'; MAX_RESPONSE_BYTES + 1]));
        let (response, verdict) = verdict_of(huge(), json!(1), usize::MAX).await;
        assert_eq!(
            (response.status(), verdict.as_str()),
            (StatusCode::INTERNAL_SERVER_ERROR, "error")
        );
        assert_eq!(
            without_personal(&state, Some("doprava"), huge())
                .await
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let portal: url::Url = "https://portal.test/".parse().expect("url");
        assert_eq!(
            with_projects_named(huge(), &["doprava"], &portal, false)
                .await
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        let tab = state
            .paired_tabs
            .pair(&connector.id, "f:1:ana", 5, now + 3600);
        let _listening = state
            .paired_tabs
            .listen(&connector.id, &tab, "f:1:ana", 5, now + 3600, now)
            .expect("paired");
        let call = Followed {
            connector: &connector,
            client: None,
            project: Some("doprava"),
            tool: "jc_data_query",
        };
        assert_eq!(
            follow_in_tab(&state, &call, huge(), "allowed")
                .await
                .status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        // A refused call or one without a project is never followed: the answer passes as it is.
        let small = || json_response(StatusCode::OK, &json!({ "id": 1 }));
        assert_eq!(
            answer_of(follow_in_tab(&state, &call, small(), "refused: tool").await).await["id"],
            1
        );
        let nowhere = Followed {
            project: None,
            ..call
        };
        assert_eq!(
            answer_of(follow_in_tab(&state, &nowhere, small(), "allowed").await).await["id"],
            1
        );
    }

    /// T-6064: an answer without instructions passes; several projects are named, each call names one.
    #[tokio::test]
    async fn initialize_names_every_project_of_a_wider_connector() {
        let portal: url::Url = "https://portal.test/".parse().expect("url");
        let plain = json_response(StatusCode::OK, &json!({ "result": { "serverInfo": {} } }));
        let kept = answer_of(with_projects_named(plain, &["a", "b"], &portal, false).await).await;
        assert_eq!(kept, json!({ "result": { "serverInfo": {} } }));
        let init = json_response(
            StatusCode::OK,
            &json!({ "result": { "instructions": "Hi." } }),
        );
        let named = answer_of(with_projects_named(init, &["a", "b"], &portal, false).await).await;
        let text = named["result"]["instructions"].as_str().expect("text");
        assert!(
            text.starts_with(
                "Hi. This connector reaches projects a, b; name one as `project` in each call."
            ),
            "{text}"
        );
        assert!(
            text.contains("https://portal.test/projects/<project>/explore?"),
            "{text}"
        );
        assert!(!text.contains(PAIRED_TAB_NOTE));
    }

    #[test]
    fn a_wider_connector_defaults_no_project_and_odd_schemas_stay() {
        let mut two = doprava();
        let mut other = two.scope[0].clone();
        other.project = "espoo".to_owned();
        two.scope.push(other);
        let mut call =
            json!({ "method": "tools/call", "params": { "name": "x", "arguments": {} } });
        assert!(!defaults_project(&two, &mut call));
        assert!(call["params"]["arguments"].get("project").is_none());
        let mut flat = json!({ "method": "tools/call", "params": "x" });
        assert!(!defaults_project(&doprava(), &mut flat));
        assert_eq!(flat["params"], "x");
        let mut tools = vec![
            json!({ "inputSchema": { "required": "project" } }),
            json!({}),
        ];
        optional_project(&mut tools);
        assert_eq!(tools[0]["inputSchema"]["required"], "project");
    }

    /// T-6867: a read past the door through no connector is the caller's own; one through a
    /// connector is held to its scope.
    #[tokio::test]
    async fn a_read_past_the_door_is_checked_only_through_a_connector() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let own = crate::ops::Caller::new(ana(), crate::ops::Via::Mcp);
        assert_eq!(admits_read(&state, &own, "espoo", &json!({})).await, Ok(()));
        let through = caller(&doprava(), ana(), "t");
        let refused = admits_read(
            &state,
            &through,
            "espoo",
            &json!({ "data": { "query": { "endpoint": "air" } } }),
        )
        .await
        .expect_err("another project");
        assert!(refused.contains("espoo"), "{refused}");
    }

    /// AG-158: an indicator read from an Endpoint names its project; a call of another tool, or
    /// an indicator from rows the client sent, is not checked.
    #[tokio::test]
    async fn an_indicator_from_an_endpoint_names_its_project() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let refused = indicator_stays_open(
            &state,
            None,
            "jc_kpi_compute",
            &json!({ "endpoint": "air", "attribute": "pm10" }),
        )
        .await
        .expect_err("no project");
        assert!(refused.contains("names its project"), "{refused}");
        assert_eq!(
            indicator_stays_open(&state, None, "jc_kpi_compute", &json!({ "rows": [] })).await,
            Ok(())
        );
        assert_eq!(
            indicator_stays_open(&state, None, "jc_resource_list", &json!({})).await,
            Ok(())
        );
        let entities = json_response(
            StatusCode::OK,
            &json!({ "jsonrpc": "2.0", "id": 1,
            "result": { "structuredContent": { "entities": [{ "id": "urn:ngsi-ld:A:1", "type": "A" }] } } }),
        );
        let answer = answer_of(without_personal(&state, None, entities).await).await;
        assert_eq!(answer["result"]["isError"], true, "{answer}");
        assert!(answer["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("names its project")));
    }

    #[test]
    fn a_refusal_without_grounding_carries_none() {
        let answer = without(
            json!({ "structuredContent": {} }),
            Err("no models".to_owned()),
        );
        assert_eq!(answer["isError"], true);
        assert!(answer.get("_meta").is_none(), "{answer}");
    }

    /// T-6442: on a Portal whose address is no base for a path, the refusal names the route alone.
    #[tokio::test]
    async fn a_portal_address_that_is_no_base_names_the_route_alone() {
        let mut config = crate::config::Config::for_tests();
        config.public_base_url = "mailto:portal@example.org".parse().expect("url");
        let state = AppState::new(config, None);
        let navigate = json!({ "project": "doprava", "page": "spaces" });
        let (response, verdict) = tab_action(
            &state,
            &doprava(),
            &ana(),
            "jc_ui_navigate",
            &navigate,
            json!(1),
        );
        assert_eq!(verdict, "refused: no tab");
        let detail = answer_of(response).await["result"]["structuredContent"]["detail"].to_string();
        assert!(
            detail.contains("this address to open: /projects/doprava/spaces"),
            "{detail}"
        );
    }

    /// JC1-22, T-6058: a connector over two projects keeps `project` required in every tool.
    #[tokio::test]
    async fn a_wider_connector_keeps_project_required() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let mut two = doprava();
        let mut other = two.scope[0].clone();
        other.project = "espoo".to_owned();
        two.scope.push(other);
        let now = crate::auth::session::now_unix();
        let tab = state.paired_tabs.pair(&two.id, "f:1:ana", 5, now + 3600);
        let _listening = state
            .paired_tabs
            .listen(&two.id, &tab, "f:1:ana", 5, now + 3600, now)
            .expect("paired");
        let (_, verdict, body) = judged(&state, &two, ana(), rpc("tools/list", json!({}))).await;
        assert_eq!(verdict, "allowed");
        let tools = body["result"]["tools"].as_array().expect("tools");
        assert!(!tools.is_empty());
        assert!(tools.iter().all(|t| t["inputSchema"]["required"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("project")))));
    }
}
