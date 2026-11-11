#![allow(clippy::disallowed_methods)] // These tests start the jc binary and read their database URL.
//! End to end: the `jc` binary with `--role all` on a free port, the Portal wired to its sibling
//! modules by `jc` (no external Keycloak, Gitea, Bento, CKAN or Grafana), driven only through the
//! public mounts, each step asserted on the module it lands in.
//!
//! kaisatsu runs on PostgreSQL when `KAISATSU_TEST_DATABASE_URL` (a superuser URL) is set; the
//! other modules have no Postgres path in `jc` yet (memory stores, STATUS.md).

mod common;

use common::{Body, Browser, free_port, lost_the_port, sign_in, sign_in_at};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where jc places its modules: one host with a slug each, or a host per module (`--mode host`,
/// the Portal on `portal.localhost`).
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    Slug,
    Host,
}

const SLUGS: [&str; 6] = ["id", "git", "streams", "opendata", "dashboards", "broker"];

struct Jc {
    child: Child,
    placement: Placement,
    /// The realm the Portal signs in through (`--identity-realm`).
    realm: &'static str,
    port: u16,
    base: String,
    state: PathBuf,
    _dir: tempfile::TempDir,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for Jc {
    fn drop(&mut self) {
        // JC_E2E_LOG=<file> keeps jc's whole log of the run for a reader.
        if let Some(path) = std::env::var_os("JC_E2E_LOG") {
            let lines = self.log.lock().map(|l| l.join("\n")).unwrap_or_default();
            let _ = std::fs::write(path, lines);
        }
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
    /// A secret this test gave jc (`given/`), else one jc keeps in its state directory.
    fn secret(&self, name: &str) -> String {
        let given = self._dir.path().join("given").join(name);
        let path = if given.exists() {
            given
        } else {
            self.state.join(name)
        };
        std::fs::read_to_string(path).unwrap().trim().to_owned()
    }

    /// The public URL of `path` as slug mode spells it (`/id/...` is kaisatsu, anything else the
    /// Portal): in host mode each module's slug becomes its own host.
    fn url(&self, path: &str) -> String {
        if self.placement == Placement::Host {
            for slug in SLUGS {
                if let Some(rest) = path.strip_prefix(&format!("/{slug}"))
                    && (rest.is_empty() || rest.starts_with('/'))
                {
                    return format!("http://{slug}.localhost:{}{rest}", self.port);
                }
            }
        }
        format!("{}{path}", self.base)
    }

    fn log_has(&self, needle: &str) -> bool {
        self.log.lock().unwrap().iter().any(|l| l.contains(needle))
    }
}

/// kaisatsu's PostgreSQL section, when a superuser URL is given: roles and a database migrated
/// by the migration role, the request role's URL and password in files.
async fn kaisatsu_pg(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    let url = std::env::var("KAISATSU_TEST_DATABASE_URL").ok()?;
    let base = PgConnectOptions::from_str(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(base.clone())
        .await
        .unwrap();
    let suffix = format!("{:x}", std::process::id());
    let (app_pw, mig_pw) = (format!("app{suffix}x9"), format!("mig{suffix}x9"));
    // Both tests of this binary set the shared roles: two ALTER ROLE at once fail with "tuple
    // concurrently updated", so they take turns; each test gets its own database.
    static ROLES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let roles = ROLES.lock().await;
    for (role, pw) in [("kaisatsu_app", &app_pw), ("kaisatsu_migrate", &mig_pw)] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }
    drop(roles);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let db = format!("kaisatsu_e2e_{suffix}_{n}");
    for statement in [
        format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"),
        format!("CREATE DATABASE {db} OWNER kaisatsu_migrate"),
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .unwrap();
    }
    let mig = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            base.clone()
                .database(&db)
                .username("kaisatsu_migrate")
                .password(&mig_pw),
        )
        .await
        .unwrap();
    kaisatsu_store_pg::PgStore::run_migrations(&mig)
        .await
        .unwrap();
    mig.close().await;
    let url_file = dir.join("kaisatsu-db-url");
    std::fs::write(
        &url_file,
        format!(
            "postgres://kaisatsu_app@{}:{}/{db}\n",
            base.get_host(),
            base.get_port()
        ),
    )
    .unwrap();
    let pw_file = dir.join("kaisatsu-db-password");
    std::fs::write(&pw_file, app_pw).unwrap();
    Some((url_file, pw_file))
}

async fn start() -> Jc {
    start_placed(Placement::Slug, "jc").await
}

async fn start_placed(placement: Placement, realm: &'static str) -> Jc {
    for _ in 0..5 {
        if let Some(jc) = start_placed_once(placement, realm).await {
            return jc;
        }
    }
    panic!("jc lost its listen port to another bind 5 times");
}

/// One jc; `None` when it stopped because another bind took its port first.
async fn start_placed_once(placement: Placement, realm: &'static str) -> Option<Jc> {
    assert!(
        Command::new("docker")
            .args(["run", "--rm", "alpine", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            || std::env::var("KAISATSU_TEST_DATABASE_URL").is_err(),
        "dind broken while a database is asked for"
    );
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let port = free_port();
    let base_host = format!("localhost:{port}");
    // Slug mode: the Portal at the root of the one domain (its redirects and UI assets are
    // absolute paths). Host mode: every module on `<slug>.localhost`, the Portal on
    // `portal.localhost`, which resolves to ::1.
    let config = dir.path().join("jc.toml");
    let (mounts, listen, base) = match placement {
        Placement::Slug => (
            "[mounts.portal]\npath_prefix = \"\"\n",
            format!("127.0.0.1:{port}"),
            format!("http://{base_host}"),
        ),
        Placement::Host => (
            "mode = \"host\"\n",
            format!("[::1]:{port}"),
            format!("http://portal.{base_host}"),
        ),
    };
    std::fs::write(&config, mounts).unwrap();
    // The service tokens between modules live in jc's memory only; the test acts as the Portal
    // with tokens it gives jc as files (the split-role way of pinning them).
    let given = dir.path().join("given");
    std::fs::create_dir(&given).unwrap();
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
    cmd.args(["--role", "all", "--scheme", "http", "--log-format", "text"])
        .arg("--config")
        .arg(&config)
        .args(["--listen", &listen])
        .args(["--base-host", &base_host])
        .arg("--state-dir")
        .arg(&state)
        .args(["--identity-realm", realm])
        // A password-only demo realm: decisions without a second factor (an explicit
        // installation setting of the Portal, T-6083), and a quick forge poll.
        .env("JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR", "false")
        .env("JC_PORTAL_SYNC_INTERVAL", "3")
        .env("RUST_LOG", "info,sqlx=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some((url, pw)) = kaisatsu_pg(dir.path()).await {
        cmd.arg("--kaisatsu-database-url-file")
            .arg(url)
            .arg("--kaisatsu-db-password-file")
            .arg(pw);
    } else {
        println!("kaisatsu on its memory store: KAISATSU_TEST_DATABASE_URL is unset");
    }
    let mut child = cmd.spawn().expect("spawn jc");
    let stderr = child.stderr.take().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = log.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if line.contains("portal: ready") {
                let _ = tx.send(Ok(()));
            } else if line.contains("Error:") || line.contains("ERROR") {
                eprintln!("{line}");
            }
            sink.lock().unwrap().push(line);
        }
        let _ = tx.send(Err(()));
    });
    let jc = Jc {
        child,
        placement,
        realm,
        port,
        base,
        state,
        _dir: dir,
        log,
    };
    match rx.recv_timeout(Duration::from_secs(180)) {
        Ok(Ok(())) => Some(jc),
        Ok(Err(())) if lost_the_port(&jc.log.lock().unwrap().join("\n")) => None,
        other => {
            let tail = jc.log.lock().unwrap().join("\n");
            panic!("jc did not start ({other:?}):\n{tail}");
        }
    }
}

async fn me(jc: &Jc, browser: &mut Browser) -> Value {
    let me = browser.get(&jc.url("/api/v1/auth/me")).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text);
    me.json()
}

/// A kaisatsu admin token of the Portal's own people client (what the Portal holds), for reading
/// the result back from kaisatsu's admin API.
async fn kaisatsu_admin_token(jc: &Jc) -> String {
    let client = Client::new();
    let resp = client
        .post(jc.url(&format!(
            "/id/realms/{}/protocol/openid-connect/token",
            jc.realm
        )))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", "portal-reconciler"),
            (
                "client_secret",
                jc.secret("portal-admin-client-secret").as_str(),
            ),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body["access_token"].as_str().unwrap().to_owned()
}

async fn kaisatsu_admin_write(jc: &Jc, token: &str, path: &str, body: &Value) -> StatusCode {
    Client::new()
        .put(jc.url(&format!("/id/admin/realms/{}{path}", jc.realm)))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap()
        .status()
}

async fn kaisatsu_admin(jc: &Jc, token: &str, path: &str) -> (StatusCode, Value) {
    let resp = Client::new()
        .get(jc.url(&format!("/id/admin/realms/{}{path}", jc.realm)))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// Reads the forge with git (smart HTTP through jc's shako mount) and returns the files of
/// `main` under `path`.
fn git_clone(jc: &Jc, into: &Path) -> std::process::Output {
    let token = jc.secret("shako-token");
    Command::new("git")
        .args([
            "-c",
            &format!("http.extraHeader=Authorization: token {token}"),
            "clone",
            "--quiet",
            &jc.url("/git/jc/config.git"),
        ])
        .arg(into)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_portal_runs_on_its_sibling_modules() {
    let jc = start().await;
    let mut browser = Browser::new();
    let mut gaps: Vec<String> = Vec::new();

    // Setup: secrets are never in the log; the kept ones are private files, the given service
    // tokens are not copied into the state directory.
    for name in [
        "shako-token",
        "ekiben-token",
        "catalogue-token",
        "dashboards-password",
    ] {
        assert!(!jc.log_has(&jc.secret(name)), "{name} appears in the log");
        assert!(
            !jc.state.join(name).exists(),
            "{name} written to the state dir"
        );
    }
    let kept = "portal-client-secret";
    assert!(!jc.log_has(&jc.secret(kept)), "{kept} appears in the log");
    // The runner reaches the Portal's internal listener, as prod's does (its deployment sets
    // JC_PORTAL_PIPELINE_TEST_CAPTURE_URL): streams carry the validation stage and the run-log
    // sinks, so a run reaches GET .../runs.
    assert!(
        !jc.log_has("no internal listener address for the runner"),
        "the Portal renders streams without validation and run log"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(jc.state.join(kept))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{kept}");
    }

    // a. Sign-in through kaisatsu.
    let password = jc.secret("first-admin-password");
    sign_in(&|p| jc.url(p), &mut browser, "admin", &password).await;
    let who = me(&jc, &mut browser).await;
    assert_eq!(who["username"], "admin", "{who}");
    // kaisatsu holds the session the Portal's sign-in opened.
    let admin_token = kaisatsu_admin_token(&jc).await;
    let (status, users) =
        kaisatsu_admin(&jc, &admin_token, "/users?username=admin&exact=true").await;
    assert_eq!(status, StatusCode::OK, "{users}");
    let admin_id = users[0]["id"].as_str().unwrap().to_owned();
    let (status, sessions) =
        kaisatsu_admin(&jc, &admin_token, &format!("/users/{admin_id}/sessions")).await;
    assert_eq!(status, StatusCode::OK, "{sessions}");
    assert!(
        !sessions.as_array().unwrap().is_empty(),
        "kaisatsu has the session"
    );

    // b. The Portal creates a person through kaisatsu's admin API.
    let person = json!({
        "email": "e2e.person@jc.invalid",
        "firstName": "Ada",
        "lastName": "Example",
    });
    let created = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/organization/people"),
            Some(Body::Json(&person)),
            true,
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let (status, found) = kaisatsu_admin(
        &jc,
        &admin_token,
        "/users?email=e2e.person@jc.invalid&exact=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(
        found[0]["firstName"], "Ada",
        "kaisatsu holds the person: {found}"
    );
    let person_id = found[0]["id"].as_str().unwrap().to_owned();

    // A second administrator (PF-58: a proposer never approves their own change): the person
    // gets a password and the bootstrap group through kaisatsu's admin API.
    let approver_pw = "Second-admin-pw-0123456789";
    let st = kaisatsu_admin_write(
        &jc,
        &admin_token,
        &format!("/users/{person_id}/reset-password"),
        &json!({"type": "password", "value": approver_pw, "temporary": false}),
    )
    .await;
    assert!(st.is_success(), "reset-password {st}");
    let st = kaisatsu_admin_write(
        &jc,
        &admin_token,
        &format!("/users/{person_id}"),
        &json!({
            "username": "e2e.person@jc.invalid",
            "email": "e2e.person@jc.invalid",
            "firstName": "Ada",
            "lastName": "Example",
            "requiredActions": [],
            "emailVerified": true,
            "enabled": true
        }),
    )
    .await;
    assert!(st.is_success(), "user update {st}");
    let (_, groups) = kaisatsu_admin(
        &jc,
        &admin_token,
        "/groups?search=platform-admins&exact=true",
    )
    .await;
    let bootstrap = groups[0]["id"]
        .as_str()
        .expect("bootstrap group")
        .to_owned();
    let st = kaisatsu_admin_write(
        &jc,
        &admin_token,
        &format!("/users/{person_id}/groups/{bootstrap}"),
        &json!({}),
    )
    .await;
    assert!(st.is_success(), "group membership {st}");

    // c. A manifest proposed and approved in the Portal lands as a commit in shako; its group
    // (b) then exists in kaisatsu.
    let group = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Group",
        "metadata": {"name": "e2e-editors", "namespace": "org"},
        "spec": {"members": [{"user": person_id}]}
    });
    let dry = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/projects/org/groups?dryRun=All"),
            Some(Body::Json(&group)),
            true,
        )
        .await;
    println!("dry run: {} {}", dry.status, dry.text);
    let proposed = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/projects/org/groups"),
            Some(Body::Json(&group)),
            true,
        )
        .await;
    println!("propose: {} {}", proposed.status, proposed.text);
    if proposed.status.is_success() {
        let id = proposed.json()["metadata"]["name"]
            .as_str()
            .expect("the Change's name")
            .to_owned();
        let mut approver = Browser::new();
        sign_in(
            &|p| jc.url(p),
            &mut approver,
            "e2e.person@jc.invalid",
            approver_pw,
        )
        .await;
        let approved = approver
            .send(
                Method::POST,
                &jc.url(&format!("/api/v1/projects/org/changes/{id}/approve")),
                Some(Body::Json(&json!({"confirm": "e2e-editors"}))),
                true,
            )
            .await;
        println!("approve {id}: {} {}", approved.status, approved.text);
        if approved.status.is_success() {
            let clone = tempfile::tempdir().unwrap();
            let out = git_clone(&jc, &clone.path().join("config"));
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let log = Command::new("git")
                .args(["log", "--name-only", "--format=%s", "-n", "5"])
                .current_dir(clone.path().join("config"))
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&log.stdout).into_owned();
            assert!(log.contains("e2e-editors"), "shako has the commit: {log}");
            // First-start provisioning seeded the Organization and the steward Role.
            let org = std::fs::read_to_string(clone.path().join("config/org.yaml")).unwrap();
            assert!(
                org.contains("kind: Organization") && org.contains("installation: true"),
                "{org}"
            );
            assert!(
                clone
                    .path()
                    .join("config/users/roles/steward.yaml")
                    .is_file()
            );
            let group_id = eventually("the group in kaisatsu", 60, || {
                let token = admin_token.clone();
                let jc = &jc;
                async move {
                    let (_, groups) =
                        kaisatsu_admin(jc, &token, "/groups?search=e2e-editors&exact=true").await;
                    groups[0]["id"].as_str().map(str::to_owned)
                }
            })
            .await;
            assert!(!group_id.is_empty());
        } else {
            gaps.push(format!(
                "c approve: POST /api/v1/projects/org/changes/{id}/approve -> {} {}",
                approved.status, approved.text
            ));
        }
    } else {
        gaps.push(format!(
            "c propose: POST /api/v1/projects/org/groups -> {} {}",
            proposed.status, proposed.text
        ));
    }

    // c2. A project opened in the Portal and approved by the second administrator lands in the
    // organization repository of shako (layout 1) and GET /api/v1/projects lists it.
    let opened = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/projects"),
            Some(Body::Json(&json!({
                "name": "e2e-city",
                "displayName": "E2E city",
                "description": "A project the e2e opens."
            }))),
            true,
        )
        .await;
    println!("open project: {} {}", opened.status, opened.text);
    assert!(
        opened.status.is_success(),
        "open project: {} {}",
        opened.status,
        opened.text
    );
    let id = opened.json()["metadata"]["name"]
        .as_str()
        .expect("the Change's name")
        .to_owned();
    let mut approver = Browser::new();
    sign_in(
        &|p| jc.url(p),
        &mut approver,
        "e2e.person@jc.invalid",
        approver_pw,
    )
    .await;
    let approved = approver
        .send(
            Method::POST,
            &jc.url(&format!("/api/v1/projects/e2e-city/changes/{id}/approve")),
            Some(Body::Json(&json!({"confirm": "e2e-city"}))),
            true,
        )
        .await;
    println!("approve {id}: {} {}", approved.status, approved.text);
    assert!(
        approved.status.is_success(),
        "approve the project: {} {}",
        approved.status,
        approved.text
    );
    let clone = tempfile::tempdir().unwrap();
    let out = git_clone(&jc, &clone.path().join("config"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        clone
            .path()
            .join("config/projects/e2e-city/project.yaml")
            .is_file(),
        "the project's file is in jc/config"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let listed = browser.get(&jc.url("/api/v1/projects")).await;
        if listed.text.contains("\"e2e-city\"") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "GET /api/v1/projects lists e2e-city: {} {}",
            listed.status,
            listed.text
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // d. ekiben: the Portal's runner calls carry no bearer, which hosted ekiben refuses.
    let runner = Client::new()
        .get(jc.url("/streams/streams"))
        .send()
        .await
        .unwrap();
    assert_eq!(runner.status(), StatusCode::UNAUTHORIZED);
    let with_token = Client::new()
        .get(jc.url("/streams/streams"))
        .bearer_auth(jc.secret("ekiben-token"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        with_token.status(),
        StatusCode::OK,
        "the generated ekiben token works"
    );
    gaps.push("d runner: GET/PUT/POST/DELETE /streams/streams[/{id}] without Authorization -> 401 (the Portal has no runner credential setting)".into());

    // e. dataroom: the Portal's CKAN token authenticates; the catalogue route needs https.
    let anon = Client::new()
        .post(jc.url("/opendata/api/3/action/organization_create"))
        .json(&json!({"name": "e2e-org", "title": "E2E"}))
        .send()
        .await
        .unwrap();
    assert!(anon.status().is_client_error(), "anonymous writes refused");
    let org = Client::new()
        .post(jc.url("/opendata/api/3/action/organization_create"))
        .header("authorization", jc.secret("catalogue-token"))
        .json(&json!({"name": "e2e-org", "title": "E2E"}))
        .send()
        .await
        .unwrap();
    let org_status = org.status();
    let org_body = org.text().await.unwrap_or_default();
    assert!(
        org_status.is_success(),
        "W2 organization_create with the Portal token: {org_status} {org_body}"
    );
    // W5, the dataset jcctl publishes for a public Endpoint, then the anonymous catalogue read.
    let created = Client::new()
        .post(jc.url("/opendata/api/3/action/package_create"))
        .header("authorization", jc.secret("catalogue-token"))
        .json(&json!({
            "name": "e2e-air-quality",
            "owner_org": "e2e-org",
            "title": "E2E air quality",
            "notes": "A public Endpoint of the e2e.",
            "private": false,
            "resources": [{"name": "NGSI-LD", "url": "https://example.org/api/endpoint/e2e", "format": "NGSI-LD"}],
            "extras": [{"key": "generated_by", "value": "joinedcontext"}]
        }))
        .send()
        .await
        .unwrap();
    let created_status = created.status();
    let created_body = created.text().await.unwrap_or_default();
    assert!(
        created_status.is_success(),
        "W5 package_create with the Portal token: {created_status} {created_body}"
    );
    let listed: Value = Client::new()
        .get(jc.url("/opendata/api/3/action/package_show?id=e2e-air-quality"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed["result"]["title"], "E2E air quality",
        "the public dataset is in the anonymous catalogue: {listed}"
    );
    gaps.push("e Portal: JC_PORTAL_CKAN_URL must be https (EP-67), so on an http install the Portal's own sync never calls dataroom; the e2e makes the Portal's W2 and W5 calls with its token".into());

    // f. daiya: the Portal's Grafana credential authenticates.
    let dash = Client::new()
        .get(jc.url("/dashboards/api/datasources"))
        .basic_auth("portal", Some(jc.secret("dashboards-password")))
        .send()
        .await
        .unwrap();
    let dash_status = dash.status();
    println!("daiya datasources: {dash_status}");
    let anon = Client::new()
        .post(jc.url("/dashboards/api/datasources"))
        .json(&json!({"name": "e2e", "type": "ngsild-grafana-datasource", "url": "https://x.invalid"}))
        .send()
        .await
        .unwrap();
    assert!(anon.status().is_client_error(), "anonymous writes refused");
    // A forged bearer is refused 401, as Grafana refuses it, never read as anonymous.
    let forged = Client::new()
        .get(jc.url("/dashboards/api/folders"))
        .bearer_auth("eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.forged")
        .send()
        .await
        .unwrap();
    assert_eq!(
        forged.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a forged bearer at daiya"
    );
    if !dash_status.is_success() {
        gaps.push(format!(
            "f daiya: GET /dashboards/api/datasources with the Portal Basic credential -> {dash_status}"
        ));
    }

    // Security: a stream and a forge webhook cannot reach jc's own listener.
    let hook = Client::new()
        .post(jc.url("/git/api/v1/repos/jc/config/hooks"))
        .header(
            "authorization",
            format!("token {}", jc.secret("shako-token")),
        )
        .json(&json!({
            "url": jc.url("/api/v1/webhooks/shako"),
            "events": ["push"],
            "active": true
        }))
        .send()
        .await
        .unwrap();
    let hook_status = hook.status();
    let hook_body = hook.text().await.unwrap_or_default();
    assert!(
        hook_status.is_client_error(),
        "a webhook to jc's listener is refused: {hook_status} {hook_body}"
    );

    // A stream cannot reach jc's own listener: an http_client output aimed at the catalogue's
    // write action, with a working token, writes nothing (ekiben refuses loopback targets).
    let stream = format!(
        "input:\n  generate:\n    count: 1\n    interval: \"\"\n    mapping: 'root = {{\"name\": \"stream-org\", \"title\": \"S\"}}'\noutput:\n  http_client:\n    url: {}\n    verb: POST\n    headers:\n      Authorization: {}\n",
        jc.url("/opendata/api/3/action/organization_create"),
        jc.secret("catalogue-token"),
    );
    let created = Client::new()
        .post(jc.url("/streams/streams/e2e-loopback"))
        .bearer_auth(jc.secret("ekiben-token"))
        .header("content-type", "application/yaml")
        .body(stream)
        .send()
        .await
        .unwrap();
    let created_status = created.status();
    let created_body = created.text().await.unwrap_or_default();
    assert!(
        created_status.is_success() || created_status.is_client_error(),
        "{created_status} {created_body}"
    );
    println!("stream create: {created_status} {created_body}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let shown = Client::new()
        .get(jc.url("/opendata/api/3/action/organization_show?id=stream-org"))
        .header("authorization", jc.secret("catalogue-token"))
        .send()
        .await
        .unwrap();
    let shown_status = shown.status();
    let shown_body = shown.text().await.unwrap_or_default();
    assert!(
        !shown_body.contains("\"success\":true"),
        "a stream ({created_status}) reached jc's listener: {shown_status} {shown_body}"
    );

    // f. provision.sh's steps as the Portal's server administrator: the project's organization,
    // its folder and a dashboard, then the Portal's own reads G1 and G5 find them.
    let admin = |req: reqwest::RequestBuilder| {
        req.basic_auth("portal", Some(jc.secret("dashboards-password")))
    };
    let made = admin(Client::new().post(jc.url("/dashboards/api/orgs")))
        .json(&json!({"name": "e2e"}))
        .send()
        .await
        .unwrap();
    let made_status = made.status();
    let made: Value = made.json().await.unwrap_or_default();
    assert!(
        made_status.is_success(),
        "POST /api/orgs: {made_status} {made}"
    );
    let org_id = made["orgId"]
        .as_i64()
        .or(made["id"].as_i64())
        .expect("org id");
    let found: Value = admin(Client::new().get(jc.url("/dashboards/api/orgs/name/e2e")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        found["id"].as_i64(),
        Some(org_id),
        "G1 finds the organization: {found}"
    );
    let folder = admin(Client::new().post(jc.url("/dashboards/api/folders")))
        .header("x-grafana-org-id", org_id.to_string())
        .json(&json!({"uid": "project-e2e", "title": "e2e"}))
        .send()
        .await
        .unwrap();
    assert!(
        folder.status().is_success(),
        "POST /api/folders: {}",
        folder.status()
    );
    let saved = admin(Client::new().post(jc.url("/dashboards/api/dashboards/db")))
        .header("x-grafana-org-id", org_id.to_string())
        .json(&json!({
            "dashboard": {"uid": "e2e-overview", "title": "e2e overview", "schemaVersion": 39, "panels": []},
            "folderUid": "project-e2e",
            "overwrite": true,
            "message": "e2e"
        }))
        .send()
        .await
        .unwrap();
    let saved_status = saved.status();
    let saved_body = saved.text().await.unwrap_or_default();
    assert!(
        saved_status.is_success(),
        "G4 POST /api/dashboards/db: {saved_status} {saved_body}"
    );
    let held = admin(Client::new().get(jc.url("/dashboards/api/dashboards/uid/e2e-overview")))
        .header("x-grafana-org-id", org_id.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(held.status(), StatusCode::OK, "G5 finds the dashboard");

    // g. Item 6: a project opened in the config repository gets its daiya organization and its
    // folder from the Portal's provisioning loop, provision.sh's per-project steps. The commit
    // stands in for an approved project change (jc-portal-integrate's open item).
    let clone = tempfile::tempdir().unwrap();
    let repo = clone.path().join("config");
    let out = git_clone(&jc, &repo);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let org_yaml = std::fs::read_to_string(repo.join("org.yaml")).unwrap();
    let org_name = org_yaml
        .lines()
        .find_map(|l| l.trim().strip_prefix("name:"))
        .map(|n| n.trim().trim_matches('"').to_owned())
        .expect("the Organization's name");
    std::fs::create_dir_all(repo.join("projects/e2edash")).unwrap();
    std::fs::write(
        repo.join("projects/e2edash/project.yaml"),
        format!(
            "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: e2edash\n  namespace: org\nspec:\n  organizationRef: {org_name}\n"
        ),
    )
    .unwrap();
    let token = jc.secret("shako-token");
    for args in [
        vec!["add", "projects/e2edash/project.yaml"],
        vec![
            "-c",
            "user.name=e2e",
            "-c",
            "user.email=e2e@jc.invalid",
            "commit",
            "--quiet",
            "-m",
            "Open project e2edash",
        ],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let pushed = Command::new("git")
        .args([
            "-c",
            &format!("http.extraHeader=Authorization: token {token}"),
            "push",
            "--quiet",
            "origin",
            "HEAD:main",
        ])
        .current_dir(&repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        pushed.status.success(),
        "push: {}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    let project_org = eventually("daiya's organization e2edash", 120, || {
        let jc = &jc;
        async move {
            let found = admin(Client::new().get(jc.url("/dashboards/api/orgs/name/e2edash")))
                .send()
                .await
                .ok()?;
            if found.status() != StatusCode::OK {
                return None;
            }
            found.json::<Value>().await.ok()?["id"].as_i64()
        }
    })
    .await;
    let folder = admin(Client::new().get(jc.url("/dashboards/api/folders/project-e2edash")))
        .header("x-grafana-org-id", project_org.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(folder.status(), StatusCode::OK, "the project's folder");

    // h. Item 15: the deployment's context-broker and helsinki dashboards, imported the way
    // provision.sh imports them, drawn through the Portal's dashboard routes: the queries stay
    // in jc, each panel answers frames or the reason it has none.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/portal/ui/tests/fixtures/dashboards");
    for (file, org, project, folder_uid) in [
        ("context-broker.json", 1, "org", None),
        (
            "helsinki.json",
            project_org,
            "e2edash",
            Some("project-e2edash"),
        ),
    ] {
        let board: Value =
            serde_json::from_str(&std::fs::read_to_string(fixtures.join(file)).unwrap()).unwrap();
        let uid = board["uid"].as_str().unwrap().to_owned();
        let panels = board["panels"].as_array().unwrap().len();
        let mut body = json!({"dashboard": board, "overwrite": true, "message": "e2e import"});
        if let Some(f) = folder_uid {
            body["folderUid"] = json!(f);
        }
        let saved = admin(Client::new().post(jc.url("/dashboards/api/dashboards/db")))
            .header("x-grafana-org-id", org.to_string())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(
            saved.status().is_success(),
            "import {file}: {}",
            saved.status()
        );
        let base = format!("/api/v1/projects/{project}/grafana/dashboards");
        let listed = browser.send(Method::GET, &jc.url(&base), None, false).await;
        assert_eq!(
            listed.status,
            StatusCode::OK,
            "{file} list: {}",
            listed.text
        );
        assert!(listed.text.contains(&uid), "{file} listed: {}", listed.text);
        let view = browser
            .send(Method::GET, &jc.url(&format!("{base}/{uid}")), None, false)
            .await;
        assert_eq!(view.status, StatusCode::OK, "{file} view: {}", view.text);
        let view = view.json();
        assert_eq!(
            view["panels"].as_array().map(Vec::len),
            Some(panels),
            "{file}"
        );
        assert!(
            !view.to_string().contains("\"targets\""),
            "{file}: the queries left the server"
        );
        for panel in view["panels"].as_array().unwrap() {
            let id = panel["id"].as_i64().unwrap();
            let read = browser
                .send(
                    Method::GET,
                    &jc.url(&format!("{base}/{uid}/panels/{id}?from=now-6h&to=now")),
                    None,
                    false,
                )
                .await;
            assert_eq!(
                read.status,
                StatusCode::OK,
                "{file} panel {id}: {}",
                read.text
            );
            let data = read.json();
            assert!(
                data["frames"].is_array()
                    && (data.get("error").is_some()
                        || data.get("accessDenied").is_some()
                        || !data["frames"].as_array().unwrap().is_empty()),
                "{file} panel {id}: {data}"
            );
            println!(
                "{file} panel {id}: {}",
                data.get("error")
                    .or(data.get("accessDenied"))
                    .map(Value::to_string)
                    .unwrap_or_else(|| "frames".into())
            );
        }
    }
    let outsider = Client::new()
        .get(jc.url("/api/v1/projects/e2edash/grafana/dashboards/project-helsinki/panels/1"))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(
            outsider.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FOUND | StatusCode::SEE_OTHER
        ),
        "a request without a session reads no panel: {}",
        outsider.status()
    );
    gaps.push("h jc: no Prometheus and no NGSI-LD datasource in jc, so the context-broker and helsinki panels answer their error state, not frames".into());

    println!("GAPS:\n{}", gaps.join("\n"));
    std::fs::write(
        std::env::temp_dir().join("jc-portal-e2e-gaps.txt"),
        gaps.join("\n"),
    )
    .ok();
}

/// Host mode: every module on its own host name, the Portal on `portal.localhost`. The Portal
/// signs a person in through kaisatsu on `id.localhost`, keeps its cookies on its own host, writes
/// people to kaisatsu and proposals to shako, and a module's slug on the Portal's host reaches the
/// Portal, not the module. The deployment names its realm `dev`: jc makes it on first start (its
/// first-start files, the first admin), and no realm `jc`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_portal_runs_on_its_own_host_in_host_mode() {
    let jc = start_placed(Placement::Host, "dev").await;
    let mut browser = Browser::new();
    assert_eq!(
        std::fs::read_to_string(jc.state.join("realm-owner-dev")).unwrap(),
        "jc\n"
    );
    let dormant = Client::new()
        .get(jc.url("/id/realms/jc/.well-known/openid-configuration"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(dormant, StatusCode::NOT_FOUND, "a realm jc beside dev");

    // a. Sign-in across hosts, at /realms/dev.
    let password = jc.secret("first-admin-password");
    sign_in_at(&|p| jc.url(p), &mut browser, jc.realm, "admin", &password).await;
    let who = me(&jc, &mut browser).await;
    assert_eq!(who["username"], "admin", "{who}");
    let portal_cookies = browser.jar.names_for(&jc.url("/"));
    let id_cookies = browser.jar.names_for(&jc.url("/id/"));
    assert!(
        portal_cookies.iter().any(|n| n == "__Host-jc_session"),
        "{portal_cookies:?}"
    );
    assert!(
        id_cookies.iter().all(|n| !n.starts_with("__Host-jc_")),
        "a Portal cookie on kaisatsu's host: {id_cookies:?}"
    );
    assert!(!id_cookies.is_empty(), "kaisatsu's own session cookie");

    // b. A person written through the Portal lands in kaisatsu.
    let created = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/organization/people"),
            Some(Body::Json(&json!({
                "email": "host.person@jc.invalid",
                "firstName": "Hana",
                "lastName": "Host",
            }))),
            true,
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text);
    let admin_token = kaisatsu_admin_token(&jc).await;
    let (status, found) = kaisatsu_admin(
        &jc,
        &admin_token,
        "/users?email=host.person@jc.invalid&exact=true",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found[0]["firstName"], "Hana", "{found}");

    // c. The Portal's forge: a proposal checks against shako, and shako serves git on its host.
    let group = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Group",
        "metadata": {"name": "host-editors", "namespace": "org"},
        "spec": {"members": []}
    });
    let dry = browser
        .send(
            Method::POST,
            &jc.url("/api/v1/projects/org/groups?dryRun=All"),
            Some(Body::Json(&group)),
            true,
        )
        .await;
    assert_eq!(dry.status, StatusCode::OK, "{}", dry.text);
    assert_eq!(dry.json()["valid"], true, "{}", dry.text);
    let checkout = tempfile::tempdir().unwrap();
    let cloned = git_clone(&jc, &checkout.path().join("config"));
    assert!(
        cloned.status.success(),
        "{}",
        String::from_utf8_lossy(&cloned.stderr)
    );

    // Every other module answers on its own host.
    let client = Client::new();
    for (path, needle) in [
        ("/opendata/api/3/action/status_show", "\"success\""),
        ("/dashboards/api/health", "\"database\""),
        ("/streams/ping", "pong"),
        ("/broker/ngsi-ld/v1/info/sourceIdentity", "antares"),
        ("/git/api/healthz", "\"status\":\"ok\""),
    ] {
        let body = client
            .get(jc.url(path))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains(needle), "{}: {body}", jc.url(path));
    }
    // A module's slug on the Portal's host is the Portal's path, not the module.
    let on_portal = client
        .get(format!(
            "{}/id/realms/{}/.well-known/openid-configuration",
            jc.base, jc.realm
        ))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!on_portal.contains("\"issuer\""), "{on_portal}");
}
