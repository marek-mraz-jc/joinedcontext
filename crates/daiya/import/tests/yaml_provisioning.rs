#![forbid(unsafe_code)]
//! The provisioning YAML subset and the two Grafana provisioning file kinds
//! (`pkg/services/provisioning/{datasources,dashboards}`): what Grafana's own testdata shapes
//! parse to, and every shape the subset cannot read refused with a 400 rather than misread
//! (a misread file could drop a datasource, a `deleteDatasources` entry or a raw secret
//! refusal without a word).

use daiya_import::yaml::parse_yaml_to_json;
use daiya_import::{
    parse_dashboard_providers_yaml, parse_datasources_yaml, safe_uid_from_name,
    validate_provisioning_path,
};
use daiya_model::{InstallationId, InstanceId, OrgId, Verdict};
use serde_json::json;

const ORG: OrgId = OrgId::from_u128(5);

fn ds(text: &str) -> daiya_model::CrResult<daiya_import::DatasourceProvisioningOutcome> {
    parse_datasources_yaml(
        text,
        InstallationId::from_u128(1),
        ORG,
        InstanceId::from_u128(1),
    )
}

fn bad_request(text: &str) -> String {
    let err = parse_yaml_to_json(text).unwrap_err();
    assert_eq!(err.status_code(), 400, "{text}: {err}");
    err.to_string()
}

#[test]
fn scalars_follow_yaml_core_types_and_quoting() {
    let doc = parse_yaml_to_json(
        "a: true\nb: False\nc: ~\nd: null\ne: 42\nf: -1.5\ng: plain text # comment\n\
         h: \"quoted # not a comment\"\ni: 'it''s'\nj: \"tab\\there \\\"q\\\" \\u00e9 \\\\ \\/ \\r\\n\\0\"\n\
         k: NaN\nl: 1e3\nm: [1, \"two\"]\nn: {\"x\": 1}\n\"quoted key\": 1\n'single key': 2\nurl: http://h:9090/x\n",
    )
    .unwrap();
    assert_eq!(doc["a"], true);
    assert_eq!(doc["b"], false);
    assert!(doc["c"].is_null() && doc["d"].is_null());
    assert_eq!(doc["e"], 42);
    assert_eq!(doc["f"], -1.5);
    assert_eq!(doc["g"], "plain text");
    assert_eq!(doc["h"], "quoted # not a comment");
    assert_eq!(doc["i"], "it's");
    assert_eq!(doc["j"], "tab\there \"q\" \u{e9} \\ / \r\n\0");
    // Not a finite number: kept as the text.
    assert_eq!(doc["k"], "NaN");
    assert_eq!(doc["l"], 1000.0);
    assert_eq!(doc["m"], json!([1, "two"]));
    assert_eq!(doc["n"], json!({ "x": 1 }));
    assert_eq!(doc["quoted key"], 1);
    assert_eq!(doc["single key"], 2);
    assert_eq!(doc["url"], "http://h:9090/x");
}

#[test]
fn documents_json_empty_and_comment_only_inputs() {
    assert_eq!(parse_yaml_to_json("").unwrap(), json!({}));
    assert_eq!(parse_yaml_to_json("---\n# only\n\n").unwrap(), json!({}));
    assert_eq!(
        parse_yaml_to_json("{\"apiVersion\": 1}").unwrap(),
        json!({ "apiVersion": 1 })
    );
    assert!(bad_request("{\"apiVersion\": ").contains("invalid JSON/YAML"));
    // A top-level sequence of scalars and of empty items.
    assert_eq!(
        parse_yaml_to_json("- a\n- 2\n-\n").unwrap(),
        json!(["a", 2, null])
    );
}

#[test]
fn nested_blocks_compact_sequences_and_aligned_items() {
    // Grafana's docs indent sequences under their key; many files write them at the key's
    // own column. Both are the same YAML.
    let indented = "apiVersion: 1\ndatasources:\n  - name: A\n    type: prometheus\n    jsonData:\n      timeInterval: 30s\n      tags:\n        - x\n        - y\n  - name: B\n";
    let compact = "apiVersion: 1\ndatasources:\n- name: A\n  type: prometheus\n  jsonData:\n    timeInterval: 30s\n    tags:\n    - x\n    - y\n- name: B\n";
    let a = parse_yaml_to_json(indented).unwrap();
    let b = parse_yaml_to_json(compact).unwrap();
    assert_eq!(a, b);
    assert_eq!(a["datasources"][0]["jsonData"]["tags"], json!(["x", "y"]));
    assert_eq!(a["datasources"][1]["name"], "B");

    // Keys aligned after a wider dash gap, an item whose first key holds a block, an item
    // that is itself a block, and an empty value as null.
    let doc = parse_yaml_to_json(
        "list:\n  -   name: wide\n      type: t\n  - jsonData:\n      a: 1\n    name: second\n  -\n    name: third\n  - name:\n    type: empty-name\nempty:\nafter: 1\n",
    )
    .unwrap();
    assert_eq!(doc["list"][0], json!({ "name": "wide", "type": "t" }));
    assert_eq!(
        doc["list"][1],
        json!({ "jsonData": { "a": 1 }, "name": "second" })
    );
    assert_eq!(doc["list"][2], json!({ "name": "third" }));
    assert_eq!(
        doc["list"][3],
        json!({ "name": null, "type": "empty-name" })
    );
    assert!(doc["empty"].is_null());
    assert_eq!(doc["after"], 1);
}

#[test]
fn shapes_the_subset_cannot_read_are_refused_not_dropped() {
    // A deeper line after a scalar value.
    assert!(bad_request("a: 1\n  b: 2\n").contains("indentation"));
    // A key line without a colon.
    assert!(bad_request("a: 1\njust words\n").contains("mapping line 2"));
    // Inside a sequence item, a line indented deeper than its keys.
    assert!(bad_request("- name: a\n     type: b\n").contains("indentation"));
    // A sequence item nested where a key belongs, then trailing content.
    assert!(bad_request("- name: a\n  - b\n").contains("unexpected content at line 2"));
    // A line without a colon among an item's keys.
    assert!(bad_request("- name: a\n  words\n").contains("unexpected content at line 2"));
    // A sequence ended by a dedented key at another level.
    assert!(bad_request("x:\n  - a\n y: 1\n").contains("unexpected"));
    // Block scalars, anchors, aliases, merge keys and tags.
    for (text, what) in [
        ("a: |\n  text\n", "block scalars"),
        ("a: >-\n  text\n", "block scalars"),
        ("a: &base\n  x: 1\n", "anchors"),
        ("a: *base\n", "aliases"),
        ("- *base\n", "aliases"),
        ("a:\n  <<: *base\n", "merge keys"),
        ("a: !!str 1\n", "tags"),
    ] {
        assert!(bad_request(text).contains(what), "{text}");
    }
    // Flow collections are JSON or refused.
    assert!(bad_request("a: {password: x}\n").contains("must be written as JSON"));
    assert!(bad_request("- [a, b]\n").contains("must be written as JSON"));
    // Bad escapes in double quotes.
    for text in [
        "a: \"\\q\"\n",
        "a: \"\\u12\"\n",
        "a: \"\\ud800\"\n",
        "a: \"x\\\"\n",
    ] {
        assert!(bad_request(text).contains("escape"), "{text}");
    }
}

#[test]
fn a_secure_json_data_flow_mapping_cannot_slip_past_the_refusal() {
    // Written as YAML flow style, the secret block used to become a string and pass.
    let err = ds("apiVersion: 1\ndatasources:\n  - name: DB\n    type: postgres\n    secureJsonData: {password: hunter2}\n")
        .unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("JSON"), "{err}");
    // Errors are logged at boot: they never quote the line that held a credential.
    for text in [
        "datasources:\n  - name: DB\n    secureJsonData: {password: hunter2}\n",
        "datasources:\n  - name: DB\n    secureJsonData:\n      password: hunter2\n        x: 1\n",
        "password: hunter2\nhunter2-only-line\n",
        "a: 1\n  password: hunter2\n",
        "- a: 1\n  - password: hunter2\n",
        "password: *hunter2\n",
        "password: |hunter2\n",
    ] {
        let err = parse_yaml_to_json(text).unwrap_err().to_string();
        assert!(!err.contains("hunter2"), "{text}: {err}");
    }
    // As JSON flow style it parses, and is refused as raw credentials.
    let out = ds("apiVersion: 1\ndatasources:\n  - name: DB\n    type: postgres\n    secureJsonData: {\"password\": \"hunter2\"}\n").unwrap();
    assert!(
        out.findings
            .iter()
            .any(|f| f.verdict == Verdict::Refuses && f.code == "provisioning.secureJsonData"),
        "{:?}",
        out.findings
    );
}

#[test]
fn datasource_files_map_like_grafana() {
    let out = ds(r#"apiVersion: 1
deleteDatasources:
  - name: Old
    orgId: 1
  - orgId: 1
datasources:
  - name: Prom
    type: prometheus
    access: proxy
    url: https://prom.example:9090/
    isDefault: true
    editable: true
    jsonData:
      timeInterval: 15s
    secretRefs:
      token: prom-token
      ignored: 3
  - name: No uid
    access: browser
    url: not a url
    secureJsonData: {}
  - 7
  - name: Direct
    uid: ""
    access: direct
    is_default: false
    json_data: "not an object"
    secure_json_data:
      password: x
"#)
    .unwrap();
    assert_eq!(out.delete_names, vec!["Old".to_owned()]);
    assert_eq!(out.datasources.len(), 3);
    let prom = &out.datasources[0];
    assert_eq!(prom.uid, safe_uid_from_name("Prom"));
    assert_eq!(prom.kind, "prometheus");
    assert!(prom.is_default && !prom.read_only);
    assert_eq!(prom.json_data, json!({ "timeInterval": "15s" }));
    assert_eq!(prom.secret_refs.len(), 1);
    assert_eq!(prom.secret_refs["token"], "prom-token");
    assert_eq!(prom.allow_hosts, vec!["prom.example".to_owned()]);
    assert_eq!(prom.org_id, ORG);

    let no_uid = &out.datasources[1];
    // Type defaults to prometheus, an unknown access mode to proxy, and a provisioned
    // datasource is read-only unless editable.
    assert_eq!(no_uid.kind, "prometheus");
    assert_eq!(no_uid.access, "proxy");
    assert!(no_uid.read_only);
    assert!(no_uid.allow_hosts.is_empty());
    assert_eq!(no_uid.json_data, json!({}));

    let direct = &out.datasources[2];
    assert_eq!(direct.uid, safe_uid_from_name("Direct"));
    assert_eq!(direct.access, "direct");
    assert_eq!(direct.json_data, json!({}));
    // The snake_case secret block is refused like the camelCase one; an empty one is not.
    let refused: Vec<_> = out
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Refuses)
        .collect();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].pointer, "/datasources/3/secureJsonData");

    // Grafana's safeUIDFromName: "P" + upper-case first 16 hex digits of SHA-256.
    let uid = safe_uid_from_name("Prom");
    assert_eq!(uid.len(), 17);
    assert!(uid.starts_with('P'));
    assert_eq!(uid, uid.to_ascii_uppercase());
}

#[test]
fn datasource_files_refuse_what_grafana_refuses() {
    for (text, needle) in [
        ("- name: a\n", "mapping"),
        ("datasources:\n  - type: prometheus\n", "missing 'name'"),
        (
            "datasources:\n  - name: a\n    isDefault: true\n  - name: b\n    isDefault: true\n",
            "Only one datasource",
        ),
        ("a: |\n  x\n", "block scalars"),
    ] {
        let err = ds(text).unwrap_err();
        assert_eq!(err.status_code(), 400, "{text}");
        assert!(err.to_string().contains(needle), "{text}: {err}");
    }
    // No datasources key at all is an empty file, as in Grafana.
    let empty = ds("apiVersion: 1\n").unwrap();
    assert!(empty.datasources.is_empty() && empty.delete_names.is_empty());
}

#[test]
fn dashboard_provider_files_v0_v1_and_defaults() {
    let root = std::env::temp_dir().join(format!("daiya-prov-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(root.join("boards")).unwrap();

    // v1 with every field.
    let v1 = parse_dashboard_providers_yaml(
        "apiVersion: 1\nproviders:\n  - name: ops\n    orgId: 1\n    type: file\n    folder: Ops\n    folderUid: ops-uid\n    editable: true\n    disableDeletion: true\n    allowUiUpdates: true\n    updateIntervalSeconds: 30\n    options:\n      path: boards\n  - just-a-string\n",
        Some(&root),
    )
    .unwrap();
    assert_eq!(v1.len(), 1);
    let p = &v1[0];
    assert_eq!(
        (p.name.as_str(), p.org_id, p.provider_type.as_str()),
        ("ops", 1, "file")
    );
    assert_eq!(
        (p.folder.as_str(), p.folder_uid.as_str()),
        ("Ops", "ops-uid")
    );
    assert!(p.editable && p.disable_deletion && p.allow_ui_updates);
    assert_eq!(p.update_interval_seconds, 30);
    assert!(p.path.ends_with("boards"));
    assert!(!p.folders_from_files_structure);

    // v0: a bare list, `org_id`, `options.folder` as the path, defaults elsewhere.
    let v0 = parse_dashboard_providers_yaml(
        "- name: legacy\n  org_id: 2\n  type: \"\"\n  options:\n    folder: boards\n    foldersFromFilesStructure: true\n",
        Some(&root),
    )
    .unwrap();
    let p = &v0[0];
    assert_eq!(p.org_id, 2);
    assert_eq!(p.provider_type, "file");
    assert!(!p.editable && !p.disable_deletion && !p.allow_ui_updates);
    assert_eq!(p.update_interval_seconds, 10);
    assert!(p.folders_from_files_structure);

    // Without a root the path is only checked for traversal.
    let free = parse_dashboard_providers_yaml(
        "providers:\n  - name: x\n    options:\n      path: /var/lib/boards\n",
        None,
    )
    .unwrap();
    assert_eq!(free[0].path, "/var/lib/boards");
    assert_eq!(
        validate_provisioning_path("rel/dir", None).unwrap(),
        std::path::PathBuf::from("rel/dir")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dashboard_provider_files_refuse_what_grafana_refuses() {
    for (text, needle) in [
        ("apiVersion: 1\n", "'providers' list"),
        ("providers:\n  - type: file\n", "missing 'name'"),
        (
            "providers:\n  - name: a\n    options:\n      path: x\n  - name: a\n    options:\n      path: y\n",
            "not unique",
        ),
        (
            "providers:\n  - name: a\n    type: git\n    options:\n      path: x\n",
            "type git is not supported",
        ),
        ("providers:\n  - name: a\n", "path param is not a string"),
        (
            "providers:\n  - name: a\n    options:\n      path: 7\n",
            "path param is not a string",
        ),
        (
            "providers:\n  - name: a\n    folder: F\n    folderUid: f\n    options:\n      path: x\n      foldersFromFilesStructure: true\n",
            "foldersFromFilesStructure",
        ),
        (
            "providers:\n  - name: a\n    options:\n      path: \"\"\n",
            "empty",
        ),
    ] {
        let err = parse_dashboard_providers_yaml(text, None).unwrap_err();
        assert_eq!(err.status_code(), 400, "{text}: {err}");
        assert!(err.to_string().contains(needle), "{text}: {err}");
    }
    // `..` is a 403, as is a root that does not exist.
    let err = parse_dashboard_providers_yaml(
        "providers:\n  - name: a\n    options:\n      path: ../x\n",
        None,
    )
    .unwrap_err();
    assert_eq!(err.status_code(), 403);
    let missing_root = std::path::Path::new("/nonexistent-daiya-root");
    let err = validate_provisioning_path("x", Some(missing_root)).unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("root cannot be resolved"));
}
