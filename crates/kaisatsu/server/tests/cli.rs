#![forbid(unsafe_code)]
//! The `kaisatsu` binary (W1b, W2, W9): `check`, `plan`, `apply`, `diff` and `export` on realm
//! files in JSON and YAML, `migrate` and `serve` on PostgreSQL with the password read from a file.
//! The PostgreSQL cases print `SKIP` without `KAISATSU_TEST_DATABASE_URL`.

use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::str::FromStr;

const BIN: &str = env!("CARGO_BIN_EXE_kaisatsu");

const REALM_YAML: &str = r#"
realm: cli
displayName: CLI realm
clients:
  - clientId: cli-app
    publicClient: false
    directAccessGrantsEnabled: true
    standardFlowEnabled: false
    secret: $(env:KAISATSU_CLI_TEST_CLIENT_SECRET)
users:
  - username: alice
    email: alice@cli.example
    enabled: true
    credentials:
      - type: password
        value: $(env:KAISATSU_CLI_TEST_ALICE_PASSWORD)
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("kaisatsu-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn file(&self, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, content).expect("write temp file");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kaisatsu(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("KAISATSU_CLI_TEST_CLIENT_SECRET", "cli-secret-123")
        .env("KAISATSU_CLI_TEST_ALICE_PASSWORD", "AlicePass123!Valid")
        .output()
        .expect("run kaisatsu")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn p(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

#[test]
fn check_reads_json_and_yaml_and_refuses_bad_files() {
    let dir = TempDir::new();
    let yaml = dir.file("realm.yaml", REALM_YAML);
    let out = kaisatsu(&["check", "-f", p(&yaml)]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let json = dir.file(
        "realm.json",
        r#"{"realm":"cli","clients":[{"clientId":"x","publicClient":false,"secret":"$(X)"}]}"#,
    );
    assert_eq!(code(&kaisatsu(&["check", "-f", p(&json)])), 0);

    // A literal secret is refused unless allowed.
    let literal = dir.file(
        "literal.yml",
        "realm: cli\nclients:\n  - clientId: x\n    publicClient: false\n    secret: plain-text-secret\n",
    );
    let out = kaisatsu(&["check", "-f", p(&literal)]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("literal secret"), "{}", stderr(&out));
    assert!(!stderr(&out).contains("plain-text-secret"));
    assert_eq!(
        code(&kaisatsu(&[
            "check",
            "-f",
            p(&literal),
            "--allow-literal-secrets"
        ])),
        0
    );

    // Broken YAML names the file; a YAML list is not a realm; a missing path fails.
    let broken = dir.file("broken.yaml", "realm: [unclosed\n");
    let out = kaisatsu(&["check", "-f", p(&broken)]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("broken.yaml"), "{}", stderr(&out));
    assert!(stderr(&out).contains("invalid YAML"), "{}", stderr(&out));
    let list = dir.file("list.yaml", "- realm: cli\n");
    assert_eq!(code(&kaisatsu(&["check", "-f", p(&list)])), 1);
    let bad_json = dir.file("bad.json", "{");
    let out = kaisatsu(&["check", "-f", p(&bad_json)]);
    assert!(stderr(&out).contains("invalid JSON"), "{}", stderr(&out));
    let out = kaisatsu(&["check", "-f", p(&dir.0.join("missing.yaml"))]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("no such realm file"),
        "{}",
        stderr(&out)
    );
    let no_realm = dir.file("norealm.json", "{}");
    assert_eq!(code(&kaisatsu(&["check", "-f", p(&no_realm)])), 1);
}

#[test]
fn a_directory_merges_json_and_yaml_in_file_name_order() {
    let dir = TempDir::new();
    let realm = TempDir::new();
    realm.file("10-base.yaml", REALM_YAML);
    realm.file(
        "20-more.json",
        r#"{"realm":"cli","roles":{"realm":[{"name":"cli-reader"}]}}"#,
    );
    realm.file("README.txt", "ignored");
    let plan_out = dir.0.join("plan.json");
    let out = kaisatsu(&[
        "plan",
        "-f",
        p(&realm.0),
        "-o",
        p(&plan_out),
        "--output",
        "markdown",
    ]);
    assert_eq!(code(&out), 2, "changes pending: {}", stderr(&out));
    let plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&plan_out).expect("plan written")).expect("json");
    let kinds: Vec<&str> = plan["report"]["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .filter_map(|c| c["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"Client"), "{kinds:?}");
    assert!(kinds.contains(&"User"), "{kinds:?}");
    assert!(kinds.contains(&"Role"), "{kinds:?}");

    // On the in-memory store every run starts empty: apply reports changes, a dry run too.
    let out = kaisatsu(&["apply", "-f", p(&realm.0)]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json report");
    assert_eq!(report["realm"], "cli");
    let out = kaisatsu(&[
        "apply",
        "-f",
        p(&realm.0),
        "--dry-run",
        "--output",
        "markdown",
    ]);
    assert_eq!(code(&out), 2);
    assert_eq!(code(&kaisatsu(&["diff", "-f", p(&realm.0)])), 2);
    let out = kaisatsu(&["export", "--realm", "cli"]);
    assert_eq!(code(&out), 1, "nothing to export from an empty store");
    // `--database-url` needs the KEK that encrypts the stored secrets.
    let out = kaisatsu(&[
        "export",
        "--realm",
        "cli",
        "--database-url",
        "postgresql://kaisatsu_app@127.0.0.1:1/x",
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--kek-file"), "{}", stderr(&out));
    // A KEK file that is not 32 bytes is refused.
    let short_kek = dir.file("kek", "short");
    let out = kaisatsu(&["export", "--realm", "cli", "--kek-file", p(&short_kek)]);
    assert_eq!(code(&out), 1);
}

#[test]
fn serve_refuses_bad_flags_before_listening() {
    let dir = TempDir::new();
    let kek = dir.file("kek", [7u8; 32]);
    let missing_kek = dir.0.join("nope");
    assert_eq!(
        code(&kaisatsu(&["serve", "--kek-file", p(&missing_kek)])),
        1
    );
    let out = kaisatsu(&[
        "serve",
        "--kek-file",
        p(&kek),
        "--trusted-proxy",
        "not-a-cidr",
    ]);
    assert_eq!(code(&out), 1);
    let out = kaisatsu(&["serve", "--kek-file", p(&kek), "--listen", "256.0.0.1:1"]);
    assert_eq!(code(&out), 1);
    let out = kaisatsu(&[
        "serve",
        "--kek-file",
        p(&kek),
        "--import-dir",
        p(&dir.0.join("missing")),
    ]);
    assert_eq!(code(&out), 1);
    // A password in the database URL is refused (W2: the password comes from a file).
    let out = kaisatsu(&[
        "serve",
        "--kek-file",
        p(&kek),
        "--database-url",
        "postgresql://kaisatsu_app:hunter2@127.0.0.1:1/kaisatsu",
    ]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("must not carry a password"));
    assert!(!stderr(&out).contains("hunter2"));
    let out = kaisatsu(&[
        "migrate",
        "--database-url",
        "postgresql://kaisatsu_migrate:hunter2@127.0.0.1:1/kaisatsu",
    ]);
    assert_eq!(code(&out), 1);
    assert!(!stderr(&out).contains("hunter2"));
}

// ---------------------------------------------------------------- PostgreSQL

struct PgEnv {
    admin: PgPool,
    base: PgConnectOptions,
    name: String,
    app_password: String,
    migrate_password: String,
}

fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

async fn pg_env(test: &str) -> Option<PgEnv> {
    let Ok(url) = std::env::var("KAISATSU_TEST_DATABASE_URL") else {
        println!("SKIP {test}: KAISATSU_TEST_DATABASE_URL is unset; live PostgreSQL test not run");
        return None;
    };
    let base = PgConnectOptions::from_str(&url).expect("KAISATSU_TEST_DATABASE_URL parses");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .expect("connect test server");
    let app_password = random_secret();
    let migrate_password = random_secret();
    for (role, pw) in [
        ("kaisatsu_app", &app_password),
        ("kaisatsu_migrate", &migrate_password),
    ] {
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
        )))
        .execute(&admin)
        .await
        .expect("test roles");
    }
    let name = format!("kaisatsu_t_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE DATABASE {name} OWNER kaisatsu_migrate"
    )))
    .execute(&admin)
    .await
    .expect("create test database");
    Some(PgEnv {
        admin,
        base,
        name,
        app_password,
        migrate_password,
    })
}

impl PgEnv {
    fn url(&self, role: &str) -> String {
        format!(
            "postgresql://{role}@{}:{}/{}",
            self.base.get_host(),
            self.base.get_port(),
            self.name
        )
    }

    async fn drop_db(self) {
        sqlx::raw_sql(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("drop test database");
    }
}

/// One HTTP/1.1 request on a fresh connection; returns status and body.
fn http(addr: &str, method: &str, path: &str, form: Option<&str>) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(addr).expect("connect to kaisatsu");
    let body = form.unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: idm.cli.example.test\r\nX-Forwarded-Proto: https\r\n\
         Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("send request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let body = raw.split_once("\r\n\r\n").map(|x| x.1).unwrap_or_default();
    (status, body.to_owned())
}

#[tokio::test]
async fn migrate_apply_and_serve_on_postgres_with_password_files() {
    let Some(env) = pg_env("migrate_apply_and_serve_on_postgres").await else {
        return;
    };
    let dir = TempDir::new();
    let kek = dir.file("kek", [9u8; 32]);
    let app_pw = dir.file("app-password", format!("{}\n", env.app_password));
    let mig_pw = dir.file("migrate-password", &env.migrate_password);
    let realm = dir.file("realm.yaml", REALM_YAML);
    let app_url = env.url("kaisatsu_app");
    let db = ["--database-url", &app_url, "--db-password-file", p(&app_pw)];

    // Before migrating, serve refuses: the schema has no tables (boot check).
    let out = kaisatsu(&[&["serve", "--kek-file", p(&kek)][..], &db[..]].concat());
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("kaisatsu migrate"),
        "{}",
        stderr(&out)
    );

    let mig_url = env.url("kaisatsu_migrate");
    let out = kaisatsu(&[
        "migrate",
        "--database-url",
        &mig_url,
        "--db-password-file",
        p(&mig_pw),
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = kaisatsu(&[
        "migrate",
        "--database-url",
        &mig_url,
        "--db-password-file",
        p(&mig_pw),
    ]);
    assert_eq!(code(&out), 0, "migrate runs once and is then a no-op");
    // The wrong password file fails without echoing anything secret.
    let wrong = dir.file("wrong", "not-the-password");
    let out = kaisatsu(&[
        "migrate",
        "--database-url",
        &mig_url,
        "--db-password-file",
        p(&wrong),
    ]);
    assert_eq!(code(&out), 1);
    assert!(!stderr(&out).contains(&env.migrate_password));

    // The migration role owns the tables, so it may not serve.
    let as_owner = kaisatsu(&[
        "serve",
        "--kek-file",
        p(&kek),
        "--database-url",
        &mig_url,
        "--db-password-file",
        p(&mig_pw),
    ]);
    assert_eq!(code(&as_owner), 1, "{}", stderr(&as_owner));

    // apply writes to PostgreSQL; the second run finds nothing to change (state persisted).
    let apply = [
        &["apply", "-f", p(&realm), "--kek-file", p(&kek)][..],
        &db[..],
    ]
    .concat();
    let out = kaisatsu(&apply);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let out = kaisatsu(&apply);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let diff = [
        &["diff", "-f", p(&realm), "--kek-file", p(&kek)][..],
        &db[..],
    ]
    .concat();
    assert_eq!(code(&kaisatsu(&diff)), 0);
    let out = kaisatsu(
        &[
            &["export", "--realm", "cli", "--kek-file", p(&kek)][..],
            &db[..],
        ]
        .concat(),
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let exported: serde_json::Value = serde_json::from_slice(&out.stdout).expect("export json");
    assert_eq!(exported["realm"], "cli");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("cli-secret-123"),
        "exports never carry secrets"
    );
    // A saved plan applies; after a change in between it is stale.
    let plan_out = dir.0.join("plan.json");
    let plan_cmd = [
        &[
            "plan",
            "-f",
            p(&realm),
            "-o",
            p(&plan_out),
            "--kek-file",
            p(&kek),
        ][..],
        &db[..],
    ]
    .concat();
    assert_eq!(code(&kaisatsu(&plan_cmd)), 0, "nothing pending");
    let changed = dir.file(
        "changed.yaml",
        REALM_YAML.replace("CLI realm", "CLI realm 2"),
    );
    let plan_changed = [
        &[
            "plan",
            "-f",
            p(&changed),
            "-o",
            p(&plan_out),
            "--kek-file",
            p(&kek),
        ][..],
        &db[..],
    ]
    .concat();
    assert_eq!(code(&kaisatsu(&plan_changed)), 2);
    let apply_plan = [
        &["apply", "-f", p(&plan_out), "--kek-file", p(&kek)][..],
        &db[..],
    ]
    .concat();
    let out = kaisatsu(&apply_plan);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    let out = kaisatsu(&apply_plan);
    assert_eq!(
        code(&out),
        3,
        "the plan's base state is gone: {}",
        stderr(&out)
    );

    // A KEK other than the one that encrypted the secrets cannot read them.
    let other_kek = dir.file("other-kek", [1u8; 32]);
    let out = kaisatsu(
        &[
            &["export", "--realm", "cli", "--kek-file", p(&other_kek)][..],
            &db[..],
        ]
        .concat(),
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));

    // serve on PostgreSQL: discovery, a password grant and the persisted client secret.
    let import = TempDir::new();
    import.file("realm.yaml", REALM_YAML);
    let mut child = Command::new(BIN)
        .args(
            [
                &[
                    "serve",
                    "--kek-file",
                    p(&kek),
                    "--listen",
                    "127.0.0.1:0",
                    "--host",
                    "idm.cli.example.test",
                    "--import-dir",
                    p(&import.0),
                    "--sweep-interval-secs",
                    "1",
                ][..],
                &db[..],
            ]
            .concat(),
        )
        .env("KAISATSU_CLI_TEST_CLIENT_SECRET", "cli-secret-123")
        .env("KAISATSU_CLI_TEST_ALICE_PASSWORD", "AlicePass123!Valid")
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn serve");
    let mut lines = BufReader::new(child.stderr.take().expect("stderr")).lines();
    let addr = loop {
        let line = lines
            .next()
            .expect("serve exited before listening")
            .expect("stderr line");
        if let Some(a) = line.strip_prefix("kaisatsu: listening on ") {
            break a.to_owned();
        }
    };
    let (st, body) = http(
        &addr,
        "GET",
        "/realms/cli/.well-known/openid-configuration",
        None,
    );
    assert_eq!(st, 200, "{body}");
    let disc: serde_json::Value = serde_json::from_str(&body).expect("discovery json");
    assert_eq!(disc["issuer"], "https://idm.cli.example.test/realms/cli");
    let grant = "grant_type=password&client_id=cli-app&client_secret=cli-secret-123\
                 &username=alice&password=AlicePass123%21Valid&scope=openid";
    let (st, body) = http(
        &addr,
        "POST",
        "/realms/cli/protocol/openid-connect/token",
        Some(grant),
    );
    assert_eq!(st, 200, "{body}");
    let tokens: serde_json::Value = serde_json::from_str(&body).expect("token json");
    assert!(tokens["access_token"].as_str().is_some());
    let (st, _) = http(
        &addr,
        "POST",
        "/realms/cli/protocol/openid-connect/token",
        Some(&grant.replace("cli-secret-123", "wrong")),
    );
    assert_eq!(st, 401);
    let (st, body) = http(
        &addr,
        "GET",
        "/realms/cli/protocol/openid-connect/certs",
        None,
    );
    assert_eq!(st, 200, "{body}");
    child.kill().expect("stop serve");
    let _ = child.wait();

    // The client secret is stored encrypted, never in clear text.
    let su = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(env.base.clone().database(&env.name))
        .await
        .expect("superuser on test db");
    let clear: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM kaisatsu.applications WHERE doc::text LIKE '%cli-secret-123%'",
    )
    .fetch_one(&su)
    .await
    .expect("query");
    assert_eq!(clear, 0);
    let refresh_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM kaisatsu.refresh_token_family")
            .fetch_one(&su)
            .await
            .expect("query");
    assert_eq!(refresh_rows, 1, "the session lives in PostgreSQL");
    su.close().await;
    env.drop_db().await;
}
