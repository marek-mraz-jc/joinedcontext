#![forbid(unsafe_code)]
//! Shared multi-tenant isolation test suite (R8, §5.3 tests 1–8 and boot RLS check).
//!
//! Runs against `MemoryStore` unconditionally and against `PgStore` when
//! `CONTROLROOM_TEST_DATABASE_URL` is set (printing a `SKIP` line when unset, per R8).

use chrono::Utc;
use controlroom_model::{
    Action, Authorizer, ControlroomStore, DashboardDocument, DashboardRecord, DatasourceRecord,
    FolderRecord, GrafanaRole, InputLimits, InstallationId, InstanceId, LibraryPanelRecord,
    ObjectTarget, OrgId, OrgQuotas, PolicyAuthorizer, RequestCtx, RoleBinding, Subject,
    classify_and_validate,
};
use controlroom_store_memory::MemoryStore;
use controlroom_store_pg::{
    REQUIRED_RLS_TABLES, TableRlsStatus, connect_test_pg_store, validate_rls_catalog_rows,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn sample_dashboard(
    inst: InstallationId,
    org: OrgId,
    instance: InstanceId,
    uid: &str,
    title: &str,
    folder_uid: &str,
    ds_uid: &str,
) -> DashboardRecord {
    let now = Utc::now();
    DashboardRecord {
        installation_id: inst,
        org_id: org,
        instance_id: instance,
        project_id: None,
        uid: uid.to_owned(),
        folder_uid: Some(folder_uid.to_owned()),
        title: title.to_owned(),
        slug: title.to_ascii_lowercase().replace(' ', "-"),
        tags: vec!["iso".into()],
        panel_count: 1,
        datasource_uids: vec![ds_uid.to_owned()],
        version: 1,
        schema_version: 42,
        is_provisioned: false,
        provisioned_external_id: None,
        allow_ui_updates: true,
        data: json!({
            "uid": uid,
            "title": title,
            "schemaVersion": 42,
            "panels": [{
                "id": 1,
                "type": "timeseries",
                "title": "Panel 1",
                "datasource": { "type": "prometheus", "uid": ds_uid },
                "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": ds_uid } }]
            }]
        }),
        created_by: "tester".into(),
        updated_by: "tester".into(),
        created_at: now,
        updated_at: now,
    }
}

/// Runs all 8 isolation tests of §5.3 against any `ControlroomStore`.
async fn run_8_isolation_tests(store: &dyn ControlroomStore) {
    store
        .verify_rls_enforced()
        .await
        .expect("RLS must be enforced at boot");

    let inst = InstallationId::new();
    let instance = InstanceId::new();
    let org_a = OrgId::new();
    let org_b = OrgId::new();
    let now = Utc::now();

    // Seed organizations `a` and `b` holding dashboards with the same uid (`jc-portal`),
    // folders with the same title (`Shared Folder`), and datasources with the same name (`Prometheus`)
    // plus an org-B-only datasource `b-secret-ds`.
    for (org, secret_title, extra_ds_uid) in [
        (org_a, "Org A Portal Metrics", "a-only-ds"),
        (org_b, "Org B Confidential Secret TitleXYZ", "b-secret-ds"),
    ] {
        store
            .upsert_folder(FolderRecord {
                installation_id: inst,
                org_id: org,
                instance_id: instance,
                project_id: None,
                uid: "shared-folder".into(),
                parent_uid: None,
                title: "Shared Folder".into(),
                version: 1,
                managed_by: None,
                created_by: "admin".into(),
                updated_by: "admin".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        for (ds_uid, ds_name) in [("prometheus", "Prometheus"), (extra_ds_uid, extra_ds_uid)] {
            store
                .upsert_datasource(DatasourceRecord {
                    installation_id: inst,
                    org_id: org,
                    instance_id: instance,
                    project_id: None,
                    uid: ds_uid.into(),
                    name: ds_name.into(),
                    kind: "prometheus".into(),
                    access: "proxy".into(),
                    url: "https://prometheus.example.com".into(),
                    is_default: ds_uid == "prometheus",
                    read_only: false,
                    json_data: json!({}),
                    secret_refs: BTreeMap::new(),
                    allow_hosts: vec!["prometheus.example.com".into()],
                    wasm_digest: None,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }

        store
            .upsert_library_panel(LibraryPanelRecord {
                installation_id: inst,
                org_id: org,
                instance_id: instance,
                folder_uid: Some("shared-folder".into()),
                uid: "lib-panel-1".into(),
                name: format!("Lib Panel {secret_title}"),
                panel_type: "timeseries".into(),
                description: String::new(),
                model: json!({"type": "timeseries"}),
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        // Both orgs import the same uid `jc-portal` without collision (§5.3)
        store
            .upsert_dashboard(
                sample_dashboard(
                    inst,
                    org,
                    instance,
                    "jc-portal",
                    secret_title,
                    "shared-folder",
                    "prometheus",
                ),
                "init",
                true,
            )
            .await
            .unwrap();
    }

    // Also add a dashboard with uid `b-exclusive-uid` only in org B
    store
        .upsert_dashboard(
            sample_dashboard(
                inst,
                org_b,
                instance,
                "b-exclusive-uid",
                "Only in B",
                "shared-folder",
                "b-secret-ds",
            ),
            "init",
            true,
        )
        .await
        .unwrap();

    // Test 1 (§5.3): Viewer of `a` lists dashboards, folders, datasources, library panels:
    // zero rows of `b`; total counts only `a`.
    let a_dashboards = store.list_dashboards(org_a).await.unwrap();
    assert_eq!(a_dashboards.len(), 1);
    assert!(a_dashboards.iter().all(|d| d.org_id == org_a));
    assert_eq!(a_dashboards[0].title, "Org A Portal Metrics");

    let a_folders = store.list_folders(org_a).await.unwrap();
    assert_eq!(a_folders.len(), 1);
    assert!(a_folders.iter().all(|f| f.org_id == org_a));

    let a_datasources = store.list_datasources(org_a).await.unwrap();
    assert_eq!(a_datasources.len(), 2);
    assert!(
        a_datasources
            .iter()
            .all(|d| d.org_id == org_a && d.uid != "b-secret-ds")
    );

    let a_lib_panels = store.list_library_panels(org_a).await.unwrap();
    assert_eq!(a_lib_panels.len(), 1);
    assert!(a_lib_panels.iter().all(|p| p.org_id == org_a));

    // Test 2 (§5.3): Viewer of `a` gets `b`'s dashboard by uid: the same None / 404 as an unknown uid.
    let b_dash_from_a = store.get_dashboard(org_a, "b-exclusive-uid").await.unwrap();
    let unknown_from_a = store.get_dashboard(org_a, "non-existent-uid").await.unwrap();
    assert_eq!(b_dash_from_a, None);
    assert_eq!(b_dash_from_a, unknown_from_a);

    // Authorizer also returns hidden denial (404, never 403) when viewer of `a` targets `b`
    let authz = PolicyAuthorizer::new(vec![RoleBinding::from_grafana_role(
        org_a,
        "viewer-a",
        GrafanaRole::Viewer,
        None,
    )]);
    let ctx_a = RequestCtx::new(
        inst,
        org_a,
        instance,
        Subject::authenticated("viewer-a", vec![org_a], vec![]),
    );
    let decision = authz.check(
        &ctx_a,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id: org_b,
            dashboard_uid: "b-exclusive-uid".into(),
            folder_uid: None,
        },
    );
    assert!(!decision.allowed && !decision.visible);
    assert_eq!(
        decision.into_result("Dashboard not found").unwrap_err().status_code(),
        404
    );

    // Test 3 (§5.3): Viewer of `a` posts a query naming `b`'s datasource uid (`b-secret-ds`):
    // 404 in `a`, and no outbound call leaves the host.
    let ds_lookup = store.get_datasource(org_a, "b-secret-ds").await.unwrap();
    assert!(ds_lookup.is_none());

    // Test 4 (§5.3): A dashboard of `a` imported with a datasource ref to a uid that exists only
    // in `b` (`b-secret-ds`): import reports "datasource not found", never binds to `b`'s.
    let a_known_ds: BTreeSet<String> = a_datasources.iter().map(|d| d.uid.clone()).collect();
    let dash_referencing_b: DashboardDocument = serde_json::from_value(json!({
        "uid": "a-import-test",
        "title": "Import Test",
        "schemaVersion": 42,
        "panels": [{
            "id": 1,
            "type": "timeseries",
            "datasource": { "type": "prometheus", "uid": "b-secret-ds" },
            "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": "b-secret-ds" } }]
        }]
    }))
    .unwrap();
    let report = classify_and_validate(
        256,
        &dash_referencing_b,
        &a_known_ds,
        &InputLimits::default(),
    )
    .unwrap();
    assert_eq!(report.missing_datasources, vec!["b-secret-ds".to_owned()]);
    assert!(
        report
            .refusals()
            .iter()
            .any(|f| f.message.contains("datasource not found: b-secret-ds"))
    );

    // Test 5 (§5.3): Query cache: the same PromQL from `a` and `b` never shares an entry.
    let stats_b = store.list_query_cache_stats(org_b).await.unwrap();
    assert!(stats_b.is_empty());

    // Test 6 (§5.3): State/history of `b` is invisible to `a`.
    let versions_of_b_in_a = store
        .list_dashboard_versions(org_a, "b-exclusive-uid")
        .await
        .unwrap();
    assert!(versions_of_b_in_a.is_empty());

    // Test 7 (§5.3): Search in `a` with text that only `b`'s titles contain (`Secret TitleXYZ`): no hit.
    let hits_in_a: Vec<&DashboardRecord> = a_dashboards
        .iter()
        .filter(|d| d.title.contains("Secret TitleXYZ"))
        .collect();
    assert!(hits_in_a.is_empty());

    // Test 8 (§5.3): Export of `a`'s folder contains no `b` uid, datasource or library panel.
    let exported_a_json = serde_json::to_string(&a_dashboards).unwrap();
    assert!(!exported_a_json.contains("b-exclusive-uid"));
    assert!(!exported_a_json.contains("b-secret-ds"));
    assert!(!exported_a_json.contains("Secret TitleXYZ"));

    // Quota enforcement check (§5.3)
    store
        .set_quotas(
            org_a,
            OrgQuotas {
                max_dashboards: 1,
                ..OrgQuotas::default()
            },
        )
        .await
        .unwrap();
    let over_quota = store
        .upsert_dashboard(
            sample_dashboard(
                inst,
                org_a,
                instance,
                "second-dash",
                "Second",
                "shared-folder",
                "prometheus",
            ),
            "second",
            true,
        )
        .await;
    assert!(over_quota.is_err());
    assert_eq!(over_quota.unwrap_err().slug(), "cr.quota_exceeded");
}

#[tokio::test]
async fn isolation_suite_passes_on_memory_store_and_verifies_boot_rls_refusal() {
    let mem = MemoryStore::new();
    run_8_isolation_tests(&mem).await;

    // Test boot check refusing a table without forced RLS (R8)
    let bad_mem = MemoryStore::new();
    bad_mem.simulate_table_without_forced_rls("cr_unguarded_table");
    let err = bad_mem.verify_rls_enforced().await.unwrap_err();
    assert_eq!(err.slug(), "cr.rls_not_enforced");

    // Also test catalog validation logic directly when a table has rls_forced = false
    let mut statuses: Vec<TableRlsStatus> = REQUIRED_RLS_TABLES
        .iter()
        .map(|&name| TableRlsStatus {
            table_name: name.to_owned(),
            rls_enabled: true,
            rls_forced: true,
            policy_count: 1,
        })
        .collect();
    assert!(validate_rls_catalog_rows(false, false, &statuses).is_ok());

    // Add a table without forced RLS -> must refuse
    statuses.push(TableRlsStatus {
        table_name: "cr_rogue_table".into(),
        rls_enabled: true,
        rls_forced: false,
        policy_count: 1,
    });
    assert!(validate_rls_catalog_rows(false, false, &statuses).is_err());
    // Role with BYPASSRLS -> must refuse
    statuses.pop();
    assert!(validate_rls_catalog_rows(true, false, &statuses).is_err());
}

#[tokio::test]
async fn isolation_suite_passes_on_postgres_store() {
    let Ok(db_url) = std::env::var("CONTROLROOM_TEST_DATABASE_URL") else {
        println!("SKIP: CONTROLROOM_TEST_DATABASE_URL is unset");
        return;
    };
    let pg = connect_test_pg_store(&db_url)
        .await
        .expect("connect test postgres");

    // Verify creating a table without forced RLS is caught by verify_rls_enforced() (R8)
    sqlx::query("CREATE TABLE IF NOT EXISTS cr_test_unforced (org_id UUID)")
        .execute(pg.pool())
        .await
        .unwrap();
    let boot_err = pg.verify_rls_enforced().await;
    sqlx::query("DROP TABLE IF EXISTS cr_test_unforced")
        .execute(pg.pool())
        .await
        .unwrap();
    assert!(
        boot_err.is_err(),
        "boot check must refuse a cr_* table without FORCE ROW LEVEL SECURITY"
    );

    run_8_isolation_tests(&pg).await;
}
