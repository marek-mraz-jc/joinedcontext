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
        app: server::app(state),
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
    kit::functions_server().is_some()
}

/// AP-84, SDK-23: the build's own `functions.js` runs, with the caller's edge token and the
/// function's own answer coming back; the Portal's token is only ever the runtime's credential.
#[tokio::test]
async fn a_published_function_runs_the_served_build_with_the_callers_token() {
    if !runs_functions() {
        return;
    }
    let rig = rig("fn-served").await;
    let cookie = format!("__Host-jc_csrf={CSRF}");
    let (status, body) = post(
        &rig,
        "/apps/open-data/api/functions/near-me?lat=60.17",
        &[
            ("x-access-token", "the-persons-edge-token"),
            ("cookie", &cookie),
            ("x-csrf-token", CSRF),
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
    assert_eq!(call["token"], "the-persons-edge-token");
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
        // The edge's token rides on a cookie, so it needs the double-submit token beside it.
        (
            "/apps/open-data/api/functions/near-me",
            &edge[..],
            Vec::new(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/apps/open-data/api/functions/near-me",
            &edge_and_bearer[..],
            Vec::new(),
            StatusCode::FORBIDDEN,
        ),
        (
            "/apps/open-data/api/functions/near-me",
            &foreign_csrf[..],
            Vec::new(),
            StatusCode::FORBIDDEN,
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
    common::bind(&mirror, "eva", "parks");
    let state = AppState::new(config, Some(oidc)).with_mirror(std::sync::Arc::new(mirror));
    Rig {
        app: server::app(state),
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
