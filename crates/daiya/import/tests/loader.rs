#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! Loading a Grafana provisioning directory (R2, §7.1, §7.3, `pkg/services/provisioning`):
//! datasources with `deleteDatasources`, dashboard providers with `folder`, `folderUid` and
//! `foldersFromFilesStructure`, organization 1 only, raw `secureJsonData` refused, paths and
//! files confined to the root, and a refused file failing the run before anything of its kind
//! is written.

mod common;

use common::FailingStore;
use daiya_import::loader::MAX_PROVISIONED_FILES;
use daiya_import::{ProvisioningTarget, load_provisioning_dir, safe_uid_from_name};
use daiya_model::{CrError, DaiyaStore, InputLimits, InstallationId, InstanceId, OrgId};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const ORG: OrgId = OrgId::from_u128(42);

fn target() -> ProvisioningTarget {
    ProvisioningTarget {
        installation_id: InstallationId::from_u128(1),
        org_id: ORG,
        instance_id: InstanceId::from_u128(1),
    }
}

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("daiya-load-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
    fn dashboard(&self, rel: &str, uid: &str) -> PathBuf {
        self.write(rel, &dashboard(uid).to_string())
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // Give back permissions a test took away, then clean up.
        let _ = std::process::Command::new("chmod")
            .args(["-R", "u+rwx"])
            .arg(&self.0)
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dashboard(uid: &str) -> Value {
    json!({
        "uid": uid,
        "title": format!("Board {uid}"),
        "schemaVersion": 39,
        "panels": [{
            "id": 1, "type": "timeseries", "title": "P",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prom" },
            "targets": [{ "refId": "A", "expr": "up" }]
        }]
    })
}

async fn load(
    store: &dyn DaiyaStore,
    root: &Path,
) -> Result<daiya_import::ProvisioningLoadReport, CrError> {
    load_provisioning_dir(store, target(), root, &InputLimits::default()).await
}

const PROM: &str = "apiVersion: 1\ndatasources:\n  - name: Prometheus\n    uid: prom\n    type: prometheus\n    url: http://prom:9090\n    isDefault: true\n";

#[tokio::test]
async fn a_full_directory_loads_datasources_folders_and_dashboards_like_grafana() {
    let root = Root::new();
    let store = FailingStore::new();
    root.write("datasources/a.yaml", PROM);
    root.write("datasources/README.md", "not yaml, ignored");
    root.write(
        "datasources/b.yml",
        "apiVersion: 1\ndatasources:\n  - name: Old\n    uid: old\n    type: prometheus\n    url: http://old\n",
    );
    // First run: two datasources.
    root.write("dashboards/none.yaml", "apiVersion: 1\nproviders: []\n");
    let first = load(&store, &root.0).await.unwrap();
    assert_eq!(first.datasources_upserted, 2);
    let old = store.get_datasource(ORG, "old").await.unwrap().unwrap();
    assert!(
        old.read_only,
        "a provisioned datasource is read-only to the UI"
    );

    // Second run: `Old` is deleted (read-only to the UI, not to provisioning), and the
    // dashboards of four providers arrive.
    root.write(
        "datasources/b.yml",
        "apiVersion: 1\ndeleteDatasources:\n  - name: Old\n    orgId: 1\n  - name: Never-existed\n",
    );
    root.write(
        "dashboards/providers.yaml",
        "apiVersion: 1\nproviders:\n\
         - name: named\n  orgId: 1\n  folder: Operations\n  allowUiUpdates: true\n  options:\n    path: boards/named\n\
         - name: by-uid\n  folderUid: fixed-uid\n  options:\n    path: boards/by-uid\n\
         - name: tree\n  options:\n    path: boards/tree\n    foldersFromFilesStructure: true\n\
         - name: general\n  options:\n    path: boards/general\n",
    );
    root.dashboard("boards/named/n.json", "named-1");
    root.write("boards/named/notes.txt", "ignored");
    root.dashboard("boards/by-uid/u.json", "uid-1");
    root.dashboard("boards/tree/top.json", "tree-top");
    root.dashboard("boards/tree/team-a/a.json", "tree-a");
    root.dashboard("boards/tree/team-a/nested/deep.json", "tree-deep");
    root.dashboard("boards/general/g.json", "general-1");
    // A folder that already exists is reused, not recreated.
    store
        .upsert_folder(daiya_model::FolderRecord {
            installation_id: InstallationId::from_u128(1),
            org_id: ORG,
            instance_id: InstanceId::from_u128(1),
            project_id: None,
            uid: "fixed-uid".into(),
            parent_uid: None,
            title: "Made by hand".into(),
            version: 1,
            managed_by: None,
            created_by: "someone".into(),
            updated_by: "someone".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .await
        .unwrap();

    let report = load(&store, &root.0).await.unwrap();
    assert_eq!(report.datasources_deleted, 1);
    assert_eq!(report.datasources_upserted, 1);
    assert!(store.get_datasource(ORG, "old").await.unwrap().is_none());
    assert_eq!(report.dashboards, 6);
    // Operations, team-a and nested are new; fixed-uid existed; general has no folder.
    assert_eq!(report.folders, 3, "{report:?}");

    let ops_uid = safe_uid_from_name("Operations");
    let ops = store.get_folder(ORG, &ops_uid).await.unwrap().unwrap();
    assert_eq!(ops.title, "Operations");
    assert_eq!(ops.managed_by.as_deref(), Some("provisioning:named"));
    assert_eq!(
        store
            .get_folder(ORG, "fixed-uid")
            .await
            .unwrap()
            .unwrap()
            .title,
        "Made by hand"
    );

    let named = store.get_dashboard(ORG, "named-1").await.unwrap().unwrap();
    assert!(named.is_provisioned && named.allow_ui_updates);
    assert_eq!(named.folder_uid.as_deref(), Some(ops_uid.as_str()));
    assert_eq!(
        named.provisioned_external_id.as_deref(),
        Some("boards/named/n.json")
    );
    assert_eq!(named.schema_version, 42);
    let by_uid = store.get_dashboard(ORG, "uid-1").await.unwrap().unwrap();
    assert_eq!(by_uid.folder_uid.as_deref(), Some("fixed-uid"));
    assert!(!by_uid.allow_ui_updates);
    // Grafana names the folder after the file's own directory; the provider path is General.
    let folder_of = |uid: &'static str| {
        let store = &store;
        async move {
            store
                .get_dashboard(ORG, uid)
                .await
                .unwrap()
                .unwrap()
                .folder_uid
        }
    };
    assert_eq!(folder_of("tree-top").await, None);
    assert_eq!(
        folder_of("tree-a").await,
        Some(safe_uid_from_name("team-a"))
    );
    assert_eq!(
        folder_of("tree-deep").await,
        Some(safe_uid_from_name("nested"))
    );
    assert_eq!(folder_of("general-1").await, None);
    assert_eq!(
        store
            .get_folder(ORG, &safe_uid_from_name("nested"))
            .await
            .unwrap()
            .unwrap()
            .title,
        "nested"
    );
}

#[tokio::test]
async fn a_folder_uid_without_a_title_is_its_own_title() {
    let root = Root::new();
    let store = FailingStore::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    folderUid: only-uid\n    options:\n      path: b\n",
    );
    root.dashboard("b/x.json", "x");
    let report = load(&store, &root.0).await.unwrap();
    assert_eq!(report.folders, 1);
    let f = store.get_folder(ORG, "only-uid").await.unwrap().unwrap();
    assert_eq!(f.title, "only-uid");
}

#[tokio::test]
async fn an_empty_or_absent_directory_loads_nothing() {
    let root = Root::new();
    let store = FailingStore::new();
    assert_eq!(load(&store, &root.0).await.unwrap(), Default::default());
    let err = load(&store, &root.0.join("absent")).await.unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("cannot read"), "{err}");
}

#[tokio::test]
async fn only_organization_1_is_provisioned() {
    let store = FailingStore::new();
    let root = Root::new();
    root.write(
        "datasources/a.yaml",
        "apiVersion: 1\ndatasources:\n  - name: A\n    orgId: 2\n",
    );
    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("orgId 2"), "{err}");

    let root = Root::new();
    root.write(
        "datasources/a.yaml",
        "apiVersion: 1\ndeleteDatasources:\n  - name: A\n    org_id: 3\n",
    );
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("orgId 3"), "{err}");
    // Not even a quoted organization number slips through.
    let root = Root::new();
    root.write(
        "datasources/a.yaml",
        "apiVersion: 1\ndatasources:\n  - name: A\n    orgId: \"2\"\n",
    );
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("orgId \"2\""), "{err}");

    // A delete for organization 2 never deletes organization 1's datasource of that name.
    let root = Root::new();
    root.write(
        "datasources/a.yaml",
        "apiVersion: 1\ndatasources:\n  - name: Shared\n    uid: shared\n",
    );
    load(&store, &root.0).await.unwrap();
    root.write(
        "datasources/a.yaml",
        "apiVersion: 1\ndelete_datasources:\n  - name: Shared\n    orgId: 2\n",
    );
    assert_eq!(load(&store, &root.0).await.unwrap_err().status_code(), 400);
    assert!(store.get_datasource(ORG, "shared").await.unwrap().is_some());

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: other\n    orgId: 2\n    options:\n      path: b\n",
    );
    root.dashboard("b/x.json", "x");
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(
        err.to_string().contains("provider 'other' names orgId 2"),
        "{err}"
    );
    assert!(store.get_dashboard(ORG, "x").await.unwrap().is_none());
}

#[tokio::test]
async fn raw_secure_json_data_fails_the_run_before_any_datasource_is_written() {
    let store = FailingStore::new();
    let root = Root::new();
    // a.yaml is fine on its own; b.yaml carries a raw password.
    root.write("datasources/a.yaml", PROM);
    root.write(
        "datasources/b.yaml",
        "apiVersion: 1\ndatasources:\n  - name: DB\n    type: postgres\n    secureJsonData:\n      password: hunter2\n",
    );
    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("secretRef"), "{err}");
    assert!(!err.to_string().contains("hunter2"), "{err}");
    assert!(store.list_datasources(ORG).await.unwrap().is_empty());

    // Unparsable YAML fails the same way.
    let root = Root::new();
    root.write(
        "datasources/a.yaml",
        "datasources:\n  - name: A\n      bad: indent\n",
    );
    assert_eq!(load(&store, &root.0).await.unwrap_err().status_code(), 400);
    let root = Root::new();
    root.write(
        "dashboards/a.yaml",
        "providers:\n  - name: a\n    options: {path: x}\n",
    );
    assert_eq!(load(&store, &root.0).await.unwrap_err().status_code(), 400);
}

#[tokio::test]
async fn a_dashboard_with_a_refused_feature_fails_the_run_with_nothing_written() {
    let store = FailingStore::new();
    let root = Root::new();
    root.write("datasources/a.yaml", PROM);
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: ok\n    folder: Fine\n    options:\n      path: ok\n  - name: bad\n    folder: Broken\n    options:\n      path: bad\n",
    );
    root.dashboard("ok/fine.json", "fine");
    let mut refused = dashboard("refused");
    refused["panels"][0]["datasource"] = json!({ "type": "elasticsearch", "uid": "es" });
    refused["panels"][0]["type"] = json!("graph");
    root.write("bad/refused.json", &refused.to_string());

    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 422, "{err}");
    let CrError::Unprocessable { detail, errors } = &err else {
        panic!("{err:?}");
    };
    assert!(detail.contains("provider 'bad'"), "{detail}");
    assert!(
        errors.iter().all(|e| e.starts_with("refused: ")),
        "{errors:?}"
    );
    assert!(!errors.is_empty());
    // Not even the good provider's dashboard or either folder landed.
    assert!(store.list_dashboards(ORG).await.unwrap().is_empty());
    assert!(store.list_folders(ORG).await.unwrap().is_empty());
}

#[tokio::test]
async fn files_and_directories_stay_inside_the_root() {
    let store = FailingStore::new();
    let outside = Root::new();
    outside.dashboard("secret.json", "outside");

    // A file symlinked out of the root.
    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.dashboard("b/fine.json", "fine");
    std::os::unix::fs::symlink(outside.0.join("secret.json"), root.0.join("b/link.json")).unwrap();
    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 403, "{err}");
    assert!(err.to_string().contains("outside the provisioning root"));
    assert!(store.get_dashboard(ORG, "outside").await.unwrap().is_none());

    // A directory symlinked out of the root.
    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    std::fs::create_dir_all(root.0.join("b")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("b/away")).unwrap();
    assert_eq!(load(&store, &root.0).await.unwrap_err().status_code(), 403);

    // A symlink that stays inside the root is followed.
    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.dashboard("shared/s.json", "shared");
    std::fs::create_dir_all(root.0.join("b")).unwrap();
    std::os::unix::fs::symlink(root.0.join("shared/s.json"), root.0.join("b/s.json")).unwrap();
    load(&store, &root.0).await.unwrap();
    assert!(store.get_dashboard(ORG, "shared").await.unwrap().is_some());

    // A dangling symlink cannot be resolved: an I/O refusal, not a skip.
    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    std::fs::create_dir_all(root.0.join("b")).unwrap();
    std::os::unix::fs::symlink(root.0.join("nowhere.json"), root.0.join("b/d.json")).unwrap();
    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("cannot read"), "{err}");
}

#[tokio::test]
async fn provider_paths_must_be_directories_of_bounded_depth_and_size() {
    let store = FailingStore::new();

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: file.json\n",
    );
    root.dashboard("file.json", "f");
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("is not a directory"), "{err}");

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.dashboard("b/1/2/3/4/5/6/7/8/ok.json", "eight");
    load(&store, &root.0).await.unwrap();
    root.dashboard("b/1/2/3/4/5/6/7/8/9/deep.json", "nine");
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("nested deeper than 8"), "{err}");

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.write("b/broken.json", "{ not json");
    let err = load(&store, &root.0).await.unwrap_err();
    assert_eq!(err.status_code(), 400);
    assert!(err.to_string().contains("is not JSON"), "{err}");

    // More files than the cap is refused, not truncated.
    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    std::fs::create_dir_all(root.0.join("b")).unwrap();
    for i in 0..=MAX_PROVISIONED_FILES {
        std::fs::write(root.0.join(format!("b/{i:05}.json")), "{}").unwrap();
    }
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("more than 10000"), "{err}");
}

#[tokio::test]
async fn unreadable_files_and_directories_are_io_refusals() {
    use std::os::unix::fs::PermissionsExt;
    let store = FailingStore::new();
    let unreadable = |p: &Path| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).unwrap();
    };
    // Running as root, permissions do not bite: nothing to observe then.
    let probe = Root::new();
    let probe_file = probe.write("probe", "x");
    unreadable(&probe_file);
    if std::fs::read(&probe_file).is_ok() {
        println!("SKIP: running with CAP_DAC_OVERRIDE, permission errors cannot be produced");
        return;
    }

    let root = Root::new();
    let f = root.write("datasources/a.yaml", PROM);
    unreadable(&f);
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("cannot read"), "{err}");

    let root = Root::new();
    std::fs::create_dir_all(root.0.join("datasources")).unwrap();
    unreadable(&root.0.join("datasources"));
    let err = load(&store, &root.0).await.unwrap_err();
    assert!(err.to_string().contains("cannot read"), "{err}");

    let root = Root::new();
    let f = root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.dashboard("b/x.json", "x");
    unreadable(&f);
    assert!(
        load(&store, &root.0)
            .await
            .unwrap_err()
            .to_string()
            .contains("cannot read")
    );

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    let x = root.dashboard("b/x.json", "x");
    unreadable(&x);
    assert!(
        load(&store, &root.0)
            .await
            .unwrap_err()
            .to_string()
            .contains("cannot read")
    );

    let root = Root::new();
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    options:\n      path: b\n",
    );
    root.dashboard("b/sub/x.json", "x");
    unreadable(&root.0.join("b/sub"));
    assert!(
        load(&store, &root.0)
            .await
            .unwrap_err()
            .to_string()
            .contains("cannot read")
    );
}

#[tokio::test]
async fn a_failing_store_fails_the_run_at_each_write() {
    let root = Root::new();
    root.write("datasources/a.yaml", PROM);
    root.write(
        "datasources/b.yaml",
        "apiVersion: 1\ndeleteDatasources:\n  - name: Gone\n",
    );
    root.write(
        "dashboards/p.yaml",
        "providers:\n  - name: p\n    folder: F\n    options:\n      path: b\n",
    );
    root.dashboard("b/x.json", "x");

    for method in [
        "get_datasource_by_name",
        "upsert_datasource",
        "list_datasources",
        "get_folder",
        "upsert_folder",
        "upsert_dashboard",
        "insert_import_report",
    ] {
        let store = FailingStore::new();
        store.fail(method);
        let err = load(&store, &root.0).await.unwrap_err();
        assert_eq!(err.status_code(), 500, "{method}: {err}");
    }

    // Deleting a provisioned datasource: the unlock write and the delete itself.
    for method in ["upsert_datasource", "delete_datasource"] {
        let store = FailingStore::new();
        let seed = Root::new();
        seed.write(
            "datasources/a.yaml",
            "apiVersion: 1\ndatasources:\n  - name: Gone\n    uid: gone\n",
        );
        load(&store, &seed.0).await.unwrap();
        store.fail(method);
        let err = load(&store, &root.0).await.unwrap_err();
        assert_eq!(err.status_code(), 500, "{method}: {err}");
        store.heal();
    }
}
