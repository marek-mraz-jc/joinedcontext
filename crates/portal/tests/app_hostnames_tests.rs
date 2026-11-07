//! T-3610 (AP-172, ADR-N-037 §7): an App lists names of its Organization's own domain. A name
//! outside the verified domain, one another App holds and any name while the domain is not
//! verified are refused before a Change exists; a name of the verified domain is proposed.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{Status, API_VERSION};
use joinedcontext_portal::state::AppState;

fn state(gitea: &wiremock::MockServer, verification: &str) -> AppState {
    let state = common::state_on(gitea);
    let mut organization = envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    );
    organization.status = Some(
        serde_json::from_value::<Status>(json!({
            "domainVerification": { "state": verification, "challenge": "c", "record": "r" }
        }))
        .expect("a status"),
    );
    state.mirror.upsert(organization);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "ovzdusie",
        "ovzdusie",
        json!({ "dataModelRef": "air" }),
    ));
    // Another project's App that holds a name already.
    state.mirror.upsert(envelope(
        "App",
        "bikes",
        "doprava",
        json!({ "hostnames": ["bikes.hel.fi"] }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-app-editor",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "app-editor",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    state
}

fn app(hostnames: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": "air-map", "namespace": "ovzdusie" },
        "spec": {
            "kind": "ui",
            "source": { "path": "apps/air-map" },
            "build": { "node": "22" },
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
            }],
            "visibility": "project",
            "lifecycle": "draft",
            "hostnames": hostnames,
        },
    })
}

async fn propose(state: &AppState, hostnames: Value) -> common::Answer {
    checked_send(
        state,
        person("jana"),
        "POST",
        "/api/v1/projects/ovzdusie/apps",
        Some(app(hostnames)),
    )
    .await
}

#[tokio::test]
async fn a_name_of_the_verified_domain_is_proposed() {
    let gitea = forge().await;
    let state = state(&gitea, "verified");
    let answer = propose(&state, json!(["air.hel.fi"])).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}

#[tokio::test]
async fn a_name_outside_the_domain_or_held_by_another_app_is_refused_before_a_change() {
    let gitea = forge().await;
    let state = state(&gitea, "verified");
    for (names, says) in [
        (
            json!(["air.example.com"]),
            "not in the organization's domain hel.fi",
        ),
        (json!(["air.hel.fi", "bikes.hel.fi"]), "held by another App"),
        (json!(["https://air.hel.fi"]), "hostnames"),
    ] {
        let answer = propose(&state, names.clone()).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{names}: {}",
            answer.text
        );
        assert!(answer.text.contains(says), "{names}: {}", answer.text);
        assert!(!answer.text.contains("doprava"), "{}", answer.text);
    }
}

#[tokio::test]
async fn no_name_is_taken_while_the_domain_is_not_verified() {
    let gitea = forge().await;
    let state = state(&gitea, "pending");
    let answer = propose(&state, json!(["air.hel.fi"])).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(answer.text.contains("verified first"), "{}", answer.text);
    // An App without hostnames is proposed as before.
    let answer = propose(&state, json!([])).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}
