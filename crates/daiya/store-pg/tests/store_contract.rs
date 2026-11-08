#![forbid(unsafe_code)]
//! One behaviour contract for every `DaiyaStore`: the memory store always, the Postgres store
//! (on the application login role, so forced RLS applies) when `DAIYA_TEST_DATABASE_URL` is
//! set, with a `SKIP` line otherwise (R8). Postgres-only checks follow: RLS seen through raw
//! SQL, the public-token policy, the boot check, the pool configuration and persistence.

use chrono::{Duration as ChronoDuration, Utc};
use daiya_model::{
    DaiyaStore, DashboardRecord, DatasourceRecord, FolderRecord, ImportReportRecord,
    InstallationId, InstanceId, LibraryPanelRecord, OrgId, OrgQuotas, PublicDashboardRecord,
    QueryCacheStatRecord, ShareType, sha256_hex,
};
use daiya_store_memory::MemoryStore;
use daiya_store_pg::{
    PgPoolConfig, PgStore, TestDatabase, connect_test_database, parse_url_without_password,
};
use serde_json::json;
use sqlx::Row;
use std::collections::BTreeMap;
use tokio::sync::RwLock;

/// Tests that create a table in the shared schema take this lock for writing, every other
/// Postgres test for reading, so a rogue table never shows up in another test's boot check.
static SCHEMA_LOCK: RwLock<()> = RwLock::const_new(());

const INST: InstallationId = InstallationId::from_u128(7);
const INSTANCE: InstanceId = InstanceId::from_u128(9);

fn folder(org: OrgId, uid: &str, parent: Option<&str>) -> FolderRecord {
    let now = Utc::now();
    FolderRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        project_id: Some("proj".into()),
        uid: uid.into(),
        parent_uid: parent.map(str::to_owned),
        title: format!("Folder {uid}"),
        version: 1,
        managed_by: None,
        created_by: "alice".into(),
        updated_by: "alice".into(),
        created_at: now,
        updated_at: now,
    }
}

fn dashboard(org: OrgId, uid: &str, folder_uid: Option<&str>) -> DashboardRecord {
    let now = Utc::now();
    DashboardRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        project_id: None,
        uid: uid.into(),
        folder_uid: folder_uid.map(str::to_owned),
        title: format!("Dash {uid}"),
        slug: format!("dash-{uid}"),
        tags: vec!["a".into(), "b".into()],
        panel_count: 2,
        datasource_uids: vec!["prom".into()],
        version: 0,
        schema_version: 42,
        is_provisioned: false,
        provisioned_external_id: None,
        allow_ui_updates: true,
        data: json!({ "title": format!("Dash {uid}"), "schemaVersion": 42, "panels": [] }),
        created_by: "alice".into(),
        updated_by: "alice".into(),
        created_at: now,
        updated_at: now,
    }
}

fn datasource(org: OrgId, uid: &str, name: &str, kind: &str) -> DatasourceRecord {
    let now = Utc::now();
    DatasourceRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        project_id: None,
        uid: uid.into(),
        name: name.into(),
        kind: kind.into(),
        access: "proxy".into(),
        url: "https://prom.example.com".into(),
        is_default: false,
        read_only: false,
        json_data: json!({ "timeInterval": "15s" }),
        secret_refs: BTreeMap::from([("token".to_owned(), "prom-token".to_owned())]),
        allow_hosts: vec!["prom.example.com".into()],
        wasm_digest: None,
        version: 1,
        created_at: now,
        updated_at: now,
    }
}

fn public_dashboard(
    org: OrgId,
    uid: &str,
    dashboard_uid: &str,
    token: &str,
) -> PublicDashboardRecord {
    let now = Utc::now();
    PublicDashboardRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        uid: uid.into(),
        dashboard_uid: dashboard_uid.into(),
        access_token_hash: sha256_hex(token.as_bytes()),
        access_token: Some(token.into()),
        is_enabled: true,
        time_selection_enabled: false,
        annotations_enabled: false,
        show_query_text: false,
        share: ShareType::Email,
        created_by: "alice".into(),
        updated_by: "alice".into(),
        created_at: now,
        updated_at: now,
    }
}

async fn contract_quotas(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    assert_eq!(store.get_quotas(org).await.unwrap(), OrgQuotas::default());
    let custom = OrgQuotas {
        max_dashboards: 3,
        max_public_range_seconds: 3600,
        min_public_interval_ms: 30_000,
        ..OrgQuotas::default()
    };
    store.set_quotas(org, custom.clone()).await.unwrap();
    assert_eq!(store.get_quotas(org).await.unwrap(), custom);
    // A second set replaces the first.
    store.set_quotas(org, OrgQuotas::default()).await.unwrap();
    assert_eq!(store.get_quotas(org).await.unwrap(), OrgQuotas::default());
}

async fn contract_folders(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    let created = store.upsert_folder(folder(org, "f-b", None)).await.unwrap();
    assert_eq!(created.version, 1);
    assert_eq!(created.project_id.as_deref(), Some("proj"));
    store.upsert_folder(folder(org, "f-a", None)).await.unwrap();

    let mut renamed = folder(org, "f-b", None);
    renamed.title = "Renamed".into();
    renamed.updated_by = "bob".into();
    let updated = store.upsert_folder(renamed).await.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.title, "Renamed");
    assert_eq!(updated.updated_by, "bob");
    assert_eq!(updated.created_by, "alice");

    let uids: Vec<String> = store
        .list_folders(org)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.uid)
        .collect();
    assert_eq!(uids, ["f-a", "f-b"], "folders are listed by uid");
    assert_eq!(
        store.get_folder(org, "f-b").await.unwrap().unwrap().title,
        "Renamed"
    );
    assert!(store.get_folder(org, "missing").await.unwrap().is_none());

    // The reserved General folder cannot be written.
    let err = store
        .upsert_folder(folder(org, "general", None))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 400);

    // Nesting: four levels are fine, a fifth is refused; a folder cannot be its own parent and
    // a chain cannot loop.
    store.upsert_folder(folder(org, "l1", None)).await.unwrap();
    store
        .upsert_folder(folder(org, "l2", Some("l1")))
        .await
        .unwrap();
    store
        .upsert_folder(folder(org, "l3", Some("l2")))
        .await
        .unwrap();
    store
        .upsert_folder(folder(org, "l4", Some("l3")))
        .await
        .unwrap();
    let too_deep = store
        .upsert_folder(folder(org, "l5", Some("l4")))
        .await
        .unwrap_err();
    assert!(too_deep.to_string().contains("depth"), "{too_deep}");
    let own_parent = store
        .upsert_folder(folder(org, "l1", Some("l1")))
        .await
        .unwrap_err();
    assert!(own_parent.to_string().contains("cyclic"), "{own_parent}");
    let looped = store
        .upsert_folder(folder(org, "l1", Some("l3")))
        .await
        .unwrap_err();
    assert!(looped.to_string().contains("cyclic"), "{looped}");

    // Deleting a folder deletes its dashboards; deleting twice answers false.
    store
        .upsert_dashboard(dashboard(org, "in-folder", Some("f-a")), "", true)
        .await
        .unwrap();
    store
        .upsert_dashboard(dashboard(org, "elsewhere", Some("f-b")), "", true)
        .await
        .unwrap();
    assert!(store.delete_folder(org, "f-a").await.unwrap());
    assert!(!store.delete_folder(org, "f-a").await.unwrap());
    assert!(
        store
            .get_dashboard(org, "in-folder")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_dashboard(org, "elsewhere")
            .await
            .unwrap()
            .is_some()
    );
    assert!(!store.delete_folder(OrgId::new(), "f-b").await.unwrap());
}

async fn contract_dashboards(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    let saved = store
        .upsert_dashboard(dashboard(org, "d1", None), "", false)
        .await
        .unwrap();
    assert_eq!(saved.version, 1);
    assert_eq!(saved.data["version"], 1);
    assert_eq!(saved.data["uid"], "d1");
    assert_eq!(saved.tags, ["a", "b"]);
    assert_eq!(saved.datasource_uids, ["prom"]);

    // A stale version without overwrite is a precondition failure, the current one saves.
    let mut stale = dashboard(org, "d1", None);
    stale.version = 7;
    let err = store.upsert_dashboard(stale, "", false).await.unwrap_err();
    assert_eq!(err.status_code(), 412);
    let mut current = dashboard(org, "d1", None);
    current.version = 1;
    current.title = "Second".into();
    let second = store
        .upsert_dashboard(current, "edited title", false)
        .await
        .unwrap();
    assert_eq!(second.version, 2);
    assert_eq!(second.title, "Second");
    assert_eq!(second.data["version"], 2);

    // Overwrite ignores the version.
    let mut forced = dashboard(org, "d1", None);
    forced.version = 99;
    assert_eq!(
        store
            .upsert_dashboard(forced, "", true)
            .await
            .unwrap()
            .version,
        3
    );

    let versions = store.list_dashboard_versions(org, "d1").await.unwrap();
    let summary: Vec<(i32, i32, &str)> = versions
        .iter()
        .map(|v| (v.version, v.parent_version, v.message.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (1, 0, "Initial save"),
            (2, 1, "edited title"),
            (3, 2, "Updated dashboard")
        ]
    );
    assert_eq!(versions[1].data["version"], 2);
    assert_eq!(versions[1].created_by, "alice");

    // A provisioned dashboard that does not allow UI updates refuses a UI save, takes a
    // provisioning save.
    let mut provisioned = dashboard(org, "prov", None);
    provisioned.is_provisioned = true;
    provisioned.allow_ui_updates = false;
    provisioned.provisioned_external_id = Some("dashboards/prov.json".into());
    store
        .upsert_dashboard(provisioned.clone(), "", true)
        .await
        .unwrap();
    let ui_save = store
        .upsert_dashboard(dashboard(org, "prov", None), "", true)
        .await
        .unwrap_err();
    assert_eq!(ui_save.status_code(), 403);
    let reprovisioned = store.upsert_dashboard(provisioned, "", true).await.unwrap();
    assert_eq!(reprovisioned.version, 2);
    assert_eq!(
        reprovisioned.provisioned_external_id.as_deref(),
        Some("dashboards/prov.json")
    );

    // Quotas: panels per dashboard and dashboards per organization.
    store
        .set_quotas(
            org,
            OrgQuotas {
                max_dashboards: 2,
                max_panels_per_dashboard: 5,
                ..OrgQuotas::default()
            },
        )
        .await
        .unwrap();
    let mut wide = dashboard(org, "d1", None);
    wide.panel_count = 6;
    let err = store.upsert_dashboard(wide, "", true).await.unwrap_err();
    assert_eq!(err.slug(), "cr.quota_exceeded");
    let err = store
        .upsert_dashboard(dashboard(org, "third", None), "", true)
        .await
        .unwrap_err();
    assert_eq!(err.slug(), "cr.quota_exceeded");
    assert!(err.to_string().contains("dashboards"));

    // Delete removes the dashboard, its history and its public dashboard.
    store
        .upsert_public_dashboard(public_dashboard(org, "pd-d1", "d1", "tok-d1"))
        .await
        .unwrap();
    assert!(store.delete_dashboard(org, "d1").await.unwrap());
    assert!(!store.delete_dashboard(org, "d1").await.unwrap());
    assert!(store.get_dashboard(org, "d1").await.unwrap().is_none());
    assert!(
        store
            .list_dashboard_versions(org, "d1")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .get_public_dashboard(org, "pd-d1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(!store.delete_dashboard(OrgId::new(), "prov").await.unwrap());
    let uids: Vec<String> = store
        .list_dashboards(org)
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.uid)
        .collect();
    assert_eq!(uids, ["prov"]);
}

async fn contract_datasources(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    let mut first = datasource(org, "uid-z", "Alpha", "prometheus");
    first.is_default = true;
    let saved = store.upsert_datasource(first).await.unwrap();
    assert_eq!(saved.version, 1);
    assert_eq!(saved.secret_refs["token"], "prom-token");
    assert_eq!(saved.allow_hosts, ["prom.example.com"]);
    assert_eq!(saved.json_data["timeInterval"], "15s");

    let mut second = datasource(org, "uid-a", "Beta", "ngsild");
    second.is_default = true;
    store.upsert_datasource(second).await.unwrap();
    let all = store.list_datasources(org).await.unwrap();
    let names: Vec<(&str, bool)> = all
        .iter()
        .map(|d| (d.name.as_str(), d.is_default))
        .collect();
    assert_eq!(
        names,
        [("Alpha", false), ("Beta", true)],
        "listed by name, one default per organization"
    );

    // Update bumps the version; the name stays unique within the organization.
    let mut renamed = datasource(org, "uid-z", "Alpha 2", "prometheus");
    renamed.url = "https://prom2.example.com".into();
    let updated = store.upsert_datasource(renamed).await.unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.url, "https://prom2.example.com");
    let clash = store
        .upsert_datasource(datasource(org, "uid-new", "Beta", "prometheus"))
        .await
        .unwrap_err();
    assert_eq!(clash.status_code(), 409);
    // The same name in another organization is fine.
    store
        .upsert_datasource(datasource(OrgId::new(), "uid-new", "Beta", "prometheus"))
        .await
        .unwrap();

    assert_eq!(
        store
            .get_datasource_by_name(org, "Beta")
            .await
            .unwrap()
            .unwrap()
            .uid,
        "uid-a"
    );
    assert!(
        store
            .get_datasource_by_name(org, "Nope")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_datasource(org, "nope").await.unwrap().is_none());

    // Read-only datasources cannot be deleted.
    let mut ro = datasource(org, "uid-ro", "Provisioned", "prometheus");
    ro.read_only = true;
    store.upsert_datasource(ro).await.unwrap();
    let err = store.delete_datasource(org, "uid-ro").await.unwrap_err();
    assert_eq!(err.status_code(), 403);
    assert!(store.delete_datasource(org, "uid-a").await.unwrap());
    assert!(!store.delete_datasource(org, "uid-a").await.unwrap());

    // Quotas: datasources and WASM plugins.
    store
        .set_quotas(
            org,
            OrgQuotas {
                max_datasources: 4,
                max_wasm_plugins: 1,
                ..OrgQuotas::default()
            },
        )
        .await
        .unwrap();
    let mut wasm = datasource(org, "w1", "Plugin 1", "wasm:sample");
    wasm.wasm_digest = Some("sha256:abc".into());
    assert_eq!(
        store
            .upsert_datasource(wasm)
            .await
            .unwrap()
            .wasm_digest
            .as_deref(),
        Some("sha256:abc")
    );
    let err = store
        .upsert_datasource(datasource(org, "w2", "Plugin 2", "wasm:sample"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("wasm_plugins"), "{err}");
    store
        .upsert_datasource(datasource(org, "p4", "Fourth", "prometheus"))
        .await
        .unwrap();
    let err = store
        .upsert_datasource(datasource(org, "p5", "Fifth", "prometheus"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("datasources"), "{err}");
    assert!(!store.delete_datasource(OrgId::new(), "p4").await.unwrap());
}

async fn contract_library_panels(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    let now = Utc::now();
    let mut panel = LibraryPanelRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        folder_uid: Some("f".into()),
        uid: "lp".into(),
        name: "CPU".into(),
        panel_type: "timeseries".into(),
        description: "cpu use".into(),
        model: json!({ "type": "timeseries", "title": "CPU" }),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    assert_eq!(
        store
            .upsert_library_panel(panel.clone())
            .await
            .unwrap()
            .version,
        1
    );
    panel.name = "CPU 2".into();
    let second = store.upsert_library_panel(panel).await.unwrap();
    assert_eq!((second.version, second.name.as_str()), (2, "CPU 2"));
    assert_eq!(store.list_library_panels(org).await.unwrap().len(), 1);
    let got = store.get_library_panel(org, "lp").await.unwrap().unwrap();
    assert_eq!(got.model["title"], "CPU");
    assert_eq!(got.description, "cpu use");
    assert!(store.get_library_panel(org, "x").await.unwrap().is_none());
    assert!(store.delete_library_panel(org, "lp").await.unwrap());
    assert!(!store.delete_library_panel(org, "lp").await.unwrap());
    assert!(
        !store
            .delete_library_panel(OrgId::new(), "lp")
            .await
            .unwrap()
    );
}

async fn contract_public_dashboards(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    // A public dashboard needs its dashboard.
    let err = store
        .upsert_public_dashboard(public_dashboard(org, "pd", "absent", "t0"))
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 404);

    store
        .upsert_dashboard(dashboard(org, "dash", None), "", true)
        .await
        .unwrap();
    let token = format!("token-{}", uuid::Uuid::new_v4());
    let created = store
        .upsert_public_dashboard(public_dashboard(org, "pd", "dash", &token))
        .await
        .unwrap();
    // The creator gets the raw token once; reads never return it.
    assert_eq!(created.access_token.as_deref(), Some(token.as_str()));
    assert_eq!(created.share, ShareType::Email);
    let read = store
        .get_public_dashboard(org, "pd")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.access_token, None);
    assert_eq!(read.access_token_hash, sha256_hex(token.as_bytes()));
    assert!(read.is_enabled);
    assert_eq!(
        store
            .get_public_dashboard_by_dashboard_uid(org, "dash")
            .await
            .unwrap()
            .unwrap()
            .uid,
        "pd"
    );
    assert!(
        store
            .get_public_dashboard_by_dashboard_uid(org, "other")
            .await
            .unwrap()
            .is_none()
    );
    let listed = store.list_public_dashboards(org).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].access_token.is_none());

    // The anonymous door finds the dashboard by the token hash alone.
    let (pd, dash) = store
        .get_public_dashboard_by_token_hash(&sha256_hex(token.as_bytes()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((pd.uid.as_str(), dash.uid.as_str()), ("pd", "dash"));
    assert_eq!(dash.org_id, org);
    assert!(
        store
            .get_public_dashboard_by_token_hash(&sha256_hex(b"unknown"))
            .await
            .unwrap()
            .is_none()
    );

    // Rotation replaces the hash: the old token no longer finds anything.
    let rotated_token = format!("rotated-{}", uuid::Uuid::new_v4());
    let mut rotated = public_dashboard(org, "pd", "dash", &rotated_token);
    rotated.is_enabled = false;
    rotated.share = ShareType::Public;
    let after = store.upsert_public_dashboard(rotated).await.unwrap();
    assert!(!after.is_enabled);
    assert_eq!(after.share, ShareType::Public);
    assert!(
        store
            .get_public_dashboard_by_token_hash(&sha256_hex(token.as_bytes()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_public_dashboard_by_token_hash(&sha256_hex(rotated_token.as_bytes()))
            .await
            .unwrap()
            .is_some()
    );

    assert!(store.delete_public_dashboard(org, "pd").await.unwrap());
    assert!(!store.delete_public_dashboard(org, "pd").await.unwrap());
    assert!(
        !store
            .delete_public_dashboard(OrgId::new(), "pd")
            .await
            .unwrap()
    );
}

async fn contract_stats_and_reports(store: &dyn DaiyaStore) {
    let org = OrgId::new();
    let mut stat = QueryCacheStatRecord {
        id: "prom".into(),
        org_id: org,
        instance_id: INSTANCE,
        datasource_uid: "prom".into(),
        hits: 1,
        misses: 2,
        entries: 3,
        bytes_used: 4,
    };
    store.upsert_query_cache_stat(stat.clone()).await.unwrap();
    stat.hits = 10;
    store.upsert_query_cache_stat(stat.clone()).await.unwrap();
    assert_eq!(store.list_query_cache_stats(org).await.unwrap(), vec![stat]);
    assert!(
        store
            .list_query_cache_stats(OrgId::new())
            .await
            .unwrap()
            .is_empty()
    );

    let base = Utc::now() - ChronoDuration::minutes(5);
    for (i, id) in ["older", "newer"].into_iter().enumerate() {
        store
            .insert_import_report(ImportReportRecord {
                id: id.into(),
                org_id: org,
                instance_id: INSTANCE,
                dry_run: i == 0,
                status: "ok".into(),
                dashboard_count: 9,
                refused_count: 0,
                mapped_count: 1,
                summary: json!({ "n": i }),
                created_at: base + ChronoDuration::minutes(i as i64),
            })
            .await
            .unwrap();
    }
    let reports = store.list_import_reports(org).await.unwrap();
    let ids: Vec<&str> = reports.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["newer", "older"], "newest report first");
    assert!(reports[1].dry_run);
    assert_eq!(reports[0].summary["n"], 1);
}

async fn run_contract(store: &dyn DaiyaStore) {
    store.verify_rls_enforced().await.unwrap();
    contract_quotas(store).await;
    contract_folders(store).await;
    contract_dashboards(store).await;
    contract_datasources(store).await;
    contract_library_panels(store).await;
    contract_public_dashboards(store).await;
    contract_stats_and_reports(store).await;
}

#[tokio::test]
async fn store_contract_holds_on_the_memory_store() {
    run_contract(&MemoryStore::new()).await;
}

async fn pg() -> Option<TestDatabase> {
    let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return None;
    };
    Some(connect_test_database(&url).await.expect("test database"))
}

#[tokio::test]
async fn store_contract_holds_on_postgres_through_the_app_role() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    run_contract(&db.app).await;
}

/// Runs `sql` in one transaction of the application role with `cr.org` (and optionally
/// `cr.public_token`) set, answering the number of rows the role can see.
async fn visible_rows(
    db: &TestDatabase,
    table: &str,
    org: Option<OrgId>,
    public_token_hash: Option<&str>,
) -> i64 {
    let mut tx = db.app.pool().begin().await.unwrap();
    if let Some(org) = org {
        sqlx::query("SELECT set_config('cr.org', $1, true)")
            .bind(org.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    if let Some(hash) = public_token_hash {
        sqlx::query("SELECT set_config('cr.public_token', $1, true)")
            .bind(hash)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    n
}

#[tokio::test]
async fn forced_rls_hides_other_organizations_from_raw_sql_of_the_app_role() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let (org_a, org_b) = (OrgId::new(), OrgId::new());
    for org in [org_a, org_b] {
        db.app
            .upsert_dashboard(dashboard(org, "same-uid", None), "", true)
            .await
            .unwrap();
        db.app
            .upsert_datasource(datasource(org, "prom", "Prometheus", "prometheus"))
            .await
            .unwrap();
    }
    db.app
        .upsert_dashboard(dashboard(org_b, "only-b", None), "", true)
        .await
        .unwrap();

    // No WHERE clause: the policy alone narrows to the transaction's organization.
    assert_eq!(
        visible_rows(&db, "cr_dashboard", Some(org_a), None).await,
        1
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard", Some(org_b), None).await,
        2
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard_version", Some(org_b), None).await,
        2
    );
    assert_eq!(
        visible_rows(&db, "cr_datasource", Some(org_a), None).await,
        1
    );
    // Without an organization the role sees nothing at all.
    for table in daiya_store_pg::REQUIRED_RLS_TABLES {
        assert_eq!(visible_rows(&db, table, None, None).await, 0, "{table}");
    }

    // Writing a row of B while the transaction is A's breaks the WITH CHECK clause, and an
    // UPDATE or DELETE aimed at B's rows touches nothing.
    let mut tx = db.app.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('cr.org', $1, true)")
        .bind(org_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let updated = sqlx::query("UPDATE cr_dashboard SET title = 'pwned' WHERE org_id = $1")
        .bind(org_b.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(updated.rows_affected(), 0);
    let deleted = sqlx::query("DELETE FROM cr_datasource WHERE org_id = $1")
        .bind(org_b.as_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(deleted.rows_affected(), 0);
    let insert = sqlx::query(
        "INSERT INTO cr_folder (installation_id, org_id, instance_id, uid, title) VALUES ($1, $2, $3, 'x', 'x')",
    )
    .bind(INST.as_uuid())
    .bind(org_b.as_uuid())
    .bind(INSTANCE.as_uuid())
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(
        insert.to_string().contains("row-level security"),
        "{insert}"
    );
    drop(tx);
    assert_eq!(
        db.app
            .get_dashboard(org_b, "same-uid")
            .await
            .unwrap()
            .unwrap()
            .title,
        "Dash same-uid"
    );
}

#[tokio::test]
async fn public_token_policy_shows_exactly_one_row_and_never_allows_writes() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let org = OrgId::new();
    let other = OrgId::new();
    let token = format!("pub-{}", uuid::Uuid::new_v4());
    let hash = sha256_hex(token.as_bytes());
    for (o, uid, tok) in [
        (org, "pd-1", token.clone()),
        (other, "pd-2", format!("x-{token}")),
    ] {
        db.app
            .upsert_dashboard(dashboard(o, "pub-dash", None), "", true)
            .await
            .unwrap();
        db.app
            .upsert_public_dashboard(public_dashboard(o, uid, "pub-dash", &tok))
            .await
            .unwrap();
    }

    // Token known, organization unknown: exactly the row of that token, and nothing else in
    // any other table.
    assert_eq!(
        visible_rows(&db, "cr_public_dashboard", None, Some(&hash)).await,
        1
    );
    assert_eq!(
        visible_rows(&db, "cr_dashboard", None, Some(&hash)).await,
        0
    );
    assert_eq!(
        visible_rows(&db, "cr_public_dashboard", None, Some("not-a-hash")).await,
        0
    );
    assert_eq!(
        visible_rows(&db, "cr_public_dashboard", None, Some("")).await,
        0
    );

    // The token policy is SELECT only: an UPDATE or DELETE through it touches nothing.
    let mut tx = db.app.pool().begin().await.unwrap();
    sqlx::query("SELECT set_config('cr.public_token', $1, true)")
        .bind(&hash)
        .execute(&mut *tx)
        .await
        .unwrap();
    let upd = sqlx::query(
        "UPDATE cr_public_dashboard SET show_query_text = TRUE WHERE access_token_hash = $1",
    )
    .bind(&hash)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(upd.rows_affected(), 0);
    let del = sqlx::query("DELETE FROM cr_public_dashboard WHERE access_token_hash = $1")
        .bind(&hash)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(del.rows_affected(), 0);
    tx.rollback().await.unwrap();

    // Through the store: the anonymous lookup returns the dashboard of the right organization.
    let (pd, dash) = db
        .app
        .get_public_dashboard_by_token_hash(&hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((pd.org_id, dash.org_id), (org, org));
    assert!(!pd.show_query_text);

    // A second public dashboard for the same dashboard is a conflict, not a 500.
    let dup = db
        .app
        .upsert_public_dashboard(public_dashboard(org, "pd-dup", "pub-dash", "dup-token"))
        .await
        .unwrap_err();
    assert_eq!(dup.status_code(), 409);
}

#[tokio::test]
async fn data_survives_a_new_store_instance() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let org = OrgId::new();
    db.app
        .upsert_dashboard(dashboard(org, "durable", None), "first", true)
        .await
        .unwrap();
    db.app
        .upsert_datasource(datasource(org, "durable-ds", "Durable", "prometheus"))
        .await
        .unwrap();
    db.app.pool().close().await;

    let reopened = PgStore::new(db.app_config.connect().await.unwrap());
    reopened.verify_rls_enforced().await.unwrap();
    let dash = reopened
        .get_dashboard(org, "durable")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dash.version, 1);
    assert_eq!(dash.data["uid"], "durable");
    assert_eq!(
        reopened
            .list_dashboard_versions(org, "durable")
            .await
            .unwrap()[0]
            .message,
        "first"
    );
    assert_eq!(
        reopened
            .get_datasource(org, "durable-ds")
            .await
            .unwrap()
            .unwrap()
            .secret_refs["token"],
        "prom-token"
    );
}

#[tokio::test]
async fn boot_check_refuses_superuser_set_role_and_unforced_tables() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // The application role passes.
    db.app.verify_rls_enforced().await.unwrap();

    // The migrating superuser does not.
    let admin_store = PgStore::new(db.admin.clone());
    let err = admin_store.verify_rls_enforced().await.unwrap_err();
    assert_eq!(err.slug(), "cr.rls_not_enforced");

    // Nor does a superuser login that switched to the application role: it could switch back.
    let url = std::env::var("DAIYA_TEST_DATABASE_URL").unwrap();
    let set_role_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE daiya_app")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let current: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&set_role_pool)
        .await
        .unwrap();
    assert_eq!(current, "daiya_app");
    let err = PgStore::new(set_role_pool)
        .verify_rls_enforced()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("SUPERUSER") || err.to_string().contains("BYPASSRLS"));

    // A cr_* table without forced RLS is refused by name, for the application role too.
    sqlx::query("CREATE TABLE IF NOT EXISTS cr_test_unforced (org_id UUID)")
        .execute(&db.admin)
        .await
        .unwrap();
    let unforced = db.app.verify_rls_enforced().await;
    sqlx::query("ALTER TABLE cr_test_unforced ENABLE ROW LEVEL SECURITY")
        .execute(&db.admin)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE cr_test_unforced FORCE ROW LEVEL SECURITY")
        .execute(&db.admin)
        .await
        .unwrap();
    let no_policy = db.app.verify_rls_enforced().await;
    sqlx::query("DROP TABLE IF EXISTS cr_test_unforced")
        .execute(&db.admin)
        .await
        .unwrap();
    let err = unforced.unwrap_err();
    assert!(err.to_string().contains("cr_test_unforced"), "{err}");
    let err = no_policy.unwrap_err();
    assert!(err.to_string().contains("cr_test_unforced"), "{err}");
    db.app.verify_rls_enforced().await.unwrap();
}

#[tokio::test]
async fn migration_is_idempotent_and_runs_concurrently() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // A recorded checksum that no longer matches makes the next boot re-run the DDL once.
    sqlx::query("UPDATE daiya_schema_migrations SET checksum = 'stale' WHERE version = 1")
        .execute(&db.admin)
        .await
        .unwrap();
    let a = daiya_store_pg::migrate(&db.admin);
    let b = daiya_store_pg::migrate(&db.admin);
    let (ra, rb) = tokio::join!(a, b);
    ra.unwrap();
    rb.unwrap();
    let policies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid WHERE c.relname = 'cr_public_dashboard'",
    )
    .fetch_one(&db.admin)
    .await
    .unwrap();
    assert_eq!(policies, 2, "org_isolation and public_token_lookup");
    let checksum: String =
        sqlx::query_scalar("SELECT checksum FROM daiya_schema_migrations WHERE version = 1")
            .fetch_one(&db.admin)
            .await
            .unwrap();
    assert_eq!(
        checksum,
        sha256_hex(daiya_store_pg::MIGRATION_SQL.as_bytes())
    );
    let app_role =
        sqlx::query("SELECT rolcanlogin, rolbypassrls FROM pg_roles WHERE rolname = 'daiya_app'")
            .fetch_one(&db.admin)
            .await
            .unwrap();
    assert!(!app_role.get::<bool, _>("rolbypassrls"));
}

#[tokio::test]
async fn pool_config_reads_the_password_file_and_refuses_passwords_in_urls() {
    let dir = std::env::temp_dir().join(format!("daiya-pgcfg-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pw = dir.join("pw");
    std::fs::write(&pw, "  s3cret\n").unwrap();
    let empty = dir.join("empty");
    std::fs::write(&empty, "\n").unwrap();

    let cfg = PgPoolConfig::from_url_and_secret_file(
        "postgres://daiya_app_login@db.internal:6543/dash",
        &pw,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            cfg.host.as_str(),
            cfg.port,
            cfg.database.as_str(),
            cfg.app_role.as_str()
        ),
        ("db.internal", 6543, "dash", "daiya_app_login")
    );
    assert_eq!(cfg.password.as_str(), "s3cret");
    assert_eq!(cfg.max_connections, 1);
    // The password never shows in Debug output of anything holding the URL.
    assert!(
        !format!(
            "{:?}",
            parse_url_without_password("postgres://u@h/d").unwrap()
        )
        .contains("s3cret")
    );

    for bad in [
        "postgres://user:inline@db/dash",
        "mysql://user@db/dash",
        "postgres://db/dash",
        "not a url",
    ] {
        let err = PgPoolConfig::from_url_and_secret_file(bad, &pw, 4)
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), 400, "{bad}: {err}");
        assert!(!err.to_string().contains("inline"), "{err}");
    }
    let err = PgPoolConfig::from_secret_file("h", 5432, "d", "r", &empty, 2)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 400);
    let err = PgPoolConfig::from_secret_file("h", 5432, "d", "r", dir.join("absent"), 2)
        .await
        .unwrap_err();
    assert_eq!(err.status_code(), 500);
    let ok = PgPoolConfig::from_secret_file("h", 5432, "d", "r", &pw, 3)
        .await
        .unwrap();
    assert_eq!((ok.max_connections, ok.password.as_str()), (3, "s3cret"));
    std::fs::remove_dir_all(&dir).unwrap();

    // A wrong password does not connect, and the error does not echo it.
    if let Ok(url) = std::env::var("DAIYA_TEST_DATABASE_URL") {
        let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
        let wrong = PgPoolConfig {
            host: opts.get_host().to_owned(),
            port: opts.get_port(),
            database: opts.get_database().unwrap_or("postgres").to_owned(),
            app_role: "daiya_app_login".into(),
            password: zeroize::Zeroizing::new("definitely-wrong".into()),
            max_connections: 1,
        };
        let err = wrong.connect().await.unwrap_err();
        assert!(!err.to_string().contains("definitely-wrong"));
    }
}

#[tokio::test]
async fn a_lost_database_is_an_internal_error_that_leaks_nothing() {
    let _schema = SCHEMA_LOCK.read().await;
    let Some(db) = pg().await else { return };
    let store = PgStore::new(db.app_config.connect().await.unwrap());
    store.pool().close().await;
    let org = OrgId::new();

    let boot = store.verify_rls_enforced().await.unwrap_err();
    assert_eq!(boot.slug(), "cr.rls_not_enforced");
    let mut errors = vec![
        store.get_quotas(org).await.unwrap_err(),
        store
            .set_quotas(org, OrgQuotas::default())
            .await
            .unwrap_err(),
        store.list_folders(org).await.unwrap_err(),
        store.get_folder(org, "f").await.unwrap_err(),
        store
            .upsert_folder(folder(org, "f", None))
            .await
            .unwrap_err(),
        store.delete_folder(org, "f").await.unwrap_err(),
        store.list_dashboards(org).await.unwrap_err(),
        store.get_dashboard(org, "d").await.unwrap_err(),
        store
            .upsert_dashboard(dashboard(org, "d", None), "", true)
            .await
            .unwrap_err(),
        store.delete_dashboard(org, "d").await.unwrap_err(),
        store.list_dashboard_versions(org, "d").await.unwrap_err(),
        store.list_datasources(org).await.unwrap_err(),
        store.get_datasource(org, "d").await.unwrap_err(),
        store.get_datasource_by_name(org, "d").await.unwrap_err(),
        store
            .upsert_datasource(datasource(org, "d", "D", "prometheus"))
            .await
            .unwrap_err(),
        store.delete_datasource(org, "d").await.unwrap_err(),
        store.list_library_panels(org).await.unwrap_err(),
        store.get_library_panel(org, "l").await.unwrap_err(),
        store.delete_library_panel(org, "l").await.unwrap_err(),
        store.list_public_dashboards(org).await.unwrap_err(),
        store.get_public_dashboard(org, "p").await.unwrap_err(),
        store
            .get_public_dashboard_by_dashboard_uid(org, "d")
            .await
            .unwrap_err(),
        store
            .get_public_dashboard_by_token_hash("abc")
            .await
            .unwrap_err(),
        store
            .upsert_public_dashboard(public_dashboard(org, "p", "d", "t"))
            .await
            .unwrap_err(),
        store.delete_public_dashboard(org, "p").await.unwrap_err(),
        store.list_query_cache_stats(org).await.unwrap_err(),
        store.list_import_reports(org).await.unwrap_err(),
    ];
    let now = Utc::now();
    errors.push(
        store
            .upsert_library_panel(LibraryPanelRecord {
                installation_id: INST,
                org_id: org,
                instance_id: INSTANCE,
                folder_uid: None,
                uid: "l".into(),
                name: "l".into(),
                panel_type: "stat".into(),
                description: String::new(),
                model: json!({}),
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap_err(),
    );
    errors.push(
        store
            .upsert_query_cache_stat(QueryCacheStatRecord {
                id: "q".into(),
                org_id: org,
                instance_id: INSTANCE,
                datasource_uid: "d".into(),
                hits: 0,
                misses: 0,
                entries: 0,
                bytes_used: 0,
            })
            .await
            .unwrap_err(),
    );
    errors.push(
        store
            .insert_import_report(ImportReportRecord {
                id: "r".into(),
                org_id: org,
                instance_id: INSTANCE,
                dry_run: false,
                status: "ok".into(),
                dashboard_count: 0,
                refused_count: 0,
                mapped_count: 0,
                summary: json!({}),
                created_at: now,
            })
            .await
            .unwrap_err(),
    );
    for err in errors {
        assert_eq!(err.status_code(), 500, "{err}");
        assert_eq!(err.public_detail(), "an internal server error occurred");
    }
    assert!(daiya_store_pg::migrate(store.pool()).await.is_err());
}

#[test]
fn the_rls_catalog_check_refuses_every_unsafe_shape() {
    use daiya_store_pg::{TableRlsStatus, validate_rls_catalog_rows};
    let safe = |name: &str| TableRlsStatus {
        table_name: name.into(),
        rls_enabled: true,
        rls_forced: true,
        policy_count: 1,
    };
    let required: Vec<TableRlsStatus> = daiya_store_pg::REQUIRED_RLS_TABLES
        .iter()
        .map(|t| safe(t))
        .collect();
    validate_rls_catalog_rows(false, false, &required).unwrap();

    let refuse = |bypass: bool, sup: bool, tables: &[TableRlsStatus], needle: &str| {
        let err = validate_rls_catalog_rows(bypass, sup, tables).unwrap_err();
        assert_eq!(err.slug(), "cr.rls_not_enforced");
        assert!(err.to_string().contains(needle), "{needle}: {err}");
    };
    refuse(true, false, &required, "BYPASSRLS");
    refuse(false, true, &required, "SUPERUSER");
    refuse(false, false, &required[1..], "is missing from schema");

    let mut unforced = required.clone();
    unforced[0].rls_forced = false;
    refuse(
        false,
        false,
        &unforced,
        "does not have FORCE ROW LEVEL SECURITY",
    );
    let mut disabled = required.clone();
    disabled[0].rls_enabled = false;
    refuse(
        false,
        false,
        &disabled,
        "does not have FORCE ROW LEVEL SECURITY",
    );
    let mut no_policy = required.clone();
    no_policy[0].policy_count = 0;
    refuse(false, false, &no_policy, "has no RLS policy attached");

    // An extra cr_* table is held to the same rule as the required ones.
    let mut extra = required.clone();
    extra.push(TableRlsStatus {
        policy_count: 0,
        ..safe("cr_extra")
    });
    refuse(false, false, &extra, "cr_extra");
}

#[tokio::test]
async fn migrations_run_for_the_owner_and_fail_cleanly_for_the_app_role() {
    let _schema = SCHEMA_LOCK.write().await;
    let Some(db) = pg().await else { return };
    // The owner's store re-runs nothing when the recorded checksum matches.
    PgStore::new(db.admin.clone())
        .run_migrations()
        .await
        .unwrap();
    // The application role owns nothing and may not migrate.
    let err = db.app.run_migrations().await.unwrap_err();
    assert_eq!(err.slug(), "cr.migration_failed", "{err}");
}

#[test]
fn a_url_the_driver_cannot_use_is_refused() {
    let err = parse_url_without_password("postgres://role@db/dash?sslmode=sometimes").unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(
        err.to_string().contains("not a valid postgres URL"),
        "{err}"
    );
}

#[tokio::test]
async fn the_test_database_helper_reports_a_bad_or_unreachable_server() {
    let err = connect_test_database("not a url").await.err().unwrap();
    assert_eq!(err.status_code(), 500);
    let err = connect_test_database("postgres://daiya_nobody@127.0.0.1:1/daiya_none")
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("failed to connect"), "{err}");
}
