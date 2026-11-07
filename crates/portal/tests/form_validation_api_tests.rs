//! The Portal's half of every form's validation (T-2731; UI-16, PF-44, AP-14): what each kind's
//! check answers for what a person types wrong, one table over the kinds the create forms write
//! (`ui/tests/form_validation.<kind>.test.tsx` holds the form's half).
//!
//! - a name that breaks the rule (empty, not a DNS label, over 63 characters) is refused, and the
//!   same manifest under a good name is green, so the refusal is the name's;
//! - a secret typed where a `secretRef` belongs is refused at the check and at the draft, and the
//!   answer never repeats it (MF-24);
//! - a reference to something that does not exist is a red finding on that reference's path,
//!   which is where the form puts its sentence.
//!
//! A name another project holds is answered by `spaces_api_tests.rs` (PF-44) and
//! `app_name_clash_tests.rs` (AP-14a).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send, Answer};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";

/// One kind as its create form writes it: the collection, the namespace, a spec that is green in
/// the state below, and the reference the form picks, when it has one.
struct Kind {
    kind: &'static str,
    plural: &'static str,
    namespace: &'static str,
    spec: Value,
    reference: Option<(&'static str, &'static str)>,
}

fn kinds() -> Vec<Kind> {
    let project = |kind, plural, spec| Kind {
        kind,
        plural,
        namespace: PROJECT,
        spec,
        reference: None,
    };
    let org = |kind, plural, spec| Kind {
        kind,
        plural,
        namespace: ORG_NAMESPACE,
        spec,
        reference: None,
    };
    vec![
        project("ContextSpace", "spaces", json!({ "isSandbox": false })),
        Kind {
            reference: Some(("spec.contextSpaceRef", "/contextSpaceRef")),
            ..project(
                "Endpoint",
                "endpoints",
                json!({
                    "contextSpaceRef": "air",
                    "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
                    "audience": "public",
                    "enabledRepresentations": ["ngsi-ld"]
                }),
            )
        },
        project(
            "Pipeline",
            "pipelines",
            json!({
                "class": "resident",
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:ovzdusie:air-public"
            }),
        ),
        // Its `spec.contextSpaceRef` is not resolved by the check (`references::fields` has no
        // Policy), so it carries no reference case here (/workspace/chyby.md, 2026-09-25).
        project(
            "Policy",
            "policies",
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                "assigner": "did:web:hel.fi",
                "assignee": { "kind": "role", "id": "public" },
                "operations": ["queryEntity"],
                "information": [{ "entities": [{ "type": "AirQualityObserved" }] }]
            }),
        ),
        Kind {
            reference: Some(("spec.contextSpaceRef", "/contextSpaceRef/name")),
            ..project(
                "Subscription",
                "subscriptions",
                json!({
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                    "entities": [{ "type": "AirQualityObserved" }],
                    "notification": { "endpoint": { "uri": "https://alerts.example.fi/hooks/air" } }
                }),
            )
        },
        project(
            "ContextSourceRegistration",
            "csrs",
            json!({
                "contextSpaceRef": "air",
                "endpoint": "https://broker.example.fi/ngsi-ld/v1",
                "information": [{ "entities": [{ "type": "AirQualityObserved" }] }],
                "federation": { "identity": "caller" }
            }),
        ),
        project(
            "DataSource",
            "datasources",
            json!({
                "type": "mqtt",
                "mqtt": {
                    "urls": ["tls://mqtt.example.fi:8883"],
                    "topics": ["sensors/aq/+/reading"],
                    "passwordRef": { "name": "mqtt-air", "key": "password" }
                }
            }),
        ),
        project(
            "SyncSource",
            "syncsources",
            json!({
                "source": { "git": { "url": "https://git.example.fi/city/regional.git", "ref": "main" } },
                "schedule": { "interval": "6h" },
                "mode": "mirror",
                "conflictPolicy": "fail"
            }),
        ),
        project(
            "ServiceAccount",
            "serviceaccounts",
            json!({
                "owner": { "user": "steward" },
                "purpose": "syncs the air stations",
                "roles": [{ "role": "viewer", "scope": { "project": PROJECT } }],
                "credentials": [{ "kind": "oauth-client", "name": "main" }]
            }),
        ),
        org(
            "Role",
            "roles",
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
        ),
        org(
            "Group",
            "groups",
            json!({ "description": "The people who look after the air", "members": [{ "user": "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14" }] }),
        ),
        project(
            "Dashboard",
            "dashboards",
            json!({
                "title": { "en": "Air" },
                "visibility": "project",
                "pages": [{ "layout": "full-map", "layers": ["stations"] }]
            }),
        ),
        // Its `spec.sourceEndpointRef` is resolved with the dashboard it is drafted beside, not by
        // the check (`references::fields`), so it carries no reference case here.
        project(
            "Layer",
            "layers",
            json!({
                "sourceEndpointRef": "air-public",
                "entityType": "AirQualityObserved",
                "style": "circle"
            }),
        ),
    ]
}

/// A steward: `Config::for_tests` makes `portal-approver` the bootstrap group, which proposes
/// every kind everywhere (verdict_findings_tests.rs).
fn steward() -> Identity {
    Identity {
        roles: vec!["portal-approver".into()],
        ..person("steward")
    }
}

/// The project as the forms find it: the space, endpoint, layer and role the specs name.
async fn state() -> AppState {
    let state = common::state_on_forge().await;
    for (kind, name, namespace, spec) in [
        (
            "ContextSpace",
            "air",
            PROJECT,
            json!({ "isSandbox": false }),
        ),
        (
            "Endpoint",
            "air-public",
            PROJECT,
            json!({ "contextSpaceRef": "air", "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "audience": "public", "enabledRepresentations": ["ngsi-ld", "ogc-features"] }),
        ),
        (
            "Layer",
            "stations",
            PROJECT,
            json!({ "sourceEndpointRef": "air-public", "entityType": "AirQualityObserved", "style": "circle" }),
        ),
        (
            "Role",
            "viewer",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
        ),
    ] {
        state.mirror.upsert(envelope(kind, name, namespace, spec));
    }
    state
}

fn manifest(kind: &Kind, name: &str, spec: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": kind.kind,
        "metadata": { "name": name, "namespace": kind.namespace },
        "spec": spec,
    })
}

/// The form's Check: `?dryRun=All` on the kind's collection.
async fn check(state: &AppState, kind: &Kind, body: Value) -> Answer {
    let uri = format!(
        "/api/v1/projects/{}/{}?dryRun=All",
        kind.namespace, kind.plural
    );
    send(state, steward(), "POST", &uri, Some(body)).await
}

fn json_of(answer: &Answer) -> Value {
    serde_json::from_str(&answer.text).unwrap_or(Value::Null)
}

/// Green: the check passed the manifest as it stands.
fn green(answer: &Answer) -> bool {
    answer.status == StatusCode::OK && json_of(answer)["verdict"]["ok"] == true
}

#[tokio::test]
async fn every_kind_is_green_under_a_good_name() {
    // The control of the table: each refusal below is the name's, the secret's or the
    // reference's, and not a spec this test got wrong.
    let state = state().await;
    let mut problems = Vec::new();
    for kind in kinds() {
        let answer = check(
            &state,
            &kind,
            manifest(&kind, "air-quality", kind.spec.clone()),
        )
        .await;
        if !green(&answer) {
            problems.push(format!("{}: {} {}", kind.kind, answer.status, answer.text));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[tokio::test]
async fn every_kind_refuses_a_name_that_breaks_the_rule_and_says_it_is_the_name() {
    let state = state().await;
    let too_long = "a".repeat(64);
    let mut problems = Vec::new();
    for kind in kinds() {
        for name in ["", "Air Quality!", too_long.as_str()] {
            let answer = check(&state, &kind, manifest(&kind, name, kind.spec.clone())).await;
            if green(&answer) || !answer.text.contains("name") {
                problems.push(format!(
                    "{} {name:?}: {} {}",
                    kind.kind, answer.status, answer.text
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[tokio::test]
async fn every_kind_refuses_a_secret_typed_in_and_never_repeats_it() {
    // Built from parts: a fixture, not a credential.
    let secret = ["sk", "live", "4f9a7c2e1b8d6a3f", "5e0c9b7a"].join("-");
    let state = state().await;
    let mut problems = Vec::new();
    for kind in kinds() {
        let mut spec = kind.spec.clone();
        spec["password"] = json!(secret);
        let typed = manifest(&kind, "air-quality", spec);

        let checked = check(&state, &kind, typed.clone()).await;
        // Refused at the check as a red verdict whose sentence says what to write instead: the
        // form shows it under the verdict (MF-24).
        if green(&checked) || !checked.text.contains("secretRef") {
            problems.push(format!(
                "{} check: {} {}",
                kind.kind, checked.status, checked.text
            ));
        }
        if checked.text.contains(&secret) {
            problems.push(format!("{} check repeated the secret", kind.kind));
        }

        // The form's draft is the other door the value reaches while a person types.
        let draft = format!(
            "/api/v1/projects/{}/drafts/{}/air-quality",
            kind.namespace, kind.kind
        );
        let saved = send(
            &state,
            steward(),
            "PUT",
            &draft,
            Some(json!({ "manifest": typed })),
        )
        .await;
        // The draft goes through the operations door, which refuses a credential 422 (T-5971).
        if saved.status != StatusCode::UNPROCESSABLE_ENTITY {
            problems.push(format!(
                "{} draft: {} {}",
                kind.kind, saved.status, saved.text
            ));
        }
        if saved.text.contains(&secret) {
            problems.push(format!("{} draft repeated the secret", kind.kind));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

#[tokio::test]
async fn a_reference_to_nothing_is_a_red_finding_on_that_reference() {
    let state = state().await;
    let with_reference: Vec<Kind> = kinds()
        .into_iter()
        .filter(|k| k.reference.is_some())
        .collect();
    assert_eq!(
        with_reference.len(),
        2,
        "Endpoint and Subscription pick a reference the check resolves"
    );
    let mut problems = Vec::new();
    for kind in with_reference {
        let (path, pointer) = kind.reference.expect("filtered");
        let mut spec = kind.spec.clone();
        *spec
            .pointer_mut(pointer)
            .expect("the reference is in the spec") = json!("gone");
        let answer = check(&state, &kind, manifest(&kind, "air-quality", spec)).await;
        let findings = json_of(&answer)["verdict"]["findings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let said = findings.iter().any(|finding| {
            finding["path"] == path
                && finding["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("gone")
        });
        if green(&answer) || !said {
            problems.push(format!(
                "{} on {path}: {} {}",
                kind.kind, answer.status, answer.text
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
