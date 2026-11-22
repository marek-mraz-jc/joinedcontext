#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // The test packs with the git client; shako runs none of this.
//! T-7522: a job asks its memory permit for at least what it then holds. Each case compares the
//! largest `admit` with the peak the process reached; the cases re-execute the test binary with a
//! fixed glibc mmap threshold, so a reading is what is held (jc runs jemalloc, which purges).

use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, Principal, RefPolicy, RepoId, RequestCtx,
    RoleBinding, ShakoResult, Subject,
};
use shako_git::{
    PushGuard, handle_receive_pack, handle_upload_pack, init_bare_repo, read_negotiation,
    write_flush, write_text_line,
};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const MIB: u64 = 1024 * 1024;
/// What a reading may exceed the admitted bytes by: the test's own buffers and allocator slack.
const SLACK: u64 = 8 * MIB;
const MALLOC: (&str, &str) = ("MALLOC_MMAP_THRESHOLD_", "1048576");

/// Runs `name` again in a process of its own with the threshold fixed; true in that process.
fn in_own_process(name: &str) -> bool {
    if std::env::var_os(MALLOC.0).is_some() {
        return true;
    }
    let status = Command::new(std::env::current_exe().expect("this test"))
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(MALLOC.0, MALLOC.1)
        .status()
        .expect("the test runs again");
    assert!(status.success(), "{status}");
    false
}

fn held() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("status");
    let kib = |field: &str| -> u64 {
        status
            .lines()
            .find_map(|l| l.strip_prefix(field))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .expect("a reading")
    };
    (kib("RssAnon:") + kib("RssShmem:")) * 1024
}

/// Runs `job` with an `admit` that records the largest ask; answers the job's result, that ask
/// and the bytes the process added at its peak.
fn measure<T>(job: impl FnOnce(&mut dyn FnMut(u64) -> ShakoResult<()>) -> T) -> (T, u64, u64) {
    let before = held();
    let peak = Arc::new(AtomicU64::new(before));
    let done = Arc::new(AtomicBool::new(false));
    let sampler = std::thread::spawn({
        let (peak, done) = (Arc::clone(&peak), Arc::clone(&done));
        move || {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(held(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });
    let mut asked = 0u64;
    let out = job(&mut |bytes| {
        asked = asked.max(bytes);
        Ok(())
    });
    done.store(true, Ordering::Relaxed);
    sampler.join().expect("sampler");
    (out, asked, peak.load(Ordering::Relaxed) - before)
}

fn git(dir: &Path, args: &[&str], stdin: &[u8]) -> Vec<u8> {
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin)
        .expect("write");
    let out = child.wait_with_output().expect("git ran");
    assert!(out.status.success(), "git {args:?}");
    out.stdout
}

/// A thin push appending one line to a large file sends a delta of a few KiB; resolving it holds
/// the base, the result and its compressed form.
#[test]
fn a_thin_delta_push_asks_for_its_base_and_result() {
    if !in_own_process("a_thin_delta_push_asks_for_its_base_and_result") {
        return;
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let work = tmp.path().join("w");
    std::fs::create_dir(&work).expect("mkdir");
    git(&work, &["init", "-q", "-b", "main"], b"");
    let commit = |message: &str| {
        git(&work, &["add", "."], b"");
        git(
            &work,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-q",
                "-m",
                message,
            ],
            b"",
        );
        String::from_utf8(git(&work, &["rev-parse", "HEAD"], b""))
            .expect("hex")
            .trim()
            .to_owned()
    };
    let mut blob = vec![0u8; 24 << 20];
    std::fs::File::open("/dev/urandom")
        .expect("urandom")
        .read_exact(&mut blob)
        .expect("random");
    std::fs::write(work.join("big"), &blob).expect("write");
    let one = commit("one");
    blob.extend_from_slice(b"one more line\n");
    std::fs::write(work.join("big"), &blob).expect("write");
    drop(blob);
    let two = commit("two");

    let (instance, org) = (InstanceId::from_u128(1), OrgId::from_u128(1));
    let dest = init_bare_repo(
        tmp.path(),
        instance,
        RepoId::from_u128(9),
        "main",
        false,
        "D",
    )
    .expect("repository");
    let authz = LocalAuthorizer::new(vec![RoleBinding {
        org_id: org,
        principal: Principal::User("p".into()),
        actions: vec![Action::Write],
        project_id: None,
        repo: None,
    }]);
    let ctx = RequestCtx::new(instance, org, Subject::authenticated("p", "p", vec![org]));
    let policy = RefPolicy::default();
    let guard = PushGuard {
        ctx: &ctx,
        authorizer: &authz,
        owner: "o",
        repo_name: "r",
        policy: &policy,
    };
    let zero = "0".repeat(40);
    for (old, new, revs) in [
        (zero.as_str(), one.as_str(), format!("{one}\n")),
        (one.as_str(), two.as_str(), format!("{two}\n^{one}\n")),
    ] {
        let pack = git(
            &work,
            &[
                "pack-objects",
                "--stdout",
                "--thin",
                "--revs",
                "--delta-base-offset",
                "-q",
            ],
            revs.as_bytes(),
        );
        let mut body = tempfile::tempfile().expect("spool");
        write_text_line(
            &mut body,
            &format!("{old} {new} refs/heads/main\0 report-status"),
        )
        .expect("command");
        write_flush(&mut body).expect("flush");
        body.write_all(&pack).expect("pack");
        let size = pack.len();
        drop(pack);
        let (out, asked, peak) =
            measure(|admit| handle_receive_pack(&dest, &guard, 512 * MIB, &body, admit));
        let report = String::from_utf8_lossy(&out.expect("push").response_bytes).into_owned();
        println!(
            "pack of {size} bytes: asked {} MiB, peak +{} MiB",
            asked / MIB,
            peak / MIB
        );
        assert!(report.contains("ok refs/heads/main"), "{report}");
        assert!(
            peak <= asked + SLACK,
            "pack of {size} bytes held +{} MiB, asked {} MiB",
            peak / MIB,
            asked / MIB
        );
    }
}

/// An upload-pack request asks before it is inflated and parsed; a request of short lines is
/// refused before its list costs more than asked.
#[test]
fn a_negotiation_asks_before_it_is_held() {
    if !in_own_process("a_negotiation_asks_before_it_is_held") {
        return;
    }
    let tmp = tempfile::tempdir().expect("tmp");
    git(tmp.path(), &["init", "-q", "--bare", "r.git"], b"");
    let repo = tmp.path().join("r.git");
    let have = format!("0032have {}\n", "1".repeat(40));
    for (mib, line) in [
        (16u64, "0000"),
        (16, "0005x"),
        (4, have.as_str()),
        (16, have.as_str()),
    ] {
        let mut gz = flate2::write::GzEncoder::new(
            tempfile::tempfile().expect("spool"),
            flate2::Compression::best(),
        );
        for _ in 0..(mib * MIB) as usize / line.len() {
            gz.write_all(line.as_bytes()).expect("write");
        }
        let body = gz.finish().expect("gzip");
        let (res, asked, peak) = measure(|admit| {
            let request = read_negotiation(&body, true, 16 * MIB, &mut *admit)?;
            handle_upload_pack(&repo, None, &request, std::io::sink(), admit)
        });
        let short = &line[..line.len().min(9)];
        println!(
            "{mib} MiB of {short:?}: asked {} MiB, peak +{} MiB, {res:?}",
            asked / MIB,
            peak / MIB
        );
        assert!(
            peak <= asked + SLACK,
            "{mib} MiB of {short:?} held +{} MiB, asked {} MiB",
            peak / MIB,
            asked / MIB
        );
    }
}
