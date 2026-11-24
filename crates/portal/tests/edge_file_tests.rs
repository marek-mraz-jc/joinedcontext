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

/// A port with nothing listening on it: the socket stays bound, so no other test takes the port,
/// and a connect to it is refused at once.
fn refused_port() -> (tokio::net::TcpSocket, u16) {
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .bind("127.0.0.1:0".parse().expect("an address"))
        .expect("bound");
    let port = socket.local_addr().expect("its address").port();
    (socket, port)
}

/// A base the API server refuses names the ConfigMap and the status; an API server that cannot
/// be reached names the ConfigMap; neither writes anything.
#[tokio::test]
async fn a_base_that_cannot_be_read_fails_the_run_by_name() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(BASE_PATH))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "kind": "Status" })))
        .mount(&server)
        .await;
    let (outcome, skipped) = edge(&server).converge(&apps(), None).await;
    assert_eq!(
        outcome,
        EdgeOutcome::Failed(
            "the API server answered 500 for ConfigMap apisix-standalone-base".into()
        )
    );
    assert!(skipped.is_empty(), "{skipped:?}");

    let (_held, port) = refused_port();
    let gone = EdgeFile::new(
        KubeClient::with_token(&format!("http://127.0.0.1:{port}"), "token").expect("a client"),
        NS.to_owned(),
    );
    let (outcome, _) = gone.converge(&apps(), None).await;
    let EdgeOutcome::Failed(reason) = outcome else {
        panic!("an unreachable API server was not reported: {outcome:?}");
    };
    assert!(
        reason.starts_with("ConfigMap apisix-standalone-base: "),
        "{reason}"
    );
}

/// A served Secret the API server refuses to read names the Secret and the status; a served
/// Secret without annotations is written with the Portal's mark alone.
#[tokio::test]
async fn the_served_secret_is_read_by_name_and_marked_when_it_has_no_annotations() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(BASE_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "apisix.yaml": BASE },
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({ "kind": "Status" })))
        .mount(&server)
        .await;
    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    assert_eq!(
        outcome,
        EdgeOutcome::Failed(
            "the API server answered 403 for Secret apisix-standalone-config".into()
        )
    );

    let server = api(Some(seeded(BASE, Value::Null))).await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    assert_eq!(outcome, EdgeOutcome::Written { apps: 1 });
    let requests = server.received_requests().await.expect("recorded");
    let put = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT")
        .expect("a PUT");
    assert_eq!(
        put_body(put)["metadata"]["annotations"],
        json!({ COMPOSED_BY: "portal" })
    );
}

/// A base that does not compose (here without the Portal's route) is reported with what it
/// lacks, and the served file is left as it is.
#[tokio::test]
async fn a_base_that_does_not_compose_is_reported_and_not_written() {
    let base = BASE.replace(
        "  - id: portal-ui\n    uri: /*\n",
        "  - id: other-ui\n    uri: /*\n",
    );
    assert_ne!(base, BASE);
    let server = api_on(&base, Some(seeded(BASE, json!({})))).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let (outcome, _) = edge(&server).converge(&apps(), None).await;
    let EdgeOutcome::Failed(reason) = outcome else {
        panic!("a base without the Portal route composed: {outcome:?}");
    };
    assert!(reason.contains("portal-ui"), "{reason}");
}

/// A host registry that cannot be read leaves the served file as it is, so its hosts keep their
/// routes until a run reads it again.
#[tokio::test]
async fn a_registry_that_cannot_be_read_leaves_the_served_file_as_it_is() {
    let server = api(Some(seeded(BASE, json!({})))).await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let (_held, port) = refused_port();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_lazy(&format!("postgres://jc:jc@127.0.0.1:{port}/jc"))
        .expect("a lazy pool");
    let (outcome, skipped) = edge(&server)
        .with_registry(Some(pool))
        .converge(&apps(), None)
        .await;
    assert_eq!(
        outcome,
        EdgeOutcome::Failed(
            "the host registry could not be read; the served file stays as it is".into()
        )
    );
    assert!(skipped.is_empty(), "{skipped:?}");
}

/// T-7137 through the run: each served host of the registry gets its route to jc in the written
/// file, a removed one gets none, and a host whose route id another host took is left out.
#[tokio::test]
async fn the_served_registry_hosts_reach_the_written_file() {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let run = format!("ef{:x}", std::process::id());
    let served = format!("{run}.city.example");
    // Both name the route `jc-org-<run>-a-city-example`: the first one takes it.
    let first = format!("{run}-a.city.example");
    let second = format!("{run}.a-city.example");
    let removed = format!("{run}-gone.city.example");
    let hosts: Vec<_> = [&served, &first, &second, &removed]
        .into_iter()
        .map(|host| joinedcontext_portal::host_registry::SeedHost {
            host: host.clone(),
            organization: "helsinki".into(),
            organization_id: None,
            role: "working".into(),
        })
        .collect();
    joinedcontext_portal::host_registry::seed(&pool, &hosts)
        .await
        .expect("seeded");
    joinedcontext_portal::host_registry::remove(&pool, &removed)
        .await
        .expect("removed");

    let base = BASE.replace(
        "routes:\n",
        "routes:\n  - id: jc\n    uri: /*\n    host: jc.city.example\n    upstream_id: jc\n",
    );
    let server = api_on(&base, Some(seeded(&base, json!({})))).await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let (outcome, _) = edge(&server)
        .with_registry(Some(pool.clone()))
        .converge(&apps(), None)
        .await;
    for host in &hosts {
        joinedcontext_portal::host_registry::remove(&pool, &host.host)
            .await
            .expect("cleaned up");
    }
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
    let on = |host: &str| {
        file["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .filter(|r| r["host"] == host)
            .count()
    };
    assert_eq!(
        (on(&served), on(&first), on(&second), on(&removed)),
        (1, 1, 0, 0)
    );
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

/// The operator's bounds hold over the Organization's own rate: a rate above the operator's
/// ceiling counts the ceiling, and the wasm shards' namespace is the one handed in.
#[tokio::test]
async fn the_operators_bounds_cap_the_organizations_rate() {
    let base = BASE.replace(
        "  - id: apps-surface\n    plugins:\n",
        "  - id: apps-surface\n    labels:\n      jc-rate-class: web\n    plugins:\n      \
         limit-count:\n        count: 300\n        time_window: 60\n        key: remote_addr\n",
    );
    let server = api_on(&base, Some(seeded(&base, json!({})))).await;
    Mock::given(method("PUT"))
        .and(path(SECRET_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let limits: OrganizationLimits =
        serde_json::from_value(json!({ "edge": { "requestsPerMinute": { "web": 2000 } } }))
            .expect("limits");
    let bounds = jc_core::kinds::OrganizationBounds(BTreeMap::from([(
        "spec.limits.edge.requestsPerMinute.web".to_string(),
        jc_core::kinds::org_settings::Bound {
            min: Some(10),
            max: Some(500),
        },
    )]));
    let edge = edge(&server)
        .with_bounds(bounds)
        .with_wasm_host(Some("jc-wasm".into()));
    assert_eq!(edge.wasm_host(), Some("jc-wasm"));
    let (outcome, _) = edge.converge(&apps(), Some(&limits)).await;
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
    assert_eq!(surface["plugins"]["limit-count"]["count"], 500);
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
/// AP-04: an App's own generated Endpoint gives its routes that Endpoint's slug; an App with no
/// client yet keeps the shared surface, and an App whose spec does not parse is skipped by name.
#[test]
fn an_apps_own_endpoint_names_its_slug_and_an_unreadable_app_is_skipped() {
    let mirror = Mirror::new();
    mirror.upsert(published("praha-mesto", "ui", json!([])));
    mirror.upsert(published("no-client-yet", "ui", json!([])));
    let mut broken = published("broken", "ui", json!([]));
    broken.spec["kind"] = json!("no-such-class");
    mirror.upsert(broken);
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Endpoint".into(),
        metadata: ObjectMeta {
            name: "app-praha-mesto".into(),
            namespace: Some("helsinki".into()),
            annotations: BTreeMap::from([(
                jc_core::annotations::GENERATED_BY.to_owned(),
                joinedcontext_portal::apps::reconciler::GENERATOR.to_owned(),
            )]),
            ..Default::default()
        },
        spec: json!({
            "slug": "k4y7pq2mzt6vhx3nbwrs5cjd8f",
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-all" },
        }),
        status: None,
    });
    let secrets: BTreeMap<String, ClientSecret> = ["praha-mesto", "broken"]
        .into_iter()
        .map(|name| (name.to_owned(), ClientSecret::from(format!("s-{name}"))))
        .collect();
    let (apps, skipped) = edge_apps(&mirror, &secrets, &pod_settings(None), None);
    let names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
    assert_eq!(names, ["praha-mesto"]);
    assert_eq!(apps[0].slugs, ["k4y7pq2mzt6vhx3nbwrs5cjd8f"]);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].0, "broken");
    assert!(
        skipped[0].1.starts_with("the App does not parse: "),
        "{skipped:?}"
    );
}

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
