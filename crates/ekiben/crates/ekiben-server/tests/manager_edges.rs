//! `StreamManager` beyond the API: per-organization file roots, the host's admission rule,
//! resources by kind, restoring kept streams and resources, replacing a stream whose new version
//! does not start, and a host that refuses a delete.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ekiben_core::{CoreError, EnvAccess, OrgId, ResourceManager, StreamConfig};
use ekiben_server::manager::format_go_duration;
use ekiben_server::{ResourceError, StreamManager, StreamPersistence};
use ekiben_testkit::full_registry;
use futures::future::BoxFuture;
use serde_json::{json, Value};

const RESIDENT: &str =
    "input:\n  generate:\n    interval: 1h\n    mapping: 'root = {}'\noutput:\n  drop: {}\n";
const ONCE: &str =
    "run: once\ninput:\n  generate:\n    count: 1\n    mapping: 'root = {}'\noutput:\n  drop: {}\n";
const BROKEN: &str = "input:\n  no_such_input: {}\noutput:\n  drop: {}\n";

fn manager() -> StreamManager {
    StreamManager::new(
        full_registry(),
        ResourceManager::new(),
        None,
        EnvAccess::AllowListed,
    )
}

fn conf(text: &str) -> StreamConfig {
    StreamConfig::from_yaml_or_json(text).unwrap()
}

fn org(id: &str) -> OrgId {
    OrgId::new(id)
}

#[test]
fn go_durations_print_in_their_unit() {
    assert_eq!(format_go_duration(Duration::ZERO), "0s");
    assert_eq!(format_go_duration(Duration::from_nanos(5_500)), "5µs");
    assert_eq!(format_go_duration(Duration::from_micros(1_500)), "1.500ms");
    assert_eq!(format_go_duration(Duration::from_millis(2_250)), "2.250s");
}

#[test]
fn each_organization_gets_a_private_directory_under_the_file_root() {
    let root = tempfile::tempdir().unwrap();
    let m = StreamManager::new(
        full_registry(),
        ResourceManager::new(),
        Some(root.path().to_path_buf()),
        false,
    );
    assert_eq!(
        m.file_root_for(&org("a")).unwrap(),
        Some(root.path().to_path_buf())
    );
    let m = m.with_org_file_roots();
    let dir = m.file_root_for(&org("city-a")).unwrap().unwrap();
    assert_eq!(dir, root.path().join("city-a"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for bad in ["..", "a/b", "/abs", ""] {
        let err = m.file_root_for(&org(bad)).expect_err(bad);
        assert!(
            matches!(err, CoreError::AccessDenied { .. }),
            "{bad}: {err}"
        );
    }
    // A file where the directory should go: refused with the reason.
    std::fs::write(root.path().join("taken"), "x").unwrap();
    let err = m.file_root_for(&org("taken")).unwrap_err().to_string();
    assert!(
        err.contains("cannot create the file directory of organization 'taken'"),
        "{err}"
    );
    let none = manager().with_org_file_roots();
    assert!(none
        .file_root_for(&org("a"))
        .unwrap_err()
        .to_string()
        .contains("no file root"));
}

#[tokio::test]
async fn admission_refuses_streams_and_resources_before_they_build() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let m = manager().with_admission(Arc::new(move |stream: Option<&str>, _: &Value| {
        seen.lock().unwrap().push(stream.map(str::to_owned));
        match stream {
            Some("ok") => Ok(()),
            _ => Err("private host not bound to this project".to_string()),
        }
    }));
    let err = m.create("no".into(), conf(RESIDENT)).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Config(ref m) if m.contains("private host")),
        "{err}"
    );
    m.create("ok".into(), conf(RESIDENT)).await.unwrap();
    assert!(m.update("ok", conf(BROKEN), None).await.is_err());
    assert!(m.read("ok", None).await.unwrap().active);
    let refused = m.put_resource(&OrgId::default(), "cache", "c", "memory: {}");
    assert!(matches!(refused, Err(ResourceError::Parse(ref why)) if why.contains("private host")));
    assert!(asked.lock().unwrap().contains(&None));
    m.stop_all(Duration::from_secs(5)).await;
}

#[test]
fn put_resource_builds_each_kind_and_refuses_the_rest() {
    let m = manager();
    let o = OrgId::default();
    for (kind, body) in [
        ("cache", "memory: {}"),
        ("rate_limit", "local:\n  count: 5\n  interval: 1s"),
        ("processor", "mapping: 'root = this'"),
        ("input", "generate:\n  count: 1\n  mapping: 'root = 1'"),
        ("output", "drop: {}"),
    ] {
        m.put_resource(&o, kind, "r", body)
            .unwrap_or_else(|e| panic!("{kind}: {e:?}"));
    }
    assert!(matches!(
        m.put_resource(&o, "widget", "w", "drop: {}"),
        Err(ResourceError::Kind)
    ));
    assert!(matches!(
        m.put_resource(&o, "cache", "c", "a: [\n"),
        Err(ResourceError::Parse(_))
    ));
    assert!(matches!(
        m.put_resource(&o, "cache", "c", "5"),
        Err(ResourceError::Parse(_))
    ));
    assert!(matches!(
        m.put_resource(&o, "processor", "p", "mapping: 'root = ('"),
        Err(ResourceError::Build(_))
    ));
    let denied = manager().with_org_file_roots();
    assert!(matches!(
        denied.put_resource(&o, "cache", "c", "memory: {}"),
        Err(ResourceError::Parse(_))
    ));
}

#[test]
fn restore_resource_names_each_failure() {
    let m = manager();
    let o = OrgId::default();
    m.restore_resource(&o, "cache", "c", "memory: {}").unwrap();
    let err = m
        .restore_resource(
            &o,
            "cache",
            "c",
            "memory: {default_ttl: '${EKIBEN_VAR_T7583_GONE}'}",
        )
        .unwrap_err();
    assert!(
        err.contains("missing variables") && err.contains("EKIBEN_VAR_T7583_GONE"),
        "{err}"
    );
    assert_eq!(
        m.restore_resource(&o, "widget", "w", "drop: {}")
            .unwrap_err(),
        "unknown resource kind `widget`"
    );
    assert!(m
        .restore_resource(&o, "cache", "c", "a: [\n")
        .unwrap_err()
        .starts_with("Error: "));
    assert!(!m
        .restore_resource(&o, "processor", "p", "mapping: 'root = ('")
        .unwrap_err()
        .is_empty());
}

#[test]
fn register_resources_counts_what_it_registers() {
    let m = manager();
    let o = OrgId::default();
    let text = concat!(
        "cache_resources:\n  - label: c\n    memory: {}\n  - label: '${EKIBEN_VAR_T7583_LABEL:c2}'\n    memory: {}\n",
        "rate_limit_resources:\n  - label: rl\n    local:\n      count: 1\n      interval: 1s\n",
        "processor_resources:\n  - label: p\n    mapping: 'root = this'\n",
        "input_resources:\n  - label: i\n    generate:\n      count: 1\n      mapping: 'root = 1'\n",
        "output_resources:\n  - label: out\n    drop: {}\n",
    );
    assert_eq!(m.register_resources(&o, text).unwrap(), 6);
    assert!(m.register_resources(&o, "input: [\n").is_err());
    assert!(m
        .register_resources(
            &o,
            "processor_resources:\n  - label: p\n    mapping: 'root = ('\n"
        )
        .is_err());
    let denied = manager().with_org_file_roots();
    assert!(denied
        .register_resources(&o, "cache_resources: []\n")
        .is_err());
}

#[tokio::test]
async fn restore_starts_a_kept_stream_and_refuses_a_missing_variable() {
    let m = manager();
    let posted: Value = serde_norway::from_str(RESIDENT).unwrap();
    m.restore(org("city-a"), "kept".into(), posted)
        .await
        .unwrap();
    assert!(m.read("kept", Some(&org("city-a"))).await.unwrap().active);
    let missing = json!({"input": {"generate": {"mapping": "root = \"${EKIBEN_VAR_T7583_GONE}\""}}, "output": {"drop": {}}});
    let err = m
        .restore(org("city-a"), "m".into(), missing)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("missing variables"), "{err}");
    let err = m
        .restore(org("city-a"), "b".into(), json!({"input": 5}))
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    m.stop_all(Duration::from_secs(5)).await;
    assert!(m.list(Some(&org("city-a"))).await.is_empty());
}

#[tokio::test]
async fn create_read_update_and_patch_answer_not_found_or_conflict() {
    let m = manager();
    m.create("s".into(), conf(RESIDENT)).await.unwrap();
    let err = m.create("s".into(), conf(RESIDENT)).await.unwrap_err();
    assert!(matches!(err, CoreError::KeyAlreadyExists(_)));
    assert!(matches!(
        m.read_stats("x", None).await,
        Err(CoreError::KeyNotFound(_))
    ));
    assert!(m.read_stats("s", None).await.unwrap().is_object());
    assert!(matches!(
        m.update("x", conf(RESIDENT), None).await,
        Err(CoreError::KeyNotFound(_))
    ));
    assert!(m.update("s", conf(RESIDENT), None).await.unwrap().is_none());
    assert!(matches!(
        m.patch("x", &json!({}), None).await,
        Err(CoreError::KeyNotFound(_))
    ));
    let err = m.patch("s", &json!({"input": 5}), None).await.unwrap_err();
    assert!(!err.to_string().is_empty());
    let status = m.read("s", None).await.unwrap();
    assert!(status.active && status.uptime_str.ends_with('s'));
    // A run-once stream keeps its record and frozen uptime.
    let summary = m
        .create("once".into(), conf(ONCE))
        .await
        .unwrap()
        .expect("summary");
    assert!(summary.success);
    let once = m.read("once", None).await.unwrap();
    assert!(!once.active);
    assert_eq!(once.last_run.unwrap().messages_in, 1);
    assert!(m.not_ready_streams().await.is_empty());
    m.stop_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_new_version_that_does_not_start_brings_back_the_previous_one() {
    let m = manager();
    m.create("res".into(), conf(RESIDENT)).await.unwrap();
    assert!(m.update("res", conf(BROKEN), None).await.is_err());
    assert!(
        m.read("res", None).await.unwrap().active,
        "the resident stream runs again"
    );
    m.create("once".into(), conf(ONCE)).await.unwrap();
    assert!(m.update("once", conf(BROKEN), None).await.is_err());
    let back = m.read("once", None).await.unwrap();
    assert!(back.last_run.is_some(), "the finished run keeps its record");
    assert!(m.replace("fresh", None, conf(BROKEN), None).await.is_err());
    assert!(m.read("fresh", None).await.is_err());
    m.stop_all(Duration::from_secs(5)).await;
}

/// Refuses deletes or saves as told.
#[derive(Default)]
struct Host {
    refuse_saves: Mutex<bool>,
    refuse_deletes: Mutex<bool>,
    resources: Mutex<Vec<String>>,
}

impl StreamPersistence for Host {
    fn saved<'a>(
        &'a self,
        _: &'a OrgId,
        _: &'a str,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *self.refuse_saves.lock().unwrap() {
                Err("database down".to_string())
            } else {
                Ok(())
            }
        })
    }
    fn deleted<'a>(&'a self, _: &'a OrgId, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if *self.refuse_deletes.lock().unwrap() {
                Err("database read-only".to_string())
            } else {
                Ok(())
            }
        })
    }
    fn resource_saved<'a>(
        &'a self,
        _: &'a OrgId,
        kind: &'a str,
        id: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.resources.lock().unwrap().push(format!("{kind}/{id}"));
            Ok(())
        })
    }
}

/// Keeps nothing but streams: the default `resource_saved` answers Ok.
struct StreamsOnly;

impl StreamPersistence for StreamsOnly {
    fn saved<'a>(
        &'a self,
        _: &'a OrgId,
        _: &'a str,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn deleted<'a>(&'a self, _: &'a OrgId, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_refused_delete_keeps_the_stream_and_a_config_dir_reports_it() {
    let host = Arc::new(Host::default());
    *host.refuse_deletes.lock().unwrap() = true;
    let m = manager().with_persistence(host.clone());
    let posted: Value = serde_norway::from_str(RESIDENT).unwrap();
    m.create_posted("s".into(), conf(RESIDENT), Some(posted))
        .await
        .unwrap();
    let err = m.delete("s", None).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("was not removed: database read-only"),
        "{err}"
    );
    assert!(m.read("s", None).await.unwrap().active);
    // Replacing goes through the same remove: refused, nothing changes.
    assert!(m.update("s", conf(RESIDENT), None).await.is_err());

    let dir = tempfile::tempdir().unwrap();
    let mut cd = ekiben_server::config_dir::ConfigDir::new(dir.path().to_path_buf());
    std::fs::write(dir.path().join("d.yaml"), RESIDENT).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    std::fs::remove_file(dir.path().join("d.yaml")).unwrap();
    let errors = cd.sync(&m).await;
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("d: delete failed: "), "{errors:?}");

    m.save_resource(&OrgId::default(), "cache", "c", "memory: {}")
        .await
        .unwrap();
    assert_eq!(*host.resources.lock().unwrap(), vec!["cache/c"]);
    let plain = manager();
    plain
        .save_resource(&OrgId::default(), "cache", "c", "memory: {}")
        .await
        .unwrap();
    let streams_only = manager().with_persistence(Arc::new(StreamsOnly));
    streams_only
        .save_resource(&OrgId::default(), "cache", "c", "memory: {}")
        .await
        .unwrap();
    m.stop_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_previous_version_that_cannot_be_saved_again_is_reported() {
    let host = Arc::new(Host::default());
    let m = manager().with_persistence(host.clone());
    let posted: Value = serde_norway::from_str(RESIDENT).unwrap();
    m.create_posted("r".into(), conf(RESIDENT), Some(posted))
        .await
        .unwrap();
    *host.refuse_saves.lock().unwrap() = true;
    let err = m
        .update("r", conf(BROKEN), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("the previous version could not be restored"),
        "{err}"
    );
    assert!(err.contains("database down"), "{err}");
    assert!(
        m.read("r", None).await.is_err(),
        "nothing runs under the id"
    );
}

/// Saves after a pause, so two creates of one id both pass the existence check.
struct SlowSave;

impl StreamPersistence for SlowSave {
    fn saved<'a>(
        &'a self,
        _: &'a OrgId,
        _: &'a str,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
    }
    fn deleted<'a>(&'a self, _: &'a OrgId, _: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_creates_of_one_id_racing_leave_one_stream() {
    let m = manager().with_persistence(Arc::new(SlowSave));
    let posted: Value = serde_norway::from_str(RESIDENT).unwrap();
    let (a, b) = tokio::join!(
        m.create_posted("r".into(), conf(RESIDENT), Some(posted.clone())),
        m.create_posted("r".into(), conf(RESIDENT), Some(posted)),
    );
    assert!(a.is_ok() != b.is_ok(), "{a:?} {b:?}");
    let lost = a.err().or(b.err()).unwrap();
    assert!(matches!(lost, CoreError::KeyAlreadyExists(_)), "{lost}");
    assert_eq!(m.list(None).await.len(), 1);

    let slow_once = "run: once\ninput:\n  generate:\n    count: 2\n    interval: 100ms\n    mapping: 'root = 1'\noutput:\n  drop: {}\n";
    let (a, b) = tokio::join!(
        m.create("o".into(), conf(slow_once)),
        m.create("o".into(), conf(slow_once)),
    );
    assert!(a.is_ok() != b.is_ok(), "{a:?} {b:?}");
    assert!(matches!(
        a.err().or(b.err()).unwrap(),
        CoreError::KeyAlreadyExists(_)
    ));
    m.stop_all(Duration::from_secs(5)).await;
}
