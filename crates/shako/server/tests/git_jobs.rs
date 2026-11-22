#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests run the git client; jc runs none of this.
//! T-7522: a slow sender costs its own Organization alone.

mod git_world;

use git_world::{World, commit_random, git, working_copy};
use shako_core::OutboundPolicy;
use std::time::Duration;
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
