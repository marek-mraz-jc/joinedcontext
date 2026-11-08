#![forbid(unsafe_code)]
//! Round-trip, fixed-point, feature census (§1.2), `check`, `plan`/`apply` stale-plan refusal,
//! and provisioning security tests over all 9 deployment dashboards (R1, R2, §7.2, §7.3, §8).

use chrono::Utc;
use daiya_import::{
    DashboardImportInput, DesiredBundle, ExitCode, apply_plan, check_dashboards_offline,
    compute_plan, export_dashboard, import_dashboards, parse_dashboard_providers_yaml,
    parse_datasources_yaml,
};
use daiya_model::{
    CanonicalJson, DaiyaStore, DataSourceInfo, DatasourceIndex, DatasourceRecord, FolderRecord,
    InputLimits, InstallationId, InstanceId, OrgId, Verdict, migrate_to_latest,
};
use daiya_store_memory::MemoryStore;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const FIXTURES: &[(&str, &str)] = &[
    ("assistant.json", include_str!("fixtures/assistant.json")),
    (
        "context-broker.json",
        include_str!("fixtures/context-broker.json"),
    ),
    (
        "context-gateway.json",
        include_str!("fixtures/context-gateway.json"),
    ),
    (
        "edge-apisix.json",
        include_str!("fixtures/edge-apisix.json"),
    ),
    ("k6-load.json", include_str!("fixtures/k6-load.json")),
    ("portal.json", include_str!("fixtures/portal.json")),
    ("slo.json", include_str!("fixtures/slo.json")),
    ("bbsk.json", include_str!("fixtures/bbsk.json")),
    ("helsinki.json", include_str!("fixtures/helsinki.json")),
];

fn all_known_datasources() -> Vec<DataSourceInfo> {
    vec![
        DataSourceInfo {
            uid: "prometheus".into(),
            name: "Prometheus".into(),
            ds_type: "prometheus".into(),
            api_version: String::new(),
            is_default: true,
        },
        DataSourceInfo {
            uid: "bbsk-kpi".into(),
            name: "bbsk-kpi".into(),
            ds_type: "ngsild-grafana-datasource".into(),
            api_version: String::new(),
            is_default: false,
        },
        DataSourceInfo {
            uid: "helsinki-cycle-counts".into(),
            name: "helsinki-cycle-counts".into(),
            ds_type: "ngsild-grafana-datasource".into(),
            api_version: String::new(),
            is_default: false,
        },
        DataSourceInfo {
            uid: "helsinki-air-quality".into(),
            name: "helsinki-air-quality".into(),
            ds_type: "ngsild-grafana-datasource".into(),
            api_version: String::new(),
            is_default: false,
        },
        DataSourceInfo {
            uid: "helsinki-bikes-ops".into(),
            name: "helsinki-bikes-ops".into(),
            ds_type: "ngsild-grafana-datasource".into(),
            api_version: String::new(),
            is_default: false,
        },
    ]
}

#[tokio::test]
async fn all_9_dashboards_match_inventory_and_import_export_import_is_fixed_point() {
    let store = MemoryStore::new();
    let inst = InstallationId::new();
    let org = OrgId::new();
    let instance = InstanceId::new();
    let now = Utc::now();

    for ds in all_known_datasources() {
        store
            .upsert_datasource(DatasourceRecord {
                installation_id: inst,
                org_id: org,
                instance_id: instance,
                project_id: None,
                uid: ds.uid,
                name: ds.name,
                kind: ds.ds_type,
                access: "proxy".into(),
                url: "https://endpoint.example.com".into(),
                is_default: ds.is_default,
                read_only: true,
                json_data: serde_json::json!({}),
                secret_refs: BTreeMap::new(),
                allow_hosts: vec!["endpoint.example.com".into()],
                wasm_digest: None,
                version: 1,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
    }

    let ds_index = DatasourceIndex::new(all_known_datasources());

    let mut inputs = Vec::with_capacity(FIXTURES.len());
    let mut panel_type_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut ds_target_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut organize_count = 0usize;
    let mut query_var_count = 0usize;

    for (name, content) in FIXTURES {
        let raw: Value = serde_json::from_str(content).expect(name);
        assert_eq!(
            raw["schemaVersion"], 39,
            "{name} must start at schemaVersion 39"
        );
        if let Some(vars) = raw["templating"]["list"].as_array() {
            query_var_count += vars.len();
        }
        if let Some(panels) = raw["panels"].as_array() {
            for p in panels {
                let p_type = p["type"].as_str().unwrap_or("");
                *panel_type_counts.entry(p_type.to_owned()).or_insert(0) += 1;
                if let Some(targets) = p["targets"].as_array() {
                    for t in targets {
                        let t_ds = t["datasource"]["type"].as_str().unwrap_or("");
                        *ds_target_counts.entry(t_ds.to_owned()).or_insert(0) += 1;
                    }
                }
                if let Some(txs) = p["transformations"].as_array() {
                    for tx in txs {
                        if tx["id"] == "organize" {
                            organize_count += 1;
                        }
                    }
                }
            }
        }
        inputs.push(DashboardImportInput {
            raw_json: raw,
            folder_uid: None,
            is_provisioned: true,
            allow_ui_updates: true,
            external_id: Some((*name).to_owned()),
        });
    }

    // Verify exact §1.2 inventory counts across our 9 files
    assert_eq!(FIXTURES.len(), 9);
    assert_eq!(panel_type_counts.get("timeseries"), Some(&44));
    assert_eq!(panel_type_counts.get("table"), Some(&8));
    assert_eq!(panel_type_counts.get("bargauge"), Some(&2));
    assert_eq!(panel_type_counts.get("stat"), Some(&1));
    assert_eq!(ds_target_counts.get("prometheus"), Some(&61));
    assert_eq!(ds_target_counts.get("ngsild-grafana-datasource"), Some(&6));
    assert_eq!(organize_count, 4);
    assert_eq!(query_var_count, 1);

    // Offline check passes with zero refusals
    let offline_report = check_dashboards_offline(
        &inputs,
        &all_known_datasources(),
        &BTreeSet::new(),
        &InputLimits::default(),
    )
    .expect("offline check");
    assert_eq!(offline_report.refused_count, 0);
    assert_eq!(offline_report.total_panels, 55);

    // Import all 9 dashboards into store
    let import_1 = import_dashboards(
        &store,
        inst,
        org,
        instance,
        "reconciler",
        &inputs,
        false,
        &InputLimits::default(),
    )
    .await
    .expect("import 1");
    assert_eq!(import_1.refused_count, 0);
    assert_eq!(import_1.dashboards.len(), 9);

    // For each of the 9 dashboards:
    // 1. export(import(f)) == CanonicalJson::export_dashboard_json(migrate_to_42(f))
    // 2. import(export(import(f))) == import(f) (fixed point, R2)
    for (name, content) in FIXTURES {
        let mut migrated: Value = serde_json::from_str(content).expect(name);
        migrate_to_latest(&mut migrated, &ds_index).expect("migrate_to_42");
        let uid = migrated["uid"].as_str().unwrap().to_owned();

        let exported_1 = export_dashboard(&store, org, &uid).await.expect("export 1");
        let mut expected_42 = CanonicalJson::export_dashboard_json(&migrated);
        if let Some(obj) = expected_42.as_object_mut() {
            obj.insert("version".to_owned(), Value::from(1));
        }
        expected_42 = CanonicalJson::canonicalize(&expected_42);

        assert_eq!(
            exported_1, expected_42,
            "export(import({name})) must equal canonical migrate_to_42({name})"
        );

        // Re-import the exported JSON into a fresh store and re-export: must be identical (fixed point)
        let store_2 = MemoryStore::new();
        for ds in all_known_datasources() {
            store_2
                .upsert_datasource(DatasourceRecord {
                    installation_id: inst,
                    org_id: org,
                    instance_id: instance,
                    project_id: None,
                    uid: ds.uid,
                    name: ds.name,
                    kind: ds.ds_type,
                    access: "proxy".into(),
                    url: "https://endpoint.example.com".into(),
                    is_default: ds.is_default,
                    read_only: true,
                    json_data: serde_json::json!({}),
                    secret_refs: BTreeMap::new(),
                    allow_hosts: vec!["endpoint.example.com".into()],
                    wasm_digest: None,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await
                .unwrap();
        }
        import_dashboards(
            &store_2,
            inst,
            org,
            instance,
            "reconciler",
            &[DashboardImportInput {
                raw_json: exported_1.clone(),
                folder_uid: None,
                is_provisioned: true,
                allow_ui_updates: true,
                external_id: None,
            }],
            false,
            &InputLimits::default(),
        )
        .await
        .expect("import 2");

        let exported_2 = export_dashboard(&store_2, org, &uid)
            .await
            .expect("export 2");
        assert_eq!(
            exported_1, exported_2,
            "import(export(import({name}))) must be a fixed point"
        );
    }

    // Verify Markdown and JSON report generation
    let md = import_1.to_markdown();
    assert!(md.contains("jc-assistant"));
    assert!(md.contains("project-helsinki"));
    assert!(md.contains("bargauge.displayMode"));
}

#[tokio::test]
async fn plan_and_apply_refuse_stale_plan_and_provisioning_guards_work() {
    let store = MemoryStore::new();
    let inst = InstallationId::new();
    let org = OrgId::new();
    let instance = InstanceId::new();
    let now = Utc::now();

    let desired = DesiredBundle {
        folders: vec![FolderRecord {
            installation_id: inst,
            org_id: org,
            instance_id: instance,
            project_id: None,
            uid: "platform".into(),
            parent_uid: None,
            title: "joinedcontext".into(),
            version: 1,
            managed_by: None,
            created_by: "ci".into(),
            updated_by: "ci".into(),
            created_at: now,
            updated_at: now,
        }],
        datasources: vec![DatasourceRecord {
            installation_id: inst,
            org_id: org,
            instance_id: instance,
            project_id: None,
            uid: "prometheus".into(),
            name: "Prometheus".into(),
            kind: "prometheus".into(),
            access: "proxy".into(),
            url: "https://prometheus.example.com".into(),
            is_default: true,
            read_only: true,
            json_data: serde_json::json!({"timeInterval": "15s"}),
            secret_refs: BTreeMap::new(),
            allow_hosts: vec!["prometheus.example.com".into()],
            wasm_digest: None,
            version: 1,
            created_at: now,
            updated_at: now,
        }],
        dashboards: vec![DashboardImportInput {
            raw_json: serde_json::from_str(FIXTURES[0].1).unwrap(),
            folder_uid: Some("platform".into()),
            is_provisioned: true,
            allow_ui_updates: false,
            external_id: Some("assistant.json".into()),
        }],
    };

    let plan = compute_plan(
        &store,
        inst,
        org,
        instance,
        "ci",
        &desired,
        &InputLimits::default(),
    )
    .await
    .expect("plan computed");
    assert!(plan.has_changes);

    // Apply succeeds when live state matches plan.base_etag
    let applied = apply_plan(
        &store,
        inst,
        org,
        instance,
        "ci",
        &plan,
        &InputLimits::default(),
    )
    .await
    .expect("apply succeeds");
    assert_eq!(applied.dashboards.len(), 1);

    // Re-applying the OLD plan now fails with StalePlan (exit code 3, R2) because live state changed!
    let stale_err = apply_plan(
        &store,
        inst,
        org,
        instance,
        "ci",
        &plan,
        &InputLimits::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(stale_err.slug(), "cr.stale_plan");
    assert_eq!(ExitCode::StalePlan.code(), 3);

    // Provisioning YAML tests:
    // 1. Datasources YAML with raw secureJsonData is flagged as Refuses (`provisioning.secureJsonData`)
    let ds_yaml = r#"
apiVersion: 1
datasources:
  - name: Prometheus
    type: prometheus
    uid: prometheus
    access: proxy
    url: "http://prometheus-operated.monitoring.svc.cluster.local:9090"
    isDefault: true
    editable: false
    jsonData:
      timeInterval: 15s
    secureJsonData:
      httpHeaderValue1: "raw-secret-token"
"#;
    let ds_outcome = parse_datasources_yaml(ds_yaml, inst, org, instance).unwrap();
    assert_eq!(ds_outcome.datasources.len(), 1);
    assert!(
        ds_outcome
            .findings
            .iter()
            .any(|f| f.code == "provisioning.secureJsonData" && f.verdict == Verdict::Refuses)
    );

    // 2. Dashboard providers YAML with path traversal (`..`) is refused (CVE-2021-43798 / R12)
    let traversal_yaml = r#"
apiVersion: 1
providers:
  - name: evil
    orgId: 1
    type: file
    options:
      path: /var/lib/grafana/dashboards/../../../etc/passwd
"#;
    let traversal_err = parse_dashboard_providers_yaml(traversal_yaml, None).unwrap_err();
    assert_eq!(traversal_err.status_code(), 403);
}
