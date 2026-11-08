#![forbid(unsafe_code)]
//! The OData 4.01 service at `/odata/v4` (R10, R10a): service document, `$metadata`, entity
//! sets with query options, bound and unbound functions and actions, and `$batch`. Every set
//! holds only the rows the caller may read; refusals are hidden 404s.

mod common;

use common::{ORG, World, dashboard, loopback, prometheus, world, world_with};
use daiya_model::{Action, Authorizer, Decision, ObjectTarget, RequestCtx};
use http::{Request, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;

async fn seeded() -> World {
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_folder("ops", "Ops").await;
    w.add_dashboard(dashboard("edge", "prom"), Some("ops"))
        .await;
    w.add_dashboard(dashboard("loose", "prom"), None).await;
    w
}

#[tokio::test]
async fn service_document_and_metadata() {
    let w = world();
    let doc = w.ok("GET", "/odata/v4", None).await;
    let names: Vec<&str> = doc["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    for set in ["Dashboards", "Folders", "Datasources", "ImportReports"] {
        assert!(names.contains(&set), "{names:?}");
    }
    let (st, xml) = w.call("GET", "/odata/v4/$metadata", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(xml.as_str().unwrap().contains("<edmx:Edmx"), "{xml}");
    let (st, json_meta) = w
        .call("GET", "/odata/v4/$metadata?$format=json", None, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(json_meta.is_object());
    let (st, by_accept) = w
        .send(
            Request::builder()
                .uri("/odata/v4/$metadata")
                .header("accept", "application/json")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(by_accept.is_object());
}

#[tokio::test]
async fn entity_sets_answer_rows_with_query_options() {
    let w = seeded().await;
    let all = w.ok("GET", "/odata/v4/Dashboards", None).await;
    assert_eq!(all["value"].as_array().unwrap().len(), 2);
    let filtered = w
        .ok(
            "GET",
            "/odata/v4/Dashboards?$filter=uid%20eq%20'edge'&$select=uid,title&$count=true",
            None,
        )
        .await;
    assert_eq!(filtered["@odata.count"], 1);
    assert_eq!(filtered["value"][0]["uid"], "edge");
    let paged = w
        .ok("GET", "/odata/v4/Dashboards?$top=1&$orderby=uid", None)
        .await;
    assert_eq!(paged["value"].as_array().unwrap().len(), 1);
    assert_eq!(paged["value"][0]["uid"], "edge");

    let one = w.ok("GET", "/odata/v4/Dashboards('edge')", None).await;
    assert_eq!(one["folderUid"], "ops");
    let (st, _) = w
        .call("GET", "/odata/v4/Dashboards('none')", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    for set in [
        "DashboardVersions",
        "Folders",
        "Datasources",
        "LibraryPanels",
        "PublicDashboards",
        "QueryCacheStats",
        "ImportReports",
    ] {
        let page = w.ok("GET", &format!("/odata/v4/{set}"), None).await;
        assert!(page["value"].is_array(), "{set}: {page}");
    }
    let reports = w.ok("GET", "/odata/v4/ImportReports", None).await;
    assert!(!reports["value"].as_array().unwrap().is_empty());
    let versions = w.ok("GET", "/odata/v4/DashboardVersions", None).await;
    assert_eq!(versions["value"].as_array().unwrap().len(), 2);

    let count = w.ok("GET", "/odata/v4/Dashboards/$count", None).await;
    assert_eq!(count, json!(2));
    let export = w.ok("GET", "/odata/v4/Folders/$export", None).await;
    assert_eq!(export.as_array().unwrap().len(), 1);
    let allowed = w
        .ok(
            "GET",
            "/odata/v4/Dashboards/$allowed?op=delete&key=edge",
            None,
        )
        .await;
    assert!(allowed.is_object(), "{allowed}");
    let (st, _) = w
        .call(
            "GET",
            "/odata/v4/Dashboards/$allowed?op=dance",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = w
        .call("GET", "/odata/v4/Dashboards/$nope", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_query_options_and_unknown_sets_are_refused() {
    let w = seeded().await;
    for uri in [
        "/odata/v4/Dashboards?$filter=nosuch%20eq%201",
        "/odata/v4/Dashboards?$filter=((",
        "/odata/v4/Dashboards/$count?$filter=((",
        "/odata/v4/Dashboards/$export?$filter=((",
    ] {
        let (st, body) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "cr.bad_request", "{body}");
    }
    for uri in ["/odata/v4/Nothing", "/odata/v4/Nothing/$count"] {
        let (st, _) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
    }
    // Without a credential, the organization's sets hold nothing readable.
    let (st, body) = w.call("GET", "/odata/v4/Dashboards", None, None).await;
    assert!(
        st == StatusCode::OK && body["value"] == json!([]) || st.is_client_error(),
        "{st} {body}"
    );
}

#[tokio::test]
async fn unbound_and_bound_functions() {
    let w = seeded().await;
    let allowed = w
        .ok("GET", "/odata/v4/Allowed(operation='Import')", None)
        .await;
    assert_eq!(allowed["allowed"], true, "{allowed}");
    let (st, denied) = w
        .call(
            "GET",
            "/odata/v4/JoinedContext.Daiya.Allowed?operation=Apply",
            Some("viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(denied["allowed"], false, "{denied}");

    for uri in [
        "/odata/v4/Datasource.Health(uid='prom')",
        "/odata/v4/Datasources('prom')/Health",
        "/odata/v4/Datasources('prom')/JoinedContext.Daiya.Health",
    ] {
        let h = w.ok("GET", uri, None).await;
        assert_eq!(h["status"], "ok", "{uri}: {h}");
    }
    for uri in [
        "/odata/v4/Datasource_EditorSchema(uid='prom')",
        "/odata/v4/Datasources('prom')/EditorSchema",
    ] {
        let e = w.ok("GET", uri, None).await;
        assert_eq!(e["kind"], "prometheus", "{uri}: {e}");
    }
    for uri in [
        "/odata/v4/Datasource.Resource(uid='prom',path='labels')",
        "/odata/v4/Datasources('prom')/Resource(path='labels')",
    ] {
        let r = w.ok("GET", uri, None).await;
        assert_eq!(r["data"], json!(["job"]), "{uri}: {r}");
    }
    for uri in [
        "/odata/v4/Dashboard.RenderPanel(uid='edge',panelId='1',from='1700000000000',to='1700000600000')",
        "/odata/v4/Dashboards('edge')/RenderPanel(panelId='1',from='1700000000000',to='1700000600000',intervalMs='60000',maxDataPoints='100')",
    ] {
        let pv = w.ok("GET", uri, None).await;
        assert_eq!(pv["panelTitle"], "Edge", "{uri}: {pv}");
    }
    for uri in [
        "/odata/v4/Dashboard_Export(uid='edge')",
        "/odata/v4/Dashboards('edge')/Export",
    ] {
        let d = w.ok("GET", uri, None).await;
        assert_eq!(d["uid"], "edge", "{uri}: {d}");
    }

    // Unknown targets answer 404 through each door.
    for uri in [
        "/odata/v4/Datasource.Health(uid='none')",
        "/odata/v4/Datasources('none')/Health",
        "/odata/v4/Datasource.EditorSchema(uid='none')",
        "/odata/v4/Datasources('none')/EditorSchema",
        "/odata/v4/Datasource.Resource(uid='none',path='labels')",
        "/odata/v4/Datasources('none')/Resource(path='labels')",
        "/odata/v4/Dashboard.RenderPanel(uid='none')",
        "/odata/v4/Dashboards('none')/RenderPanel",
        "/odata/v4/Dashboard.Export(uid='none')",
        "/odata/v4/Dashboards('none')/Export",
    ] {
        let (st, _) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn actions_query_import_plan_apply_and_rotate() {
    let w = seeded().await;
    let q = json!({
        "from": "1700000000000", "to": "1700000600000",
        "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
    });
    let sets = w
        .ok("POST", "/odata/v4/Datasource.Query", Some(q.clone()))
        .await;
    assert_eq!(sets["value"][0]["refId"], "A");
    let bound = w
        .ok(
            "POST",
            "/odata/v4/Datasources('prom')/Query",
            Some(json!({
                "from": "1700000000000", "to": "1700000600000",
                "queries": [{ "refId": "B", "expr": "up" }]
            })),
        )
        .await;
    assert_eq!(bound["value"][0]["refId"], "B");
    let health = w
        .ok("POST", "/odata/v4/Datasources('prom')/Health", None)
        .await;
    assert_eq!(health["status"], "ok");
    let export = w
        .ok(
            "POST",
            "/odata/v4/Dashboards('edge')/JoinedContext.Daiya.Export",
            None,
        )
        .await;
    assert_eq!(export["uid"], "edge");

    let dry = w
        .ok(
            "POST",
            "/odata/v4/Import",
            Some(json!({ "dryRun": true, "dashboard": dashboard("dry", "prom") })),
        )
        .await;
    assert!(dry.is_object());
    let (st, _) = w
        .call("GET", "/odata/v4/Dashboards('dry')", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a dry run writes nothing");
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Import",
            Some("viewer"),
            Some(json!({ "dashboard": dashboard("v", "prom") })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let plan = w
        .ok(
            "POST",
            "/odata/v4/Plan",
            Some(json!({ "bundle": { "dashboards": [{ "rawJson": dashboard("planned", "prom") }] } })),
        )
        .await;
    assert!(plan.is_object(), "{plan}");
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Apply",
            Some("editor"),
            Some(json!({ "plan": plan.clone() })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "apply needs approve");
    w.ok("POST", "/odata/v4/Apply", Some(json!({ "plan": plan })))
        .await;
    w.ok("GET", "/odata/v4/Dashboards('planned')", None).await;
    let (st, _) = w
        .call("POST", "/odata/v4/Plan", Some("viewer"), Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Public dashboards: create through the compat route, rotate through both OData doors,
    // query through the action with the current token only.
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/edge/public-dashboards",
            Some(json!({ "isEnabled": true })),
        )
        .await;
    let pub_uid = created["uid"].as_str().unwrap().to_owned();
    let first = created["accessToken"].as_str().unwrap().to_owned();
    let rotated = w
        .ok(
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some(json!({ "uid": pub_uid })),
        )
        .await;
    let second = rotated["accessToken"].as_str().unwrap().to_owned();
    assert_ne!(first, second);
    let rotated = w
        .ok(
            "POST",
            &format!("/odata/v4/PublicDashboards('{pub_uid}')/Rotate"),
            None,
        )
        .await;
    let token = rotated["accessToken"].as_str().unwrap().to_owned();
    let answer = w
        .ok(
            "POST",
            "/odata/v4/PublicDashboard_Query",
            Some(json!({ "accessToken": token, "panelId": 1 })),
        )
        .await;
    assert!(answer["value"].is_array(), "{answer}");
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/PublicDashboard.Query",
            None,
            Some(json!({ "accessToken": first, "panelId": 1 })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a rotated token is gone");
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/PublicDashboard.Rotate",
            Some("admin"),
            Some(json!({ "uid": "none" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/PublicDashboards('none')/Rotate",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn malformed_action_bodies_and_unknown_actions() {
    let w = seeded().await;
    for (uri, body) in [
        ("/odata/v4/Datasource.Query", json!({ "queries": "x" })),
        ("/odata/v4/Import", json!({ "dryRun": "yes" })),
        ("/odata/v4/Plan", json!({ "bundle": { "folders": "x" } })),
        ("/odata/v4/Apply", json!({ "plan": 3 })),
        ("/odata/v4/PublicDashboard.Query", json!({ "panelId": 1 })),
        (
            "/odata/v4/Datasources('prom')/Query",
            json!({ "queries": 1 }),
        ),
    ] {
        let (st, answer) = w.call("POST", uri, Some("admin"), Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {answer}");
    }
    let (st, _) = w
        .call("POST", "/odata/v4/Dance", Some("admin"), Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Folders('ops')/Dance",
            Some("admin"),
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Errors of the executed action come back as its OData error.
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Datasource.Query",
            Some("admin"),
            Some(json!({ "from": "2", "to": "1" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Datasources('none')/Query",
            Some("admin"),
            Some(json!({ "queries": [{ "refId": "A" }] })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    for uri in [
        "/odata/v4/Datasources('none')/Health",
        "/odata/v4/Dashboards('none')/Export",
    ] {
        let (st, _) = w.call("POST", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn batch_runs_reads_allowed_and_import_and_refuses_the_rest() {
    let w = seeded().await;
    let answer = w
        .ok(
            "POST",
            "/odata/v4/$batch",
            Some(json!({ "requests": [
                { "id": "1", "method": "GET", "url": "/odata/v4/Dashboards?$select=uid" },
                { "id": "2", "method": "get", "url": "Allowed(operation='Import')" },
                { "id": "3", "method": "POST", "url": "Import", "body": { "dashboard": dashboard("batched", "prom") } },
                { "id": "4", "method": "POST", "url": "Import", "body": { "dryRun": "x" } },
                { "id": "5", "method": "DELETE", "url": "Dashboards('edge')" },
                { "id": "6", "method": "GET", "url": "Nothing" },
                { "id": "7", "method": "GET", "url": "Dashboards?$filter=nosuch%20eq%201" }
            ]})),
        )
        .await;
    let by_id = |id: &str| -> Value {
        answer["responses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id("1")["status"], 200);
    assert_eq!(by_id("1")["body"]["value"].as_array().unwrap().len(), 2);
    assert_eq!(by_id("2")["body"]["allowed"], true);
    assert_eq!(by_id("3")["status"], 200);
    assert_eq!(by_id("4")["status"], 400);
    assert_eq!(by_id("5")["status"], 501);
    assert_eq!(by_id("6")["status"], 404);
    // A query option the service cannot parse is refused, never ignored into an unfiltered read.
    assert_eq!(by_id("7")["status"], 400, "{}", by_id("7"));
    w.ok("GET", "/odata/v4/Dashboards('batched')", None).await;

    // A viewer's import inside a batch is refused like the direct call.
    let (st, answer) = w
        .call(
            "POST",
            "/odata/v4/$batch",
            Some("viewer"),
            Some(json!({ "requests": [
                { "id": "1", "method": "POST", "url": "Import", "body": { "dashboard": dashboard("v", "prom") } }
            ]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(answer["responses"][0]["status"], 403);

    let too_many: Vec<Value> = (0..=daiya_api::odata::MAX_BATCH_REQUESTS)
        .map(|i| json!({ "id": i.to_string(), "method": "GET", "url": "Dashboards" }))
        .collect();
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/$batch",
            Some("admin"),
            Some(json!({ "requests": too_many })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// Denies read on dashboard `secret` and folder `hidden` (and what lives in it); allows
/// everything else in ORG for members.
struct DenySome;

impl Authorizer for DenySome {
    fn check(&self, ctx: &RequestCtx, _action: Action, object: &ObjectTarget) -> Decision {
        if !ctx.subject.is_member_of(object.org_id()) || object.org_id() != ctx.org_id {
            return Decision::deny_hidden("other organization");
        }
        let hidden = match object {
            ObjectTarget::Dashboard {
                dashboard_uid,
                folder_uid,
                ..
            } => dashboard_uid == "secret" || folder_uid.as_deref() == Some("hidden"),
            ObjectTarget::PublicDashboard { dashboard_uid, .. } => dashboard_uid == "secret",
            ObjectTarget::Folder { folder_uid, .. } => folder_uid == "hidden",
            ObjectTarget::LibraryPanel { folder_uid, .. } => {
                folder_uid.as_deref() == Some("hidden")
            }
            _ => false,
        };
        if hidden {
            Decision::deny_hidden("not yours")
        } else {
            Decision::allow("member")
        }
    }
}

/// Each entity set filters per object with the same decision as the direct routes: a caller
/// who cannot read dashboard `secret` or folder `hidden` sees neither, nor their versions or
/// public records.
#[tokio::test]
async fn every_entity_set_hides_what_the_caller_cannot_read() {
    let w = world_with(Arc::new(DenySome), loopback());
    w.add_folder("hidden", "Hidden").await;
    w.add_folder("open", "Open").await;
    w.add_dashboard(dashboard("secret", "prom"), None).await;
    w.add_dashboard(dashboard("public", "prom"), Some("open"))
        .await;
    // Saved by the store directly: the decision would hide it from the route as well.
    w.ok(
        "POST",
        "/api/dashboards/uid/public/public-dashboards",
        Some(json!({ "isEnabled": true })),
    )
    .await;
    {
        use daiya_model::DaiyaStore;
        let pubdash = w
            .store
            .inner
            .get_public_dashboard_by_dashboard_uid(ORG, "public")
            .await
            .unwrap()
            .unwrap();
        let mut leaked = pubdash.clone();
        leaked.uid = "pub-secret".into();
        leaked.dashboard_uid = "secret".into();
        leaked.access_token_hash = "00".repeat(32);
        w.store.inner.upsert_public_dashboard(leaked).await.unwrap();
        for (uid, folder) in [("lp-open", "open"), ("lp-hidden", "hidden")] {
            let now = chrono::Utc::now();
            w.store
                .inner
                .upsert_library_panel(daiya_model::LibraryPanelRecord {
                    installation_id: daiya_model::InstallationId::from_u128(1),
                    org_id: ORG,
                    instance_id: daiya_model::InstanceId::from_u128(1),
                    folder_uid: Some(folder.into()),
                    uid: uid.into(),
                    name: uid.into(),
                    panel_type: "stat".into(),
                    description: String::new(),
                    model: json!({ "type": "stat" }),
                    version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }
    }

    let dashboards = w.ok("GET", "/odata/v4/Dashboards", None).await;
    assert_eq!(uids(&dashboards, "uid"), ["public"]);
    let versions = w.ok("GET", "/odata/v4/DashboardVersions", None).await;
    assert_eq!(uids(&versions, "dashboardUid"), ["public"], "{versions}");
    let folders = w.ok("GET", "/odata/v4/Folders", None).await;
    assert_eq!(uids(&folders, "uid"), ["open"], "{folders}");
    let pubs = w.ok("GET", "/odata/v4/PublicDashboards", None).await;
    assert_eq!(uids(&pubs, "dashboardUid"), ["public"], "{pubs}");
    let panels = w.ok("GET", "/odata/v4/LibraryPanels", None).await;
    assert_eq!(uids(&panels, "uid"), ["lp-open"], "{panels}");
}

fn uids(page: &Value, field: &str) -> Vec<String> {
    let mut out: Vec<String> = page["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[field].as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[tokio::test]
async fn store_failures_are_errors_in_every_set_and_action() {
    let w = seeded().await;
    let cases: &[(&'static str, &str, &str)] = &[
        ("list_dashboards", "GET", "/odata/v4/Dashboards"),
        ("list_dashboards", "GET", "/odata/v4/DashboardVersions"),
        (
            "list_dashboard_versions",
            "GET",
            "/odata/v4/DashboardVersions",
        ),
        ("list_folders", "GET", "/odata/v4/Folders"),
        ("list_datasources", "GET", "/odata/v4/Datasources"),
        ("list_library_panels", "GET", "/odata/v4/LibraryPanels"),
        (
            "list_public_dashboards",
            "GET",
            "/odata/v4/PublicDashboards",
        ),
        ("list_query_cache_stats", "GET", "/odata/v4/QueryCacheStats"),
        ("list_import_reports", "GET", "/odata/v4/ImportReports"),
        ("list_dashboards", "GET", "/odata/v4/Dashboards/$count"),
        (
            "get_datasource",
            "GET",
            "/odata/v4/Datasources('prom')/EditorSchema",
        ),
        (
            "get_datasource",
            "GET",
            "/odata/v4/Datasources('prom')/Resource(path='labels')",
        ),
        (
            "get_dashboard",
            "GET",
            "/odata/v4/Dashboards('edge')/Export",
        ),
    ];
    for (method_name, method, uri) in cases {
        w.store.fail(method_name);
        let (st, body) = w.call(method, uri, Some("admin"), None).await;
        w.store.heal();
        assert_eq!(
            st,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method_name} {uri}: {body}"
        );
    }
    // A datasource whose record cannot be read reports unhealthy, never "ok".
    w.store.fail("get_datasource");
    let h = w
        .ok("GET", "/odata/v4/Datasources('prom')/Health", None)
        .await;
    w.store.heal();
    assert_eq!(h["status"], "error", "{h}");

    for (method_name, body, uri) in [
        (
            "upsert_dashboard",
            json!({ "dashboard": dashboard("x", "prom") }),
            "/odata/v4/Import",
        ),
        ("list_dashboards", json!({ "bundle": {} }), "/odata/v4/Plan"),
    ] {
        w.store.fail(method_name);
        let (st, answer) = w.call("POST", uri, Some("admin"), Some(body)).await;
        w.store.heal();
        assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{uri}: {answer}");
    }
    w.store.fail("list_dashboards");
    let (st, answer) = w
        .call(
            "POST",
            "/odata/v4/$batch",
            Some("admin"),
            Some(json!({ "requests": [{ "id": "1", "method": "GET", "url": "Dashboards" }] })),
        )
        .await;
    w.store.heal();
    assert_eq!(st, StatusCode::OK);
    assert_eq!(answer["responses"][0]["status"], 500);
}
