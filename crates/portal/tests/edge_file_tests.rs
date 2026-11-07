//! The file the edge serves, written through the API server (T-2668, ADR-N-030, AP-112): the
//! reconciler reads helm's base, adds each App's routes and replaces the Secret APISIX mounts,
//! only when it changed, and never says a client secret in what it reports.

use base64::Engine as _;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use std::collections::BTreeMap;

use jc_core::kinds::OrganizationLimits;
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::apps::reconciler::Settings;
use joinedcontext_portal::reconciler::app_clients::ClientSecret;
use joinedcontext_portal::reconciler::edge_file::{
    compose, edge_apps, EdgeApp, EdgeFile, EdgeLimits, EdgeOutcome, Upstream, COMPOSED_BY,
};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;

const NS: &str = "apisix";
const BASE_PATH: &str = "/api/v1/namespaces/apisix/configmaps/apisix-standalone-base";
const SECRET_PATH: &str = "/api/v1/namespaces/apisix/secrets/apisix-standalone-config";
const CLIENT_SECRET: &str = "kc-generated-7f3a9c2e";

const BASE: &str = r#"plugin_configs:
  - id: apps-surface
    plugins:
      openid-connect:
        client_id: edge
        client_secret: ${{EDGE_CLIENT_SECRET}}
        unauth_action: auth
        session:
          secret: ${{OIDC_SESSION_SECRET}}
  - id: context-endpoint
    plugins:
      request-id:
        include_in_response: true
upstreams:
  - id: portal-ui
    nodes:
      "portal.jc.svc.cluster.local:8080": 1
  - id: context-endpoint
    nodes:
      "context-gateway.jc.svc.cluster.local:8080": 1
routes:
  - id: portal-ui
    uri: /*
    host: portal.city.example
    upstream_id: portal-ui
  - id: context-endpoint
    uri: /api/endpoint/*
    host: city.example
    upstream_id: context-endpoint
    plugin_config_id: context-endpoint
#END
"#;

fn apps() -> Vec<EdgeApp> {
    vec![EdgeApp {
        name: "air-quality".into(),
        public: false,
        upstream: Upstream::Pod {
            namespace: "jc-helsinki-apps".into(),
            csp: "default-src 'self'; frame-ancestors https://portal.city.example".into(),
        },
        secret: ClientSecret::from(CLIENT_SECRET.to_owned()),
        slugs: vec!["k4y7pq2mzt6vhx3nbwrs5cjd8f".into()],
        hostnames: Vec::new(),
        server: None,
    }]
}

fn b64(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

/// The Secret as helm seeded it: the base, helm's own labels and `keep`.
fn seeded(file: &str, annotations: Value) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": {
            "name": "apisix-standalone-config",
            "namespace": NS,
            "resourceVersion": "4711",
            "labels": { "app.kubernetes.io/managed-by": "Helm" },
            "annotations": annotations,
        },
        "type": "Opaque",
        "data": { "apisix.yaml": b64(file) },
    })
}

async fn api(secret: Option<Value>) -> MockServer {
    api_on(BASE, secret).await
}

/// The API server with this base in helm's ConfigMap.
async fn api_on(base: &str, secret: Option<Value>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(BASE_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": { "name": "apisix-standalone-base", "namespace": NS },
            "data": { "apisix.yaml": base },
        })))
        .mount(&server)
        .await;
    let answer = match secret {
        Some(secret) => ResponseTemplate::new(200).set_body_json(secret),
        None => ResponseTemplate::new(404).set_body_json(json!({ "kind": "Status", "code": 404 })),
    };
    Mock::given(method("GET"))
        .and(path(SECRET_PATH))
        .respond_with(answer)
        .mount(&server)
        .await;
    server
}

fn edge(server: &MockServer) -> EdgeFile {
    EdgeFile::new(
        KubeClient::with_token(&server.uri(), "token").expect("a client"),
        NS.to_owned(),
    )
}

fn put_body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("a JSON body")
}

/// AP-112: the seeded Secret is replaced with the composed file under its own resourceVersion,
/// keeps helm's labels and `keep`, and is marked as the Portal's from then on.
#[tokio::test]
async fn the_seeded_secret_is_replaced_with_the_composed_file_and_marked() {
    let server = api(Some(seeded(
        BASE,
        json!({ "helm.sh/resource-policy": "keep" }),
    )))
    .await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let (outcome, skipped) = edge(&server).converge(&apps(), None).await;
    assert_eq!(outcome, EdgeOutcome::Written { apps: 1 });
    assert!(skipped.is_empty(), "{skipped:?}");

    let requests = server.received_requests().await.expect("recorded");
    let put = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT")
        .expect("a PUT");
    let body = put_body(put);
    assert_eq!(body["metadata"]["resourceVersion"], "4711");
    assert_eq!(
        body["metadata"]["labels"]["app.kubernetes.io/managed-by"],
        "Helm"
    );
    assert_eq!(
        body["metadata"]["annotations"]["helm.sh/resource-policy"],
        "keep"
    );
    assert_eq!(body["metadata"]["annotations"][COMPOSED_BY], "portal");
    let file = base64::engine::general_purpose::STANDARD
        .decode(body["data"]["apisix.yaml"].as_str().expect("the file"))
        .expect("base64");
    let file = String::from_utf8(file).expect("utf-8");
    assert_eq!(
        file,
        compose(BASE, &apps(), &EdgeLimits::default())
            .expect("composes")
            .file
    );
    assert!(file.contains("app-air-quality") && file.contains(CLIENT_SECRET));
    assert!(file.ends_with("#END\n"));
}

/// An unchanged file is no write, so an unchanged run is no reload.
#[tokio::test]
async fn an_unchanged_file_is_not_written_again() {
    let file = compose(BASE, &apps(), &EdgeLimits::default())
        .expect("composes")
        .file;
    let server = api(Some(seeded(&file, json!({ COMPOSED_BY: "portal" })))).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    assert_eq!(outcome, EdgeOutcome::Unchanged { apps: 1 });
}

/// The reconciler never creates the Secret (its Role cannot): a missing one says who seeds it.
#[tokio::test]
async fn a_missing_secret_is_reported_and_not_created() {
    let server = api(None).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;

    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    let EdgeOutcome::Failed(reason) = outcome else {
        panic!("a missing Secret was not reported: {outcome:?}");
    };
    assert!(reason.contains("configuration chart seeds it"), "{reason}");
}

/// A refused write (a race with another writer, a Role without `update`) is reported by status,
/// and what the API server echoes back never carries the secret into the log.
#[tokio::test]
async fn a_refused_write_is_reported_without_the_file() {
    let server = api(Some(seeded(BASE, json!({})))).await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "kind": "Status",
            "message": format!("Operation cannot be fulfilled: client_secret: {CLIENT_SECRET}"),
        })))
        .mount(&server)
        .await;

    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    let EdgeOutcome::Failed(reason) = outcome else {
        panic!("a refused write was not reported: {outcome:?}");
    };
    assert!(reason.contains("409"), "{reason}");
    assert!(!reason.contains(CLIENT_SECRET), "{reason}");
}

/// T-2892, ADR-N-035: the Organization lowers the web rate; the next run writes the file with
/// the lowered count on the labelled chain, and the served file changes without a helm release.
#[tokio::test]
async fn a_lowered_organization_rate_is_written_on_the_next_run() {
    let base = BASE.replace(
        "  - id: apps-surface\n    plugins:\n",
        "  - id: apps-surface\n    labels:\n      jc-rate-class: web\n    plugins:\n      \
         limit-count:\n        count: 300\n        time_window: 60\n        key: remote_addr\n",
    );
    assert_ne!(base, BASE, "the base carries the label");
    let served = compose(&base, &apps(), &EdgeLimits::of(None, &Default::default()))
        .expect("composes")
        .file;
    let server = api_on(
        &base,
        Some(seeded(&served, json!({ COMPOSED_BY: "portal" }))),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let limits: OrganizationLimits =
        serde_json::from_value(json!({ "edge": { "requestsPerMinute": { "web": 60 } } }))
            .expect("limits");

    let (outcome, _) = edge(&server).converge(&apps(), Some(&limits)).await;
    assert_eq!(outcome, EdgeOutcome::Written { apps: 1 });

    let requests = server.received_requests().await.expect("recorded");
    let put = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT")
        .expect("a PUT");
    let file = base64::engine::general_purpose::STANDARD
        .decode(
            put_body(put)["data"]["apisix.yaml"]
                .as_str()
                .expect("the file"),
        )
        .expect("base64");
    let file: Value = serde_norway::from_slice(&file).expect("YAML");
    let surface = file["plugin_configs"]
        .as_array()
        .expect("plugin_configs")
        .iter()
        .find(|pc| pc["id"] == "apps-surface")
        .expect("the surface");
    assert_eq!(surface["plugins"]["limit-count"]["count"], 60);
    // The App's own chain copies the surface, so it counts the lowered rate too.
    let own = file["plugin_configs"]
        .as_array()
        .expect("plugin_configs")
        .iter()
        .find(|pc| pc["id"] == "app-air-quality")
        .expect("the App's chain");
    assert_eq!(own["plugins"]["limit-count"]["count"], 60);
}

/// A published App of `class` in `helsinki`, declaring `connect_src`.
fn published(name: &str, class: &str, connect_src: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "App".into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some("helsinki".into()),
            ..Default::default()
        },
        spec: json!({
            "kind": class,
            "source": { "path": "./src" },
            "build": { "rust": "1.90", "node": "22" },
            "visibility": "project",
            "lifecycle": "published",
            "csp": { "connectSrc": connect_src },
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-all" },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
                "representations": ["ngsi-ld"]
            }],
            "limits": { "requestsPerMinute": 600, "maxFileRows": 20000 }
        }),
        status: None,
    }
}

fn pod_settings(basemap_base: Option<&str>) -> Settings {
    Settings {
        host: "portal.city.example".into(),
        apex: "city.example".into(),
        gateway_url: Some("http://context-gateway.jc.svc.cluster.local:8080".into()),
        service_url: Some("http://portal.joinedcontext.svc.cluster.local:8080".into()),
        namespace: "jc".into(),
        release: Some("jc".into()),
        service_account: Some("portal".into()),
        org_domain: "city.example".into(),
        apisix_namespace: NS.into(),
        image_repository: None,
        pull_secret: None,
        basemap_base: basemap_base.map(str::to_owned),
    }
}

/// A pod App's page gets the policy its manifest allows, which the App's own server never sends
/// (AP-12, AR-2, T-3011): only its own host, its declared https origin and the project's basemap,
/// framed by the Portal alone. A static App's host writes its own, so its route carries none.
#[test]
fn a_pod_app_route_carries_the_policy_of_its_manifest() {
    let mirror = Mirror::new();
    mirror.upsert(published(
        "air-quality",
        "ui-rust",
        json!([
            "https://api.example.fi",
            "https://evil.example; script-src 'unsafe-inline'"
        ]),
    ));
    mirror.upsert(published("praha-mesto", "ui", json!([])));
    let secrets: BTreeMap<String, ClientSecret> = ["air-quality", "praha-mesto"]
        .into_iter()
        .map(|name| (name.to_owned(), ClientSecret::from(format!("s-{name}"))))
        .collect();

    let (apps, skipped) = edge_apps(
        &mirror,
        &secrets,
        &pod_settings(Some("https://portal.city.example/")),
        None,
    );
    assert!(skipped.is_empty(), "{skipped:?}");
    let upstream = |name: &str| {
        apps.iter()
            .find(|app| app.name == name)
            .unwrap_or_else(|| panic!("{name} is routed"))
            .upstream
            .clone()
    };
    assert_eq!(upstream("praha-mesto"), Upstream::Static);
    let Upstream::Pod { namespace, csp } = upstream("air-quality") else {
        panic!("air-quality runs in a pod");
    };
    assert_eq!(namespace, "jc-helsinki-apps");
    for directive in [
        "script-src 'self';",
        "object-src 'none';",
        "base-uri 'self';",
        "connect-src 'self' https://api.example.fi \
         https://portal.city.example/api/v1/projects/helsinki/basemap/;",
        "img-src 'self' data: blob: https://portal.city.example/api/v1/projects/helsinki/basemap/;",
    ] {
        assert!(csp.contains(directive), "{directive} in {csp}");
    }
    assert!(
        csp.ends_with("frame-ancestors https://portal.city.example"),
        "{csp}"
    );
    assert!(!csp.contains("evil.example"), "{csp}");
    assert_eq!(csp.matches("script-src").count(), 1, "{csp}");

    // Without a basemap nothing of the Portal but its frame is named.
    let (apps, _) = edge_apps(&mirror, &secrets, &pod_settings(None), None);
    let air = apps
        .iter()
        .find(|app| app.name == "air-quality")
        .expect("routed");
    let Upstream::Pod { csp, .. } = &air.upstream else {
        panic!("air-quality runs in a pod");
    };
    assert!(
        csp.contains("connect-src 'self' https://api.example.fi;"),
        "{csp}"
    );
    assert!(!csp.contains("basemap"), "{csp}");
}

/// T-3579: a published wasm App placed on a shard names that shard's Service as its server, so
/// the edge routes its `/api` there; a wasm App on no shard yet, a `ui` App, or a Portal without
/// the shards' namespace names none.
#[test]
fn a_wasm_app_on_a_shard_names_its_shards_service_as_its_server() {
    let mirror = Mirror::new();
    let mut placed = published("kpi-forecast", "wasm", json!([]));
    placed.status = Some(serde_json::from_value(json!({ "shard": 1 })).expect("a status"));
    mirror.upsert(placed);
    mirror.upsert(published("news-topics", "wasm", json!([])));
    mirror.upsert(published("praha-mesto", "ui", json!([])));
    let secrets: BTreeMap<String, ClientSecret> = ["kpi-forecast", "news-topics", "praha-mesto"]
        .into_iter()
        .map(|name| (name.to_owned(), ClientSecret::from(format!("s-{name}"))))
        .collect();

    let (apps, skipped) = edge_apps(&mirror, &secrets, &pod_settings(None), Some("wasm-host"));
    assert!(skipped.is_empty(), "{skipped:?}");
    let server = |name: &str| {
        apps.iter()
            .find(|app| app.name == name)
            .unwrap_or_else(|| panic!("{name} is routed"))
            .server
            .clone()
    };
    assert_eq!(
        server("kpi-forecast").as_deref(),
        Some("jc-wasm-host-1.wasm-host.svc.cluster.local:8080")
    );
    assert_eq!(server("news-topics"), None, "on no shard yet");
    assert_eq!(server("praha-mesto"), None, "a ui App has no server");

    let (apps, _) = edge_apps(&mirror, &secrets, &pod_settings(None), None);
    assert!(apps.iter().all(|app| app.server.is_none()), "{apps:?}");
}
