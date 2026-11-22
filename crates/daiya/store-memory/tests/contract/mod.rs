//! One behaviour contract for every `DaiyaStore` (R8), shared by the memory store's own tests and
//! daiya-store-pg's (`#[path]` include), so each store's package runs the same checks.

use daiya_model::{
    ApplyBundle, DaiyaStore, DashboardRecord, DatasourceRecord, FolderRecord, ImportReportRecord,
    InstallationId, InstanceId, LibraryPanelRecord, MAX_LIVE_PLANS, OrgId, OrgQuotas, PlanRecord,
    PublicDashboardRecord, ShareType, org_state_etag, sha256_hex,
};
use jc_time::TimestampTz;
use jiff::SignedDuration;
use serde_json::json;
use std::collections::BTreeMap;

pub const INST: InstallationId = InstallationId::from_u128(7);
pub const INSTANCE: InstanceId = InstanceId::from_u128(9);

pub fn folder(org: OrgId, uid: &str, parent: Option<&str>) -> FolderRecord {
    let now = TimestampTz::now();
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

pub fn dashboard(org: OrgId, uid: &str, folder_uid: Option<&str>) -> DashboardRecord {
    let now = TimestampTz::now();
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

pub fn datasource(org: OrgId, uid: &str, name: &str, kind: &str) -> DatasourceRecord {
    let now = TimestampTz::now();
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

pub fn public_dashboard(
    org: OrgId,
    uid: &str,
    dashboard_uid: &str,
    token: &str,
) -> PublicDashboardRecord {
    let now = TimestampTz::now();
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
    let org = OrgId::generate();
    assert_eq!(store.get_quotas(org).await.unwrap(), OrgQuotas::default());
    let custom = OrgQuotas {
        max_dashboards: 3,
        max_public_range_seconds: 3600,
        min_public_interval_ms: 30_000,
        ..OrgQuotas::default()
    };
    store
        .set_quotas(InstallationId::SINGLE, org, INSTANCE, custom.clone())
        .await
        .unwrap();
    assert_eq!(store.get_quotas(org).await.unwrap(), custom);
    // A second set replaces the first.
    store
        .set_quotas(InstallationId::SINGLE, org, INSTANCE, OrgQuotas::default())
        .await
        .unwrap();
    assert_eq!(store.get_quotas(org).await.unwrap(), OrgQuotas::default());
}

async fn contract_folders(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
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

    // Deleting a folder deletes its dashboards with their versions and public records;
    // deleting twice answers false.
    store
        .upsert_dashboard(dashboard(org, "in-folder", Some("f-a")), "", true)
        .await
        .unwrap();
    store
        .upsert_public_dashboard(public_dashboard(org, "pd-in", "in-folder", "tok-in"))
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
            .get_public_dashboard(org, "pd-in")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_dashboard_versions(org, "in-folder")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .get_dashboard(org, "elsewhere")
            .await
            .unwrap()
            .is_some()
    );
    assert!(!store.delete_folder(OrgId::generate(), "f-b").await.unwrap());
}

async fn contract_dashboards(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
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
            InstallationId::SINGLE,
            org,
            INSTANCE,
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
    assert!(
        !store
            .delete_dashboard(OrgId::generate(), "prov")
            .await
            .unwrap()
    );
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
    let org = OrgId::generate();
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
        .upsert_datasource(datasource(
            OrgId::generate(),
            "uid-new",
            "Beta",
            "prometheus",
        ))
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
            InstallationId::SINGLE,
            org,
            INSTANCE,
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
    assert!(
        !store
            .delete_datasource(OrgId::generate(), "p4")
            .await
            .unwrap()
    );
}

async fn contract_library_panels(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
    let now = TimestampTz::now();
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
            .delete_library_panel(OrgId::generate(), "lp")
            .await
            .unwrap()
    );
}

async fn contract_public_dashboards(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
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
            .delete_public_dashboard(OrgId::generate(), "pd")
            .await
            .unwrap()
    );
}

/// Every filter, order and page of every set answers what the reference evaluator answers over
/// the set's own rows, so the SQL translation and the memory store cannot drift apart.
async fn contract_rows(store: &dyn DaiyaStore) {
    use daiya_model::rows::answer_rows;
    use daiya_model::{ReadScope, RowQuery, RowSet, ScopeColumn};
    use odata_table::{Cond, SortKey, TextOp};
    use odata_url::CompareOp;

    let org = OrgId::generate();
    let mut parent = folder(org, "B", None);
    parent.title = "ä% folder".into();
    parent.managed_by = Some("git".into());
    store.upsert_folder(parent).await.unwrap();
    for uid in ["a", "Z", "ä", "a_%"] {
        let parent = (uid != "Z").then_some("B");
        store.upsert_folder(folder(org, uid, parent)).await.unwrap();
        let mut board = dashboard(org, uid, parent);
        board.title = match uid {
            "a" => "Same".into(),
            "Z" => "same".into(),
            _ => "Same".into(),
        };
        board.is_provisioned = uid == "ä";
        board.provisioned_external_id = board.is_provisioned.then(|| "/dir/a.json".into());
        store
            .upsert_dashboard(board.clone(), "", true)
            .await
            .unwrap();
        if uid == "a" {
            store.upsert_dashboard(board, "again", true).await.unwrap();
        }
        let mut source = datasource(org, uid, &format!("DS {uid}"), "prometheus");
        source.is_default = uid == "a";
        store.upsert_datasource(source).await.unwrap();
        let now = TimestampTz::now();
        store
            .upsert_library_panel(LibraryPanelRecord {
                installation_id: INST,
                org_id: org,
                instance_id: INSTANCE,
                folder_uid: parent.map(str::to_owned),
                uid: uid.into(),
                name: format!("Panel {uid}"),
                panel_type: "stat".into(),
                description: String::new(),
                model: json!({ "type": "stat" }),
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let token = format!("token-{}", uuid::Uuid::new_v4());
        store
            .upsert_public_dashboard(public_dashboard(org, &format!("p-{uid}"), uid, &token))
            .await
            .unwrap();
        store
            .insert_import_report(
                InstallationId::SINGLE,
                ImportReportRecord {
                    id: uid.into(),
                    org_id: org,
                    instance_id: INSTANCE,
                    dry_run: uid == "Z",
                    status: "ok".into(),
                    dashboard_count: uid.len() as i32,
                    refused_count: 0,
                    mapped_count: 1,
                    summary: json!({}),
                    created_at: now,
                },
            )
            .await
            .unwrap();
    }

    for set in RowSet::ALL {
        let read = |query: RowQuery| async move { store.read_rows(org, query).await.unwrap() };
        let all = read(RowQuery::every(set, ReadScope::All)).await.rows;
        assert!(all.len() >= 4, "{set:?}");
        let other = store
            .read_rows(OrgId::generate(), RowQuery::every(set, ReadScope::All))
            .await
            .unwrap();
        assert!(
            other.rows.is_empty(),
            "{set:?}: another organization's rows"
        );

        let members: Vec<(String, serde_json::Value)> = all[0]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(name, _)| *name != "tags")
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let mut filters = vec![Cond::Const(true), Cond::Const(false)];
        for (name, _) in &members {
            let path = vec![name.clone()];
            let null = Cond::IsNull { path: path.clone() };
            filters.extend([null.clone(), Cond::Not(Box::new(null.clone()))]);
            let mut values: Vec<serde_json::Value> =
                all.iter().map(|row| row[name].clone()).collect();
            values.extend([json!(1.5), json!(true)]);
            // A time is compared with a time: the API spells the literal as the rows do.
            if !name.ends_with("At") {
                values.push(json!("a"));
            }
            values.dedup();
            for value in values.into_iter().filter(|value| !value.is_null()) {
                for op in [
                    CompareOp::Eq,
                    CompareOp::Ne,
                    CompareOp::Gt,
                    CompareOp::Ge,
                    CompareOp::Lt,
                    CompareOp::Le,
                ] {
                    let one = Cond::Compare {
                        path: path.clone(),
                        op,
                        value: value.clone(),
                    };
                    filters.push(Cond::Not(Box::new(one.clone())));
                    filters.push(Cond::Not(Box::new(Cond::And(
                        Box::new(one.clone()),
                        Box::new(null.clone()),
                    ))));
                    filters.push(one);
                }
            }
            if !name.ends_with("At") {
                for needle in ["a", "%", "_", "ä", "", "S"] {
                    for op in [TextOp::Contains, TextOp::StartsWith, TextOp::EndsWith] {
                        let one = Cond::Text {
                            path: path.clone(),
                            op,
                            needle: needle.into(),
                        };
                        filters.push(Cond::Not(Box::new(one.clone())));
                        filters.push(one);
                    }
                }
            }
        }
        for filter in filters {
            let mut query = RowQuery::every(set, ReadScope::All);
            query.window.filter = filter;
            query.window.count = true;
            let want = answer_rows(all.clone(), &query);
            assert_eq!(read(query.clone()).await, want, "{query:?}");
        }

        for (name, value) in &members {
            for descending in [false, true] {
                let mut query = RowQuery::every(set, ReadScope::All);
                if name != set.key() {
                    query.window.order.insert(
                        0,
                        SortKey {
                            path: vec![name.clone()],
                            descending,
                        },
                    );
                } else {
                    query.window.order[0].descending = descending;
                }
                let sorted = answer_rows(all.clone(), &query).rows;
                let mut walked = Vec::new();
                query.window.limit = Some(1);
                loop {
                    let page = read(query.clone()).await.rows;
                    let Some(last) = page.last() else { break };
                    let after = query.window.order.iter();
                    query.window.after = Some(after.map(|k| last[&k.path[0]].clone()).collect());
                    walked.extend(page);
                    assert!(walked.len() <= all.len(), "{set:?} by {name}: no progress");
                }
                assert_eq!(
                    walked, sorted,
                    "{set:?} by {name} desc={descending}, from {value}"
                );
                query.window.after = None;
                query.window.skip = 1;
                query.window.limit = Some(2);
                let want = answer_rows(all.clone(), &query);
                assert_eq!(read(query).await, want, "{set:?} by {name}: skip");
            }
        }

        let mut only = RowQuery::every(set, ReadScope::All);
        only.window.limit = Some(0);
        only.window.count = true;
        let counted = read(only).await;
        assert_eq!(
            (counted.rows.len(), counted.count),
            (0, Some(all.len() as u64))
        );

        for scope in [
            ReadScope::Nothing,
            ReadScope::In {
                column: ScopeColumn::Object,
                values: ["a".to_owned(), "nul\0".to_owned()].into(),
            },
            ReadScope::In {
                column: ScopeColumn::Folder,
                values: ["B".to_owned()].into(),
            },
        ] {
            let mut query = RowQuery::every(set, scope);
            query.window.count = true;
            let want = answer_rows(all.clone(), &query);
            assert!(want.rows.len() < all.len(), "{query:?} narrows");
            assert_eq!(read(query.clone()).await, want, "{query:?}");
        }
    }
}

async fn contract_stats_and_reports(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
    let base = TimestampTz(TimestampTz::now().0 - SignedDuration::from_mins(5));
    for (i, id) in ["older", "newer"].into_iter().enumerate() {
        store
            .insert_import_report(
                InstallationId::SINGLE,
                ImportReportRecord {
                    id: id.into(),
                    org_id: org,
                    instance_id: INSTANCE,
                    dry_run: i == 0,
                    status: "ok".into(),
                    dashboard_count: 9,
                    refused_count: 0,
                    mapped_count: 1,
                    summary: json!({ "n": i }),
                    created_at: TimestampTz(base.0 + SignedDuration::from_mins(i as i64)),
                },
            )
            .await
            .unwrap();
    }
    let reports = store.list_import_reports(org).await.unwrap();
    let ids: Vec<&str> = reports.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["newer", "older"], "newest report first");
    assert!(reports[1].dry_run);
    assert_eq!(reports[0].summary["n"], 1);
}

/// Plans are kept per organization, expire, and are capped; `apply_bundle` writes a bundle whole
/// or not at all, refuses a moved base etag, and lets one of two concurrent bundles on one base
/// through (R2, §7.3).
async fn contract_plans_and_apply(store: &dyn DaiyaStore) {
    let org = OrgId::generate();
    // Whole milliseconds, so the timestamps survive Postgres' microsecond precision.
    let now = TimestampTz::from_millis(TimestampTz::now().as_millis()).unwrap();
    let hour = TimestampTz::from_millis(now.as_millis() + 3_600_000).unwrap();
    let plan = PlanRecord {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        digest: "a".repeat(64),
        base_etag: "e".into(),
        desired: json!({ "folders": [{ "uid": "f" }] }),
        created_by: "pia".into(),
        created_at: now,
        expires_at: hour,
    };
    store.insert_plan(plan.clone()).await.unwrap();
    assert_eq!(
        store.get_plan(org, &plan.digest).await.unwrap(),
        Some(plan.clone())
    );
    assert_eq!(
        store
            .get_plan(OrgId::generate(), &plan.digest)
            .await
            .unwrap(),
        None
    );
    let mut renewed = plan.clone();
    renewed.created_by = "ola".into();
    store.insert_plan(renewed.clone()).await.unwrap();
    assert_eq!(
        store.get_plan(org, &plan.digest).await.unwrap(),
        Some(renewed)
    );

    // An expired plan leaves at the next insert; live plans are capped.
    let mut expired = plan.clone();
    expired.digest = "b".repeat(64);
    expired.expires_at = TimestampTz::from_millis(now.as_millis() - 1).unwrap();
    store.insert_plan(expired.clone()).await.unwrap();
    for n in 1..MAX_LIVE_PLANS {
        let mut more = plan.clone();
        more.digest = format!("{n:064x}");
        store.insert_plan(more).await.unwrap();
    }
    assert_eq!(store.get_plan(org, &expired.digest).await.unwrap(), None);
    let mut over = plan.clone();
    over.digest = "c".repeat(64);
    let err = store.insert_plan(over).await.unwrap_err();
    assert_eq!(err.slug(), "cr.quota_exceeded", "{err}");

    let bundle = ApplyBundle {
        installation_id: INST,
        org_id: org,
        instance_id: INSTANCE,
        base_etag: org_state_etag(&[], &[], &[]),
        folders: vec![folder(org, "f", None)],
        datasources: vec![datasource(org, "prom", "Prom", "prometheus")],
        dashboards: vec![dashboard(org, "d", Some("f"))],
        message: "imported".into(),
    };
    let nothing_written = || async {
        assert!(store.list_folders(org).await.unwrap().is_empty());
        assert!(store.list_datasources(org).await.unwrap().is_empty());
        assert!(store.list_dashboards(org).await.unwrap().is_empty());
    };

    // A failure after the folders and datasources (the dashboard quota) writes nothing.
    let quotas = store.get_quotas(org).await.unwrap();
    let mut none = quotas.clone();
    none.max_dashboards = 0;
    store
        .set_quotas(InstallationId::SINGLE, org, INSTANCE, none)
        .await
        .unwrap();
    let err = store.apply_bundle(bundle.clone()).await.unwrap_err();
    assert_eq!(err.slug(), "cr.quota_exceeded", "{err}");
    nothing_written().await;
    store
        .set_quotas(InstallationId::SINGLE, org, INSTANCE, quotas)
        .await
        .unwrap();

    // A base etag the state no longer has is stale, and writes nothing.
    let mut stale = bundle.clone();
    stale.base_etag = "0".repeat(64);
    let err = store.apply_bundle(stale).await.unwrap_err();
    assert_eq!(err.slug(), "cr.stale_plan", "{err}");
    nothing_written().await;

    // Two bundles on one base: one is written, the other refused as stale.
    let mut other = bundle.clone();
    other.folders[0].title = "Other".into();
    let (one, two) = tokio::join!(store.apply_bundle(bundle), store.apply_bundle(other));
    let outcomes = [one, two];
    assert_eq!(
        outcomes.iter().filter(|r| r.is_ok()).count(),
        1,
        "{outcomes:?}"
    );
    assert!(
        outcomes
            .iter()
            .any(|r| r.as_ref().is_err_and(|e| e.slug() == "cr.stale_plan")),
        "{outcomes:?}"
    );
    let saved = outcomes.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].version, 1);
    assert_eq!(store.list_folders(org).await.unwrap().len(), 1);
    assert_eq!(store.list_dashboards(org).await.unwrap().len(), 1);
}

pub async fn run_contract(store: &dyn DaiyaStore) {
    store.verify_rls_enforced().await.unwrap();
    contract_quotas(store).await;
    contract_folders(store).await;
    contract_dashboards(store).await;
    contract_datasources(store).await;
    contract_library_panels(store).await;
    contract_public_dashboards(store).await;
    contract_stats_and_reports(store).await;
    contract_rows(store).await;
    contract_plans_and_apply(store).await;
}
