//! T-4000 (PF-115, API/01 §45): a space moves to another broker by one job. It holds the space
//! by an annotation, copies once the gateway holds it, compares both digests and switches
//! `brokerRef`; a mismatch leaves the space where it was and purges the target.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send, REPO};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::space_move::Moves;
use joinedcontext_portal::state::AppState;

const MANIFEST: &str = "projects/ovzdusie/spaces/air/space.yaml";
const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                     name: air\n  namespace: ovzdusie\nspec:\n  title: Air\n";
const EXPORT: &str = "{\"antaresExport\":1,\"tenant\":\"ovzdusie-air\"}\n\
                      {\"kind\":\"entity\",\"doc\":{\"id\":\"urn:ngsi-ld:A:1\"}}\n\
                      {\"end\":true,\"records\":1}\n";
const TENANT: &str = "/q/tenants/ovzdusie-air";

struct World {
    state: AppState,
    forge: MockServer,
    source: MockServer,
    target: MockServer,
    /// The target's name: the template puts it where a host goes, so it is the mock's address.
    to: String,
}

fn digest(entities: u64) -> Value {
    json!({"tenant": "ovzdusie-air", "entities": entities, "attrInstances": 3, "sha256": "ab".repeat(32)})
}

async fn world(held: bool, target_digest: Value, on_main: &str) -> World {
    // A fresh move reads the target once before it copies: an empty tenant (F-POR4-015).
    let empty_first = !on_main.contains("moving-to");
    world_with(held, target_digest, on_main, empty_first).await
}

async fn world_with(held: bool, target_digest: Value, on_main: &str, empty_first: bool) -> World {
    let forge = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{MANIFEST}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": base64::engine::general_purpose::STANDARD.encode(on_main),
        })))
        .with_priority(1)
        .mount(&forge)
        .await;

    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/metrics"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "# TYPE jc_gateway_space_held gauge\njc_gateway_space_held{{space=\"ovzdusie-air\"}} {}\n",
            u8::from(held)
        )))
        .mount(&gateway)
        .await;

    let source = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/export")))
        .respond_with(ResponseTemplate::new(200).set_body_string(EXPORT))
        .mount(&source)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/digest")))
        .respond_with(ResponseTemplate::new(200).set_body_json(digest(1)))
        .mount(&source)
        .await;
    let target = MockServer::start().await;
    if empty_first {
        Mock::given(method("GET"))
            .and(path(format!("{TENANT}/digest")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"tenant": "ovzdusie-air", "entities": 0, "attrInstances": 0, "sha256": "00".repeat(32)}),
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&target)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{TENANT}/import")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&target)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{TENANT}/digest")))
        .respond_with(ResponseTemplate::new(200).set_body_json(target_digest))
        .mount(&target)
        .await;
    Mock::given(method("DELETE"))
        .and(path(TENANT))
        .respond_with(ResponseTemplate::new(204))
        .mount(&target)
        .await;

    let token = std::env::temp_dir().join(format!(
        "space-move-{}-{}",
        std::process::id(),
        source.address().port()
    ));
    std::fs::write(&token, "admin-token").expect("write the token");
    let port = target.address().port();
    let mut config = Config::for_tests();
    config.broker_url = Some(source.uri());
    config.broker_url_template = Some(format!("http://{{broker}}:{port}"));
    config.broker_admin_token_file = Some(token);
    config.gateway_url = Some(gateway.uri());
    let client = GiteaClient::new(
        forge.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let mut state = AppState::new(config, None).with_gitea(Arc::new(client));
    state.space_moves = Arc::new(Moves::new(
        Duration::from_millis(20),
        Duration::from_millis(300),
    ));
    let image = format!("ghcr.io/x/antares@sha256:{}", "a".repeat(64));
    let to = "localhost".to_owned();
    for item in [
        envelope(
            "Role",
            "org-admin",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
        ),
        envelope(
            "RoleBinding",
            "org-admins",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "org-admin", "scope": { "organization": "bb" } }),
        ),
        envelope(
            "ContextBroker",
            "main",
            ORG_NAMESPACE,
            json!({ "default": true, "image": image }),
        ),
        envelope(
            "ContextBroker",
            &to,
            ORG_NAMESPACE,
            json!({ "image": image }),
        ),
        envelope("ContextSpace", "air", "ovzdusie", json!({ "title": "Air" })),
    ] {
        state.mirror.upsert(item);
    }
    std::mem::forget(gateway);
    World {
        state,
        forge,
        source,
        target,
        to,
    }
}

/// Starts the move and waits for it to end.
async fn moved(world: &World) -> Value {
    let started = send(
        &world.state,
        person("jana"),
        "POST",
        "/api/v1/projects/ovzdusie/spaces/air/move",
        Some(json!({"to": world.to})),
    )
    .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text);
    for _ in 0..200 {
        let now = send(
            &world.state,
            person("jana"),
            "GET",
            "/api/v1/projects/ovzdusie/spaces/air/move",
            None,
        )
        .await;
        let body: Value = serde_json::from_str(&now.text).expect("a move");
        if body["state"] == "done" || body["state"] == "failed" {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the move did not end");
}

/// The manifests the job committed, in order.
async fn commits(forge: &MockServer) -> Vec<String> {
    forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().ends_with(MANIFEST))
        .map(|r| {
            let body: Value = serde_json::from_slice(&r.body).expect("a contents write");
            let content = base64::engine::general_purpose::STANDARD
                .decode(body["content"].as_str().expect("content"))
                .expect("base64");
            String::from_utf8(content).expect("utf-8")
        })
        .collect()
}

async fn sent(server: &MockServer, verb: &str, route: &str) -> Vec<Vec<u8>> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == route)
        .map(|r| r.body)
        .collect()
}

#[tokio::test]
async fn a_move_holds_copies_compares_and_switches_the_broker() {
    let world = world(true, digest(1), SPACE).await;
    let job = moved(&world).await;
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(
        (&job["from"], &job["to"]),
        (&json!("main"), &json!(world.to))
    );
    assert_eq!(job["source"], job["target"]);
    assert_eq!(job["source"]["entities"], 1);

    let written = commits(&world.forge).await;
    assert_eq!(written.len(), 2, "{written:?}");
    assert!(
        written[0].contains("joinedcontext.com/moving-to: localhost"),
        "{}",
        written[0]
    );
    assert!(
        written[1].contains("brokerRef: localhost") && !written[1].contains("moving-to"),
        "{}",
        written[1]
    );
    assert!(
        written[1].contains("title: Air"),
        "the rest of the manifest stays: {}",
        written[1]
    );
    // The export reached the target as it left the source, with the admin token.
    let imported = sent(&world.target, "POST", &format!("{TENANT}/import")).await;
    assert_eq!(imported, [EXPORT.as_bytes().to_vec()]);
    assert!(sent(&world.target, "DELETE", TENANT).await.is_empty());
    assert!(
        !job.to_string().contains("admin-token") && !job.to_string().contains(&world.source.uri())
    );
}

#[tokio::test]
async fn a_copy_that_does_not_match_leaves_the_space_where_it_was() {
    // The space is already held, as a move the Portal forgot in a restart left it: starting
    // again resumes without a second hold commit.
    let held = SPACE.replace(
        "  namespace: ovzdusie\n",
        "  namespace: ovzdusie\n  annotations:\n    joinedcontext.com/moving-to: localhost\n",
    );
    let world = world(true, digest(0), &held).await;
    let job = moved(&world).await;
    assert_eq!(job["state"], "failed", "{job}");
    let reason = job["reason"].as_str().expect("a reason");
    assert!(
        reason.contains("1 entities") && reason.contains("the target 0"),
        "{reason}"
    );
    assert_ne!(job["source"], job["target"]);
    // The target held nothing before: its copy is purged, and the space is released on main.
    assert_eq!(sent(&world.target, "DELETE", TENANT).await.len(), 1);
    let written = commits(&world.forge).await;
    assert_eq!(
        written.len(),
        1,
        "only the release is committed: {written:?}"
    );
    assert!(
        !written[0].contains("brokerRef") && !written[0].contains("annotations"),
        "{}",
        written[0]
    );
}

#[tokio::test]
async fn nothing_is_copied_before_the_gateway_holds_the_space() {
    let world = world(false, digest(1), SPACE).await;
    let job = moved(&world).await;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(
        job["reason"]
            .as_str()
            .is_some_and(|r| r.contains("not every gateway pod took the hold")),
        "{job}"
    );
    assert!(sent(&world.source, "GET", &format!("{TENANT}/export"))
        .await
        .is_empty());
    assert!(
        sent(&world.target, "DELETE", TENANT).await.is_empty(),
        "nothing was imported to purge"
    );
}

#[tokio::test]
async fn only_an_administrator_moves_and_only_to_another_declared_broker() {
    let world = world(true, digest(1), SPACE).await;
    let route = "/api/v1/projects/ovzdusie/spaces/air/move";
    let refused = send(
        &world.state,
        person("peter"),
        "POST",
        route,
        Some(json!({"to": world.to})),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    for to in ["nowhere", "main"] {
        let bad = send(
            &world.state,
            person("jana"),
            "POST",
            route,
            Some(json!({"to": to})),
        )
        .await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{to}: {}", bad.text);
    }
    let none = send(&world.state, person("jana"), "GET", route, None).await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
    assert!(commits(&world.forge).await.is_empty());
}

/// F-POR4-015: a target tenant that already holds data is refused before anything is copied, and
/// it is never purged: the space stays where it was and the hold is released.
#[tokio::test]
async fn a_target_that_already_holds_data_is_refused_and_kept() {
    let world = world_with(true, digest(5), SPACE, false).await;
    let job = moved(&world).await;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(
        job["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("already holds 5 entities"),
        "{job}"
    );
    assert!(
        sent(&world.target, "POST", &format!("{TENANT}/import"))
            .await
            .is_empty(),
        "copied into it"
    );
    assert!(
        sent(&world.target, "DELETE", TENANT).await.is_empty(),
        "the target's data was purged"
    );
}
