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
