//! `POST /apps/{name}/api/functions/{fn}`: a published application's function runs in
//! `jc-functions` with the caller's own token (AP-84, SDK-23, AP-26).
//!
//! The runtime and the realm are fakes; the bundle on disk is what the lane builds, with its
//! integrity manifest. What is asserted is what reaches the runtime: the build's `functions.js`
//! and never a body-supplied source, the edge's token and never the Portal's, no token at all for
//! an anonymous caller, and the refusals that never reach it.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::agents::kit;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const BUNDLE: &str = "export { default as \"near-me\" } from \"./near-me.js\";";
const CSRF: &str = "apps-origin-csrf";

/// A realm that hands the Portal its own service token, and a runtime that answers every call.
async fn realm_and_runtime() -> (MockServer, MockServer) {
    let realm = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&realm)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/helsinki/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(&realm)
        .await;
    let runtime = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 200, "body": { "stations": 3 }, "logs": ["near-me 3"]
        })))
        .mount(&runtime)
        .await;
    (realm, runtime)
}

/// `{apps_dir}/{app}/` holding an index and `functions.js`, both in its integrity manifest.
fn bundle_dir(root: &std::path::Path, app: &str, functions: &str) {
    let dir = root.join(app);
    std::fs::create_dir_all(&dir).expect("app dir");
    let index = b"<!doctype html><title>app</title>";
    std::fs::write(dir.join("index.html"), index).expect("index");
    std::fs::write(dir.join("functions.js"), functions).expect("functions");
    let integrity = json!({
        "index.html": sri_sha384(index),
        "functions.js": sri_sha384(BUNDLE.as_bytes()),
    });
    std::fs::write(dir.join("integrity.json"), integrity.to_string()).expect("integrity");
}

fn app(name: &str, visibility: &str) -> joinedcontext_portal::resource::ResourceEnvelope {
    common::envelope(
        "App",
        name,
        "helsinki",
        json!({
            "kind": "static",
            "source": { "path": "." },
            "build": {},
            "visibility": visibility,
            "lifecycle": "published",
            "dataNeeds": []
        }),
    )
}

struct Rig {
    app: axum::Router,
    state: AppState,
    runtime: MockServer,
    /// The suite's own realm; `None` where the process's signing realm stands in.
    _realm: Option<MockServer>,
    _root: Scratch,
}

/// `test` names the scratch directory, so tests running at once never share one.
async fn rig(test: &str) -> Rig {
    let (realm, runtime) = realm_and_runtime().await;
    let root = Scratch::new(test);
    bundle_dir(root.path(), "open-data", BUNDLE);
    bundle_dir(root.path(), "members", BUNDLE);
    bundle_dir(
        root.path(),
        "tampered",
        "export default () => ({ status: 200, body: 'not the build' });",
    );
    let issuer = format!("{}/realms/helsinki", realm.uri());
    let runtime_uri = runtime.uri();
    let apps_dir = root.path().to_string_lossy().into_owned();
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_FUNCTIONS_URL" => Some(runtime_uri.as_str()),
            "JC_TRUST_EDGE_TOKEN" => Some("true"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.apps_dir = Some(apps_dir);
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let mirror = Mirror::new();
    mirror.upsert(app("open-data", "public"));
    mirror.upsert(app("members", "project"));
    mirror.upsert(app("tampered", "public"));
    let state = AppState::new(config, Some(oidc)).with_mirror(std::sync::Arc::new(mirror));
    Rig {
        app: server::app(state.clone()),
        state,
        runtime,
        _realm: Some(realm),
        _root: root,
    }
}

async fn post(
    rig: &Rig,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = rig
        .app
        .clone()
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn invocations(runtime: &MockServer) -> Vec<Value> {
    runtime
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("an invocation"))
        .collect()
}

/// The server module is embedded from `sdk/dist`; a Portal built without it says 503, which the
/// preview's own case covers, and nothing below would reach the runtime.
fn runs_functions() -> bool {
    kit::sdk_built_or_skip(kit::functions_server().is_some(), "functions-server.js")
}

/// AP-84, SDK-23: the build's own `functions.js` runs, with the caller's edge token and the
/// function's own answer coming back; the Portal's token is only ever the runtime's credential.
#[tokio::test]
async fn a_published_function_runs_the_served_build_with_the_callers_token() {
    if !runs_functions() {
        return;
    }
    let (_realm, runtime) = realm_and_runtime().await;
    let rig = organization_rig("fn-served", runtime).await;
    let eva = common::REALM.person_token("bbsk-open", "eva", &[]);
    let csrf = index_csrf(&rig, "bbsk-open", &eva).await;
    let cookie = format!("__Host-jc_csrf={csrf}");
    let (status, body) = post(
        &rig,
        "/apps/bbsk-open/api/functions/near-me?lat=60.17",
        &[
            ("x-access-token", &eva),
            ("cookie", &cookie),
            ("x-csrf-token", &csrf),
        ],
        br#"{"source":"export default () => 1","radius":500}"#.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "stations": 3 }));

    let [call] = invocations(&rig.runtime)
        .await
        .try_into()
        .expect("one invocation");
    assert_eq!(call["token"], eva);
    assert_eq!(
        call["files"]["@app/functions.js"], BUNDLE,
        "the served build, not the body"
    );
    assert_eq!(call["entry"], "@app/entry.js");
    assert!(call["files"]["@app/entry.js"]
        .as_str()
        .unwrap_or_default()
        .contains("\"near-me\""));
    assert_eq!(call["request"]["query"], json!({ "lat": "60.17" }));
    assert_eq!(call["request"]["body"]["radius"], 500);
    assert!(
        !call.to_string().contains("the-portals-own-token"),
        "the Portal's token stays in the header"
    );
}

/// AP-26, AP-84: a public app's anonymous caller reaches the runtime with no token at all.
#[tokio::test]
async fn an_anonymous_call_to_a_public_app_carries_no_token() {
    if !runs_functions() {
        return;
    }
    let rig = rig("fn-anonymous").await;
    let (status, _) = post(
        &rig,
        "/apps/open-data/api/functions/near-me",
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let [call] = invocations(&rig.runtime)
        .await
        .try_into()
        .expect("one invocation");
    assert_eq!(call["token"], Value::Null);
    assert_eq!(call["request"]["body"], Value::Null);
}

/// AP-84: every refusal below is answered before the runtime is asked anything.
#[tokio::test]
async fn refusals_never_reach_the_runtime() {
    let rig = rig("fn-refusals").await;
    let edge = [("x-access-token", "the-persons-edge-token")];
    // The edge sets its token from the session cookie whatever else the request carries, so a
    // bearer beside it is no credential of this call and waives no CSRF check (T-4915).
    let edge_and_bearer = [
        ("x-access-token", "the-persons-edge-token"),
        ("authorization", "Bearer planted-by-another-app"),
    ];
    // A token the page could not read off this App's own host never matches its cookie.
    let foreign_csrf = [
        ("x-access-token", "the-persons-edge-token"),
        ("cookie", "__Host-jc_csrf=this-apps-token"),
        ("x-csrf-token", "another-apps-token"),
    ];
    for (uri, headers, body, expected) in [
        // A name no function file could have.
        (
            "/apps/open-data/api/functions/Near_Me",
            &[][..],
            Vec::new(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/apps/open-data/api/functions/-x",
            &[][..],
            Vec::new(),
            StatusCode::NOT_FOUND,
        ),
        // An app nobody published.
        (
            "/apps/nothing/api/functions/near-me",
            &[][..],
            Vec::new(),
            StatusCode::NOT_FOUND,
        ),
        // A project app to someone with no session is the same 404 as a missing one (AP-18).
        (
            "/apps/members/api/functions/near-me",
            &[][..],
            Vec::new(),
            StatusCode::NOT_FOUND,
        ),
        // An edge token this realm cannot verify names nobody: 401 before any pair is read,
        // whatever rides beside it (S1-52). The CSRF refusals of a verified person's token are
        // an_app_hosts_call_needs_the_token_bound_to_its_person_and_app.
        (
            "/apps/open-data/api/functions/near-me",
            &edge[..],
            Vec::new(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/apps/open-data/api/functions/near-me",
            &edge_and_bearer[..],
            Vec::new(),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/apps/open-data/api/functions/near-me",
            &foreign_csrf[..],
            Vec::new(),
            StatusCode::UNAUTHORIZED,
        ),
        // Not JSON.
        (
            "/apps/open-data/api/functions/near-me",
            &[][..],
            b"{".to_vec(),
            StatusCode::BAD_REQUEST,
        ),
        // More than the runtime takes.
        (
            "/apps/open-data/api/functions/near-me",
            &[][..],
            vec![b' '; 256 * 1024 + 1],
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let (status, body) = post(&rig, uri, headers, body).await;
        assert_eq!(status, expected, "{uri}: {body}");
    }
    assert!(invocations(&rig.runtime).await.is_empty());
}

/// AP-12, AP-84: a `functions.js` that is not the bytes the lane recorded never runs.
#[tokio::test]
async fn a_bundle_that_fails_its_integrity_digest_never_runs() {
    let rig = rig("fn-tampered").await;
    let (status, _) = post(
        &rig,
        "/apps/tampered/api/functions/near-me",
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(invocations(&rig.runtime).await.is_empty());
}

/// A published App with no `functions.js`, a bundle that is no UTF-8 although it matches its
/// digest, and a Portal that serves no App directory all answer the same 404 as a missing
/// function, and the runtime is never asked.
#[tokio::test]
async fn an_app_without_a_readable_bundle_answers_404() {
    let rig = rig("fn-unreadable").await;
    let root = rig.state.config.apps_dir.clone().expect("an apps dir");
    let bare = std::path::Path::new(&root).join("open-data-bare");
    std::fs::create_dir_all(&bare).expect("app dir");
    std::fs::write(bare.join("index.html"), "<!doctype html>").expect("index");
    let binary = std::path::Path::new(&root).join("open-data-binary");
    std::fs::create_dir_all(&binary).expect("app dir");
    let bytes = [0xff_u8, 0xfe, 0x00, 0x80];
    std::fs::write(binary.join("functions.js"), bytes).expect("bundle");
    std::fs::write(
        binary.join("integrity.json"),
        json!({ "functions.js": sri_sha384(&bytes) }).to_string(),
    )
    .expect("integrity");
    rig.state.mirror.upsert(app("open-data-bare", "public"));
    rig.state.mirror.upsert(app("open-data-binary", "public"));
    for name in ["open-data-bare", "open-data-binary"] {
        let (status, body) = post(
            &rig,
            &format!("/apps/{name}/api/functions/near-me"),
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
        assert_eq!(
            body["detail"],
            format!("app '{name}' has no function 'near-me'")
        );
    }

    let mut config = (*rig.state.config).clone();
    config.apps_dir = None;
    let mut state = rig.state.clone();
    state.config = std::sync::Arc::new(config);
    let no_dir = Rig {
        app: server::app(state.clone()),
        state,
        runtime: MockServer::start().await,
        _realm: None,
        _root: Scratch::new("fn-unreadable-none"),
    };
    let (status, _) = post(
        &no_dir,
        "/apps/open-data/api/functions/near-me",
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(invocations(&rig.runtime).await.is_empty());
}

/// With an apps origin configured, a call that does not come in on the App's own host is sent
/// there with its path and query, and a name that is no DNS label has no origin at all.
#[tokio::test]
async fn a_call_off_the_apps_own_origin_is_sent_there() {
    let rig = rig("fn-origin").await;
    let mut config = (*rig.state.config).clone();
    config.apps_url = Some("https://example.org".parse().expect("a url"));
    let mut state = rig.state.clone();
    state.config = std::sync::Arc::new(config);
    let app = server::app(state);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/apps/open-data/api/functions/near-me?lat=60.17")
                .header("host", "portal.example.org")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(response.status().is_redirection(), "{}", response.status());
    assert_eq!(
        response.headers()["location"],
        "https://open-data.apps.example.org/api/functions/near-me?lat=60.17"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/apps/Not_A_Label/api/functions/near-me")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(invocations(&rig.runtime).await.is_empty());
}

/// A directory of this test process, removed when it goes out of scope.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("jc-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A Portal over the process's signing realm, so a person's token of App `app-{name}` verifies,
/// with `runtime` as jc-functions and App `bbsk-events` shared with the organization. Eva holds a
/// binding in project `parks` only: an organization member with no grant on the App's space.
async fn organization_rig(test: &str, runtime: MockServer) -> Rig {
    Mock::given(method("POST"))
        .and(path("/realms/banskabystrica/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(common::REALM.server)
        .await;
    let root = Scratch::new(test);
    bundle_dir(root.path(), "bbsk-events", BUNDLE);
    bundle_dir(root.path(), "bbsk-open", BUNDLE);
    let runtime_uri = runtime.uri();
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_FUNCTIONS_URL" => Some(runtime_uri.as_str()),
            "JC_TRUST_EDGE_TOKEN" => Some("true"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.apps_dir = Some(root.path().to_string_lossy().into_owned());
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let mirror = Mirror::new();
    mirror.upsert(app("bbsk-events", "organization"));
    mirror.upsert(app("bbsk-open", "public"));
    common::bind(&mirror, "eva", "parks");
    common::bind(&mirror, "otto", "parks");
    let state = AppState::new(config, Some(oidc)).with_mirror(std::sync::Arc::new(mirror));
    Rig {
        app: server::app(state.clone()),
        state,
        runtime,
        _realm: None,
        _root: root,
    }
}

/// T-4912 exploit: an organization member without rights on the App's space calls its function.
/// The runtime gets Eva's own token and never the Portal's or an App account's, so the Endpoint
/// refuses her data call; that refusal comes back as her 403, which the page shows as no access.
#[tokio::test]
async fn an_organization_apps_function_acts_as_the_viewer_and_answers_her_refusal() {
    if !runs_functions() {
        return;
    }
    let runtime = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 403,
            "body": { "type": "https://joinedcontext.com/errors/forbidden", "title": "Access Denied by Policy", "status": 403 },
            "logs": []
        })))
        .mount(&runtime)
        .await;
    let rig = organization_rig("fn-organization", runtime).await;
    // The edge's token on an App's host is the App's own (audience app-bbsk-events); the call
    // echoes the CSRF token the App's index handed her for it (S1-52).
    let eva = common::REALM.person_token("bbsk-events", "eva", &[]);
    let csrf = index_csrf(&rig, "bbsk-events", &eva).await;
    let cookie = format!("__Host-jc_csrf={csrf}");
    let (status, body) = post(
        &rig,
        "/apps/bbsk-events/api/functions/near-me",
        &[
            ("x-access-token", &eva),
            ("cookie", &cookie),
            ("x-csrf-token", &csrf),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["title"], "Access Denied by Policy");

    let [call] = invocations(&rig.runtime)
        .await
        .try_into()
        .expect("one invocation");
    assert_eq!(
        call["token"], eva,
        "the viewer's own token, as the edge set it"
    );
    assert_eq!(call["request"]["user"]["id"], "sub-eva");
    assert!(call["files"]["@app/entry.js"]
        .as_str()
        .unwrap_or_default()
        .contains("answeringRefusals(found)"));
    assert!(
        !call.to_string().contains("the-portals-own-token"),
        "the Portal's token is the runtime's credential alone"
    );
}

/// T-7837: each refusal of jc-functions reaches the caller as its own status through the real
/// route: full 429 with Retry-After, too large 413, any other status or an answer that is no
/// outcome 503.
#[tokio::test]
async fn each_runtime_refusal_reaches_the_caller_as_its_status() {
    if !runs_functions() {
        return;
    }
    for (at, (runtime_answer, want)) in [
        (ResponseTemplate::new(429), StatusCode::TOO_MANY_REQUESTS),
        (ResponseTemplate::new(413), StatusCode::PAYLOAD_TOO_LARGE),
        (ResponseTemplate::new(500), StatusCode::SERVICE_UNAVAILABLE),
        (
            ResponseTemplate::new(200).set_body_string("not an outcome"),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let runtime = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/invoke"))
            .respond_with(runtime_answer)
            .mount(&runtime)
            .await;
        let rig = organization_rig(&format!("fn-refused-{at}"), runtime).await;
        let eva = common::REALM.person_token("bbsk-events", "eva", &[]);
        let cookie = format!("__Host-jc_csrf={CSRF}");
        let (status, body) = post(
            &rig,
            "/apps/bbsk-events/api/functions/near-me",
            &[
                ("x-access-token", &eva),
                ("cookie", &cookie),
                ("x-csrf-token", CSRF),
            ],
            Vec::new(),
        )
        .await;
        assert_eq!(status, want, "{body}");
        assert_eq!(invocations(&rig.runtime).await.len(), 1);
    }
}

/// RTE-12: an edge token that names a Portal session binds the CSRF token to that person, so
/// the call passes with the token the Portal handed her and is refused with a plain
/// double-submit pair any page on the host could have set.
#[tokio::test]
async fn a_portal_sessions_call_needs_the_token_bound_to_her() {
    if !runs_functions() {
        return;
    }
    let runtime = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 200, "body": { "events": 2 }, "logs": []
        })))
        .mount(&runtime)
        .await;
    let rig = organization_rig("fn-bound", runtime).await;
    let eva = common::REALM.person_token_of("joinedcontext-portal", "edge", "eva", &[]);
    let bound = common::csrf_echo::edge_csrf(&rig.state, &eva).await;
    let cookie = format!("__Host-jc_csrf={CSRF}");
    let (status, _) = post(
        &rig,
        "/apps/bbsk-open/api/functions/near-me",
        &[
            ("x-access-token", &eva),
            ("cookie", &cookie),
            ("x-csrf-token", CSRF),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(invocations(&rig.runtime).await.is_empty());

    let cookie = format!("__Host-jc_csrf={bound}");
    let (status, body) = post(
        &rig,
        "/apps/bbsk-open/api/functions/near-me",
        &[
            ("x-access-token", &eva),
            ("cookie", &cookie),
            ("x-csrf-token", &bound),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "events": 2 }));
    let [call] = invocations(&rig.runtime)
        .await
        .try_into()
        .expect("one invocation");
    assert_eq!(call["token"], eva);
}

/// The CSRF token the App's index hands the person `edge_token` names, from its `Set-Cookie`.
async fn index_csrf(rig: &Rig, app: &str, edge_token: &str) -> String {
    let response = rig
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/apps/{app}/"))
                .header("x-access-token", edge_token)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| cookie.strip_prefix("__Host-jc_csrf=")?.split(';').next())
        .expect("the index hands a CSRF cookie")
        .to_owned()
}

/// `(status, problem type)` of a POST to `uri` with `token` as the edge's and `csrf` echoed.
async fn call_with(rig: &Rig, uri: &str, token: &str, csrf: Option<&str>) -> (StatusCode, String) {
    call_as(rig, "POST", uri, token, csrf).await
}

async fn call_as(
    rig: &Rig,
    method: &str,
    uri: &str,
    token: &str,
    csrf: Option<&str>,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-access-token", token);
    if let Some(csrf) = csrf {
        request = request
            .header("cookie", format!("__Host-jc_csrf={csrf}"))
            .header("x-csrf-token", csrf);
    }
    let response = rig
        .app
        .clone()
        .oneshot(request.body(Body::from("{}")).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let kind = body["type"].as_str().unwrap_or_default();
    (
        status,
        kind.rsplit('/').next().unwrap_or_default().to_owned(),
    )
}

/// RTE-12, S1-52: on an App's host the CSRF token is bound to the person and the App the edge's
/// token names. Otto's call with the token minted for Eva is refused, as is Eva's with a plain
/// pair any page on the host could set, or with her token of another App; her own passes.
#[tokio::test]
async fn an_app_hosts_call_needs_the_token_bound_to_its_person_and_app() {
    if !runs_functions() {
        return;
    }
    let runtime = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 200, "body": { "events": 2 }, "logs": []
        })))
        .mount(&runtime)
        .await;
    let rig = organization_rig("fn-app-bound", runtime).await;
    let uri = "/apps/bbsk-events/api/functions/near-me";
    let eva = common::REALM.person_token("bbsk-events", "eva", &[]);
    let otto = common::REALM.person_token("bbsk-events", "otto", &[]);
    let evas = index_csrf(&rig, "bbsk-events", &eva).await;
    let ottos = index_csrf(&rig, "bbsk-events", &otto).await;
    assert_ne!(evas, ottos, "each person's token is their own");
    let eva_open = common::REALM.person_token("bbsk-open", "eva", &[]);
    let evas_open = index_csrf(&rig, "bbsk-open", &eva_open).await;
    assert_ne!(evas, evas_open, "one person's token differs per App");

    for (who, token, csrf) in [
        ("otto with eva's", &otto, Some(evas.as_str())),
        ("eva with the anonymous pair", &eva, Some(CSRF)),
        (
            "eva with her token of another App",
            &eva,
            Some(evas_open.as_str()),
        ),
        ("eva without a token", &eva, None),
    ] {
        let (status, kind) = call_with(&rig, uri, token, csrf).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}");
        assert_eq!(kind, "forbidden", "{who}");
    }
    // A bearer beside the edge's token is no credential of this call and waives no check
    // (T-4915).
    let (status, _) = post(
        &rig,
        "/apps/bbsk-open/api/functions/near-me",
        &[
            ("x-access-token", &eva_open),
            ("authorization", "Bearer planted-by-another-app"),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "eva with a bearer and no token"
    );
    assert!(invocations(&rig.runtime).await.is_empty());

    let (status, _) = call_with(&rig, uri, &eva, Some(&evas)).await;
    assert_eq!(status, StatusCode::OK, "eva with her own token");
    let (status, _) = call_with(&rig, uri, &otto, Some(&ottos)).await;
    assert_eq!(status, StatusCode::OK, "otto with his own token");
    let calls = invocations(&rig.runtime).await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["token"], eva);
    assert_eq!(calls[1]["token"], otto);
}

/// An edge token that verifies neither as the App's nor as the Portal's names nobody: `401`,
/// and no double-submit pair stands in for it.
#[tokio::test]
async fn an_expired_or_foreign_edge_token_is_401_whatever_pair_it_carries() {
    let runtime = MockServer::start().await;
    let rig = organization_rig("fn-expired", runtime).await;
    let now = joinedcontext_portal::auth::session::now_unix();
    let expired = common::REALM.person_token_with(
        "app-bbsk-open",
        "app-bbsk-open",
        "eva",
        &[],
        json!({ "exp": now - 600, "iat": now - 900 }),
    );
    for token in [expired.as_str(), "not-a-token"] {
        let (status, kind) = call_with(
            &rig,
            "/apps/bbsk-open/api/functions/near-me",
            token,
            Some(CSRF),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(kind, "unauthorized");
    }
    assert!(invocations(&rig.runtime).await.is_empty());
}

/// Every App-host service write goes through `services::gate`, which binds the CSRF token the
/// same way: a cross-person pair is `403 forbidden` on each, and the person's own pair passes
/// the check (the App turns no service on, so the gate's next step answers `service-off`).
#[tokio::test]
async fn every_service_write_refuses_another_persons_csrf_token() {
    let runtime = MockServer::start().await;
    let rig = organization_rig("svc-app-bound", runtime).await;
    let eva = common::REALM.person_token("bbsk-events", "eva", &[]);
    let otto = common::REALM.person_token("bbsk-events", "otto", &[]);
    let evas = index_csrf(&rig, "bbsk-events", &eva).await;
    for (method, uri) in [
        ("POST", "/apps/bbsk-events/api/services/email/send"),
        ("POST", "/apps/bbsk-events/api/services/ai/complete"),
        ("PUT", "/apps/bbsk-events/api/services/files/report.csv"),
        ("DELETE", "/apps/bbsk-events/api/services/files/report.csv"),
        (
            "POST",
            "/apps/bbsk-events/api/services/files/report.csv:url",
        ),
    ] {
        let (status, kind) = call_as(&rig, method, uri, &otto, Some(&evas)).await;
        assert_eq!(
            (status, kind.as_str()),
            (StatusCode::FORBIDDEN, "forbidden"),
            "{method} {uri}: otto with eva's token"
        );
        let (_, kind) = call_as(&rig, method, uri, &eva, Some(&evas)).await;
        assert_eq!(
            kind, "service-off",
            "{method} {uri}: eva with her own token"
        );
    }
}
