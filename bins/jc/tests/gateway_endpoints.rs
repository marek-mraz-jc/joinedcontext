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
    let dir = tempfile::tempdir().unwrap();
    realm_file(
        &dir.path().join("realms"),
        &dir.path().join("realm-secrets"),
        "jc",
        "00-e2e.json",
    );
    let mut jc = start_at(dir.path(), free_port(), &[], &[]).await;
    jc._dir = Some(dir);
    jc
}

/// Starts `jc` over `root` (state, `realms` import dir and `realm-secrets` under it) on `port`,
/// with `args` and `envs` beside the test's own.
async fn start_at(root: &Path, port: u16, args: &[&str], envs: &[(&str, &str)]) -> Jc {
    let state = root.join("state");
    let realms = root.join("realms");
    let realm_secrets = root.join("realm-secrets");
    let base_host = format!("localhost:{port}");
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
        .args(["--base-host", &base_host])
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
    eventually(
        "the public Endpoint answers its types",
        jc,
        120,
        || async {
            let (status, body) =
                call(jc, get.clone(), &format!("{public}/types"), None, None).await;
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
        },
    )
    .await;
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
    assert_eq!(admins, json!([]), "jc made a first admin in the imported realm");
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

/// A deployment whose realm is `dev` (`--identity-realm dev`) over an imported Keycloak `dev`
/// export, with kaisatsu also holding a twin realm `jc` (same clients, people and secrets):
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
    export(&realms, &secrets, &state, "dev", &base, "00-dev-export.json");
    realm_file(&realms, &secrets, "dev", "10-dev-e2e.json");
    export(&realms, &secrets, &state, "jc", &base, "20-jc-export.json");
    realm_file(&realms, &secrets, "jc", "30-jc-e2e.json");
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

    // The twin in realm jc: the same client, person, password and secret, a valid token of
    // that realm. The gateway checks the issuer exactly: 401 on the closed Endpoint, and on the
    // public one too (a presented token is never read as anonymous).
    let twin = token_in(&jc, "jc", "e2e-reader", PERSON).await;
    assert_eq!(issuer_of(&twin), format!("{base}/id/realms/jc"));
    for path in [read.clone(), format!("{public}/entities?type=Thing")] {
        let (status, body) = call(&jc, get.clone(), &path, Some(&twin), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {body}");
    }
    let (status, body) = call(
        &jc,
        post.clone(),
        &format!("{closed}/entities"),
        Some(&twin),
        Some(&entity("Thing", "two")),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // The Portal: dev's token for portal-api is the person, as a bearer and as the edge's
    // X-Access-Token; the twin's is 401 either way.
    let member = token_in(&jc, "dev", "e2e-mcp", PERSON).await;
    let twin_member = token_in(&jc, "jc", "e2e-mcp", PERSON).await;
    for edge in [false, true] {
        assert_eq!(portal_me(&jc, &member, edge).await, StatusCode::OK, "edge {edge}");
        assert_eq!(
            portal_me(&jc, &twin_member, edge).await,
            StatusCode::UNAUTHORIZED,
            "a realm-jc token at the Portal, edge {edge}"
        );
    }

    // The linker links the Endpoint's scope to portal-api in dev, and never in jc.
    let scope = format!("endpoint:{CLOSED}");
    eventually("dev's portal-api holds the Endpoint scope", &jc, 60, || async {
        let portal = portal_client(&jc, "dev").await;
        portal["optionalClientScopes"]
            .as_array()
            .is_some_and(|s| s.contains(&json!(scope)))
            .then_some(())
            .ok_or(portal.to_string())
    })
    .await;
    let twin_portal = portal_client(&jc, "jc").await;
    assert!(
        !twin_portal.to_string().contains("endpoint:e2e"),
        "the linker wrote into realm jc: {twin_portal}"
    );

    // Token exchange at dev: dev's subject token is exchanged, the twin's refused.
    let (status, body) = exchange_at_dev(&jc, &member).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = exchange_at_dev(&jc, &twin_member).await;
    assert!(status.is_client_error(), "{status} {body}");
    // kaisatsu answers as Keycloak 26 does for a subject token it cannot verify (signature,
    // exact issuer, audience): invalid_request "Invalid token".
    let refused = (body["error"].as_str(), body["error_description"].as_str());
    assert!(
        matches!(refused.0, Some("invalid_grant" | "invalid_token"))
            || refused == (Some("invalid_request"), Some("Invalid token")),
        "{status} {body}"
    );

    // Keys stay per realm.
    let (dev_kids, jc_kids) = (kids(&jc, "dev").await, kids(&jc, "jc").await);
    assert!(!dev_kids.is_empty() && !jc_kids.is_empty());
    assert!(
        dev_kids.iter().all(|k| !jc_kids.contains(k)),
        "{dev_kids:?} {jc_kids:?}"
    );

    // A person signs in to the Portal at /realms/dev.
    let mut browser = Browser::new();
    common::sign_in_at(&|p| jc.url(p), &mut browser, "dev", PERSON, PERSON_PASSWORD).await;
    let me = browser.get(&jc.url("/api/v1/auth/me")).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text);
    assert!(me.text.contains(PERSON), "{}", me.text);

    // Restart on the same state, the twin's files gone: dev is still as imported, and no realm
    // jc exists (no dormant realm, no first admin anywhere).
    drop(jc);
    std::fs::remove_file(realms.join("20-jc-export.json")).unwrap();
    std::fs::remove_file(realms.join("30-jc-e2e.json")).unwrap();
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
