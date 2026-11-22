#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests run the git client; jc runs none of this.
//! T-7522: what a push and a clone cost the forge in memory. One test, a process of its own:
//! the reading is the whole process's.

mod git_world;

use git_world::{World, commit_random, git, working_copy};
use shako_core::OutboundPolicy;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const MIB: u64 = 1024 * 1024;
/// A push or clone may add this much, whatever the repository's size.
const BOUND_MIB: u64 = 64;

fn status_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .expect("status")
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .expect("a reading")
}

/// What the process holds that the page cache cannot drop.
fn held_kib() -> u64 {
    status_kib("RssAnon:") + status_kib("RssShmem:")
}

/// The anonymous and shared memory `work` added at its peak, in MiB. A pack the server maps
/// from disk is the page cache's and is not counted.
async fn peak_mib(work: impl Future<Output = ()>) -> u64 {
    let before = held_kib();
    let peak = Arc::new(AtomicU64::new(before));
    let done = Arc::new(AtomicBool::new(false));
    let sampler = std::thread::spawn({
        let (peak, done) = (Arc::clone(&peak), Arc::clone(&done));
        move || {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(held_kib(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    });
    work.await;
    done.store(true, Ordering::Relaxed);
    sampler.join().expect("sampler");
    (peak.load(Ordering::Relaxed) - before) * 1024 / MIB
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_and_a_clone_of_200_mib_stay_under_the_bound() {
    let world = World::start(OutboundPolicy::default()).await;
    let url = world.repository("city", "large").await;
    let work = tempfile::tempdir().expect("work");
    working_copy(work.path(), &url).await;
    commit_random(work.path(), 7, 200, 8).await;

    let push = peak_mib(git(work.path(), &["push", "-q", "origin", "main"])).await;
    let into = tempfile::tempdir().expect("into");
    let clone = peak_mib(git(into.path(), &["clone", "-q", "--bare", &url, "copy"])).await;
    println!("200 MiB repository: push +{push} MiB, clone +{clone} MiB at the peak");
    assert!(push < BOUND_MIB, "push +{push} MiB, bound {BOUND_MIB} MiB");
    assert!(
        clone < BOUND_MIB,
        "clone +{clone} MiB, bound {BOUND_MIB} MiB"
    );

    // Deltas: twelve versions of one file, which the client packs as a chain.
    let url = world.repository("city", "chained").await;
    let work = tempfile::tempdir().expect("work");
    working_copy(work.path(), &url).await;
    commit_random(work.path(), 11, 4, 4).await;
    let file = work.path().join("11-0.bin");
    for version in 1..12usize {
        let mut bytes = std::fs::read(&file).expect("read");
        bytes[version * 4096] ^= 0xff;
        std::fs::write(&file, bytes).expect("write");
        git(work.path(), &["commit", "-q", "-am", "edit"]).await;
    }
    let push = peak_mib(git(work.path(), &["push", "-q", "origin", "main"])).await;
    let into = tempfile::tempdir().expect("into");
    let clone = peak_mib(git(into.path(), &["clone", "-q", "--bare", &url, "copy"])).await;
    println!("a chain of 12 deltas on 4 MiB: push +{push} MiB, clone +{clone} MiB at the peak");
    assert!(push < BOUND_MIB, "push +{push} MiB, bound {BOUND_MIB} MiB");
    assert!(
        clone < BOUND_MIB,
        "clone +{clone} MiB, bound {BOUND_MIB} MiB"
    );
    git(
        &into.path().join("copy"),
        &["fsck", "--strict", "--no-progress"],
    )
    .await;
}
