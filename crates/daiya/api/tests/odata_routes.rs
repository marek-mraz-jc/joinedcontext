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

/// The two sets that hang off no dashboard, folder or datasource follow the Organization's read
/// decision: a caller outside it, and one bound to a folder, reads no row and no count.
#[tokio::test]
async fn organization_wide_sets_need_read_on_the_organization() {
    use daiya_model::DaiyaStore as _;
    let w = seeded().await;
    w.store
        .inner
        .insert_import_report(
            daiya_model::InstallationId::SINGLE,
            daiya_model::ImportReportRecord {
                id: "report-1".into(),
                org_id: ORG,
                instance_id: daiya_model::InstanceId::from_u128(1),
                dry_run: false,
                status: "applied".into(),
                dashboard_count: 1,
                refused_count: 0,
                mapped_count: 1,
                summary: json!({}),
                created_at: jc_time::TimestampTz::now(),
            },
        )
        .await
        .unwrap();
    let key = daiya_query::CacheKey::new(
        ORG,
        w.state.default_instance_id,
        &[],
        "prom",
        "r1",
        &json!("up"),
        0,
        60_000,
        15_000,
    );
    assert!(w.state.query_cache.get(&key).is_none(), "one counted miss");
    for set in ["ImportReports", "QueryCacheStats"] {
        for who in [None, Some("outsider"), Some("ops-viewer")] {
            for path in [set.to_owned(), format!("{set}/$count")] {
                let mut request = Request::builder()
                    .uri(format!("/odata/v4/{path}"))
                    .header("x-daiya-org-id", ORG.to_string());
                if let Some(who) = who {
                    request = request.header("authorization", format!("Bearer {who}"));
                }
                let (st, body) = w
                    .send(request.body(axum::body::Body::empty()).unwrap())
                    .await;
                assert_eq!(st, StatusCode::OK, "{path} as {who:?}");
                let none = if path.ends_with("$count") {
                    body == json!(0)
                } else {
                    body["value"] == json!([])
                };
                assert!(none, "{path} as {who:?} answers {body}");
            }
        }
        let page = w.ok("GET", &format!("/odata/v4/{set}"), None).await;
        assert_ne!(
            page["value"],
            json!([]),
            "a reader of the Organization keeps {set}"
        );
    }
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
    // JSON only when JSON is asked for and XML is not.
    for accept in ["application/json, application/xml", "text/plain"] {
        let (st, body) = w
            .send(
                Request::builder()
                    .uri("/odata/v4/$metadata")
                    .header("accept", accept)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            body.as_str().is_some_and(|x| x.contains("<edmx:Edmx")),
            "{accept}: {body}"
        );
    }
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
    // Delete: the set is read only; read: the caller's organization rights hold it.
    assert_eq!(allowed["allowed"], false, "{allowed}");
    assert_eq!(allowed["reason"], "Dashboards is served read only");
    let readable = w
        .ok(
            "GET",
            "/odata/v4/Dashboards/$allowed?op=read&key=edge",
            None,
        )
        .await;
    assert_eq!(readable["allowed"], true, "{readable}");
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
    for uri in [
        "/odata/v4/Nothing",
        "/odata/v4/Nothing/$count",
        "/odata/v4/Nothing/$allowed?op=read",
        "/odata/v4/Dashboards('edge')/Nope",
    ] {
        let (st, _) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
    }
    // An option the table service does not implement says so (501), never a silent full list.
    let (st, body) = w
        .call(
            "GET",
            "/odata/v4/Dashboards?$apply=groupby((uid))",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED, "{body}");
    // A body that is no JSON is refused at both kinds of action, the public one included.
    for uri in [
        "/odata/v4/PublicDashboard.Query",
        "/odata/v4/Datasource.Query",
    ] {
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", "Bearer admin")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("{not json"))
            .unwrap();
        let (st, body) = w.send(req).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}: {body}");
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
    // The range named in the call is the range the source is asked for.
    let (url, seen) = prometheus().await;
    let ranged = world();
    ranged.add_prometheus("prom", &url).await;
    ranged.add_dashboard(dashboard("edge", "prom"), None).await;
    ranged
        .ok(
            "GET",
            "/odata/v4/Dashboard.RenderPanel(uid='edge',panelId='1',from='1700000000000',to='1700000600000')",
            None,
        )
        .await;
    let asked = seen
        .lock()
        .iter()
        .find(|head| head.contains("query_range"))
        .cloned()
        .unwrap_or_default();
    // Aligned down to the 15 s step: 1700000600 s reads as 1700000595 s.
    assert!(asked.contains("&end=1700000595.000&"), "{asked}");
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
    let digest = plan["digest"]
        .as_str()
        .expect("Plan answers a digest")
        .to_owned();
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Apply",
            Some("editor"),
            Some(json!({ "digest": digest })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "apply needs approve");
    // Apply writes only a bundle the server kept from a Plan: a bundle in the body is refused.
    let mut other = plan.clone();
    other["desired"] = json!({ "dashboards": [{ "rawJson": dashboard("unreviewed", "prom") }] });
    for body in [
        json!({ "plan": other.clone() }),
        json!({ "digest": digest, "desired": other["desired"] }),
        other.clone(),
    ] {
        let (st, answer) = w
            .call("POST", "/odata/v4/Apply", Some("admin"), Some(body))
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{answer}");
    }
    let (st, _) = w
        .call(
            "POST",
            "/odata/v4/Apply",
            Some("admin"),
            Some(json!({ "digest": "0".repeat(64) })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a digest no Plan answered");
    w.ok("POST", "/odata/v4/Apply", Some(json!({ "digest": digest })))
        .await;
    w.ok("GET", "/odata/v4/Dashboards('planned')", None).await;
    let (st, _) = w
        .call(
            "GET",
            "/odata/v4/Dashboards('unreviewed')",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a body bundle was never written");

    // A plan's datasources pass the checks a datasource save runs: a URL the outbound guard
    // refuses, or a read-only record, is refused at Plan (so no digest exists to apply), and
    // nothing is written.
    let source = |uid: &str, url: &str, read_only: bool| {
        json!({
            "installationId": "00000000-0000-0000-0000-000000000001",
            "orgId": ORG.to_string(), "instanceId": "00000000-0000-0000-0000-000000000001",
            "uid": uid, "name": uid, "type": "prometheus", "access": "proxy", "url": url,
            "isDefault": false, "readOnly": read_only, "jsonData": {}, "version": 1,
            "createdAt": "2026-11-19T00:00:00Z", "updatedAt": "2026-11-19T00:00:00Z"
        })
    };
    for (bad, want) in [
        (
            source("meta", "http://169.254.169.254/latest", false),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            source("ro", "https://prom.example", true),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let bundle = json!({ "datasources": [bad] });
        let (st, answer) = w
            .call(
                "POST",
                "/odata/v4/Plan",
                Some("admin"),
                Some(json!({ "bundle": bundle })),
            )
            .await;
        assert_eq!(st, want, "{answer}");
    }
    for uid in ["meta", "ro"] {
        let (st, _) = w
            .call(
                "GET",
                &format!("/api/datasources/uid/{uid}"),
                Some("admin"),
                None,
            )
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uid} was not written");
    }
    // A plan naming a datasource provisioning made read-only is refused (403, as its save
    // route); one naming a writable stored datasource is planned.
    {
        use daiya_model::DaiyaStore;
        let mut prov = w
            .store
            .inner
            .get_datasource(ORG, "prom")
            .await
            .unwrap()
            .unwrap();
        prov.uid = "prov".into();
        prov.name = "Provisioned".into();
        prov.read_only = true;
        w.store.inner.upsert_datasource(prov).await.unwrap();
    }
    let prom_url = w.ok("GET", "/api/datasources/uid/prom", None).await["url"].clone();
    let (st, answer) = w
        .call(
            "POST",
            "/odata/v4/Plan",
            Some("admin"),
            Some(json!({ "bundle": { "datasources": [source("prov", "https://prom.example", false)] } })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{answer}");
    assert!(
        answer.to_string().contains("read-only data source 'prov'"),
        "{answer}"
    );
    let planned = w
        .ok(
            "POST",
            "/odata/v4/Plan",
            Some(json!({ "bundle": { "datasources": [source("prom", prom_url.as_str().unwrap(), false)] } })),
        )
        .await;
    assert!(planned["digest"].is_string(), "{planned}");
    let (st, _) = w
        .call("POST", "/odata/v4/Plan", Some("viewer"), Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Public dashboards: create through the compat route, rotate through both OData doors,
    // query through the action with the current token only.
    // Published with a panel that only computes: a public dashboard reads no Prometheus.
    let mut published = dashboard("published", "prom");
    published["panels"][0]["datasource"] = json!({ "type": "__expr__", "uid": "__expr__" });
    published["panels"][0]["targets"] = json!([{ "refId": "A", "type": "math",
        "expression": "1 + 1", "datasource": { "type": "__expr__", "uid": "__expr__" } }]);
    w.add_dashboard(published, None).await;
    let created = w
        .ok(
            "POST",
            "/api/dashboards/uid/published/public-dashboards",
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

/// Every write and decision of daiya, through its door, leaves exactly one audit row that
/// commits with it (S9-28): the kind of `AUDITED_WRITES`, the actor's subject id and ids, never
/// a token, a secret or a name. A write operation of the registry or a door missing from the
/// table fails this test.
#[tokio::test]
async fn every_write_operation_leaves_one_audit_row() {
    use daiya_api::AUDITED_WRITES;
    use daiya_model::DaiyaStore as _;
    let w = world();
    let (prom, _) = prometheus().await;
    let ds = |name: &str| json!({ "uid": "prom", "name": name, "type": "prometheus", "url": prom });
    let mut steps: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateOrg",
            "POST",
            "/api/orgs".into(),
            Some(json!({ "name": "tampere" })),
        ),
        (
            "AddDatasource",
            "POST",
            "/api/datasources".into(),
            Some(ds("Prom")),
        ),
        (
            "UpdateDatasource",
            "PUT",
            "/api/datasources/uid/prom".into(),
            Some(ds("Prom city")),
        ),
        (
            "CreateFolder",
            "POST",
            "/api/folders".into(),
            Some(json!({ "uid": "ops", "title": "Ops" })),
        ),
        (
            "Import",
            "POST",
            "/api/dashboards/db".into(),
            Some(json!({ "dashboard": dashboard("edge", "prom"), "overwrite": true })),
        ),
        (
            "Import",
            "POST",
            "/odata/v4/Import".into(),
            Some(json!({ "dashboard": dashboard("two", "prom") })),
        ),
        (
            "Plan",
            "POST",
            "/odata/v4/Plan".into(),
            Some(
                json!({ "bundle": { "dashboards": [{ "rawJson": dashboard("planned", "prom") }] } }),
            ),
        ),
        (
            "SavePublicDashboard",
            "POST",
            "/api/dashboards/uid/edge/public-dashboards".into(),
            // Disabled: an enabled one needs Endpoint datasources.
            Some(json!({ "isEnabled": false })),
        ),
        (
            "DeleteDashboard",
            "DELETE",
            "/api/dashboards/uid/two".into(),
            None,
        ),
        ("DeleteFolder", "DELETE", "/api/folders/ops".into(), None),
    ];
    let mut tokens = Vec::new();
    let mut seen = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        let (operation, method, uri, body) = steps[i].clone();
        let before = w.store.list_audit(ORG).await.expect("rows").len();
        let (st, answer) = w.call(method, &uri, Some("admin"), body).await;
        assert_eq!(st, StatusCode::OK, "{operation}: {answer}");
        let rows = w.store.list_audit(ORG).await.expect("rows");
        assert_eq!(rows.len(), before + 1, "{operation} leaves one row");
        let row = rows.last().expect("row");
        let kind = AUDITED_WRITES
            .iter()
            .find(|(name, _)| *name == operation)
            .map(|(_, kind)| *kind);
        assert_eq!(
            (row.operation.as_str(), Some(row.kind.as_str())),
            (operation, kind)
        );
        assert_eq!(
            (row.actor_id.as_str(), row.decision.as_str()),
            ("admin", "allow")
        );
        if let Some(token) = answer["accessToken"].as_str() {
            tokens.push(token.to_owned());
        }
        let text = serde_json::to_string(row).expect("row json");
        for never in tokens.iter().map(String::as_str).chain(["@", "Board "]) {
            assert!(
                !text.contains(never),
                "{operation}: the row holds {never}: {text}"
            );
        }
        match operation {
            "CreateOrg" => {
                let number = answer["orgId"].as_i64().expect("org number");
                steps.push(("RetireOrg", "DELETE", format!("/api/orgs/{number}"), None));
            }
            "Plan" => {
                let digest = answer["digest"].as_str().expect("digest").to_owned();
                // Apply right after its Plan: any later write moves the organization's state.
                steps.insert(
                    i + 1,
                    (
                        "Apply",
                        "POST",
                        "/odata/v4/Apply".into(),
                        Some(json!({ "digest": digest })),
                    ),
                );
            }
            "SavePublicDashboard" if method == "POST" => {
                let uid = answer["uid"].as_str().expect("uid").to_owned();
                steps.push((
                    "PublicDashboard/Rotate",
                    "POST",
                    "/odata/v4/PublicDashboard.Rotate".into(),
                    Some(json!({ "uid": uid })),
                ));
                steps.push((
                    "SavePublicDashboard",
                    "PATCH",
                    format!("/api/dashboards/uid/edge/public-dashboards/{uid}"),
                    Some(json!({ "timeSelectionEnabled": true })),
                ));
            }
            "DeleteFolder" => {
                steps.push((
                    "DeleteDatasource",
                    "DELETE",
                    "/api/datasources/uid/prom".into(),
                    None,
                ));
            }
            _ => {}
        }
        seen.push(operation);
        i += 1;
    }
    let unaudited: Vec<&str> = AUDITED_WRITES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !seen.contains(name))
        .collect();
    assert!(
        unaudited.is_empty(),
        "writes without an audit row step: {unaudited:?}"
    );
    let registry_writes: Vec<String> = w
        .state
        .registry
        .operations
        .iter()
        .filter(|op| op.required_action != Action::Read)
        .map(|op| op.name.clone())
        .filter(|name| !AUDITED_WRITES.iter().any(|(audited, _)| audited == name))
        .collect();
    assert!(
        registry_writes.is_empty(),
        "registry writes without an audit kind: {registry_writes:?}"
    );
    // A write whose name has no audit kind fails, so it never stands without its row.
    let ctx = RequestCtx::new(
        daiya_model::InstallationId::from_u128(1),
        ORG,
        daiya_model::InstanceId::from_u128(1),
        daiya_model::Subject::authenticated("admin", vec![ORG], vec![]),
    );
    let before = w.store.list_audit(ORG).await.expect("rows").len();
    let err = daiya_api::state::audit(&w.state, &ctx, "Dashboard/Archive", "edge".into())
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 500, "{err}");
    assert!(err.to_string().contains("'Dashboard/Archive'"), "{err}");
    assert_eq!(w.store.list_audit(ORG).await.expect("rows").len(), before);
}

#[tokio::test]
async fn malformed_action_bodies_and_unknown_actions() {
    let w = seeded().await;
    for (uri, body) in [
        ("/odata/v4/Datasource.Query", json!({ "queries": "x" })),
        ("/odata/v4/Import", json!({ "dryRun": "yes" })),
        // The provisioning fields belong to the loader alone.
        (
            "/odata/v4/Import",
            json!({ "dashboards": [{ "rawJson": dashboard("p", "prom"), "isProvisioned": true }] }),
        ),
        (
            "/odata/v4/Import",
            json!({ "dashboards": [{ "rawJson": dashboard("p", "prom"), "externalId": "boards/p.json" }] }),
        ),
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
    let (st, answer) = w
        .call(
            "POST",
            "/odata/v4/$batch",
            Some("admin"),
            Some(json!({ "requests": &too_many[1..] })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "exactly the limit is served");
    assert_eq!(
        answer["responses"].as_array().map(Vec::len),
        Some(daiya_api::odata::MAX_BATCH_REQUESTS)
    );

    // Import is the one action a batch runs: a POST to a set is not an import.
    let (st, answer) = w
        .call(
            "POST",
            "/odata/v4/$batch",
            Some("admin"),
            Some(json!({ "requests": [
                { "id": "1", "method": "POST", "url": "Dashboards", "body": { "dashboard": dashboard("p", "prom") } }
            ]})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(answer["responses"][0]["status"], 501, "{answer}");
}

/// The page makes no caller a member of the Organization its link names; only the server
/// administrator is one of every live organization.
#[tokio::test]
async fn the_page_names_no_outsider_a_member() {
    let w = world_with(Arc::new(DenySome), loopback());
    w.add_dashboard(dashboard("open", "prom"), None).await;
    let (st, _) = w.call("GET", "/d/open?orgId=1", Some("viewer"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = w
        .call("GET", "/d/open?orgId=1", Some("outsider"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
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

    fn read_scope(&self, ctx: &RequestCtx, kind: daiya_model::ReadKind) -> daiya_model::ReadScope {
        use daiya_model::{ReadKind, ReadScope, ScopeColumn};
        if !ctx.subject.is_member_of(ctx.org_id) {
            return ReadScope::Nothing;
        }
        let (column, open) = match kind {
            ReadKind::Dashboard | ReadKind::PublicDashboard => (ScopeColumn::Object, "public"),
            ReadKind::Folder => (ScopeColumn::Object, "open"),
            ReadKind::LibraryPanel => (ScopeColumn::Folder, "open"),
            ReadKind::Organization | ReadKind::Datasource => return ReadScope::All,
        };
        ReadScope::In {
            column,
            values: [open.to_owned()].into(),
        }
    }
}

/// Allows members everything in ORG except reading the Organization itself.
struct NoOrganizationRead;

impl Authorizer for NoOrganizationRead {
    fn check(&self, ctx: &RequestCtx, _action: Action, object: &ObjectTarget) -> Decision {
        if !ctx.subject.is_member_of(object.org_id()) || object.org_id() != ctx.org_id {
            return Decision::deny_hidden("other organization");
        }
        if matches!(object, ObjectTarget::Organization { .. }) {
            return Decision::deny_hidden("no read on the organization");
        }
        Decision::allow("member")
    }

    fn read_scope(
        &self,
        _ctx: &RequestCtx,
        _kind: daiya_model::ReadKind,
    ) -> daiya_model::ReadScope {
        daiya_model::ReadScope::All
    }
}

/// The cache statistics list an entry only to a caller who may read the Organization, even
/// when the caller may read the entry's dashboard.
#[tokio::test]
async fn cache_statistics_need_a_read_on_the_organization() {
    let deciders: [(Arc<dyn Authorizer>, usize); 2] =
        [(Arc::new(DenySome), 1), (Arc::new(NoOrganizationRead), 0)];
    for (authz, want) in deciders {
        let w = world_with(authz, loopback());
        let key = daiya_query::cache::PublicCacheKey::new(
            ORG,
            daiya_model::InstanceId::from_u128(1),
            "dash",
            1,
            "r",
            1,
            0,
            60_000,
            1_000,
            100,
        );
        w.state.query_cache.insert_public(
            key.clone(),
            Vec::new(),
            std::time::Duration::from_secs(60),
        );
        assert!(w.state.query_cache.get_public(&key).is_some());
        let stats = w.ok("GET", "/odata/v4/QueryCacheStats", None).await;
        assert_eq!(
            stats["value"].as_array().map(Vec::len),
            Some(want),
            "{stats}"
        );
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
        Some(json!({ "isEnabled": false })),
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
            let now = jc_time::TimestampTz::now();
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

/// `and`, `or` and `not` combine; a time compares with a date or a timestamp literal, and a
/// literal that is no time matches nothing; text functions work on text; a `$skiptoken` longer
/// than any this service writes is refused before it is decoded.
#[tokio::test]
async fn filters_combine_compare_times_and_refuse_what_they_cannot_read() {
    let w = seeded().await;
    for (filter, want) in [
        ("uid%20eq%20'edge'%20or%20uid%20eq%20'loose'", 2),
        ("uid%20eq%20'edge'%20and%20not%20(uid%20eq%20'loose')", 1),
        ("not%20(uid%20eq%20'edge')", 1),
        ("updatedAt%20gt%202000-01-01", 2),
        ("updatedAt%20lt%202000-01-01", 0),
        ("updatedAt%20eq%20'soon'", 0),
        ("contains(title,'edge')", 1),
        ("startswith(title,'Board')", 2),
    ] {
        let uri = format!("/odata/v4/Dashboards/$count?$filter={filter}");
        assert_eq!(w.ok("GET", &uri, None).await, json!(want), "{filter}");
    }
    let oversized = format!("/odata/v4/Folders?$skiptoken={}", "A".repeat(40 * 1024));
    {
        let (st, body) = w.call("GET", &oversized, Some("admin"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.to_string().contains("not one this service wrote"),
            "{body}"
        );
    }
}

/// Grants read on the whole organization in the read scope but denies folder `hidden` in the
/// per-object decision: an authorizer whose two answers disagree.
struct WideScope;

impl Authorizer for WideScope {
    fn check(&self, ctx: &RequestCtx, _action: Action, object: &ObjectTarget) -> Decision {
        match object {
            ObjectTarget::Folder { folder_uid, .. } if folder_uid == "hidden" => {
                Decision::deny_hidden("not yours")
            }
            ObjectTarget::Dashboard { dashboard_uid, .. } if dashboard_uid == "secret" => {
                Decision::deny_hidden("not yours")
            }
            _ if ctx.subject.is_member_of(object.org_id()) => Decision::allow("member"),
            _ => Decision::deny_hidden("other organization"),
        }
    }

    fn read_scope(&self, ctx: &RequestCtx, _kind: daiya_model::ReadKind) -> daiya_model::ReadScope {
        if ctx.subject.is_member_of(ctx.org_id) {
            daiya_model::ReadScope::All
        } else {
            daiya_model::ReadScope::Nothing
        }
    }
}

/// A row the read scope lets through but the decision refuses is an error, never an answer:
/// the set, its export and search show no row of it.
#[tokio::test]
async fn a_read_scope_wider_than_the_decision_is_an_error_not_a_leak() {
    let w = world_with(Arc::new(WideScope), loopback());
    w.add_folder("open", "Open").await;
    w.add_dashboard(dashboard("shown", "prom"), Some("open"))
        .await;
    {
        use daiya_model::DaiyaStore;
        let mut hidden = w
            .store
            .inner
            .get_folder(ORG, "open")
            .await
            .unwrap()
            .unwrap();
        hidden.uid = "hidden".into();
        hidden.title = "Hidden".into();
        w.store.inner.upsert_folder(hidden).await.unwrap();
        let mut secret = w
            .store
            .inner
            .get_dashboard(ORG, "shown")
            .await
            .unwrap()
            .unwrap();
        secret.uid = "secret".into();
        secret.title = "Secret".into();
        w.store
            .inner
            .upsert_dashboard(secret, "copied", true)
            .await
            .unwrap();
    }
    for uri in ["/odata/v4/Folders", "/odata/v4/Folders/$export"] {
        let (st, body) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{uri}: {body}");
        assert!(!body.to_string().contains("Hidden"), "{body}");
    }
    // Search lists what the folder list can answer and no dashboard of the disagreeing set.
    let hits = w.ok("GET", "/api/search", None).await;
    assert!(!hits.to_string().contains("Secret"), "{hits}");
    assert!(
        !hits.to_string().contains("shown"),
        "the whole set is withheld: {hits}"
    );
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
        ("read_rows", "GET", "/odata/v4/Dashboards"),
        ("read_rows", "GET", "/odata/v4/DashboardVersions"),
        ("read_rows", "GET", "/odata/v4/Folders"),
        ("read_rows", "GET", "/odata/v4/Datasources"),
        ("read_rows", "GET", "/odata/v4/LibraryPanels"),
        ("read_rows", "GET", "/odata/v4/PublicDashboards"),
        ("read_rows", "GET", "/odata/v4/ImportReports"),
        ("read_rows", "GET", "/odata/v4/Dashboards('edge')"),
        ("read_rows", "GET", "/odata/v4/Dashboards/$count"),
        ("read_rows", "GET", "/odata/v4/Dashboards/$export"),
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
    w.store.fail("read_rows");
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

/// A page ordered by a long title hands back a cursor of a few kilobytes; it is read back,
/// well inside the 32 KiB a `$skiptoken` may hold.
#[tokio::test]
async fn a_cursor_after_a_long_title_is_read_back() {
    use daiya_model::DaiyaStore;
    let w = world();
    let long = "t".repeat(1_500);
    let now = jc_time::TimestampTz::now();
    for i in 0..501 {
        w.store
            .inner
            .upsert_folder(daiya_model::FolderRecord {
                installation_id: daiya_model::InstallationId::from_u128(1),
                org_id: ORG,
                instance_id: daiya_model::InstanceId::from_u128(1),
                project_id: None,
                uid: format!("f{i:03}"),
                parent_uid: None,
                title: format!("{long}{i:03}"),
                version: 1,
                managed_by: None,
                created_by: "t".into(),
                updated_by: "t".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
    }
    let first = w.ok("GET", "/odata/v4/Folders?$orderby=title", None).await;
    let link = first["@odata.nextLink"].as_str().unwrap().to_owned();
    assert!(link.len() > 2_000, "{}", link.len());
    let (st, next) = w
        .call("GET", &format!("/odata/v4/{link}"), Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{next}");
    assert_eq!(next["value"][0]["uid"], "f500", "{next}");

    // The same cursor grown to exactly 32 KiB is read; one character more (base64 of one more
    // byte) is refused unread.
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let token = link.split_once("$skiptoken=").unwrap().1;
    let decoded = b64.decode(token).unwrap();
    let grown = |len: usize| {
        let mut cursor: Value = serde_json::from_slice(&decoded).unwrap();
        let title = cursor["after"][0].as_str().unwrap().to_owned();
        cursor["after"][0] = json!(format!("{title}{}", "z".repeat(len - decoded.len())));
        b64.encode(serde_json::to_vec(&cursor).unwrap())
    };
    let at_limit = grown(24_576);
    assert_eq!(at_limit.len(), 32 * 1024);
    let (st, body) = w
        .call(
            "GET",
            &format!("/odata/v4/Folders?$orderby=title&$skiptoken={at_limit}"),
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let over = grown(24_577);
    assert_eq!(over.len(), 32 * 1024 + 2);
    let (st, _) = w
        .call(
            "GET",
            &format!("/odata/v4/Folders?$orderby=title&$skiptoken={over}"),
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// An export stops at 10 000 rows and says how to narrow it, rather than holding a set of any
/// size in memory.
#[tokio::test]
async fn an_export_over_ten_thousand_rows_is_refused_with_how_to_narrow_it() {
    use daiya_model::DaiyaStore;
    let w = world();
    let now = jc_time::TimestampTz::now();
    for i in 0..10_001 {
        w.store
            .inner
            .upsert_folder(daiya_model::FolderRecord {
                installation_id: daiya_model::InstallationId::from_u128(1),
                org_id: ORG,
                instance_id: daiya_model::InstanceId::from_u128(1),
                project_id: None,
                uid: format!("f{i:05}"),
                parent_uid: None,
                title: format!("F{i}"),
                version: 1,
                managed_by: None,
                created_by: "t".into(),
                updated_by: "t".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
    }
    let (st, body) = w
        .call("GET", "/odata/v4/Folders/$export", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string().contains("narrow it with $filter"),
        "{body}"
    );
    let narrowed = w
        .ok(
            "GET",
            "/odata/v4/Folders/$export?$filter=uid%20eq%20'f00007'",
            None,
        )
        .await;
    assert_eq!(narrowed.as_array().unwrap().len(), 1);
    // Exactly the limit is still one export.
    let at_limit = w
        .ok(
            "GET",
            "/odata/v4/Folders/$export?$filter=uid%20ne%20'f00007'",
            None,
        )
        .await;
    assert_eq!(at_limit.as_array().unwrap().len(), 10_000);
}

#[tokio::test]
async fn paging_keys_cache_stats_and_refused_exports() {
    let w = seeded().await;
    // More rows than one page: a nextLink with a skiptoken the service accepts back.
    {
        use daiya_model::DaiyaStore;
        let now = jc_time::TimestampTz::now();
        for i in 0..501 {
            w.store
                .inner
                .upsert_folder(daiya_model::FolderRecord {
                    installation_id: daiya_model::InstallationId::from_u128(1),
                    org_id: ORG,
                    instance_id: daiya_model::InstanceId::from_u128(1),
                    project_id: None,
                    uid: format!("f{i:03}"),
                    parent_uid: None,
                    title: format!("F{i}"),
                    version: 1,
                    managed_by: None,
                    created_by: "t".into(),
                    updated_by: "t".into(),
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }
    }
    let first = w.ok("GET", "/odata/v4/Folders", None).await;
    assert_eq!(first["value"].as_array().unwrap().len(), 500);
    let next = first["@odata.nextLink"].as_str().unwrap();
    let rest = w.ok("GET", &format!("/odata/v4/{next}"), None).await;
    assert_eq!(rest["value"].as_array().unwrap().len(), 2, "501 + ops");

    // The next link keeps the options and starts behind the last row of that order.
    let asked = "Folders?$orderby=title%20desc&$select=uid&$count=true&$skip=1";
    let page = w.ok("GET", &format!("/odata/v4/{asked}"), None).await;
    assert_eq!(page["@odata.count"], 502);
    assert_eq!(page["value"].as_array().unwrap().len(), 500);
    let link = page["@odata.nextLink"].as_str().unwrap();
    assert!(link.starts_with(&format!("{asked}&$skiptoken=")), "{link}");
    let last = w.ok("GET", &format!("/odata/v4/{link}"), None).await;
    assert_eq!(
        last["value"],
        json!([{ "uid": "f000" }]),
        "titles descend to F0"
    );
    assert_eq!(
        last["@odata.count"], 502,
        "the count is the set's, on every page"
    );
    let top = w.ok("GET", "/odata/v4/Folders?$top=501", None).await;
    let link = top["@odata.nextLink"].as_str().unwrap();
    let end = w.ok("GET", &format!("/odata/v4/{link}"), None).await;
    assert_eq!(
        end["value"].as_array().unwrap().len(),
        1,
        "$top counts over pages"
    );
    assert!(end.get("@odata.nextLink").is_none(), "{end}");

    // A cursor carries no right: the caller's scope applies behind it too.
    let token = next.split_once("$skiptoken=").unwrap().1;
    let (st, seen) = w
        .call(
            "GET",
            &format!("/odata/v4/{next}"),
            Some("ops-viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        seen["value"].as_array().unwrap().len(),
        1,
        "ops alone: {seen}"
    );
    for uri in [
        format!("/odata/v4/Folders?$orderby=title&$skiptoken={token}"),
        format!("/odata/v4/Dashboards?$skiptoken={token}"),
        format!("/odata/v4/Folders?$skiptoken={token}&$skiptoken={token}"),
        "/odata/v4/Folders?$skiptoken=500".to_owned(),
        "/odata/v4/Folders?$filter=title%20eq%20'a%00b'".to_owned(),
        "/odata/v4/Folders?$filter=contains(updatedAt,'2026')".to_owned(),
    ] {
        let (st, _) = w.call("GET", &uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}");
    }

    // A time is compared as a time, whatever the literal's spelling.
    for (filter, count) in [
        ("updatedAt%20gt%202000-01-01T00:00:00Z", 502),
        ("updatedAt%20lt%202000-01-01T02:00:00%2B02:00", 0),
    ] {
        let uri = format!("/odata/v4/Folders/$count?$filter={filter}");
        assert_eq!(w.ok("GET", &uri, None).await, json!(count), "{filter}");
    }

    // `$batch` answers a set and a key as the routes do.
    let batch = w
        .ok(
            "POST",
            "/odata/v4/$batch",
            Some(json!({ "requests": [
                { "id": "1", "method": "GET", "url": "Folders?$count=true&$top=1" },
                { "id": "2", "method": "GET", "url": "Folders('f007')" },
            ] })),
        )
        .await;
    assert_eq!(
        batch["responses"][0]["body"]["@odata.count"], 502,
        "{batch}"
    );
    assert_eq!(batch["responses"][1]["body"]["uid"], "f007", "{batch}");

    // An export reads page after page by cursor and answers every row once.
    let export = w
        .ok("GET", "/odata/v4/Folders/$export?$select=uid", None)
        .await;
    let exported: Vec<&str> = export
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uid"].as_str().unwrap())
        .collect();
    assert_eq!(exported.len(), 502);
    assert_eq!(
        exported
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        502
    );

    // A cursor's values have the types of its order's properties: a null starts anywhere, a
    // number is an integer, a time is spelled as the rows spell it; anything else is foreign.
    {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let token = next.split_once("$skiptoken=").unwrap().1;
        let real: Value = serde_json::from_slice(&b64.decode(token).unwrap()).unwrap();
        let keyed = |set: &str, by: &str, after: Value| {
            let mut cursor = real.clone();
            cursor["set"] = json!(set);
            let mut order = vec![json!([by, false])];
            order.extend(real["order"].as_array().unwrap().iter().cloned());
            cursor["order"] = json!(order);
            let mut values = vec![after];
            values.extend(real["after"].as_array().unwrap().iter().cloned());
            cursor["after"] = json!(values);
            b64.encode(serde_json::to_vec(&cursor).unwrap())
        };
        for (set, by, after, want) in [
            ("Folders", "version", json!(1), StatusCode::OK),
            ("Folders", "version", json!(null), StatusCode::OK),
            ("Folders", "version", json!(1.5), StatusCode::BAD_REQUEST),
            ("Folders", "version", json!("1"), StatusCode::BAD_REQUEST),
            (
                "Folders",
                "updatedAt",
                json!("2026-11-19T00:00:00+00:00"),
                StatusCode::OK,
            ),
            (
                "Folders",
                "updatedAt",
                json!("2026-11-19"),
                StatusCode::BAD_REQUEST,
            ),
            ("Dashboards", "isProvisioned", json!(false), StatusCode::OK),
            (
                "Dashboards",
                "isProvisioned",
                json!("no"),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let uri = format!(
                "/odata/v4/{set}?$orderby={by}&$skiptoken={}",
                keyed(set, by, after.clone())
            );
            let (st, body) = w.call("GET", &uri, Some("admin"), None).await;
            assert_eq!(st, want, "{set} by {by} after {after}: {body}");
        }
    }

    // A skiptoken this service did not write is refused at every door.
    for uri in [
        "/odata/v4/Folders?$skiptoken=garbage",
        "/odata/v4/Folders/$count?$skiptoken=garbage",
        "/odata/v4/Folders/$export?$skiptoken=garbage",
    ] {
        let (st, _) = w.call("GET", uri, Some("admin"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{uri}");
    }
    let batch = w
        .ok(
            "POST",
            "/odata/v4/$batch",
            Some(json!({ "requests": [{ "id": "1", "method": "GET", "url": "Folders?$skiptoken=garbage" }] })),
        )
        .await;
    assert_eq!(batch["responses"][0]["status"], 400);

    // Key forms: `('k')` and `(uid='k')`; function arguments without a value are ignored.
    let by_name = w.ok("GET", "/odata/v4/Dashboards(uid='edge')", None).await;
    assert_eq!(by_name["uid"], "edge");
    let allowed = w.ok("GET", "/odata/v4/Allowed(junk)", None).await;
    assert_eq!(allowed["allowed"], true);

    // An export the caller may not read is the same 404 as an unknown dashboard.
    let (st, _) = w
        .call(
            "GET",
            "/odata/v4/Dashboards('loose')/Export",
            Some("ops-viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Query cache statistics: live entries of this organization and stored ones.
    w.ok(
        "POST",
        "/api/ds/query",
        Some(json!({
            "from": "1700000000000", "to": "1700000600000",
            "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
        })),
    )
    .await;
    let stats = w.ok("GET", "/odata/v4/QueryCacheStats", None).await;
    let rows = stats["value"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["datasourceUid"] == "prom" && r["entries"] == 1),
        "the live entry: {stats}"
    );
}
