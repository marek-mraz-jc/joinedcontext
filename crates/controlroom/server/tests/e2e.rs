#![forbid(unsafe_code)]
//! End-to-end integration tests for `controlroom` (R1–R12):
//! 1. Import our 9 dashboards + OData 4.01 `$metadata`, `$filter`, `$select`, `$orderby`, `$top`, `$count`, `$batch`.
//! 2. Query mocked Prometheus and mocked NGSI-LD Endpoint (verifying caller token exchange for Endpoint audience) + `panel_view` / `RenderPanel`.
//! 3. Mixed panel (`-- Mixed --`) with Prometheus + NGSI-LD + `math` expression + `reduce` expression.
//! 4. OData, MCP, and CLI parity (`Allowed` and actions give the exact same answer and decision) + `jc-router` mount prefix, cookies, and CSP.
//! 5. Public dashboard rules of R7 (opt-in off by default, 256-bit hashed token, forged target/variable refusal, range cap, cache isolation, metadata stripping, token rotation 404, per-IP rate limit).
//! 6. Cross-organization 404s (never 403) and zero outbound calls on cross-org datasource query.
//! 7. Regression tests for the §4.3 / R12 CVE classes (datasource proxy credential leak, SSRF & redirect block, stored XSS in titles/markdown, public dashboard data leak, provisioning path traversal).

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::Query as AxumQuery;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::get;
use chrono::Utc;
use controlroom_api::state::TokenBinding;
use controlroom_api::{PanelTimeRange, panel_view};
use controlroom_import::parse_dashboard_providers_yaml;
use controlroom_mcp::call_mcp_tool;
use controlroom_model::{
    ControlroomStore, DatasourceRecord, GrafanaRole, InstallationId, InstanceId, OrgId,
    PolicyAuthorizer, RequestCtx, RoleBinding, Subject,
};
use controlroom_query::{
    GuardConfig, MapSecretResolver, NgsiLdDatasource, OutboundGuard, PassthroughAudienceExchanger,
    PrometheusDatasource,
};
use controlroom_server::{ControlroomState, execute_cli_command, router};
use controlroom_store_memory::MemoryStore;
use http_body_util::BodyExt;
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

const NINE_DASHBOARDS: &[(&str, &str)] = &[
    (
        "assistant.json",
        include_str!("../../import/tests/fixtures/assistant.json"),
    ),
    (
        "context-broker.json",
        include_str!("../../import/tests/fixtures/context-broker.json"),
    ),
    (
        "context-gateway.json",
        include_str!("../../import/tests/fixtures/context-gateway.json"),
    ),
    (
        "edge-apisix.json",
        include_str!("../../import/tests/fixtures/edge-apisix.json"),
    ),
    (
        "k6-load.json",
        include_str!("../../import/tests/fixtures/k6-load.json"),
    ),
    (
        "portal.json",
        include_str!("../../import/tests/fixtures/portal.json"),
    ),
    (
        "slo.json",
        include_str!("../../import/tests/fixtures/slo.json"),
    ),
    (
        "bbsk.json",
        include_str!("../../import/tests/fixtures/bbsk.json"),
    ),
    (
        "helsinki.json",
        include_str!("../../import/tests/fixtures/helsinki.json"),
    ),
];

/// Spawns a local mock HTTP server for Prometheus and NGSI-LD endpoints, returning `(base_url, call_counter, last_auth_header)`.
async fn spawn_mock_upstream() -> (
    String,
    Arc<AtomicUsize>,
    Arc<parking_lot::Mutex<Option<String>>>,
) {
    let call_count = Arc::new(AtomicUsize::new(0));
    let last_auth = Arc::new(parking_lot::Mutex::new(None));

    let cc_prom = call_count.clone();
    let auth_prom = last_auth.clone();
    let cc_ngsi = call_count.clone();
    let auth_ngsi = last_auth.clone();

    let app = Router::new()
        .route(
            "/api/v1/query_range",
            get(
                move |headers: HeaderMap, AxumQuery(_q): AxumQuery<BTreeMap<String, String>>| {
                    let cc = cc_prom.clone();
                    let la = auth_prom.clone();
                    async move {
                        cc.fetch_add(1, Ordering::SeqCst);
                        if let Some(a) = headers.get("authorization").and_then(|v| v.to_str().ok())
                        {
                            *la.lock() = Some(a.to_owned());
                        }
                        Json(json!({
                            "status": "success",
                            "data": {
                                "resultType": "matrix",
                                "result": [
                                    {
                                        "metric": { "__name__": "up", "status": "200", "kind": "chat" },
                                        "values": [
                                            [1700000000.0, "10"],
                                            [1700000060.0, "20"]
                                        ]
                                    }
                                ]
                            }
                        }))
                    }
                },
            ),
        )
        .route(
            "/api/v1/query",
            get(|_headers: HeaderMap| async move {
                Json(json!({
                    "status": "success",
                    "data": {
                        "resultType": "vector",
                        "result": [
                            {
                                "metric": { "target": "portal", "surface": "web" },
                                "value": [1700000060.0, "0.999"]
                            }
                        ]
                    }
                }))
            }),
        )
        .route(
            "/api/endpoint/{slug}/ngsi-ld/v1/entities",
            get(move |headers: HeaderMap| {
                let cc = cc_ngsi.clone();
                let la = auth_ngsi.clone();
                async move {
                    cc.fetch_add(1, Ordering::SeqCst);
                    if let Some(a) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
                        *la.lock() = Some(a.to_owned());
                    }
                    Json(json!([
                        {
                            "id": "urn:ngsi-ld:AirQualityObserved:station-1",
                            "type": "AirQualityObserved",
                            "name": { "type": "Property", "value": "Kallio" },
                            "pm25": { "type": "Property", "value": 7.5, "unitCode": "GQ", "observedAt": "2026-11-01T12:00:00Z" },
                            "pm10": { "type": "Property", "value": 16.2, "unitCode": "GQ", "observedAt": "2026-11-01T12:00:00Z" },
                            "title": { "type": "Property", "value": "Kraj, Banská Bystrica" },
                            "currentValue": { "type": "Property", "value": 110000 }
                        }
                    ]))
                }
            }),
        )
        .route(
            "/api/endpoint/{slug}/ngsi-ld/v1/temporal/entities",
            get(|headers: HeaderMap| async move {
                let _ = headers;
                Json(json!([
                    {
                        "id": "urn:ngsi-ld:TrafficFlowObserved:eco-1",
                        "type": "TrafficFlowObserved",
                        "name": { "type": "Property", "value": "Baana" },
                        "intensity": {
                            "type": "Property",
                            "values": [
                                [15.0, "2023-11-14T22:13:20Z"],
                                [25.0, "2023-11-14T22:14:20Z"]
                            ]
                        }
                    }
                ]))
            }),
        )
        .route(
            "/redirect-ssrf",
            get(|| async move {
                (
                    StatusCode::FOUND,
                    [("Location", "http://169.254.169.254/latest/meta-data/")],
                    "",
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), call_count, last_auth)
}

async fn send_req(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json_val = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, headers, json_val)
}

async fn setup_world(upstream_url: &str) -> (ControlroomState, OrgId, OrgId, Router) {
    let inst = InstallationId::from_u128(1);
    let instance = InstanceId::from_u128(1);
    let org_a = OrgId::from_u128(10);
    let org_b = OrgId::from_u128(20);
    let store = Arc::new(MemoryStore::new());

    let authz = Arc::new(PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(org_a, "admin", GrafanaRole::Admin, None),
        RoleBinding::from_grafana_role(org_a, "viewer-a", GrafanaRole::Viewer, None),
        RoleBinding::from_grafana_role(org_b, "admin-b", GrafanaRole::Admin, None),
        RoleBinding::from_grafana_role(
            org_a,
            "folder-viewer",
            GrafanaRole::Viewer,
            Some("ops".into()),
        ),
    ]));

    let mut secrets = MapSecretResolver::new();
    secrets.insert(org_a, "prom-token-ref", "super-secret-prom-credential-xyz");
    let secrets = Arc::new(secrets);

    let mut state =
        ControlroomState::new(inst, org_a, instance, store.clone(), authz).with_tokens(vec![
            TokenBinding::new(
                "admin-a-token",
                Subject::authenticated("admin", vec![org_a], vec![]),
            ),
            TokenBinding::new(
                "user-alice-jwt",
                Subject::authenticated("admin", vec![org_a], vec![]),
            ),
            TokenBinding::new(
                "caller-jwt",
                Subject::authenticated("admin", vec![org_a], vec![]),
            ),
            TokenBinding::new(
                "viewer-a-token",
                Subject::authenticated("viewer-a", vec![org_a], vec![]),
            ),
            TokenBinding::new(
                "admin-b-token",
                Subject::authenticated("admin-b", vec![org_b], vec![]),
            ),
            TokenBinding::new(
                "folder-viewer-token",
                Subject::authenticated("folder-viewer", vec![org_a], vec![]),
            ),
        ]);
    state.secret_resolver = secrets.clone();
    // The NGSI-LD drivers below carry their own exchanger; the state's default (deny) stays.

    let host_only = url::Url::parse(upstream_url)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned();
    let guard = OutboundGuard::new(GuardConfig {
        allow_hosts: vec![host_only.clone()],
        allow_http: true,
        allow_loopback_for_tests: true,
        ..GuardConfig::default()
    });

    let now = Utc::now();

    // Register Prometheus + NGSI-LD datasources in org_a
    let mut prom_secrets = BTreeMap::new();
    prom_secrets.insert("token".into(), "prom-token-ref".into());
    store
        .upsert_datasource(DatasourceRecord {
            installation_id: inst,
            org_id: org_a,
            instance_id: instance,
            project_id: None,
            uid: "prometheus".into(),
            name: "Prometheus".into(),
            kind: "prometheus".into(),
            access: "proxy".into(),
            url: upstream_url.to_owned(),
            is_default: true,
            read_only: true,
            json_data: json!({"timeInterval": "15s"}),
            secret_refs: prom_secrets,
            allow_hosts: vec![host_only.clone()],
            wasm_digest: None,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let prom_driver = Arc::new(
        PrometheusDatasource::new("prometheus", upstream_url, guard.clone(), secrets.clone())
            .with_secret_ref("prom-token-ref"),
    );
    state.register_datasource_driver(org_a, "prometheus", prom_driver);

    for (uid, slug) in [
        ("bbsk-kpi", "qfhhh5no5wz4lk3rfjisdtx3chiyfig3"),
        ("helsinki-cycle-counts", "kvhnixtbe5dex2wt2kx3i37dzw6yzsxt"),
        ("helsinki-air-quality", "jtjnhkaau64zmzz5csmcwem52icvufy2"),
        ("helsinki-bikes-ops", "ydp6nddchg4sewmylsiclg4377mlldrj"),
    ] {
        let ep_url = format!("{upstream_url}/api/endpoint/{slug}");
        store
            .upsert_datasource(DatasourceRecord {
                installation_id: inst,
                org_id: org_a,
                instance_id: instance,
                project_id: None,
                uid: uid.into(),
                name: uid.into(),
                kind: "ngsild-grafana-datasource".into(),
                access: "proxy".into(),
                url: ep_url.clone(),
                is_default: false,
                read_only: false,
                json_data: json!({"oauthPassThru": true, "pageSize": 1000}),
                secret_refs: BTreeMap::new(),
                allow_hosts: vec![host_only.clone()],
                wasm_digest: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let ngsi_driver = Arc::new(NgsiLdDatasource::new(
            uid,
            ep_url,
            true,
            guard.clone(),
            Arc::new(PassthroughAudienceExchanger),
        ));
        state.register_datasource_driver(org_a, uid, ngsi_driver);
    }

    // Register a datasource in org_b (`b-private-ds`)
    store
        .upsert_datasource(DatasourceRecord {
            installation_id: inst,
            org_id: org_b,
            instance_id: instance,
            project_id: None,
            uid: "b-private-ds".into(),
            name: "B Private Prom".into(),
            kind: "prometheus".into(),
            access: "proxy".into(),
            url: upstream_url.to_owned(),
            is_default: true,
            read_only: false,
            json_data: json!({}),
            secret_refs: BTreeMap::new(),
            allow_hosts: vec![host_only],
            wasm_digest: None,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let mount = Mount::new("controlroom", "https", "city.example", "/controlroom").unwrap();
    let app = router(state.clone(), mount);
    (state, org_a, org_b, app)
}

#[tokio::test]
async fn e2e_import_9_dashboards_odata_service_and_mocked_prometheus_and_ngsild_queries() {
    let (upstream_url, _call_count, last_auth) = spawn_mock_upstream().await;
    let (state, org_a, _org_b, app) = setup_world(&upstream_url).await;

    // 1. Check OData $metadata lists all 8 entity sets and all R10a operations
    let (status, headers, meta_val) = send_req(
        &app,
        Request::builder()
            .uri("/odata/v4/$metadata")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'self'")
    );
    let meta_xml = meta_val.as_str().expect("xml string");
    for required in [
        "Dashboards",
        "DashboardVersions",
        "Folders",
        "Datasources",
        "LibraryPanels",
        "PublicDashboards",
        "QueryCacheStats",
        "ImportReports",
        "Datasource/Query",
        "Datasource/Health",
        "Datasource/EditorSchema",
        "Datasource/Resource",
        "Dashboard/RenderPanel",
        "Dashboard/Export",
        "Import",
        "Plan",
        "Apply",
        "PublicDashboard/Rotate",
        "PublicDashboard/Query",
        "Allowed",
    ] {
        assert!(meta_xml.contains(required), "$metadata missing {required}");
    }

    // 2. Import all 9 dashboards through OData `POST /odata/v4/Import`
    for (filename, content) in NINE_DASHBOARDS {
        let dash_json: Value = serde_json::from_str(content).expect(filename);
        let (st, _, body) = send_req(
            &app,
            Request::builder()
                .method("POST")
                .uri("/odata/v4/Import")
                .header("authorization", "Bearer admin-a-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "dryRun": false,
                        "dashboard": dash_json,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "failed importing {filename}: {body}");
        assert_eq!(body["refusedCount"], 0);
    }

    // 3. Query OData `Dashboards` entity set with $filter, $orderby, $select, $count
    let (st, _, list_body) = send_req(
        &app,
        Request::builder()
            .uri("/odata/v4/Dashboards?$count=true&$orderby=uid&$select=uid,title,panelCount")
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list_body["@odata.count"], 9);
    assert_eq!(list_body["value"].as_array().unwrap().len(), 9);
    assert_eq!(list_body["value"][0]["uid"], "jc-assistant");
    assert_eq!(list_body["value"][0]["panelCount"], 6);

    // 4. Render panel 1 of `jc-assistant` (Prometheus) via `GET /odata/v4/Dashboards('jc-assistant')/RenderPanel?panelId=1`
    let (st, _, pv_prom) = send_req(
        &app,
        Request::builder()
            .uri("/odata/v4/Dashboards('jc-assistant')/RenderPanel?panelId=1")
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pv_prom["dashboardUid"], "jc-assistant");
    assert_eq!(pv_prom["panelId"], 1);
    assert_eq!(pv_prom["frameSets"].as_array().unwrap().len(), 2); // refId A and B
    assert!(
        pv_prom["htmlTable"]
            .as_str()
            .unwrap()
            .contains("<table class=\"cr-panel-table\"")
    );

    // 5. Render panel 3 of `project-helsinki` (NGSI-LD entity table + `organize` transformation!)
    // Verify the caller's token is exchanged for the Endpoint's audience (`jtjnhkaau64zmzz5csmcwem52icvufy2`)
    let (st, _, pv_ngsi) = send_req(
        &app,
        Request::builder()
            .uri("/odata/v4/Dashboards('project-helsinki')/RenderPanel?panelId=3")
            .header("authorization", "Bearer user-alice-jwt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        last_auth.lock().as_deref(),
        Some("Bearer exchanged:jtjnhkaau64zmzz5csmcwem52icvufy2:user-alice-jwt"),
        "NGSI-LD datasource must exchange caller's token for Endpoint audience"
    );
    // Verify `organize` transformation renamed `name -> Station`, `pm25 -> PM2.5`, `pm10 -> PM10` and excluded `id`, `type`, `observedAt`
    let fields = pv_ngsi["frameSets"][0]["frames"][0]["fields"]
        .as_array()
        .unwrap();
    let field_names: Vec<&str> = fields.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(field_names, ["Station", "PM2.5", "PM10"]);

    // 6. Verify direct in-process `panel_view` function for Portal & dataroom (R6b)
    let direct_subj =
        Subject::authenticated("admin", vec![org_a], vec![]).with_bearer_token("direct-jwt");
    let direct_view = panel_view(
        &state,
        &direct_subj,
        org_a,
        "project-bbsk",
        1,
        PanelTimeRange::default(),
    )
    .await
    .expect("direct panel_view");
    assert_eq!(direct_view.panel_title, "Obyvateľstvo");
    let bbsk_cols: Vec<&str> = direct_view.frame_sets[0].frames[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(bbsk_cols, ["Územie", "Obyvatelia"]);
}

#[tokio::test]
async fn e2e_mixed_panel_with_math_and_reduce_expressions() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (_state, _org_a, _org_b, app) = setup_world(&upstream_url).await;

    // Mixed query: Prometheus (A) + NGSI-LD temporal (B, hidden) + Math expression C ($A + 5) + Reduce expression D (mean of C, dropNN)
    let query_payload = json!({
        "from": "2023-11-14T22:10:00Z",
        "to": "2023-11-14T22:20:00Z",
        "queries": [
            {
                "refId": "A",
                "datasource": { "type": "prometheus", "uid": "prometheus" },
                "expr": "up",
                "hide": true
            },
            {
                "refId": "B",
                "datasource": { "type": "ngsild-grafana-datasource", "uid": "helsinki-cycle-counts" },
                "queryType": "temporal",
                "entityType": "TrafficFlowObserved",
                "attributes": ["intensity"]
            },
            {
                "refId": "C",
                "datasource": { "type": "__expr__", "uid": "__expr__" },
                "type": "math",
                "expression": "$A + 5"
            },
            {
                "refId": "D",
                "datasource": { "type": "__expr__", "uid": "__expr__" },
                "type": "reduce",
                "expression": "$C",
                "reducer": "mean",
                "settings": { "mode": "dropNN" }
            }
        ]
    });

    let (st, _, res) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/odata/v4/Datasource.Query")
            .header("authorization", "Bearer caller-jwt")
            .header("content-type", "application/json")
            .body(Body::from(query_payload.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "mixed query failed: {res}");
    let sets = res["value"].as_array().unwrap();
    // Hidden refId "A" is excluded; "B", "C", "D" are returned
    let ref_ids: Vec<&str> = sets.iter().map(|s| s["refId"].as_str().unwrap()).collect();
    assert_eq!(ref_ids, ["B", "C", "D"]);

    // A had values [10, 20] -> C ($A + 5) has [15, 25] -> D (mean(C)) has [20.0]
    let d_val = sets[2]["frames"][0]["fields"][0]["values"][0]
        .as_f64()
        .unwrap();
    assert_eq!(d_val, 20.0);
}

#[tokio::test]
async fn e2e_odata_mcp_and_cli_parity_and_jc_router_mount() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (state, org_a, _org_b, app) = setup_world(&upstream_url).await;

    let ctx = RequestCtx::new(
        state.installation_id,
        org_a,
        state.default_instance_id,
        Subject::authenticated("viewer-a", vec![org_a], vec![]),
    );

    // Check `Allowed` for `Apply` as a Viewer via OData, MCP, and CLI: all 3 must return the exact same Decision!
    let (st, _, odata_dec) = send_req(
        &app,
        Request::builder()
            .uri("/odata/v4/Allowed?operation=Apply&object=Organization:default")
            .header("authorization", "Bearer viewer-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let mcp_dec = call_mcp_tool(
        &state,
        &ctx,
        "controlroom_allowed",
        json!({ "operation": "Apply", "object": "Organization:default" }),
    )
    .await
    .unwrap();

    let cli_dec = execute_cli_command(
        &state,
        &ctx,
        "allowed",
        json!({ "operation": "Apply", "object": "Organization:default" }),
    )
    .await
    .unwrap();

    assert_eq!(odata_dec, mcp_dec);
    assert_eq!(mcp_dec, cli_dec);
    assert_eq!(odata_dec["allowed"], false);

    // Mount on `jc-router` under `/portal/controlroom` and verify prefix-aware links and `__Secure-` cookie prefix (R11)
    let mount = Mount::new(
        "controlroom",
        "https",
        "city.example",
        "/portal/controlroom",
    )
    .unwrap();
    let mounted_router = router(state, mount);
    let jc = JcRouterBuilder::new("https", "city.example")
        .mount(
            MountSpec {
                module: "controlroom".into(),
                host: Some("city.example".into()),
                path_prefix: Some("/portal/controlroom".into()),
            },
            mounted_router,
        )
        .build()
        .unwrap();

    let resp = jc
        .dispatch(
            Request::builder()
                .uri("/portal/controlroom/mount-info")
                .header("host", "city.example")
                .body(Body::empty())
                .unwrap(),
            None,
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let info: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        info["publicBase"],
        "https://city.example/portal/controlroom"
    );
    assert_eq!(info["sessionCookieName"], "__Secure-wk_cr_session");
}

#[tokio::test]
async fn e2e_public_dashboard_rules_of_r7() {
    let (upstream_url, call_count, _) = spawn_mock_upstream().await;
    let (_state, _org_a, org_b, app) = setup_world(&upstream_url).await;

    // Import `slo.json` into org_a
    let slo_json: Value = serde_json::from_str(NINE_DASHBOARDS[6].1).unwrap();
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/dashboards/db")
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "dashboard": slo_json, "overwrite": true }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 1. Create public dashboard without specifying isEnabled -> off by default (R7)
    let (st, _, created_default) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/dashboards/uid/jc-slo/public-dashboards")
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(json!({}).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(created_default["isEnabled"], false);
    let token_1 = created_default["accessToken"]
        .as_str()
        .expect("256-bit token")
        .to_owned();
    assert_eq!(token_1.len(), 64); // 256 bits = 64 hex chars

    // Viewing while disabled returns 404
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .uri(format!("/api/public/dashboards/{token_1}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // 2. Enable the public dashboard
    let pub_uid = created_default["uid"].as_str().unwrap().to_owned();
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .method("PATCH")
            .uri(format!(
                "/api/dashboards/uid/jc-slo/public-dashboards/{pub_uid}"
            ))
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "isEnabled": true, "timeSelectionEnabled": true }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 3. View enabled public dashboard: verify query expressions and datasource UIDs are stripped!
    let (st, _, view_body) = send_req(
        &app,
        Request::builder()
            .uri(format!("/api/public/dashboards/{token_1}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let view_str = view_body.to_string();
    assert!(!view_str.contains("jc:probe_availability:ratio30d"));
    assert!(view_body["dashboard"]["panels"][0]["datasource"].is_null());

    // 4. Forged target/query/expr/datasource in public query is strictly refused with 400 (R7)
    for forged in [
        json!({ "targets": [{ "refId": "A", "expr": "secret_metric" }] }),
        json!({ "expr": "secret_metric" }),
        json!({ "datasource": { "uid": "other" } }),
        json!({ "variables": { "undeclared_var": "val" } }),
    ] {
        let (st, _, _) = send_req(
            &app,
            Request::builder()
                .method("POST")
                .uri(format!("/api/public/dashboards/{token_1}/panels/3/query"))
                .header("content-type", "application/json")
                .body(Body::from(forged.to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "forged public payload {forged} must be refused"
        );
    }

    // 5. Valid public query runs saved panel 3 and caches the result; answer carries frames only (no executedQueryString)
    let before_calls = call_count.load(Ordering::SeqCst);
    let (st, _, pub_q1) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/public/dashboards/{token_1}/panels/3/query"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "intervalMs": 60000,
                    "timeRange": { "from": "1700000000000", "to": "1700003600000" }
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let after_first = call_count.load(Ordering::SeqCst);
    assert_eq!(after_first, before_calls + 1);
    let q1_str = pub_q1.to_string();
    assert!(!q1_str.contains("executedQueryString"));
    assert!(!q1_str.contains("super-secret-prom-credential-xyz"));
    assert!(!q1_str.contains(org_b.to_string().as_str()));

    // Second identical public query hits the cache without calling upstream
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/public/dashboards/{token_1}/panels/3/query"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "intervalMs": 60000,
                    "timeRange": { "from": "1700000000000", "to": "1700003600000" }
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(call_count.load(Ordering::SeqCst), after_first);

    // 6. Rotate token (`POST /odata/v4/PublicDashboards('{pub_uid}')/Rotate`): old token immediately answers 404 (same body as unknown token)
    let (st, _, rotated) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri(format!("/odata/v4/PublicDashboards('{pub_uid}')/Rotate"))
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let token_2 = rotated["accessToken"].as_str().unwrap().to_owned();
    assert_ne!(token_1, token_2);

    let (st_old, _, body_old) = send_req(
        &app,
        Request::builder()
            .uri(format!("/api/public/dashboards/{token_1}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let (_st_unk, _, body_unk) = send_req(
        &app,
        Request::builder()
            .uri("/api/public/dashboards/0000000000000000000000000000000000000000000000000000000000000000")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st_old, StatusCode::NOT_FOUND);
    assert_eq!(body_old, body_unk);

    // New rotated token works
    let (st_new, _, _) = send_req(
        &app,
        Request::builder()
            .uri(format!("/api/public/dashboards/{token_2}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st_new, StatusCode::OK);
}

#[tokio::test]
async fn e2e_cross_organization_404s_and_cve_regression_suite() {
    let (upstream_url, call_count, _) = spawn_mock_upstream().await;
    let (state, org_a, org_b, app) = setup_world(&upstream_url).await;

    // Import a dashboard into org_b
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/odata/v4/Import")
            .header("x-controlroom-org-id", org_b.to_string())
            .header("authorization", "Bearer admin-b-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "dryRun": false,
                    "dashboard": {
                        "uid": "b-secret-dash",
                        "title": "Org B Confidential",
                        "schemaVersion": 42,
                        "panels": []
                    }
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Cross-organization read from org_a for org_b's dashboard -> 404 (never 403), identical to unknown UID
    let (st_cross, _, body_cross) = send_req(
        &app,
        Request::builder()
            .uri("/api/dashboards/uid/b-secret-dash")
            .header("x-controlroom-org-id", org_a.to_string())
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let (_st_unk, _, body_unk) = send_req(
        &app,
        Request::builder()
            .uri("/api/dashboards/uid/unknown-uid-xyz")
            .header("x-controlroom-org-id", org_a.to_string())
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st_cross, StatusCode::NOT_FOUND);
    assert_eq!(body_cross, body_unk);

    // Cross-organization datasource query from org_a naming org_b's datasource (`b-private-ds`) -> 404 and zero outbound calls!
    let calls_before = call_count.load(Ordering::SeqCst);
    let (st_ds, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/ds/query")
            .header("x-controlroom-org-id", org_a.to_string())
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "queries": [
                        { "refId": "A", "datasource": { "uid": "b-private-ds" }, "expr": "up" }
                    ]
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st_ds, StatusCode::NOT_FOUND);
    assert_eq!(call_count.load(Ordering::SeqCst), calls_before);

    // =====================================================================
    // §4.3 / R12 CVE Regression Tests
    // =====================================================================

    // CVE Class 1: Datasource proxy credential leak (CVE-2022-31130)
    // Reading datasource metadata or exporting dashboards never exposes the secret value (`super-secret-prom-credential-xyz`).
    let (st, _, ds_dto) = send_req(
        &app,
        Request::builder()
            .uri("/api/datasources/uid/prometheus")
            .header("authorization", "Bearer admin-a-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ds_text = ds_dto.to_string();
    assert!(!ds_text.contains("super-secret-prom-credential-xyz"));
    assert_eq!(ds_dto["secureJsonFields"]["token"], true);

    // CVE Class 2: SSRF through datasource URL & redirect bypass (CVE-2022-29170)
    let strict_guard = OutboundGuard::new(GuardConfig {
        allow_hosts: vec![
            url::Url::parse(&upstream_url)
                .unwrap()
                .host_str()
                .unwrap()
                .to_owned(),
            "169.254.169.254".into(),
        ],
        allow_http: true,
        allow_loopback_for_tests: true,
        ..GuardConfig::default()
    });
    // Cloud metadata IP is blocked even if listed in allow_hosts!
    let meta_attempt = strict_guard
        .send(controlroom_query::OutboundRequest {
            method: reqwest::Method::GET,
            url: "http://169.254.169.254/latest/meta-data/".into(),
            headers: vec![],
            body: None,
            timeout: None,
        })
        .await;
    assert!(meta_attempt.is_err());

    // Upstream 302 redirect to metadata is blocked!
    let redirect_attempt = strict_guard
        .send(controlroom_query::OutboundRequest {
            method: reqwest::Method::GET,
            url: format!("{upstream_url}/redirect-ssrf"),
            headers: vec![],
            body: None,
            timeout: None,
        })
        .await;
    assert!(redirect_attempt.is_err());
    assert!(
        redirect_attempt
            .unwrap_err()
            .to_string()
            .contains("redirect")
    );

    // CVE Class 3: Stored XSS through panel titles and Markdown text panels (CVE-2023-22462)
    let xss_dash = json!({
        "uid": "xss-test",
        "title": "XSS <script>alert('title')</script>",
        "schemaVersion": 42,
        "panels": [
            {
                "id": 1,
                "type": "text",
                "title": "<img src=x onerror=alert(1)>",
                "options": {
                    "content": "# Heading <script>alert('md')</script>\n<svg onload=alert(2)>"
                }
            }
        ]
    });
    let (st, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/odata/v4/Import")
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "dryRun": false, "dashboard": xss_dash }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let pv_xss = panel_view(
        &state,
        &Subject::authenticated("admin", vec![org_a], vec![]),
        org_a,
        "xss-test",
        1,
        PanelTimeRange::default(),
    )
    .await
    .unwrap();
    assert!(!pv_xss.html_table.contains("<script>"));
    assert!(!pv_xss.html_table.contains("<svg"));
    assert!(pv_xss.html_table.contains("&lt;script&gt;"));

    // Also verify `javascript:` link in a dashboard is rejected on import (422)
    let js_link_dash = json!({
        "uid": "js-link-dash",
        "title": "Bad Link",
        "schemaVersion": 42,
        "links": [
            { "title": "Click", "type": "link", "url": "javascript:alert(document.cookie)" }
        ]
    });
    let (st_js, _, _) = send_req(
        &app,
        Request::builder()
            .method("POST")
            .uri("/odata/v4/Import")
            .header("authorization", "Bearer admin-a-token")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "dryRun": false, "dashboard": js_link_dash }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(st_js, StatusCode::UNPROCESSABLE_ENTITY);

    // CVE Class 5: Path traversal in provisioning paths (CVE-2021-43798)
    let bad_prov = r#"
apiVersion: 1
providers:
  - name: escape
    type: file
    options:
      path: "../../etc/shadow"
"#;
    assert!(parse_dashboard_providers_yaml(bad_prov, None).is_err());
}

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

fn simple_dash(uid: &str) -> Value {
    json!({ "uid": uid, "title": format!("Dash {uid}"), "schemaVersion": 42, "panels": [] })
}

/// Identity comes only from verified credentials: forged `X-Controlroom-*` identity headers, an
/// unknown bearer token and no credential at all are anonymous and see nothing (404).
#[tokio::test]
async fn security_forged_identity_headers_grant_nothing() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (_state, org_a, org_b, app) = setup_world(&upstream_url).await;

    // Forged headers claiming admin-b in org_b, no credential.
    let forged = Request::builder()
        .method("POST")
        .uri("/odata/v4/Import")
        .header("x-controlroom-org-id", org_b.to_string())
        .header("x-controlroom-orgs", org_b.to_string())
        .header("x-controlroom-subject", "admin-b")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "dryRun": false, "dashboard": simple_dash("forged") }).to_string(),
        ))
        .unwrap();
    let (st, _, _) = send_req(&app, forged).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // No credential at all: the default organization is not writable either.
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/odata/v4/Import",
            None,
            Some(json!({ "dryRun": false, "dashboard": simple_dash("anon") })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // An unknown bearer token is anonymous too.
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/odata/v4/Import",
            Some("not-a-known-token"),
            Some(json!({ "dryRun": false, "dashboard": simple_dash("unknown") })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Nothing was written in either organization.
    for (org, token) in [(org_a, "admin-a-token"), (org_b, "admin-b-token")] {
        let r = Request::builder()
            .uri("/api/search?type=dash-db")
            .header("x-controlroom-org-id", org.to_string())
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let (st, _, hits) = send_req(&app, r).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(hits.as_array().unwrap().len(), 0, "{hits}");
    }

    // A member of org_b selecting org_a by header gets 404, not org_a's data.
    let r = Request::builder()
        .uri("/api/datasources/uid/prometheus")
        .header("x-controlroom-org-id", org_a.to_string())
        .header("authorization", "Bearer admin-b-token")
        .body(Body::empty())
        .unwrap();
    let (st, _, _) = send_req(&app, r).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// `Dashboard/Export` (OData GET, bound GET, bound POST, MCP) decides read on the dashboard in its
/// folder: a viewer scoped to another folder gets the same 404 as for an unknown dashboard.
#[tokio::test]
async fn security_export_requires_read_on_the_dashboard() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (state, org_a, _org_b, app) = setup_world(&upstream_url).await;
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/odata/v4/Import",
            Some("admin-a-token"),
            Some(json!({ "dryRun": false, "dashboard": simple_dash("root-dash") })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st_unknown, _, body_unknown) = send_req(
        &app,
        req(
            "GET",
            "/odata/v4/Dashboards('no-such-dash')/Export",
            Some("folder-viewer-token"),
            None,
        ),
    )
    .await;
    assert_eq!(st_unknown, StatusCode::NOT_FOUND);
    for (method, uri) in [
        ("GET", "/odata/v4/Dashboards('root-dash')/Export"),
        ("POST", "/odata/v4/Dashboards('root-dash')/Export"),
        ("GET", "/odata/v4/Dashboard.Export(uid='root-dash')"),
    ] {
        let (st, _, body) =
            send_req(&app, req(method, uri, Some("folder-viewer-token"), None)).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
        assert_eq!(
            body["error"]["code"], body_unknown["error"]["code"],
            "{method} {uri}"
        );
        assert!(!body.to_string().contains("Dash root-dash"));
    }
    let viewer = RequestCtx::new(
        state.installation_id,
        org_a,
        state.default_instance_id,
        Subject::authenticated("folder-viewer", vec![org_a], vec![]),
    );
    let mcp = call_mcp_tool(
        &state,
        &viewer,
        "controlroom_export",
        json!({ "uid": "root-dash" }),
    )
    .await;
    assert!(mcp.is_err());

    // The admin still exports it.
    let (st, _, body) = send_req(
        &app,
        req(
            "GET",
            "/odata/v4/Dashboards('root-dash')/Export",
            Some("admin-a-token"),
            None,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["uid"], "root-dash");
}

/// Grafana compat folder, search, organization and public-dashboard config routes decide like the
/// native ones: another organization's objects answer 404 and never appear in a list.
#[tokio::test]
async fn security_compat_routes_authorize_every_object() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (_state, org_a, _org_b, app) = setup_world(&upstream_url).await;

    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/api/folders",
            Some("admin-a-token"),
            Some(json!({ "uid": "secret-folder", "title": "Org A Secret Folder" })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/api/dashboards/db",
            Some("admin-a-token"),
            Some(json!({ "dashboard": simple_dash("a-pub"), "overwrite": true })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/api/dashboards/uid/a-pub/public-dashboards",
            Some("admin-a-token"),
            Some(json!({ "isEnabled": true })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let as_b_in_a = |uri: &str| {
        Request::builder()
            .uri(uri)
            .header("x-controlroom-org-id", org_a.to_string())
            .header("authorization", "Bearer admin-b-token")
            .body(Body::empty())
            .unwrap()
    };
    let (st, _, folders) = send_req(&app, as_b_in_a("/api/folders")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!folders.to_string().contains("secret-folder"), "{folders}");
    let (st, _, _) = send_req(&app, as_b_in_a("/api/folders/secret-folder")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, hits) = send_req(&app, as_b_in_a("/api/search")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!hits.to_string().contains("secret-folder"), "{hits}");
    let (st, _, _) = send_req(&app, as_b_in_a("/api/org")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, cfg) = send_req(
        &app,
        as_b_in_a("/api/dashboards/uid/a-pub/public-dashboards"),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{cfg}");

    // The folder viewer of folder `ops` does not see an unrelated folder either.
    let (st, _, folders) = send_req(
        &app,
        req("GET", "/api/folders", Some("folder-viewer-token"), None),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!folders.to_string().contains("secret-folder"));

    // Organizations: creating one needs the installation admin; lookups need membership.
    let (st, _, _) = send_req(
        &app,
        req("POST", "/api/orgs", None, Some(json!({ "name": "evil" }))),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/api/orgs",
            Some("admin-b-token"),
            Some(json!({ "name": "evil" })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, created) = send_req(
        &app,
        req(
            "POST",
            "/api/orgs",
            Some("admin-a-token"),
            Some(json!({ "name": "city-two" })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let (st, _, _) = send_req(&app, req("GET", "/api/orgs/name/city-two", None, None)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = send_req(
        &app,
        req(
            "GET",
            "/api/orgs/name/city-two",
            Some("admin-b-token"),
            None,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, found) = send_req(
        &app,
        req(
            "GET",
            "/api/orgs/name/city-two",
            Some("admin-a-token"),
            None,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(found["name"], "city-two");

    // A public dashboard can only be created for a dashboard that exists in the organization.
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/api/dashboards/uid/does-not-exist/public-dashboards",
            Some("admin-a-token"),
            Some(json!({ "isEnabled": true })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// The query cache key carries the variable values: a query with `job=b` never gets `job=a`'s
/// cached answer. A reversed time range is refused; `$batch` is capped.
#[tokio::test]
async fn security_cache_key_variables_time_range_and_batch_cap() {
    let (upstream_url, call_count, _) = spawn_mock_upstream().await;
    let (_state, _org_a, _org_b, app) = setup_world(&upstream_url).await;
    let query = |job: &str| {
        json!({
            "from": "2023-11-14T22:10:00Z",
            "to": "2023-11-14T22:20:00Z",
            "variables": { "job": job },
            "queries": [{
                "refId": "A",
                "datasource": { "type": "prometheus", "uid": "prometheus" },
                "expr": "up{job=\"$job\"}"
            }]
        })
    };
    let before = call_count.load(Ordering::SeqCst);
    for job in ["a", "b", "a"] {
        let (st, _, body) = send_req(
            &app,
            req(
                "POST",
                "/odata/v4/Datasource.Query",
                Some("admin-a-token"),
                Some(query(job)),
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }
    // a, b go upstream; the second a is a cache hit.
    assert_eq!(call_count.load(Ordering::SeqCst), before + 2);

    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/odata/v4/Datasource.Query",
            Some("admin-a-token"),
            Some(json!({
                "from": "2023-11-14T22:20:00Z",
                "to": "2023-11-14T22:10:00Z",
                "queries": [{ "refId": "A", "datasource": { "uid": "prometheus" }, "expr": "up" }]
            })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let subs: Vec<Value> = (0..=controlroom_api::odata::MAX_BATCH_REQUESTS)
        .map(|i| json!({ "id": i.to_string(), "method": "GET", "url": "Dashboards" }))
        .collect();
    let (st, _, _) = send_req(
        &app,
        req(
            "POST",
            "/odata/v4/$batch",
            Some("admin-a-token"),
            Some(json!({ "requests": subs })),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// Without a configured exchanger no caller token leaves controlroom: the default refuses.
#[tokio::test]
async fn security_default_token_exchanger_never_forwards_the_caller_token() {
    let (upstream_url, _, _) = spawn_mock_upstream().await;
    let (state, _org_a, _org_b, _app) = setup_world(&upstream_url).await;
    assert!(
        state
            .token_exchanger
            .exchange_for_audience("user-alice-jwt", "aud")
            .is_err()
    );
}
