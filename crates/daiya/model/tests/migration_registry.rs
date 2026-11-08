#![forbid(unsafe_code)]
//! Schema migrations 36..=42 on the edge cases of Grafana's `schemaversion/v36..v42.go`, the
//! v2beta1 conversion (§7.1), and the operation registry with its `Allowed` check (R10, R10a).

use daiya_model::migration::{
    get_datasource_ref, is_datasource_ref, is_v2beta1_dashboard, migrate_datasource_name_to_ref,
};
use daiya_model::{
    Action, CrError, DataSourceInfo, DatasourceIndex, GrafanaRole, InstallationId, InstanceId,
    ObjectTarget, OperationKind, OrgId, PolicyAuthorizer, RequestCtx, RoleBinding, Subject,
    convert_v2beta1_to_v1, daiya_registry, get_schema_version, migrate_to_latest, migrate_v36,
    migrate_v37, migrate_v38, migrate_v39, migrate_v40, migrate_v41, migrate_v42,
};
use serde_json::{Value, json};

fn info(uid: &str, name: &str, ty: &str, api: &str, default: bool) -> DataSourceInfo {
    DataSourceInfo {
        uid: uid.into(),
        name: name.into(),
        ds_type: ty.into(),
        api_version: api.into(),
        is_default: default,
    }
}

fn index() -> DatasourceIndex {
    DatasourceIndex::new(vec![
        info("prom-uid", "Prometheus", "prometheus", "", true),
        info("loki-uid", "Loki", "loki", "v1", false),
        info("", "Nameless uid", "x", "", false),
        info("no-name", "", "y", "", false),
    ])
}

#[test]
fn the_datasource_index_looks_up_by_name_uid_and_default() {
    let idx = index();
    assert_eq!(idx.lookup("Loki").unwrap().uid, "loki-uid");
    assert_eq!(idx.lookup("loki-uid").unwrap().name, "Loki");
    assert_eq!(idx.lookup_by_uid("prom-uid").unwrap().name, "Prometheus");
    assert!(idx.lookup_by_uid("Prometheus").is_none());
    assert_eq!(idx.lookup_by_name("Prometheus").unwrap().uid, "prom-uid");
    assert!(idx.lookup_by_name("prom-uid").is_none());
    assert_eq!(idx.lookup("no-name").unwrap().ds_type, "y");
    assert!(idx.lookup("").is_none());
    assert_eq!(idx.get_default().unwrap().uid, "prom-uid");
    assert!(DatasourceIndex::default().get_default().is_none());

    assert_eq!(get_datasource_ref(None), Value::Null);
    assert_eq!(
        get_datasource_ref(idx.lookup("Loki")),
        json!({"uid": "loki-uid", "type": "loki", "apiVersion": "v1"})
    );
    assert_eq!(
        get_datasource_ref(idx.get_default()),
        json!({"uid": "prom-uid", "type": "prometheus"})
    );
}

#[test]
fn datasource_names_migrate_to_refs_like_grafana_migrate_datasource_name_to_ref() {
    let idx = index();
    assert!(is_datasource_ref(&json!({"uid": "a"})));
    assert!(is_datasource_ref(&json!({"type": "a"})));
    assert!(!is_datasource_ref(&json!({"uid": ""})));
    assert!(!is_datasource_ref(&json!("a")));

    // returnDefaultAsNull
    assert_eq!(
        migrate_datasource_name_to_ref(None, true, &idx),
        Value::Null
    );
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!("default")), true, &idx),
        Value::Null
    );
    // An existing ref stays as it is.
    let r = json!({"uid": "custom", "type": "t"});
    assert_eq!(migrate_datasource_name_to_ref(Some(&r), false, &idx), r);
    // null and "default" resolve to the default datasource.
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&Value::Null), false, &idx),
        json!({"uid": "prom-uid", "type": "prometheus"})
    );
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!("default")), false, &DatasourceIndex::default()),
        json!({"uid": "default"})
    );
    assert_eq!(
        migrate_datasource_name_to_ref(None, false, &DatasourceIndex::default()),
        Value::Null
    );
    // Names and uids resolve; unknown names become a uid-only ref; "" an empty ref.
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!("Loki")), false, &idx),
        json!({"uid": "loki-uid", "type": "loki", "apiVersion": "v1"})
    );
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!("Gone")), false, &idx),
        json!({"uid": "Gone"})
    );
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!("")), false, &idx),
        json!({})
    );
    assert_eq!(
        migrate_datasource_name_to_ref(Some(&json!(5)), false, &idx),
        Value::Null
    );
}

#[test]
fn schema_versions_read_from_numbers_floats_and_strings() {
    assert_eq!(get_schema_version(&json!({})), 0);
    assert_eq!(get_schema_version(&json!({"schemaVersion": 39})), 39);
    assert_eq!(get_schema_version(&json!({"schemaVersion": 39.0})), 39);
    assert_eq!(get_schema_version(&json!({"schemaVersion": "38"})), 38);
    assert_eq!(get_schema_version(&json!({"schemaVersion": "x"})), 0);
    assert_eq!(get_schema_version(&json!({"schemaVersion": 70000})), 0);
    assert_eq!(get_schema_version(&json!({"schemaVersion": true})), 0);
}

#[test]
fn v36_fills_defaults_and_uses_the_first_target_datasource_for_a_default_panel() {
    let mut d = json!({
        "schemaVersion": 35,
        "annotations": {"list": [{"name": "a", "datasource": "Loki"}, {"name": "b"}, "not-an-object"]},
        "templating": {"list": [
            {"type": "query", "name": "q", "datasource": null},
            {"type": "query", "name": "q2", "datasource": "Loki"},
            {"type": "custom", "name": "c", "datasource": null},
            7
        ]},
        "panels": [
            {"type": "stat"},
            {"type": "timeseries", "targets": [{"refId": "A", "datasource": "Loki"}, {"refId": "B", "datasource": {"uid": "__expr__", "type": "__expr__"}}]},
            {"type": "timeseries", "datasource": "-- Mixed --", "targets": [{"refId": "A"}, {"refId": "B", "datasource": "Loki"}]},
            {"type": "table", "datasource": {}, "targets": [{"refId": "A", "datasource": {"type": "prometheus"}}]},
            {"type": "row", "panels": [
                {"type": "stat"},
                {"type": "stat", "datasource": "Loki", "targets": [{"refId": "A"}, 3]},
                "nested-junk"
            ]},
            "junk"
        ]
    });
    migrate_v36(&mut d, &index());
    assert_eq!(d["schemaVersion"], 36);
    let loki = json!({"uid": "loki-uid", "type": "loki", "apiVersion": "v1"});
    let prom = json!({"uid": "prom-uid", "type": "prometheus"});
    assert_eq!(d["annotations"]["list"][0]["datasource"], loki);
    assert_eq!(d["annotations"]["list"][1]["datasource"], prom);
    assert_eq!(d["annotations"]["list"][2], "not-an-object");
    assert_eq!(d["templating"]["list"][0]["datasource"], prom);
    // v36 only fills null query-variable datasources; names are left to the frontend.
    assert_eq!(d["templating"]["list"][1]["datasource"], "Loki");
    assert_eq!(d["templating"]["list"][2]["datasource"], Value::Null);

    // A panel without targets gets one and the default datasource.
    assert_eq!(
        d["panels"][0]["targets"],
        json!([{"refId": "A", "datasource": prom}])
    );
    assert_eq!(d["panels"][0]["datasource"], prom);

    // Default panel datasource takes the first real target datasource; expressions do not count.
    assert_eq!(d["panels"][1]["targets"][0]["datasource"], loki);
    assert_eq!(d["panels"][1]["datasource"], loki);
    assert_eq!(
        d["panels"][1]["targets"][1]["datasource"]["uid"],
        "__expr__"
    );

    // Mixed panel: a target without a datasource gets the default, not the mixed ref.
    assert_eq!(d["panels"][2]["datasource"], json!({"uid": "-- Mixed --"}));
    assert_eq!(d["panels"][2]["targets"][0]["datasource"], prom);
    assert_eq!(d["panels"][2]["targets"][1]["datasource"], loki);

    // An empty panel datasource object is kept; a target ref without uid inherits the panel's.
    assert_eq!(d["panels"][3]["datasource"], json!({}));
    assert_eq!(d["panels"][3]["targets"][0]["datasource"], json!({}));

    // Nested row panels: no targets added; datasource names migrated.
    assert!(d["panels"][4]["panels"][0].get("targets").is_none());
    assert_eq!(d["panels"][4]["panels"][1]["datasource"], loki);
    assert_eq!(
        d["panels"][4]["panels"][1]["targets"][0]["datasource"],
        loki
    );
    assert_eq!(d["panels"][4]["panels"][1]["targets"][1], 3);
    assert_eq!(d["panels"][5], "junk");

    // Not an object: nothing to do, for every step.
    let mut not_obj = json!([1]);
    migrate_v36(&mut not_obj, &index());
    migrate_v37(&mut not_obj);
    migrate_v38(&mut not_obj);
    migrate_v39(&mut not_obj);
    migrate_v40(&mut not_obj);
    migrate_v41(&mut not_obj);
    migrate_v42(&mut not_obj);
    assert_eq!(not_obj, json!([1]));
}

#[test]
fn v37_v38_v39_walk_rows_and_skip_panels_they_do_not_concern() {
    let mut d = json!({"panels": [
        "junk",
        {"type": "timeseries"},
        {"type": "timeseries", "options": {}},
        {"type": "row", "panels": [
            {"type": "timeseries", "options": {"legend": {"displayMode": "hidden"}}},
            {"type": "timeseries", "options": {"legend": {"displayMode": "table"}}}
        ]}
    ]});
    migrate_v37(&mut d);
    assert_eq!(
        d["panels"][3]["panels"][0]["options"]["legend"],
        json!({"displayMode": "list", "showLegend": false})
    );
    assert_eq!(
        d["panels"][3]["panels"][1]["options"]["legend"],
        json!({"displayMode": "table", "showLegend": true})
    );
    assert_eq!(d["panels"][2]["options"], json!({}));

    let mut t = json!({"panels": [
        "junk",
        {"type": "stat", "fieldConfig": {"defaults": {"custom": {"displayMode": "lcd-gauge"}}}},
        {"type": "table"},
        {"type": "table", "fieldConfig": {
            "defaults": {"custom": {"displayMode": "color-background-solid"}},
            "overrides": [
                "junk",
                {"matcher": {"id": "byName"}},
                {"properties": ["junk",
                    {"id": "custom.displayMode", "value": "gradient-gauge"},
                    {"id": "custom.displayMode"},
                    {"id": "custom.displayMode", "value": 3},
                    {"id": "custom.width", "value": 100}
                ]}
            ]}},
        {"type": "row", "panels": [
            {"type": "table", "fieldConfig": {"defaults": {"custom": {"displayMode": "json-view"}}}},
            {"type": "table", "fieldConfig": {"defaults": {"custom": {"displayMode": "color-background"}}}},
            {"type": "table", "fieldConfig": {"defaults": {"custom": {"displayMode": "basic"}}}}
        ]}
    ]});
    migrate_v38(&mut t);
    // Only table panels migrate.
    assert_eq!(
        t["panels"][1]["fieldConfig"]["defaults"]["custom"]["displayMode"],
        "lcd-gauge"
    );
    assert_eq!(
        t["panels"][3]["fieldConfig"]["defaults"]["custom"],
        json!({"cellOptions": {"type": "color-background", "mode": "basic"}})
    );
    let props = &t["panels"][3]["fieldConfig"]["overrides"][2]["properties"];
    assert_eq!(
        props[1],
        json!({"id": "custom.cellOptions", "value": {"type": "gauge", "mode": "gradient"}})
    );
    assert_eq!(props[2], json!({"id": "custom.cellOptions", "value": {}}));
    assert_eq!(props[3]["id"], "custom.cellOptions");
    assert_eq!(props[4], json!({"id": "custom.width", "value": 100}));
    let nested = &t["panels"][4]["panels"];
    assert_eq!(
        nested[0]["fieldConfig"]["defaults"]["custom"]["cellOptions"],
        json!({"type": "json-view"})
    );
    assert_eq!(
        nested[1]["fieldConfig"]["defaults"]["custom"]["cellOptions"],
        json!({"type": "color-background", "mode": "gradient"})
    );
    assert_eq!(
        nested[2]["fieldConfig"]["defaults"]["custom"]["cellOptions"],
        json!({"type": "gauge", "mode": "basic"})
    );

    let mut x = json!({"panels": [
        "junk",
        {"type": "table"},
        {"type": "table", "transformations": [
            "junk",
            {"id": "organize", "options": {"refIdToStat": {"A": "mean"}}},
            {"id": "timeSeriesTable"},
            {"id": "timeSeriesTable", "options": {}},
            {"id": "timeSeriesTable", "options": {"refIdToStat": {"A": "mean", "B": "max"}}}
        ]},
        {"type": "row", "panels": [{"type": "table", "transformations": [
            {"id": "timeSeriesTable", "options": {"refIdToStat": {"C": "last"}}}
        ]}]}
    ]});
    migrate_v39(&mut x);
    let txs = &x["panels"][2]["transformations"];
    assert_eq!(txs[1]["options"], json!({"refIdToStat": {"A": "mean"}}));
    assert!(txs[2].get("options").is_none());
    assert_eq!(txs[3]["options"], json!({}));
    assert_eq!(
        txs[4]["options"],
        json!({"A": {"stat": "mean"}, "B": {"stat": "max"}})
    );
    assert_eq!(
        x["panels"][3]["panels"][0]["transformations"][0]["options"],
        json!({"C": {"stat": "last"}})
    );
}

#[test]
fn v40_v41_v42_normalize_refresh_timepicker_and_hide_from() {
    let mut d = json!({"refresh": false, "timepicker": {"time_options": ["5m"], "hidden": true}});
    migrate_v40(&mut d);
    migrate_v41(&mut d);
    assert_eq!(d["refresh"], "");
    assert_eq!(d["timepicker"], json!({"hidden": true}));
    let mut keep = json!({"refresh": "1m"});
    migrate_v40(&mut keep);
    migrate_v41(&mut keep);
    assert_eq!(keep["refresh"], "1m");
    assert_eq!(keep["schemaVersion"], 41);

    let mut h = json!({"panels": [
        "junk",
        {"type": "timeseries", "fieldConfig": {"overrides": [
            "junk",
            {"matcher": {}},
            {"properties": [
                "junk",
                {"id": "custom.hideFrom", "value": {"viz": true, "legend": false}},
                {"id": "custom.hideFrom", "value": {"viz": false}},
                {"id": "custom.hideFrom"},
                {"id": "color", "value": {"viz": true}}
            ]}
        ]}},
        {"type": "row", "panels": [
            "junk",
            {"type": "timeseries", "fieldConfig": {"overrides": [{"properties": [
                {"id": "custom.hideFrom", "value": {"viz": true}}
            ]}]}}
        ]}
    ]});
    migrate_v42(&mut h);
    let props = &h["panels"][1]["fieldConfig"]["overrides"][2]["properties"];
    assert_eq!(
        props[1]["value"],
        json!({"viz": true, "legend": false, "tooltip": true})
    );
    assert_eq!(props[2]["value"], json!({"viz": false}));
    assert!(props[3].get("value").is_none());
    assert_eq!(props[4]["value"], json!({"viz": true}));
    assert_eq!(
        h["panels"][2]["panels"][1]["fieldConfig"]["overrides"][0]["properties"][0]["value"]["tooltip"],
        true
    );
}

#[test]
fn v2beta1_documents_convert_with_layout_queries_and_library_panels() {
    let v2 = json!({
        "apiVersion": "dashboard.grafana.app/v2beta1",
        "kind": "Dashboard",
        "metadata": {"name": "v2-uid"},
        "spec": {
            "title": "V2 board",
            "description": "converted",
            "editable": false,
            "tags": ["a"],
            "cursorSync": "Tooltip",
            "timeSettings": {"from": "now-24h", "to": "now-1h", "timezone": "utc", "autoRefresh": "30s"},
            "elements": {
                "panel-1": {"kind": "Panel", "spec": {
                    "id": 1, "title": "CPU",
                    "vizConfig": {"group": "stat", "spec": {"options": {"reduceOptions": {}}, "fieldConfig": {"defaults": {"unit": "percent"}, "overrides": []}}},
                    "data": {"kind": "QueryGroup", "spec": {
                        "queries": [
                            {"kind": "PanelQuery", "spec": {"refId": "A", "hidden": true,
                                "query": {"kind": "DataQuery", "group": "prometheus", "datasource": {"name": "prom-uid"}, "spec": {"expr": "up"}}}},
                            {"kind": "PanelQuery", "spec": {"refId": "B",
                                "datasource": {"uid": "loki-uid"},
                                "query": {"kind": "loki", "spec": {"expr": "{job=\"x\"}"}}}},
                            {"kind": "PanelQuery", "spec": {"query": {"group": "ngsild"}}},
                            {"kind": "PanelQuery", "spec": {"refId": "Z"}},
                            {"kind": "PanelQuery"}
                        ],
                        "transformations": [{"kind": "organize", "spec": {"id": "organize", "options": {}}}, {"kind": "broken"}]
                    }}
                }},
                "panel-2": {"kind": "Panel", "spec": {"id": 2}},
                "lib": {"kind": "LibraryPanel", "spec": {"id": 3, "title": "Shared", "libraryPanel": {"uid": "lib1", "name": "Shared"}}},
                "broken": {"kind": "Panel"},
                "junk": 5
            },
            "layout": {"kind": "GridLayout", "spec": {"items": [
                {"kind": "GridLayoutItem", "spec": {"x": 0, "y": 0, "width": 6, "height": 4, "element": {"kind": "ElementReference", "name": "panel-1"}}},
                {"kind": "GridLayoutItem", "spec": {"element": {"name": "lib"}}},
                {"kind": "GridLayoutItem"}
            ]}}
        }
    });
    assert!(is_v2beta1_dashboard(&v2));
    let v1 = convert_v2beta1_to_v1(&v2).unwrap();
    assert_eq!(v1["uid"], "v2-uid");
    assert_eq!(v1["title"], "V2 board");
    assert_eq!(v1["description"], "converted");
    assert_eq!(v1["editable"], false);
    assert_eq!(v1["graphTooltip"], 2);
    assert_eq!(v1["timezone"], "utc");
    assert_eq!(v1["refresh"], "30s");
    assert_eq!(v1["time"], json!({"from": "now-24h", "to": "now-1h"}));
    assert_eq!(v1["schemaVersion"], 42);

    let panels = v1["panels"].as_array().unwrap();
    assert_eq!(panels.len(), 3, "{v1}");
    let by_id = |id: u64| panels.iter().find(|p| p["id"] == id).unwrap();
    let cpu = by_id(1);
    assert_eq!(cpu["type"], "stat");
    assert_eq!(cpu["gridPos"], json!({"x": 0, "y": 0, "w": 6, "h": 4}));
    assert_eq!(cpu["fieldConfig"]["defaults"]["unit"], "percent");
    let targets = cpu["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 3);
    assert_eq!(
        targets[0],
        json!({"refId": "A", "hide": true, "expr": "up", "datasource": {"type": "prometheus", "uid": "prom-uid"}})
    );
    assert_eq!(
        targets[1]["datasource"],
        json!({"type": "loki", "uid": "loki-uid"})
    );
    assert_eq!(
        targets[2],
        json!({"refId": "A", "datasource": {"type": "ngsild", "uid": "ngsild"}})
    );
    assert_eq!(
        cpu["transformations"],
        json!([{"id": "organize", "options": {}}])
    );

    let plain = by_id(2);
    assert_eq!(plain["type"], "timeseries");
    assert_eq!(plain["gridPos"], json!({"x": 0, "y": 0, "w": 12, "h": 8}));
    assert_eq!(
        plain["fieldConfig"],
        json!({"defaults": {}, "overrides": []})
    );
    assert!(plain.get("transformations").is_none());

    let lib = by_id(3);
    assert_eq!(lib["type"], "library-panel-ref");
    assert_eq!(lib["libraryPanel"]["uid"], "lib1");
    // Layout items without coordinates stack by index (8 rows each).
    assert_eq!(lib["gridPos"], json!({"x": 0, "y": 8, "w": 12, "h": 8}));

    // A bare v2 spec without metadata, without optional settings.
    let bare = json!({"uid": "bare", "elements": {}, "layout": {}, "cursorSync": "Crosshair"});
    assert!(is_v2beta1_dashboard(&bare));
    let v1 = convert_v2beta1_to_v1(&bare).unwrap();
    assert_eq!(v1["uid"], "bare");
    assert_eq!(v1["title"], "Converted Dashboard");
    assert_eq!(v1["graphTooltip"], 1);
    assert_eq!(v1["time"], json!({"from": "now-6h", "to": "now"}));
    assert_eq!(v1["timezone"], "browser");
    assert_eq!(v1["editable"], true);
    assert!(v1.get("description").is_none());
    let anonymous = convert_v2beta1_to_v1(&json!({"elements": {}, "layout": {}})).unwrap();
    assert_eq!(anonymous["uid"], "converted-v2");
    assert_eq!(anonymous["graphTooltip"], 0);

    assert!(!is_v2beta1_dashboard(&json!({"elements": {}})));
    assert!(!is_v2beta1_dashboard(&json!({"spec": {"elements": {}}})));
    assert!(!is_v2beta1_dashboard(&json!("x")));
    let err = convert_v2beta1_to_v1(&json!([])).unwrap_err();
    assert_eq!(err.status_code(), 400);
}

#[test]
fn the_chain_converts_v2_refuses_non_objects_and_never_reruns_steps() {
    let mut v2 = json!({"spec": {"title": "x", "elements": {}, "layout": {}}});
    let report = migrate_to_latest(&mut v2, &index()).unwrap();
    assert!(report.converted_from_v2beta1);
    assert_eq!(report.original_schema_version, 42);
    assert!(report.steps_applied.is_empty());
    assert_eq!(report.warnings.len(), 1);

    let mut bad = json!("dashboard");
    assert!(matches!(
        migrate_to_latest(&mut bad, &index()),
        Err(CrError::MigrationFailed(_))
    ));

    let mut mid = json!({"schemaVersion": 39, "refresh": null, "timepicker": {"time_options": []}});
    let report = migrate_to_latest(&mut mid, &index()).unwrap();
    assert_eq!(report.steps_applied, [40, 41, 42]);
    assert!(report.warnings.is_empty());
    assert_eq!(mid["refresh"], "");
    assert_eq!(mid["timepicker"], json!({}));

    let mut newer = json!({"schemaVersion": 42, "refresh": 5});
    let report = migrate_to_latest(&mut newer, &index()).unwrap();
    assert!(report.steps_applied.is_empty());
    assert_eq!(newer["refresh"], 5, "a v42 document is not migrated again");
}

fn ctx(subject: Subject) -> RequestCtx {
    RequestCtx::new(
        InstallationId::from_u128(1),
        OrgId::from_u128(1),
        InstanceId::from_u128(1),
        subject,
    )
}

#[test]
fn operations_are_found_by_odata_mcp_and_cli_names() {
    let reg = daiya_registry();
    let by_odata = reg.find_operation("datasource/query").unwrap();
    assert_eq!(by_odata.name, "Datasource/Query");
    assert_eq!(
        reg.find_operation(&by_odata.mcp_tool).unwrap().name,
        by_odata.name
    );
    assert_eq!(
        reg.find_operation(&by_odata.cli_command).unwrap().name,
        by_odata.name
    );
    assert!(reg.find_operation("Drop").is_none());
    let allowed = reg.find_operation("Allowed").unwrap();
    assert_eq!(allowed.kind, OperationKind::Function);
    assert_eq!(allowed.required_action, Action::Read);
    assert!(
        reg.operations
            .iter()
            .any(|op| op.kind == OperationKind::Action)
    );
}

#[test]
fn allowed_uses_the_same_authorizer_on_every_object_kind() {
    let reg = daiya_registry();
    let org = OrgId::from_u128(1);
    let authz = PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(org, "viewer", GrafanaRole::Viewer, None),
        RoleBinding::from_grafana_role(org, "admin", GrafanaRole::Admin, None),
    ]);
    let viewer = ctx(Subject::authenticated("viewer", vec![org], vec![]));
    let admin = ctx(Subject::authenticated("admin", vec![org], vec![]));

    for spec in [
        "Dashboard:d",
        "dashboard:d",
        "Dashboards:d",
        "Datasource:prom",
        "datasource:prom",
        "Datasources:prom",
        "Folder:f",
        "folder:f",
        "Folders:f",
        "LibraryPanel:l",
        "libraryPanel:l",
        "LibraryPanels:l",
        "PublicDashboard:d",
        "publicDashboard:d",
        "PublicDashboards:d",
        "Organization:x",
        "nonsense",
    ] {
        assert!(
            reg.check_allowed(&authz, &viewer, "read", Some(spec))
                .allowed,
            "{spec}"
        );
        assert!(
            !reg.check_allowed(&authz, &viewer, "Apply", Some(spec))
                .allowed,
            "{spec}"
        );
        assert!(
            reg.check_allowed(&authz, &admin, "Apply", Some(spec))
                .allowed,
            "{spec}"
        );
    }
    // Verbs, operations, and unknown names (which need Admin).
    assert!(
        reg.check_allowed(&authz, &viewer, "Datasource/Query", Some("Datasource:prom"))
            .allowed
    );
    assert!(!reg.check_allowed(&authz, &viewer, "edit", None).allowed);
    assert!(
        !reg.check_allowed(&authz, &viewer, "frobnicate", None)
            .allowed
    );
    assert!(
        reg.check_allowed(&authz, &admin, "frobnicate", None)
            .allowed
    );
    // Another organization's member sees nothing.
    let stranger = ctx(Subject::authenticated(
        "viewer",
        vec![OrgId::from_u128(2)],
        vec![],
    ));
    let d = reg.check_allowed(&authz, &stranger, "read", Some("Dashboard:d"));
    assert!(!d.allowed && !d.visible);
    let _ = ObjectTarget::Organization { org_id: org };
}

#[test]
fn csdl_json_and_xml_describe_the_same_sets_and_operations() {
    let reg = daiya_registry();
    let json = reg.csdl_json();
    assert_eq!(json["$Version"], "4.01");
    let ns = json["namespace"].as_str().unwrap();
    let sets = json["entitySets"].as_array().unwrap();
    assert_eq!(sets.len(), reg.service.sets.len());
    for s in sets {
        let name = s["name"].as_str().unwrap();
        assert!(s["entityType"].as_str().unwrap().starts_with(ns));
        assert!(!s["key"].as_str().unwrap().is_empty(), "{name}");
    }
    let ops = json["operations"].as_array().unwrap();
    assert_eq!(ops.len(), reg.operations.len());
    let query = ops
        .iter()
        .find(|o| o["name"] == "Datasource/Query")
        .unwrap();
    assert_eq!(query["requiredAction"], "read");
    assert!(query["parameters"].as_array().is_some());

    let xml = reg.csdl_xml();
    assert!(xml.starts_with("<?xml"));
    assert!(xml.contains("<EntityContainer"));
    assert!(xml.contains("FunctionImport Name=\"Allowed\""));
    assert!(
        xml.contains("ActionImport Name=\"Datasource_Query\"")
            || xml.contains("FunctionImport Name=\"Datasource_Query\"")
    );
    for t in [
        "Edm.String",
        "Edm.Int64",
        "Edm.Boolean",
        "Edm.DateTimeOffset",
    ] {
        assert!(xml.contains(t), "{t}");
    }
    assert!(xml.trim_end().ends_with("</edmx:Edmx>"));
}

#[test]
fn steps_leave_dashboards_without_panels_and_empty_rows_alone() {
    let mut empty = json!({"title": "no panels"});
    migrate_v36(&mut empty, &index());
    migrate_v37(&mut empty);
    migrate_v38(&mut empty);
    migrate_v39(&mut empty);
    migrate_v42(&mut empty);
    assert_eq!(empty, json!({"title": "no panels", "schemaVersion": 42}));

    let rows = json!({"panels": [{"type": "row"}, {"type": "row", "panels": "x"}]});
    for step in [migrate_v37, migrate_v38, migrate_v39, migrate_v42] {
        let mut d = rows.clone();
        step(&mut d);
        assert_eq!(d["panels"], rows["panels"]);
    }

    // v38: lcd gauges, and table defaults without a displayMode stay as they are.
    let mut t = json!({"panels": [
        {"type": "table", "fieldConfig": {"defaults": {"custom": {"displayMode": "lcd-gauge"}}}},
        {"type": "table", "fieldConfig": {"defaults": {"custom": {"width": 80}}}},
        {"type": "table", "fieldConfig": {"defaults": {"custom": {"displayMode": 4}}}}
    ]});
    migrate_v38(&mut t);
    assert_eq!(
        t["panels"][0]["fieldConfig"]["defaults"]["custom"]["cellOptions"],
        json!({"type": "gauge", "mode": "lcd"})
    );
    assert_eq!(
        t["panels"][1]["fieldConfig"]["defaults"]["custom"],
        json!({"width": 80})
    );
    // A non-string displayMode is not Grafana's to migrate: it stays verbatim (R1 lossless).
    assert_eq!(
        t["panels"][2]["fieldConfig"]["defaults"]["custom"],
        json!({"displayMode": 4})
    );

    // v36 without any default datasource: a panel with no datasource keeps null and its
    // targets stay without one.
    let mut d = json!({"panels": [{"type": "stat", "targets": [{"refId": "A"}, "junk"]}]});
    migrate_v36(&mut d, &DatasourceIndex::default());
    assert_eq!(d["panels"][0]["datasource"], Value::Null);
    assert_eq!(d["panels"][0]["targets"][0], json!({"refId": "A"}));
}
