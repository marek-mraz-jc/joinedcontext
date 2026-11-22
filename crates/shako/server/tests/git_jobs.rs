#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests run the git client; jc runs none of this.
//! T-7522: a slow sender costs its own Organization alone, and a push mirror costs the push
//! nothing and the wire only what is new.

mod git_world;

use git_world::{World, commit_random, git, working_copy};
use serde_json::json;
use shako_core::OutboundPolicy;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_organizations_slow_senders_do_not_refuse_another() {
    let world = World::start(OutboundPolicy::default()).await;
    world.repository("city", "slow").await;
    let town = world.repository("town", "quick").await;
    let mut senders = Vec::new();
    for _ in 0..24 {
        let mut socket = tokio::net::TcpStream::connect(world.addr)
            .await
            .expect("connect");
        let head = format!(
            "POST /city/slow.git/git-receive-pack HTTP/1.1\r\nHost: {}\r\n\
             Authorization: token city-token\r\nContent-Length: 1048576\r\n\r\n",
            world.addr
        );
        socket.write_all(head.as_bytes()).await.expect("send");
        senders.push(socket);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    let work = tempfile::tempdir().expect("work");
    working_copy(work.path(), &town).await;
    commit_random(work.path(), 3, 1, 1).await;
    git(work.path(), &["push", "-q", "origin", "main"]).await;
    git(work.path(), &["clone", "-q", "--bare", &town, "copy"]).await;
}

/// git gzips an upload-pack request over 1 KiB: a clone of many branches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clone_with_a_gzipped_negotiation_is_served() {
    let world = World::start(OutboundPolicy::default()).await;
    let url = world.repository("city", "branches").await;
    let work = tempfile::tempdir().expect("work");
    working_copy(work.path(), &url).await;
    commit_random(work.path(), 13, 1, 1).await;
    for n in 0..40 {
        git(
            work.path(),
            &["commit", "-q", "--allow-empty", "-m", &n.to_string()],
        )
        .await;
        git(work.path(), &["branch", &format!("b{n}")]).await;
    }
    git(work.path(), &["push", "-q", "--all", "origin"]).await;
    git(work.path(), &["clone", "-q", "--bare", &url, "copy"]).await;
}

/// A remote that takes three seconds over each push and records its size.
#[derive(Default)]
struct Remote {
    tip: Option<String>,
    pushes: Vec<usize>,
    /// How long the remote takes to answer a push.
    delay: Duration,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirror_follows_the_push_and_sends_only_the_new_objects() {
    let remote = Arc::new(Mutex::new(Remote {
        delay: Duration::from_secs(3),
        ..Remote::default()
    }));
    let seen = Arc::clone(&remote);
    let world = World::start(OutboundPolicy {
        allowed_hosts: Some(vec!["github.com".into()]),
        mock_handler: Some(Arc::new(move |method, _, _, body| {
            if method == "GET" {
                let mut out = Vec::new();
                let zero = "0".repeat(40);
                let line = match &seen.lock().expect("remote").tip {
                    Some(tip) => format!("{tip} refs/heads/main\0 report-status"),
                    None => format!("{zero} capabilities^{{}}\0 report-status"),
                };
                shako_git::write_text_line(&mut out, &line).ok()?;
                shako_git::write_flush(&mut out).ok()?;
                return Some((200, out));
            }
            let delay = seen.lock().expect("remote").delay;
            std::thread::sleep(delay);
            let mut remote = seen.lock().expect("remote");
            remote.tip = Some(String::from_utf8_lossy(body.get(45..85)?).into_owned());
            remote.pushes.push(body.len());
            Some((200, b"000eunpack ok\n0017ok refs/heads/main\n0000".to_vec()))
        })),
        ..OutboundPolicy::default()
    })
    .await;
    let url = world.repository("city", "mirrored").await;
    world
        .post(
            "city",
            "repos/city/mirrored/push_mirrors",
            json!({
                "remote_address": "https://github.com/joinedcontext-apps/mirrored.git",
                "remote_username": "x-access-token",
                "remote_password": "mirror-test",
                "interval": "8h0m0s",
                "sync_on_commit": true
            }),
        )
        .await;

    let work = tempfile::tempdir().expect("work");
    working_copy(work.path(), &url).await;
    let mut took = Vec::new();
    for (seed, mib) in [(5, 8), (9, 1)] {
        commit_random(work.path(), seed, mib, mib).await;
        let started = Instant::now();
        git(work.path(), &["push", "-q", "origin", "main"]).await;
        took.push(started.elapsed());
        let wanted = took.len();
        tokio::time::timeout(Duration::from_secs(60), async {
            while remote.lock().expect("remote").pushes.len() < wanted {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the mirror was pushed");
    }
    let sent = remote.lock().expect("remote").pushes.clone();
    println!("pushes took {took:?}; the mirror was sent {sent:?} bytes");
    assert!(
        sent[1] < 2 * 1024 * 1024,
        "the second mirror push sent {} bytes",
        sent[1]
    );
    assert!(
        took[1] < Duration::from_secs(2),
        "the push waited {:?}",
        took[1]
    );

    // "Sync now" while that repository's sync runs answers with its result.
    git(work.path(), &["commit", "-q", "--allow-empty", "-m", "3"]).await;
    git(work.path(), &["push", "-q", "origin", "main"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    world
        .post("city", "repos/city/mirrored/push_mirrors-sync", json!({}))
        .await;
    assert_eq!(
        remote.lock().expect("remote").pushes.len(),
        3,
        "sync now answered before the sync it waits for ended"
    );

    // A sync longer than "sync now" waits answers 503: the Portal reads an empty last error
    // after a success as a pushed mirror.
    remote.lock().expect("remote").delay = Duration::from_secs(12);
    git(work.path(), &["commit", "-q", "--allow-empty", "-m", "4"]).await;
    git(work.path(), &["push", "-q", "origin", "main"]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = world
        .post_status("city", "repos/city/mirrored/push_mirrors-sync", json!({}))
        .await;
    assert_eq!(status, 503, "a sync without its result is not a success");
}

/// Every object git wants for the new refs is in the delta: a merge of an older branch, content
/// that returns to an earlier state, a tag of a tag, tags of a tree and of a blob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mirror_delta_is_complete_to_git() {
    let work = tempfile::tempdir().expect("work");
    let dir = work.path();
    working_copy(dir, "unused").await;
    std::fs::create_dir(dir.join("kept")).expect("mkdir");
    for (path, text) in [("kept/a", "a"), ("b", "one")] {
        std::fs::write(dir.join(path), text).expect("write");
    }
    git(dir, &["add", "."]).await;
    git(dir, &["commit", "-q", "-m", "held"]).await;
    git(dir, &["branch", "older"]).await;
    std::fs::write(dir.join("b"), "two").expect("write");
    git(dir, &["commit", "-q", "-am", "held too"]).await;
    git(dir, &["clone", "-q", "--bare", ".", "remote.git"]).await;
    let remote = dir.join("remote.git");
    let held = refs_of(&remote).await;

    std::fs::write(dir.join("b"), "one").expect("write");
    git(dir, &["commit", "-q", "-am", "back"]).await;
    git(dir, &["checkout", "-q", "older"]).await;
    std::fs::write(dir.join("kept/c"), "c").expect("write");
    git(dir, &["add", "kept"]).await;
    git(dir, &["commit", "-q", "-m", "beside"]).await;
    git(dir, &["checkout", "-q", "main"]).await;
    git(dir, &["merge", "-q", "--no-ff", "-m", "merge", "older"]).await;
    for (tag, of) in [
        ("inner", "HEAD"),
        ("outer", "inner"),
        ("tree", "HEAD^{tree}"),
        ("blob", "HEAD:kept/c"),
    ] {
        git(dir, &["tag", "-a", "-m", tag, tag, of]).await;
    }
    git(dir, &["clone", "-q", "--mirror", ".", "forge.git"]).await;

    let mut body = Vec::new();
    shako_git::write_send_pack(&dir.join("forge.git"), &held, None, &mut body, &mut |_| {
        Ok(())
    })
    .expect("delta");
    let start = body
        .windows(4)
        .position(|bytes| bytes == b"PACK")
        .expect("a pack");
    std::fs::write(remote.join("objects/pack/pack-delta.pack"), &body[start..]).expect("write");
    git(
        &remote,
        &["index-pack", "--strict", "objects/pack/pack-delta.pack"],
    )
    .await;
    for (name, new) in &refs_of(&dir.join("forge.git")).await {
        git(&remote, &["update-ref", name, &new.to_string()]).await;
    }
    git(&remote, &["fsck", "--strict", "--no-dangling"]).await;
    assert_eq!(
        refs_of(&remote).await,
        refs_of(&dir.join("forge.git")).await
    );
}

async fn refs_of(repo: &std::path::Path) -> BTreeMap<String, gix_hash::ObjectId> {
    git(repo, &["pack-refs", "--all", "--no-prune"]).await;
    std::fs::read_to_string(repo.join("packed-refs"))
        .expect("packed-refs")
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(id, _)| !id.starts_with('#'))
        .map(|(id, name)| {
            let id = gix_hash::ObjectId::from_hex(id.as_bytes()).expect("an object id");
            (name.to_owned(), id)
        })
        .collect()
}
