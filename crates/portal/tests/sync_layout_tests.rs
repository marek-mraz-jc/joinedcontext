//! The mirror of a layout 2 organization: the organization repository and each registered
//! project's own repository, assembled at the entry's ref (T-2642, CC-86, PF-86).

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ORG: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n  \
                   name: banskabystrica\n  namespace: org\nspec:\n  domain: banskabystrica.sk\n  \
                   locales: [\"sk\"]\n  defaultLocale: sk\n";
const ENTRY: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                     name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n  \
                     repository: { name: ovzdusie }\n  ref: main\n";
const PROJECT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                       name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: banskabystrica\n";
const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                     name: ovzdusie\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n";

/// One repository of the forge at `git_ref`: its tree and each file's contents.
async fn repository(server: &MockServer, repo: &str, git_ref: &str, files: &[(&str, &str)]) {
    let base = format!("/api/v1/repos/test-owner/{repo}");
    let tree: Vec<_> = files
        .iter()
        .map(|(file, _)| json!({ "path": file, "type": "blob", "sha": format!("blob-{file}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{base}/git/trees/{git_ref}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree", "truncated": false, "tree": tree
        })))
        .mount(server)
        .await;
    for (file, body) in files {
        Mock::given(method("GET"))
            .and(path(format!("{base}/contents/{file}")))
            .and(query_param("ref", git_ref))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": format!("blob-{file}"), "content": STANDARD.encode(body)
            })))
            .mount(server)
            .await;
    }
}

/// The organization repository `test-repo` of layout 2 at `c0ffee`, registering `ovzdusie`.
async fn organization(server: &MockServer) {
    let base = "/api/v1/repos/test-owner/test-repo";
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/branches/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main", "commit": { "id": "c0ffee" }
        })))
        .mount(server)
        .await;
    repository(
        server,
        "test-repo",
        "c0ffee",
        &[
            ("org.yaml", ORG),
            (".jc/layout", "2\n"),
            ("projects/ovzdusie.yaml", ENTRY),
        ],
    )
    .await;
}

fn syncer(server: &MockServer) -> (Syncer, Arc<Mirror>) {
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "t",
    )
    .expect("client");
    let mirror = Arc::new(Mirror::new());
    (Syncer::new(Arc::new(client), Arc::clone(&mirror)), mirror)
}

/// CC-86: the project's manifests come from its own repository, mounted under its slug, and a
/// resource's source link opens the file in that repository, not in the organization's.
#[tokio::test]
async fn a_project_is_read_from_its_own_repository() {
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
        ],
    )
    .await;
    let (syncer, mirror) = syncer(&server);
    syncer
        .sync_once()
        .await
        .expect("the organization assembles");

    let space = mirror
        .get("ovzdusie", "ContextSpace", "ovzdusie")
        .expect("the project's space is in the mirror");
    let source = space
        .status
        .as_ref()
        .and_then(|status| status.source_url.clone())
        .expect("a source link");
    // Behind the forge's sign-in, so the path travels encoded in `redirect_to`.
    let decoded = source.replace("%2F", "/");
    assert!(
        decoded.contains("/test-owner/ovzdusie/src/branch/main/spaces/ovzdusie/space.yaml"),
        "{source}"
    );
    assert!(
        mirror.get("org", "Project", "ovzdusie").is_some(),
        "the project is there"
    );
    assert_eq!(
        mirror.repository_of("ovzdusie").as_deref(),
        Some("ovzdusie"),
        "writes to the project go to its own repository (CC-87)"
    );
}

/// T-6455: a merge into a project's own repository goes live from a render apart from the
/// assembly the runs share, which a run in flight may be reading.
#[tokio::test]
async fn a_merge_into_a_project_goes_live_from_its_own_render() {
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
        ],
    )
    .await;
    let (syncer, mirror) = syncer(&server);
    syncer.sync_once().await.expect("the first sync");

    server.reset().await;
    organization(&server).await;
    let second = SPACE.replace(
        "name: ovzdusie\n  namespace: ovzdusie",
        "name: vendor\n  namespace: ovzdusie",
    );
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
            ("spaces/vendor/space.yaml", &second),
        ],
    )
    .await;
    syncer.go_live().await.expect("the merge loads");
    assert!(mirror.get("ovzdusie", "ContextSpace", "vendor").is_some());
}

/// CC-86: a project repository that cannot be read leaves that project at its last render and
/// the organization syncs; the project is not unloaded.
#[tokio::test]
async fn an_unreadable_project_keeps_its_last_render() {
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
        ],
    )
    .await;
    let (syncer, mirror) = syncer(&server);
    syncer.sync_once().await.expect("the first sync");
    assert!(mirror.get("ovzdusie", "ContextSpace", "ovzdusie").is_some());

    server.reset().await;
    organization(&server).await;
    syncer
        .sync_once()
        .await
        .expect("the organization still syncs");
    assert!(
        mirror.get("ovzdusie", "ContextSpace", "ovzdusie").is_some(),
        "the space the last assembly rendered stays"
    );
}

/// PF-77: once a project's registry entry is gone (its deletion Change merged), the next sync
/// archives the project's repository, keeping its history, and removes nothing.
#[tokio::test]
async fn a_project_taken_out_of_the_registry_has_its_repository_archived() {
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
        ],
    )
    .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/repos/test-owner/ovzdusie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "archived": true })))
        .mount(&server)
        .await;
    let (syncer, mirror) = syncer(&server);
    syncer.sync_once().await.expect("the first sync");

    server.reset().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/repos/test-owner/ovzdusie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "archived": true })))
        .mount(&server)
        .await;
    let base = "/api/v1/repos/test-owner/test-repo";
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{base}/branches/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main", "commit": { "id": "d00d" }
        })))
        .mount(&server)
        .await;
    repository(
        &server,
        "test-repo",
        "d00d",
        &[("org.yaml", ORG), (".jc/layout", "2\n")],
    )
    .await;
    syncer
        .sync_once()
        .await
        .expect("the sync after the deletion");

    let archived: Vec<serde_json::Value> = server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    assert_eq!(archived, [json!({ "archived": true })]);
    assert!(mirror.repository_of("ovzdusie").is_none());
    assert!(
        server
            .received_requests()
            .await
            .expect("requests")
            .iter()
            .all(|r| r.method.as_str() != "DELETE"),
        "nothing is removed"
    );
}

/// A cluster holding the edge's Ingress and Certificate and the host (Certificate and Ingress)
/// the Portal once made for the App `luft`, answering every delete.
async fn cluster_with_app_host() -> MockServer {
    const CERTS: &str = "/apis/cert-manager.io/v1/namespaces/apisix/certificates";
    const INGRESSES: &str = "/apis/networking.k8s.io/v1/namespaces/apisix/ingresses";
    let cluster = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{INGRESSES}/apisix")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": { "name": "apisix" },
            "spec": { "rules": [{ "host": "city.example", "http": { "paths": [{
                "path": "/", "pathType": "Prefix",
                "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
            }] } }] },
        })))
        .mount(&cluster)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{CERTS}/apisix-edge")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "spec": { "issuerRef": { "name": "letsencrypt-prod", "kind": "ClusterIssuer" } },
        })))
        .mount(&cluster)
        .await;
    for (collection, kind) in [(CERTS, "Certificate"), (INGRESSES, "Ingress")] {
        let held = json!({ "kind": kind, "metadata": { "name": "app-luft", "labels": {
            "app.kubernetes.io/managed-by": "joinedcontext-portal",
            "app.kubernetes.io/component": "app-host",
            "joinedcontext.com/app": "luft",
        } } });
        Mock::given(method("GET"))
            .and(path(collection))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [held] })))
            .mount(&cluster)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("{collection}/app-luft")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&cluster)
            .await;
    }
    cluster
}

/// A syncer with the edge and the App hosts on `cluster`, as a replica that just started.
fn syncer_on_cluster(server: &MockServer, cluster: &MockServer) -> (Syncer, Arc<Mirror>) {
    use joinedcontext_portal::apps::kube::KubeClient;
    use joinedcontext_portal::apps::reconciler::Settings;
    use joinedcontext_portal::reconciler::app_hosts::AppHosts;
    use joinedcontext_portal::reconciler::edge_file::EdgeFile;
    let kube = || KubeClient::with_token(&cluster.uri(), "token").expect("a kube client");
    let settings = Settings {
        host: "city.example".into(),
        apex: "city.example".into(),
        gateway_url: None,
        service_url: None,
        namespace: "jc".into(),
        org_domain: "banskabystrica.sk".into(),
        apisix_namespace: "apisix".into(),
        image_repository: None,
        pull_secret: None,
        basemap_base: None,
        release: None,
        service_account: None,
    };
    let (syncer, mirror) = syncer(server);
    let syncer = syncer
        .with_edge_file(
            Arc::new(EdgeFile::new(kube(), "apisix".to_owned())),
            settings,
        )
        .with_app_hosts(Arc::new(AppHosts::new(kube(), "apisix".to_owned())));
    (syncer, mirror)
}

async fn deletes(cluster: &MockServer) -> usize {
    cluster
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .count()
}

/// AP-133: a replica whose first run cannot read a project's repository holds none of that
/// project's Apps, which is not the same as their being retired: their certificates stay, or
/// every restart during a forge hiccup re-requests them against Let's Encrypt's weekly limit.
#[tokio::test]
async fn an_unstaged_project_keeps_its_app_hosts() {
    let server = MockServer::start().await;
    organization(&server).await;
    let cluster = cluster_with_app_host().await;
    let (syncer, _mirror) = syncer_on_cluster(&server, &cluster);

    syncer
        .sync_once()
        .await
        .expect("the organization syncs without the project");
    assert_eq!(
        deletes(&cluster).await,
        0,
        "no App host is removed while a project's repository did not stage"
    );
}

/// AP-133: once every project stages, an App host no published App names is removed.
#[tokio::test]
async fn a_complete_run_retires_an_unpublished_app_host() {
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
        ],
    )
    .await;
    let cluster = cluster_with_app_host().await;
    let (syncer, _mirror) = syncer_on_cluster(&server, &cluster);

    syncer.sync_once().await.expect("the organization syncs");
    assert_eq!(
        deletes(&cluster).await,
        2,
        "the Certificate and the Ingress of an App no project publishes"
    );
}

/// AP-149, T-3603: a wasm App whose manifest in its project's repository records the publish
/// (`status.build` with its component, and `status.shard`) keeps both in the mirror, so the host
/// places it on that shard.
#[tokio::test]
async fn a_published_wasm_app_keeps_its_build_and_shard() {
    let digest = format!("sha256:{}", "2".repeat(64));
    let component = format!("sha256:{}", "1".repeat(64));
    let app = format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: App\nmetadata:\n  name: kpi-forecast\n  \
         namespace: ovzdusie\nspec:\n  kind: wasm\n  source:\n    git:\n      url: \
         https://forge.example.org/git/org/ovzdusie_kpi-forecast.git\n      ref: main\n  \
         build:\n    node: \"22\"\n    rust: \"1.90\"\n  server:\n    jobs:\n      - name: \
         daily-forecasts\n        schedule: \"20 3 * * *\"\n        export: record-forecasts\n  \
         visibility: public\n  lifecycle: published\n  dataNeeds: []\nstatus:\n  phase: Pending\n  \
         build:\n    digest: {digest}\n    commit: \"{commit}\"\n    sdkVersion: 0.4.1\n    builtAt: \"2026-10-10T20:04:45Z\"\n    component: {component}\n  \
         shard: 0\n",
        commit = "6".repeat(40),
    );
    let server = MockServer::start().await;
    organization(&server).await;
    repository(
        &server,
        "ovzdusie",
        "main",
        &[
            (".jc/layout", "2\n"),
            ("project.yaml", PROJECT),
            ("spaces/ovzdusie/space.yaml", SPACE),
            ("apps/kpi-forecast/app.yaml", &app),
        ],
    )
    .await;
    let (syncer, mirror) = syncer(&server);
    syncer
        .sync_once()
        .await
        .expect("the organization assembles");

    let app = mirror
        .get("ovzdusie", "App", "kpi-forecast")
        .expect("the App is in the mirror");
    let status = app.status.as_ref().expect("a status");
    let build = status.build.as_ref().expect("the build the lane recorded");
    assert_eq!(build.component.as_deref(), Some(component.as_str()));
    assert_eq!(status.shard, Some(0), "the shard the publish recorded");
    let placed = joinedcontext_portal::reconciler::wasm_shards::PlacedApp::of(&app)
        .expect("a published wasm App with its component is placed");
    assert_eq!(placed.shard, 0);
    assert_eq!(placed.digest, component);
}
