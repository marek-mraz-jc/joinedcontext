#![forbid(unsafe_code)]
//! The Portal's read-only git door (T-6896) in front of real shako: `git clone` over HTTP with a
//! Keycloak bearer token in the header, through the Portal's door, reads the repositories the
//! person may read, and every refusal (no token, a token in the URL, no right, a push) is the
//! same `404`.

use jc_router::{JcRouterBuilder, Mount, MountSpec};
use joinedcontext_portal::auth::bearer::BearerVerifier;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::shako::{Author, ShakoClient};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{API_VERSION, ObjectMeta, ResourceEnvelope};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use p256::pkcs8::EncodePrivateKey;
use serde_json::{Value, json};
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, OutboundPolicy, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_server::{ShakoState, router};
use shako_store_memory::MemoryStore;
use std::collections::BTreeMap;
use std::sync::Arc;

const OWNER: &str = "joinedcontext";
const PORTAL_TOKEN: &str = "portal-token-0123456789";
const INSTALLATION: &str = "configuration";
const PROJECT: &str = "ovzdusie";
const PROJECT_REPO: &str = "ovzdusie-config";
const ISSUER: &str = "https://idm.city.example/realms/city";
const KID: &str = "door-key";

const AUTHOR: Author<'static> = Author {
    name: "Alice Steward",
    email: "alice@city.example",
};

struct World {
    _tmp: tempfile::TempDir,
    /// `http://127.0.0.1:{port}/git/`: the Portal's door.
    door: String,
    signer: EncodingKey,
}

/// shako on one listener, the Portal's door on another, the two repositories with a file each.
async fn world() -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let org = OrgId::from_u128(10);
    let authz = Arc::new(LocalAuthorizer::new(vec![RoleBinding {
        org_id: org,
        principal: "jc-portal".into(),
        actions: vec![Action::Admin, Action::Write, Action::Approve],
        project_id: None,
        repo: None,
    }]));
    let verifier = TokenVerifier::new(ShakoSigningKey::generate().expect("key"))
        .with_static_tokens(vec![StaticTokenBinding::new(
            PORTAL_TOKEN,
            Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
        )]);
    let guard = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(vec!["127.0.0.1".into()]),
        allow_loopback_for_tests: true,
        ..OutboundPolicy::default()
    });
    let shako = ShakoState::new(
        InstanceId::from_u128(1),
        org,
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        authz,
        verifier,
        guard,
    );
    shako.register_owner_org(OWNER, org);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let jc = JcRouterBuilder::new("http", host.clone())
        .mount(
            MountSpec {
                module: "shako".into(),
                host: None,
                path_prefix: Some("/git".into()),
            },
            router(
                shako,
                Mount::new("shako", "http", host.clone(), "/git").unwrap(),
            ),
        )
        .build()
        .expect("mount table");
    tokio::spawn(async move { axum::serve(listener, jc.into_axum_router()).await.unwrap() });

    let base = format!("http://{host}/git/");
    let forge = ShakoClient::from_env(|key| match key {
        "JC_SHAKO_URL" => Some(base.clone()),
        "JC_SHAKO_OWNER" => Some(OWNER.into()),
        "JC_SHAKO_REPO" => Some(INSTALLATION.into()),
        "JC_SHAKO_TOKEN" => Some(PORTAL_TOKEN.into()),
        _ => None,
    })
    .expect("config")
    .expect("forge configured");
    for (repo, file) in [(INSTALLATION, "org.yaml"), (PROJECT_REPO, "project.yaml")] {
        let client = forge.for_repository(repo);
        assert!(client.ensure_repository(repo).await.unwrap());
        client
            .write_files(
                "main",
                "Add",
                AUTHOR,
                &[(file.into(), "a: 1\n".into(), None)],
            )
            .await
            .unwrap();
    }

    let (signer, keys) = keypair();
    let bearer = BearerVerifier::with_jwks_url(
        &url::Url::parse(ISSUER).unwrap(),
        "portal",
        url::Url::parse("http://127.0.0.1:9/certs").unwrap(),
    );
    assert_eq!(bearer.install(&keys), 1);
    let mut config = Config::for_tests();
    config.bootstrap_admins = "platform-admins".into();
    let mut state = AppState::new(config, None)
        .with_shako(Arc::new(forge))
        .with_mirror(Arc::new(Mirror::new()));
    state.bearer = Some(Arc::new(bearer));
    state.mirror.set_layout(2);
    state
        .mirror
        .set_repositories(BTreeMap::from([(PROJECT.into(), PROJECT_REPO.into())]));
    state.mirror.upsert(envelope(
        "Organization",
        "city",
        json!({ "displayName": "City" }),
    ));
    for (role, kinds, verbs) in [
        ("viewer-role", json!(["Project"]), json!(["read"])),
        (
            "org-admin",
            json!(["*"]),
            json!(["read", "propose", "approve", "delete"]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (binding, group, role, scope) in [
        (
            "viewers",
            "viewers",
            "viewer-role",
            json!({ "project": PROJECT }),
        ),
        (
            "city-admins",
            "city-admins",
            "org-admin",
            json!({ "organization": "city" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            json!({ "subjects": [{ "group": group }], "role": role, "scope": scope }),
        ));
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let door = format!("http://{}/git/", listener.local_addr().unwrap());
    let app = joinedcontext_portal::forge::git_door::router().with_state(state);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    World {
        _tmp: tmp,
        door,
        signer,
    }
}

fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// The real git client, with `home` as its whole configuration and no prompt.
#[allow(clippy::disallowed_methods)] // The test runs git as a person does; jc runs none of this.
fn git(home: &std::path::Path) -> std::process::Command {
    let mut git = std::process::Command::new("git");
    git.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .current_dir(home);
    git
}

fn keypair() -> (EncodingKey, JwkSet) {
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let signer = EncodingKey::from_ec_der(secret.to_pkcs8_der().unwrap().as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).unwrap();
    jwk["kid"] = json!(KID);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    (
        signer,
        serde_json::from_value(json!({ "keys": [jwk] })).unwrap(),
    )
}

impl World {
    /// A Keycloak access token of `name` in `group`.
    fn token(&self, name: &str, group: &str) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.into());
        let claims = json!({
            "iss": ISSUER, "typ": "Bearer", "aud": "portal", "sub": format!("sub-{name}"),
            "exp": now + 300, "iat": now, "preferred_username": name, "groups": [group],
        });
        encode(&header, &claims, &self.signer).unwrap()
    }

    fn url(&self, repo: &str) -> String {
        format!("{}{OWNER}/{repo}.git", self.door)
    }

    /// `git clone` of `repo` with `token` in the header (none: anonymous); the clone's top
    /// files on success, git's error text otherwise.
    async fn clone(&self, repo: &str, token: Option<String>) -> Result<Vec<String>, String> {
        let url = self.url(repo);
        tokio::task::spawn_blocking(move || {
            let into = tempfile::tempdir().unwrap();
            let mut git = git(into.path());
            if let Some(token) = token {
                git.arg("-c")
                    .arg(format!("http.extraHeader=Authorization: Bearer {token}"));
            }
            let out = git
                .args(["clone", "--quiet", &url])
                .arg(into.path().join("c"))
                .output()
                .unwrap();
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
            let mut files: Vec<String> = std::fs::read_dir(into.path().join("c"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name != ".git")
                .collect();
            files.sort();
            Ok(files)
        })
        .await
        .unwrap()
    }

    async fn status(&self, method: &str, path: &str, token: Option<&str>) -> u16 {
        let http = reqwest::Client::new();
        let url = format!("{}{OWNER}/{path}", self.door);
        let mut request = match method {
            "POST" => http
                .post(url)
                .header("content-type", "application/x-git-upload-pack-request")
                .body("0000"),
            _ => http.get(url),
        };
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_clones_their_project_repository() {
    let w = world().await;
    let files = w
        .clone(PROJECT_REPO, Some(w.token("vera", "viewers")))
        .await;
    assert!(
        files
            .as_ref()
            .is_ok_and(|f| f.contains(&"project.yaml".to_owned())),
        "{files:?}"
    );
    let advert = w
        .status(
            "GET",
            &format!("{PROJECT_REPO}.git/info/refs?service=git-upload-pack"),
            Some(&w.token("vera", "viewers")),
        )
        .await;
    assert_eq!(advert, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_member_gets_404_for_a_project_repository() {
    let w = world().await;
    let outsider = w.token("otto", "outsiders");
    let refs = format!("{PROJECT_REPO}.git/info/refs?service=git-upload-pack");
    assert_eq!(w.status("GET", &refs, Some(&outsider)).await, 404);
    // The same answer as for a repository that is not there, or of another owner.
    let missing = "nowhere.git/info/refs?service=git-upload-pack";
    assert_eq!(w.status("GET", missing, Some(&outsider)).await, 404);
    let member = w.token("vera", "viewers");
    assert_eq!(w.status("GET", missing, Some(&member)).await, 404);
    let err = w.clone(PROJECT_REPO, Some(outsider)).await.unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_platform_admins_clone_the_installation_repository() {
    let w = world().await;
    let admin = w.token("pat", "platform-admins");
    let files = w.clone(INSTALLATION, Some(admin)).await;
    assert!(
        files
            .as_ref()
            .is_ok_and(|f| f.contains(&"org.yaml".to_owned())),
        "{files:?}"
    );
    // An Organization's administrator and a project's member are not the installation's.
    for (name, group) in [("cora", "city-admins"), ("vera", "viewers")] {
        let token = w.token(name, group);
        let refs = format!("{INSTALLATION}.git/info/refs?service=git-upload-pack");
        assert_eq!(w.status("GET", &refs, Some(&token)).await, 404, "{group}");
        assert_eq!(
            w.status(
                "POST",
                &format!("{INSTALLATION}.git/git-upload-pack"),
                Some(&token)
            )
            .await,
            404,
            "{group}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receive_pack_is_refused() {
    let w = world().await;
    let admin = w.token("pat", "platform-admins");
    for (method, path) in [
        (
            "GET",
            "configuration.git/info/refs?service=git-receive-pack",
        ),
        ("POST", "configuration.git/git-receive-pack"),
        ("GET", "configuration.git/git-upload-pack"),
        (
            "POST",
            "configuration.git/info/refs?service=git-upload-pack",
        ),
        ("GET", "configuration/info/refs?service=git-upload-pack"),
        ("GET", "configuration.git/HEAD"),
        ("GET", "configuration"),
    ] {
        assert_eq!(
            w.status(method, path, Some(&admin)).await,
            404,
            "{method} {path}"
        );
    }
    // A real push from a real clone has nothing to talk to.
    let url = w.url(INSTALLATION);
    let out = tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            git(dir.path())
                .arg("-c")
                .arg(format!("http.extraHeader=Authorization: Bearer {admin}"))
                .args(args)
                .output()
                .unwrap()
        };
        let cloned = run(&["clone", "--quiet", &url, "c"]);
        assert!(
            cloned.status.success(),
            "{}",
            String::from_utf8_lossy(&cloned.stderr)
        );
        std::fs::write(dir.path().join("c/x.yaml"), "x: 1\n").unwrap();
        let in_clone = |args: &[&str]| {
            let mut all = vec!["-C", "c"];
            all.extend_from_slice(args);
            run(&all)
        };
        in_clone(&["add", "x.yaml"]);
        in_clone(&[
            "-c",
            "user.name=a",
            "-c",
            "user.email=a@b",
            "commit",
            "-qm",
            "x",
        ]);
        in_clone(&["push", "origin", "HEAD:main"])
    })
    .await
    .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not found"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_anonymous_clone_is_404() {
    let w = world().await;
    let err = w.clone(PROJECT_REPO, None).await.unwrap_err();
    assert!(err.contains("not found"), "{err}");
    let refs = format!("{PROJECT_REPO}.git/info/refs?service=git-upload-pack");
    assert_eq!(w.status("GET", &refs, None).await, 404);
    assert_eq!(w.status("GET", &refs, Some("not-a-jwt")).await, 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_in_the_query_string_is_refused() {
    let w = world().await;
    let token = w.token("vera", "viewers");
    let refs = format!("{PROJECT_REPO}.git/info/refs?service=git-upload-pack");
    for path in [
        format!("{refs}&access_token={token}"),
        format!("{PROJECT_REPO}.git/info/refs?access_token={token}&service=git-upload-pack"),
    ] {
        assert_eq!(w.status("GET", &path, None).await, 404);
        assert_eq!(w.status("GET", &path, Some(&token)).await, 404);
    }
    let pack = format!("{PROJECT_REPO}.git/git-upload-pack?access_token={token}");
    assert_eq!(w.status("POST", &pack, Some(&token)).await, 404);
    // The header alone reads.
    assert_eq!(w.status("GET", &refs, Some(&token)).await, 200);
}
