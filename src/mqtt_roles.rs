//! The roles of an MQTT broker a ServiceAccount holds, and who may grant them (ADR-N-048 §2.13,
//! PL-75, T-5876).
//!
//! The decision is `jc_core::mqtt::roles_held`, the one the gateway makes at CONNECT and at every
//! publish; this module only reads the manifests it takes from the mirror. A grant is a
//! RoleBinding `mqtt/<broker>/<role>` or `mqtt/*/<role>` on the account; a proposal adding one is
//! re-checked here against the proposer's rights and the roles the brokers declare, whatever form
//! sent it.

use chrono::{DateTime, Utc};
use jc_core::kinds::{MqttBrokerSpec, RoleBinding, ServiceAccountSpec};
use jc_core::mqtt::{roles_held, Account, Binding, Broker, Grant, Held};
use serde_json::Value;

use crate::error::ApiError;
use crate::store::{ListOptions, Mirror};

/// The account's RoleBindings as `roles_held` reads them.
pub fn bindings(roles: &[RoleBinding]) -> impl Iterator<Item = Binding<'_>> {
    roles.iter().map(|binding| Binding {
        role: &binding.role,
        project: binding.scope.project.as_deref(),
        validity: binding.validity.as_ref(),
    })
}

/// The roles of broker `broker` (project `project`) that account `name` holds at `now`.
pub fn held<'a>(
    project: &'a str,
    broker: &'a str,
    spec: &'a MqttBrokerSpec,
    name: &str,
    roles: &[RoleBinding],
    now: DateTime<Utc>,
) -> Vec<Held<'a>> {
    roles_held(
        Broker {
            name: broker,
            project,
            roles: &spec.roles,
        },
        Account { name, project },
        bindings(roles),
        now,
    )
}

/// Whether the account is named by one of the broker's roles at all, in or out of a binding's
/// window: its Keycloak client then carries the broker's audience, and the gateway decides the
/// window at each CONNECT and publish (ADR-N-048 §2.7).
pub fn names_account(
    broker: &str,
    spec: &MqttBrokerSpec,
    name: &str,
    account: Option<&ServiceAccountSpec>,
) -> bool {
    spec.roles_of(name).next().is_some()
        || account.is_some_and(|account| {
            account.roles.iter().any(|binding| {
                Grant::parse(&binding.role)
                    .and_then(Result::ok)
                    .is_some_and(|grant| {
                        spec.roles
                            .iter()
                            .any(|role| grant.names(broker, &role.name))
                    })
            })
        })
}

/// The brokers of `project` the mirror and the Change being checked hold, each read, the ones
/// `readable` refuses left out: a broker the proposer may not read is not there for them.
fn brokers(
    mirror: &Mirror,
    project: &str,
    readable: &dyn Fn(&Value) -> bool,
) -> Vec<(String, MqttBrokerSpec)> {
    let mirrored = mirror
        .list(project, "MqttBroker", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|envelope| serde_json::to_value(envelope).ok());
    let bundled = crate::references::bundled()
        .into_iter()
        .filter(|manifest| manifest.get("kind").and_then(Value::as_str) == Some("MqttBroker"));
    mirrored
        .chain(bundled)
        .filter(|manifest| readable(manifest))
        .filter_map(|manifest| {
            let name = manifest.pointer("/metadata/name")?.as_str()?.to_owned();
            let spec = serde_json::from_value(manifest.get("spec")?.clone()).ok()?;
            Some((name, spec))
        })
        .collect()
}

/// Refuses a ServiceAccount proposal granting a broker role the proposer may not grant (step
/// 4b-mqtt of the verdict): 403 when they may not read the project's brokers, the reference
/// refusal for a broker that is not there for them, 422 for a role no such broker declares. Each
/// names `spec.roles[i].role`. A grant the account already holds is not judged again, so a
/// person may edit the rest of an account whose grants someone else made.
pub fn check_grants(
    state: &crate::state::AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    let Ok(account) = serde_json::from_value::<ServiceAccountSpec>(spec.clone()) else {
        return Ok(());
    };
    let previous: Vec<String> = state
        .mirror
        .get(project, "ServiceAccount", name)
        .and_then(|envelope| serde_json::from_value::<ServiceAccountSpec>(envelope.spec).ok())
        .map(|held| held.roles.into_iter().map(|binding| binding.role).collect())
        .unwrap_or_default();
    let effective = crate::permissions::for_request(state, identity, project);
    let readable = |manifest: &Value| effective.may_read_manifest("MqttBroker", manifest);
    let mut declared = None;
    for (i, binding) in account.roles.iter().enumerate() {
        let Some(Ok(grant)) = Grant::parse(&binding.role) else {
            continue;
        };
        if previous.contains(&binding.role) {
            continue;
        }
        let field = format!("spec.roles[{i}].role");
        if !effective.may_read("MqttBroker") {
            return Err(ApiError::Denied(format!(
                "{field}: you may not read the MQTT brokers of project '{project}', so you cannot \
                 grant their role `{}`; ask a project admin to grant it",
                binding.role
            )));
        }
        let brokers = declared.get_or_insert_with(|| brokers(&state.mirror, project, &readable));
        refusal(project, &field, grant, brokers)?;
    }
    Ok(())
}

/// The refusal of one grant against the brokers there are for the proposer.
fn refusal(
    project: &str,
    field: &str,
    grant: Grant<'_>,
    brokers: &[(String, MqttBrokerSpec)],
) -> Result<(), ApiError> {
    if let Some(broker) = grant.broker {
        if !brokers.iter().any(|(name, _)| name == broker) {
            return Err(crate::references::refusal(
                project,
                &crate::references::Missing {
                    path: field.to_owned(),
                    kind: "MqttBroker".to_owned(),
                    name: broker.to_owned(),
                },
            ));
        }
    }
    let declares = brokers
        .iter()
        .any(|(name, spec)| spec.roles.iter().any(|role| grant.names(name, &role.name)));
    if declares {
        return Ok(());
    }
    let which = grant.broker.map_or_else(
        || "no MQTT broker".to_owned(),
        |b| format!("MQTT broker '{b}'"),
    );
    Err(ApiError::Unprocessable {
        detail: format!(
            "{field}: {which} of project '{project}' declares the role `{}`; pick a role the \
             broker lists",
            grant.role
        ),
        errors: vec![field.to_owned()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn broker(roles: Value) -> MqttBrokerSpec {
        serde_json::from_value(json!({ "roles": roles })).expect("a broker spec")
    }

    fn sensors() -> Vec<(String, MqttBrokerSpec)> {
        vec![(
            "air".to_owned(),
            broker(json!([{ "name": "sensor", "publish": ["air/${clientid}/pm10"] }])),
        )]
    }

    fn judged(role: &str) -> Result<(), ApiError> {
        let grant = Grant::parse(role).expect("a grant").expect("well formed");
        refusal("helsinki", "spec.roles[0].role", grant, &sensors())
    }

    #[test]
    fn a_declared_role_of_a_broker_or_of_any_broker_is_grantable() {
        assert!(judged("mqtt/air/sensor").is_ok());
        assert!(judged("mqtt/*/sensor").is_ok());
    }

    #[test]
    fn a_role_no_broker_declares_is_422_naming_the_field() {
        for role in ["mqtt/air/camera", "mqtt/*/camera"] {
            match judged(role) {
                Err(ApiError::Unprocessable { errors, .. }) => {
                    assert_eq!(errors, vec!["spec.roles[0].role".to_owned()]);
                }
                other => panic!("{role}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_broker_that_is_not_there_for_the_proposer_is_the_reference_refusal() {
        match judged("mqtt/water/sensor") {
            Err(ApiError::Invalid { errors, detail }) => {
                assert_eq!(errors, vec!["spec.roles[0].role".to_owned()]);
                assert!(detail.contains("MqttBroker 'water'"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    fn person(name: &str) -> crate::auth::session::Identity {
        crate::auth::session::Identity {
            client: None,
            subject: format!("sub-{name}"),
            username: name.into(),
            email: Some(format!("{name}@hel.fi")),
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        }
    }

    fn manifest(
        kind: &str,
        name: &str,
        namespace: &str,
        spec: Value,
    ) -> crate::resource::ResourceEnvelope {
        crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: crate::resource::ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    #[test]
    fn a_proposal_is_judged_by_its_proposers_rights_and_a_held_grant_is_not_judged_again() {
        use crate::permissions::ORG_NAMESPACE;
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let upsert = |m| state.mirror.upsert(m);
        upsert(manifest(
            "Role",
            "accounts",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ServiceAccount"], "verbs": ["read", "propose"] }] }),
        ));
        upsert(manifest(
            "Role",
            "both",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ServiceAccount", "MqttBroker"], "verbs": ["read", "propose"] }] }),
        ));
        for (who, role) in [("aino", "accounts"), ("stef", "both")] {
            upsert(manifest(
                "RoleBinding",
                who,
                ORG_NAMESPACE,
                json!({
                    "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": { "project": "helsinki" }
                }),
            ));
        }
        upsert(manifest(
            "MqttBroker",
            "air",
            "helsinki",
            json!({ "roles": [{ "name": "sensor", "publish": ["air/${clientid}/pm10"] }] }),
        ));
        let account = |role: &str| {
            json!({
                "owner": { "user": "stef" }, "purpose": "air", "credentials": [{ "kind": "oauth-client", "name": "main" }],
                "roles": [{ "role": role, "scope": { "project": "helsinki" } }]
            })
        };
        let judge = |who: &str, role: &str| {
            check_grants(&state, &person(who), "helsinki", "fleet", &account(role))
        };
        assert!(
            matches!(judge("aino", "mqtt/air/sensor"), Err(ApiError::Denied(d)) if d.starts_with("spec.roles[0].role"))
        );
        assert!(judge("stef", "mqtt/air/sensor").is_ok());
        assert!(matches!(
            judge("stef", "mqtt/*/vent"),
            Err(ApiError::Unprocessable { .. })
        ));
        assert!(
            judge("aino", "project-reader").is_ok(),
            "no MQTT grant, nothing to judge"
        );
        upsert(manifest(
            "ServiceAccount",
            "fleet",
            "helsinki",
            account("mqtt/air/sensor"),
        ));
        assert!(
            judge("aino", "mqtt/air/sensor").is_ok(),
            "a grant the account already holds"
        );
    }

    #[test]
    fn a_binding_names_the_account_for_the_audience_in_or_out_of_its_window() {
        let spec = broker(json!([{ "name": "sensor", "publish": ["air/${clientid}/pm10"] }]));
        let account: ServiceAccountSpec = serde_json::from_value(json!({
            "owner": { "user": "anna" },
            "purpose": "air sensors",
            "credentials": [],
            "roles": [{
                "role": "mqtt/*/sensor",
                "scope": { "project": "helsinki" },
                "validity": { "notAfter": "2020-01-01T00:00:00Z" }
            }]
        }))
        .expect("an account");
        assert!(names_account("air", &spec, "fleet", Some(&account)));
        assert!(!names_account("air", &spec, "fleet", None));
        assert!(held(
            "helsinki",
            "air",
            &spec,
            "fleet",
            &account.roles,
            Utc::now()
        )
        .is_empty());
    }
}
