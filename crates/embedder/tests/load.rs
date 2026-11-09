//! The load figure and the memory the service needs, on this host: the `jc-embedder` binary as
//! built, two deployments indexing at once and a question asked meanwhile. Ignored by default
//! (it takes a minute and reads the host's load); run it with
//!   cargo test --release -p jc-embedder --test load -- --ignored --nocapture
//! and JC_ASSISTANT_TEST_MODEL_DIR and ORT_DYLIB_PATH set.

mod common;

use jc_door::testing::Issuer;
use jc_embedder::Kind;
use jc_embedder::client::{Remote, RemoteConfig};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn status_kib(pid: u32, field: &str) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .expect("status")
        .lines()
        .find_map(|l| l.strip_prefix(field))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .expect("field")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement: run it by hand with --release"]
async fn two_deployments_index_while_a_question_is_asked() {
    let threads: usize = std::env::var("EMBEDDER_LOAD_THREADS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(2);
    let a = Arc::new(Issuer::start("https://id.a.example/realms/a", "a1").await);
    let b = Arc::new(Issuer::start("https://id.b.example/realms/b", "b1").await);
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let dir = std::env::temp_dir().join(format!("embedder-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let config = dir.join("embedder.toml");
    std::fs::write(
        &config,
        format!(
            "listen_addr = \"127.0.0.1:{port}\"\nmodel_path = {model:?}\nthreads = {threads}\n\
             [[deployments]]\nname = \"a\"\nissuer = {ai:?}\njwks_url = {aj:?}\nclients = [\"assistant-a\"]\nmax_texts_per_minute = 100000\n\
             [[deployments]]\nname = \"b\"\nissuer = {bi:?}\njwks_url = {bj:?}\nclients = [\"assistant-b\"]\nmax_texts_per_minute = 100000\n",
            model = std::env::var("JC_ASSISTANT_TEST_MODEL_DIR").expect("JC_ASSISTANT_TEST_MODEL_DIR"),
            ai = a.issuer,
            aj = a.jwks_url,
            bi = b.issuer,
            bj = b.jwks_url,
        ),
    )
    .expect("config");
    let started = Instant::now();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_jc-embedder"))
        .args(["--config", config.to_str().expect("path")])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("jc-embedder starts");
    let pid = child.id();
    let url = format!("http://127.0.0.1:{port}");
    while reqwest::get(format!("{url}/healthz")).await.is_err() {
        assert!(started.elapsed() < Duration::from_secs(60), "no /healthz");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let loaded = started.elapsed();
    let idle = status_kib(pid, "VmRSS:");
    let client = |issuer: Arc<Issuer>, id: &'static str| {
        let url = url.clone();
        async move {
            Remote::new(RemoteConfig {
                url,
                token_url: common::token_endpoint(issuer, id, "embedder").await,
                client_id: id.into(),
                client_secret: "s".into(),
            })
            .expect("client")
        }
    };
    let ra = Arc::new(client(Arc::clone(&a), "assistant-a").await);
    let rb = Arc::new(client(Arc::clone(&b), "assistant-b").await);
    // A chunk of the assistant's size (CHUNK_CHARS 1,200), as the indexer sends them.
    let passage = "Mestský úrad je otvorený v pondelok, stredu a piatok od 8:00 do 15:30; \
                   v utorok a vo štvrtok od 8:00 do 12:00. Podateľňa prijíma podania aj elektronicky. "
        .repeat(7);
    let per_deployment = 96;
    let began = Instant::now();
    let index = |r: Arc<Remote>, tag: &'static str| {
        let passage = passage.clone();
        tokio::spawn(async move {
            let batch: Vec<String> = (0..per_deployment)
                .map(|i| format!("{tag}{i} {passage}"))
                .collect();
            r.embed(Kind::Passage, &batch).await.expect("indexed").len()
        })
    };
    let (ia, ib) = (index(Arc::clone(&ra), "a"), index(Arc::clone(&rb), "b"));
    let mut waits = Vec::new();
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let asked = Instant::now();
        rb.embed(Kind::Query, &["Kedy je otvorený mestský úrad?".into()])
            .await
            .expect("question");
        waits.push(asked.elapsed());
    }
    let done = ia.await.expect("a") + ib.await.expect("b");
    let took = began.elapsed();
    let peak = status_kib(pid, "VmHWM:");
    println!(
        "threads {threads}, host load {:?}: model ready in {loaded:?}, RSS {} MiB idle, peak {} MiB; \
         {done} passages of {} bytes from two deployments in {took:.1?} = {:.1} texts/s; \
         a question during indexing took {:?} (max {:?})",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .next(),
        idle / 1024,
        peak / 1024,
        passage.len() + 4,
        done as f64 / took.as_secs_f64(),
        waits.iter().sum::<Duration>() / waits.len() as u32,
        waits.iter().max().expect("waits"),
    );
    child.kill().ok();
    child.wait().ok();
    std::fs::remove_dir_all(&dir).ok();
}
