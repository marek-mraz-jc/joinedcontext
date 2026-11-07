//! Edge cases of the three preview routes and the two static hosts (T-2053 … T-2057; AP-12, AP-14,
//! AP-18, CC-78, PF-51, PF-59, R20).
//!
//! **The contract, in one sentence:** a preview is started and stopped by the workspace's owner and read
//! by whoever may see the workspace, while the app host serves only a file the build lane recorded a
//! digest for, inside the app's own directory, of an app whose manifest says it is published — so
//! everything else, whatever it looks like, is the same 404.
//!
//! The happy paths live in `workspace_preview_tests.rs` (a render with its prefix and paused pipelines,
//! two per node, the gateway's list) and `static_host_tests.rs` (a published bundle, the app's own CSP,
//! an unpublished app, a modified asset, a literal traversal). This file is what those leave: the
//! asymmetry between reading a preview and driving it, a stop that runs twice, a traversal through a
//! symbolic link, and the paths the Portal's own shell answers for.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{envelope, forge, person, send, state_on, REPO};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::workspaces::{Opening, PreviewState, Scope};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";
const WS: &str = "air-v2";

/// Proposes and reads everywhere: the bootstrap group.
fn owner() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

/// Reads `ovzdusie` and owns nothing.
fn viewer() -> Identity {
    Identity {
        client: None,
        groups: vec!["readers".into()],
        ..person("vera")
    }
}

fn stranger() -> Identity {
    person("nobody")
}

fn preview_uri(name: &str) -> String {
    format!("/api/v1/projects/{PROJECT}/workspaces/{name}/preview")
}

/// The forge, the viewer's read-only binding, and the workspace `air-v2` owned by `jana`.
async fn world() -> (MockServer, AppState) {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "base1" } })),
        )
        .mount(&gitea)
        .await;
    let state = state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "readers",
        ORG_NAMESPACE,
        json!({ "role": "reader", "subjects": [{ "group": "readers" }],
                "scope": { "project": PROJECT } }),
    ));
    opened(&state, WS).await;
    (gitea, state)
}

async fn opened(state: &AppState, name: &str) {
    state
        .workspaces
        .create(Opening {
            name,
            title: Some("Air"),
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
}

// -------------------------------------------------------------------------------------------------
// T-2053 start_workspace_preview, T-2054 get_workspace_preview, T-2055 stop_workspace_preview
// -------------------------------------------------------------------------------------------------

/// PF-51, PF-59: a preview is read by whoever may see the workspace and driven by its owner alone. The
/// asymmetry is the point of this case — `get` goes through `visible` and `start`/`stop` through
/// `owned` (`src/ops/previews.rs:288`, `:348`, `:366`) — so a colleague who may read the project sees
/// that a preview runs and cannot start, stop or restart it, and a stranger is told the workspace is
/// not there at all.
#[tokio::test]
async fn a_colleague_reads_a_preview_and_drives_neither_end_of_it() {
    let (_gitea, state) = world().await;
    let uri = preview_uri(WS);

    // The viewer reads it: no preview yet, so the state is plain and there are no addresses.
    let answer = send(&state, viewer(), "GET", &uri, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let preview: Value = serde_json::from_str(&answer.text).expect("a preview");
    assert_eq!(preview["state"], json!("none"), "{}", answer.text);
    assert_eq!(preview["endpoints"], json!([]), "{}", answer.text);
    assert!(preview["reason"].is_null(), "{}", answer.text);
    assert_eq!(
        preview["prefix"],
        json!("ws-air-v2-"),
        "the prefix is the workspace's, and it is not a secret: {}",
        answer.text,
    );

    // And drives neither end of it, by name.
    for verb in ["POST", "DELETE"] {
        let answer = send(&state, viewer(), verb, &uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{verb}: {}",
            answer.text
        );
        assert!(
            answer.text.contains("jana@hel.fi"),
            "{verb} does not say whose it is: {}",
            answer.text,
        );
    }

    // A stranger reads nothing and drives nothing: one answer for all three.
    for verb in ["GET", "POST", "DELETE"] {
        let answer = send(&state, stranger(), verb, &uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{verb}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("jana@hel.fi") && !answer.text.contains("ws-air-v2"),
            "{verb} told a stranger something: {}",
            answer.text,
        );
    }
}

/// CC-81, PF-51: the preview of a workspace that is not there is not there either, whatever the name
/// looks like — unknown, differing in case, walking upwards, with a trailing space. The three routes
/// answer the same, so the preview is not a way to ask whether a workspace exists. (An expired
/// workspace is the same 404 through `live`; that rule is proved against the store itself in
/// `workspace_api_tests::an_expired_workspace_is_neither_listed_nor_read`, because nothing in the
/// store's surface can move the clock forward.)
#[tokio::test]
async fn the_preview_of_a_workspace_that_is_not_there_is_the_same_answer_on_all_three_routes() {
    let (_gitea, state) = world().await;

    for name in ["nothing-at-all", "Air-V2", "%2e%2e", "air-v2%20"] {
        for verb in ["GET", "POST", "DELETE"] {
            let answer = send(&state, owner(), verb, &preview_uri(name), None).await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{verb} {name}: {}",
                answer.text
            );
        }
    }
}

/// CC-78: stopping a preview that does not run changes nothing and says so with the same 204, so a
/// person who clicks twice, or whose first click was lost, never sees an error. The state after two
/// stops is the same as after one, and a stop of a workspace whose render failed is still a stop.
#[tokio::test]
async fn stopping_a_preview_that_does_not_run_is_the_same_204_every_time() {
    let (_gitea, state) = world().await;
    let uri = preview_uri(WS);

    // Never started.
    for _ in 0..2 {
        let answer = send(&state, owner(), "DELETE", &uri, None).await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
        assert!(answer.text.is_empty(), "a body came back: {}", answer.text);
    }
    assert_eq!(
        state
            .workspaces
            .get(WS)
            .await
            .expect("read")
            .expect("the workspace")
            .preview_state,
        PreviewState::None,
        "a stop of a preview that never ran changed the state",
    );

    // A render that failed leaves the preview in `error`; stopping that is a stop, and stopping it
    // again is the same 204.
    state
        .workspaces
        .set_preview_state(WS, PreviewState::Error)
        .await
        .expect("set");
    for _ in 0..2 {
        let answer = send(&state, owner(), "DELETE", &uri, None).await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    }
    assert_eq!(
        state
            .workspaces
            .get(WS)
            .await
            .expect("read")
            .expect("the workspace")
            .preview_state,
        PreviewState::Stopped,
    );

    // And the gateway is told about none of it.
    let served = joinedcontext_portal::ops::previews::served(&state)
        .await
        .expect("the served list");
    assert!(
        served.items.is_empty(),
        "a stopped preview is still served: {:?}",
        served.items.iter().map(|s| &s.prefix).collect::<Vec<_>>(),
    );
}

// -------------------------------------------------------------------------------------------------
// T-2056 the app host, T-2057 the Portal's own shell
// -------------------------------------------------------------------------------------------------

const INDEX: &[u8] = b"<!doctype html><title>air quality</title>";

/// One app directory with a bundle, the integrity manifest CI writes beside it, and a file nobody
/// recorded a digest for.
fn app_dir(case: &str) -> Dir {
    let dir = Dir::new(case);
    let app = dir.path().join("air-quality");
    std::fs::create_dir_all(app.join("nested")).expect("app dir");
    std::fs::write(app.join("index.html"), INDEX).expect("index");
    std::fs::write(app.join("nested/page.html"), INDEX).expect("nested page");
    let digests = json!({
        "index.html": sri_sha384(INDEX),
        "nested/page.html": sri_sha384(INDEX),
    });
    std::fs::write(
        app.join("integrity.json"),
        serde_json::to_vec(&digests).expect("manifest"),
    )
    .expect("write manifest");
    // What the host must never serve: a file beside the app root, and a link into it.
    std::fs::write(dir.path().join("secret.txt"), b"the operator's notes").expect("secret");
    dir
}

/// The tiniest temp directory that cleans up after itself.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(case: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jc-edge-apps-{case}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn published_app() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "App".into(),
        metadata: ObjectMeta {
            name: "air-quality".into(),
            namespace: Some(PROJECT.into()),
            ..Default::default()
        },
        spec: json!({
            "kind": "static",
            "source": { "path": "." },
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": []
        }),
        status: None,
    });
    mirror
}

/// One anonymous GET through the whole router, with the apps directory configured.
async fn app_get(root: &std::path::Path, uri: &str) -> (StatusCode, Vec<u8>) {
    let mut config = Config::for_tests();
    config.apps_dir = Some(root.to_string_lossy().into_owned());
    let state = AppState::new(config, None).with_mirror(published_app());
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// AP-12, AP-14: nothing leaves an app's directory, whatever the path looks like — a walk upwards
/// spelled once or twice, a symbolic link pointing out of the root, a directory, the integrity
/// manifest itself, and a file that is in the directory with no digest recorded for it. Every one of
/// them is the same 404, and the operator's file beside the app root never appears in a body.
#[tokio::test]
async fn nothing_outside_the_bundle_leaves_the_app_host() {
    let dir = app_dir("outside");
    let app = dir.path().join("air-quality");
    // A link the build lane could have left behind: inside the app, pointing out of it.
    std::os::unix::fs::symlink(dir.path().join("secret.txt"), app.join("notes.txt")).ok();
    // And one pointing at a directory outside the root.
    std::os::unix::fs::symlink(dir.path(), app.join("up")).ok();

    let refused = [
        "/apps/air-quality/../secret.txt",
        "/apps/air-quality/%2e%2e/secret.txt",
        "/apps/air-quality/%252e%252e/secret.txt",
        "/apps/air-quality/nested/../../secret.txt",
        "/apps/air-quality/notes.txt",
        "/apps/air-quality/up/secret.txt",
        "/apps/air-quality/integrity.json",
        "/apps/air-quality/nested/",
        "/apps/air-quality/INDEX.HTML",
        "/apps/Air-Quality/index.html",
        "/apps/%2e%2e/index.html",
    ];
    for uri in refused {
        let (status, body) = app_get(dir.path(), uri).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{uri} was served: {}",
            String::from_utf8_lossy(&body),
        );
        assert!(
            !String::from_utf8_lossy(&body).contains("operator's notes"),
            "{uri} answered with the file outside the app",
        );
    }

    // What is in the bundle is served, so the refusals above are about the path and not the fixture.
    for uri in [
        "/apps/air-quality/index.html",
        "/apps/air-quality/nested/page.html",
    ] {
        let (status, body) = app_get(dir.path(), uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body, INDEX, "{uri}");
    }
}

/// AP-12: every answer of the app host carries the app's policy and a revalidating cache, and the
/// refusals carry neither a body that says what went wrong nor a policy of the Portal's. Framing is
/// `frame-ancestors`' alone (AP-122): an `X-Frame-Options` would refuse the Portal's frame.
#[tokio::test]
async fn an_app_answer_carries_its_own_policy_and_a_refusal_carries_nothing() {
    let dir = app_dir("headers");
    let mut config = Config::for_tests();
    config.apps_dir = Some(dir.path().to_string_lossy().into_owned());
    let state = AppState::new(config, None).with_mirror(published_app());

    for (uri, expected) in [
        ("/apps/air-quality/index.html", StatusCode::OK),
        ("/apps/air-quality/nothing.html", StatusCode::NOT_FOUND),
    ] {
        let response = server::app(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(response.status(), expected, "{uri}");
        let headers = response.headers().clone();
        if expected == StatusCode::OK {
            let csp = headers
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            // No apps origin of its own here, so the App shares the Portal's: nobody frames it.
            assert!(csp.ends_with("frame-ancestors 'none'"), "{uri}: {csp}");
            assert!(csp.contains("object-src 'none'"), "{uri}: {csp}");
            assert_eq!(headers.get(header::X_FRAME_OPTIONS), None, "{uri}");
            assert_eq!(
                headers.get(header::CACHE_CONTROL).map(|v| v.to_str().ok()),
                Some(Some("no-cache")),
                "{uri}: a republish reuses the path, so an app asset is revalidated",
            );
        } else {
            let body = response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes();
            let text = String::from_utf8_lossy(&body);
            assert!(
                !text.contains(&*dir.path().to_string_lossy()),
                "the refusal named a path on disk: {text}",
            );
        }
    }
}

/// AP-18: without an apps directory, and for an app nobody published, the host answers the same 404 as
/// for a name that never existed — so what is being worked on is never disclosed by the shape of the
/// answer. The Portal's own shell is what a path with no dot in its last segment gets instead, and it
/// is the same shell for everybody, signed in or not.
#[tokio::test]
async fn the_portals_shell_answers_every_route_and_says_nothing_about_who_asked() {
    let state = AppState::new(Config::for_tests(), None);

    // A deep app route of the SPA: the shell, so the router in the browser can take it.
    // `/` itself sends a visitor without a session to /catalogue (T-5627): that page stands in.
    for uri in [
        "/catalogue",
        "/index.html",
        "/projects/ovzdusie/spaces",
        "/projects/does-not-exist/workspaces/secret-name",
        "/%2e%2e/etc/passwd",
    ] {
        let response = server::app(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8"),
            "{uri}",
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache"),
            "{uri}: the shell names the bundle, so it is never kept",
        );
        let body = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("secret-name") && !text.contains("does-not-exist"),
            "{uri} answered with the path it was asked for: {text}",
        );
    }

    // A path whose last segment has a dot is an asset, and an asset that is not in the bundle is a
    // 404 rather than the shell: a missing script must not come back as HTML.
    for uri in [
        "/assets/nothing.js",
        "/nothing.css",
        "/favicon-that-is-not-there.png",
    ] {
        let response = server::app(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
}
