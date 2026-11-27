//! Adding a device to an MQTT broker (T-5532, PF-47, ADR-N-048 §5, API/01 §56).
//!
//! A device signs in by its own key pair: a `ServiceAccount` whose `oauth-client` credential
//! carries the public half as `jwks` becomes the Keycloak client `{project}-{name}` with
//! `client-jwt` (T-4015). The brokers page generates the key pair in the person's browser or takes
//! a pasted public key; only the public key reaches this door, which refuses any private member.
//! One Change writes the account and adds it to the broker role the person picked; it always waits
//! for a second person, because it lets a new identity publish.
//!
//! `GET .../devices` lists what the form may offer: the broker's roles with their publish filters,
//! empty for a caller who may not propose both kinds, so the dropdowns hold only what the `POST`
//! accepts. The `POST` checks each value again.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::service_account::Jwks;
use jc_core::kinds::{MqttBrokerSpec, Verb};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::import::{digest, propose_files, riskiest, Proposal};
use crate::api::mutate::{propose_with_identity, ProposeOutcome};
use crate::auth::CurrentUser;
use crate::change::{Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{self, ResourceEnvelope, API_VERSION};
use crate::state::AppState;

const BROKER: &str = "MqttBroker";
const ACCOUNT: &str = "ServiceAccount";
/// The credential's name in every device account this door writes.
const CREDENTIAL: &str = "device-key";
/// The one grant of a device account, on its project: jc-core wants one, and this name is no
/// Portal `Role` and scopes no space, so it gives no Portal right and no Endpoint audience; the
/// broker audience comes from the broker role that lists the account (`workload_clients`).
const DEVICE_ROLE: &str = "mqtt-device";
/// JWK members of a private or symmetric key (RFC 7518 §6): none may leave the device.
const PRIVATE_MEMBERS: [&str; 8] = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];

/// One role the form may offer, with the topic filters a device of it may publish to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRole {
    pub name: String,
    pub publish: Vec<String>,
}

/// What the Add device form of one broker may offer the caller.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOptions {
    /// Whether the caller may propose a device here: `ServiceAccount` and this `MqttBroker`.
    pub may_add: bool,
    /// The broker's roles, empty unless `mayAdd`.
    pub roles: Vec<DeviceRole>,
    /// The realm's token endpoint a device asks for its access token, absent without a login.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// The `aud` of the device's client assertion: the realm issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
}

/// A device to add: its account name, the role and topic picked, and its public keys.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AddDevice {
    /// The ServiceAccount's name, a DNS label such as `sensor-001`.
    pub name: String,
    /// One of the broker's roles.
    pub role: String,
    /// One of the role's publish filters; required when the role publishes.
    #[serde(default)]
    pub topic: Option<String>,
    /// A JSON Web Key Set of public keys only (PF-47).
    pub jwks: Value,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/mqttbrokers/{name}/devices",
        get(device_options).post(add_device),
    )
}

/// The broker, or the 404 of one that does not exist for a caller who may not read brokers (PF-59).
fn broker(
    state: &AppState,
    effective: &crate::permissions::Effective,
    project: &str,
    name: &str,
) -> Result<(ResourceEnvelope, MqttBrokerSpec), ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "MQTT broker '{name}' not found in project '{project}'"
        ))
    };
    if !effective.may_read_project() || !effective.may_read(BROKER) {
        return Err(not_found());
    }
    let mut env = state
        .mirror
        .get(project, BROKER, name)
        .ok_or_else(not_found)?;
    env.strip_status();
    let spec = serde_json::from_value(env.spec.clone()).map_err(|e| {
        ApiError::Conflict(format!(
            "MQTT broker '{name}' in the repository does not parse ({e}); fix its manifest first"
        ))
    })?;
    Ok((env, spec))
}

fn as_value(env: &ResourceEnvelope) -> Result<Value, ApiError> {
    serde_json::to_value(env).map_err(ApiError::internal)
}

fn issuer(state: &AppState) -> Option<String> {
    state
        .config
        .oidc
        .as_ref()
        .map(|oidc| oidc.issuer.as_str().trim_end_matches('/').to_owned())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/devices",
    summary = "What The Add Device Form Of A Broker May Offer",
    description = "The broker's roles with their publish filters, listed only for a person who may propose a ServiceAccount and this MqttBroker, and the realm's token endpoint a device asks (T-5532, PF-47). Whoever may read the project's MqttBrokers.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    responses(
        (status = 200, description = "The form's options", body = DeviceOptions),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
    )
)]
pub async fn device_options(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<DeviceOptions>, ApiError> {
    let identity = &user.0.identity;
    let effective = crate::permissions::for_request(&state, identity, &project);
    let (env, spec) = broker(&state, &effective, &project, &name)?;
    let may_add = may_add_here(identity, &effective, &env).is_ok();
    Ok(Json(options(spec, may_add, issuer(&state))))
}

/// The form's options: the broker's roles only for a caller who may add a device.
fn options(spec: MqttBrokerSpec, may_add: bool, issuer: Option<String>) -> DeviceOptions {
    let roles = if may_add {
        spec.roles
            .into_iter()
            .map(|role| DeviceRole {
                name: role.name,
                publish: role.publish,
            })
            .collect()
    } else {
        Vec::new()
    };
    DeviceOptions {
        may_add,
        roles,
        token_url: issuer
            .as_ref()
            .map(|issuer| format!("{issuer}/protocol/openid-connect/token")),
        issuer,
    }
}

fn unprocessable(detail: &str, error: String) -> ApiError {
    ApiError::Unprocessable {
        detail: detail.to_owned(),
        errors: vec![error],
    }
}

/// The pasted or generated key set, public keys only and every PF-47 rule held.
pub fn public_jwks(value: &Value) -> Result<Jwks, ApiError> {
    const DETAIL: &str = "the device's key set is not a public JSON Web Key Set";
    let keys = value.get("keys").and_then(Value::as_array).ok_or_else(|| {
        unprocessable(
            DETAIL,
            "jwks: a JSON Web Key Set is an object {\"keys\": [...]}".to_owned(),
        )
    })?;
    for (at, key) in keys.iter().enumerate() {
        if let Some(member) = PRIVATE_MEMBERS.iter().find(|m| key.get(**m).is_some()) {
            return Err(unprocessable(
                DETAIL,
                format!(
                    "jwks.keys[{at}].{member}: a private key member; send the public key only, the private key stays on the device"
                ),
            ));
        }
    }
    let jwks: Jwks = serde_json::from_value(value.clone())
        .map_err(|e| unprocessable(DETAIL, format!("jwks: {e}")))?;
    jwks.validate()
        .map_err(|e| unprocessable(DETAIL, format!("jwks: {e}")))?;
    Ok(jwks)
}

/// The account manifest of a device that signs in by `jwks` alone.
fn device_account(
    project: &str,
    request: &AddDevice,
    jwks: &Jwks,
    owner: &str,
    broker: &str,
) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": ACCOUNT,
        "metadata": { "name": request.name, "namespace": project },
        "spec": {
            "owner": { "user": owner },
            "purpose": format!("MQTT device on broker {broker}, role {}", request.role),
            "roles": [{ "role": DEVICE_ROLE, "scope": { "project": project } }],
            "credentials": [{ "kind": "oauth-client", "name": CREDENTIAL, "jwks": jwks }],
        },
    })
}

/// The single-manifest door as a dry run: permission, invariants and lane, a refusal named.
async fn checked(
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    name: &str,
    operation: Operation,
    manifest: Value,
) -> Result<Lane, ApiError> {
    let outcome = propose_with_identity(
        identity,
        state,
        project,
        plural,
        (operation == Operation::Update).then_some(name),
        operation,
        true,
        manifest,
    )
    .await?;
    lane_of(plural, name, outcome)
}

/// The lane of a dry run's plan; a plan that does not pass its door is refused with its findings.
fn lane_of(plural: &str, name: &str, outcome: ProposeOutcome) -> Result<Lane, ApiError> {
    match outcome {
        ProposeOutcome::DryRun(result) if result.valid => Ok(result.lane),
        ProposeOutcome::DryRun(result) => Err(ApiError::Invalid {
            detail: format!("{plural}/{name} does not pass its door"),
            errors: result.findings,
        }),
        _ => Err(ApiError::Internal(
            "a dry run answered with something other than its plan".into(),
        )),
    }
}

fn file(kind: &str, project: &str, manifest: &Value) -> Result<(String, String), ApiError> {
    let info = crate::api::mutate::catalogued(kind)?;
    let name = manifest["metadata"]["name"].as_str().unwrap_or_default();
    let path =
        resource::repository_path(info, project, None, name).map_err(ApiError::BadRequest)?;
    let content = serde_norway::to_string(manifest).map_err(ApiError::internal)?;
    Ok((path, content))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/devices",
    summary = "Add A Device To An MQTT Broker",
    description = "Proposes one Change, in the caller's name, that writes a ServiceAccount signing in by the public keys given (`spec.credentials[].jwks`, PF-47) and adds it to one role of the broker. A private key member is refused (422 naming it); role and topic must be the broker's own (422 naming the field). The Change always waits for a second person (T-5532).",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
    ),
    request_body(
        content = AddDevice,
        example = json!({
            "name": "sensor-001",
            "role": "camera",
            "topic": "cameras/${clientid}/#",
            "jwks": { "keys": [{ "kty": "EC", "crv": "P-256", "kid": "k1",
                "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
                "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" }] }
        })
    ),
    responses(
        (status = 202, description = "The Change that adds the device, waiting for a second person", body = crate::change::Change),
        (status = 400, description = "The body does not read, or a manifest does not pass its door", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not propose a ServiceAccount or this MqttBroker, or is not a person", body = ProblemDetails),
        (status = 404, description = "No such broker, or not readable by the caller", body = ProblemDetails),
        (status = 409, description = "A ServiceAccount of that name exists", body = ProblemDetails),
        (status = 422, description = "A field names what the broker or PF-47 refuses", body = ProblemDetails),
        (status = 503, description = "No forge", body = ProblemDetails),
    )
)]
pub async fn add_device(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, broker_name)): Path<(String, String)>,
    body: Result<Json<AddDevice>, JsonRejection>,
) -> Result<Response, ApiError> {
    let change = add(&state, &user.0.identity, &project, &broker_name, body).await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// A person who may propose a ServiceAccount and this broker; an agent is refused.
fn may_add_here(
    identity: &crate::auth::Identity,
    effective: &crate::permissions::Effective,
    env: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a person adds a device at the Portal; an agent proposes the manifests with jc_resource_propose".into(),
        ));
    }
    effective.check(ACCOUNT, Verb::Propose, None)?;
    effective.check(BROKER, Verb::Propose, Some(&as_value(env)?))?;
    Ok(())
}

/// The checked request proposed as one Change: the caller's rights first, then the body.
async fn add(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    broker_name: &str,
    body: Result<Json<AddDevice>, JsonRejection>,
) -> Result<crate::change::Change, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    let (env, spec) = broker(state, &effective, project, broker_name)?;
    may_add_here(identity, &effective, &env)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let jwks = valid_request(state, project, broker_name, &spec, &request)?;
    let change =
        propose_device(state, identity, project, broker_name, &request, &jwks, env).await?;
    tracing::info!(project = %project, broker = %broker_name, device = %request.name, by = %identity.subject, "MQTT device proposed");
    Ok(change)
}

/// The request's name, role, topic and key set checked against the broker; the public keys.
fn valid_request(
    state: &AppState,
    project: &str,
    broker_name: &str,
    spec: &MqttBrokerSpec,
    request: &AddDevice,
) -> Result<Jwks, ApiError> {
    if !resource::is_dns1123(&request.name) {
        return Err(unprocessable(
            "the device's name is not a DNS label",
            format!("name: '{}' is not a DNS label of lowercase letters, digits and '-', such as sensor-001", request.name),
        ));
    }
    if state.mirror.get(project, ACCOUNT, &request.name).is_some() {
        return Err(ApiError::Conflict(format!(
            "a ServiceAccount '{}' exists in project '{project}'; pick another name, or rotate its key on its own page",
            request.name
        )));
    }
    let Some(role) = spec.roles.iter().find(|role| role.name == request.role) else {
        return Err(no_role(broker_name, &request.role));
    };
    match (&request.topic, role.publish.is_empty()) {
        (Some(topic), _) if !role.publish.contains(topic) => {
            return Err(unprocessable(
                "the topic is outside the role",
                format!(
                    "topic: role '{}' publishes only to {}",
                    role.name,
                    role.publish.join(", ")
                ),
            ))
        }
        (None, false) => {
            return Err(unprocessable(
                "the topic is missing",
                format!(
                    "topic: pick one of role '{}''s filters: {}",
                    role.name,
                    role.publish.join(", ")
                ),
            ))
        }
        _ => {}
    }
    public_jwks(&request.jwks)
}

/// Names `account` in each of the spec's roles called `role`, once: a stale manifest that
/// lists it already keeps one entry. False when the spec holds no such role.
fn add_to_role(spec: &mut Value, role: &str, account: &str) -> bool {
    let mut matched = false;
    let roles = spec.get_mut("roles").and_then(Value::as_array_mut);
    for entry in roles
        .into_iter()
        .flatten()
        .filter(|r| r["name"] == role)
        .filter_map(Value::as_object_mut)
    {
        matched = true;
        let accounts = entry
            .entry("serviceAccounts")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(accounts) = accounts.as_array_mut() {
            if !accounts.iter().any(|a| a == account) {
                accounts.push(Value::String(account.to_owned()));
            }
        }
    }
    matched
}

fn no_role(broker_name: &str, role: &str) -> ApiError {
    unprocessable(
        "the role is not one of the broker's",
        format!("role: broker '{broker_name}' has no role '{role}'"),
    )
}

/// The one Change that writes the device's account and adds it to its role of the broker.
async fn propose_device(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    broker_name: &str,
    request: &AddDevice,
    jwks: &Jwks,
    mut env: ResourceEnvelope,
) -> Result<crate::change::Change, ApiError> {
    let account = device_account(project, request, jwks, &identity.username, broker_name);
    // A role written without `serviceAccounts` gets the list; a role the broker does not hold is
    // a refusal, never a Change that adds nobody (F-POR2-013).
    if !add_to_role(&mut env.spec, &request.role, &request.name) {
        return Err(no_role(broker_name, &request.role));
    }
    let broker_manifest = as_value(&env)?;

    let lane = riskiest(
        checked(
            identity,
            state,
            project,
            "serviceaccounts",
            &request.name,
            Operation::Create,
            account.clone(),
        )
        .await?,
        // The account the role now lists is the other file of this Change (MF-13, T-5590).
        crate::references::BUNDLE
            .scope(
                vec![account.clone()],
                checked(
                    identity,
                    state,
                    project,
                    "mqttbrokers",
                    broker_name,
                    Operation::Update,
                    broker_manifest.clone(),
                ),
            )
            .await?,
    );
    let files = vec![
        file(ACCOUNT, project, &account)?,
        file(BROKER, project, &broker_manifest)?,
    ];
    let branch = format!(
        "portal/add-device-{project}-{}-{:08x}",
        request.name,
        digest(&files)
    );
    let body = format!(
        "Adds the device `{device}` to MQTT broker `{broker_name}`, role `{role}` (T-5532, PF-47):\n\n\
         - ServiceAccount `{device}`: signs in as Keycloak client `{project}-{device}` with its own key pair; \
         the Change holds the public key{plural} only ({kids}).\n\
         - MqttBroker `{broker_name}`: role `{role}` lists `{device}`{topic}.\n\n\
         Approve only if you know who holds the private key.",
        device = request.name,
        role = request.role,
        plural = if jwks.keys.len() == 1 { "" } else { "s" },
        kids = jwks.keys.iter().map(|k| format!("kid `{}`", k.kid)).collect::<Vec<_>>().join(", "),
        topic = request.topic.as_deref().map(|t| format!(" and publishes to `{t}`")).unwrap_or_default(),
    );
    propose_files(
        state,
        identity,
        project,
        Proposal {
            files,
            branch,
            title: format!("add MQTT device {} to {broker_name}", request.name),
            body,
            lane,
            summary: PlanSummary::new(1, 1, 0),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Value {
        json!({ "kty": "EC", "crv": "P-256", "kid": "k1",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" })
    }

    fn refusal(value: Value) -> String {
        let err = public_jwks(&value).expect_err("a refusal");
        assert!(
            matches!(err, ApiError::Unprocessable { .. }),
            "not a 422: {err:?}"
        );
        format!("{err:?}")
    }

    fn dry_run(valid: bool, findings: Vec<String>) -> ProposeOutcome {
        ProposeOutcome::DryRun(crate::api::dry_run::DryRunResult {
            valid,
            lane: Lane::Yellow,
            plan: crate::plan::PlanDiff::empty(),
            restarts_stream: false,
            probe: None,
            verdict: None,
            findings,
            awaited: Vec::new(),
            goes_with: Vec::new(),
            binds_endpoints: Vec::new(),
        })
    }

    #[test]
    fn a_dry_run_that_does_not_pass_its_door_is_refused_with_its_findings() {
        assert_eq!(
            lane_of("mqttbrokers", "edge", dry_run(true, Vec::new())).expect("a lane"),
            Lane::Yellow
        );
        let refused = lane_of(
            "mqttbrokers",
            "edge",
            dry_run(false, vec!["spec.roles[0]: names no ServiceAccount".into()]),
        );
        assert!(
            matches!(&refused, Err(ApiError::Invalid { detail, errors })
                if detail == "mqttbrokers/edge does not pass its door"
                    && errors == &["spec.roles[0]: names no ServiceAccount".to_string()]),
            "{refused:?}"
        );
        let committed = ProposeOutcome::Bundled(crate::api::mutate::BundledCommit {
            path: "projects/p/mqttbrokers/edge.yaml".into(),
            lane: Lane::Yellow,
            waits_on: Vec::new(),
        });
        let odd = lane_of("mqttbrokers", "edge", committed);
        assert!(
            matches!(&odd, Err(ApiError::Internal(m)) if m.contains("other than its plan")),
            "{odd:?}"
        );
    }

    /// A kind the catalogue does not hold has no file to write.
    #[test]
    fn a_kind_outside_the_catalogue_has_no_file() {
        let refused = file("NoSuchKind", "p", &json!({ "metadata": { "name": "x" } }));
        assert!(
            matches!(&refused, Err(ApiError::Internal(m)) if m.contains("NoSuchKind")),
            "{refused:?}"
        );
    }

    /// A broker whose manifest holds no such role adds nobody: 422 naming the role, and
    /// nothing is proposed.
    #[tokio::test]
    async fn a_device_for_a_role_the_broker_lacks_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let person = crate::ops::testkit::person("ana", &[]);
        let request = AddDevice {
            name: "meter-1".into(),
            role: "publisher".into(),
            topic: None,
            jwks: json!({ "keys": [key()] }),
        };
        let jwks = public_jwks(&request.jwks).expect("a public key set");
        let env: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "MqttBroker",
            "metadata": { "name": "edge", "namespace": "helsinki" },
            "spec": { "roles": [{ "name": "reader" }] }
        }))
        .expect("an envelope");
        let refused =
            propose_device(&state, &person, "helsinki", "edge", &request, &jwks, env).await;
        assert!(
            matches!(&refused, Err(ApiError::Unprocessable { errors, .. })
                if errors == &["role: broker 'edge' has no role 'publisher'".to_string()]),
            "{refused:?}"
        );
    }

    #[test]
    fn a_role_names_the_device_once_and_an_unknown_role_adds_nobody() {
        let mut spec = json!({ "roles": [
            { "name": "publisher", "serviceAccounts": ["meter-1"] },
            { "name": "reader" },
            "not a role",
        ] });
        assert!(
            add_to_role(&mut spec, "publisher", "meter-1"),
            "a stale manifest"
        );
        assert_eq!(spec["roles"][0]["serviceAccounts"], json!(["meter-1"]));
        assert!(add_to_role(&mut spec, "publisher", "meter-2"));
        assert_eq!(
            spec["roles"][0]["serviceAccounts"],
            json!(["meter-1", "meter-2"])
        );
        assert!(
            add_to_role(&mut spec, "reader", "meter-3"),
            "a role with no list"
        );
        assert_eq!(spec["roles"][1]["serviceAccounts"], json!(["meter-3"]));
        let before = spec.clone();
        assert!(!add_to_role(&mut spec, "admin", "meter-4"));
        assert!(
            !add_to_role(&mut json!({}), "publisher", "meter-4"),
            "no roles at all"
        );
        let mut odd = json!({ "roles": [{ "name": "publisher", "serviceAccounts": "meter-1" }] });
        assert!(
            add_to_role(&mut odd, "publisher", "meter-5"),
            "the role is there"
        );
        assert_eq!(
            odd["roles"][0]["serviceAccounts"],
            json!("meter-1"),
            "a list that is no list is left for the door to refuse"
        );
        assert_eq!(spec, before);
    }

    #[test]
    fn an_agents_token_may_not_add_a_device_whatever_its_rights() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let agent = crate::api::mcp_elicitations::tests::client_session()
            .0
            .identity;
        let effective = crate::permissions::for_request(&state, &agent, "helsinki");
        let env: ResourceEnvelope = serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "MqttBroker",
            "metadata": { "name": "edge", "namespace": "helsinki" }, "spec": { "roles": [] }
        }))
        .expect("an envelope");
        let refused = may_add_here(&agent, &effective, &env);
        assert!(
            matches!(&refused, Err(ApiError::Denied(why)) if why.contains("jc_resource_propose")),
            "{refused:?}"
        );
    }

    #[test]
    fn a_public_key_set_passes_and_a_private_member_or_no_set_is_named() {
        let jwks = public_jwks(&json!({ "keys": [key()] })).expect("a public key set");
        assert_eq!(jwks.keys.len(), 1);
        let mut private = key();
        private["d"] = json!("ZGVtbw");
        assert!(refusal(json!({ "keys": [private] }))
            .contains("\"jwks.keys[0].d: a private key member"));
        assert!(refusal(json!([key()])).contains("\"jwks: a JSON Web Key Set is an object"));
        assert!(
            refusal(json!({ "keys": [] })).contains("\"jwks: "),
            "an empty key set"
        );
        let mut unknown = key();
        unknown["x5u"] = json!("https://keys.example.org/k1");
        assert!(
            refusal(json!({ "keys": [unknown] })).contains("\"jwks: unknown field"),
            "a member no public JWK has"
        );
        let six: Vec<Value> = (0..6)
            .map(|n| {
                let mut each = key();
                each["kid"] = json!(format!("k{n}"));
                each
            })
            .collect();
        assert!(
            refusal(json!({ "keys": six })).contains("one to five keys"),
            "six keys"
        );
    }
}
