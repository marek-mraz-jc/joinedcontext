#![forbid(unsafe_code)]
//! Every CLI operation against a real state (R10a): each runs the operation and authorizer
//! decision OData runs, a missing or malformed argument is a 400 (never an empty uid or panel
//! 1), an unknown command a 404, and a viewer sees no more than through the HTTP API.

use axum::Router;
use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_model::{
    GrafanaRole, InstallationId, InstanceId, OrgId, PolicyAuthorizer, RequestCtx, RoleBinding,
    Subject,
};
use daiya_query::{OpenedNetwork, OutboundPolicy};
use daiya_server::execute_cli_command;
use daiya_store_memory::MemoryStore;
use http::Request;
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(91);

/// Every request line the Prometheus mock answered, so a test can read what was asked.
static ASKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

async fn prometheus_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            if let Some(line) = head.lines().next() {
                ASKED.lock().unwrap().push(line.to_owned());
            }
            let body = if head.contains("/api/v1/labels") {
                json!({ "status": "success", "data": ["job", "instance"] })
            } else if head.contains("/api/endpoint/site/") {
                json!([{ "id": "urn:ngsi-ld:Site:web", "type": "Site",
                         "visitors": { "type": "Property", "value": 7 } }])
            } else {
                json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                    { "metric": { "job": "web" }, "values": [[1700000000, "7"]] }
                ]}})
            };
            let body = body.to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

fn dashboard(uid: &str, title: &str) -> Value {
    json!({
        "uid": uid,
        "title": title,
        "schemaVersion": 42,
        "time": { "from": "now-1h", "to": "now" },
        "panels": [{
            "id": 1, "type": "timeseries", "title": "Web",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prom" },
            "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": "prom" } }]
        }]
    })
}

struct Fixture {
    url: String,
    state: DaiyaState,
    app: Router,
    admin: RequestCtx,
    viewer: RequestCtx,
    stranger: RequestCtx,
}

fn ctx(subject: Subject) -> RequestCtx {
    RequestCtx::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        subject,
    )
}

async fn http(app: &Router, method: &str, uri: &str, body: Value) -> (u16, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", "Bearer admin-token")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn fixture() -> Fixture {
    let url = prometheus_mock().await;
    let authz = Arc::new(PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(ORG, "admin", GrafanaRole::Admin, None),
        RoleBinding::from_grafana_role(ORG, "viewer", GrafanaRole::Viewer, None),
    ]));
    let mut state = DaiyaState::new(
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
        Arc::new(MemoryStore::new()),
        authz,
    )
    .with_tokens(vec![TokenBinding::new(
        "admin-token",
        Subject::authenticated("admin", vec![ORG], vec![]),
    )]);
    state.outbound_policy = OutboundPolicy {
        allow_http: true,
        allowed_networks: vec![OpenedNetwork::new(
            "127.0.0.0/8".parse().unwrap(),
            ORG,
            &["prometheus", "ngsild", "odata", "postgres"],
        )],
        ..OutboundPolicy::default()
    };
    state.own_reach =
        vec![daiya_query::OwnReach::endpoints(&format!("{url}/api/endpoint/")).unwrap()];
    let app = api_router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    let (st, body) = http(
        &app,
        "POST",
        "/api/datasources",
        json!({ "uid": "prom", "name": "Prom", "type": "prometheus", "url": url.clone() }),
    )
    .await;
    assert_eq!(st, 200, "{body}");
    for (uid, title) in [("web", "Web traffic"), ("db", "Database")] {
        let (st, body) = http(
            &app,
            "POST",
            "/api/dashboards/db",
            json!({ "dashboard": dashboard(uid, title), "overwrite": true }),
        )
        .await;
        assert_eq!(st, 200, "{body}");
    }
    Fixture {
        url,
        state,
        app,
        admin: ctx(Subject::authenticated("admin", vec![ORG], vec![])),
        viewer: ctx(Subject::authenticated("viewer", vec![ORG], vec![])),
        stranger: ctx(Subject::authenticated("stranger", vec![ORG], vec![])),
    }
}

#[tokio::test]
async fn an_unknown_command_is_404() {
    let f = fixture().await;
    let err = execute_cli_command(&f.state, &f.admin, "drop-tables", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

#[tokio::test]
async fn a_registry_operation_the_dispatcher_does_not_wire_is_501_not_a_guess() {
    let mut f = fixture().await;
    let mut registry = (*f.state.registry).clone();
    let mut op = registry.operations[0].clone();
    op.name = "Dashboard/Archive".into();
    op.cli_command = "dashboard-archive".into();
    registry.operations.push(op);
    f.state.registry = Arc::new(registry);
    let err = execute_cli_command(&f.state, &f.admin, "dashboard-archive", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 501, "{err}");
    assert!(err.to_string().contains("Dashboard/Archive"), "{err}");
}

#[tokio::test]
async fn datasource_commands_query_health_schema_and_resources() {
    let f = fixture().await;
    let answer = execute_cli_command(
        &f.state,
        &f.viewer,
        "query",
        json!({
            "from": "1700000000000", "to": "1700000600000",
            "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
        }),
    )
    .await
    .unwrap();
    assert!(answer.to_string().contains('7'), "{answer}");

    let err = execute_cli_command(
        &f.state,
        &f.admin,
        "query",
        json!({ "queries": "not-a-list" }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 400);

    let health = execute_cli_command(
        &f.state,
        &f.admin,
        "datasource-health",
        json!({ "uid": "prom" }),
    )
    .await
    .unwrap();
    assert_eq!(health["status"], "ok", "{health}");

    let schema = execute_cli_command(
        &f.state,
        &f.admin,
        "editor-schema",
        json!({ "uid": "prom" }),
    )
    .await
    .unwrap();
    assert!(schema.is_object(), "{schema}");

    let labels = execute_cli_command(
        &f.state,
        &f.admin,
        "datasource-resource",
        json!({ "uid": "prom", "path": "api/v1/labels", "params": { "match[]": "up" } }),
    )
    .await
    .unwrap();
    assert!(labels.to_string().contains("instance"), "{labels}");

    // A datasource of the organization the caller cannot see is a 404, like over HTTP.
    let err = execute_cli_command(
        &f.state,
        &f.stranger,
        "datasource-health",
        json!({ "uid": "prom" }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

#[tokio::test]
async fn missing_or_malformed_arguments_are_400_not_defaults() {
    let f = fixture().await;
    let cases = [
        ("datasource-health", json!({})),
        ("editor-schema", json!({ "uid": 7 })),
        ("datasource-resource", json!({ "uid": "prom" })),
        ("export", json!({})),
        ("render-panel", json!({ "uid": "web" })),
        ("render-panel", json!({ "uid": "web", "panelId": -1 })),
        (
            "render-panel",
            json!({ "uid": "web", "panelId": 4_294_967_297_u64 }),
        ),
        ("rotate-public-token", json!({})),
        ("public-query", json!({ "panelId": 1 })),
        ("public-query", json!({ "accessToken": "t" })),
        ("allowed", json!({})),
        ("import", json!({ "dryRun": "yes" })),
        ("plan", json!({ "bundle": { "folders": 3 } })),
        ("apply", json!({ "plan": { "orgId": "x" } })),
    ];
    for (command, args) in cases {
        let err = execute_cli_command(&f.state, &f.admin, command, args.clone())
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), 400, "{command} {args}: {err}");
    }
}

#[tokio::test]
async fn render_panel_and_export_run_the_dashboard_operations() {
    let f = fixture().await;
    let view = execute_cli_command(
        &f.state,
        &f.viewer,
        "render-panel",
        json!({ "uid": "web", "panelId": 1, "from": "now-6h", "to": "now" }),
    )
    .await
    .unwrap();
    assert_eq!(view["panelTitle"], "Web", "{view}");

    // The panel's range is the one asked for (epoch ms 2010-01-01 to 2010-01-02), not the dashboard's.
    execute_cli_command(
        &f.state,
        &f.viewer,
        "render-panel",
        json!({ "uid": "web", "panelId": 1, "from": "1262304000000", "to": "1262390400000" }),
    )
    .await
    .unwrap();
    let asked = ASKED.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .any(|l| l.contains("start=1262304000.000") && l.contains("end=1262390400.000")),
        "{asked:#?}"
    );

    let exported = execute_cli_command(&f.state, &f.viewer, "export", json!({ "uid": "db" }))
        .await
        .unwrap();
    assert_eq!(exported["uid"], "db", "{exported}");
    assert_eq!(exported["schemaVersion"], 42);

    let err = execute_cli_command(
        &f.state,
        &f.viewer,
        "render-panel",
        json!({ "uid": "web", "panelId": 99 }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

#[tokio::test]
async fn import_plan_and_apply_write_only_for_an_editor() {
    let f = fixture().await;
    let dry = execute_cli_command(
        &f.state,
        &f.admin,
        "import",
        json!({ "dryRun": true, "dashboard": dashboard("new", "New one") }),
    )
    .await
    .unwrap();
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert!(
        f.state
            .store
            .get_dashboard(ORG, "new")
            .await
            .unwrap()
            .is_none()
    );

    let err = execute_cli_command(
        &f.state,
        &f.viewer,
        "import",
        json!({ "dashboard": dashboard("new", "New one") }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err.status_code(), 403 | 404), "{err}");

    let plan = execute_cli_command(
        &f.state,
        &f.admin,
        "plan",
        json!({ "bundle": { "dashboards": [{ "rawJson": dashboard("planned", "Planned") }] } }),
    )
    .await
    .unwrap();
    assert_eq!(plan["hasChanges"], true, "{plan}");
    // The bundle may also be given as the arguments themselves.
    let same = execute_cli_command(
        &f.state,
        &f.admin,
        "plan",
        json!({ "dashboards": [{ "rawJson": dashboard("planned", "Planned") }] }),
    )
    .await
    .unwrap();
    assert_eq!(same["baseEtag"], plan["baseEtag"]);

    // Apply takes only the digest of a plan the server kept; a bundle in the arguments is refused.
    for args in [
        json!({ "plan": plan.clone() }),
        json!({ "digest": plan["digest"], "desired": plan["desired"] }),
    ] {
        let err = execute_cli_command(&f.state, &f.admin, "apply", args)
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), 400, "{err}");
    }
    let by_digest = json!({ "digest": plan["digest"] });
    let applied = execute_cli_command(&f.state, &f.admin, "apply", by_digest.clone())
        .await
        .unwrap();
    assert_eq!(applied["dashboards"][0]["uid"], "planned", "{applied}");
    assert!(
        f.state
            .store
            .get_dashboard(ORG, "planned")
            .await
            .unwrap()
            .is_some()
    );
    // The same plan again is stale: the state it was computed on is gone.
    let err = execute_cli_command(&f.state, &f.admin, "apply", by_digest)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 409, "{err}");
}

#[tokio::test]
async fn public_dashboard_commands_rotate_query_and_decide() {
    let f = fixture().await;
    // A public dashboard reads only an Endpoint (WRK-04).
    let source = json!({ "type": "ngsild-grafana-datasource", "uid": "site" });
    let mut board = dashboard("published", "Published");
    board["panels"][0]["datasource"] = source.clone();
    board["panels"][0]["targets"] = json!([{ "refId": "A", "queryType": "entity",
        "entityType": "Site", "datasource": source }]);
    for (uri, body) in [
        (
            "/api/datasources",
            json!({ "uid": "site", "name": "Site", "type": "ngsild-grafana-datasource",
                    "url": format!("{}/api/endpoint/site", f.url) }),
        ),
        (
            "/api/dashboards/db",
            json!({ "dashboard": board, "overwrite": true }),
        ),
    ] {
        let (st, saved) = http(&f.app, "POST", uri, body).await;
        assert_eq!(st, 200, "{saved}");
    }
    let (st, created) = http(
        &f.app,
        "POST",
        "/api/dashboards/uid/published/public-dashboards",
        json!({ "isEnabled": true }),
    )
    .await;
    assert_eq!(st, 200, "{created}");
    let old_token = created["accessToken"].as_str().unwrap().to_owned();

    let answer = execute_cli_command(
        &f.state,
        &f.stranger,
        "public-query",
        json!({ "accessToken": old_token, "panelId": 1 }),
    )
    .await
    .unwrap();
    assert!(answer["value"].to_string().contains('7'), "{answer}");

    let rotated = execute_cli_command(
        &f.state,
        &f.admin,
        "rotate-public-token",
        json!({ "uid": "published" }),
    )
    .await
    .unwrap();
    let new_token = rotated["accessToken"].as_str().unwrap();
    assert_ne!(new_token, old_token);
    let err = execute_cli_command(
        &f.state,
        &f.stranger,
        "public-query",
        json!({ "accessToken": old_token, "panelId": 1 }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404, "a rotated token is gone");

    // A viewer may not rotate.
    let err = execute_cli_command(
        &f.state,
        &f.viewer,
        "rotate-public-token",
        json!({ "uid": "published" }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err.status_code(), 403 | 404), "{err}");

    let yes = execute_cli_command(
        &f.state,
        &f.viewer,
        "allowed",
        json!({ "operation": "export", "object": "Dashboard:web" }),
    )
    .await
    .unwrap();
    assert_eq!(yes["allowed"], true, "{yes}");
    let no = execute_cli_command(
        &f.state,
        &f.viewer,
        "allowed",
        json!({ "operation": "apply" }),
    )
    .await
    .unwrap();
    assert_eq!(no["allowed"], false, "{no}");
}
