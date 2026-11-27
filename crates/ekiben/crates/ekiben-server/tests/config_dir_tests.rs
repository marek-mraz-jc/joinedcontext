//! `ekiben streams --config-dir`: one directory (a ConfigMap in every replica) drives the
//! streams and organization quotas of a running server without a restart.

use std::time::Duration;

use ekiben_core::{OrgId, ResourceManager};
use ekiben_server::config_dir::{parse_quotas_file, ConfigDir};
use ekiben_server::StreamManager;
use ekiben_testkit::full_registry;

const STREAM: &str = r#"
org: org-a
input:
  generate:
    interval: 50ms
    mapping: 'root = {"v": 1}'
output:
  drop: {}
"#;

fn manager() -> StreamManager {
    StreamManager::new(full_registry(), ResourceManager::new(), None, false)
}

fn quotas(m: &StreamManager, org: &str) -> ekiben_core::OrgQuotas {
    m.resources.get_or_create_org(&OrgId::new(org)).get_quotas()
}

#[tokio::test]
async fn a_config_dir_creates_updates_and_deletes_streams_and_sets_quotas_live() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("ingest.yaml"), STREAM).unwrap();
    std::fs::write(
        p.join("quotas.yaml"),
        "org-a:\n  max_messages_per_sec: 1000\n",
    )
    .unwrap();
    std::fs::create_dir(p.join("..data")).unwrap(); // ConfigMap bookkeeping entries are skipped
    let m = manager();
    let mut cd = ConfigDir::new(p.to_path_buf());
    // Streams are keyed by (organization, id): the file's stream is listed under its `org`.
    let org_a = OrgId::new("org-a");

    assert!(cd.sync(&m).await.is_empty());
    assert!(m.list(Some(&org_a)).await.contains_key("ingest"));
    assert_eq!(quotas(&m, "org-a").max_messages_per_sec, 1000);

    // A quota change applies without touching the running stream.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    std::fs::write(p.join("quotas.yaml"), "org-a:\n  max_messages_per_sec: 7\n").unwrap();
    assert!(cd.sync(&m).await.is_empty());
    assert_eq!(quotas(&m, "org-a").max_messages_per_sec, 7);
    let up = m.list(Some(&org_a)).await["ingest"].uptime;
    assert!(
        up >= 1.0,
        "the stream kept running through the quota change ({up} s)"
    );

    // A refused quotas file keeps the previous quotas and is reported once.
    std::fs::write(p.join("quotas.yaml"), "org-a:\n  max_messages_per_sec: 0\n").unwrap();
    let errs = cd.sync(&m).await;
    assert!(
        errs[0].contains("max_messages_per_sec must be at least 1"),
        "{errs:?}"
    );
    assert_eq!(quotas(&m, "org-a").max_messages_per_sec, 7);
    assert!(cd.sync(&m).await.is_empty(), "reported once");

    // A broken stream file keeps the running version.
    std::fs::write(p.join("ingest.yaml"), "input: [not a stream").unwrap();
    assert_eq!(cd.sync(&m).await.len(), 1);
    assert!(m.list(Some(&org_a)).await["ingest"].active);

    // Removing quotas.yaml returns the organization to the defaults; removing the file stops
    // the stream.
    std::fs::remove_file(p.join("quotas.yaml")).unwrap();
    std::fs::remove_file(p.join("ingest.yaml")).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    assert!(!m.list(Some(&org_a)).await.contains_key("ingest"));
    assert_eq!(
        quotas(&m, "org-a").max_messages_per_sec,
        ekiben_core::OrgQuotas::default().max_messages_per_sec
    );
}

#[test]
fn a_quotas_file_refuses_unknown_fields_bad_limits_and_bad_org_ids() {
    assert!(parse_quotas_file("org-a:\n  max_mesages_per_sec: 5\n")
        .unwrap_err()
        .contains("unknown field `max_mesages_per_sec`"));
    assert!(parse_quotas_file("org-a:\n  max_messages_per_sec: -1\n").is_err());
    assert!(parse_quotas_file("../etc:\n  max_messages_per_sec: 5\n").is_err());
    assert!(parse_quotas_file("- a\n").is_err());
    let ok = parse_quotas_file("a:\n  nats_subject_prefix: a\nb: {}\n").unwrap();
    assert_eq!(ok["a"].nats_subject_prefix.as_deref(), Some("a"));
    assert!(ok.contains_key("b"));
}

#[test]
fn a_quotas_file_names_each_problem() {
    assert!(parse_quotas_file("~").unwrap().is_empty());
    assert!(parse_quotas_file("").unwrap().is_empty());
    assert!(parse_quotas_file("a: [\n").is_err());
    assert_eq!(
        parse_quotas_file("1: {}\n").unwrap_err(),
        "an organization id must be a string"
    );
    assert_eq!(
        parse_quotas_file("[1]\n").unwrap_err(),
        "quotas.yaml must map organization ids to quotas"
    );
    assert!(parse_quotas_file("bad org: {}\n")
        .unwrap_err()
        .contains("organization id 'bad org' contains disallowed characters"));
    assert_eq!(
        parse_quotas_file("org-a:\n  max_messages_per_sec: 0\n").unwrap_err(),
        "org-a: max_messages_per_sec must be at least 1"
    );
    assert!(parse_quotas_file("org-a:\n  max_messages_per_sec: many\n")
        .unwrap_err()
        .starts_with("org-a: field `max_messages_per_sec`: "));
    // An array would set fields by position; a non-string field name is no field.
    assert_eq!(
        parse_quotas_file("org-a: [1]\n").unwrap_err(),
        "org-a: expected a map of quota fields"
    );
    assert!(parse_quotas_file("org-a: {[1]: 2}\n")
        .unwrap_err()
        .starts_with("org-a: "));
}

#[tokio::test]
async fn a_missing_directory_is_reported_and_a_broken_file_once() {
    let m = manager();
    let dir = tempfile::tempdir().unwrap();
    let errors = ConfigDir::new(dir.path().join("gone")).sync(&m).await;
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("gone"), "{errors:?}");

    let mut cd = ConfigDir::new(dir.path().to_path_buf());
    std::fs::write(dir.path().join("broken.yaml"), "input: [\n").unwrap();
    std::fs::write(dir.path().join("quotas.yaml"), "org-a:\n  bogus: 1\n").unwrap();
    let errors = cd.sync(&m).await;
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors
        .iter()
        .any(|e| e.starts_with("quotas.yaml: org-a: unknown field `bogus`")));
    assert!(errors
        .iter()
        .any(|e| e.starts_with("broken:") && e.ends_with("the previous version keeps running")));
    // Reported once: the same text is not tried again every second.
    assert!(cd.sync(&m).await.is_empty());
    // Fixed, it starts; removed from the directory, it is no longer remembered.
    std::fs::write(dir.path().join("broken.yaml"), STREAM).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    assert!(m.read("broken", Some(&OrgId::new("org-a"))).await.is_ok());
    m.stop_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_changed_file_restarts_its_stream_and_odd_entries_are_skipped() {
    use std::os::unix::ffi::OsStrExt;
    let m = manager();
    let dir = tempfile::tempdir().unwrap();
    let mut cd = ConfigDir::new(dir.path().to_path_buf());
    std::fs::write(dir.path().join("s.yaml"), STREAM).unwrap();
    std::fs::create_dir(dir.path().join("folder.yaml")).unwrap();
    let odd = std::ffi::OsStr::from_bytes(b"bad-\xff.yaml");
    std::fs::write(dir.path().join(odd), STREAM).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    let org = OrgId::new("org-a");
    assert_eq!(m.list(Some(&org)).await.len(), 1, "only s.yaml is a stream");
    std::fs::write(dir.path().join("s.yaml"), STREAM.replace("50ms", "70ms")).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    let got = m.read("s", Some(&org)).await.unwrap();
    assert_eq!(got.config.unwrap()["input"]["generate"]["interval"], "70ms");
    m.stop_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn a_file_moved_to_another_organization_moves_its_stream() {
    let m = manager();
    let dir = tempfile::tempdir().unwrap();
    let mut cd = ConfigDir::new(dir.path().to_path_buf());
    std::fs::write(dir.path().join("s.yaml"), STREAM).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    std::fs::write(dir.path().join("s.yaml"), STREAM.replace("org-a", "org-b")).unwrap();
    assert!(cd.sync(&m).await.is_empty());
    assert!(m.read("s", Some(&OrgId::new("org-a"))).await.is_err());
    assert!(m.read("s", Some(&OrgId::new("org-b"))).await.is_ok());
    // A new version that does not build is refused; the running one stays.
    std::fs::write(
        dir.path().join("s.yaml"),
        STREAM.replace("org-a", "org-b").replace(
            "output:",
            "pipeline:\n  processors:\n    - mapping: 'root = ('\noutput:",
        ),
    )
    .unwrap();
    let errors = cd.sync(&m).await;
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(m.read("s", Some(&OrgId::new("org-b"))).await.is_ok());
    // Moved to another organization, a version that does not build is refused there too.
    std::fs::write(
        dir.path().join("s.yaml"),
        STREAM.replace("org-a", "org-c").replace(
            "output:",
            "pipeline:\n  processors:\n    - mapping: 'root = ('\noutput:",
        ),
    )
    .unwrap();
    let errors = cd.sync(&m).await;
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("s:"), "{errors:?}");
    assert!(m.read("s", Some(&OrgId::new("org-c"))).await.is_err());
    m.stop_all(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn watch_applies_the_directory_without_a_call() {
    let m = manager();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("w.yaml"), STREAM).unwrap();
    std::fs::write(dir.path().join("bad.yaml"), "input: [\n").unwrap();
    tokio::spawn(ConfigDir::new(dir.path().to_path_buf()).watch(m.clone()));
    let org = OrgId::new("org-a");
    for _ in 0..50 {
        if m.read("w", Some(&org)).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(m.read("w", Some(&org)).await.is_ok());
    m.stop_all(Duration::from_secs(5)).await;
}
