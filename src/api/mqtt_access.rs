//! `GET /api/v1/projects/{project}/mqttbrokers/{name}/access?account=<sa>`: which roles of a
//! broker a ServiceAccount holds now and why (API/01 §47, ADR-N-048 §2.13, T-5876).
//!
//! The answer is `jc_core::mqtt::roles_held` at the time of the request, the decision the gateway
//! makes at CONNECT and at every publish; the route reads the two manifests from the mirror and
//! decides nothing of its own. Beside it, the account's grants of this broker outside their
//! window, so a person sees why a device that held a role holds none now.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use jc_core::kinds::{MqttBrokerSpec, RoleBinding, ServiceAccountSpec};
use jc_core::mqtt::Grant;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::session::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// The account the broker's roles are explained for.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct AccessQuery {
    /// A ServiceAccount of the broker's project, by name, e.g. `air-fleet`.
    pub account: Option<String>,
}

/// One role the account holds.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeldRole {
    /// The role's name on the broker.
    pub role: String,
    /// Topic filters the account's devices may publish to.
    pub publish: Vec<String>,
    /// Topic filters they may subscribe to.
    pub subscribe: Vec<String>,
    /// Held through the role's deprecated `serviceAccounts` list.
    pub legacy: bool,
    /// Held through a RoleBinding `mqtt/<broker>/<role>` or `mqtt/*/<role>` in force.
    pub bound: bool,
    /// When the last grant of the role ends; absent while one has no end.
    #[schema(value_type = Option<String>, format = DateTime)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
}

/// One grant of this broker the account carries that is not in force now.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutOfWindow {
    /// The binding's role, e.g. `mqtt/air/sensor`.
    pub grant: String,
    #[schema(value_type = Option<String>, format = DateTime)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[schema(value_type = Option<String>, format = DateTime)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
}

/// The roles of a broker one account holds at `at`.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    pub broker: String,
    pub account: String,
    /// The instant the roles were decided at.
    #[schema(value_type = String, format = DateTime)]
    pub at: DateTime<Utc>,
    /// Empty when the account holds no role: its devices are refused at CONNECT.
    pub roles: Vec<HeldRole>,
    /// Grants of this broker outside their validity window.
    pub out_of_window: Vec<OutOfWindow>,
}

/// The answer for `account` on broker `broker` at `now`.
pub fn access(
    project: &str,
    broker: &str,
    spec: &MqttBrokerSpec,
    name: &str,
    bindings: &[RoleBinding],
    now: DateTime<Utc>,
) -> Access {
    let roles = crate::mqtt_roles::held(project, broker, spec, name, bindings, now)
        .into_iter()
        .map(|held| HeldRole {
            role: held.role.name.clone(),
            publish: held.role.publish.clone(),
            subscribe: held.role.subscribe.clone(),
            legacy: held.legacy,
            bound: held.bound,
            until: held.until,
        })
        .collect();
    let names_this_broker = |role: &str| {
        Grant::parse(role)
            .and_then(Result::ok)
            .is_some_and(|grant| spec.roles.iter().any(|r| grant.names(broker, &r.name)))
    };
    let out_of_window = bindings
        .iter()
        .filter(|binding| names_this_broker(&binding.role) && !binding.in_force(now))
        .map(|binding| OutOfWindow {
            grant: binding.role.clone(),
            not_before: binding.validity.as_ref().and_then(|v| v.not_before),
            not_after: binding.validity.as_ref().and_then(|v| v.not_after),
        })
        .collect();
    Access {
        broker: broker.to_owned(),
        account: name.to_owned(),
        at: now,
        roles,
        out_of_window,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mqttbrokers/{name}/access",
    summary = "Explain An MQTT Broker's Roles For A ServiceAccount",
    description = "Which roles of the broker the ServiceAccount holds now, through a RoleBinding `mqtt/<broker>/<role>` or `mqtt/*/<role>` in force or the role's deprecated `serviceAccounts` list, with the topic filters each grants and when the last grant ends: the decision the gateway makes at CONNECT and publish (ADR-N-048 §2.13). Grants of the broker outside their validity window are listed beside it.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "MqttBroker name"),
        AccessQuery,
    ),
    responses(
        (status = 200, description = "The roles the account holds; empty when it holds none", body = Access),
        (status = 400, description = "No account named, or a name that is no DNS label", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller may not read this project's MQTT brokers", body = ProblemDetails),
        (status = 404, description = "No such broker or account, or not readable by the caller", body = ProblemDetails),
        (status = 422, description = "The broker's manifest does not read", body = ProblemDetails),
    )
)]
pub async fn get_mqtt_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<AccessQuery>,
) -> Result<Json<Access>, ApiError> {
    explain(
        &state,
        &user.0.identity,
        &project,
        &name,
        query.account.unwrap_or_default(),
    )
    .map(Json)
}

/// The route's checks in the order PF-59 asks, then [`access`] at now.
fn explain(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    name: &str,
    account_name: String,
) -> Result<Access, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    // PF-59: a caller who may not read the project learns nothing of its brokers.
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!(
            "MQTT broker '{name}' not found in project '{project}'"
        )));
    }
    if !effective.may_read("MqttBroker") {
        return Err(ApiError::Denied(format!(
            "you may not read the MQTT brokers of project '{project}'; ask a project admin for a \
             role that reads MqttBroker"
        )));
    }
    let broker = state
        .mirror
        .get(project, "MqttBroker", name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "MQTT broker '{name}' not found in project '{project}'"
            ))
        })?;
    jc_core::names::validate_dns1123_label(&account_name).map_err(|_| {
        ApiError::BadRequest(format!(
            "account: name a ServiceAccount of project '{project}', e.g. `air-fleet`; got \
             '{account_name}'"
        ))
    })?;
    let unknown = || {
        ApiError::NotFound(format!(
            "ServiceAccount '{account_name}' not found in project '{project}'"
        ))
    };
    let account = state
        .mirror
        .get(project, "ServiceAccount", &account_name)
        .ok_or_else(unknown)?;
    let readable = serde_json::to_value(&account)
        .is_ok_and(|manifest| effective.may_read_manifest("ServiceAccount", &manifest));
    if !readable {
        return Err(unknown());
    }
    let spec: MqttBrokerSpec =
        serde_json::from_value(broker.spec).map_err(|err| ApiError::Unprocessable {
            detail: format!("the manifest of MQTT broker '{name}' does not read: {err}"),
            errors: vec!["spec".to_owned()],
        })?;
    // An account manifest that does not read grants nothing; its roles are then only legacy.
    let bindings = serde_json::from_value::<ServiceAccountSpec>(account.spec)
        .map(|spec| spec.roles)
        .unwrap_or_default();
    Ok(access(
        project,
        name,
        &spec,
        &account_name,
        &bindings,
        Utc::now(),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/mqttbrokers/{name}/access",
        get(get_mqtt_access),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn broker() -> MqttBrokerSpec {
        serde_json::from_value(json!({ "roles": [
            { "name": "sensor", "publish": ["air/${clientid}/pm10"] },
            { "name": "camera", "serviceAccounts": ["legacy-cam"], "publish": ["cams/${clientid}/#"] },
        ] }))
        .expect("a broker")
    }

    fn account(roles: serde_json::Value) -> Vec<RoleBinding> {
        serde_json::from_value(roles).expect("bindings")
    }

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("an instant")
    }

    #[test]
    fn a_bound_role_carries_its_filters_and_its_end() {
        let fleet = account(json!([{
            "role": "mqtt/air/sensor", "scope": { "project": "helsinki" },
            "validity": { "notAfter": "2026-12-01T00:00:00Z" }
        }]));
        let answer = access(
            "helsinki",
            "air",
            &broker(),
            "fleet",
            &fleet,
            at("2026-11-01T00:00:00Z"),
        );
        assert_eq!(
            answer.roles,
            vec![HeldRole {
                role: "sensor".into(),
                publish: vec!["air/${clientid}/pm10".into()],
                subscribe: Vec::new(),
                legacy: false,
                bound: true,
                until: Some(at("2026-12-01T00:00:00Z")),
            }]
        );
        assert!(answer.out_of_window.is_empty());
    }

    #[test]
    fn an_expired_grant_holds_nothing_and_is_listed_out_of_window() {
        let fleet = account(json!([{
            "role": "mqtt/*/sensor", "scope": { "project": "helsinki" },
            "validity": { "notAfter": "2026-10-01T00:00:00Z" }
        }]));
        let answer = access(
            "helsinki",
            "air",
            &broker(),
            "fleet",
            &fleet,
            at("2026-11-01T00:00:00Z"),
        );
        assert!(answer.roles.is_empty());
        assert_eq!(answer.out_of_window.len(), 1);
        assert_eq!(answer.out_of_window[0].grant, "mqtt/*/sensor");
    }

    #[test]
    fn a_role_listing_the_account_is_held_as_legacy() {
        let answer = access(
            "helsinki",
            "air",
            &broker(),
            "legacy-cam",
            &account(json!([])),
            Utc::now(),
        );
        assert_eq!(answer.roles.len(), 1);
        assert!(answer.roles[0].legacy && !answer.roles[0].bound);
    }
}
