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
        prefix: None,
        legacy: false,
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
    // Without the Portal's database there is no project key: odpad stays under apps/1/<id>/.
    assert_eq!(
        policies[1],
        artifact_store::shard_policy_document(
            "apps",
            &[format!(
                "apps/1/{}/",
                joinedcontext_portal::apps::apps_db::app_id("praha", "odpad")
            )]
        )
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

/// An in-memory S3 bucket: listing, GET, HEAD, PUT (with `x-amz-copy-source` and
/// `If-None-Match: *`) and DELETE, enough for a move to run against.
#[derive(Clone, Default)]
struct Bucket(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>);

impl Bucket {
    fn put(&self, key: &str, body: &[u8]) {
        self.0.lock().unwrap().insert(key.to_owned(), body.to_vec());
    }
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn keys(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

impl wiremock::Respond for Bucket {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let path = request.url.path().trim_start_matches("/apps/").to_owned();
        let mut objects = self.0.lock().unwrap();
        match request.method.as_str() {
            "GET" if path.is_empty() || path == "/apps" => {
                let prefix = request
                    .url
                    .query_pairs()
                    .find(|(k, _)| k == "prefix")
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                let contents: String = objects
                    .iter()
                    .filter(|(key, _)| key.starts_with(&prefix))
                    .map(|(key, body)| {
                        format!(
                            "<Contents><Key>{key}</Key><Size>{}</Size></Contents>",
                            body.len()
                        )
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_string(format!(
                    "<ListBucketResult><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
                ))
            }
            "GET" | "HEAD" => match objects.get(&path) {
                Some(body) => ResponseTemplate::new(200).set_body_bytes(body.clone()),
                None => ResponseTemplate::new(404),
            },
            "PUT" => {
                let body = match request.headers.get("x-amz-copy-source") {
                    Some(source) => {
                        let source = source.to_str().unwrap().trim_start_matches("/apps/");
                        match objects.get(source) {
                            Some(body) => body.clone(),
                            None => return ResponseTemplate::new(404),
                        }
                    }
                    None => request.body.clone(),
                };
                if request.headers.get("if-none-match").is_some() && objects.contains_key(&path) {
                    return ResponseTemplate::new(412);
                }
                objects.insert(path, body);
                ResponseTemplate::new(200)
            }
            "DELETE" => {
                objects.remove(&path);
                ResponseTemplate::new(204)
            }
            _ => ResponseTemplate::new(405),
        }
    }
}

/// TEN-15: an App's files move from `apps/<shard>/<id>/` under its project's key. An object the
/// App wrote under the key meanwhile is kept, never overwritten by the old copy; every moved one
/// reads back the same bytes; the rollback entry names both prefixes and each object's hash; the
/// old prefix is empty after, and the placement and the shard's key stop naming it.
#[tokio::test]
async fn an_apps_old_files_move_under_its_project_key_and_a_write_made_meanwhile_is_kept() {
    use joinedcontext_portal::apps::files::{keyed_prefix, legacy_prefix, move_legacy, Moved};
    use joinedcontext_portal::reconciler::wasm_shards::{placement, prefixes};
    let server = MockServer::start().await;
    let bucket = Bucket::default();
    Mock::given(wiremock::matchers::any())
        .respond_with(bucket.clone())
        .mount(&server)
        .await;
    let store = Client::new(Settings {
        bucket: "apps".to_owned(),
        ..store_settings(server.uri())
    })
    .expect("a store client");
    let key = jc_types::ProjectKey::new(jc_types::OrgId::mint(), jc_types::ProjectId::mint());
    let other = jc_types::ProjectKey::new(key.org, jc_types::ProjectId::mint());
    let id = joinedcontext_portal::apps::apps_db::app_id("helsinki", "notes");
    let id = id.as_str();
    let (legacy, keyed) = (legacy_prefix(2, id), keyed_prefix(&key, id));
    assert_eq!(keyed, format!("{}apps/{id}/", key.object_prefix()));
    assert_ne!(
        keyed,
        keyed_prefix(&other, id),
        "two projects never share a prefix"
    );
    bucket.put(&format!("{legacy}notes/a.txt"), b"old a");
    bucket.put(&format!("{legacy}notes/b.txt"), b"old b");
    // The App wrote b under its key after the placement named it: that write wins.
    bucket.put(&format!("{keyed}notes/b.txt"), b"new b");
    bucket.put("apps/2/someone_else/c.txt", b"not this App's");

    let moved = move_legacy(&store, "apps", (id, &legacy, &keyed), "20261117T120000Z")
        .await
        .expect("moved");
    assert_eq!(
        moved,
        Moved {
            copied: 1,
            superseded: 1,
            done: true
        }
    );
    assert_eq!(
        bucket.get(&format!("{keyed}notes/a.txt")).as_deref(),
        Some(&b"old a"[..])
    );
    assert_eq!(
        bucket.get(&format!("{keyed}notes/b.txt")).as_deref(),
        Some(&b"new b"[..])
    );
    assert!(
        bucket.keys().iter().all(|k| !k.starts_with(&legacy)),
        "the old prefix is empty"
    );
    assert!(
        bucket.get("apps/2/someone_else/c.txt").is_some(),
        "another App's files stay"
    );
    let rollback: Value = serde_json::from_slice(
        &bucket
            .get(&format!("apps/moved/{id}/20261117T120000Z.json"))
            .expect("a rollback entry"),
    )
    .expect("JSON");
    assert_eq!(rollback["from"], legacy.as_str());
    assert_eq!(rollback["to"], keyed.as_str());
    assert_eq!(rollback["objects"].as_array().map(Vec::len), Some(2));
    assert!(rollback.to_string().contains("superseded"));
    assert!(
        !rollback.to_string().contains("old a"),
        "no payload in the entry"
    );

    // A second move finds nothing and is done without writing.
    let again = move_legacy(&store, "apps", (id, &legacy, &keyed), "20261117T120100Z")
        .await
        .expect("moved");
    assert!(again.done && again.copied == 0);
    assert!(bucket
        .get(&format!("apps/moved/{id}/20261117T120100Z.json"))
        .is_none());

    // The placement names the keyed prefix; the shard's key reaches the old one only until moved.
    let mut placed = app("helsinki", "notes", 2);
    placed.prefix = Some(keyed.clone());
    placed.legacy = true;
    let (moving, _) = placement(2, std::slice::from_ref(&placed));
    assert_eq!(moving["apps"][0]["prefix"], keyed.as_str());
    assert_eq!(moving["apps"][0]["legacy"], true);
    assert_eq!(prefixes(2, &moving), [keyed.clone(), legacy.clone()]);
    placed.legacy = false;
    let (done, _) = placement(2, &[placed]);
    assert_eq!(prefixes(2, &done), [keyed]);
}

/// TEN-15, T-7210: a project's published artifacts move from `{area}/{org}/{project}/` under its
/// project key. A newer write under the key is kept, the rollback entry sits outside every
/// project's prefix with hashes and no payload, a sibling project's old artifacts are untouched,
/// and once the old layout is empty the reader's policy stops naming it.
#[tokio::test]
async fn a_projects_old_artifacts_move_under_its_key_and_the_old_layout_is_then_refused() {
    use jc_types::{OrgId, ProjectId, ProjectKey};
    use joinedcontext_portal::artifact_store::{move_artifacts, policy_document, Legacy, Role};
    let server = MockServer::start().await;
    let bucket = Bucket::default();
    Mock::given(wiremock::matchers::any())
        .respond_with(bucket.clone())
        .mount(&server)
        .await;
    let store = Client::new(Settings {
        bucket: "apps".to_owned(),
        ..store_settings(server.uri())
    })
    .expect("a store client");
    let key = ProjectKey::new(OrgId::mint(), ProjectId::mint());
    let keyed = key.object_prefix();
    let legacy = Legacy::new("hel", "air").expect("labels");
    bucket.put("schemas/hel/air/aq/v1/aq.linkml.yaml", b"old model");
    bucket.put("mappings/hel/air/m/m.bloblang", b"old mapping");
    bucket.put(&format!("{keyed}mappings/m/m.bloblang"), b"newer mapping");
    bucket.put(
        "schemas/hel/airx/aq/v1/aq.linkml.yaml",
        b"a sibling's model",
    );

    let moved = move_artifacts(&store, &key, &legacy, "20261117T130000Z")
        .await
        .expect("the move runs");
    assert_eq!((moved.copied, moved.superseded, moved.done), (1, 1, true));
    assert_eq!(
        bucket
            .get(&format!("{keyed}schemas/aq/v1/aq.linkml.yaml"))
            .as_deref(),
        Some(&b"old model"[..])
    );
    assert_eq!(
        bucket
            .get(&format!("{keyed}mappings/m/m.bloblang"))
            .as_deref(),
        Some(&b"newer mapping"[..]),
        "the old copy overwrote a newer write"
    );
    let keys = bucket.keys();
    assert!(!keys
        .iter()
        .any(|k| k.starts_with("schemas/hel/air/") || k.starts_with("mappings/hel/air/")));
    assert!(
        keys.contains(&"schemas/hel/airx/aq/v1/aq.linkml.yaml".to_owned()),
        "{keys:?}"
    );
    let rollback = bucket
        .get("moved/schemas/hel/air/20261117T130000Z.json")
        .expect("a rollback entry per area");
    let rollback: Value = serde_json::from_slice(&rollback).expect("JSON");
    assert_eq!(rollback["from"], "schemas/hel/air/");
    assert_eq!(rollback["to"], format!("{keyed}schemas/"));
    assert_eq!(rollback["objects"][0]["outcome"], "copied");
    assert!(
        !rollback.to_string().contains("old model"),
        "a payload in the rollback entry"
    );

    let again = move_artifacts(&store, &key, &legacy, "20261117T130100Z")
        .await
        .expect("a second run");
    assert_eq!((again.copied, again.superseded, again.done), (0, 0, true));
    // Moved: the converge passes no legacy, and the reader reads the key alone.
    let reader = policy_document("apps", &key, Role::Reader, None).to_string();
    assert!(!reader.contains("/hel/air/"), "{reader}");
    assert!(
        reader.contains(&format!("apps/{keyed}schemas/*")),
        "{reader}"
    );
}

#[test]
fn a_shards_key_reaches_its_apps_own_prefixes_and_the_components_and_is_its_own() {
    // Two projects' Apps: A's on shard 3, keyed and not yet moved; B's elsewhere.
    let a = "org/0b8e7f1c-1d2e-4f30-8a41-5b6c7d8e9f00/project/1c9f8e7d-6b5a-4c3d-9e2f-0a1b2c3d4e5f/apps/x1/";
    let b = "org/0b8e7f1c-1d2e-4f30-8a41-5b6c7d8e9f00/project/2d0a9f8e-7c6b-4d5e-8f30-1b2c3d4e5f60/apps/x2/";
    let policy =
        artifact_store::shard_policy_document("apps", &[a.to_owned(), "apps/3/x1/".to_owned()]);
    let text = policy.to_string();
    assert!(text.contains(&format!("arn:aws:s3:::apps/{a}*")));
    assert!(text.contains("arn:aws:s3:::apps/apps/3/x1/*"));
    assert!(text.contains("arn:aws:s3:::apps/components/*"));
    assert!(!text.contains(b), "never another project's prefix");
    assert!(
        !text.contains("apps/apps/3/*"),
        "never the whole old shard prefix"
    );
    assert!(!text.contains("apps/org/*"), "never every Organization's");
    // Writing and deleting are on the App's prefixes alone; the components are read-only.
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
        serde_json::json!([format!("{a}*"), "apps/3/x1/*"])
    );
    // A shard with no App reads the components and nothing else.
    let empty = artifact_store::shard_policy_document("apps", &[]);
    assert_eq!(empty["Statement"].as_array().map(Vec::len), Some(1));

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
