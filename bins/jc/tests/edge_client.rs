#![allow(clippy::disallowed_methods)] // These tests start the jc binary and read their database URL.
//! The edge check at start (T-0252): a realm that holds the `edge` client (APISIX's
//! `openid-connect`) must pin RS256 for its ID and access tokens, or `jc` exits before it listens
//! and names the client; with both attributes it starts. On PostgreSQL, a realm whose RS256 key
//! was retired by hand gets a fresh ACTIVE one at the next start.
//!
//! The PostgreSQL case runs when `JC_PG_TEST_DATABASE_URL` (or `KAISATSU_TEST_DATABASE_URL`)
//! names a superuser connection; it prints `SKIP` otherwise.

use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::time::{Duration, Instant};

const ID_ALG: &str = "id.token.signed.response.alg";
const ACCESS_ALG: &str = "access.token.signed.response.alg";

/// Writes realm `dev` with an `edge` client carrying `attributes` into `dir`.
fn realm_file(dir: &Path, attributes: Value) {
    std::fs::create_dir_all(dir).unwrap();
    let realm = json!({
        "realm": "dev",
        "enabled": true,
        "clients": [{
            "clientId": "edge",
            "enabled": true,
            "publicClient": false,
            "standardFlowEnabled": true,
            "redirectUris": ["https://portal.city.test/*"],
            "attributes": attributes
        }]
    });
    std::fs::write(dir.join("dev.json"), realm.to_string()).unwrap();
}

fn jc(state: &Path, import: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    cmd.args(["--profile", "development"]);
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") {
            cmd.env_remove(key);
        }
    }
    cmd.args([
        "--role",
        "kaisatsu",
        "--scheme",
        "http",
        "--log-format",
        "text",
    ])
    .args(["--listen", "127.0.0.1:0", "--base-host", "city.test"])
    .args(["--identity-realm", "dev"])
    .arg("--state-dir")
    .arg(state)
    .arg("--kaisatsu-import-dir")
    .arg(import)
    .env("RUST_LOG", "info,sqlx=warn");
    cmd
}

/// Runs `cmd` until it logs "jc listening" (`Ok` with the running child) or exits (`Err` with
/// its exit code and log).
fn run(mut cmd: Command) -> Result<Child, (Option<i32>, String)> {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn jc");
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut log = String::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("jc listening") {
                let _ = tx.send(Ok(()));
            }
            log.push_str(&line);
            log.push('\n');
        }
        let _ = tx.send(Err(log));
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(())) => Ok(child),
        Ok(Err(log)) => Err((child.wait().unwrap().code(), log)),
        Err(_) => {
            let _ = child.kill();
            panic!("jc neither listened nor exited within 120 s");
        }
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn an_edge_client_without_rs256_stops_jc_and_with_it_jc_starts() {
    let dir = tempfile::tempdir().unwrap();
    let import = dir.path().join("import");
    realm_file(&import, json!({}));
    let (code, log) = run(jc(&dir.path().join("state"), &import)).expect_err("refused");
    assert_eq!(code, Some(1), "{log}");
    assert!(log.contains("client edge"), "{log}");
    assert!(log.contains(ID_ALG), "{log}");
    assert!(!log.contains("jc listening"), "{log}");

    realm_file(&import, json!({ ID_ALG: "RS256", ACCESS_ALG: "ES256" }));
    let (code, log) = run(jc(&dir.path().join("state"), &import)).expect_err("refused");
    assert_eq!(code, Some(1), "{log}");
    assert!(log.contains(ACCESS_ALG), "{log}");

    realm_file(&import, json!({ ID_ALG: "RS256", ACCESS_ALG: "RS256" }));
    let child = run(jc(&dir.path().join("state"), &import)).expect("jc starts");
    stop(child);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retired_rs256_key_is_replaced_at_start_on_postgres() {
    let Ok(admin_url) = std::env::var("JC_PG_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("KAISATSU_TEST_DATABASE_URL"))
    else {
        println!("SKIP: JC_PG_TEST_DATABASE_URL is unset");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let base = PgConnectOptions::from_str(&admin_url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .expect("database unreachable");
    let db = format!("jc_edge_{:x}", std::process::id());
    for statement in [
        format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        format!("CREATE DATABASE {db}"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .unwrap();
    }
    // The migrations grant to the cluster-wide `kaisatsu_app`, as tests/restart_pg.rs does
    // (which resets the same passwords: do not run the two binaries at once).
    let mut toml = String::from("[kaisatsu.database]\nmax_connections = 3\n");
    for (role, key) in [("kaisatsu_app", ""), ("kaisatsu_migrate", "migrate_")] {
        let mut pw = [0u8; 16];
        getrandom::fill(&mut pw).unwrap();
        let pw = hex::encode(pw);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';
             GRANT CREATE, CONNECT ON DATABASE {db} TO {role};"
        )))
        .execute(&admin)
        .await
        .unwrap();
        let file = dir.path().join(format!("{role}-password"));
        std::fs::write(&file, &pw).unwrap();
        toml.push_str(&format!(
            "{key}url = \"postgres://{role}@{}:{}/{db}\"\n{key}password_file = {file:?}\n",
            base.get_host(),
            base.get_port()
        ));
    }
    let config = dir.path().join("jc.toml");
    std::fs::write(&config, toml).unwrap();
    let import = dir.path().join("import");
    realm_file(&import, json!({ ID_ALG: "RS256", ACCESS_ALG: "RS256" }));
    let state = dir.path().join("state");
    let start = || {
        let mut cmd = jc(&state, &import);
        cmd.arg("--config").arg(&config);
        run(cmd).expect("jc starts")
    };
    stop(start());

    let on_db = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.database(&db))
        .await
        .unwrap();
    let active_rs256 = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT kid FROM kaisatsu.signing_keys
             WHERE doc->>'algorithm' = 'RS256' AND doc->>'status' = 'ACTIVE'",
        )
        .fetch_all(&on_db)
        .await
        .unwrap()
    };
    let before = active_rs256().await;
    assert_eq!(before.len(), 1, "{before:?}");
    sqlx::query(
        "UPDATE kaisatsu.signing_keys SET doc = jsonb_set(doc, '{status}', '\"RETIRED\"')
         WHERE doc->>'algorithm' = 'RS256'",
    )
    .execute(&on_db)
    .await
    .unwrap();
    assert!(active_rs256().await.is_empty());

    stop(start());
    let after = active_rs256().await;
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after, before);

    on_db.close().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .unwrap();
}
