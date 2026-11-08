#![forbid(unsafe_code)]
//! Every MCP tool against a real state (R10, R10a): the tool list mirrors the operation
//! registry, each tool runs the same operation and authorizer decision as OData and the CLI,
//! a missing or malformed argument is a 400 (never an empty uid or panel 1), an unknown tool
//! a 404, and a viewer sees no more through MCP than through the HTTP API.

use axum::Router;
use axum::body::Body;
use daiya_api::{DaiyaState, TokenBinding, api_router};
use daiya_mcp::{call_mcp_tool, list_mcp_tools};
use daiya_model::{
    GrafanaRole, InstallationId, InstanceId, OrgId, PolicyAuthorizer, RequestCtx, RoleBinding,
    Subject,
};
use daiya_query::OutboundPolicy;
use daiya_store_memory::MemoryStore;
use http::Request;
use http_body_util::BodyExt;
use jc_router::Mount;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const ORG: OrgId = OrgId::from_u128(91);

async fn prometheus_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = if head.contains("/api/v1/labels") {
                json!({ "status": "success", "data": ["job", "instance"] })
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
        allowed_networks: vec!["127.0.0.0/8".parse().unwrap()],
        ..OutboundPolicy::default()
    };
    let app = api_router(
        state.clone(),
        Mount::new("daiya", "https", "x.example", "").unwrap(),
    );
    let (st, body) = http(
        &app,
        "POST",
        "/api/datasources",
        json!({ "uid": "prom", "name": "Prom", "type": "prometheus", "url": url }),
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
        state,
        app,
        admin: ctx(Subject::authenticated("admin", vec![ORG], vec![])),
        viewer: ctx(Subject::authenticated("viewer", vec![ORG], vec![])),
        stranger: ctx(Subject::authenticated("stranger", vec![ORG], vec![])),
    }
}

#[tokio::test]
async fn the_tool_list_mirrors_the_registry_with_required_parameters() {
    let f = fixture().await;
    let tools = list_mcp_tools(&f.state);
    assert_eq!(tools.len(), f.state.registry.operations.len() + 1);
    assert_eq!(tools[0].name, "daiya_list_dashboards");
    for op in &f.state.registry.operations {
        let tool = tools.iter().find(|t| t.name == op.mcp_tool).unwrap();
        assert_eq!(tool.input_schema["type"], "object");
        for p in &op.params {
            assert_eq!(
                tool.input_schema["properties"][&p.name]["type"], p.json_type,
                "{} {}",
                op.mcp_tool, p.name
            );
            let required = tool.input_schema["required"].as_array().unwrap();
            assert_eq!(
                required.contains(&Value::String(p.name.clone())),
                p.required,
                "{} {}",
                op.mcp_tool,
                p.name
            );
        }
    }
}

#[tokio::test]
async fn list_dashboards_filters_by_title_and_hides_what_the_caller_cannot_read() {
    let f = fixture().await;
    let all = call_mcp_tool(&f.state, &f.admin, "daiya_list_dashboards", json!({}))
        .await
        .unwrap();
    assert_eq!(all["value"].as_array().unwrap().len(), 2, "{all}");
    let filtered = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_list_dashboards",
        json!({ "query": "TRAFFIC" }),
    )
    .await
    .unwrap();
    assert_eq!(filtered["value"].as_array().unwrap().len(), 1, "{filtered}");
    assert_eq!(filtered["value"][0]["uid"], "web");
    assert_eq!(filtered["value"][0]["panelCount"], 1);
    let none = call_mcp_tool(&f.state, &f.stranger, "daiya_list_dashboards", json!({}))
        .await
        .unwrap();
    assert_eq!(none["value"], json!([]), "{none}");
}

#[tokio::test]
async fn an_unknown_tool_is_404() {
    let f = fixture().await;
    let err = call_mcp_tool(&f.state, &f.admin, "daiya_drop_tables", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

#[tokio::test]
async fn datasource_tools_query_health_schema_and_resources() {
    let f = fixture().await;
    let answer = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_datasource_query",
        json!({
            "from": "1700000000000", "to": "1700000600000",
            "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
        }),
    )
    .await
    .unwrap();
    assert!(answer.to_string().contains('7'), "{answer}");

    let err = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_datasource_query",
        json!({ "queries": "not-a-list" }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 400);

    let health = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_datasource_health",
        json!({ "uid": "prom" }),
    )
    .await
    .unwrap();
    assert_eq!(health["status"], "ok", "{health}");

    let schema = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_datasource_editor_schema",
        json!({ "uid": "prom" }),
    )
    .await
    .unwrap();
    assert!(schema.is_object(), "{schema}");

    let labels = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_datasource_resource",
        json!({ "uid": "prom", "path": "api/v1/labels" }),
    )
    .await
    .unwrap();
    assert!(labels.to_string().contains("instance"), "{labels}");

    // A datasource of the organization the caller cannot see is a 404, like over HTTP.
    let err = call_mcp_tool(
        &f.state,
        &f.stranger,
        "daiya_datasource_health",
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
        ("daiya_datasource_health", json!({})),
        ("daiya_datasource_editor_schema", json!({ "uid": 7 })),
        ("daiya_datasource_resource", json!({ "uid": "prom" })),
        ("daiya_export_dashboard", json!({})),
        ("daiya_render_panel", json!({ "uid": "web" })),
        ("daiya_render_panel", json!({ "uid": "web", "panelId": -1 })),
        (
            "daiya_render_panel",
            json!({ "uid": "web", "panelId": 4_294_967_297_u64 }),
        ),
        ("daiya_rotate_public_dashboard", json!({})),
        ("daiya_public_dashboard_query", json!({ "panelId": 1 })),
        (
            "daiya_public_dashboard_query",
            json!({ "accessToken": "t" }),
        ),
        ("daiya_allowed", json!({})),
        ("daiya_import", json!({ "dryRun": "yes" })),
        ("daiya_plan", json!({ "bundle": { "folders": 3 } })),
        ("daiya_apply", json!({ "plan": { "orgId": "x" } })),
    ];
    for (tool, args) in cases {
        let tool_name = f
            .state
            .registry
            .operations
            .iter()
            .find(|o| o.mcp_tool == tool)
            .map(|o| o.mcp_tool.clone())
            .unwrap_or_else(|| panic!("no tool {tool}"));
        let err = call_mcp_tool(&f.state, &f.admin, &tool_name, args.clone())
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), 400, "{tool} {args}: {err}");
    }
}

#[tokio::test]
async fn render_panel_and_export_run_the_dashboard_operations() {
    let f = fixture().await;
    let view = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_render_panel",
        json!({ "uid": "web", "panelId": 1, "from": "now-6h", "to": "now" }),
    )
    .await
    .unwrap();
    assert_eq!(view["panelTitle"], "Web", "{view}");

    let exported = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_export_dashboard",
        json!({ "uid": "db" }),
    )
    .await
    .unwrap();
    assert_eq!(exported["uid"], "db", "{exported}");
    assert_eq!(exported["schemaVersion"], 42);

    let err = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_render_panel",
        json!({ "uid": "web", "panelId": 99 }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404);
}

#[tokio::test]
async fn import_plan_and_apply_write_only_for_an_editor() {
    let f = fixture().await;
    let dry = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_import",
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

    let err = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_import",
        json!({ "dashboard": dashboard("new", "New one") }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err.status_code(), 403 | 404), "{err}");

    let plan = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_plan",
        json!({ "bundle": { "dashboards": [{ "rawJson": dashboard("planned", "Planned") }] } }),
    )
    .await
    .unwrap();
    assert_eq!(plan["hasChanges"], true, "{plan}");
    // The bundle may also be given as the arguments themselves.
    let same = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_plan",
        json!({ "dashboards": [{ "rawJson": dashboard("planned", "Planned") }] }),
    )
    .await
    .unwrap();
    assert_eq!(same["baseEtag"], plan["baseEtag"]);

    let applied = call_mcp_tool(&f.state, &f.admin, "daiya_apply", json!({ "plan": plan }))
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
    let err = call_mcp_tool(&f.state, &f.admin, "daiya_apply", plan)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 409, "{err}");
}

#[tokio::test]
async fn public_dashboard_tools_rotate_query_and_decide() {
    let f = fixture().await;
    let (st, created) = http(
        &f.app,
        "POST",
        "/api/dashboards/uid/web/public-dashboards",
        json!({ "isEnabled": true }),
    )
    .await;
    assert_eq!(st, 200, "{created}");
    let old_token = created["accessToken"].as_str().unwrap().to_owned();

    let answer = call_mcp_tool(
        &f.state,
        &f.stranger,
        "daiya_public_dashboard_query",
        json!({ "accessToken": old_token, "panelId": 1 }),
    )
    .await
    .unwrap();
    assert!(answer["value"].to_string().contains('7'), "{answer}");

    let rotated = call_mcp_tool(
        &f.state,
        &f.admin,
        "daiya_rotate_public_dashboard",
        json!({ "uid": "web" }),
    )
    .await
    .unwrap();
    let new_token = rotated["accessToken"].as_str().unwrap();
    assert_ne!(new_token, old_token);
    let err = call_mcp_tool(
        &f.state,
        &f.stranger,
        "daiya_public_dashboard_query",
        json!({ "accessToken": old_token, "panelId": 1 }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404, "a rotated token is gone");

    // A viewer may not rotate.
    let err = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_rotate_public_dashboard",
        json!({ "uid": "web" }),
    )
    .await
    .unwrap_err();
    assert!(matches!(err.status_code(), 403 | 404), "{err}");

    let yes = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_allowed",
        json!({ "operation": "export", "object": "Dashboard:web" }),
    )
    .await
    .unwrap();
    assert_eq!(yes["allowed"], true, "{yes}");
    let no = call_mcp_tool(
        &f.state,
        &f.viewer,
        "daiya_allowed",
        json!({ "operation": "apply" }),
    )
    .await
    .unwrap();
    assert_eq!(no["allowed"], false, "{no}");
}
