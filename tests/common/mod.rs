//! Signed-in requests against the Portal router and the forge fixtures the permission tests share.
//!
//! Each test binary that declares `mod common;` compiles the whole file, and none of them uses
//! all of it, so what one binary leaves unused is not dead code.
#![allow(dead_code)]

pub mod apps_db;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

pub mod doors;
pub mod routes;

pub const CSRF: &str = "test-csrf-token-permissions";

/// The mock forge's repository path.
pub const REPO: &str = "/api/v1/repos/test-owner/test-repo";

/// A person signed in as `{name}@hel.fi`, with no group or realm role.
pub fn person(name: &str) -> Identity {
    Identity {
        client: None,
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@hel.fi")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

pub fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// Binds `{who}@hel.fi` to reading Apps in `project`: what makes a person a member of the project,
/// and so of the organization, for an App's visibility (T-4910).
pub fn bind(mirror: &Mirror, who: &str, project: &str) {
    let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
    mirror.upsert(envelope(
        "Role",
        "app-readers",
        org,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        &format!("{who}-in-{project}"),
        org,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": "app-readers",
                "scope": { "project": project } }),
    ));
}

/// A forge file's content as Gitea returns it.
pub fn encode(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text)
}

pub fn cookie(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let session = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar =
        session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

pub struct Answer {
    pub status: StatusCode,
    pub text: String,
}

impl Answer {
    /// Whether the problem document's `errors[]` holds an entry at `path` whose message
    /// contains `message` (T-5949: errors are `{path, message}` objects).
    pub fn names_field(&self, path: &str, message: &str) -> bool {
        let body: serde_json::Value = serde_json::from_str(&self.text).unwrap_or_default();
        body["errors"].as_array().is_some_and(|errors| {
            errors.iter().any(|e| {
                e["path"] == path && e["message"].as_str().is_some_and(|m| m.contains(message))
            })
        })
    }
}

/// One request as `identity`, through the whole router: session cookie, CSRF and all.
pub async fn send(
    state: &AppState,
    identity: Identity,
    http: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let content_type = if http == "PATCH" {
        "application/merge-patch+json"
    } else {
        "application/json"
    };
    let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
    send_body(state, identity, http, uri, content_type, body).await
}

/// [`send`] with a body of any type: an upload's raw bytes.
pub async fn send_body(
    state: &AppState,
    identity: Identity,
    http: &str,
    uri: &str,
    content_type: &str,
    body: Body,
) -> Answer {
    let config = state.config.clone();
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(http)
                .uri(uri)
                .header(header::COOKIE, cookie(&config, identity))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, content_type)
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    Answer {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// A request the way every door makes it since T-0956 (PF-57): a proposal of a manifest — POST,
/// PUT or PATCH on a resource route, or a `*_propose` operation carrying a `manifest` — is checked
/// first with the same body (`?dryRun=All`, or `jc_manifest_dry_run`), and then sent. Any other
/// request is sent as it is; an approval is never repeated as a check.
pub async fn checked_send(
    state: &AppState,
    identity: Identity,
    http: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    if matches!(http, "POST" | "PUT" | "PATCH") && !uri.contains("dryRun") {
        let path = uri.split('?').next().unwrap_or(uri);
        let rest: Vec<&str> = path
            .strip_prefix("/api/v1/projects/")
            .map(|rest| rest.split('/').collect())
            .unwrap_or_default();
        match rest.as_slice() {
            [project, "ops", op] if op.ends_with("_propose") => {
                if let Some(manifest) = body.as_ref().and_then(|b| b.get("manifest")) {
                    let check = serde_json::json!({ "manifest": manifest });
                    let uri = format!("/api/v1/projects/{project}/ops/jc_manifest_dry_run");
                    send(state, identity.clone(), "POST", &uri, Some(check)).await;
                }
            }
            [_, plural] | [_, plural, _]
                if joinedcontext_portal::resource::by_plural(plural).is_some() =>
            {
                let joiner = if uri.contains('?') { '&' } else { '?' };
                let dry = format!("{uri}{joiner}dryRun=All");
                send(state, identity.clone(), http, &dry, body.clone()).await;
            }
            _ => {}
        }
        if let (Some(project), Some(name)) = (
            rest.first(),
            body.as_ref()
                .and_then(|b| b.get("manifest").unwrap_or(b).pointer("/metadata/name"))
                .and_then(Value::as_str)
                .or_else(|| rest.get(2).copied()),
        ) {
            records_checked(state, project, name).await;
        }
    }
    send(state, identity, http, uri, body).await
}

/// A Pipeline also proposes on a green record check of its spec (T-4840), which the workbench
/// keeps on its draft: the check of the draft's manifest, recorded the way a sample would give it.
/// Anything but a Pipeline draft with a verdict is left alone.
pub async fn records_checked(state: &AppState, project: &str, name: &str) {
    use joinedcontext_portal::ops::verdict::{digest_of, RecordsVerdict};
    let store = state.drafts.clone();
    let Ok(Some(draft)) = store.get(project, "Pipeline", name).await else {
        return;
    };
    let Some(mut verdict) = draft.verdict else {
        return;
    };
    verdict.records = Some(Box::new(RecordsVerdict {
        ok: true,
        checked: 1,
        rejected: 0,
        spec_digest: digest_of(&draft.manifest["spec"]),
        sample_digest: digest_of(&Value::Array(Vec::new())),
    }));
    let _ = store.set_verdict(project, "Pipeline", name, verdict).await;
}

/// A mirror holding the resources a manifest under test points at (MF-13, T-2233).
///
/// The dry run of every door refuses a manifest naming a resource that is not there, so a test
/// proposing one has to say what exists — the same thing the person's project says. Each pair is
/// `(kind, name)`, all in `namespace`.
pub fn mirror_holding(namespace: &str, resources: &[(&str, &str)]) -> Arc<Mirror> {
    let mirror = Mirror::new();
    for (kind, name) in resources {
        mirror.upsert(joinedcontext_portal::resource::ResourceEnvelope {
            api_version: joinedcontext_portal::resource::API_VERSION.to_owned(),
            kind: (*kind).to_owned(),
            metadata: joinedcontext_portal::resource::ObjectMeta {
                name: (*name).to_owned(),
                namespace: Some(namespace.to_owned()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        });
    }
    Arc::new(mirror)
}

/// The Portal's state against the mock forge `gitea`, with an empty mirror.
pub fn state_on(gitea: &MockServer) -> AppState {
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client))
}

/// A state on a forge that takes every write and answers for as long as the test binary runs.
/// A dropped `MockServer` goes back to wiremock's pool, and the next test that takes it resets
/// its mocks while this state still asks it: a 404 from a forge that held the file (T-3824).
pub async fn state_on_forge() -> AppState {
    let gitea = forge().await;
    let state = state_on(&gitea);
    std::mem::forget(gitea);
    state
}

/// A forge that takes every write: branches, files, merge requests, merges and closes. The files
/// and merge requests a test reads are mounted on top of it.
pub async fn forge() -> MockServer {
    let gitea = MockServer::start().await;
    mount_repository(&gitea, REPO).await;
    gitea
}

/// One more repository of the forge that takes every write, at `REPO`-style `repo` (a project
/// repository of layout 2 beside the organization's, CC-85).
pub async fn mount_repository(gitea: &MockServer, repo: &str) {
    Mock::given(method("GET"))
        .and(path(repo))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(gitea)
        .await;
    for verb in ["PUT", "POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!("^{repo}/contents/.*")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "commit": { "sha": "commit-1" } })),
            )
            .mount(gitea)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{repo}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 9, "html_url": "https://gitea.example/pulls/9", "state": "open",
            "mergeable": true, "merged": false
        })))
        .mount(gitea)
        .await;
    Mock::given(path_regex(format!("^{repo}/pulls/[0-9]+/(merge|reviews)$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(gitea)
        .await;
    Mock::given(method("PATCH"))
        .and(path_regex(format!("^{repo}/pulls/[0-9]+$")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "state": "closed" })))
        .mount(gitea)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{repo}/contents/.*")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(9)
        .mount(gitea)
        .await;
    // Every file of one change goes in one commit to `POST /contents`, which is the repository
    // path itself and not a path under it, so the per-file mocks above never match it (T-0900).
    Mock::given(method("POST"))
        .and(path(format!("{repo}/contents")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } })),
        )
        .mount(gitea)
        .await;
    // One open change per resource: a removal asks the forge for the open merge requests first
    // (CC-34, T-0883). This fixture has none open — at the fallback priority, so a suite that
    // mounts a forge with an open change of its own is answered its own, not this empty list.
    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(9)
        .mount(gitea)
        .await;
    // A removal lists the tree to find the files the resource owns beside its manifest (T-0900).
    // This fixture holds manifests alone, so the listing is empty and a delete removes the one
    // file; a suite with side files mounts a tree of its own, and this one stands at the fallback
    // priority so that tree is the one answered.
    Mock::given(method("GET"))
        .and(path_regex(format!("^{repo}/git/trees/.*")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": [], "truncated": false })),
        )
        .with_priority(9)
        .mount(gitea)
        .await;
}

/// A proposal the way every door makes one since T-0956: the same request as a dry run first, which
/// records a green verdict for the manifest (PF-57), then the proposal itself.
pub trait CheckFirst {
    async fn oneshot_checked(
        self,
        request: Request<Body>,
    ) -> Result<axum::response::Response, std::convert::Infallible>;
}

impl CheckFirst for axum::Router {
    async fn oneshot_checked(
        self,
        request: Request<Body>,
    ) -> Result<axum::response::Response, std::convert::Infallible> {
        let (parts, body) = request.into_parts();
        let bytes = body.collect().await.expect("request body").to_bytes();
        let uri = parts.uri.to_string();
        let dry_uri = if uri.contains('?') {
            format!("{uri}&dryRun=All")
        } else {
            format!("{uri}?dryRun=All")
        };
        let mut check = Request::builder().method(parts.method.clone()).uri(dry_uri);
        for (name, value) in &parts.headers {
            check = check.header(name, value);
        }
        let checked = self
            .clone()
            .oneshot(
                check
                    .body(Body::from(bytes.clone()))
                    .expect("check request"),
            )
            .await?;
        assert_eq!(
            checked.status(),
            StatusCode::OK,
            "the check before the proposal"
        );
        self.oneshot(Request::from_parts(parts, Body::from(bytes)))
            .await
    }
}

// --- the realm the internal listener verifies against (T-2271; AG-52, PF-46) ------------------

/// A realm that signs workload tokens, for the suites that call the Portal's internal listener.
///
/// The listener takes a ServiceAccount token per route and no static string any more, so a test
/// that calls one has to present a real signed token. One realm per process, leaked on purpose:
/// the Portal's JWKS cache holds its URL for as long as the test binary runs.
pub struct Realm {
    pub issuer: String,
    signer: jsonwebtoken::EncodingKey,
    /// The realm's server, for a suite that needs one more of its endpoints.
    pub server: &'static MockServer,
}

/// What a token for the internal listener must be issued for.
pub const INTERNAL_AUDIENCE: &str = "portal-internal";

impl Realm {
    /// A token of `client`, issued for `audience`, valid for five minutes.
    pub fn token(&self, client: &str, audience: &str) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some("key-workload-test".to_owned());
        let now = joinedcontext_portal::auth::session::now_unix();
        let claims = json!({
            "iss": self.issuer,
            "typ": "Bearer",
            "aud": audience,
            "sub": format!("service-account-{client}"),
            "azp": client,
            "exp": now + 300,
            "iat": now,
        });
        jsonwebtoken::encode(&header, &claims, &self.signer).expect("sign")
    }

    /// A person's token of App `app`'s own client `app-{app}` (ADR-N-030): issued for it, obtained
    /// by it, carrying `roles` as that client's roles and a realm role of the same names beside
    /// them, which must count for nothing.
    pub fn person_token(&self, app: &str, who: &str, roles: &[&str]) -> String {
        self.person_token_of(&format!("app-{app}"), &format!("app-{app}"), who, roles)
    }

    /// [`Self::person_token`] with the audience and the obtaining client (`azp`) chosen apart.
    pub fn person_token_of(
        &self,
        audience: &str,
        client: &str,
        who: &str,
        roles: &[&str],
    ) -> String {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some("key-workload-test".to_owned());
        let now = joinedcontext_portal::auth::session::now_unix();
        let claims = json!({
            "iss": self.issuer,
            "typ": "Bearer",
            "aud": audience,
            "sub": format!("sub-{who}"),
            "azp": client,
            "preferred_username": format!("{who}@hel.fi"),
            "email": format!("{who}@hel.fi"),
            "email_verified": true,
            "realm_access": { "roles": roles },
            "resource_access": { client: { "roles": roles }, "app-other": { "roles": ["steward"] } },
            "exp": now + 300,
            "iat": now,
        });
        jsonwebtoken::encode(&header, &claims, &self.signer).expect("sign")
    }

    /// The token a workload presents on the internal listener.
    pub fn workload(&self, client: &str) -> String {
        self.token(client, INTERNAL_AUDIENCE)
    }
}

/// The process's realm, started on a runtime of its own so a synchronous `config()` can name its
/// issuer. The server and the runtime are both leaked: the Portal's JWKS cache holds that URL for
/// as long as the test binary runs, and a dropped mock server answers nothing.
pub static REALM: std::sync::LazyLock<Realm> = std::sync::LazyLock::new(|| {
    use p256::pkcs8::EncodePrivateKey;

    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = jsonwebtoken::EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!("key-workload-test");
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    // On a thread of its own: this runs the first time a test asks for the realm, and that test is
    // already inside a runtime — `block_on` there would panic. The runtime is leaked, so its worker
    // keeps answering the JWKS request after this thread is gone.
    let issuer = std::thread::spawn(move || {
        let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a runtime for the realm"),
        ));
        runtime.block_on(async move {
            let server: &'static MockServer = Box::leak(Box::new(MockServer::start().await));
            let issuer = format!(
                "{}/realms/banskabystrica",
                server.uri().trim_end_matches('/')
            );
            Mock::given(method("GET"))
                .and(path("/realms/banskabystrica/protocol/openid-connect/certs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [jwk] })))
                .mount(server)
                .await;
            // A Portal that knows an issuer discovers it at startup, so the document is here too:
            // one realm serves both the key a workload token is verified with and that discovery.
            Mock::given(method("GET"))
                .and(path(
                    "/realms/banskabystrica/.well-known/openid-configuration",
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
                    "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
                    "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
                    "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
                    "response_types_supported": ["code"],
                    "subject_types_supported": ["public"],
                    "id_token_signing_alg_values_supported": ["ES256", "RS256"],
                })))
                .mount(server)
                .await;
            (issuer, server)
        })
    })
    .join()
    .expect("the realm started");
    let (issuer, server) = issuer;
    Realm {
        issuer,
        signer,
        server,
    }
});

/// The clients the internal listener's routes belong to, as the deployment names them.
pub const AGENT_PROXY_CLIENT: &str = "helsinki-agent-proxy";
pub const PIPELINE_RUNNER_CLIENT: &str = "helsinki-pipelines";

/// What `choose_path` sends first on a conversation that starts without a path (AG-88).
pub const ROUTER_PROMPT: &str = "route a person's message to one of the assistant's paths";

/// A stub model that routes free text to no path, ahead of a test's scripted answers, so a script
/// written for the conversation's own turns reads the same as before paths (T-2693).
pub async fn route_to_no_path(proxy: &MockServer) {
    route_to(proxy, None).await;
}

/// A stub model that routes free text to `path` (an id such as `share-data`), or to none.
pub async fn route_to(proxy: &MockServer, route: Option<&str>) {
    let routed = json!({ "path": route, "reason": "a script" }).to_string();
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .and(wiremock::matchers::body_string_contains(ROUTER_PROMPT))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-router", "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": routed }, "finish_reason": "stop" }],
            "usage": { "total_tokens": 10 }
        })))
        .with_priority(1)
        .mount(proxy)
        .await;
}

/// Whether a request to the stub model is `choose_path`'s, which a test counting turns skips.
pub fn is_router(body: &[u8]) -> bool {
    String::from_utf8_lossy(body).contains(ROUTER_PROMPT)
}

/// A pipeline runner that takes every project's write budget, which the Portal declares before
/// it sends the project a stream (T-5803). The streams a test reads are mounted on top of it.
pub async fn runner() -> MockServer {
    let runner = MockServer::start().await;
    take_budgets(&runner).await;
    runner
}

/// Mounts the write budgets [`runner`] takes, again after a test resets its mocks.
pub async fn take_budgets(runner: &MockServer) {
    Mock::given(method("POST"))
        .and(path_regex(
            "^/([a-z0-9-]+/)?resources/rate_limit/[a-z0-9_]+__pipeline_egress$",
        ))
        .respond_with(ResponseTemplate::new(200))
        .mount(runner)
        .await;
}

/// What a [`runner`] was asked about its streams, without the write budgets the Portal declares
/// before them (T-5803).
pub async fn stream_requests(runner: &MockServer) -> Vec<wiremock::Request> {
    runner
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !r.url.path().contains("/resources/rate_limit/"))
        .collect()
}

/// Every `(path, text)` the commits of `POST {repo}/contents` wrote, each commit in the order it
/// lists its files.
pub async fn committed_files(gitea: &wiremock::MockServer, repo: &str) -> Vec<(String, String)> {
    use base64::Engine;
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{repo}/contents"))
        .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
        .flat_map(|commit| commit["files"].as_array().cloned().unwrap_or_default())
        .filter_map(|file| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(file["content"].as_str()?)
                .ok()?;
            Some((
                file["path"].as_str()?.to_owned(),
                String::from_utf8(bytes).ok()?,
            ))
        })
        .collect()
}

/// The paths a deletion removed in its one commit, `POST /contents` (F-R2POR3-019), sorted.
pub async fn removed_in_one_commit(gitea: &wiremock::MockServer) -> Vec<String> {
    let mut paths: Vec<String> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/contents"))
        .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
        .flat_map(|commit| commit["files"].as_array().cloned().unwrap_or_default())
        .filter(|file| file["operation"] == "delete")
        .filter_map(|file| file["path"].as_str().map(str::to_owned))
        .collect();
    paths.sort();
    paths
}
