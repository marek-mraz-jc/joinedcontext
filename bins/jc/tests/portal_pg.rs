#![allow(clippy::disallowed_methods)] // These tests start the jc binary and read their database URL.
//! The Portal inside jc on PostgreSQL (T-7820): the owner migrates, a login that owns nothing
//! serves, the jc.toml Organizations and hosts are seeded into the registry, and the host table
//! follows the registry through its notifications, a failed read and a lost listener; a Portal
//! database named without its owner, with an owner that is no Postgres URL or with a request
//! login inside the owner is refused, the password never printed.
//!
//! Runs when `JC_PG_TEST_DATABASE_URL` names a superuser connection that only prepares.

mod common;

use common::{free_port, lost_the_port};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const OWNER: &str = "jc_ppg_owner";
const APP: &str = "jc_ppg_app";
const MEMBER: &str = "jc_ppg_member";

fn password(role: &str) -> String {
    format!("pw-{role}-7820")
}

/// One ALTER ROLE at a time: the tests of this binary share the roles.
static ROLES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Db {
    base: PgConnectOptions,
    name: String,
}

impl Db {
    fn url(&self, role: &str) -> String {
        format!(
            "postgres://{role}:{}@{}:{}/{}",
            password(role),
            self.base.get_host(),
            self.base.get_port(),
            self.name
        )
    }

    async fn admin(&self) -> sqlx::PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(self.base.clone().database(&self.name))
            .await
            .unwrap()
    }
}

/// A database owned by [`OWNER`], [`APP`] a login that owns nothing and [`MEMBER`] a login
/// inside the owner role.
async fn prepare(tag: &str) -> Option<Db> {
    let Ok(url) = std::env::var("JC_PG_TEST_DATABASE_URL") else {
        println!("SKIP: JC_PG_TEST_DATABASE_URL is unset");
        return None;
    };
    let base = PgConnectOptions::from_str(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .unwrap();
    let name = format!("jc_ppg_{tag}_{:x}", std::process::id());
    let roles = ROLES.lock().await;
    for role in [OWNER, APP, MEMBER] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{}';",
            password(role)
        )))
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("GRANT {OWNER} TO {MEMBER}")))
        .execute(&admin)
        .await
        .unwrap();
    drop(roles);
    for statement in [
        format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        format!("CREATE DATABASE {name} OWNER {OWNER}"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .unwrap();
    }
    Some(Db { base, name })
}

struct Jc {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

impl Drop for Jc {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
    }
}

impl Jc {
    fn text(&self) -> String {
        self.log.lock().unwrap().join("\n")
    }

    /// Waits until the log holds `needle` (`count` times), else fails with the log.
    async fn wait_for(&self, needle: &str, count: usize, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let text = self.text();
            if text.matches(needle).count() >= count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no `{needle}` x{count} in jc's log:\n{text}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// jc with every module in host mode on `localhost`, the two Organizations of
/// `tests/fixtures/two-orgs.toml`, the Portal on `serving` and migrating as `migration`.
fn spawn(dir: tempfile::TempDir, serving: &str, migration: Option<&str>) -> Jc {
    let config = dir.path().join("jc.toml");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-orgs.toml");
    let text = format!(
        "mode = \"host\"\n{}",
        std::fs::read_to_string(fixture).unwrap()
    );
    std::fs::write(&config, text).unwrap();
    let given = dir.path().join("given");
    std::fs::create_dir_all(&given).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    cmd.args(["--profile", "development"]);
    for (name, flag) in [
        ("shako-token", "--shako-token-file"),
        ("ekiben-token", "--ekiben-token-file"),
        ("catalogue-token", "--dataroom-portal-token-file"),
        ("dashboards-password", "--daiya-portal-password-file"),
    ] {
        let file = given.join(name);
        std::fs::write(&file, format!("test-{name}-{:032x}", std::process::id())).unwrap();
        cmd.arg(flag).arg(file);
    }
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") || key.starts_with("JC_") {
            cmd.env_remove(key);
        }
    }
    // `*.localhost` resolves to loopback: the Portal's sign-in realm at `idm.localhost` answers.
    let port = free_port();
    cmd.args(["--role", "all", "--scheme", "http", "--log-format", "text"])
        .arg("--config")
        .arg(&config)
        .args(["--listen", &format!("[::1]:{port}")])
        .args(["--base-host", &format!("localhost:{port}")])
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .args(["--identity-realm", "jc"])
        .env("JC_PORTAL_DATABASE_URL", serving)
        .env("RUST_LOG", "info,sqlx=warn,jc::modules::portal=debug")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(migration) = migration {
        cmd.env("JC_PORTAL_MIGRATION_DATABASE_URL", migration);
    }
    let mut child = cmd.spawn().expect("spawn jc");
    let stderr = child.stderr.take().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            sink.lock().unwrap().push(line);
        }
    });
    Jc {
        child,
        log,
        _dir: dir,
    }
}

/// [`spawn`] until it keeps its port, then waits for the Portal.
async fn start(db: &Db) -> Jc {
    for _ in 0..5 {
        let jc = spawn(
            tempfile::tempdir().unwrap(),
            &db.url(APP),
            Some(&db.url(OWNER)),
        );
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            let text = jc.text();
            if text.contains("portal: ready") {
                return jc;
            }
            if lost_the_port(&text) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !lost_the_port(&jc.text()) {
            panic!("jc did not start:\n{}", jc.text());
        }
    }
    panic!("jc lost its listen port to another bind 5 times");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_portal_seeds_and_follows_its_registry_on_postgres() {
    let Some(db) = prepare("run").await else {
        return;
    };
    let jc = start(&db).await;
    jc.wait_for("Organization registry: jc.toml Organizations seeded", 1, 30)
        .await;
    jc.wait_for("host registry: jc.toml hosts seeded", 1, 30)
        .await;
    jc.wait_for("host registry: host table rebuilt", 1, 30)
        .await;
    let admin = db.admin().await;
    let seeded: i64 = sqlx::query_scalar("SELECT count(*) FROM organization_hosts")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(seeded >= 8, "both Organizations' four hosts: {seeded}");
    // A notification rebuilds at once, not after the minute's timer.
    let before = jc.text().matches("host table rebuilt").count();
    sqlx::query("SELECT pg_notify('organization_hosts', '')")
        .execute(&admin)
        .await
        .unwrap();
    jc.wait_for("host registry: host table rebuilt", before + 1, 20)
        .await;
    // A registry the serving login may not read keeps the table and says so.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "REVOKE SELECT ON organization_hosts FROM {APP}"
    )))
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("SELECT pg_notify('organization_hosts', '')")
        .execute(&admin)
        .await
        .unwrap();
    jc.wait_for("host table kept, the rebuild failed", 1, 20)
        .await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "GRANT SELECT ON organization_hosts TO {APP}"
    )))
    .execute(&admin)
    .await
    .unwrap();
    // A listener the server ends while the login may not connect is lost; the next round
    // finds no notifications and rebuilds on the timer. The login is let in again after.
    let roles = ROLES.lock().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER ROLE {APP} NOLOGIN")))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
         WHERE usename = $1 AND query ILIKE 'LISTEN%'",
    )
    .bind(APP)
    .execute(&admin)
    .await
    .unwrap();
    jc.wait_for("notifications lost, listening again", 1, 30)
        .await;
    jc.wait_for("no notifications, rebuilding on the timer", 1, 30)
        .await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER ROLE {APP} LOGIN")))
        .execute(&admin)
        .await
        .unwrap();
    drop(roles);
    let text = jc.text();
    for secret in [password(APP), password(OWNER)] {
        assert!(!text.contains(&secret), "jc printed a password");
    }
}

/// Runs jc with this Portal database setting until its log names `refusal`; the Portal is
/// built again and again meanwhile, nothing else starts it.
async fn refused(serving: &str, migration: Option<&str>, refusal: &str) {
    let jc = spawn(tempfile::tempdir().unwrap(), serving, migration);
    jc.wait_for(refusal, 1, 120).await;
    let text = jc.text();
    assert!(!text.contains("portal: ready"), "{text}");
    for role in [APP, OWNER, MEMBER] {
        assert!(!text.contains(&password(role)), "jc printed a password");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsafe_portal_database_setting_is_refused() {
    let Some(db) = prepare("refuse").await else {
        return;
    };
    refused(
        &db.url(APP),
        None,
        "JC_PORTAL_DATABASE_URL is set without JC_PORTAL_MIGRATION_DATABASE_URL",
    )
    .await;
    refused(
        &db.url(APP),
        Some("jc_ppg_owner at the portal database"),
        "JC_PORTAL_MIGRATION_DATABASE_URL is not a Postgres URL",
    )
    .await;
    // A login inside the owner role could switch its tables' row-level security off: the
    // Portal's own check refuses it before jc's (`pg::refuse_unsafe_request_role`) is reached.
    refused(
        &db.url(MEMBER),
        Some(&db.url(OWNER)),
        &format!("the serving role `{MEMBER}` (JC_PORTAL_DATABASE_URL) owns, or is a member of the role that owns"),
    )
    .await;
}
