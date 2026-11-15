//! A large file streams through the `csv` input: peak memory stays a small fraction of the file.
//! Its own test binary, so no other test moves the process's peak.

use std::io::Write;
use std::sync::Arc;

use ekiben_core::{ComponentConfig, CoreError, OrgId, Registry, ResourceManager, StreamContext};

#[tokio::test]
async fn a_large_csv_file_streams_in_bounded_memory() {
    const FILE_BYTES: u64 = 64 << 20;
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("big.csv");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    writeln!(f, "id,name,value,comment").unwrap();
    let mut i = 0u64;
    while f.get_ref().metadata().unwrap().len() < FILE_BYTES {
        for _ in 0..10_000 {
            writeln!(f, "{i},sensor-{i},{}.5,\"a quoted, comma field\"", i % 997).unwrap();
            i += 1;
        }
        f.flush().unwrap();
    }
    drop(f);

    let reg = Arc::new(Registry::new());
    ekiben_io_file::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org-file"));
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let sctx = StreamContext {
        stream_id: "mem".into(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: ekiben_core::StreamMetrics::new("mem", "org-file"),
        label: "root".into(),
        path: "root".into(),
        file_root: Some(tmp.path().to_path_buf()),
        env_vars: Arc::new(Default::default()),
        shutdown_rx: rx,
    };
    let conf = ComponentConfig::from_json_value(
        &serde_json::json!({"csv": {"paths": ["big.csv"], "batch_count": 100}}),
        "stdin",
        None,
    )
    .unwrap();
    let mut input = reg.build_input(&conf, &sctx).unwrap();

    let before = peak_rss();
    let mut rows = 0u64;
    loop {
        match input.read_batch(&sctx).await {
            Ok((batch, ack)) => {
                rows += batch.len() as u64;
                ack.ack();
            }
            Err(CoreError::EndOfInput) => break,
            Err(e) => panic!("{e}"),
        }
    }
    let grew = peak_rss().saturating_sub(before);
    assert_eq!(rows, i);
    assert!(
        grew < FILE_BYTES / 4,
        "peak memory grew {grew} bytes reading a {FILE_BYTES}-byte file"
    );
}

fn peak_rss() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .unwrap()
        * 1024
}
