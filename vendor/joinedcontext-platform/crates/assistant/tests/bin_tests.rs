//! The `jc-assistant` binary as its pod runs it: each refusal at start names what failed, and a
//! configured service turns ready after its first minute and stops on SIGTERM or Ctrl-C.
//! The database cases need `JC_ASSISTANT_TEST_DATABASE_URL` (see store_tests.rs).

#[path = "common/db.rs"]
mod db;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A scratch directory of this test's own, empty.
fn scratch(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-assistant-bin-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

/// The binary with only `vars` set; the coverage profile path is passed through so the child's
/// counters land beside the tests'.
fn command(vars: &[(&str, String)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_jc-assistant"));
    command.env_clear().env("RUST_LOG", "info");
    for key in ["LLVM_PROFILE_FILE", "PATH"] {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    for (key, value) in vars {
        command.env(key, value);
    }
    command
}

fn run(vars: &[(&str, String)]) -> Output {
    command(vars).output().expect("the binary runs")
}

/// The binary failed and its log names `what`.
fn refused(vars: &[(&str, String)], what: &str) {
    let output = run(vars);
    let log = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{what}: it started\n{log}");
    assert!(log.contains(what), "the log names {what:?}\n{log}");
}

/// The settings a whole start needs: the database, an empty organization checkout, the shared
/// embedder (built, never called while nothing is crawled) and the chat's addresses.
fn configured(dir: &Path, url: &str) -> Vec<(&'static str, String)> {
    let secret = dir.join("client-secret");
    std::fs::write(&secret, "test-only-client-secret\n").expect("secret file");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).expect("checkout");
    vec![
        ("JC_ASSISTANT_DATABASE_URL", url.to_owned()),
        ("JC_ASSISTANT_REPO_DIR", repo.display().to_string()),
        (
            "JC_ASSISTANT_ASSEMBLY_DIR",
            dir.join("assembly").display().to_string(),
        ),
        ("JC_ASSISTANT_ORG_DOMAIN", "bin.example".into()),
        ("JC_ASSISTANT_EMBEDDER_URL", "http://127.0.0.1:9/embed".into()),
        (
            "JC_ASSISTANT_EMBEDDER_TOKEN_URL",
            "http://127.0.0.1:9/token".into(),
        ),
        (
            "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE",
            secret.display().to_string(),
        ),
        ("JC_ASSISTANT_GATEWAY_URL", "http://127.0.0.1:9".into()),
        ("JC_ASSISTANT_PROXY_URL", "http://127.0.0.1:9".into()),
        ("JC_ASSISTANT_TOKEN_URL", "http://127.0.0.1:9/token".into()),
        ("JC_ASSISTANT_LLM", "test-model".into()),
        ("JC_ASSISTANT_PORTAL_URL", "https://portal.bin.example".into()),
        (
            "JC_ASSISTANT_CLIENT_SECRET_FILE",
            secret.display().to_string(),
        ),
        ("HOSTNAME", "jc-assistant-bin-test".into()),
    ]
}

/// `vars` with `key` set to `value` (replaced when present).
fn with(vars: &[(&'static str, String)], key: &'static str, value: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<_> = vars.iter().filter(|(k, _)| *k != key).cloned().collect();
    out.push((key, value.to_owned()));
    out
}

fn without(vars: &[(&'static str, String)], key: &str) -> Vec<(&'static str, String)> {
    vars.iter().filter(|(k, _)| *k != key).cloned().collect()
}

/// A loopback port nothing listens on at the moment of asking.
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port()
}

#[test]
fn a_start_without_its_required_settings_names_them() {
    refused(
        &[],
        "JC_ASSISTANT_DATABASE_URL, JC_ASSISTANT_REPO_DIR and JC_ASSISTANT_ORG_DOMAIN are required",
    );
    // An empty value counts as unset.
    refused(
        &[
            ("JC_ASSISTANT_DATABASE_URL", "postgres://x".into()),
            ("JC_ASSISTANT_REPO_DIR", "/tmp".into()),
            ("JC_ASSISTANT_ORG_DOMAIN", "  ".into()),
        ],
        "are required",
    );
}

#[test]
fn an_unusable_embedder_stops_the_start() {
    let dir = scratch("embedder");
    let base = configured(&dir, "postgres://127.0.0.1:9/none");
    // The local model: a thread count out of range, then a directory with no model in it.
    let local = without(
        &without(
            &without(&base, "JC_ASSISTANT_EMBEDDER_URL"),
            "JC_ASSISTANT_EMBEDDER_TOKEN_URL",
        ),
        "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE",
    );
    refused(
        &with(&local, "JC_ASSISTANT_EMBED_THREADS", "0"),
        "JC_ASSISTANT_EMBED_THREADS is a number from 1 to 16",
    );
    refused(
        &with(&local, "JC_ASSISTANT_EMBED_THREADS", "17"),
        "the embedding model is not usable",
    );
    refused(
        &with(
            &local,
            "JC_ASSISTANT_MODEL_DIR",
            &dir.join("no-model").display().to_string(),
        ),
        "the embedding model is not usable",
    );
    refused(&local, "JC_ASSISTANT_MODEL_DIR or JC_ASSISTANT_EMBEDDER_URL is required");
    // The shared embedder: its secret file missing, then empty.
    refused(
        &with(
            &base,
            "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE",
            &dir.join("absent").display().to_string(),
        ),
        "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE could not be read",
    );
    let empty = dir.join("empty-secret");
    std::fs::write(&empty, " \n").expect("empty secret");
    refused(
        &with(
            &base,
            "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE",
            &empty.display().to_string(),
        ),
        "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE is empty",
    );
    refused(
        &without(&base, "JC_ASSISTANT_EMBEDDER_TOKEN_URL"),
        "JC_ASSISTANT_EMBEDDER_TOKEN_URL is required with JC_ASSISTANT_EMBEDDER_URL",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_database_address_that_does_not_parse_is_unreachable() {
    let dir = scratch("dburl");
    refused(
        &configured(&dir, "postgres://%zz@/"),
        "the assistant's database is not reachable",
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_after_the_database_names_its_setting() {
    let (admin, pool, name) = db::database("bin_refusals").await;
    let url = test_url(&name);
    let dir = scratch("refusals");
    let base = configured(&dir, &url);
    // The chat's credential: neither file, then both.
    refused(
        &without(&base, "JC_ASSISTANT_CLIENT_SECRET_FILE"),
        "JC_ASSISTANT_CLIENT_ASSERTION_FILE or JC_ASSISTANT_CLIENT_SECRET_FILE is required",
    );
    refused(
        &with(&base, "JC_ASSISTANT_CLIENT_ASSERTION_FILE", "/var/run/token"),
        "are both set",
    );
    // A missing chat address, then one the library refuses.
    refused(
        &without(&base, "JC_ASSISTANT_LLM"),
        "JC_ASSISTANT_LLM is required",
    );
    refused(
        &with(&base, "JC_ASSISTANT_VIZ_URL", "not a url"),
        "the chart renderer's address is not usable",
    );
    // An address the service cannot listen on.
    refused(
        &with(&base, "JC_ASSISTANT_BIND", "127.0.0.1:not-a-port"),
        "the chat and the health probe cannot listen",
    );
    // A migration whose recorded checksum no longer matches the file.
    sqlx::query("UPDATE _sqlx_migrations SET checksum = '\\x00'::bytea")
        .execute(&admin_of(&name).await)
        .await
        .expect("the record is changed");
    refused(&base, "the assistant's migrations did not run");
    let _ = std::fs::remove_dir_all(&dir);
    db::drop_database(admin, pool, &name).await;
}

/// The test server's address with `database` in place of its own.
fn test_url(database: &str) -> String {
    let url = std::env::var("JC_ASSISTANT_TEST_DATABASE_URL")
        .expect("JC_ASSISTANT_TEST_DATABASE_URL is set");
    let mut parsed = reqwest::Url::parse(&url).expect("the test address parses");
    parsed.set_path(database);
    parsed.to_string()
}

/// A superuser pool on `database`.
async fn admin_of(database: &str) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&test_url(database))
        .await
        .expect("the test database answers")
}

/// Starts the binary configured over `name`, waits until `/readyz` answers 200 (the first minute
/// went through), sends `signal` and returns its exit and log.
async fn ready_then(name: &str, test: &str, signal: &str) -> (bool, String) {
    let dir = scratch(test);
    let base = configured(&dir, &test_url(name));
    // One source due every minute whose host never resolves: the minute queues and works it.
    let sources = dir.join("repo/projects/binproj/assistant/sources");
    std::fs::create_dir_all(&sources).expect("sources directory");
    std::fs::write(
        sources.join("web.yaml"),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: KnowledgeSource\nmetadata:\n  name: web\n  namespace: binproj\nspec:\n  source: website\n  startUrls: [https://jc-bin-test.invalid/]\n  schedule: \"* * * * *\"\n",
    )
    .expect("source manifest");
    std::fs::write(
        sources.join("guide.yaml"),
        "apiVersion: joinedcontext.com/v1alpha1\nkind: KnowledgeSource\nmetadata:\n  name: guide\n  namespace: binproj\nspec:\n  source: guide\n",
    )
    .expect("guide manifest");
    let port = free_port();
    let mut child = command(&with(&base, "JC_ASSISTANT_BIND", &format!("127.0.0.1:{port}")))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut ready = false;
    while Instant::now() < deadline {
        if let Ok(answer) = client
            .get(format!("http://127.0.0.1:{port}/readyz"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            if answer.status() == reqwest::StatusCode::OK {
                ready = true;
                break;
            }
        }
        if child.try_wait().expect("child status").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Ready comes before the queueing: wait until both sources have a job, then a moment for
    // the minute to work them.
    let database = admin_of(name).await;
    while ready && Instant::now() < deadline {
        let jobs: i64 = sqlx::query_scalar("SELECT count(DISTINCT source) FROM crawl_jobs")
            .fetch_one(&database)
            .await
            .expect("the jobs are counted");
        if jobs >= 2 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    database.close().await;
    let pid = child.id().to_string();
    let killed = Command::new("kill")
        .args([signal, &pid])
        .status()
        .expect("kill runs");
    assert!(killed.success(), "the signal is sent");
    let output = child.wait_with_output().expect("the binary stops");
    let log = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(ready, "the service turned ready\n{log}");
    let _ = std::fs::remove_dir_all(&dir);
    (output.status.success(), log)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configured_service_turns_ready_and_stops_on_sigterm() {
    let (admin, pool, name) = db::database("bin_sigterm").await;
    let (stopped, log) = ready_then(&name, "sigterm", "-TERM").await;
    assert!(stopped, "SIGTERM stops it cleanly\n{log}");
    assert!(log.contains("stopping"), "{log}");
    assert!(log.contains("sources due"), "the due source was queued\n{log}");
    db::drop_database(admin, pool, &name).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_configured_service_stops_on_ctrl_c() {
    let (admin, pool, name) = db::database("bin_sigint").await;
    let (stopped, log) = ready_then(&name, "sigint", "-INT").await;
    assert!(stopped, "Ctrl-C stops it cleanly\n{log}");
    db::drop_database(admin, pool, &name).await;
}

/// A minute over a database that never answers logs each step it could not do, turns ready and
/// queues nothing, so the next minute tries the same minutes again.
#[tokio::test(flavor = "multi_thread")]
async fn a_minute_without_its_database_turns_ready_and_queues_nothing() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let dir = scratch("deadpool");
    let vars = configured(&dir, "unused");
    let sources = dir.join("repo/projects/deadproj/assistant/sources");
    std::fs::create_dir_all(&sources).expect("sources directory");
    for (name, spec) in [
        ("web", "source: website\n  startUrls: [https://jc-bin-test.invalid/]\n  schedule: \"* * * * *\""),
        ("guide", "source: guide"),
    ] {
        std::fs::write(
            sources.join(format!("{name}.yaml")),
            format!("apiVersion: joinedcontext.com/v1alpha1\nkind: KnowledgeSource\nmetadata:\n  name: {name}\n  namespace: deadproj\nspec:\n  {spec}\n"),
        )
        .expect("source manifest");
    }
    let lookup = |key: &str| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
    // Nothing listens on port 9 of the loopback: every acquire fails after its short timeout.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://postgres@127.0.0.1:9/none")
        .expect("a lazy pool");
    let embedder = assistant::embed::Embedder::from_lookup(&lookup, 1).expect("the remote embedder");
    let credential = assistant::host::credential(&lookup).expect("the credential");
    let config = assistant::host::chat_config(&lookup, credential).expect("the chat config");
    let (chat_state, reader) =
        assistant::service::chat_state(config, &pool, &embedder, "bin.example").expect("the chat");
    let ready = Arc::new(AtomicBool::new(false));
    let mut minute = assistant::host::Minute::new(
        pool,
        embedder,
        assistant::crawl::Crawler::public().expect("the crawler"),
        reader,
        "bin.example".into(),
        "dead".into(),
        assistant::host::checkout(lookup).expect("the checkout"),
        chat_state,
        Arc::clone(&ready),
    );
    minute.work().await;
    assert!(ready.load(Ordering::Relaxed), "a dead database still turns the pod ready");
    assert_eq!(minute.queued_until, None, "no minute counts as queued");
    assert!(minute.catalogue_digests.is_empty(), "no guide counts as queued");
    // A checkout that cannot be read leaves the minute before anything else.
    std::fs::remove_dir_all(dir.join("repo")).expect("the checkout goes");
    std::fs::write(dir.join("repo"), "a file, not a checkout").expect("the file");
    minute.work().await;
    assert_eq!(minute.queued_until, None);
    let _ = std::fs::remove_dir_all(&dir);
}
