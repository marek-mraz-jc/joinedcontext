#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests run the git client; jc runs none of this.
//! T-7522: a slow sender costs its own Organization alone, and a push mirror costs the push
//! nothing and the wire only what is new.

mod git_world;

use git_world::{World, commit_random, git, working_copy};
use serde_json::json;
use shako_core::OutboundPolicy;
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

/// A remote that takes three seconds over each push and records its size.
#[derive(Default)]
struct Remote {
    tip: Option<String>,
    pushes: Vec<usize>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mirror_follows_the_push_and_sends_only_the_new_objects() {
    let remote = Arc::new(Mutex::new(Remote::default()));
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
            std::thread::sleep(Duration::from_secs(3));
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
}
