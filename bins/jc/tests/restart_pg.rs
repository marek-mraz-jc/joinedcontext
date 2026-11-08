//! End to end on real Postgres: `jc --role all` with every module's `[<module>.database]`
//! section, a write through each public mount (a shako repository and a commit, a daiya
//! dashboard, a dataroom dataset, an ekiben stream, a kaisatsu user, an Antares entity), a stop,
//! a second start on the same database and state directory, and every write read back.
//!
//! Runs when `JC_PG_TEST_DATABASE_URL` names a superuser connection (PostGIS installed, for
//! Antares); prints `SKIP` otherwise. The superuser only prepares: one database, the login roles
//! of each module (request and migration roles, no SUPERUSER, no BYPASSRLS) and the extensions.
//! jc migrates every module itself, as its migration role, and serves as the request roles.

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

struct Jc {
    child: Child,
    base: String,
    state: PathBuf,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Jc {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn secret(&self, name: &str) -> String {
        std::fs::read_to_string(self.state.join(name))
            .unwrap()
            .trim()
            .to_owned()
    }

    /// SIGTERM, then waits for the exit (a clean stop is part of what a restart means).
    fn stop(mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Jc {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Request and migration login roles per module. Antares migrates as its own request role.
const ROLES: &[(&str, &str, &str)] = &[
    ("kaisatsu", "kaisatsu_app", "kaisatsu_migrate"),
    ("shako", "shako_app", "shako_migrate"),
    ("daiya", "daiya_app", "daiya_migrate"),
    ("dataroom", "dataroom_app", "dataroom_migrate"),
    ("ekiben", "ekiben_app", "ekiben_migrate"),
    ("antares", "antares_app", ""),
];

fn password() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).unwrap();
    hex::encode(buf)
}

/// Prepares the database as the superuser and writes jc's config: one `[<module>.database]`
/// section per module, every password in its own 0600-style file. `memory` names modules left
/// on their memory store (the red proof).
async fn prepare(dir: &Path, admin_url: &str, memory: &[&str]) -> PathBuf {
    let base = PgConnectOptions::from_str(admin_url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .expect("JC_PG_TEST_DATABASE_URL is set but unreachable");
    let db = format!("jc_restart_{:x}", std::process::id());
    for statement in [
        format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        format!("CREATE DATABASE {db}"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .unwrap();
    }
    let mut toml = String::from("[mounts.portal]\npath_prefix = \"\"\n");
    for (module, app, migrate) in ROLES {
        let mut section = None;
        for role in [app, migrate].into_iter().filter(|r| !r.is_empty()) {
            let pw = password();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
                 ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
                 GRANT CREATE, CONNECT ON DATABASE {db} TO {role};"
            )))
            .execute(&admin)
            .await
            .unwrap();
            let file = dir.join(format!("{role}-password"));
            std::fs::write(&file, pw).unwrap();
            let url = format!(
                "postgres://{role}@{}:{}/{db}",
                base.get_host(),
                base.get_port()
            );
            section = Some(match section {
                None => format!(
                    "[{module}.database]\nurl = \"{url}\"\npassword_file = {file:?}\nmax_connections = 3\n"
                ),
                Some(s) => format!("{s}migrate_url = \"{url}\"\nmigrate_password_file = {file:?}\n"),
            });
        }
        if !memory.contains(module) {
            writeln!(toml, "{}", section.unwrap()).unwrap();
        }
    }
    // Antares needs PostGIS and btree_gin, which only a superuser creates (unaccent: dataroom).
    let on_db = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.database(&db))
        .await
        .unwrap();
    // daiya and Antares keep their tables in schema public, where Postgres 15+ lets only the
    // owner create.
    sqlx::raw_sql(
        "CREATE EXTENSION IF NOT EXISTS postgis; CREATE EXTENSION IF NOT EXISTS btree_gin;
         CREATE EXTENSION IF NOT EXISTS unaccent;
         GRANT CREATE ON SCHEMA public TO daiya_migrate, antares_app;",
    )
    .execute(&on_db)
    .await
    .unwrap();
    let config = dir.join("jc.toml");
    std::fs::write(&config, toml).unwrap();
    config
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(config: &Path, state: &Path, port: u16) -> Jc {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") || key.starts_with("JC_") || key.starts_with("ANTARES_") {
            cmd.env_remove(key);
        }
    }
    let base_host = format!("localhost:{port}");
    cmd.args(["--role", "all", "--scheme", "http", "--log-format", "text"])
        .arg("--config")
        .arg(config)
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--base-host", &base_host])
        .arg("--state-dir")
        .arg(state)
        // One jc process serves the database.
        .env("ANTARES_ALLOW_SHARED_LOCAL", "1")
        .env("JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR", "false")
        .env("RUST_LOG", "info,sqlx=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn jc");
    let stderr = child.stderr.take().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = log.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("jc listening") {
                let _ = tx.send(Ok(()));
            }
            sink.lock().unwrap().push(line);
        }
        let _ = tx.send(Err(()));
    });
    let jc = Jc {
        child,
        base: format!("http://{base_host}"),
        state: state.to_path_buf(),
        log,
    };
    match rx.recv_timeout(Duration::from_secs(240)) {
        Ok(Ok(())) => jc,
        other => {
            let tail = jc.log.lock().unwrap().join("\n");
            panic!("jc did not start ({other:?}):\n{tail}");
        }
    }
}

async fn eventually<F, Fut, T>(what: &str, secs: u64, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(Instant::now() < deadline, "{what}: not within {secs} s");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn ok(what: &str, req: reqwest::RequestBuilder) -> Value {
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert!(status.is_success(), "{what}: {status} {body}");
    serde_json::from_str(&body).unwrap_or(Value::Null)
}

const ENTITY: &str = "urn:ngsi-ld:AirQualityObserved:jc-restart-1";
const USER: &str = "restart.person@jc.invalid";

async fn kaisatsu_token(jc: &Jc) -> String {
    let body = ok(
        "kaisatsu client_credentials",
        Client::new()
            .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", "portal-reconciler"),
                (
                    "client_secret",
                    jc.secret("portal-admin-client-secret").as_str(),
                ),
            ]),
    )
    .await;
    body["access_token"].as_str().unwrap().to_owned()
}

/// One write through each public mount.
async fn write_everything(jc: &Jc) {
    let http = Client::new();
    let shako = format!("token {}", jc.secret("shako-token"));
    ok(
        "shako repository",
        http.post(jc.url("/git/api/v1/orgs/jc/repos"))
            .header("authorization", &shako)
            .json(&json!({"name": "restart", "private": true, "auto_init": true, "default_branch": "main"})),
    )
    .await;
    ok(
        "shako commit",
        http.put(jc.url("/git/api/v1/repos/jc/restart/contents/kept.yaml"))
            .header("authorization", &shako)
            .json(&json!({"content": "a2VwdDogdHJ1ZQo=", "message": "Add kept.yaml", "branch": "main"})),
    )
    .await;

    let dashboards = jc.secret("dashboards-password");
    ok(
        "daiya dashboard",
        http.post(jc.url("/dashboards/api/dashboards/db"))
            .basic_auth("portal", Some(&dashboards))
            .json(&json!({
                "dashboard": {"uid": "restart-overview", "title": "Restart overview", "schemaVersion": 39, "panels": []},
                "overwrite": true,
                "message": "restart"
            })),
    )
    .await;

    let catalogue = jc.secret("catalogue-token");
    ok(
        "dataroom organization",
        http.post(jc.url("/opendata/api/3/action/organization_create"))
            .header("authorization", &catalogue)
            .json(&json!({"name": "restart-org", "title": "Restart"})),
    )
    .await;
    ok(
        "dataroom dataset",
        http.post(jc.url("/opendata/api/3/action/package_create"))
            .header("authorization", &catalogue)
            .json(&json!({
                "name": "restart-air-quality",
                "owner_org": "restart-org",
                "title": "Restart air quality",
                "notes": "Kept across a restart.",
                "private": false,
                "resources": [{"name": "NGSI-LD", "url": "https://example.org/api/endpoint/restart", "format": "NGSI-LD"}]
            })),
    )
    .await;

    ok(
        "ekiben stream",
        http.post(jc.url("/streams/streams/restart-stream"))
            .bearer_auth(jc.secret("ekiben-token"))
            .header("content-type", "application/yaml")
            .body("input:\n  generate:\n    interval: 1h\n    mapping: 'root = \"kept\"'\noutput:\n  drop: {}\n"),
    )
    .await;

    let token = kaisatsu_token(jc).await;
    let created = http
        .post(jc.url("/id/admin/realms/jc/users"))
        .bearer_auth(&token)
        .json(&json!({"username": USER, "email": USER, "enabled": true, "emailVerified": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED, "kaisatsu user");

    ok(
        "antares entity",
        http.post(jc.url("/broker/ngsi-ld/v1/entities"))
            .header("content-type", "application/json")
            .json(&json!({
                "id": ENTITY,
                "type": "AirQualityObserved",
                "pm10": {"type": "Property", "value": 17}
            })),
    )
    .await;
}

/// Reads every write back; a missing one names its module.
async fn read_everything(jc: &Jc) {
    let http = Client::new();
    let shako = format!("token {}", jc.secret("shako-token"));
    let file = ok(
        "shako commit after the restart",
        http.get(jc.url("/git/api/v1/repos/jc/restart/contents/kept.yaml?ref=main"))
            .header("authorization", &shako),
    )
    .await;
    assert_eq!(file["content"].as_str().map(str::trim), Some("a2VwdDogdHJ1ZQo="));
    let repo = ok(
        "shako repository after the restart",
        http.get(jc.url("/git/api/v1/repos/jc/restart"))
            .header("authorization", &shako),
    )
    .await;
    assert_eq!(repo["name"], "restart", "{repo}");

    let dash = ok(
        "daiya dashboard after the restart",
        http.get(jc.url("/dashboards/api/dashboards/uid/restart-overview"))
            .basic_auth("portal", Some(jc.secret("dashboards-password"))),
    )
    .await;
    assert_eq!(dash["dashboard"]["title"], "Restart overview");

    let dataset = ok(
        "dataroom dataset after the restart",
        http.get(jc.url("/opendata/api/3/action/package_show?id=restart-air-quality")),
    )
    .await;
    assert_eq!(dataset["result"]["title"], "Restart air quality");

    let token = jc.secret("ekiben-token");
    let stream = eventually("ekiben stream after the restart", 30, || {
        let req = http
            .get(jc.url("/streams/streams/restart-stream"))
            .bearer_auth(&token);
        async move {
            let resp = req.send().await.ok()?;
            resp.status().is_success().then_some(())?;
            resp.json::<Value>().await.ok()
        }
    })
    .await;
    assert_eq!(stream["active"], true, "{stream}");

    let token = kaisatsu_token(jc).await;
    let users = ok(
        "kaisatsu user after the restart",
        http.get(jc.url(&format!(
            "/id/admin/realms/jc/users?username={USER}&exact=true"
        )))
        .bearer_auth(&token),
    )
    .await;
    assert_eq!(users.as_array().map(Vec::len), Some(1), "{users}");

    let entity = ok(
        "antares entity after the restart",
        http.get(jc.url(&format!("/broker/ngsi-ld/v1/entities/{ENTITY}"))),
    )
    .await;
    assert_eq!(entity["pm10"]["value"], 17, "{entity}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_module_keeps_its_writes_across_a_restart_on_postgres() {
    let Ok(admin_url) = std::env::var("JC_PG_TEST_DATABASE_URL") else {
        println!("SKIP: JC_PG_TEST_DATABASE_URL is unset");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let config = prepare(dir.path(), &admin_url, &[]).await;
    let state = dir.path().join("state");
    let port = free_port();

    let first = start(&config, &state, port);
    write_everything(&first).await;
    let log = first.log.lock().unwrap().join("\n");
    for module in ["shako", "ekiben", "kaisatsu"] {
        assert!(
            !log.contains(&format!("{module}: database_url_file is not set")),
            "{module} fell back to memory"
        );
    }
    first.stop();

    let second = start(&config, &state, port);
    read_everything(&second).await;
    second.stop();
}

/// A module with a database section refuses to start when the database refuses it: a wrong
/// password stops jc, it never falls back to memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_module_whose_database_refuses_it_stops_jc() {
    let Ok(admin_url) = std::env::var("JC_PG_TEST_DATABASE_URL") else {
        println!("SKIP: JC_PG_TEST_DATABASE_URL is unset");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("jc.toml");
    let pw = dir.path().join("pw");
    std::fs::write(&pw, "wrong-password").unwrap();
    let base = PgConnectOptions::from_str(&admin_url).unwrap();
    std::fs::write(
        &config,
        format!(
            "[daiya.database]\nurl = \"postgres://daiya_app@{}:{}/postgres\"\npassword_file = {pw:?}\n",
            base.get_host(),
            base.get_port()
        ),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jc"))
        .args(["--role", "daiya", "--log-format", "text"])
        .arg("--config")
        .arg(&config)
        .args(["--listen", &format!("127.0.0.1:{}", free_port())])
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("daiya"), "{stderr}");
    assert!(!stderr.contains("wrong-password"), "{stderr}");
    assert!(!stderr.contains("jc listening"), "{stderr}");
}

/// Runs `cmd` to its exit; one still running after `limit` (it started serving) is killed and
/// answers `None`.
fn exit_within(mut cmd: Command, limit: Duration) -> (Option<i32>, String) {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        text
    });
    let deadline = Instant::now() + limit;
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    (code, reader.join().unwrap())
}

/// Each module's boot RLS check runs inside jc as it does standalone: a database section that
/// connects as a superuser (which bypasses every policy) stops jc before it listens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_superuser_request_role_is_refused_by_every_module() {
    let Ok(admin_url) = std::env::var("JC_PG_TEST_DATABASE_URL") else {
        println!("SKIP: JC_PG_TEST_DATABASE_URL is unset");
        return;
    };
    let base = PgConnectOptions::from_str(&admin_url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .unwrap();
    let db = format!("jc_rls_{:x}", std::process::id());
    for statement in [
        format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        format!("CREATE DATABASE {db}"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .unwrap();
    }
    let password = url::Url::parse(&admin_url)
        .unwrap()
        .password()
        .unwrap_or_default()
        .to_owned();
    let dir = tempfile::tempdir().unwrap();
    let pw = dir.path().join("pw");
    std::fs::write(&pw, &password).unwrap();
    let url = format!(
        "postgres://{}@{}:{}/{db}",
        base.get_username(),
        base.get_host(),
        base.get_port()
    );
    for module in ["kaisatsu", "shako", "daiya", "dataroom", "ekiben", "antares"] {
        let config = dir.path().join(format!("{module}.toml"));
        std::fs::write(
            &config,
            format!("[{module}.database]\nurl = \"{url}\"\npassword_file = {pw:?}\n"),
        )
        .unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
        for (key, _) in std::env::vars() {
            if key.starts_with("JCBIN_") || key.starts_with("JC_") || key.starts_with("ANTARES_") {
                cmd.env_remove(key);
            }
        }
        cmd.args(["--role", module, "--log-format", "text"])
            .arg("--config")
            .arg(&config)
            .args(["--listen", &format!("127.0.0.1:{}", free_port())])
            .arg("--state-dir")
            .arg(dir.path().join(format!("state-{module}")))
            .env("ANTARES_ALLOW_SHARED_LOCAL", "1");
        let (code, stderr) = exit_within(cmd, Duration::from_secs(90));
        assert_eq!(code, Some(1), "{module}: {stderr}");
        assert!(!stderr.contains("jc listening"), "{module}: {stderr}");
        let lower = stderr.to_lowercase();
        assert!(
            lower.contains("superuser") || lower.contains("bypass"),
            "{module} names the refused role attribute: {stderr}"
        );
        assert!(
            password.is_empty() || !stderr.contains(&password),
            "{module} printed the password"
        );
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .unwrap();
}
