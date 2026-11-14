//! `kind: Portal` through the Portal's resource routes (ODP-08, ODP-09, ODP-12, T-6873): an
//! Organization's administrator writes only that Organization's portals, never reads another
//! Organization's, never deletes a `default`, and takes only a host the operator listed.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::doors::http;
use common::envelope;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

fn admin(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@example.org")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

/// The installation and two Organizations, `bbsk` and `praha`, each with one administrator, a
/// `default` portal and a further one; `bbsk` may take `doprava.bbsk.sk`, the cap is 6.
fn world() -> AppState {
    let mut config = Config::for_tests();
    config.portal_settings =
        joinedcontext_portal::portals::parse("bbsk=doprava.bbsk.sk", Some("6"), "").expect("settings");
    let state = AppState::new(config, None).with_mirror(Arc::new(Mirror::new()));
    let m = &state.mirror;
    for (name, spec) in [
        ("ops", json!({ "domain": "ops.example", "installation": true })),
        ("bbsk", json!({ "domain": "bbsk.sk" })),
        ("praha", json!({ "domain": "praha.eu" })),
    ] {
        m.upsert(envelope("Organization", name, ORG_NAMESPACE, spec));
    }
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    m.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    for (who, org) in [("bbadmin", "bbsk"), ("prahaadmin", "praha")] {
        m.upsert(envelope(
            "RoleBinding",
            &format!("{org}-admins"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@example.org") }], "role": "org-admin",
                "scope": { "organization": org } }),
        ));
        for path in ["default", "skoly"] {
            m.upsert(portal(org, path, None));
        }
    }
    state
}

fn portal(org: &str, path: &str, host: Option<&str>) -> joinedcontext_portal::resource::ResourceEnvelope {
    let mut spec = json!({ "organizationRef": org, "path": path, "title": { "sk": "Doprava" } });
    if let Some(host) = host {
        spec["host"] = json!(host);
    }
    envelope("Portal", &format!("{org}-{path}"), ORG_NAMESPACE, spec)
}

fn manifest(org: &str, path: &str, host: Option<&str>) -> Value {
    serde_json::to_value(portal(org, path, host)).expect("json")
}

fn route(name: &str) -> String {
    format!("/api/v1/projects/{ORG_NAMESPACE}/portals/{name}")
}

async fn put(state: &AppState, who: &str, manifest: Value) -> common::doors::Answer {
    let name = manifest["metadata"]["name"].as_str().expect("name").to_owned();
    http(state, admin(who), "PUT", &format!("{}?dryRun=All", route(&name)), Some(manifest)).await
}

/// What refused `answer`: a 422's body, or a dry run's red verdict; `None` when it passed.
fn refusal(answer: &common::doors::Answer) -> Option<String> {
    match answer.status {
        StatusCode::UNPROCESSABLE_ENTITY => Some(answer.body.to_string()),
        s if s.is_success() && answer.body["valid"] == json!(false) => {
            Some(answer.body["verdict"]["findings"].to_string())
        }
        _ => None,
    }
}

#[tokio::test]
async fn nobody_signed_in_reads_or_writes_a_portal() {
    let state = world();
    for (method, uri) in [
        ("GET", format!("/api/v1/projects/{ORG_NAMESPACE}/portals")),
        ("GET", route("bbsk-skoly")),
        ("DELETE", route("bbsk-skoly")),
    ] {
        let response = joinedcontext_portal::server::app(state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        // A write without a session meets the CSRF door first: 403, never a write.
        let refused = match method {
            "GET" => response.status() == StatusCode::UNAUTHORIZED,
            _ => matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        };
        assert!(refused, "{method} {uri}: {}", response.status());
    }
}

#[tokio::test]
async fn an_organization_admin_proposes_only_its_own_portals() {
    let state = world();
    let own = put(&state, "bbadmin", manifest("bbsk", "mobility", None)).await;
    assert!(own.status.is_success() && refusal(&own).is_none(), "{} {}", own.status, own.body);
    let foreign = put(&state, "bbadmin", manifest("praha", "mobility", None)).await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN, "{}", foreign.body);
    // Taking another Organization's portal over by naming its manifest.
    let mut over = manifest("bbsk", "skoly2", None);
    over["metadata"]["name"] = json!("praha-skoly");
    let over = put(&state, "bbadmin", over).await;
    assert!(
        matches!(over.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND)
            || refusal(&over).is_some_and(|r| r.contains("spec.organizationRef")),
        "{} {}",
        over.status,
        over.body
    );
}

#[tokio::test]
async fn another_organizations_portal_is_not_there() {
    let state = world();
    let theirs = http(&state, admin("bbadmin"), "GET", &route("praha-skoly"), None).await;
    assert_eq!(theirs.status, StatusCode::NOT_FOUND, "{}", theirs.body);
    let own = http(&state, admin("bbadmin"), "GET", &route("bbsk-skoly"), None).await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.body);
    let list = http(
        &state,
        admin("bbadmin"),
        "GET",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/portals"),
        None,
    )
    .await;
    let listed = list.body.to_string();
    assert!(listed.contains("bbsk-skoly") && !listed.contains("praha-skoly"), "{listed}");
    let gone = http(&state, admin("bbadmin"), "DELETE", &route("praha-skoly"), None).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.body);
}

#[tokio::test]
async fn deleting_the_default_portal_is_refused_and_a_further_one_is_not() {
    let state = world();
    let refused = http(&state, admin("bbadmin"), "DELETE", &format!("{}?dryRun=All", route("bbsk-default")), None).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", refused.body);
    assert!(refused.body.to_string().contains("the default portal cannot be deleted"), "{}", refused.body);
    let further = http(&state, admin("bbadmin"), "DELETE", &format!("{}?dryRun=All", route("bbsk-skoly")), None).await;
    assert!(further.status.is_success(), "{} {}", further.status, further.body);
}

#[tokio::test]
async fn a_host_not_listed_and_the_cap_are_refused_naming_the_field() {
    let state = world();
    let listed = put(&state, "bbadmin", manifest("bbsk", "doprava", Some("doprava.bbsk.sk"))).await;
    assert!(refusal(&listed).is_none(), "{} {}", listed.status, listed.body);
    let taken = put(&state, "bbadmin", manifest("bbsk", "doprava", Some("www.praha.eu"))).await;
    let taken = refusal(&taken).unwrap_or_else(|| panic!("{}", taken.body));
    assert!(taken.contains("spec.host") && taken.contains("www.praha.eu is not listed"), "{taken}");
    // Three Organizations and three further portals: six, the cap.
    state.mirror.upsert(portal("bbsk", "doprava", Some("doprava.bbsk.sk")));
    let past = put(&state, "bbadmin", manifest("bbsk", "sport", None)).await;
    let past = refusal(&past).unwrap_or_else(|| panic!("{}", past.body));
    assert!(past.contains("portals.max (6) reached"), "{past}");
}
