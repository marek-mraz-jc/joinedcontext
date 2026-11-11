#![allow(clippy::disallowed_methods)] // These tests start the jc binary.
//! End to end: Endpoints inside `jc --role all`, as on dev. The test writes a project to shako's
//! organization repository (one space, a public and a closed Endpoint, their Policies), waits for
//! the Portal's reconciler to write the views and rule documents into Antares and for the gateway
//! to load the Endpoints from its checkout, then reads and writes only through the public mount:
//! `/api/endpoint/{slug}/...` and `/api/mcp`, anonymous and with kaisatsu tokens.
//!
//! The people and clients come from a realm file in kaisatsu's import directory: a person in the
//! project's reader and writer groups, a client whose tokens name the closed Endpoint, and one
//! whose tokens name only another Endpoint.

mod common;

use base64::Engine as _;
use common::{Browser, free_port, lost_the_port, sign_in};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROJECT: &str = "e2e";
const SPACE: &str = "things";
/// The owner segment every entity id of the default Organization (`jc.invalid`) carries (ADR-N-068).
const OWNER: &str = "invalid_jc";
const PUBLIC: &str = "e2epublicendpointaaaaaaaaa";
const CLOSED: &str = "e2eclosedendpointaaaaaaaaa";
const ELSEWHERE: &str = "e2eotherendpointaaaaaaaaaa";
const PERSON: &str = "e2e.person";
const PERSON_PASSWORD: &str = "e2e-person-password-not-secret";
/// A person in none of the project's groups: no Policy of the space names them.
const STRANGER: &str = "e2e.stranger";
const READER_SECRET: &str = "e2e-reader-client-secret-not-secret";
/// The project's pipeline client secret (`{PROJECT}-pipelines`), and another project's.
const PIPELINES_SECRET: &str = "e2e-pipelines-client-secret-not-secret";
const OTHER_PIPELINES_SECRET: &str = "other-pipelines-client-secret-not-secret";
/// The client of the pipeline `feed` itself, `{PROJECT}-pl-feed`, as prod made it for each
/// Pipeline (PL-19) and an import switches to its secret: its tokens name the closed Endpoint the
/// pipeline writes and the Portal's internal listener its reports post to, nothing else.
const FEED_CLIENT: &str = "e2e-pl-feed";
const FEED_SECRET: &str = "e2e-pl-feed-client-secret-not-secret";
/// A device of the project's MQTT broker: ServiceAccount `cam`, Keycloak client `e2e-cam`, its
/// tokens bound to the broker's release `mqtt-e2e-cams` (ADR-N-048).
const CAMERA: &str = "e2e-cam";
const CAMERA_SECRET: &str = "e2e-camera-client-secret-not-secret";
const CAMERAS: &str = "mqtt-e2e-cams";
/// The organization repository, not jc's default `jc/config`: the gateway must check out the
/// repository the Portal is told to use (`JC_SHAKO_OWNER`, `JC_SHAKO_REPO`).
const FORGE_OWNER: &str = "e2e-city";
const FORGE_REPO: &str = "configuration";

struct Jc {
    child: Child,
    base: String,
    root: PathBuf,
    state: PathBuf,
    /// The directory `root` is, when this `jc` owns it (a restart test keeps its own).
    _dir: Option<tempfile::TempDir>,
    log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// The file the whole log goes to: one per test and run, so a red run keeps its evidence.
    log_file: PathBuf,
}

impl Drop for Jc {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.save_log();
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
    /// A secret the test gave jc as a file, else jc's state file.
    fn secret(&self, name: &str) -> String {
        let given = self.root.join("given").join(name);
        let path = if given.exists() {
            given
        } else {
            self.state.join(name)
        };
        std::fs::read_to_string(path).unwrap().trim().to_owned()
    }

    fn log_has(&self, needle: &str) -> bool {
        self.log.lock().unwrap().iter().any(|l| l.contains(needle))
    }

    /// Waits until jc's log holds `needle`: the reader thread copies jc's stderr a moment after
    /// jc wrote it, so a line jc logged before it answered can still be on its way.
    async fn wait_log(&self, what: &str, needle: &str) {
        eventually(what, self, 30, || async {
            self.log_has(needle)
                .then_some(())
                .ok_or(format!("no line holding {needle:?} yet"))
        })
        .await;
    }

    /// Puts the test's own step into the log's timeline, so a red run shows what jc had logged
    /// by then.
    fn mark(&self, step: &str) {
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        self.log.lock().unwrap().push(format!("{at} TEST {step}"));
    }

    fn save_log(&self) {
        let _ = std::fs::write(&self.log_file, self.log.lock().unwrap().join("\n"));
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The last lines of the child's log; the whole log goes to a file named in the answer.
    fn tail(&self) -> String {
        self.save_log();
        let log = self.log.lock().unwrap();
        format!(
            "{}\n(whole log: {})",
            log[log.len().saturating_sub(60)..].join("\n"),
            self.log_file.display()
        )
    }
}

/// The test's people and clients, imported into `realm` (file `file` of the import dir) before
/// the Portal's own files. Their secrets are `$(file:...)` under `secrets`
/// (`--kaisatsu-import-secrets-dir`).
fn realm_file(dir: &Path, secrets: &Path, realm: &str, file: &str) {
    std::fs::create_dir_all(secrets).unwrap();
    let client_secret = secrets.join("e2e-client-secret");
    let person_password = secrets.join("e2e-person-password");
    std::fs::write(&client_secret, READER_SECRET).unwrap();
    std::fs::write(&person_password, PERSON_PASSWORD).unwrap();
    let client_secret = format!("$(file:{})", client_secret.display());
    let person_password = format!("$(file:{})", person_password.display());
    // Each project's pipeline client, its secret a file named by the client: kaisatsu reads it
    // for the client, jc hands it to ekiben as `JC_CLIENT_SECRET_<PROJECT>`.
    let pipelines_secret = secrets.join(format!("{PROJECT}-pipelines"));
    std::fs::write(&pipelines_secret, PIPELINES_SECRET).unwrap();
    std::fs::write(secrets.join("other-pipelines"), OTHER_PIPELINES_SECRET).unwrap();
    let pipelines_secret = format!("$(file:{})", pipelines_secret.display());
    let feed_secret = secrets.join(FEED_CLIENT);
    std::fs::write(&feed_secret, FEED_SECRET).unwrap();
    let feed_secret = format!("$(file:{})", feed_secret.display());
    let camera_secret = secrets.join(CAMERA);
    std::fs::write(&camera_secret, CAMERA_SECRET).unwrap();
    let camera_secret = format!("$(file:{})", camera_secret.display());
    let audience = |name: &str, slug: &str| {
        json!({
            "name": name,
            "protocol": "openid-connect",
            "protocolMapper": "oidc-audience-mapper",
            "consentRequired": false,
            "config": {
                "included.custom.audience": slug,
                "access.token.claim": "true",
                "id.token.claim": "false"
            }
        })
    };
    let groups = json!({
        "name": "groups",
        "protocol": "openid-connect",
        "protocolMapper": "oidc-group-membership-mapper",
        "consentRequired": false,
        "config": {
            "claim.name": "groups",
            "full.path": "true",
            "access.token.claim": "true",
            "id.token.claim": "false"
        }
    });
    let client = |id: &str, slug: &str| {
        json!({
            "clientId": id,
            "enabled": true,
            "protocol": "openid-connect",
            "publicClient": false,
            "clientAuthenticatorType": "client-secret",
            "secret": client_secret,
            "standardFlowEnabled": false,
            "directAccessGrantsEnabled": true,
            "serviceAccountsEnabled": false,
            "fullScopeAllowed": false,
            "redirectUris": [],
            "webOrigins": [],
            "defaultClientScopes": ["basic"],
            "protocolMappers": [audience("endpoint", slug), groups.clone()]
        })
    };
    let realm = json!({
        "realm": realm,
        "enabled": true,
        "groups": [
            { "name": format!("{PROJECT}-readers") },
            { "name": format!("{PROJECT}-writers") }
        ],
        // `e2e-mcp` stands in for an MCP client: its person tokens name the Portal's client, which
        // `/api/v1/mcp` takes and exchanges for one Endpoint (`endpoint:{slug}`).
        "clients": [
            {
                "clientId": format!("{PROJECT}-pipelines"),
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": false,
                "clientAuthenticatorType": "client-secret",
                "secret": pipelines_secret,
                "standardFlowEnabled": false,
                "directAccessGrantsEnabled": false,
                "serviceAccountsEnabled": true,
                "fullScopeAllowed": false,
                "redirectUris": [],
                "webOrigins": [],
                "defaultClientScopes": ["basic"],
                "protocolMappers": [audience("endpoint", CLOSED)]
            },
            {
                "clientId": CAMERA,
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": false,
                "clientAuthenticatorType": "client-secret",
                "secret": camera_secret,
                "standardFlowEnabled": false,
                "directAccessGrantsEnabled": false,
                "serviceAccountsEnabled": true,
                "fullScopeAllowed": false,
                "redirectUris": [],
                "webOrigins": [],
                "defaultClientScopes": ["basic"],
                "protocolMappers": [audience("broker", CAMERAS)]
            },
            {
                "clientId": FEED_CLIENT,
                "enabled": true,
                "protocol": "openid-connect",
                "publicClient": false,
                "clientAuthenticatorType": "client-secret",
                "secret": feed_secret,
                "standardFlowEnabled": false,
                "directAccessGrantsEnabled": false,
                "serviceAccountsEnabled": true,
                "fullScopeAllowed": false,
                "redirectUris": [],
                "webOrigins": [],
                "defaultClientScopes": ["basic"],
                "attributes": { "joinedcontext.serviceaccount": format!("{PROJECT}/pl-feed") },
                "protocolMappers": [audience("endpoint", CLOSED), audience("reports", "portal-internal")]
            },
            client("e2e-reader", CLOSED),
            client("e2e-elsewhere", ELSEWHERE),
            client("e2e-mcp", "portal-api"),
            // The command line's client as on prod (audience `jcctl-endpoint`), confidential with
            // the password grant here so the test signs in without a device flow; jc's linker
            // gives it every Endpoint's scope as optional.
            client("jcctl", "jcctl-endpoint"),
        ],
        "users": [{
            "username": STRANGER,
            "enabled": true,
            "email": "e2e.stranger@jc.invalid",
            "emailVerified": true,
            "firstName": "No",
            "lastName": "Grant",
            "groups": [],
            "credentials": [{
                "type": "password",
                "value": person_password.clone(),
                "temporary": false
            }]
        }, {
            "username": PERSON,
            "enabled": true,
            "email": "e2e.person@jc.invalid",
            "emailVerified": true,
            "firstName": "End",
            "lastName": "Toend",
            "groups": [format!("/{PROJECT}-readers"), format!("/{PROJECT}-writers")],
            "credentials": [{
                "type": "password",
                "value": person_password,
                "temporary": false
            }]
        }]
    });
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file), realm.to_string()).unwrap();
}

async fn start() -> Jc {
    start_with(false).await
}

/// As [`start`]; `pipelines` sets the Portal's proposal validation to lax (`JC_BRANDING_FILE`),
/// since a Pipeline's check cannot run inside jc yet (`jc_pipeline_test` needs
/// `JC_PORTAL_PIPELINE_TEST_CAPTURE_URL`, which jc does not fill), and names the public host by
/// the listener's own address: a stream asks kaisatsu's public token endpoint, and ekiben refuses
/// a loopback name such as `localhost`.
async fn start_with(pipelines: bool) -> Jc {
    start_with_env(pipelines, &[]).await
}

/// As [`start_with`], with `envs` beside the test's own.
async fn start_with_env(pipelines: bool, envs: &[(&str, &str)]) -> Jc {
    for _ in 0..5 {
        if let Some(jc) = start_with_once(pipelines, envs).await {
            return jc;
        }
    }
    panic!("jc lost its listen port to another bind 5 times");
}

/// One [`start_with`] in a directory and on a port of its own; `None` when another bind took
/// the port before jc did.
async fn start_with_once(pipelines: bool, envs: &[(&str, &str)]) -> Option<Jc> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    realm_file(
        &root.join("realms"),
        &root.join("realm-secrets"),
        "jc",
        "00-e2e.json",
    );
    let port = free_port();
    let mut jc = if pipelines {
        // The runner's token as a file, so the test reads the runner's API as the Portal does.
        let given = root.join("given");
        std::fs::create_dir_all(&given).unwrap();
        let ekiben = given.join("ekiben-token");
        std::fs::write(
            &ekiben,
            format!("test-ekiben-token-{:032x}", std::process::id()),
        )
        .unwrap();
        let branding = root.join("branding.yaml");
        std::fs::write(&branding, "instanceName: \"e2e\"\nvalidation: lax\n").unwrap();
        start_on(
            root,
            &format!("127.0.0.1:{port}"),
            port,
            &["--ekiben-token-file", ekiben.to_str().unwrap()],
            &[&[("JC_BRANDING_FILE", branding.to_str().unwrap())], envs].concat(),
        )
        .await?
    } else {
        start_at(root, port, &[], envs).await?
    };
    jc._dir = Some(dir);
    Some(jc)
}

/// Starts `jc` over `root` (state, `realms` import dir and `realm-secrets` under it) on `port`,
/// with `args` and `envs` beside the test's own; `None` when another bind took `port` before jc
/// did ([`free_port`] cannot hold it for jc).
async fn start_at(root: &Path, port: u16, args: &[&str], envs: &[(&str, &str)]) -> Option<Jc> {
    start_on(root, &format!("localhost:{port}"), port, args, envs).await
}

/// As [`start_at`], with the public host named `base_host`.
async fn start_on(
    root: &Path,
    base_host: &str,
    port: u16,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Option<Jc> {
    let state = root.join("state");
    let realms = root.join("realms");
    let realm_secrets = root.join("realm-secrets");
    let config = root.join("jc.toml");
    std::fs::write(&config, "[mounts.portal]\npath_prefix = \"\"\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
    cmd.args(["--profile", "development"]);
    // The service tokens between modules live in jc's memory only; the test acts as the Portal
    // with a shako token it gives jc as a file.
    let given = root.join("given");
    std::fs::create_dir_all(&given).unwrap();
    let shako = given.join("shako-token");
    std::fs::write(
        &shako,
        format!("test-shako-token-{:032x}", std::process::id()),
    )
    .unwrap();
    cmd.arg("--shako-token-file").arg(&shako);
    for (key, _) in std::env::vars() {
        if key.starts_with("JCBIN_") || key.starts_with("JC_") || key.starts_with("ANTARES_") {
            cmd.env_remove(key);
        }
    }
    cmd.args(["--role", "all", "--scheme", "http", "--log-format", "text"])
        .arg("--config")
        .arg(&config)
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--base-host", base_host])
        .arg("--state-dir")
        .arg(&state)
        .arg("--kaisatsu-import-dir")
        .arg(&realms)
        .arg("--kaisatsu-import-secrets-dir")
        .arg(&realm_secrets)
        .env("JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR", "false")
        .env("JC_PORTAL_SYNC_INTERVAL", "3")
        .env("JC_SHAKO_OWNER", FORGE_OWNER)
        .env("JC_SHAKO_REPO", FORGE_REPO)
        .env("RUST_LOG", "info,sqlx=warn")
        .args(args)
        .envs(envs.iter().copied())
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
        root: root.to_path_buf(),
        state,
        _dir: None,
        log,
        // The test's thread carries its name: one file per test, pid and start.
        log_file: Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "gateway_endpoints-{}-{}-{}.log",
            std::thread::current()
                .name()
                .unwrap_or("jc")
                .replace("::", "-"),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        )),
    };
    match rx.recv_timeout(Duration::from_secs(180)) {
        Ok(Ok(())) => Some(jc),
        Ok(Err(())) if lost_the_port(&jc.log.lock().unwrap().join("\n")) => None,
        other => panic!("jc did not start ({other:?}):\n{}", jc.tail()),
    }
}

const HEADER: &str = "apiVersion: joinedcontext.com/v1alpha1";

/// The project's manifests at their layout 1 paths in the organization repository.
fn manifests() -> Vec<(String, String)> {
    let at = |rel: &str| format!("projects/{PROJECT}/{rel}");
    let space = |rel: &str| at(&format!("spaces/{SPACE}/{rel}"));
    let endpoint = |name: &str, slug: &str, audience: &str, extra: &str| {
        format!(
            "{HEADER}\nkind: Endpoint\nmetadata:\n  name: {name}\n  namespace: {PROJECT}\nspec:\n  \
             contextSpaceRef: {SPACE}\n  slug: {slug}\n  audience: {audience}\n  \
             enabledRepresentations: [ngsi-ld, mcp]\n{extra}"
        )
    };
    let policy = |name: &str, assignee: &str, operations: &str, types: &str| {
        format!(
            "{HEADER}\nkind: Policy\nmetadata:\n  name: {name}\n  namespace: {PROJECT}\nspec:\n  \
             contextSpaceRef: {SPACE}\n  assigner: did:web:jc.invalid\n  assignee: {assignee}\n  \
             operations: [{operations}]\n  information:\n    - entities:\n{types}"
        )
    };
    // No DataModel: a view shows only the Attributes a Policy lists.
    let thing = "        - type: Thing\n      propertyNames: [name]\n";
    vec![
        (
            at("project.yaml"),
            format!(
                "{HEADER}\nkind: Project\nmetadata:\n  name: {PROJECT}\n  namespace: org\nspec:\n  \
                 organizationRef: jc\n"
            ),
        ),
        (
            space("space.yaml"),
            format!(
                "{HEADER}\nkind: ContextSpace\nmetadata:\n  name: {SPACE}\n  namespace: {PROJECT}\n\
                 spec:\n  isSandbox: false\n  urnSegment: {SPACE}\n"
            ),
        ),
        (
            space("endpoints/public.yaml"),
            endpoint("public", PUBLIC, "public", ""),
        ),
        (
            space("endpoints/closed.yaml"),
            endpoint(
                "closed",
                CLOSED,
                "project-list",
                &format!("  allowedProjects: [{PROJECT}]\n"),
            ),
        ),
        (
            space("policies/public-read.yaml"),
            policy(
                "public-read",
                "{ kind: role, id: public }",
                "retrieveOps",
                thing,
            ),
        ),
        (
            space("policies/readers.yaml"),
            policy(
                "readers",
                &format!("{{ kind: group, id: {PROJECT}-readers }}"),
                "retrieveOps",
                thing,
            ),
        ),
        (
            space("policies/writers.yaml"),
            policy(
                "writers",
                &format!("{{ kind: group, id: {PROJECT}-writers }}"),
                "createEntity, upsertBatch",
                thing,
            ),
        ),
    ]
}

/// Writes `path` on `main` of shako's organization repository (Gitea contents API).
/// jc provisions that repository beside the Portal after it listens, so a first write waits for it.
async fn put_file(jc: &Jc, path: &str, content: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let resp = Client::new()
            .put(jc.url(&format!(
                "/git/api/v1/repos/{FORGE_OWNER}/{FORGE_REPO}/contents/{path}"
            )))
            .header(
                "authorization",
                format!("token {}", jc.secret("shako-token")),
            )
            .json(&json!({
                "content": base64::engine::general_purpose::STANDARD.encode(content),
                "message": format!("Add {path}"),
                "branch": "main",
            }))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND
            && body.contains("repository not found")
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        assert!(status.is_success(), "PUT contents/{path}: {status} {body}");
        jc.mark(&format!("PUT {path} answered {status}"));
        return;
    }
}

async fn eventually<F, Fut, T>(what: &str, jc: &Jc, secs: u64, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match probe().await {
            Ok(v) => return v,
            Err(last) if Instant::now() >= deadline => {
                panic!("{what}: not within {secs} s; last: {last}\n{}", jc.tail())
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

/// A person's token from `client` by the password grant.
async fn token(jc: &Jc, client: &str) -> String {
    token_of(jc, client, PERSON).await
}

async fn token_of(jc: &Jc, client: &str, username: &str) -> String {
    token_in(jc, "jc", client, username).await
}

/// A person's token from `client` of realm `jc` asking `scope`, or the refusal.
async fn scoped_token(
    jc: &Jc,
    client: &str,
    username: &str,
    scope: &str,
) -> Result<String, String> {
    let resp = Client::new()
        .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
        .form(&[
            ("grant_type", "password"),
            ("client_id", client),
            ("client_secret", READER_SECRET),
            ("username", username),
            ("password", PERSON_PASSWORD),
            ("scope", scope),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    match body["access_token"].as_str() {
        Some(token) if status == StatusCode::OK => Ok(token.to_owned()),
        _ => Err(format!("{status} {body}")),
    }
}

/// A person's token from `client` of `realm` by the password grant.
async fn token_in(jc: &Jc, realm: &str, client: &str, username: &str) -> String {
    let resp = Client::new()
        .post(jc.url(&format!("/id/realms/{realm}/protocol/openid-connect/token")))
        .form(&[
            ("grant_type", "password"),
            ("client_id", client),
            ("client_secret", READER_SECRET),
            ("username", username),
            ("password", PERSON_PASSWORD),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    assert_eq!(
        status,
        StatusCode::OK,
        "password grant for {client}: {body}"
    );
    body["access_token"].as_str().unwrap().to_owned()
}

async fn call(
    jc: &Jc,
    method: reqwest::Method,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, String) {
    let mut req = Client::new().request(method, jc.url(path));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if let Some(body) = body {
        req = req
            .header("content-type", "application/json")
            .body(body.to_string());
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    (status, resp.text().await.unwrap_or_default())
}

fn entity(kind: &str, local: &str) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:{kind}:{OWNER}:{local}"),
        "type": kind,
        "name": { "type": "Property", "value": local },
    })
}

/// Writes the project to the organization repository and waits until the public Endpoint
/// answers: the gateway loaded it from its checkout and the Portal wrote its views.
async fn open_project(jc: &Jc) {
    // jc creates the organization repository in the Portal's background start, after it
    // listens: write once shako holds it.
    eventually(
        "shako holds the organization repository",
        jc,
        120,
        || async {
            let resp = Client::new()
                .get(jc.url(&format!("/git/api/v1/repos/{FORGE_OWNER}/{FORGE_REPO}")))
                .header(
                    "authorization",
                    format!("token {}", jc.secret("shako-token")),
                )
                .send()
                .await
                .map_err(|e| e.to_string())?;
            match resp.status() {
                s if s.is_success() => Ok(()),
                s => Err(s.to_string()),
            }
        },
    )
    .await;
    for (path, content) in manifests() {
        put_file(jc, &path, &content).await;
    }
    let get = reqwest::Method::GET;
    let public = format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1");

    // The gateway loads the Endpoints from its checkout, the Portal writes the views into
    // Antares: the public Endpoint's types answer once both have run.
    eventually("the public Endpoint answers its types", jc, 120, || async {
        let (status, body) = call(jc, get.clone(), &format!("{public}/types"), None, None).await;
        if status == StatusCode::OK {
            return Ok(());
        }
        let views = Client::new()
            .get(jc.url("/broker/q/views"))
            .bearer_auth(jc.secret("antares-admin-token"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap_or_default();
        Err(format!("{status} {body}; the broker's views: {views}"))
    })
    .await;
    // The hub and its MCP tools decide from the gateway's own table (its checkout of the
    // repository, reloaded after every forge pass, every 10 s), not from the views above: wait
    // until that table grants the public Endpoint to an anonymous caller.
    eventually(
        "the gateway's own table lists the public Endpoint to an anonymous caller",
        jc,
        60,
        || async {
            let (status, body) = call(jc, get.clone(), "/api/endpoints", None, None).await;
            (status == StatusCode::OK && body.contains(PUBLIC))
                .then_some(())
                .ok_or(format!("{status} {body}"))
        },
    )
    .await;
}

/// The jc image runs no `git` (distroless): the gateway reads shako over its HTTP API, so with an
/// empty PATH it starts and the public Endpoint answers an anonymous read. On the git resolver
/// the gateway was not built ("git does not run") and every Endpoint read answered 503.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_reads_the_forge_with_no_program_on_the_path() {
    // A start that lost its port to another bind begins again in a fresh directory.
    let mut started = None;
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        realm_file(
            &root.join("realms"),
            &root.join("realm-secrets"),
            "jc",
            "00-e2e.json",
        );
        let empty = root.join("no-programs");
        std::fs::create_dir_all(&empty).unwrap();
        let envs = [("PATH", empty.to_str().unwrap())];
        if let Some(jc) = start_at(root, free_port(), &[], &envs).await {
            started = Some((dir, jc));
            break;
        }
    }
    let (_dir, jc) = started.expect("jc lost its listen port to another bind 5 times");
    open_project(&jc).await;
    let (status, body) = call(
        &jc,
        reqwest::Method::GET,
        &format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1/types"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        jc.log_has("gateway: the forge checkout read"),
        "{}",
        jc.tail()
    );
    assert!(!jc.log_has("git does not run"), "{}", jc.tail());
}

/// The MQTT brokers' CONNECT check (ADR-N-048) inside jc: `JC_GATEWAY_INTERNAL_BIND` opens the
/// gateway's internal listener, which admits a device whose kaisatsu token names its broker,
/// refuses a wrong password, a token under another client id and a caller with no broker's mesh
/// identity, and the public listener never answers the check. Before, jc dropped the gateway's
/// internal router and nothing listened on that address.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mqtt_connect_check_answers_on_the_internal_listener_alone() {
    let mut started = None;
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        realm_file(
            &root.join("realms"),
            &root.join("realm-secrets"),
            "jc",
            "00-e2e.json",
        );
        let internal = format!("127.0.0.1:{}", free_port());
        let envs = [("JC_GATEWAY_INTERNAL_BIND", internal.as_str())];
        if let Some(jc) = start_at(root, free_port(), &[], &envs).await {
            started = Some((dir, jc, internal));
            break;
        }
    }
    let (_dir, jc, internal) = started.expect("jc lost its listen port to another bind 5 times");
    open_project(&jc).await;
    put_file(
        &jc,
        &format!("projects/{PROJECT}/access/serviceaccounts/cam.yaml"),
        &format!(
            "{HEADER}\nkind: ServiceAccount\nmetadata:\n  name: cam\n  namespace: {PROJECT}\n\
             spec:\n  owner:\n    user: {PERSON}\n  purpose: \"a camera\"\n  roles:\n    \
             - role: space-writer\n      scope:\n        project: {PROJECT}\n  credentials:\n    \
             - kind: oauth-client\n      name: default\n"
        ),
    )
    .await;
    put_file(
        &jc,
        &format!("projects/{PROJECT}/mqttbrokers/cams.yaml"),
        &format!(
            "{HEADER}\nkind: MqttBroker\nmetadata:\n  name: cams\n  namespace: {PROJECT}\n\
             spec:\n  roles:\n    - name: camera\n      serviceAccounts: [cam]\n      \
             publish: [\"cams/${{clientid}}/frame\"]\n"
        ),
    )
    .await;

    // What RMQTT's auth-http posts, with the mesh identity Linkerd stamps on the broker's call.
    let identity = format!("{CAMERAS}.jc-mqtt.serviceaccount.identity.linkerd.cluster.local");
    let ask = |base: String, identity: Option<String>, username: &str, password: String| {
        let body = json!({
            "clientid": username, "username": username, "password": password,
            "ipaddr": "203.0.113.7",
        });
        async move {
            let mut req = Client::new()
                .post(format!("{base}/internal/mqtt/auth"))
                .json(&body);
            if let Some(identity) = identity {
                req = req.header("l5d-client-id", identity);
            }
            let resp = req.send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            Ok::<_, String>((status, text))
        }
    };
    let token = client_token(&jc, CAMERA, CAMERA_SECRET).await;
    let on_internal = format!("http://{internal}");
    let allowed = eventually(
        "the internal listener admits the camera once the gateway holds its broker",
        &jc,
        90,
        || {
            let asked = ask(
                on_internal.clone(),
                Some(identity.clone()),
                CAMERA,
                token.clone(),
            );
            async move {
                let (status, body) = asked.await?;
                let answer: Value =
                    serde_json::from_str(&body).map_err(|e| format!("{e}: {body}"))?;
                (status == StatusCode::OK && answer["result"] == "allow")
                    .then_some(answer)
                    .ok_or(format!("{status} {body}"))
            }
        },
    )
    .await;
    assert_eq!(allowed["superuser"], false, "{allowed}");
    assert!(allowed["expire_at"].as_i64().is_some(), "{allowed}");

    let refused = |(status, body): (StatusCode, String)| {
        let answer: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        status == StatusCode::OK && answer["result"] == "deny"
    };
    let wrong = ask(
        on_internal.clone(),
        Some(identity.clone()),
        CAMERA,
        "not-a-token".into(),
    )
    .await
    .unwrap();
    assert!(refused(wrong.clone()), "a wrong password: {wrong:?}");
    let empty = ask(
        on_internal.clone(),
        Some(identity.clone()),
        CAMERA,
        String::new(),
    )
    .await
    .unwrap();
    assert!(refused(empty.clone()), "no password: {empty:?}");
    let other = ask(
        on_internal.clone(),
        Some(identity.clone()),
        "e2e-other",
        token.clone(),
    )
    .await
    .unwrap();
    assert!(refused(other.clone()), "another client id: {other:?}");
    let unmeshed = ask(on_internal.clone(), None, CAMERA, token.clone())
        .await
        .unwrap();
    assert!(refused(unmeshed.clone()), "no mesh identity: {unmeshed:?}");

    // The public listener: no mount answers the check, with the broker's identity and a good token.
    let (status, body) = ask(
        jc.base.clone(),
        Some(identity.clone()),
        CAMERA,
        token.clone(),
    )
    .await
    .unwrap();
    let answer: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    assert!(
        answer.get("result").is_none(),
        "the public listener answered the check: {status} {body}"
    );
}

/// ADR-N-052: the gateway's anonymous OAuth server, as prod runs it, signs with a key jc
/// generates and keeps (0600) and never logs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_anonymous_oauth_server_runs_on_a_key_jc_keeps() {
    let jc = start().await;
    // The gateway answers 503 until its first load; then the document, not a 404.
    let body = eventually("the OAuth server's document", &jc, 60, || async {
        let (status, body) = call(
            &jc,
            reqwest::Method::GET,
            "/.well-known/oauth-authorization-server",
            None,
            None,
        )
        .await;
        match status {
            StatusCode::OK => Ok(body),
            StatusCode::SERVICE_UNAVAILABLE => Err(body),
            other => panic!("{other} {body}\n{}", jc.tail()),
        }
    })
    .await;
    let document: Value = serde_json::from_str(&body).unwrap();
    assert!(
        document["registration_endpoint"]
            .as_str()
            .is_some_and(|u| u.ends_with("/oauth/register")),
        "{document}"
    );
    let register = json!({
        "redirect_uris": ["http://127.0.0.1:8976/callback"],
        "client_name": "jc e2e",
    });
    let (status, body) = call(
        &jc,
        reqwest::Method::POST,
        "/oauth/register",
        None,
        Some(&register),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let client: Value = serde_json::from_str(&body).unwrap();
    assert!(
        client["client_id"].as_str().is_some_and(|c| !c.is_empty()),
        "{client}"
    );
    let (status, body) = call(
        &jc,
        reqwest::Method::GET,
        "/oauth/authorize?client_id=not-issued-by-this-server&redirect_uri=http%3A%2F%2Flocalhost%2Fcallback&response_type=code",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let key = jc.secret("gateway-oauth-key");
    assert!(key.len() >= 32);
    assert!(!jc.log_has(&key), "the OAuth key appears in the log");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(jc.state.join("gateway-oauth-key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoints_answer_inside_jc_as_on_dev() {
    let jc = start().await;
    let started = Instant::now();
    open_project(&jc).await;
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;
    let public = format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1");
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    // A write through the closed Endpoint by a writer of the project lands in Antares.
    let person = token(&jc, "e2e-reader").await;
    let writes = format!("{closed}/entities");
    let thing = entity("Thing", "one");
    let (status, body) = eventually("a writer's create lands", &jc, 60, || async {
        let (status, body) = call(&jc, post.clone(), &writes, Some(&person), Some(&thing)).await;
        (status == StatusCode::CREATED)
            .then_some((status, body.clone()))
            .ok_or(format!("{status} {body}"))
    })
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let admin = jc.secret("antares-admin-token");
    // A layout 1 space is the tenant of its own name (the Portal's `cs-things` view).
    let tenant = SPACE;
    // The broker's own admin token reads no data under the joinedcontext engine; its tenant
    // list shows the write made the space's tenant, and the reads below show the Entity.
    let resp = Client::new()
        .get(jc.url("/broker/q/tenants"))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    let landed = resp.status();
    let tenants = resp.text().await.unwrap_or_default();
    assert_eq!(landed, StatusCode::OK, "{tenants}");
    assert!(tenants.contains(&format!("\"{tenant}\"")), "{tenants}");

    // A write outside the writers' Policy (a type it does not grant) is refused.
    let other = entity("Other", "one");
    let (status, body) = call(&jc, post.clone(), &writes, Some(&person), Some(&other)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A write no Policy grants the caller is refused before the view, which would answer 405
    // with the writes it serves somebody else: 403 and no `Allow` (prod's zilina-doprava and
    // praha-mesto answered 405 to an anonymous create). An anonymous caller of the public
    // Endpoint is its `public` role, refused as the decision point refuses anyone (GW1) and as
    // the gateway's `model_judged` and quality-rule refusals answer it: 403, not 401.
    // The writer's token names the closed Endpoint, where the writers' Policy applies.
    for (who, root, token, want) in [
        ("anonymous", &public, None, StatusCode::FORBIDDEN),
        (
            "a writer",
            &closed,
            Some(person.as_str()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let one = format!("{root}/entities/urn:ngsi-ld:Thing:invalid_jc:one");
        for (method, path, body) in [
            (post.clone(), format!("{root}/entities"), Some(&thing)),
            (reqwest::Method::DELETE, one.clone(), None),
            (reqwest::Method::PATCH, format!("{one}/attrs"), Some(&thing)),
            (
                post.clone(),
                format!("{root}/entityOperations/delete"),
                None,
            ),
        ] {
            if token.is_some() && method == post && path.ends_with("/entities") {
                continue; // the writers' Policy grants this create
            }
            let mut req = Client::new()
                .request(method.clone(), jc.url(&path))
                .header("jc-reason", "smoke");
            if let Some(token) = token {
                req = req.bearer_auth(token);
            }
            if let Some(body) = body {
                req = req
                    .header("content-type", "application/json")
                    .body(body.to_string());
            }
            let resp = req.send().await.unwrap();
            let (status, allow) = (resp.status(), resp.headers().get("allow").cloned());
            let text = resp.text().await.unwrap_or_default();
            assert_eq!(status, want, "{who} {method} {path}: {text}");
            assert!(allow.is_none(), "{who} {method} {path}: Allow {allow:?}");
        }
    }

    // Anonymous: the public Endpoint answers types and an entity query, the closed one 401.
    let (status, body) = call(&jc, get.clone(), &format!("{public}/types"), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Thing"), "{body}");
    // The catalogue's explorer reads from a sandboxed page (viz beside the Portal): `Origin: null`
    // gets the same public answer with CORS for `null` and no credentials.
    let resp = Client::new()
        .get(jc.url(&format!("{public}/entities?type=Thing")))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["access-control-allow-origin"], "null");
    assert!(
        resp.headers()
            .get("access-control-allow-credentials")
            .is_none()
    );
    let body = resp.text().await.unwrap_or_default();
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{public}/entities?type=Thing"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{closed}/entities?type=Thing"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // A person holding the readers' Policy reads the closed Endpoint.
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{closed}/entities?type=Thing"),
        Some(&person),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");

    // The same person's token for another Endpoint is refused on this one.
    let elsewhere = token(&jc, "e2e-elsewhere").await;
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{closed}/entities?type=Thing"),
        Some(&elsewhere),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // The scope path, as on prod: a person's `jcctl` token (audience `jcctl-endpoint`) names one
    // Endpoint by its scope `endpoint:{slug}`, which jc's linker links to `jcctl` as the
    // deployment's admin-permissions Job does, and reads it with the person's own grants.
    let scope = format!("endpoint:{CLOSED}");
    let cli = eventually(
        "jcctl holds the closed Endpoint's scope",
        &jc,
        30,
        || async { scoped_token(&jc, "jcctl", PERSON, &scope).await },
    )
    .await;
    let read = format!("{closed}/entities?type=Thing");
    let (status, body) = call(&jc, get.clone(), &read, Some(&cli), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");
    // The same token on another Endpoint, even a public one, names a scope that is not that
    // Endpoint's: refused.
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{public}/types"),
        Some(&cli),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    // A person without a grant gets the scope, never the data: the scope names the Endpoint.
    let stranger_cli = scoped_token(&jc, "jcctl", STRANGER, &scope).await.unwrap();
    let (status, body) = call(&jc, get.clone(), &read, Some(&stranger_cli), None).await;
    assert!(
        matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
        "a person without a grant read the closed Endpoint: {status} {body}"
    );
    assert!(!body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");
    // jc links every Endpoint's scope to `portal-api` too (prod does not, T-6063): the Portal's
    // own service account asking that scope and the hub audience is its own
    // `service-account-portal-api` user, whom no Policy grants, so it reads nothing and the link
    // opens no Endpoint beyond the person exchange below.
    let portal = Client::new()
        .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", "portal-api"),
            ("client_secret", jc.secret("portal-client-secret").as_str()),
            ("scope", format!("{scope} gateway-hub").as_str()),
        ])
        .send()
        .await
        .unwrap();
    let status = portal.status();
    let body: Value = portal.json().await.unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "portal-api's own token: {body}");
    let own = body["access_token"].as_str().unwrap();
    let claims = claims_of(own);
    assert!(
        claims["scope"]
            .as_str()
            .unwrap_or_default()
            .contains(&scope),
        "{claims}"
    );
    assert!(claims["aud"].to_string().contains("mcp-hub"), "{claims}");
    let (status, body) = call(&jc, get.clone(), &read, Some(own), None).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "portal-api's own token read the closed Endpoint: {status} {body}"
    );
    assert!(!body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");

    // `jc_data_query` through the Portal's MCP: the Portal exchanges the person's token for
    // `endpoint:{slug}` (the scope jc's kaisatsu links to portal-api, as the deployment's
    // admin-permissions Job does on dev) and reads as them through the gateway.
    let data_query = |id: u32| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "jc_data_query",
                "arguments": { "project": PROJECT, "endpoint": "closed", "type": "Thing" }
            }
        })
    };
    let member = token(&jc, "e2e-mcp").await;
    let body = eventually(
        "a member's jc_data_query reads the closed Endpoint",
        &jc,
        60,
        || async {
            let (status, body) = call(
                &jc,
                post.clone(),
                "/api/v1/mcp",
                Some(&member),
                Some(&data_query(3)),
            )
            .await;
            (status == StatusCode::OK && body.contains("urn:ngsi-ld:Thing:invalid_jc:one"))
                .then_some(body.clone())
                .ok_or(format!("{status} {body}"))
        },
    )
    .await;
    assert!(!body.contains("not open to data reads"), "{body}");
    // A person with no grant on the Endpoint gets a token exchanged the same way, and the
    // gateway's Policy still refuses them: the scope names the Endpoint, never a right to it.
    let stranger = token_of(&jc, "e2e-mcp", STRANGER).await;
    jc.mark("the stranger's jc_data_query sent");
    let (status, body) = call(
        &jc,
        post.clone(),
        "/api/v1/mcp",
        Some(&stranger),
        Some(&data_query(4)),
    )
    .await;
    jc.mark(&format!(
        "the stranger's jc_data_query answered {status}: {body}"
    ));
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains("urn:ngsi-ld:Thing:invalid_jc:one"),
        "a person without a grant read the closed Endpoint: {body}"
    );
    // The Portal logs the refusal before it answers; the line may still be in the pipe.
    jc.wait_log(
        "the stranger's read reaches the gateway's refusal",
        "the gateway refused a shared Endpoint read",
    )
    .await;

    // A caller who claims the gateway's mesh identity or a subject on the public broker mount
    // is nobody but itself: the claim reaches no rule, and the admin token is still asked for.
    let resp = Client::new()
        .get(jc.url("/broker/ngsi-ld/v1/entities?type=Thing"))
        .header("NGSILD-Tenant", tenant)
        .header("l5d-client-id", "context-gateway.jc")
        .header("x-jc-client", "context-gateway")
        .header("x-jc-subject", PERSON)
        .header("x-jc-groups", format!("{PROJECT}-readers"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert!(
        !body.contains("urn:ngsi-ld:Thing:invalid_jc:one"),
        "the broker believed a claimed identity: {status} {body}"
    );

    // MCP through the hub: the tools list and one query of the public Endpoint, anonymously.
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let (status, body) = call(&jc, post.clone(), "/api/mcp", None, Some(&list)).await;
    jc.mark(&format!("the hub's tools/list answered {status}"));
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("no tools: {body}"))
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"query_entities"), "{names:?}");
    let query = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "query_entities",
            "arguments": { "endpoint": PUBLIC, "type": ["Thing"] }
        }
    });
    let (status, body) = call(&jc, post.clone(), "/api/mcp", None, Some(&query)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");

    // The gateway's revocation poll (every 10 s) is admitted by the Portal's internal listener:
    // its token names the issuer the Portal checks and the client the route belongs to. Checked
    // below, once the log has passed a poll that landed.
    tokio::time::sleep(Duration::from_secs(25).saturating_sub(started.elapsed())).await;

    // A person the Portal disables is refused by the gateway within one revocation poll (10 s)
    // with the token they still hold: the gateway reads the Portal's internal listener at the
    // free port it took, which jc hands on.
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{closed}/entities?type=Thing"),
        Some(&person),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "before the revocation: {body}");
    let mut admin = Browser::new();
    sign_in(
        &|p| jc.url(p),
        &mut admin,
        "admin",
        &jc.secret("first-admin-password"),
    )
    .await;
    let listed = admin.get(&jc.url("/api/v1/organization/people")).await;
    let id = listed.json()["items"]
        .as_array()
        .and_then(|items| items.iter().find(|p| p["email"] == "e2e.person@jc.invalid"))
        .and_then(|p| p["id"].as_str())
        .unwrap_or_else(|| panic!("{PERSON} in the Portal's people: {}", listed.text))
        .to_owned();
    let disabled = admin
        .send(
            reqwest::Method::POST,
            &jc.url(&format!("/api/v1/organization/people/{id}/disable")),
            None,
            true,
        )
        .await;
    assert!(
        disabled.status.is_success(),
        "disable {PERSON}: {} {}",
        disabled.status,
        disabled.text
    );
    let revoked = Instant::now();
    loop {
        let (status, body) = call(
            &jc,
            get.clone(),
            &format!("{closed}/entities?type=Thing"),
            Some(&person),
            None,
        )
        .await;
        if status == StatusCode::UNAUTHORIZED {
            break;
        }
        assert!(
            revoked.elapsed() < Duration::from_secs(25),
            "the gateway still answers the revoked token {status}: {body}\n{}",
            jc.tail()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    println!("refused {:?} after the revocation", revoked.elapsed());
    // The refusal follows a poll the Portal answered; once its line is in the test's copy of the
    // log, every poll before it is too, and none of them was refused.
    jc.wait_log(
        "the gateway's refusal of the revoked token",
        "token issued before its subject was revoked",
    )
    .await;
    assert!(
        !jc.log_has("the Portal answered 401"),
        "the Portal refused the gateway:\n{}",
        jc.tail()
    );
}

/// Brief 119: the assistant counts Endpoints on jc's own listener from loopback (`reach`), not
/// through the edge. An anonymous read of the closed Endpoint gets the same status and body
/// there as from the edge (a trusted proxy's address with its mesh identity and forwarding
/// headers). A loopback read that forges the edge's identity, a client address and the gateway's
/// client and subject headers gets that answer too: neither the gateway nor the broker took any
/// of them, or the writer's entity would show.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_loopback_self_read_is_decided_as_a_read_through_the_edge() {
    const EDGE: &str = "apisix.jc.serviceaccount.identity.linkerd.cluster.local";
    // The edge's address is 127.0.0.2, a loopback address the test's own reads never come from.
    let jc = start_with_env(
        false,
        &[
            ("JCBIN_TRUSTED_PROXIES", "127.0.0.2/32"),
            ("JCBIN_TRUSTED_PROXY_IDENTITIES", EDGE),
        ],
    )
    .await;
    open_project(&jc).await;
    let person = token(&jc, "e2e-reader").await;
    let writes = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1/entities");
    let thing = entity("Thing", "one");
    eventually("a writer's create lands", &jc, 60, || async {
        let (status, body) = call(
            &jc,
            reqwest::Method::POST,
            &writes,
            Some(&person),
            Some(&thing),
        )
        .await;
        (status == StatusCode::CREATED)
            .then_some(())
            .ok_or(format!("{status} {body}"))
    })
    .await;
    let host = jc.base.trim_start_matches("http://").to_owned();
    let path = format!("{writes}?type=Thing&count=true&limit=0");
    // Dialed at 127.0.0.1 by number, so the edge's client can bind 127.0.0.2 (`localhost` may
    // name ::1 first); the request names jc's host either way.
    let read = |client: Client, headers: Vec<(&'static str, String)>| {
        let url = jc.url(&path).replacen("//localhost:", "//127.0.0.1:", 1);
        let host = host.clone();
        async move {
            let mut req = client.get(url).header("host", host);
            for (name, value) in headers {
                req = req.header(name, value);
            }
            let resp = req.send().await.unwrap();
            let status = resp.status();
            (status, resp.text().await.unwrap_or_default())
        }
    };
    let edge = Client::builder()
        .local_address(std::net::IpAddr::from([127, 0, 0, 2]))
        .build()
        .unwrap();
    let through_edge = read(
        edge,
        vec![
            ("l5d-client-id", EDGE.to_owned()),
            ("x-forwarded-for", "203.0.113.7".to_owned()),
            ("x-forwarded-proto", "http".to_owned()),
            ("x-forwarded-host", host.clone()),
        ],
    )
    .await;
    let on_loopback = read(Client::new(), Vec::new()).await;
    let forged = read(
        Client::new(),
        vec![
            ("l5d-client-id", EDGE.to_owned()),
            ("x-forwarded-for", "203.0.113.7".to_owned()),
            ("x-jc-client", "context-gateway".to_owned()),
            ("x-jc-subject", PERSON.to_owned()),
            ("x-jc-groups", format!("{PROJECT}-readers")),
        ],
    )
    .await;
    jc.mark(&format!(
        "edge {} loopback {} forged {}",
        through_edge.0, on_loopback.0, forged.0
    ));
    assert!(
        through_edge.0.is_client_error(),
        "an anonymous read of the closed Endpoint: {through_edge:?}"
    );
    assert_eq!(on_loopback, through_edge, "loopback and edge decide alike");
    assert_eq!(forged, through_edge, "a forged header changed the decision");
    assert!(!forged.1.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{forged:?}");
}

const MODELLED: &str = "modelled";
const MODELLED_SLUG: &str = "e2emodelledendpointaaaaaaa";

/// A space with a data model and its public Endpoint serving OGC API Features and OData.
fn modelled_manifests() -> Vec<(String, String)> {
    let space = |rel: &str| format!("projects/{PROJECT}/spaces/{MODELLED}/{rel}");
    let linkml = "\
id: https://jc.invalid/models/things
name: things
default_prefix: things
default_range: string
prefixes:
  things: https://jc.invalid/models/things/
  linkml: https://w3id.org/linkml/
imports:
  - linkml:types
  - ngsi-ld-core
classes:
  Thing:
    is_a: Entity
    slots: [name, location]
slots:
  name:
    range: string
    annotations:
      ngsi_ld_kind: Property
";
    vec![
        (
            format!("projects/{PROJECT}/project.yaml"),
            format!(
                "{HEADER}\nkind: Project\nmetadata:\n  name: {PROJECT}\n  namespace: org\nspec:\n  \
                 organizationRef: jc\n"
            ),
        ),
        (
            space("space.yaml"),
            format!(
                "{HEADER}\nkind: ContextSpace\nmetadata:\n  name: {MODELLED}\n  namespace: {PROJECT}\n\
                 spec:\n  isSandbox: false\n  urnSegment: {MODELLED}\n  \
                 dataModelRef: {{ kind: DataModel, name: things }}\n"
            ),
        ),
        (
            space("datamodels/things.yaml"),
            format!(
                "{HEADER}\nkind: DataModel\nmetadata:\n  name: things\n  namespace: {PROJECT}\n\
                 spec:\n  contextSpaceRef: {MODELLED}\n  linkml: ./things.linkml.yaml\n  \
                 version: 1.0.0\n  lifecycle: draft\n  classes: [Thing]\n"
            ),
        ),
        (space("datamodels/things.linkml.yaml"), linkml.to_owned()),
        (
            space("endpoints/modelled.yaml"),
            format!(
                "{HEADER}\nkind: Endpoint\nmetadata:\n  name: modelled\n  namespace: {PROJECT}\n\
                 spec:\n  contextSpaceRef: {MODELLED}\n  slug: {MODELLED_SLUG}\n  audience: public\n  \
                 enabledRepresentations: [ngsi-ld, ogc-features, odata]\n"
            ),
        ),
        (
            space("policies/public-read.yaml"),
            format!(
                "{HEADER}\nkind: Policy\nmetadata:\n  name: public-read\n  namespace: {PROJECT}\n\
                 spec:\n  contextSpaceRef: {MODELLED}\n  assigner: did:web:jc.invalid\n  \
                 assignee: {{ kind: role, id: public }}\n  operations: [retrieveOps]\n  \
                 information:\n    - entities:\n        - type: Thing\n      \
                 propertyNames: [name, location]\n"
            ),
        ),
    ]
}

/// jc's Model Tools compiles the space's model, the reconciler writes the Tenant's schema to
/// Antares, and the Endpoint then answers OGC API Features and OData through the public mount
/// (without a schema both answer 404 "no such resource").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_modelled_endpoint_answers_ogc_and_odata_inside_jc() {
    let jc = start().await;
    for (path, content) in modelled_manifests() {
        put_file(&jc, &path, &content).await;
    }
    let get = reqwest::Method::GET;
    let base = format!("/api/endpoint/{MODELLED_SLUG}");
    let collections = eventually("the OGC collections answer", &jc, 120, || async {
        let (status, body) = call(
            &jc,
            get.clone(),
            &format!("{base}/ogc/features/collections"),
            None,
            None,
        )
        .await;
        (status == StatusCode::OK)
            .then(|| body.clone())
            .ok_or(format!("{status} {body}"))
    })
    .await;
    let collections: Value = serde_json::from_str(&collections).unwrap();
    let ids: Vec<&str> = collections["collections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(ids.contains(&"Thing"), "{collections}");
    // The files prod's broker writes (features `parquet` and `tabular`): items as CSV and
    // Parquet, /collections as a ZIP. A build without them refuses `f` as an unknown parameter.
    let ogc = format!("{base}/ogc/features/collections");
    for (path, media, magic) in [
        (format!("{ogc}/Thing/items?f=csv"), "text/csv", &b"id"[..]),
        (
            format!("{ogc}/Thing/items?f=parquet"),
            "application/vnd.apache.parquet",
            &b"PAR1"[..],
        ),
        (format!("{ogc}?f=zip"), "application/zip", &b"PK"[..]),
    ] {
        let resp = Client::new().get(jc.url(&path)).send().await.unwrap();
        let status = resp.status();
        let kind = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = resp.bytes().await.unwrap();
        let start = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
        assert_eq!(status, StatusCode::OK, "{path}: {start}");
        assert!(kind.starts_with(media), "{path}: {kind}");
        assert!(body.starts_with(magic), "{path}: {start}");
    }
    // OData reads the same schema through its own surface: it answers once that has it too.
    let metadata = eventually("the OData $metadata answers", &jc, 60, || async {
        let (status, body) = call(
            &jc,
            get.clone(),
            &format!("{base}/odata/v4/$metadata"),
            None,
            None,
        )
        .await;
        (status == StatusCode::OK)
            .then(|| body.clone())
            .ok_or(format!("{status} {body}"))
    })
    .await;
    assert!(metadata.contains("EntitySet"), "{metadata}");
    assert!(metadata.contains("Thing"), "{metadata}");
}

/// [`manifests`] with the space modelled (a Pipeline's output type must be a class of the
/// space's model, or the Endpoint's view shows it whole and the writer refuses the view), plus
/// what a pipeline of the project needs: its `pipelines` account, the account's write Policy on
/// the space and a clock DataSource.
fn pipeline_manifests() -> Vec<(String, String)> {
    let at = |rel: &str| format!("projects/{PROJECT}/{rel}");
    let model = |rel: &str| {
        modelled_manifests()
            .into_iter()
            .find(|(path, _)| path.ends_with(rel))
            .map(|(_, text)| text.replace(MODELLED, SPACE))
            .unwrap()
    };
    let mut out: Vec<(String, String)> = manifests()
        .into_iter()
        .map(|(path, text)| {
            if path.ends_with(&format!("{SPACE}/space.yaml")) {
                let text = format!("{text}  dataModelRef: {{ kind: DataModel, name: things }}\n");
                (path, text)
            } else {
                (path, text)
            }
        })
        .collect();
    out.push((
        at(&format!("spaces/{SPACE}/datamodels/things.yaml")),
        model("datamodels/things.yaml"),
    ));
    out.push((
        at(&format!("spaces/{SPACE}/datamodels/things.linkml.yaml")),
        model("datamodels/things.linkml.yaml"),
    ));
    out.extend([
        (
            at("access/serviceaccounts/pipelines.yaml"),
            format!(
                "{HEADER}\nkind: ServiceAccount\nmetadata:\n  name: pipelines\n  namespace: {PROJECT}\n\
                 spec:\n  owner: {{ user: e2e-owner }}\n  purpose: The project's streams write Things\n  \
                 roles:\n    - role: steward\n      scope: {{ contextSpace: {SPACE} }}\n  credentials:\n    - kind: oauth-client\n      name: main\n"
            ),
        ),
        (
            at(&format!("spaces/{SPACE}/policies/pipelines-write.yaml")),
            format!(
                "{HEADER}\nkind: Policy\nmetadata:\n  name: pipelines-write\n  namespace: {PROJECT}\n\
                 spec:\n  contextSpaceRef: {SPACE}\n  assigner: did:web:jc.invalid\n  \
                 assignee: {{ kind: serviceAccount, id: pipelines }}\n  \
                 operations: [upsertBatch, createBatch, queryBatch]\n  information:\n    \
                 - entities:\n        - type: Thing\n      propertyNames: [name]\n"
            ),
        ),
        (
            at("datasources/clock.yaml"),
            format!(
                "{HEADER}\nkind: DataSource\nmetadata:\n  name: clock\n  namespace: {PROJECT}\n\
                 spec:\n  type: generate\n  input:\n    interval: 2s\n    \
                 mapping: 'root = {{\"n\": 1}}'\n"
            ),
        ),
    ]);
    out
}

/// The Pipeline a person proposes in the Portal: one Thing per tick of the clock, written
/// through the closed Endpoint as the project's `pipelines` client.
fn pipeline() -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "feed", "namespace": PROJECT },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "clock" } },
            "compute": {
                "kind": "bloblang",
                "bloblang": "root = {\"id\": \"urn:ngsi-ld:Thing:\" + env(\"JC_ORG_ID\") + \":fed\", \"type\": \"Thing\", \"name\": {\"type\": \"Property\", \"value\": \"fed\"}}"
            },
            "targetEndpoint": format!("urn:ngsi-ld:Endpoint:jc.invalid:{SPACE}:closed"),
            "output": { "type": "Thing", "mode": "upsert" }
        }
    })
}

/// A client-credentials token of `client` in realm `jc`.
async fn client_token(jc: &Jc, client: &str, secret: &str) -> String {
    let resp = Client::new()
        .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client),
            ("client_secret", secret),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{client}'s token: {body}");
    body["access_token"].as_str().unwrap().to_owned()
}

/// A kaisatsu admin token of the Portal's own people client.
async fn kaisatsu_admin_token(jc: &Jc) -> String {
    let resp = Client::new()
        .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
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
    let body: Value = resp.json().await.unwrap();
    body["access_token"]
        .as_str()
        .expect("admin token")
        .to_owned()
}

/// A second administrator (PF-58: a proposer never approves their own change), made through
/// kaisatsu's admin API; answers the username.
async fn second_admin(jc: &Jc, password: &str) -> String {
    let token = kaisatsu_admin_token(jc).await;
    let admin = |m: reqwest::Method, path: &str| {
        Client::new()
            .request(m, jc.url(&format!("/id/admin/realms/jc{path}")))
            .bearer_auth(&token)
    };
    let username = "e2e.approver@jc.invalid";
    let made = admin(reqwest::Method::POST, "/users")
        .json(&json!({
            "username": username, "email": username, "firstName": "Second", "lastName": "Admin",
            "enabled": true, "emailVerified": true,
            "credentials": [{"type": "password", "value": password, "temporary": false}]
        }))
        .send()
        .await
        .unwrap();
    assert!(
        made.status().is_success(),
        "create approver: {}",
        made.status()
    );
    let found: Value = admin(
        reqwest::Method::GET,
        &format!("/users?username={username}&exact=true"),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let id = found[0]["id"].as_str().expect("approver id").to_owned();
    let reset = admin(reqwest::Method::PUT, &format!("/users/{id}/reset-password"))
        .json(&json!({"type": "password", "value": password, "temporary": false}))
        .send()
        .await
        .unwrap();
    assert!(reset.status().is_success(), "password: {}", reset.status());
    let groups: Value = admin(
        reqwest::Method::GET,
        "/groups?search=platform-admins&exact=true",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let group = groups[0]["id"].as_str().expect("bootstrap group");
    let joined = admin(reqwest::Method::PUT, &format!("/users/{id}/groups/{group}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(joined.status().is_success(), "join: {}", joined.status());
    username.to_owned()
}

/// Writes the project and waits until the closed Endpoint answers and the gateway admits its
/// `pipelines` account there.
async fn the_feed_project(jc: &Jc) {
    eventually(
        "shako holds the organization repository",
        jc,
        120,
        || async {
            let resp = Client::new()
                .get(jc.url(&format!("/git/api/v1/repos/{FORGE_OWNER}/{FORGE_REPO}")))
                .header(
                    "authorization",
                    format!("token {}", jc.secret("shako-token")),
                )
                .send()
                .await
                .map_err(|e| e.to_string())?;
            resp.status()
                .is_success()
                .then_some(())
                .ok_or(resp.status().to_string())
        },
    )
    .await;
    for (path, content) in pipeline_manifests() {
        put_file(jc, &path, &content).await;
    }
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    let reader = token(jc, "e2e-reader").await;
    eventually("the closed Endpoint answers", jc, 120, || async {
        let (status, body) = call(
            jc,
            reqwest::Method::GET,
            &format!("{closed}/types"),
            Some(&reader),
            None,
        )
        .await;
        (status == StatusCode::OK)
            .then_some(())
            .ok_or(format!("{status} {body}"))
    })
    .await;
    // The stream's writes pass the gateway's own table (its checkout, reloaded after each forge
    // pass, every 10 s), not only the views above: a run that starts before that table holds the
    // `pipelines` ServiceAccount is refused at the door and ends. Wait until the account's token
    // passes the door with the read its Policy grants (queryBatch), which writes nothing.
    let pipelines = client_token(jc, &format!("{PROJECT}-pipelines"), PIPELINES_SECRET).await;
    eventually(
        "the gateway's own table admits the pipelines account at the closed Endpoint",
        jc,
        60,
        || async {
            let (status, body) = call(
                jc,
                reqwest::Method::POST,
                &format!("{closed}/entityOperations/query"),
                Some(&pipelines),
                Some(&json!({ "type": "Query", "entities": [{ "type": "Thing" }] })),
            )
            .await;
            (status == StatusCode::OK)
                .then_some(())
                .ok_or(format!("{status} {body}"))
        },
    )
    .await;
}

/// [`the_feed_project`], then one administrator proposes [`pipeline`] and a second approves it;
/// waits until its Thing landed through the closed Endpoint: answers the stream as the runner held
/// it and the entity.
async fn the_feed_lands(jc: &Jc) -> (Option<String>, String) {
    the_feed_project(jc).await;
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    let reader = token(jc, "e2e-reader").await;
    // The administrator proposes the Pipeline, a second administrator approves it.
    let mut proposer = Browser::new();
    sign_in(
        &|p| jc.url(p),
        &mut proposer,
        "admin",
        &jc.secret("first-admin-password"),
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let source = proposer
            .get(&jc.url(&format!("/api/v1/projects/{PROJECT}/datasources/clock")))
            .await;
        if source.status == StatusCode::OK {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the DataSource in the Portal: {} {}\n{}",
            source.status,
            source.text,
            jc.tail()
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let proposed = proposer
        .send(
            reqwest::Method::POST,
            &jc.url(&format!("/api/v1/projects/{PROJECT}/pipelines")),
            Some(common::Body::Json(&pipeline())),
            true,
        )
        .await;
    assert!(
        proposed.status.is_success(),
        "propose: {} {}",
        proposed.status,
        proposed.text
    );
    let change = proposed.json()["metadata"]["name"]
        .as_str()
        .expect("the Change's name")
        .to_owned();
    let approver_pw = "Second-admin-pw-0123456789";
    let approver_name = second_admin(jc, approver_pw).await;
    let mut approver = Browser::new();
    sign_in(&|p| jc.url(p), &mut approver, &approver_name, approver_pw).await;
    let approved = approver
        .send(
            reqwest::Method::POST,
            &jc.url(&format!(
                "/api/v1/projects/{PROJECT}/changes/{change}/approve"
            )),
            Some(common::Body::Json(&json!({"confirm": "feed"}))),
            true,
        )
        .await;
    assert!(
        approved.status.is_success(),
        "approve: {} {}",
        approved.status,
        approved.text
    );

    // The stream writes the Thing; the reader finds it through the same Endpoint.
    let started = Instant::now();
    let deadline = started + Duration::from_secs(180);
    let mut rendered = None;
    let mut landed = None;
    let entity = loop {
        // Both until both are seen: a run loads, writes and ends between two reads of one round
        // (117 ms on a loaded host), and the Portal loads the run again at its next tick.
        if let (Some(_), Some(body)) = (&rendered, &landed) {
            break String::clone(body);
        }
        if rendered.is_none() {
            let held = Client::new()
                .get(jc.url(&format!("/streams/streams/{PROJECT}.feed")))
                .bearer_auth(jc.secret("ekiben-token"))
                .send()
                .await
                .unwrap();
            if held.status().is_success() {
                rendered = Some(held.text().await.unwrap_or_default());
            }
        }
        let (status, body) = call(
            jc,
            reqwest::Method::GET,
            &format!("{closed}/entities?type=Thing&limit=5"),
            Some(&reader),
            None,
        )
        .await;
        if landed.is_none() && status == StatusCode::OK && body.contains("Thing:") {
            landed = Some(body.clone());
        }
        if Instant::now() > deadline && landed.is_some() {
            panic!(
                "the Thing landed, and the runner never showed the stream within 180 s:\n{}",
                jc.tail()
            );
        }
        if Instant::now() > deadline {
            let pipeline = proposer
                .get(&jc.url(&format!("/api/v1/projects/{PROJECT}/pipelines/feed")))
                .await;
            let endpoint = proposer
                .get(&jc.url(&format!("/api/v1/projects/{PROJECT}/endpoints/closed")))
                .await;
            println!("the Endpoint: {}", endpoint.text);
            // What the stream's own write answers, for the reader of a failure.
            let tok = client_token(jc, &format!("{PROJECT}-pipelines"), PIPELINES_SECRET).await;
            let (wstatus, wbody) = call(
                jc,
                reqwest::Method::POST,
                &format!("{closed}/entityOperations/upsert?options=update"),
                Some(&tok),
                Some(&json!([{"id": "urn:ngsi-ld:Thing:invalid_jc:probe", "type": "Thing", "name": {"type": "Property", "value": "p"}}])),
            )
            .await;
            let view = Client::new()
                .get(jc.url(&format!("/broker/q/views/{CLOSED}")))
                .bearer_auth(jc.secret("antares-admin-token"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap_or_default();
            panic!(
                "the pipeline's Thing is not in Antares: {status} {body}\nthe stream's write by hand: {wstatus} {wbody}\nthe view: {view}\nthe Pipeline: {}\nthe stream: {rendered:?}\n{}",
                pipeline.text,
                jc.tail()
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    println!("landed after {:?}: {entity}", started.elapsed());
    (rendered, entity)
}

/// A Pipeline proposed and approved in the Portal runs on ekiben inside jc and its Entities land
/// in Antares: the stream reads `JC_GATEWAY_URL`, `JC_TOKEN_URL` and `JC_CLIENT_SECRET_E2E`
/// from jc's mounts and its client secrets directory (the realm file's own secret file), reaches
/// jc's own listener only at the gateway door and the token endpoint, and names no other
/// project's secret. No secret value shows in the runner's API or jc's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipeline_approved_in_the_portal_writes_through_ekiben_inside_jc() {
    let jc = start_with(true).await;
    let (rendered, entity) = the_feed_lands(&jc).await;
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    let reader = token(&jc, "e2e-reader").await;
    assert!(entity.contains("fed"), "{entity}");
    let counted = Client::new()
        .get(jc.url(&format!("{closed}/entities?type=Thing&count=true&limit=0")))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap();
    let total = counted
        .headers()
        .get("ngsild-results-count")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    println!(
        "count of Thing through the closed Endpoint: {} {total}",
        counted.status()
    );
    assert_eq!(total, "1", "one Thing, upserted on every tick");

    // The runner held the stream as the Portal rendered it: the project's own client, no other
    // project's, and the secret only as ekiben's redaction.
    let stream = rendered.expect("the runner held the stream while it ran");
    assert!(
        stream.contains(&format!("\"client_key\":\"{PROJECT}-pipelines\"")),
        "{stream}"
    );
    assert!(
        stream.contains("\"client_secret\":\"REDACTED\""),
        "{stream}"
    );
    assert!(!stream.contains("other-pipelines"), "{stream}");
    for secret in [PIPELINES_SECRET, OTHER_PIPELINES_SECRET] {
        assert!(!stream.contains(secret), "the runner's API serves a secret");
        assert!(!jc.log_has(secret), "jc's log holds a secret");
    }
    assert!(
        jc.log_has("ekiben runner variables from jc"),
        "{}",
        jc.tail()
    );

    // ekiben names the stream's calls to jc's listener, and the broker's line for a write
    // carries that id (the logs suite's rid.flow.ekiben-antares). The stream's report sinks to
    // the Portal name theirs too, so any of the named ids will do.
    eventually("the broker logs a stream's id", &jc, 60, || async {
        let log = jc.log.lock().unwrap();
        let ids: Vec<&str> = log
            .iter()
            .filter(|l| l.contains("http_client: jc's listener answered"))
            .filter_map(|l| l.split("request_id=").nth(1)?.split_whitespace().next())
            .collect();
        ids.iter()
            .any(|id| {
                let span = format!("request{{module=\"antares\" request_id={id}}}");
                log.iter().any(|l| l.contains(&span))
            })
            .then_some(())
            .ok_or(format!("named ids {ids:?}, none on a broker line"))
    })
    .await;
}

/// jc-pipeline-audience (prod's od-* 401s): with `JC_PORTAL_PIPELINE_IDENTITY=pipeline-secret` a
/// stream signs in as its own Pipeline's client `{project}-pl-{pipeline}` by that client's secret
/// (jc's client secrets dir hands it to ekiben), never as the project's `{project}-pipelines`. Its
/// writes land through the Endpoint it writes (A); the same token is refused at another Endpoint
/// (B) by the gateway's unchanged binding check; its `aud` is A and the Portal's internal listener
/// only, a token far under the edge's 8 KB.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_signs_in_as_its_pipelines_own_client_bound_to_its_endpoint_alone() {
    let jc = start_with_env(true, &[("JC_PORTAL_PIPELINE_IDENTITY", "pipeline-secret")]).await;
    the_feed_project(&jc).await;
    // The Pipeline is in the repository already, as an imported one is: the gateway derives its
    // account `pl-feed` and Policies from there (PL-19). A run before the gateway's table holds
    // them is refused 403 and waits out its backoff, so the test runs it once the table admits
    // the pipeline's own token.
    put_file(
        &jc,
        &format!("projects/{PROJECT}/pipelines/feed/pipeline.yaml"),
        &serde_json::to_string_pretty(&pipeline()).unwrap(),
    )
    .await;
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    let own = client_token(&jc, FEED_CLIENT, FEED_SECRET).await;
    eventually(
        "the gateway's own table admits the pipeline's own account at the closed Endpoint",
        &jc,
        90,
        || async {
            let (status, body) = call(
                &jc,
                reqwest::Method::POST,
                &format!("{closed}/entityOperations/query"),
                Some(&own),
                Some(&json!({ "type": "Query", "entities": [{ "type": "Thing" }] })),
            )
            .await;
            (status == StatusCode::OK)
                .then_some(())
                .ok_or(format!("{status} {body}"))
        },
    )
    .await;
    let mut admin = Browser::new();
    sign_in(
        &|p| jc.url(p),
        &mut admin,
        "admin",
        &jc.secret("first-admin-password"),
    )
    .await;
    let reader = token(&jc, "e2e-reader").await;
    let deadline = Instant::now() + Duration::from_secs(180);
    let (mut rendered, mut landed) = (None, false);
    let mut asked = Instant::now() - Duration::from_secs(60);
    while rendered.is_none() || !landed {
        assert!(
            Instant::now() < deadline,
            "the stream ran as its own client: {}, its Thing landed: {landed}\n{}",
            rendered.is_some(),
            jc.tail()
        );
        // Run now, again while nothing landed (409 while a run is in the runner).
        if !landed && asked.elapsed() > Duration::from_secs(20) {
            let run = admin
                .send(
                    reqwest::Method::POST,
                    &jc.url(&format!("/api/v1/projects/{PROJECT}/pipelines/feed/runs")),
                    None,
                    true,
                )
                .await;
            println!("run now: {} {}", run.status, run.text);
            asked = Instant::now();
        }
        if rendered.is_none() {
            let held = Client::new()
                .get(jc.url(&format!("/streams/streams/{PROJECT}.feed")))
                .bearer_auth(jc.secret("ekiben-token"))
                .send()
                .await
                .unwrap();
            if held.status().is_success() {
                rendered = Some(held.text().await.unwrap_or_default());
            }
        }
        let (status, body) = call(
            &jc,
            reqwest::Method::GET,
            &format!("{closed}/entities?type=Thing&limit=5"),
            Some(&reader),
            None,
        )
        .await;
        landed |= status == StatusCode::OK && body.contains("Thing:invalid_jc:fed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The runner held the stream as the pipeline's own client, its secret only as a variable.
    let stream = rendered.expect("the runner held the stream while it ran");
    assert!(
        stream.contains(&format!("\"client_key\":\"{FEED_CLIENT}\"")),
        "{stream}"
    );
    assert!(
        !stream.contains(&format!("{PROJECT}-pipelines")) && !stream.contains("other-pipelines"),
        "a stream presents no project-wide client: {stream}"
    );
    for secret in [FEED_SECRET, PIPELINES_SECRET, OTHER_PIPELINES_SECRET] {
        assert!(!stream.contains(secret), "the runner's API serves a secret");
        assert!(!jc.log_has(secret), "jc's log holds a secret");
    }
    assert!(
        jc.log_has("JC_PIPELINE_SECRET_E2E_PL_FEED"),
        "ekiben holds the pipeline's own secret variable:\n{}",
        jc.tail()
    );

    // The token the stream gets: bound to A and the internal listener, nothing else, and small.
    let mut aud: Vec<String> = match &claims_of(&own)["aud"] {
        Value::String(one) => vec![one.clone()],
        Value::Array(many) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        other => panic!("aud: {other}"),
    };
    aud.sort();
    assert_eq!(aud, [CLOSED, "portal-internal"]);
    assert!(own.len() < 8 * 1024 / 4, "{} bytes", own.len());
    let thing = json!([{"id": "urn:ngsi-ld:Thing:invalid_jc:ab", "type": "Thing", "name": {"type": "Property", "value": "ab"}}]);
    let (status, body) = call(
        &jc,
        reqwest::Method::POST,
        &format!("/api/endpoint/{CLOSED}/ngsi-ld/v1/entityOperations/upsert?options=update"),
        Some(&own),
        Some(&thing),
    )
    .await;
    assert!(status.is_success(), "A: {status} {body}");
    let (status, body) = call(
        &jc,
        reqwest::Method::POST,
        &format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1/entityOperations/upsert?options=update"),
        Some(&own),
        Some(&thing),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "B: {body}");
}

/// A redirect only the deployment's export holds: jc's own realm file never writes it, so it
/// stays only while jc leaves the imported `portal-api` alone.
const IMPORTED_CALLBACK: &str = "https://portal.dev.invalid/api/v1/auth/callback";

/// Writes `value` as a 0600 file, as jc's state directory keeps a secret.
fn write_secret(path: &Path, value: &str) {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(value.as_bytes()).unwrap();
}

/// `realm` as a Keycloak export of the deployment brings it into the import dir (file `file`):
/// the shipped Portal realm under that name for the Portal at `base`, the gateway's client, a
/// redirect of its own ([`IMPORTED_CALLBACK`]), and its client secrets as `$(file:...)` under
/// `secrets`. The same secrets go into jc's `state` directory, where jc reads its clients'
/// secrets: the cutover's way of keeping the realm's own.
fn export(realms: &Path, secrets: &Path, state: &Path, realm: &str, base: &str, file: &str) {
    for dir in [realms, secrets, state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let placeholder = |name: &str| format!("$(file:{})", secrets.join(name).display());
    for name in [
        "portal-client-secret",
        "portal-admin-client-secret",
        "gateway-client-secret",
    ] {
        if !secrets.join(name).exists() {
            let value = format!(
                "{name}-{:032x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            write_secret(&secrets.join(name), &value);
            write_secret(&state.join(name), &value);
        }
    }
    let text = include_str!("../../../crates/kaisatsu/import/realms/jc-portal.json")
        .replace("https://portal.jc.localhost", base)
        .replace(
            "$(CLIENT_SECRET_PORTAL_API)",
            &placeholder("portal-client-secret"),
        )
        .replace(
            "$(CLIENT_SECRET_PORTAL_RECONCILER)",
            &placeholder("portal-admin-client-secret"),
        );
    let mut value: Value = serde_json::from_str(&text).unwrap();
    value["id"] = json!(realm);
    value["realm"] = json!(realm);
    let clients = value["clients"].as_array_mut().unwrap();
    let portal = clients
        .iter_mut()
        .find(|c| c["clientId"] == "portal-api")
        .unwrap();
    portal["redirectUris"]
        .as_array_mut()
        .unwrap()
        .push(json!(IMPORTED_CALLBACK));
    let (_, gateway) = jc::setup::gateway_realm_file(&jc::setup::Realm::parse(realm).unwrap());
    let mut gateway = gateway["clients"][0].clone();
    gateway["secret"] = json!(placeholder("gateway-client-secret"));
    clients.push(gateway);
    std::fs::write(realms.join(file), value.to_string()).unwrap();
}

/// The `iss` of a token, read without a check (the services under test do the checking).
fn issuer_of(token: &str) -> String {
    claims_of(token)["iss"].as_str().unwrap().to_owned()
}

/// The claims of a JWT, unverified.
fn claims_of(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap(),
    )
    .unwrap()
}

/// A service-account token of `client` in `realm` (client credentials, the secret jc keeps).
async fn service_token(jc: &Jc, realm: &str, client: &str, secret: &str) -> String {
    let resp = Client::new()
        .post(jc.url(&format!("/id/realms/{realm}/protocol/openid-connect/token")))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client),
            ("client_secret", &jc.secret(secret)),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "{client} in {realm}: {body}");
    body["access_token"].as_str().unwrap().to_owned()
}

/// A kaisatsu admin read in `realm` as that realm's `portal-api` (view-clients, query-users).
async fn admin_read(jc: &Jc, realm: &str, path: &str) -> Value {
    let token = service_token(jc, realm, "portal-api", "portal-client-secret").await;
    let (status, body) = call(
        jc,
        reqwest::Method::GET,
        &format!("/id/admin/realms/{realm}{path}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{realm}{path}: {body}");
    serde_json::from_str(&body).unwrap()
}

/// `portal-api` of `realm` as kaisatsu's admin API shows it.
async fn portal_client(jc: &Jc, realm: &str) -> Value {
    let found = admin_read(jc, realm, "/clients?clientId=portal-api").await;
    found[0].clone()
}

/// What jc wrote into the imported `dev`: nothing of its own first-start files.
async fn assert_dev_left_as_imported(jc: &Jc) {
    assert_eq!(
        std::fs::read_to_string(jc.state.join("realm-owner-dev")).unwrap(),
        "import\n"
    );
    let portal = portal_client(jc, "dev").await;
    assert!(
        portal["redirectUris"]
            .as_array()
            .unwrap()
            .contains(&json!(IMPORTED_CALLBACK)),
        "jc overwrote the imported portal-api: {portal}"
    );
    let admins = admin_read(jc, "dev", "/users?username=admin&exact=true").await;
    assert_eq!(
        admins,
        json!([]),
        "jc made a first admin in the imported realm"
    );
    assert!(jc.log_has("the deployment brings the Portal's realm"));
}

/// The Portal's answer to `/api/v1/auth/me` for `token` sent as a bearer or as the edge's
/// `X-Access-Token`.
async fn portal_me(jc: &Jc, token: &str, edge: bool) -> StatusCode {
    let req = Client::new().get(jc.url("/api/v1/auth/me"));
    let req = if edge {
        req.header("x-access-token", token)
    } else {
        req.bearer_auth(token)
    };
    req.send().await.unwrap().status()
}

/// The realm's token exchange (RFC 8693) at `dev` as `portal-api`, for one Endpoint as the
/// Portal's data reads ask it.
async fn exchange_at_dev(jc: &Jc, subject: &str) -> (StatusCode, Value) {
    let scope = format!("gateway-hub endpoint:{CLOSED}");
    let resp = Client::new()
        .post(jc.url("/id/realms/dev/protocol/openid-connect/token"))
        .form(&[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("client_id", "portal-api"),
            ("client_secret", &jc.secret("portal-client-secret")),
            ("subject_token", subject),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            (
                "requested_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            ("audience", "mcp-hub"),
            ("scope", &scope),
        ])
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// The key ids a realm publishes.
async fn kids(jc: &Jc, realm: &str) -> Vec<String> {
    let (status, body) = call(
        jc,
        reqwest::Method::GET,
        &format!("/id/realms/{realm}/protocol/openid-connect/certs"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let set: Value = serde_json::from_str(&body).unwrap();
    set["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["kid"].as_str().unwrap().to_owned())
        .collect()
}

/// The realms kaisatsu holds beside the deployment's `dev`, each with `dev`'s clients, people and
/// secrets: jc's default, and one whose issuer begins with dev's (an issuer prefix match admits
/// it).
const TWINS: [&str; 2] = ["jc", "devx"];

/// The Keycloak exports of realm `dev` and the twins under `root`, issued at `base`, with the
/// test's people and clients beside each.
fn dev_exports(root: &Path, base: &str) {
    let (realms, secrets, state) = (
        root.join("realms"),
        root.join("realm-secrets"),
        root.join("state"),
    );
    export(&realms, &secrets, &state, "dev", base, "00-dev-export.json");
    realm_file(&realms, &secrets, "dev", "10-dev-e2e.json");
    for twin in TWINS {
        export(
            &realms,
            &secrets,
            &state,
            twin,
            base,
            &format!("20-{twin}-export.json"),
        );
        realm_file(&realms, &secrets, twin, &format!("30-{twin}-e2e.json"));
    }
}

/// A deployment whose realm is `dev` (`--identity-realm dev`) over an imported Keycloak `dev`
/// export, with kaisatsu also holding twin realms ([`TWINS`]):
/// jc writes nothing into `dev`, the Portal signs in there, the gateway admits `dev`'s tokens,
/// the linker links the Endpoint scopes in `dev`; a valid `jc` token admits nowhere. A restart
/// without the `jc` files leaves `dev` as imported and makes no `jc` realm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deployment_on_realm_dev_admits_its_realm_and_no_other() {
    let args = ["--identity-realm", "dev"];
    let envs = [("JC_TRUST_EDGE_TOKEN", "true")];
    // The exports name the port, so a start that lost its port to another bind begins again in
    // a fresh directory on another port.
    let mut started = None;
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        dev_exports(dir.path(), &format!("http://localhost:{port}"));
        if let Some(jc) = start_at(dir.path(), port, &args, &envs).await {
            started = Some((dir, port, jc));
            break;
        }
    }
    let (dir, port, jc) = started.expect("jc lost its listen port to another bind 5 times");
    let root = dir.path();
    let base = format!("http://localhost:{port}");
    let (realms, state) = (root.join("realms"), root.join("state"));
    assert_dev_left_as_imported(&jc).await;

    open_project(&jc).await;
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;
    let public = format!("/api/endpoint/{PUBLIC}/ngsi-ld/v1");
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");

    // The gateway admits dev's tokens: a writer's create and a reader's read land.
    let reader = token_in(&jc, "dev", "e2e-reader", PERSON).await;
    assert_eq!(issuer_of(&reader), format!("{base}/id/realms/dev"));
    let thing = entity("Thing", "one");
    eventually("a dev writer's create lands", &jc, 60, || async {
        let (status, body) = call(
            &jc,
            post.clone(),
            &format!("{closed}/entities"),
            Some(&reader),
            Some(&thing),
        )
        .await;
        (status == StatusCode::CREATED)
            .then_some(())
            .ok_or(format!("{status} {body}"))
    })
    .await;
    let read = format!("{closed}/entities?type=Thing");
    let (status, body) = call(&jc, get.clone(), &read, Some(&reader), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:invalid_jc:one"), "{body}");

    // Each twin: the same client, person, password and secret, a valid token of its realm.
    // The gateway checks the issuer exactly (`devx`'s begins with dev's): 401 on the closed
    // Endpoint, and on the public one too (a presented token is never read as anonymous).
    for realm in TWINS {
        let twin = token_in(&jc, realm, "e2e-reader", PERSON).await;
        assert_eq!(issuer_of(&twin), format!("{base}/id/realms/{realm}"));
        for path in [read.clone(), format!("{public}/entities?type=Thing")] {
            let (status, body) = call(&jc, get.clone(), &path, Some(&twin), None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{realm} {path}: {body}");
        }
        let (status, body) = call(
            &jc,
            post.clone(),
            &format!("{closed}/entities"),
            Some(&twin),
            Some(&entity("Thing", "two")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{realm}: {body}");
    }

    // The Portal: dev's token for portal-api is the person, as a bearer and as the edge's
    // X-Access-Token; the twin's is 401 either way.
    let member = token_in(&jc, "dev", "e2e-mcp", PERSON).await;
    let mut twin_members = Vec::new();
    for realm in TWINS {
        twin_members.push(token_in(&jc, realm, "e2e-mcp", PERSON).await);
    }
    for edge in [false, true] {
        for (realm, twin) in TWINS.iter().zip(&twin_members) {
            assert_eq!(
                portal_me(&jc, twin, edge).await,
                StatusCode::UNAUTHORIZED,
                "a realm-{realm} token at the Portal, edge {edge}"
            );
        }
        assert_eq!(
            portal_me(&jc, &member, edge).await,
            StatusCode::OK,
            "edge {edge}"
        );
    }

    // The linker links the Endpoint's scope to portal-api in dev, and never in jc.
    let scope = format!("endpoint:{CLOSED}");
    eventually(
        "dev's portal-api holds the Endpoint scope",
        &jc,
        60,
        || async {
            let portal = portal_client(&jc, "dev").await;
            portal["optionalClientScopes"]
                .as_array()
                .is_some_and(|s| s.contains(&json!(scope)))
                .then_some(())
                .ok_or(portal.to_string())
        },
    )
    .await;
    for realm in TWINS {
        let twin_portal = portal_client(&jc, realm).await;
        assert!(
            !twin_portal.to_string().contains("endpoint:e2e"),
            "the linker wrote into realm {realm}: {twin_portal}"
        );
    }

    // Token exchange at dev: dev's subject token is exchanged, the twin's refused.
    let (status, body) = exchange_at_dev(&jc, &member).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let dev_kids = kids(&jc, "dev").await;
    assert!(!dev_kids.is_empty());
    for (realm, twin) in TWINS.iter().zip(&twin_members) {
        let (status, body) = exchange_at_dev(&jc, twin).await;
        assert!(status.is_client_error(), "{realm}: {status} {body}");
        // kaisatsu answers as Keycloak 26 does for a subject token it cannot verify (signature,
        // exact issuer, audience): invalid_request "Invalid token".
        let refused = (body["error"].as_str(), body["error_description"].as_str());
        assert!(
            matches!(refused.0, Some("invalid_grant" | "invalid_token"))
                || refused == (Some("invalid_request"), Some("Invalid token")),
            "{realm}: {status} {body}"
        );
        // Keys stay per realm.
        let twin_kids = kids(&jc, realm).await;
        assert!(!twin_kids.is_empty());
        assert!(
            dev_kids.iter().all(|k| !twin_kids.contains(k)),
            "{realm}: {dev_kids:?} {twin_kids:?}"
        );
    }

    // A person signs in to the Portal at /realms/dev.
    let mut browser = Browser::new();
    common::sign_in_at(&|p| jc.url(p), &mut browser, "dev", PERSON, PERSON_PASSWORD).await;
    let me = browser.get(&jc.url("/api/v1/auth/me")).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text);
    assert!(me.text.contains(PERSON), "{}", me.text);

    // Restart on the same state, the twin's files gone: dev is still as imported, and no realm
    // jc exists (no dormant realm, no first admin anywhere).
    drop(jc);
    for twin in TWINS {
        std::fs::remove_file(realms.join(format!("20-{twin}-export.json"))).unwrap();
        std::fs::remove_file(realms.join(format!("30-{twin}-e2e.json"))).unwrap();
    }
    let jc = start_at(root, port, &args, &envs)
        .await
        .unwrap_or_else(|| panic!("another bind took port {port} between jc's two starts"));
    assert_dev_left_as_imported(&jc).await;
    let (status, body) = call(
        &jc,
        get.clone(),
        "/id/realms/jc/.well-known/openid-configuration",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a realm jc exists: {body}");
    assert!(!state.join("realm-owner-jc").exists());
}
