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
use common::{Browser, sign_in};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROJECT: &str = "e2e";
const SPACE: &str = "things";
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

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The last lines of the child's log; the whole log goes to a file named in the answer.
    fn tail(&self) -> String {
        let log = self.log.lock().unwrap();
        let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("gateway_endpoints.log");
        let _ = std::fs::write(&file, log.join("\n"));
        format!(
            "{}\n(whole log: {})",
            log[log.len().saturating_sub(60)..].join("\n"),
            file.display()
        )
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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
            client("e2e-reader", CLOSED),
            client("e2e-elsewhere", ELSEWHERE),
            client("e2e-mcp", "portal-api"),
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
            &[("JC_BRANDING_FILE", branding.to_str().unwrap())],
        )
        .await
    } else {
        start_at(root, port, &[], &[]).await
    };
    jc._dir = Some(dir);
    jc
}

/// Starts `jc` over `root` (state, `realms` import dir and `realm-secrets` under it) on `port`,
/// with `args` and `envs` beside the test's own.
async fn start_at(root: &Path, port: u16, args: &[&str], envs: &[(&str, &str)]) -> Jc {
    start_on(root, &format!("localhost:{port}"), port, args, envs).await
}

/// As [`start_at`], with the public host named `base_host`.
async fn start_on(
    root: &Path,
    base_host: &str,
    port: u16,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Jc {
    let state = root.join("state");
    let realms = root.join("realms");
    let realm_secrets = root.join("realm-secrets");
    let config = root.join("jc.toml");
    std::fs::write(&config, "[mounts.portal]\npath_prefix = \"\"\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jc"));
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
    };
    match rx.recv_timeout(Duration::from_secs(180)) {
        Ok(Ok(())) => jc,
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
        "id": format!("urn:ngsi-ld:{kind}:{local}"),
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
}

/// The jc image runs no `git` (distroless): the gateway reads shako over its HTTP API, so with an
/// empty PATH it starts and the public Endpoint answers an anonymous read. On the git resolver
/// the gateway was not built ("git does not run") and every Endpoint read answered 503.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_reads_the_forge_with_no_program_on_the_path() {
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
    let jc = start_at(root, free_port(), &[], &[("PATH", empty.to_str().unwrap())]).await;
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
    assert!(jc.log_has("gateway: the forge checkout read"), "{}", jc.tail());
    assert!(!jc.log_has("git does not run"), "{}", jc.tail());
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

    // Anonymous: the public Endpoint answers types and an entity query, the closed one 401.
    let (status, body) = call(&jc, get.clone(), &format!("{public}/types"), None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Thing"), "{body}");
    let (status, body) = call(
        &jc,
        get.clone(),
        &format!("{public}/entities?type=Thing"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ngsi-ld:Thing:one"), "{body}");
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
    assert!(body.contains("urn:ngsi-ld:Thing:one"), "{body}");

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
            (status == StatusCode::OK && body.contains("urn:ngsi-ld:Thing:one"))
                .then_some(body.clone())
                .ok_or(format!("{status} {body}"))
        },
    )
    .await;
    assert!(!body.contains("not open to data reads"), "{body}");
    // A person with no grant on the Endpoint gets a token exchanged the same way, and the
    // gateway's Policy still refuses them: the scope names the Endpoint, never a right to it.
    let stranger = token_of(&jc, "e2e-mcp", STRANGER).await;
    let (status, body) = call(
        &jc,
        post.clone(),
        "/api/v1/mcp",
        Some(&stranger),
        Some(&data_query(4)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains("urn:ngsi-ld:Thing:one"),
        "a person without a grant read the closed Endpoint: {body}"
    );
    assert!(
        jc.log_has("the gateway refused a shared Endpoint read"),
        "the stranger's read never reached the gateway's refusal:\n{}",
        jc.tail()
    );

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
        !body.contains("urn:ngsi-ld:Thing:one"),
        "the broker believed a claimed identity: {status} {body}"
    );

    // MCP through the hub: the tools list and one query of the public Endpoint, anonymously.
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let (status, body) = call(&jc, post.clone(), "/api/mcp", None, Some(&list)).await;
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
    assert!(body.contains("urn:ngsi-ld:Thing:one"), "{body}");

    // The gateway's revocation poll (every 10 s) is admitted by the Portal's internal listener:
    // its token names the issuer the Portal checks and the client the route belongs to.
    tokio::time::sleep(Duration::from_secs(25).saturating_sub(started.elapsed())).await;
    assert!(
        !jc.log_has("the Portal answered 401"),
        "the Portal refused the gateway:\n{}",
        jc.tail()
    );

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
    let (status, metadata) = call(
        &jc,
        get.clone(),
        &format!("{base}/odata/v4/$metadata"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{metadata}");
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
                "bloblang": "root = {\"id\": \"urn:ngsi-ld:Thing:fed\", \"type\": \"Thing\", \"name\": {\"type\": \"Property\", \"value\": \"fed\"}}"
            },
            "targetEndpoint": format!("urn:ngsi-ld:Endpoint:jc.invalid:{SPACE}:closed"),
            "output": { "type": "Thing", "mode": "upsert" }
        }
    })
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

/// A Pipeline proposed and approved in the Portal runs on ekiben inside jc and its Entities land
/// in Antares: the stream reads `JC_GATEWAY_URL`, `JC_TOKEN_URL` and `JC_CLIENT_SECRET_E2E`
/// from jc's mounts and its client secrets directory (the realm file's own secret file), reaches
/// jc's own listener only at the gateway door and the token endpoint, and names no other
/// project's secret. No secret value shows in the runner's API or jc's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pipeline_approved_in_the_portal_writes_through_ekiben_inside_jc() {
    let jc = start_with(true).await;
    eventually(
        "shako holds the organization repository",
        &jc,
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
        put_file(&jc, &path, &content).await;
    }
    let closed = format!("/api/endpoint/{CLOSED}/ngsi-ld/v1");
    let reader = token(&jc, "e2e-reader").await;
    eventually("the closed Endpoint answers", &jc, 120, || async {
        let (status, body) = call(
            &jc,
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
    let approver_name = second_admin(&jc, approver_pw).await;
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
    let entity = loop {
        // The stream first: it is held before it writes, so a Thing that lands between two
        // polls never leaves the stream unseen.
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
        if status == StatusCode::OK && body.contains("Thing:") {
            break body;
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
            let tok: Value = Client::new()
                .post(jc.url("/id/realms/jc/protocol/openid-connect/token"))
                .form(&[
                    ("grant_type", "client_credentials"),
                    ("client_id", &format!("{PROJECT}-pipelines")),
                    ("client_secret", PIPELINES_SECRET),
                ])
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let (wstatus, wbody) = call(
                &jc,
                reqwest::Method::POST,
                &format!("{closed}/entityOperations/upsert?options=update"),
                tok["access_token"].as_str(),
                Some(&json!([{"id": "urn:ngsi-ld:Thing:probe", "type": "Thing", "name": {"type": "Property", "value": "p"}}])),
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
    let payload = token.split('.').nth(1).unwrap();
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap(),
    )
    .unwrap();
    claims["iss"].as_str().unwrap().to_owned()
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

/// A deployment whose realm is `dev` (`--identity-realm dev`) over an imported Keycloak `dev`
/// export, with kaisatsu also holding twin realms ([`TWINS`]):
/// jc writes nothing into `dev`, the Portal signs in there, the gateway admits `dev`'s tokens,
/// the linker links the Endpoint scopes in `dev`; a valid `jc` token admits nowhere. A restart
/// without the `jc` files leaves `dev` as imported and makes no `jc` realm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deployment_on_realm_dev_admits_its_realm_and_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let port = free_port();
    let base = format!("http://localhost:{port}");
    let (realms, secrets, state) = (
        root.join("realms"),
        root.join("realm-secrets"),
        root.join("state"),
    );
    export(
        &realms,
        &secrets,
        &state,
        "dev",
        &base,
        "00-dev-export.json",
    );
    realm_file(&realms, &secrets, "dev", "10-dev-e2e.json");
    for twin in TWINS {
        export(
            &realms,
            &secrets,
            &state,
            twin,
            &base,
            &format!("20-{twin}-export.json"),
        );
        realm_file(&realms, &secrets, twin, &format!("30-{twin}-e2e.json"));
    }
    let args = ["--identity-realm", "dev"];
    let envs = [("JC_TRUST_EDGE_TOKEN", "true")];
    let jc = start_at(root, port, &args, &envs).await;
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
    assert!(body.contains("urn:ngsi-ld:Thing:one"), "{body}");

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
    let jc = start_at(root, port, &args, &envs).await;
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
