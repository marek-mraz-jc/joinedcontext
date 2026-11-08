//! End to end: the `jc` binary with `--role all` on a free port, the Portal wired to its sibling
//! modules by `jc` (no external Keycloak, Gitea, Bento, CKAN or Grafana), driven only through the
//! public mounts, each step asserted on the module it lands in.
//!
//! kaisatsu runs on PostgreSQL when `KAISATSU_TEST_DATABASE_URL` (a superuser URL) is set; the
//! other modules have no Postgres path in `jc` yet (memory stores, STATUS.md).

use reqwest::{Client, Method, StatusCode, header};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const REALM: &str = "jc";

struct Jc {
    child: Child,
    base: String,
    state: PathBuf,
    _dir: tempfile::TempDir,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
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
    fn secret(&self, name: &str) -> String {
        std::fs::read_to_string(self.state.join(name))
            .unwrap()
            .trim()
            .to_owned()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn log_has(&self, needle: &str) -> bool {
        self.log.lock().unwrap().iter().any(|l| l.contains(needle))
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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
    for (role, pw) in [("kaisatsu_app", &app_pw), ("kaisatsu_migrate", &mig_pw)] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DO $$ BEGIN CREATE ROLE {role}; EXCEPTION WHEN duplicate_object THEN NULL; END $$;
             ALTER ROLE {role} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE PASSWORD '{pw}';"
        )))
        .execute(&admin)
        .await
        .unwrap();
    }
    let db = format!("kaisatsu_e2e_{suffix}");
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
    // The Portal at the root of the one domain: its redirects and UI assets are absolute paths.
    let config = dir.path().join("jc.toml");
    std::fs::write(&config, "[mounts.portal]\npath_prefix = \"\"\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") || key.starts_with("JC_") {
            cmd.env_remove(key);
        }
    }
    cmd.args(["--role", "all", "--scheme", "http", "--log-format", "text"])
        .arg("--config")
        .arg(&config)
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--base-host", &base_host])
        .arg("--state-dir")
        .arg(&state)
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
            if line.contains("jc listening") {
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
        base: format!("http://{base_host}"),
        state,
        _dir: dir,
        log,
    };
    match rx.recv_timeout(Duration::from_secs(180)) {
        Ok(Ok(())) => jc,
        other => {
            let tail = jc.log.lock().unwrap().join("\n");
            panic!("jc did not start ({other:?}):\n{tail}");
        }
    }
}

/// A browser-like cookie jar: name -> (value, path). Secure cookies are kept on
/// `http://localhost`, as browsers do for localhost.
#[derive(Default)]
struct Jar(BTreeMap<String, (String, String)>);

impl Jar {
    fn store(&mut self, headers: &header::HeaderMap) {
        for raw in headers.get_all(header::SET_COOKIE) {
            let raw = raw.to_str().unwrap_or_default();
            let mut parts = raw.split(';').map(str::trim);
            let Some((name, value)) = parts.next().and_then(|kv| kv.split_once('=')) else {
                continue;
            };
            let mut path = "/".to_owned();
            let mut expired = false;
            for attr in parts {
                let lower = attr.to_ascii_lowercase();
                if let Some(p) = lower.strip_prefix("path=") {
                    path = attr[5..5 + p.len()].to_owned();
                }
                if lower == "max-age=0" {
                    expired = true;
                }
            }
            if expired || value.is_empty() {
                self.0.remove(name);
            } else {
                self.0.insert(name.to_owned(), (value.to_owned(), path));
            }
        }
    }

    fn header_for(&self, url: &str) -> String {
        let path = url
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/");
        self.0
            .iter()
            .filter(|(_, (_, p))| path.starts_with(p.as_str()))
            .map(|(n, (v, _))| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(|(v, _)| v.as_str())
    }
}

struct Browser {
    client: Client,
    jar: Jar,
}

struct Answer {
    status: StatusCode,
    headers: header::HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    fn location(&self) -> String {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

impl Browser {
    fn new() -> Self {
        Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
            jar: Jar::default(),
        }
    }

    async fn send(
        &mut self,
        method: Method,
        url: &str,
        body: Option<Body<'_>>,
        csrf: bool,
    ) -> Answer {
        let mut req = self.client.request(method, url);
        let cookies = self.jar.header_for(url);
        if !cookies.is_empty() {
            req = req.header(header::COOKIE, cookies);
        }
        if csrf && let Some(token) = self.jar.get("__Host-jc_csrf") {
            req = req.header("x-csrf-token", token);
        }
        req = match body {
            Some(Body::Form(fields)) => req.form(fields),
            Some(Body::Json(value)) => req.json(value),
            None => req,
        };
        let resp = req.send().await.unwrap_or_else(|e| panic!("{url}: {e}"));
        let status = resp.status();
        let headers = resp.headers().clone();
        self.jar.store(&headers);
        Answer {
            status,
            headers,
            text: resp.text().await.unwrap_or_default(),
        }
    }

    async fn get(&mut self, url: &str) -> Answer {
        self.send(Method::GET, url, None, false).await
    }

    /// Follows redirects by hand, keeping cookies, until a non-redirect answer or `stop` matches
    /// the next location.
    async fn follow(&mut self, mut url: String, stop: impl Fn(&str) -> bool) -> (String, Answer) {
        for _ in 0..12 {
            let answer = self.get(&url).await;
            if !answer.status.is_redirection() {
                return (url, answer);
            }
            let next = answer.location();
            let next = if next.starts_with("http") {
                next
            } else {
                let origin = &url[..url[8..].find('/').map_or(url.len(), |i| i + 8)];
                format!("{origin}{next}")
            };
            if stop(&next) {
                return (next, answer);
            }
            url = next;
        }
        panic!("too many redirects from {url}");
    }
}

enum Body<'a> {
    Form(&'a [(&'a str, &'a str)]),
    Json(&'a Value),
}

fn hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    let at = html
        .find(&marker)
        .unwrap_or_else(|| panic!("no {name} in {html}"));
    let tag_start = html[..at].rfind('<').unwrap();
    let tag_end = at + html[at..].find('>').unwrap();
    let tag = &html[tag_start..tag_end];
    let v = tag.find("value=\"").unwrap() + 7;
    tag[v..v + tag[v..].find('"').unwrap()].replace("&amp;", "&")
}

/// a. Signs `username` in to the Portal through kaisatsu: the Portal's login redirect, kaisatsu's
/// authorization endpoint (code + PKCE), its login form, back to the Portal's callback.
async fn sign_in(jc: &Jc, browser: &mut Browser, username: &str, password: &str) {
    let (login_url, page) = browser
        .follow(jc.url("/api/v1/auth/login"), |_| false)
        .await;
    assert_eq!(page.status, StatusCode::OK, "{login_url}: {}", page.text);
    assert!(
        login_url.starts_with(&jc.url(&format!("/id/realms/{REALM}/login-actions/authenticate"))),
        "the Portal sends the browser to kaisatsu: {login_url}"
    );
    let csrf = hidden(&page.text, "csrf_token");
    let session = hidden(&page.text, "session_code");
    let fields = [
        ("csrf_token", csrf.as_str()),
        ("session_code", session.as_str()),
        ("username", username),
        ("password", password),
    ];
    let posted = browser
        .send(Method::POST, &login_url, Some(Body::Form(&fields)), false)
        .await;
    assert!(
        posted.status.is_redirection(),
        "kaisatsu accepts the password: {} {}",
        posted.status,
        posted.text
    );
    let callback = posted.location();
    assert!(
        callback.starts_with(&jc.url("/api/v1/auth/callback?")),
        "{callback}"
    );
    assert!(callback.contains("code="), "{callback}");
    let (_, home) = browser.follow(callback, |_| false).await;
    assert!(home.status.is_success(), "{} {}", home.status, home.text);
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
        .post(jc.url(&format!("/id/realms/{REALM}/protocol/openid-connect/token")))
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
        .put(jc.url(&format!("/id/admin/realms/{REALM}{path}")))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap()
        .status()
}

async fn kaisatsu_admin(jc: &Jc, token: &str, path: &str) -> (StatusCode, Value) {
    let resp = Client::new()
        .get(jc.url(&format!("/id/admin/realms/{REALM}{path}")))
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

    // Setup: secrets are private files, never in the log.
    for name in ["portal-client-secret", "shako-token", "ekiben-token"] {
        let value = jc.secret(name);
        assert!(!jc.log_has(&value), "{name} appears in the log");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(jc.state.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }

    // a. Sign-in through kaisatsu.
    let password = jc.secret("first-admin-password");
    sign_in(&jc, &mut browser, "admin", &password).await;
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
    let st = kaisatsu_admin_write(
        &jc,
        &admin_token,
        &format!("/users/{person_id}/reset-password"),
        &json!({"type": "password", "value": approver_pw, "temporary": false}),
    )
    .await;
    assert!(st.is_success(), "reset-password {st}");
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
        sign_in(&jc, &mut approver, "e2e.person@jc.invalid", approver_pw).await;
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
            "url": jc.url("/api/v1/webhooks/gitea"),
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
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
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
        vec!["-c", "user.name=e2e", "-c", "user.email=e2e@jc.invalid", "commit", "--quiet", "-m", "Open project e2edash"],
    ] {
        let out = Command::new("git").args(&args).current_dir(&repo).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let pushed = Command::new("git")
        .args(["-c", &format!("http.extraHeader=Authorization: token {token}"), "push", "--quiet", "origin", "HEAD:main"])
        .current_dir(&repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(pushed.status.success(), "push: {}", String::from_utf8_lossy(&pushed.stderr));
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
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/portal/ui/tests/fixtures/dashboards");
    for (file, org, project, folder_uid) in [
        ("context-broker.json", 1, "org", None),
        ("helsinki.json", project_org, "e2edash", Some("project-e2edash")),
    ] {
        let board: Value = serde_json::from_str(&std::fs::read_to_string(fixtures.join(file)).unwrap()).unwrap();
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
        assert!(saved.status().is_success(), "import {file}: {}", saved.status());
        let base = format!("/api/v1/projects/{project}/grafana/dashboards");
        let listed = browser.send(Method::GET, &jc.url(&base), None, false).await;
        assert_eq!(listed.status, StatusCode::OK, "{file} list: {}", listed.text);
        assert!(listed.text.contains(&uid), "{file} listed: {}", listed.text);
        let view = browser.send(Method::GET, &jc.url(&format!("{base}/{uid}")), None, false).await;
        assert_eq!(view.status, StatusCode::OK, "{file} view: {}", view.text);
        let view = view.json();
        assert_eq!(view["panels"].as_array().map(Vec::len), Some(panels), "{file}");
        assert!(!view.to_string().contains("\"targets\""), "{file}: the queries left the server");
        for panel in view["panels"].as_array().unwrap() {
            let id = panel["id"].as_i64().unwrap();
            let read = browser
                .send(Method::GET, &jc.url(&format!("{base}/{uid}/panels/{id}?from=now-6h&to=now")), None, false)
                .await;
            assert_eq!(read.status, StatusCode::OK, "{file} panel {id}: {}", read.text);
            let data = read.json();
            assert!(
                data["frames"].is_array() && (data.get("error").is_some() || data.get("accessDenied").is_some() || !data["frames"].as_array().unwrap().is_empty()),
                "{file} panel {id}: {data}"
            );
            println!("{file} panel {id}: {}", data.get("error").or(data.get("accessDenied")).map(Value::to_string).unwrap_or_else(|| "frames".into()));
        }
    }
    let outsider = Client::new()
        .get(jc.url("/api/v1/projects/e2edash/grafana/dashboards/project-helsinki/panels/1"))
        .send()
        .await
        .unwrap();
    assert!(
        matches!(outsider.status(), StatusCode::UNAUTHORIZED | StatusCode::FOUND | StatusCode::SEE_OTHER),
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
