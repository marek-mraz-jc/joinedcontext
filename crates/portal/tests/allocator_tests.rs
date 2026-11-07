//! The Portal binary's allocator against the reconcile's largest short-lived allocations
//! (T-5283): the runner's whole /metrics text per project with a Live stream, read, scraped and
//! freed every reconcile, and the mirror each sync builds afresh from the config repository's
//! manifests and swaps in for the old one (`load_mirror` in `src/reconciler/daemon.rs`).

use std::time::Duration;

/// The binary's allocator (`src/main.rs`), so this test measures what the image runs.
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// The resident memory of this process now, in KiB (Linux `VmRSS`).
fn resident_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .expect("/proc/self/status")
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmRSS")
}

/// Projects with a Live stream, and the /metrics text each one's runner answers: 16 x 1.9 MiB,
/// prod's 30 MB (222,075 lines, T-5282).
const PROJECTS: usize = 16;
const LINES: usize = 14_000;

/// One runner's /metrics text as the HTTP client collects it: the body arrives in 16 KiB chunks
/// and the buffer grows as they come.
async fn scrape_once(project: usize) -> String {
    let mut body = Vec::new();
    let mut line = 0;
    while line < LINES {
        let mut chunk = String::with_capacity(16 << 10);
        while chunk.len() < (16 << 10) - 160 && line < LINES {
            chunk.push_str(&format!(
                "output_sent{{label=\"\",path=\"root.output\",stream=\"p{project}-pipeline-{line}\"}} {line}\n"
            ));
            line += 1;
        }
        body.extend_from_slice(chunk.as_bytes());
        tokio::task::yield_now().await;
    }
    String::from_utf8(body).expect("utf-8")
}

/// A sync's fresh mirror: the config repository's 2,318 manifests (dev, T-5283), each parsed into
/// small JSON blocks on whichever worker thread the sync runs (the mirror on dev is 27.6 MB, T-5284).
async fn fresh_mirror(sync: usize) -> Vec<serde_json::Value> {
    (0..2_318)
        .map(|n| {
            let fields: Vec<String> = (0..6)
                .map(|f| {
                    format!("\"field{f}\":{{\"name\":\"p{n}-s{sync}-{f}\",\"value\":[{f},{n}]}}")
                })
                .collect();
            serde_json::from_str(&format!("{{{}}}", fields.join(","))).expect("a manifest")
        })
        .collect()
}

/// Reconciles in a row: a fresh mirror swapped in for the old one, and every project's text read
/// at once and held until each stream's counters are folded from it, then dropped; the Portal keeps
/// 1 MiB of other small blocks per reconcile. Returns the MiB this process grew by at the end of
/// the last reconcile and, the mirror dropped, once it went quiet.
async fn reconciles() -> (u64, u64) {
    let before = resident_kib();
    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut mirror = Vec::new();
    let mut busy = 0;
    for sync in 0..8 {
        mirror = tokio::spawn(fresh_mirror(sync)).await.expect("joined");
        let reads: Vec<_> = (0..PROJECTS)
            .map(|project| tokio::spawn(scrape_once(project)))
            .collect();
        let mut texts = Vec::new();
        for read in reads {
            texts.push(read.await.expect("joined"));
        }
        // The stall watch's fold: each stream's own series, picked out of its project's text.
        let streams: Vec<Vec<String>> = texts
            .iter()
            .map(|text| text.lines().step_by(7).map(str::to_owned).collect())
            .collect();
        assert!(streams.iter().all(|lines| lines.len() == LINES / 7));
        kept.extend((0..1_000).map(|_| vec![1u8; 1024]));
        busy = resident_kib().saturating_sub(before) / 1024;
        drop((texts, streams));
    }
    assert_eq!(mirror.len(), 2_318);
    drop(mirror);
    // jemalloc purges what was freed after `dirty_decay_ms`, on its background thread; a quiet
    // Portal still answers a probe now and then.
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::hint::black_box(vec![0u8; 4096]);
    }
    let idle = resident_kib().saturating_sub(before) / 1024;
    drop(kept);
    (busy, idle)
}

/// Set in the child process of the test below, which runs the reconciles and prints what they left.
const CHILD: &str = "JC_PORTAL_TEST_ALLOCATOR_CHILD";

/// The allocator settings the image starts the Portal with, read from the Dockerfile.
fn deployed_malloc_conf() -> String {
    include_str!("../Dockerfile")
        .split_whitespace()
        .find_map(|word| word.strip_prefix("_RJEM_MALLOC_CONF="))
        .expect("the Dockerfile sets _RJEM_MALLOC_CONF")
        .to_owned()
}

/// T-5283: the Portal on dev held 474 Mi against its 320 Mi request with glibc's malloc
/// (MALLOC_ARENA_MAX=2), and its mirror is 27.6 MB of that (T-5284). The reconciles run in a
/// child process of their own, with the image's allocator settings, so their memory is no other
/// test's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_metrics_texts_of_a_reconcile_leave_once_it_ends() {
    const NAME: &str = "the_metrics_texts_of_a_reconcile_leave_once_it_ends";
    if std::env::var_os(CHILD).is_some() {
        let (busy, idle) = reconciles().await;
        eprintln!("reconciles MiB {busy} {idle}");
        return;
    }
    assert!(
        include_str!("../src/main.rs").contains(
            "static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;"
        ),
        "the binary no longer allocates with jemalloc, which this test measures"
    );
    let output = std::process::Command::new(std::env::current_exe().expect("this test"))
        .args(["--exact", NAME, "--nocapture"])
        .env(CHILD, "1")
        .env("_RJEM_MALLOC_CONF", deployed_malloc_conf())
        .output()
        .expect("the reconciles ran");
    let printed = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{printed}");
    let (busy, idle) = printed
        .lines()
        .find_map(|line| line.strip_prefix("reconciles MiB "))
        .and_then(|mib| mib.trim().split_once(' '))
        .and_then(|(busy, idle)| Some((busy.parse::<u64>().ok()?, idle.parse::<u64>().ok()?)))
        .unwrap_or_else(|| panic!("no measurement in {printed}"));
    eprintln!("a reconcile grew the process by {busy} MiB and the quiet left {idle} MiB");
    assert!(
        busy > 24,
        "a reconcile held only {busy} MiB: this no longer measures the texts"
    );
    // Measured (release, 4 worker threads, T-5283): jemalloc 80-95 MiB at the end of a reconcile
    // and 11-12 MiB left, 8 MiB of it `kept`; glibc's malloc with MALLOC_ARENA_MAX=2, which dev
    // ran, 65-78 MiB and 44-55 MiB left; glibc's defaults with a mirror ten times this one's
    // size 566-703 MiB and 557-691 MiB left.
    assert!(idle < 24, "the reconciles left {idle} MiB behind");
}
