//! Each WASM shard's placement and store key (T-3426, AP-157, AP-158, ADR-N-044).
//!
//! Against a mocked API server and a mocked store admin API: what one converge writes for every
//! shard, that a retired App leaves its shard's placement on the next one, that a shard's key is
//! scoped to its own prefix and the components, and that no Secret value reaches a problem.

use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::artifact_store::{self, Client, Credential, Settings};
use joinedcontext_portal::reconciler::wasm_shards::{PlacedApp, WasmShards};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ROOT_SECRET: &str = "the-root-secret-nobody-else-holds";
const NAMESPACE: &str = "apps-host";

fn store_settings(endpoint: String) -> Settings {
    Settings {
        endpoint,
        bucket: "jc-artifacts".to_owned(),
        region: "us-east-1".to_owned(),
        root_access_key: "jc-root".to_owned(),
        root_secret_key: ROOT_SECRET.to_owned(),
    }
}

fn app(project: &str, name: &str, shard: u32) -> PlacedApp {
    PlacedApp {
        project: project.to_owned(),
        name: name.to_owned(),
        shard,
        digest: format!("sha256:{}", "b".repeat(64)),
        // The App's own Endpoint, the one gateway path its component may call (AP-147, AP-157).
        endpoint: Some(format!("ep{}", name.replace('-', ""))),
        jobs: Vec::new(),
    }
}

async fn answering(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(status).set_body_string("{}"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

fn patches(received: &[Request]) -> Vec<(String, Value)> {
    received
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| {
            (
                request.url.path().to_owned(),
                serde_json::from_slice(&request.body).expect("a JSON body"),
            )
        })
        .collect()
}

fn placement_of(body: &Value) -> Value {
    serde_json::from_str(body["data"]["placements.json"].as_str().expect("the key")).expect("JSON")
}

#[tokio::test]
async fn every_shard_gets_its_placement_and_its_key_and_a_retired_app_leaves_on_the_next_converge()
{
    let kube_server = answering(200).await;
    let store_server = answering(200).await;
    let shards = WasmShards::new(
        KubeClient::with_token(&kube_server.uri(), "token").expect("a client"),
        NAMESPACE.to_owned(),
        2,
        "apps".to_owned(),
    );
    let store = Client::new(store_settings(store_server.uri())).expect("a store client");

    let problems = shards
        .converge(
            &[app("helsinki", "notes", 0), app("praha", "odpad", 1)],
            Some(&store),
        )
        .await;
    assert!(problems.is_empty(), "{problems:?}");

    let written = patches(&kube_server.received_requests().await.expect("recorded"));
    let maps: Vec<&(String, Value)> = written
        .iter()
        .filter(|(_, body)| body["kind"] == "ConfigMap")
        .collect();
    assert_eq!(
        maps.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "/api/v1/namespaces/apps-host/configmaps/jc-wasm-host-0-placements",
            "/api/v1/namespaces/apps-host/configmaps/jc-wasm-host-1-placements",
        ]
    );
    let zero = placement_of(&maps[0].1);
    assert_eq!(zero["shard"], "0");
    assert_eq!(zero["apps"][0]["name"], "notes");
    assert_eq!(zero["apps"][0]["endpoint"], "epnotes");
    assert_eq!(zero["apps"][0]["tenant"], "helsinki");
    assert_eq!(
        zero["apps"].as_array().map(Vec::len),
        Some(1),
        "shard 0 names no App of shard 1"
    );

    let secrets: Vec<&(String, Value)> = written
        .iter()
        .filter(|(_, body)| body["kind"] == "Secret")
        .collect();
    assert_eq!(secrets.len(), 2);
    assert_eq!(
        secrets[1].0,
        "/api/v1/namespaces/apps-host/secrets/apps-host-1-store"
    );
    let key = Credential::derive_shard(ROOT_SECRET, 1);
    assert_eq!(secrets[1].1["stringData"]["access-key"], key.access_key);
    assert_eq!(secrets[1].1["stringData"]["secret-key"], key.secret_key);

    // The store was told shard 1's policy, which names shard 1's prefix and the components only.
    let policies: Vec<Value> = store_server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|request| request.url.path() == "/rustfs/admin/v3/add-canned-policy")
        .map(|request| serde_json::from_slice(&request.body).expect("a policy"))
        .collect();
    assert_eq!(
        policies[1],
        artifact_store::shard_policy_document("apps", 1)
    );

    // notes is retired: the next converge writes shard 0 without it.
    kube_server.reset().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&kube_server)
        .await;
    assert!(shards
        .converge(&[app("praha", "odpad", 1)], Some(&store))
        .await
        .is_empty());
    let again = patches(&kube_server.received_requests().await.expect("recorded"));
    let zero = again
        .iter()
        .find(|(path, _)| path.ends_with("jc-wasm-host-0-placements"))
        .expect("shard 0 again");
    assert_eq!(placement_of(&zero.1)["apps"], serde_json::json!([]));
}

#[tokio::test]
async fn without_a_store_only_placements_are_written_and_a_refusal_says_which_shard_without_a_secret(
) {
    let kube_server = answering(403).await;
    let shards = WasmShards::new(
        KubeClient::with_token(&kube_server.uri(), "token").expect("a client"),
        NAMESPACE.to_owned(),
        1,
        "apps".to_owned(),
    );
    let problems = shards.converge(&[app("helsinki", "notes", 0)], None).await;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("shard 0: its placement was not written"),
        "{problems:?}"
    );
    let written = patches(&kube_server.received_requests().await.expect("recorded"));
    assert!(
        written.iter().all(|(_, body)| body["kind"] == "ConfigMap"),
        "no Secret without a store"
    );

    // A store that refuses the key: the problem names the shard, never the derived secret.
    let kube_ok = answering(200).await;
    let store_server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&store_server)
        .await;
    let shards = WasmShards::new(
        KubeClient::with_token(&kube_ok.uri(), "token").expect("a client"),
        NAMESPACE.to_owned(),
        1,
        "apps".to_owned(),
    );
    let store = Client::new(store_settings(store_server.uri())).expect("a store client");
    let problems = shards.converge(&[], Some(&store)).await;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("shard 0: the store refused its key"),
        "{problems:?}"
    );
    assert!(!problems[0].contains(&Credential::derive_shard(ROOT_SECRET, 0).secret_key));
}

#[tokio::test]
async fn the_portal_applies_no_configmap_but_a_shards_placement() {
    let kube_server = answering(200).await;
    let kube = KubeClient::with_token(&kube_server.uri(), "token").expect("a client");
    let other = serde_json::json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": "apisix-standalone-base", "namespace": NAMESPACE },
        "data": {},
    });
    assert!(kube.apply_placement(&other).await.is_err());
    let not_a_map = serde_json::json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": { "name": "jc-wasm-host-0-placements", "namespace": NAMESPACE },
    });
    assert!(kube.apply_placement(&not_a_map).await.is_err());
    for name in [
        "jc-wasm-host--placements",
        "jc-wasm-host-a-placements",
        "jc-wasm-host-0-placements-x",
    ] {
        let map = serde_json::json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": { "name": name, "namespace": NAMESPACE }, "data": {},
        });
        assert!(kube.apply_placement(&map).await.is_err(), "{name}");
    }
    // And the general apply still refuses a ConfigMap of any name.
    let placement = serde_json::json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": "jc-wasm-host-0-placements", "namespace": NAMESPACE }, "data": {},
    });
    assert!(kube.apply(&placement).await.is_err());
    assert!(
        kube_server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty(),
        "nothing was sent"
    );
    kube.apply_placement(&placement)
        .await
        .expect("a shard's placement is applied");
}

#[test]
fn a_shards_key_reaches_its_own_prefix_and_the_components_and_is_its_own() {
    let policy = artifact_store::shard_policy_document("apps", 3);
    let text = policy.to_string();
    assert!(text.contains("arn:aws:s3:::apps/apps/3/*"));
    assert!(text.contains("arn:aws:s3:::apps/components/*"));
    assert!(!text.contains("apps/apps/*"), "never every shard's prefix");
    assert!(!text.contains("apps/apps/1/"), "never another shard's");
    // Writing and deleting are on the shard's prefix alone; the components are read-only.
    let statements = policy["Statement"].as_array().expect("statements");
    let components = statements
        .iter()
        .find(|s| s["Resource"][0] == "arn:aws:s3:::apps/components/*")
        .expect("components");
    assert_eq!(components["Action"], serde_json::json!(["s3:GetObject"]));
    let listing = statements
        .iter()
        .find(|s| s["Action"][0] == "s3:ListBucket")
        .expect("listing");
    assert_eq!(
        listing["Condition"]["StringLike"]["s3:prefix"],
        serde_json::json!(["apps/3/*"])
    );

    let a = Credential::derive_shard(ROOT_SECRET, 0);
    let b = Credential::derive_shard(ROOT_SECRET, 1);
    assert_ne!(a.secret_key, b.secret_key);
    assert_eq!(a.access_key, "jc-apps-shard-0");
    assert_eq!(
        Credential::derive_shard(ROOT_SECRET, 0).secret_key,
        a.secret_key,
        "derived, not drawn"
    );
    assert!(
        !format!("{a:?}").contains(&a.secret_key),
        "Debug never prints the secret"
    );
}
