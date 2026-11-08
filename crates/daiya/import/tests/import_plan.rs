#![forbid(unsafe_code)]
//! Import, export, check, plan and apply beyond the 9-file round trip (R2, §7.2, §7.3): the
//! reports (json and markdown), library panels inlined (also inside collapsed rows), the
//! Grafana API wrapper, generated uids, the allow file of `check`, a plan of the live state
//! that has no changes, every diff kind, apply rewriting ownership, and a failing store
//! surfacing as a 500 from every step.

mod common;

use chrono::Utc;
use common::FailingStore;
use daiya_import::{
    DashboardImportInput, DesiredBundle, ExitCode, ImportReport, apply_plan,
    check_dashboards_offline, compute_plan, export_dashboard, import_dashboards,
    inline_library_panels, prepare_dashboard,
};
use daiya_model::{
    DaiyaStore, DataSourceInfo, DatasourceIndex, DatasourceRecord, FeatureFinding, FolderRecord,
    InputLimits, InstallationId, InstanceId, LibraryPanelRecord, OrgId, Verdict,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const ORG: OrgId = OrgId::from_u128(7);
const INST: InstallationId = InstallationId::from_u128(1);
const INSTANCE: InstanceId = InstanceId::from_u128(1);

fn dashboard(uid: &str) -> Value {
    json!({
        "uid": uid,
        "title": format!("Board {uid}"),
        "schemaVersion": 42,
        "version": 1,
        "panels": [{
            "id": 1, "type": "timeseries", "title": "P",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prom" },
            "targets": [{ "refId": "A", "expr": "up" }]
        }]
    })
}

fn refused(uid: &str) -> Value {
    let mut d = dashboard(uid);
    d["panels"][0]["type"] = json!("graph");
    d["panels"][0]["datasource"] = json!({ "type": "elasticsearch", "uid": "es" });
    d
}

fn input(raw: Value) -> DashboardImportInput {
    DashboardImportInput {
        raw_json: raw,
        folder_uid: None,
        is_provisioned: false,
        allow_ui_updates: false,
        external_id: None,
    }
}

fn folder(uid: &str, title: &str) -> FolderRecord {
    FolderRecord {
        installation_id: INST,
        org_id: ORG,
        instance_id: INSTANCE,
        project_id: None,
        uid: uid.into(),
        parent_uid: None,
        title: title.into(),
        version: 1,
        managed_by: None,
        created_by: "t".into(),
        updated_by: "t".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn datasource(uid: &str) -> DatasourceRecord {
    DatasourceRecord {
        installation_id: INST,
        org_id: ORG,
        instance_id: INSTANCE,
        project_id: None,
        uid: uid.into(),
        name: format!("DS {uid}"),
        kind: "prometheus".into(),
        access: "proxy".into(),
        url: "https://prom.example".into(),
        is_default: false,
        read_only: false,
        json_data: json!({}),
        secret_refs: BTreeMap::new(),
        allow_hosts: vec!["prom.example".into()],
        wasm_digest: None,
        version: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn library_panel(uid: &str) -> LibraryPanelRecord {
    LibraryPanelRecord {
        installation_id: INST,
        org_id: ORG,
        instance_id: INSTANCE,
        folder_uid: None,
        uid: uid.into(),
        name: "Shared".into(),
        panel_type: "stat".into(),
        description: String::new(),
        model: json!({
            "id": 99, "gridPos": { "x": 9, "y": 9, "w": 1, "h": 1 },
            "libraryPanel": { "uid": "other" },
            "type": "stat", "title": "From library", "options": { "colorMode": "value" }
        }),
        version: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

async fn import(
    store: &dyn DaiyaStore,
    inputs: &[DashboardImportInput],
    dry_run: bool,
) -> ImportReport {
    import_dashboards(
        store,
        INST,
        ORG,
        INSTANCE,
        "tester",
        inputs,
        dry_run,
        &InputLimits::default(),
    )
    .await
    .unwrap()
}

#[test]
fn library_panels_are_inlined_at_the_top_and_inside_collapsed_rows() {
    let libs: BTreeMap<String, LibraryPanelRecord> =
        [("lib".to_owned(), library_panel("lib"))].into();
    let mut dash = json!({
        "panels": [
            { "id": 1, "gridPos": { "x": 0, "y": 0, "w": 6, "h": 4 }, "libraryPanel": { "uid": "lib" }, "title": "Own title" },
            { "id": 2, "libraryPanel": { "uid": "missing" } },
            "not a panel",
            { "id": 3, "type": "row", "collapsed": true, "panels": [
                { "id": 4, "gridPos": { "x": 0, "y": 1, "w": 6, "h": 4 }, "libraryPanel": { "uid": "lib" } }
            ]}
        ]
    });
    inline_library_panels(&mut dash, &libs);
    let top = &dash["panels"][0];
    // The panel's own keys win; the library's id, gridPos and libraryPanel never replace them.
    assert_eq!(top["title"], "Own title");
    assert_eq!(top["type"], "stat");
    assert_eq!(top["options"]["colorMode"], "value");
    assert_eq!(top["id"], 1);
    assert_eq!(top["gridPos"]["x"], 0);
    assert_eq!(top["libraryPanel"]["uid"], "lib");
    // An unknown library panel is left as it is.
    assert_eq!(
        dash["panels"][1],
        json!({ "id": 2, "libraryPanel": { "uid": "missing" } })
    );
    // Inside the collapsed row too (Grafana keeps a collapsed row's children there).
    let nested = &dash["panels"][3]["panels"][0];
    assert_eq!(nested["type"], "stat", "{dash}");
    assert_eq!(nested["title"], "From library");
    assert_eq!(nested["id"], 4);

    // A document without panels is untouched.
    let mut empty = json!({ "title": "x" });
    inline_library_panels(&mut empty, &libs);
    assert_eq!(empty, json!({ "title": "x" }));
}

#[test]
fn prepare_accepts_the_grafana_api_wrapper_and_reports_migration_warnings() {
    let index = DatasourceIndex::new(vec![]);
    let (doc, canonical, report) = prepare_dashboard(
        &json!({ "dashboard": dashboard("wrapped"), "folderUid": "f1", "overwrite": true }),
        &index,
        &BTreeSet::new(),
        &BTreeMap::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert_eq!(doc.uid.as_deref(), Some("wrapped"));
    assert_eq!(canonical["uid"], "wrapped");
    assert_eq!(report.folder_uid.as_deref(), Some("f1"));

    // Below schemaVersion 36 Grafana's own migrator must run first: a named warning, mapped.
    let mut old = dashboard("old");
    old["schemaVersion"] = json!(30);
    let (_, _, report) = prepare_dashboard(
        &old,
        &index,
        &BTreeSet::new(),
        &BTreeMap::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "schema.migrationWarning" && f.verdict == Verdict::Maps),
        "{:?}",
        report.findings
    );

    // A document the schema refuses is a 422 naming the problem.
    let mut broken = dashboard("broken");
    broken["panels"] = json!("not a list");
    let err = prepare_dashboard(
        &broken,
        &index,
        &BTreeSet::new(),
        &BTreeMap::new(),
        &InputLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(err.status_code(), 400 | 422), "{err}");
}

#[test]
fn check_honours_the_allow_file_and_keeps_the_input_folder() {
    let ds = vec![DataSourceInfo {
        uid: "prom".into(),
        name: "Prometheus".into(),
        ds_type: "prometheus".into(),
        api_version: String::new(),
        is_default: true,
    }];
    let mut with_folder = input(refused("r"));
    with_folder.folder_uid = Some("from-input".into());
    let strict = check_dashboards_offline(
        &[with_folder.clone(), input(dashboard("ok"))],
        &ds,
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert_eq!(strict.exit_code, ExitCode::Refused.code());
    assert!(strict.dry_run);
    assert_eq!(
        strict.dashboards[0].folder_uid.as_deref(),
        Some("from-input")
    );
    assert_eq!(strict.total_panels, 2);

    let codes: BTreeSet<String> = strict.dashboards[0]
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Refuses)
        .map(|f| f.code.clone())
        .collect();
    let allowed =
        check_dashboards_offline(&[with_folder], &ds, &codes, &InputLimits::default()).unwrap();
    assert_eq!(allowed.refused_count, 0);
    assert_ne!(allowed.exit_code, ExitCode::Refused.code());

    let clean = check_dashboards_offline(
        &[input(dashboard("ok"))],
        &ds,
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert_eq!(
        clean.exit_code,
        ExitCode::Ok.code(),
        "{:?}",
        clean.dashboards
    );

    // A mapped feature is a warning (2): an old schema version.
    let mut old = dashboard("old");
    old["schemaVersion"] = json!(30);
    let mapped = check_dashboards_offline(
        &[input(old)],
        &ds,
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert_eq!(mapped.exit_code, ExitCode::ChangesOrWarnings.code());

    // An invalid document fails the whole check.
    let mut broken = dashboard("broken");
    broken["panels"] = json!("x");
    assert!(
        check_dashboards_offline(
            &[input(broken)],
            &ds,
            &BTreeSet::new(),
            &InputLimits::default()
        )
        .is_err()
    );
}

#[test]
fn the_markdown_report_lists_every_refusal_and_mapping_with_its_pointer() {
    let ds = vec![];
    let mut report = check_dashboards_offline(
        &[input(refused("r")), input(dashboard("ok"))],
        &ds,
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .unwrap();
    report.provisioning_findings.push(FeatureFinding {
        pointer: "/datasources/0/secureJsonData".into(),
        code: "provisioning.secureJsonData".into(),
        verdict: Verdict::Refuses,
        message: "name a secretRef".into(),
    });
    let md = report.to_markdown();
    assert!(
        md.starts_with("# daiya Import Report (dry_run=true, exit_code=1)"),
        "{md}"
    );
    assert!(md.contains("| `r` | Board r | 42 -> 42 | 1 |"), "{md}");
    assert!(md.contains("## Findings"));
    assert!(
        md.contains(
            "**[Refuses]** `provisioning.secureJsonData` (`/datasources/0/secureJsonData`)"
        )
    );
    assert!(md.contains("- `r` **[Refuses]**"), "{md}");
    // Supported features are counted, not listed.
    assert!(!md.contains("- `ok`"), "{md}");
    assert_eq!(report.to_json()["exitCode"], 1);

    // Nothing but supported features: no findings section.
    let clean = check_dashboards_offline(
        &[input(dashboard("ok"))],
        &[DataSourceInfo {
            uid: "prom".into(),
            name: "P".into(),
            ds_type: "prometheus".into(),
            api_version: String::new(),
            is_default: true,
        }],
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .unwrap();
    assert!(!clean.to_markdown().contains("## Findings"));
}

#[tokio::test]
async fn import_writes_records_reports_and_an_audit_row() {
    let store = FailingStore::new();
    store
        .upsert_library_panel(library_panel("lib"))
        .await
        .unwrap();

    // Dry run: a report, an audit row, nothing stored.
    let dry = import(&store, &[input(dashboard("a"))], true).await;
    assert!(dry.dry_run);
    assert!(store.get_dashboard(ORG, "a").await.unwrap().is_none());

    let mut no_uid = dashboard("");
    no_uid.as_object_mut().unwrap().remove("uid");
    no_uid["panels"][0]["libraryPanel"] = json!({ "uid": "lib", "name": "Shared" });
    let mut provisioned = input(dashboard("prov"));
    provisioned.is_provisioned = true;
    provisioned.external_id = Some("boards/prov.json".into());
    let mut placed = input(json!({ "dashboard": dashboard("placed"), "folderUid": "wrapper" }));
    placed.folder_uid = Some("explicit".into());
    let report = import(
        &store,
        &[input(no_uid), provisioned, placed, input(refused("r"))],
        false,
    )
    .await;
    assert_eq!(report.exit_code, ExitCode::Refused.code());
    // The count is of refused features, all of them in the one refused dashboard.
    let refused_in_r = report.dashboards[3]
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Refuses)
        .count();
    assert!(refused_in_r > 0);
    assert_eq!(report.refused_count, refused_in_r);

    let all = store.list_dashboards(ORG).await.unwrap();
    // A refused dashboard is kept (not drawn), as §7.1 says.
    assert_eq!(all.len(), 4);
    let generated = all
        .iter()
        .find(|d| !["prov", "placed", "r"].contains(&d.uid.as_str()))
        .unwrap();
    assert!(
        uuid::Uuid::parse_str(&generated.uid).is_ok(),
        "{}",
        generated.uid
    );
    // A UI import stays editable; the library panel was inlined before storing.
    assert!(generated.allow_ui_updates && !generated.is_provisioned);
    assert_eq!(generated.data["panels"][0]["options"]["colorMode"], "value");
    let prov = store.get_dashboard(ORG, "prov").await.unwrap().unwrap();
    assert!(prov.is_provisioned && !prov.allow_ui_updates);
    assert_eq!(
        prov.provisioned_external_id.as_deref(),
        Some("boards/prov.json")
    );
    assert_eq!(prov.created_by, "tester");
    // The input's folder wins over the wrapper's.
    let placed = store.get_dashboard(ORG, "placed").await.unwrap().unwrap();
    assert_eq!(placed.folder_uid.as_deref(), Some("explicit"));

    let audits = store.list_import_reports(ORG).await.unwrap();
    assert_eq!(audits.len(), 2);
    assert_eq!(audits[0].status, "completed_with_refusals");
    assert_eq!(audits[0].dashboard_count, 4);
    assert_eq!(audits[1].status, "success");
    assert!(audits[1].dry_run);
}

#[tokio::test]
async fn export_strips_the_numeric_id_and_answers_404_for_an_unknown_uid() {
    let store = FailingStore::new();
    let mut d = dashboard("e");
    d["id"] = json!(12);
    import(&store, &[input(d)], false).await;
    let exported = export_dashboard(&store, ORG, "e").await.unwrap();
    assert!(exported.get("id").is_none(), "{exported}");
    assert_eq!(exported["schemaVersion"], 42);
    let err = export_dashboard(&store, ORG, "nope").await.unwrap_err();
    assert_eq!(err.status_code(), 404);
    // Another organization's dashboard does not exist here.
    assert_eq!(
        export_dashboard(&store, OrgId::from_u128(8), "e")
            .await
            .unwrap_err()
            .status_code(),
        404
    );
}

#[tokio::test]
async fn a_plan_of_the_live_state_has_no_changes_and_each_difference_is_named() {
    let store = FailingStore::new();
    store.upsert_folder(folder("f", "Folder")).await.unwrap();
    store.upsert_datasource(datasource("prom")).await.unwrap();
    // A library panel the dashboard uses: the plan compares the inlined document.
    store
        .upsert_library_panel(library_panel("lib"))
        .await
        .unwrap();
    let mut with_lib = dashboard("d");
    with_lib["panels"][0]["libraryPanel"] = json!({ "uid": "lib", "name": "Shared" });
    let mut live = input(with_lib);
    live.folder_uid = Some("f".into());
    import(&store, std::slice::from_ref(&live), false).await;

    let same = DesiredBundle {
        folders: vec![folder("f", "Folder")],
        datasources: vec![datasource("prom")],
        dashboards: vec![live.clone()],
    };
    let plan = compute_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &same,
        &InputLimits::default(),
    )
    .await
    .unwrap();
    assert!(!plan.has_changes, "{:?}", plan.diffs);
    assert!(plan.diffs.iter().all(|d| d.action == "unchanged"));
    assert_eq!(plan.diffs.len(), 3);

    // Every field apply would write counts as a change.
    let mut ds_changes: Vec<DatasourceRecord> = Vec::new();
    for change in 0..10 {
        let mut ds = datasource("prom");
        match change {
            0 => ds.url = "https://other.example".into(),
            1 => ds.name = "Renamed".into(),
            2 => ds.kind = "loki".into(),
            3 => ds.access = "direct".into(),
            4 => ds.is_default = true,
            5 => ds.read_only = true,
            6 => ds.json_data = json!({ "timeInterval": "1m" }),
            7 => {
                ds.secret_refs.insert("token".into(), "new-ref".into());
            }
            8 => ds.allow_hosts.push("extra.example".into()),
            _ => ds.wasm_digest = Some("sha256:abc".into()),
        }
        ds_changes.push(ds);
    }
    for ds in ds_changes {
        let bundle = DesiredBundle {
            datasources: vec![ds],
            ..Default::default()
        };
        let plan = compute_plan(
            &store,
            INST,
            ORG,
            INSTANCE,
            "t",
            &bundle,
            &InputLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            plan.diffs[0].action, "update",
            "{:?}",
            bundle.datasources[0]
        );
        assert!(plan.has_changes);
    }

    let mut moved = live.clone();
    moved.folder_uid = Some("elsewhere".into());
    let mut retitled = live.raw_json.clone();
    retitled["title"] = json!("New title");
    let changed = DesiredBundle {
        folders: vec![folder("f", "Renamed folder"), folder("new", "New")],
        datasources: vec![datasource("new-ds")],
        dashboards: vec![moved, input(retitled), input(dashboard("fresh"))],
    };
    let plan = compute_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &changed,
        &InputLimits::default(),
    )
    .await
    .unwrap();
    let actions: Vec<(String, String, String)> = plan
        .diffs
        .iter()
        .map(|d| (d.kind.clone(), d.uid.clone(), d.action.clone()))
        .collect();
    let expect = |k: &str, u: &str, a: &str| (k.to_owned(), u.to_owned(), a.to_owned());
    assert_eq!(
        actions,
        vec![
            expect("Folder", "f", "update"),
            expect("Folder", "new", "create"),
            expect("Datasource", "new-ds", "create"),
            expect("Dashboard", "d", "update"),
            expect("Dashboard", "d", "update"),
            expect("Dashboard", "fresh", "create"),
        ]
    );
    let mut reparented = folder("f", "Folder");
    reparented.parent_uid = Some("new".into());
    let plan = compute_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &DesiredBundle {
            folders: vec![reparented],
            ..Default::default()
        },
        &InputLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(plan.diffs[0].action, "update");
}

#[tokio::test]
async fn apply_writes_folders_datasources_and_dashboards_as_this_organization() {
    let store = FailingStore::new();
    let other_org = OrgId::from_u128(99);
    let mut foreign_folder = folder("f", "Folder");
    foreign_folder.org_id = other_org;
    let mut foreign_ds = datasource("prom");
    foreign_ds.org_id = other_org;
    foreign_ds.installation_id = InstallationId::from_u128(5);
    let bundle = DesiredBundle {
        folders: vec![foreign_folder],
        datasources: vec![foreign_ds],
        dashboards: vec![input(dashboard("d"))],
    };
    let plan = compute_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &bundle,
        &InputLimits::default(),
    )
    .await
    .unwrap();
    let report = apply_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &plan,
        &InputLimits::default(),
    )
    .await
    .unwrap();
    assert!(!report.dry_run);
    // Whatever organization the bundle named, apply writes into the one it runs for.
    assert!(store.get_folder(ORG, "f").await.unwrap().is_some());
    assert!(store.get_folder(other_org, "f").await.unwrap().is_none());
    let ds = store.get_datasource(ORG, "prom").await.unwrap().unwrap();
    assert_eq!(ds.installation_id, INST);
    assert!(
        store
            .get_datasource(other_org, "prom")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_dashboard(ORG, "d").await.unwrap().is_some());

    // A plan for another organization is a 404, and a mangled desired state a 400.
    let err = apply_plan(
        &store,
        INST,
        other_org,
        INSTANCE,
        "t",
        &plan,
        &InputLimits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 404);
    let mut fresh = compute_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &DesiredBundle::default(),
        &InputLimits::default(),
    )
    .await
    .unwrap();
    fresh.desired = json!({ "folders": "not a list" });
    let err = apply_plan(
        &store,
        INST,
        ORG,
        INSTANCE,
        "t",
        &fresh,
        &InputLimits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("invalid desired state"), "{err}");
}

#[tokio::test]
async fn a_failing_store_is_a_500_from_import_plan_apply_and_export() {
    let bundle = DesiredBundle {
        folders: vec![folder("f", "F")],
        datasources: vec![datasource("prom")],
        dashboards: vec![input(dashboard("d"))],
    };
    for method in [
        "list_datasources",
        "list_library_panels",
        "upsert_dashboard",
        "insert_import_report",
    ] {
        let store = FailingStore::new();
        store.fail(method);
        let err = import_dashboards(
            &store,
            INST,
            ORG,
            INSTANCE,
            "t",
            &[input(dashboard("d"))],
            false,
            &InputLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status_code(), 500, "import {method}");
    }
    for method in [
        "list_folders",
        "list_datasources",
        "list_dashboards",
        "list_library_panels",
    ] {
        let store = FailingStore::new();
        store.fail(method);
        let err = compute_plan(
            &store,
            INST,
            ORG,
            INSTANCE,
            "t",
            &bundle,
            &InputLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status_code(), 500, "plan {method}");
    }
    for method in ["upsert_folder", "upsert_datasource", "upsert_dashboard"] {
        let store = FailingStore::new();
        let plan = compute_plan(
            &store,
            INST,
            ORG,
            INSTANCE,
            "t",
            &bundle,
            &InputLimits::default(),
        )
        .await
        .unwrap();
        store.fail(method);
        let err = apply_plan(
            &store,
            INST,
            ORG,
            INSTANCE,
            "t",
            &plan,
            &InputLimits::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status_code(), 500, "apply {method}");
    }
    let store = FailingStore::new();
    store.fail("get_dashboard");
    assert_eq!(
        export_dashboard(&store, ORG, "d")
            .await
            .unwrap_err()
            .status_code(),
        500
    );
}
