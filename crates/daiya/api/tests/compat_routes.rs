#![forbid(unsafe_code)]
//! The Grafana HTTP compatibility routes (R2, R10a, COMPAT.md) answer as Grafana does: status
//! codes, hidden 404s across organizations and folders, and no stored write a Grafana server
//! would refuse.

mod common;

use common::{ORG, OTHER_ORG, dashboard, prometheus, world};
use http::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn health_answers_without_a_credential() {
    let w = world();
    let (st, body) = w.call("GET", "/api/health", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["database"], "ok");
}

#[tokio::test]
async fn organizations_are_created_by_the_installation_admin_and_seen_by_members() {
    let w = world();
    // Only the admin of the main organization creates organizations.
    let (st, _) = w
        .call(
            "POST",
            "/api/orgs",
            Some("editor"),
            Some(json!({ "name": "Helsinki" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = w
        .call(
            "POST",
            "/api/orgs",
            None,
            Some(json!({ "name": "Helsinki" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let created = w
        .ok("POST", "/api/orgs", Some(json!({ "name": "Helsinki" })))
        .await;
    assert_eq!(created["orgId"], 2);
    assert_eq!(created["message"], "Organization created");
    let (st, body) = w
        .call(
            "POST",
            "/api/orgs",
            Some("admin"),
            Some(json!({ "name": "Helsinki" })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");

    // Lookup by name: the installation admin sees it; a member of the main org sees its own.
    let found = w.ok("GET", "/api/orgs/name/Helsinki", None).await;
    assert_eq!(found["id"], 2);
    let (st, main) = w
        .call("GET", "/api/orgs/name/Main%20Org.", Some("viewer"), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(main["orgId"], 1);
    // A viewer of the main org is no member of Helsinki and no installation admin.
    let (st, _) = w
        .call("GET", "/api/orgs/name/Helsinki", Some("viewer"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", "/api/orgs/name/Nowhere", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", "/api/orgs/name/Main%20Org.", Some("outsider"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Current organization.
    let (st, org) = w.call("GET", "/api/org", Some("viewer"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(org["id"], 1);
    assert_eq!(org["name"], "Main Org.");
    let (st, _) = w.call("GET", "/api/org", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // A member of an organization the directory does not know reads it as "Main Org." 1.
    let (st, org) = w.call("GET", "/api/org", Some("outsider"), None).await;
    assert_eq!(st, StatusCode::OK, "{org}");
    assert_eq!(org["uuid"], json!(OTHER_ORG));
}

#[tokio::test]
async fn datasource_crud_answers_as_grafana() {
    let w = world();
    let (url, _) = prometheus().await;
    let added = w
        .ok(
            "POST",
            "/api/datasources",
            Some(json!({
                "uid": "prom", "name": "Prom", "type": "prometheus", "url": url,
                "isDefault": true, "jsonData": { "timeInterval": "15s" },
                "secretRefs": { "basicAuthPassword": "prom-pass" }
            })),
        )
        .await;
    assert_eq!(added["message"], "Datasource added");
    assert_eq!(added["datasource"]["uid"], "prom");
    assert_eq!(added["datasource"]["access"], "proxy");

    // Same uid, then same name: 409 as Grafana's ErrDataSourceUidExists / NameExists.
    let (st, _) = w
        .call(
            "POST",
            "/api/datasources",
            Some("admin"),
            Some(json!({ "uid": "prom", "name": "Other", "type": "prometheus" })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = w
        .call(
            "POST",
            "/api/datasources",
            Some("admin"),
            Some(json!({ "uid": "prom2", "name": "Prom", "type": "prometheus" })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    // A viewer cannot add one; an anonymous caller does not see the organization.
    let (st, _) = w
        .call(
            "POST",
            "/api/datasources",
            Some("viewer"),
            Some(json!({ "name": "V", "type": "prometheus" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let list = w.ok("GET", "/api/datasources", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["uid"], "prom");
    assert_eq!(list[0]["isDefault"], true);
    let (st, list) = w
        .call("GET", "/api/datasources", Some("outsider"), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list, json!([]));

    // Read: secret refs only as secureJsonFields booleans.
    let got = w.ok("GET", "/api/datasources/uid/prom", None).await;
    assert_eq!(got["secureJsonFields"]["basicAuthPassword"], true);
    assert!(!got.to_string().contains("prom-pass"), "{got}");
    assert_eq!(got["version"], 1);
    let (st, _) = w
        .call("GET", "/api/datasources/uid/prom", Some("outsider"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", "/api/datasources/uid/none", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Update an existing one.
    let updated = w
        .ok(
            "PUT",
            "/api/datasources/uid/prom",
            Some(json!({ "name": "Prom", "type": "prometheus", "url": url, "access": "direct" })),
        )
        .await;
    assert_eq!(updated["message"], "Datasource updated");
    assert_eq!(updated["datasource"]["access"], "direct");
    assert_eq!(updated["datasource"]["version"], 2);
    let (st, _) = w
        .call(
            "PUT",
            "/api/datasources/uid/prom",
            Some("viewer"),
            Some(json!({ "name": "Prom", "type": "prometheus" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Delete.
    let (st, _) = w
        .call("DELETE", "/api/datasources/uid/prom", Some("viewer"), None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let deleted = w.ok("DELETE", "/api/datasources/uid/prom", None).await;
    assert_eq!(deleted["message"], "Data source deleted");
    let (st, _) = w
        .call("DELETE", "/api/datasources/uid/prom", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Grafana `PUT /api/datasources/uid/:uid` loads the datasource first and answers 404
/// (`ErrDataSourceNotFound`) for an unknown uid; it never creates one.
#[tokio::test]
async fn updating_an_unknown_datasource_is_404_and_creates_nothing() {
    let w = world();
    let (st, body) = w
        .call(
            "PUT",
            "/api/datasources/uid/ghost",
            Some("admin"),
            Some(json!({ "name": "Ghost", "type": "prometheus", "url": "https://p.example" })),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(w.ok("GET", "/api/datasources", None).await, json!([]));
}

/// Raw credentials never enter the store through the API: `secureJsonData` is refused with
/// 400 naming `secretRefs`, on create and on update (§7.1, the same rule provisioning keeps).
#[tokio::test]
async fn raw_secure_json_data_is_refused_on_create_and_update() {
    let w = world();
    let (st, body) = w
        .call(
            "POST",
            "/api/datasources",
            Some("admin"),
            Some(json!({
                "uid": "pg", "name": "PG", "type": "postgres", "url": "db.example:5432",
                "secureJsonData": { "password": "hunter2" }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("secretRefs"), "{body}");
    assert!(!body.to_string().contains("hunter2"), "{body}");
    let (st, _) = w
        .call("GET", "/api/datasources/uid/pg", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "nothing was stored");

    // An empty secureJsonData carries no credential and is accepted.
    w.ok(
        "POST",
        "/api/datasources",
        Some(json!({
            "uid": "pg", "name": "PG", "type": "postgres", "url": "db.example:5432",
            "secureJsonData": {}
        })),
    )
    .await;
    let (st, body) = w
        .call(
            "PUT",
            "/api/datasources/uid/pg",
            Some("admin"),
            Some(json!({
                "name": "PG", "type": "postgres", "url": "db.example:5432",
                "secureJsonData": { "password": "hunter2" }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("secretRefs"), "{body}");
    let got = w.ok("GET", "/api/datasources/uid/pg", None).await;
    assert_eq!(got["version"], 1, "the refused update wrote nothing");
}

/// Grafana generates a short uid (`util.GenerateShortUID`) for a datasource or folder created
/// without one, and refuses a uid with characters outside `[a-zA-Z0-9-_]` or over 40.
#[tokio::test]
async fn a_missing_uid_is_generated_and_an_invalid_uid_refused() {
    let w = world();
    let added = w
        .ok(
            "POST",
            "/api/datasources",
            Some(json!({ "name": "Air Quality / Bratislava", "type": "prometheus" })),
        )
        .await;
    let uid = added["datasource"]["uid"].as_str().unwrap().to_owned();
    assert_ne!(uid, "Air Quality / Bratislava");
    assert!(is_short_uid(&uid), "{uid}");
    w.ok("GET", &format!("/api/datasources/uid/{uid}"), None)
        .await;
    let second = w
        .ok(
            "POST",
            "/api/datasources",
            Some(json!({ "uid": "", "name": "Second", "type": "prometheus" })),
        )
        .await;
    assert!(is_short_uid(second["datasource"]["uid"].as_str().unwrap()));
    assert_ne!(second["datasource"]["uid"], json!(uid));

    for bad in ["a/b", "has space", &"x".repeat(41)] {
        let (st, body) = w
            .call(
                "POST",
                "/api/datasources",
                Some("admin"),
                Some(
                    json!({ "uid": bad, "name": format!("n-{}", bad.len()), "type": "prometheus" }),
                ),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }

    let folder = w
        .ok(
            "POST",
            "/api/folders",
            Some(json!({ "title": "Ops / Night shift" })),
        )
        .await;
    let fuid = folder["uid"].as_str().unwrap();
    assert!(is_short_uid(fuid), "{fuid}");
    assert_eq!(folder["url"], format!("/dashboards/f/{fuid}"));
    let (st, _) = w
        .call(
            "POST",
            "/api/folders",
            Some("admin"),
            Some(json!({ "uid": "a b", "title": "Bad" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

fn is_short_uid(uid: &str) -> bool {
    (1..=40).contains(&uid.len())
        && uid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Grafana's AddDataSourceCommand / UpdateDataSourceCommand carry `readOnly` as `json:"-"`:
/// only provisioning marks a datasource read-only, and the API cannot update one so marked.
#[tokio::test]
async fn the_api_cannot_make_a_datasource_read_only_nor_change_a_provisioned_one() {
    let w = world();
    w.ok(
        "POST",
        "/api/datasources",
        Some(json!({ "uid": "ro", "name": "RO", "type": "prometheus", "readOnly": true })),
    )
    .await;
    let got = w.ok("GET", "/api/datasources/uid/ro", None).await;
    assert_eq!(got["readOnly"], false, "{got}");
    w.ok("DELETE", "/api/datasources/uid/ro", None).await;

    // A provisioned (read-only) record refuses an API update with 403.
    let now = chrono::Utc::now();
    use daiya_model::DaiyaStore;
    w.store
        .inner
        .upsert_datasource(daiya_model::DatasourceRecord {
            installation_id: daiya_model::InstallationId::from_u128(1),
            org_id: ORG,
            instance_id: daiya_model::InstanceId::from_u128(1),
            project_id: None,
            uid: "prov".into(),
            name: "Provisioned".into(),
            kind: "prometheus".into(),
            access: "proxy".into(),
            url: String::new(),
            is_default: false,
            read_only: true,
            json_data: json!({}),
            secret_refs: Default::default(),
            allow_hosts: vec![],
            wasm_digest: None,
            version: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    let (st, body) = w
        .call(
            "PUT",
            "/api/datasources/uid/prov",
            Some("admin"),
            Some(json!({ "name": "Provisioned", "type": "prometheus", "url": "https://evil.example" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    let got = w.ok("GET", "/api/datasources/uid/prov", None).await;
    assert_eq!(got["url"], "");
}

#[tokio::test]
async fn folders_list_read_and_delete_within_the_callers_folders() {
    let w = world();
    w.add_folder("ops", "Ops").await;
    w.add_folder("finance", "Finance").await;

    // Grafana answers 409 for a folder whose uid exists; POST never overwrites it.
    let (st, _) = w
        .call(
            "POST",
            "/api/folders",
            Some("admin"),
            Some(json!({ "uid": "ops", "title": "Renamed" })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let ops = w.ok("GET", "/api/folders/ops", None).await;
    assert_eq!(ops["title"], "Ops");
    assert_eq!(ops["version"], 1);

    let (st, _) = w
        .call(
            "POST",
            "/api/folders",
            Some("viewer"),
            Some(json!({ "title": "X" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let all = w.ok("GET", "/api/folders", None).await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (st, scoped) = w
        .call("GET", "/api/folders", Some("ops-viewer"), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(scoped.as_array().unwrap().len(), 1, "{scoped}");
    assert_eq!(scoped[0]["uid"], "ops");
    let (st, _) = w
        .call("GET", "/api/folders/finance", Some("ops-viewer"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", "/api/folders/nope", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // A nested folder keeps its parent.
    w.ok(
        "POST",
        "/api/folders",
        Some(json!({ "uid": "ops-night", "title": "Night", "parentUid": "ops" })),
    )
    .await;
    let night = w.ok("GET", "/api/folders/ops-night", None).await;
    assert_eq!(night["parentUid"], "ops");

    let (st, _) = w
        .call("DELETE", "/api/folders/finance", Some("viewer"), None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let del = w.ok("DELETE", "/api/folders/finance", None).await;
    assert_eq!(del["message"], "Folder deleted");
    let (st, _) = w
        .call("DELETE", "/api/folders/finance", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboards_save_read_version_search_and_delete() {
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_folder("ops", "Ops").await;
    w.add_folder("finance", "Finance").await;
    let saved = w
        .add_dashboard(dashboard("edge", "prom"), Some("ops"))
        .await;
    assert_eq!(saved["status"], "success");
    assert_eq!(saved["uid"], "edge");
    assert_eq!(saved["slug"], "board-edge");
    assert_eq!(saved["url"], "/d/edge/board-edge");
    assert_eq!(saved["folderUid"], "ops");
    w.add_dashboard(dashboard("money", "prom"), Some("finance"))
        .await;
    w.add_dashboard(dashboard("loose", "prom"), None).await;

    // A viewer cannot save.
    let (st, _) = w
        .call(
            "POST",
            "/api/dashboards/db",
            Some("viewer"),
            Some(json!({ "dashboard": dashboard("v", "prom") })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let got = w.ok("GET", "/api/dashboards/uid/edge", None).await;
    assert_eq!(got["meta"]["folderUid"], "ops");
    assert_eq!(got["meta"]["url"], "/d/edge/board-edge");
    assert_eq!(got["dashboard"]["uid"], "edge");
    let (st, _) = w
        .call("GET", "/api/dashboards/uid/money", Some("ops-viewer"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call("GET", "/api/dashboards/uid/none", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Versions follow the dashboard's own folder.
    let (st, vers) = w
        .call(
            "GET",
            "/api/dashboards/uid/edge/versions",
            Some("ops-viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{vers}");
    assert_eq!(vers["versions"].as_array().unwrap().len(), 1);
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/uid/money/versions",
            Some("ops-viewer"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/uid/none/versions",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Search: folders and dashboards the caller reads, filtered by query, tag and type.
    let all = w.ok("GET", "/api/search", None).await;
    assert_eq!(all.as_array().unwrap().len(), 5, "{all}");
    let dash = w
        .ok("GET", "/api/search?type=dash-db&query=EDGE", None)
        .await;
    assert_eq!(uids(&dash), ["edge"]);
    let folders = w
        .ok("GET", "/api/search?type=dash-folder&query=fin", None)
        .await;
    assert_eq!(uids(&folders), ["finance"]);
    let tagged = w.ok("GET", "/api/search?type=dash-db&tag=city", None).await;
    assert_eq!(tagged.as_array().unwrap().len(), 3);
    let none = w
        .ok("GET", "/api/search?tag=nothing&type=dash-db", None)
        .await;
    assert_eq!(none, json!([]));
    let (st, scoped) = w.call("GET", "/api/search", Some("ops-viewer"), None).await;
    assert_eq!(st, StatusCode::OK);
    let mut got = uids(&scoped);
    got.sort();
    assert_eq!(got, ["edge", "ops"]);

    // Delete.
    let (st, _) = w
        .call("DELETE", "/api/dashboards/uid/edge", Some("viewer"), None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let del = w.ok("DELETE", "/api/dashboards/uid/edge", None).await;
    assert_eq!(del["message"], "Dashboard Board edge deleted");
    let (st, _) = w
        .call("DELETE", "/api/dashboards/uid/edge", Some("admin"), None)
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

fn uids(hits: &Value) -> Vec<String> {
    hits.as_array()
        .unwrap()
        .iter()
        .map(|h| h["uid"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn ds_query_and_render_answer_through_the_stored_driver() {
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_dashboard(dashboard("edge", "prom"), None).await;

    let answer = w
        .ok(
            "POST",
            "/api/ds/query",
            Some(json!({
                "from": "1700000000000", "to": "1700000600000",
                "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
            })),
        )
        .await;
    assert_eq!(answer["results"]["A"]["status"], 200);
    assert_eq!(
        answer["results"]["A"]["frames"][0]["fields"][1]["values"],
        json!([41.0, 42.0])
    );
    let (st, body) = w
        .call(
            "POST",
            "/api/ds/query",
            Some("admin"),
            Some(json!({ "from": "2", "to": "1", "queries": [] })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    let view = w
        .ok(
            "GET",
            "/api/dashboards/edge/render?panel=1&from=1700000000000&to=1700000600000",
            None,
        )
        .await;
    assert_eq!(view["panelTitle"], "Edge");
    assert!(view["htmlTable"].as_str().unwrap().contains("<table"));
    let text = w
        .ok("GET", "/api/dashboards/edge/render?panel=2", None)
        .await;
    let html = text["htmlTable"].as_str().unwrap();
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(!html.contains("<script>"), "{html}");
    let (st, _) = w
        .call(
            "GET",
            "/api/dashboards/edge/render?panel=9",
            Some("admin"),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Every handler answers a store failure with the store's error (500 envelope), never a
/// success built on a missing read.
#[tokio::test]
async fn store_failures_surface_as_errors() {
    let w = world();
    let (url, _) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    w.add_folder("ops", "Ops").await;
    w.add_dashboard(dashboard("edge", "prom"), Some("ops"))
        .await;

    let cases: &[(&'static str, &str, &str, Option<Value>)] = &[
        ("list_datasources", "GET", "/api/datasources", None),
        ("get_datasource", "GET", "/api/datasources/uid/prom", None),
        (
            "delete_datasource",
            "DELETE",
            "/api/datasources/uid/prom",
            None,
        ),
        (
            "upsert_datasource",
            "POST",
            "/api/datasources",
            Some(json!({ "uid": "n", "name": "N", "type": "prometheus" })),
        ),
        (
            "get_datasource",
            "POST",
            "/api/datasources",
            Some(json!({ "uid": "n", "name": "N", "type": "prometheus" })),
        ),
        (
            "get_datasource_by_name",
            "POST",
            "/api/datasources",
            Some(json!({ "uid": "n", "name": "N", "type": "prometheus" })),
        ),
        (
            "upsert_datasource",
            "PUT",
            "/api/datasources/uid/prom",
            Some(json!({ "name": "Prom prom", "type": "prometheus" })),
        ),
        (
            "get_datasource",
            "PUT",
            "/api/datasources/uid/prom",
            Some(json!({ "name": "Prom prom", "type": "prometheus" })),
        ),
        ("list_folders", "GET", "/api/folders", None),
        ("get_folder", "GET", "/api/folders/ops", None),
        ("delete_folder", "DELETE", "/api/folders/ops", None),
        (
            "upsert_folder",
            "POST",
            "/api/folders",
            Some(json!({ "uid": "new", "title": "New" })),
        ),
        (
            "get_folder",
            "POST",
            "/api/folders",
            Some(json!({ "uid": "new", "title": "New" })),
        ),
        ("get_dashboard", "GET", "/api/dashboards/uid/edge", None),
        (
            "delete_dashboard",
            "DELETE",
            "/api/dashboards/uid/edge",
            None,
        ),
        ("get_dashboard", "DELETE", "/api/dashboards/uid/edge", None),
        (
            "list_dashboard_versions",
            "GET",
            "/api/dashboards/uid/edge/versions",
            None,
        ),
        (
            "get_dashboard",
            "GET",
            "/api/dashboards/uid/edge/versions",
            None,
        ),
        (
            "upsert_dashboard",
            "POST",
            "/api/dashboards/db",
            Some(json!({ "dashboard": dashboard("x", "prom") })),
        ),
        (
            "get_datasource",
            "POST",
            "/api/ds/query",
            Some(json!({ "queries": [{ "refId": "A", "datasource": { "uid": "prom" } }] })),
        ),
        ("get_dashboard", "GET", "/api/dashboards/edge/render", None),
    ];
    for (method_name, method, uri, body) in cases {
        w.store.fail(method_name);
        let (st, answer) = w.call(method, uri, Some("admin"), body.clone()).await;
        w.store.heal();
        assert_eq!(
            st,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method_name}: {method} {uri}: {answer}"
        );
        assert_eq!(answer["error"]["code"], "cr.internal_error", "{answer}");
    }

    // Search degrades to what the store could list, never to an error page.
    w.store.fail("list_folders");
    let hits = w.ok("GET", "/api/search", None).await;
    w.store.heal();
    assert_eq!(uids(&hits), ["edge"]);
}

/// A dashboard deleted (or a store failing) between the read decision and the export
/// answers the export's error, not a half-built answer.
#[tokio::test]
async fn a_failing_export_is_an_error() {
    let w = world();
    w.add_dashboard(dashboard("edge", "prom"), None).await;
    w.store.fail_after("get_dashboard", 1);
    let (st, body) = w
        .call("GET", "/api/dashboards/uid/edge", Some("admin"), None)
        .await;
    w.store.heal();
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

/// The render route takes Grafana's `var-<name>` in place of the dashboard's current value, so
/// the Portal's variable picker reaches the saved query; the value stays inside its literal.
#[tokio::test]
async fn render_takes_template_variables_from_the_address() {
    let w = world();
    let (url, seen) = prometheus().await;
    w.add_prometheus("prom", &url).await;
    let mut board = dashboard("vars", "prom");
    board["templating"] = json!({ "list": [
        { "name": "job", "type": "custom", "current": { "value": "api" } }
    ]});
    board["panels"][0]["targets"][0]["expr"] = json!("up{job=\"$job\"}");
    w.add_dashboard(board, None).await;

    w.ok("GET", "/api/dashboards/vars/render?panel=1", None)
        .await;
    w.ok(
        "GET",
        "/api/dashboards/vars/render?panel=1&var-job=edge%22%7D&maxDataPoints=50",
        None,
    )
    .await;
    let seen = seen.lock().clone();
    let queries: Vec<&String> = seen.iter().filter(|q| q.contains("query_range")).collect();
    assert_eq!(queries.len(), 2, "{seen:?}");
    assert!(queries[0].contains("api"), "{}", queries[0]);
    assert!(
        queries[1].contains("edge%5C%22%7D") || queries[1].contains("edge\\\"}"),
        "the quote is escaped inside its literal: {}",
        queries[1]
    );
}
