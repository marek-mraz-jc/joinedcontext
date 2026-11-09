//! The Portal's own App build lane (AP-130, AP-131, T-2794, jc build lane): a push to an App's
//! default branch or a Rebuild queues a build, one pass starts its Job in the build namespace
//! with a token of that build alone, and the pod reports back on the build listener.
//!
//! Every test runs against a stub that answers as both the forge and the API server (the fake
//! cluster API), so what is asserted is what leaves the Portal: a Job and a claim named after the
//! App, the token in a Secret its Job owns and only its hash in the store, one build at a time,
//! and a token that reaches its own build and nothing else. The store is a database of each
//! test's own; the tests skip when `JC_PORTAL_TEST_DATABASE_URL` is not set.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::build_pods::{dispatch_once, trigger_once, Class};
use joinedcontext_portal::apps::build_records::{self, Trigger};
use joinedcontext_portal::apps::kube::KubeClient;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request as Seen, ResponseTemplate};

mod common;

const NS: &str = "smartcity";
const PROJECT: &str = "ovzdusie";
const COMMIT: &str = "8c56954a1f0e3d2c1b0a99887766554433221100";
const NEWER: &str = "9d67065b2f1e4d3c2b1a00998877665544332211";
const BUNDLE: &[u8] = b"the bundle the build pod made";
const SBOM: &[u8] = b"{\"bomFormat\":\"CycloneDX\"}";
const NODE_WORKFLOW: &str =
    "jobs:\n  build:\n    runs-on: app-build-node\n  propose:\n    runs-on: node-22\n";

use common::build_lane::{settings, Work};

fn app_manifest(name: &str, built: Option<&str>) -> ResourceEnvelope {
    let mut app: ResourceEnvelope = serde_json::from_value(json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": {
            "kind": "static",
            "source": { "git": {
                "url": format!("https://git.example/test-owner/{PROJECT}_{name}.git"),
                "ref": "main",
            }},
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": PROJECT },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
            }],
        },
    }))
    .expect("an App");
    if let Some(commit) = built {
        app.status = serde_json::from_value(json!({ "build": {
            "digest": format!("sha256:{}", "a".repeat(64)),
            "commit": commit,
            "sdkVersion": "0.4.1",
            "builtAt": "2026-09-17T06:00:00Z",
        }}))
        .expect("a status");
    }
    app
}

/// A Portal on `server` (forge and cluster in one stub) with the build lane on `db`, holding
/// the project's space and `apps`.
fn state(
    server: &MockServer,
    db: &PgPool,
    work: &std::path::Path,
    apps: &[ResourceEnvelope],
) -> AppState {
    let mut config = Config::for_tests();
    config.build_pods = Some(settings(work));
    let shako = ShakoClient::new(
        server.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a forge client");
    let state = AppState::new(config, None)
        .with_shako(Arc::new(shako))
        .with_db(db.clone());
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        PROJECT,
        PROJECT,
        json!({ "dataModelRef": "air" }),
    ));
    for app in apps {
        state.mirror.upsert(app.clone());
    }
    state
}

fn kube(server: &MockServer) -> KubeClient {
    KubeClient::with_token(&server.uri(), "portal-sa-token").expect("a kube client")
}

fn repo(app: &str) -> String {
    format!("/odata/Repositories(Owner='test-owner',Name='{PROJECT}_{app}')")
}

/// The App's repository: default branch `main` at `head`, the template's build workflow.
async fn app_repo(server: &MockServer, app: &str, head: &str, workflow: Option<&str>) {
    let repo = repo(app);
    Mock::given(method("GET"))
        .and(path(repo.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": head })))
        .mount(server)
        .await;
    use base64::Engine;
    let file = match workflow {
        Some(text) => ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "f1",
            "Content": base64::engine::general_purpose::STANDARD.encode(text),
        })),
        None => ResponseTemplate::new(404),
    };
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", "'.gitea/workflows/build.yml'"))
        .respond_with(file)
        .mount(server)
        .await;
}

/// The build Jobs the cluster holds, and the claims (none).
async fn cluster(server: &MockServer, jobs: Value) {
    Mock::given(method("GET"))
        .and(path(format!("/apis/batch/v1/namespaces/{NS}/jobs")))
        .and(query_param(
            "labelSelector",
            "app.kubernetes.io/component=build-pod,joinedcontext.com/build-app",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": jobs })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NS}/persistentvolumeclaims"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
}

/// The Job `name` is not there at the first read and there with its uid after.
async fn job_appears(server: &MockServer, name: &str) {
    let at = format!("/apis/batch/v1/namespaces/{NS}/jobs/{name}");
    Mock::given(method("GET"))
        .and(path(at.clone()))
        .respond_with(ResponseTemplate::new(404))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "metadata": { "name": name, "uid": "0f6c-uid" } })),
        )
        .mount(server)
        .await;
}

fn body(request: &Seen) -> Value {
    serde_json::from_slice(&request.body).expect("a JSON body")
}

async fn applied(server: &MockServer) -> Vec<(String, Value)> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .map(|r| (r.url.path().to_owned(), body(r)))
        .collect()
}

/// Every request to the cluster is one the build namespace's Role `portal-build-pods` grants,
/// unchanged from the forge-driven lane (deployment components/gitea-runner/charts/build-pods):
/// Jobs and claims get, list, create (apply), patch, delete; Secrets create and patch only, so
/// never a read; nothing on pods, their logs or exec, and nothing outside the namespace.
async fn only_what_the_role_grants(server: &MockServer) {
    for request in server.received_requests().await.unwrap_or_default() {
        let path = request.url.path();
        let Some(rest) = path
            .strip_prefix(&format!("/apis/batch/v1/namespaces/{NS}/"))
            .or_else(|| path.strip_prefix(&format!("/api/v1/namespaces/{NS}/")))
        else {
            assert!(
                !path.starts_with("/api/") && !path.starts_with("/apis/"),
                "a request outside the build namespace: {} {path}",
                request.method
            );
            continue;
        };
        let resource = rest.split('/').next().unwrap_or_default();
        let verb = request.method.as_str();
        let granted = match resource {
            "jobs" | "persistentvolumeclaims" => matches!(verb, "GET" | "PATCH" | "DELETE"),
            "secrets" => verb == "PATCH",
            _ => false,
        };
        assert!(granted, "{verb} {path} is not in the Role");
    }
}

/// The token the Portal wrote into the build's Secret.
async fn token_of(server: &MockServer, job: &str) -> String {
    applied(server)
        .await
        .into_iter()
        .find(|(p, _)| p == &format!("/api/v1/namespaces/{NS}/secrets/{job}"))
        .and_then(|(_, secret)| secret["stringData"]["token"].as_str().map(str::to_owned))
        .expect("the token Secret")
}

/// A request to the build listener.
async fn listener(
    state: &AppState,
    http: &str,
    uri: &str,
    token: Option<&str>,
    body: Body,
) -> (StatusCode, String) {
    let mut request = Request::builder().method(http).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if http == "POST" {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let response = server::build_app(state.clone())
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("an answer");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Queues a build of `app` at `commit` and starts it through one pass; the build's id and token.
async fn started(
    server: &MockServer,
    db: &PgPool,
    state: &AppState,
    app: &str,
    commit: &str,
) -> (i64, String) {
    let id = build_records::enqueue(db, PROJECT, app, commit, Class::Node, Trigger::Push, None)
        .await
        .expect("queued");
    let job = format!("build-{app}-{id}");
    job_appears(server, &job).await;
    let pass = dispatch_once(
        &kube(server),
        db,
        &state.mirror,
        state.config.build_pods.as_ref().expect("lane"),
    )
    .await
    .expect("the pass");
    assert_eq!(pass.started, std::slice::from_ref(&job));
    (id, token_of(server, &job).await)
}

#[tokio::test]
async fn a_queued_build_gets_one_job_its_own_cache_and_a_token_only_its_hash_is_kept() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    cluster(&server, json!([])).await;
    let state = state(&server, &db, work.path(), &[app_manifest("bikes", None)]);
    let (id, token) = started(&server, &db, &state, "bikes", COMMIT).await;

    let applied = applied(&server).await;
    let paths: Vec<&str> = applied.iter().map(|(p, _)| p.as_str()).collect();
    let job = format!("build-bikes-{id}");
    assert_eq!(
        paths,
        [
            format!("/api/v1/namespaces/{NS}/persistentvolumeclaims/build-cache-bikes"),
            format!("/apis/batch/v1/namespaces/{NS}/jobs/{job}"),
            format!("/api/v1/namespaces/{NS}/secrets/{job}"),
            format!("/apis/batch/v1/namespaces/{NS}/jobs/{job}"),
        ],
        "the claim and the Job first, the token owned by the Job, then the Job marked"
    );
    assert!(applied[1].1["metadata"]["annotations"]
        .get("joinedcontext.com/build-token-written")
        .is_none());
    assert_eq!(
        applied[3].1["metadata"]["annotations"]["joinedcontext.com/build-token-written"],
        "true"
    );
    assert!(
        !applied[1].1.to_string().contains(&token),
        "the Job carries no token, only the name of its Secret"
    );
    assert_eq!(
        applied[2].1["metadata"]["ownerReferences"][0]["uid"],
        "0f6c-uid"
    );
    assert_eq!(token.len(), 64);

    let held = build_records::token_of(&db, id)
        .await
        .expect("read")
        .expect("a hash");
    assert_eq!(
        held,
        Sha256::digest(token.as_bytes()).to_vec(),
        "only the hash is kept"
    );
    let build = build_records::get(&db, id)
        .await
        .expect("read")
        .expect("the build");
    assert_eq!(build.state, "running");
    assert!(build
        .expires_at
        .is_some_and(|at| at > chrono::Utc::now() + chrono::Duration::minutes(20)));
    only_what_the_role_grants(&server).await;
}

#[tokio::test]
async fn a_job_left_without_its_token_is_removed_and_the_build_started_again() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    cluster(&server, json!([])).await;
    let state = state(&server, &db, work.path(), &[app_manifest("bikes", None)]);
    let id = build_records::enqueue(
        &db,
        PROJECT,
        "bikes",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
    let job = format!("build-bikes-{id}");
    let at = format!("/apis/batch/v1/namespaces/{NS}/jobs/{job}");
    // A pass that stopped half way left the Job without its token mark.
    Mock::given(method("GET"))
        .and(path(at.clone()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "metadata": { "name": job } })),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/apis/batch/v1/namespaces/{NS}/jobs/{job}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "metadata": { "name": job, "uid": "0f6c-uid" } })),
        )
        .mount(&server)
        .await;
    let pass = dispatch_once(
        &kube(&server),
        &db,
        &state.mirror,
        state.config.build_pods.as_ref().expect("lane"),
    )
    .await
    .expect("the pass");
    assert_eq!(pass.started, [job]);
    only_what_the_role_grants(&server).await;
}

#[tokio::test]
async fn a_second_build_waits_while_one_runs_and_one_app_never_runs_two() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    let running = json!([{ "metadata": { "name": "build-bikes-1" }, "status": { "active": 1 } }]);
    cluster(&server, running).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("bikes", None), app_manifest("events", None)],
    );
    let settings = state.config.build_pods.clone().expect("lane");
    build_records::enqueue(
        &db,
        PROJECT,
        "events",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");

    let pass = dispatch_once(&kube(&server), &db, &state.mirror, &settings)
        .await
        .expect("the pass");
    assert!(
        pass.started.is_empty(),
        "MAX_RUNNING is 1 per deployment: the second build waits"
    );
    assert!(applied(&server).await.is_empty());

    // One build of an App runs, a newer push waits behind it and replaces the one waiting.
    let first = build_records::enqueue(
        &db,
        PROJECT,
        "bikes",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
    assert!(
        build_records::start(&db, first, "t", chrono::Duration::minutes(25))
            .await
            .expect("started")
    );
    let waiting = build_records::enqueue(
        &db,
        PROJECT,
        "bikes",
        NEWER,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
    let again = build_records::enqueue(
        &db,
        PROJECT,
        "bikes",
        NEWER,
        Class::Node,
        Trigger::Rebuild,
        Some("jana"),
    )
    .await
    .expect("queued");
    assert_eq!(waiting, again, "one pending build per App");
    let next = build_records::next_pending(&db)
        .await
        .expect("read")
        .expect("events waits");
    assert_eq!(
        next.app, "events",
        "the App with a running build is skipped"
    );
    build_records::finish(&db, next.id, Err("gone"))
        .await
        .expect("ended");
    assert!(
        build_records::next_pending(&db)
            .await
            .expect("read")
            .is_none(),
        "bikes' second build waits until its first ends"
    );
    build_records::finish(&db, first, Ok("sha256:x"))
        .await
        .expect("ended");
    assert_eq!(
        build_records::next_pending(&db)
            .await
            .expect("read")
            .map(|b| b.id),
        Some(waiting)
    );
}

#[tokio::test]
async fn a_new_head_is_built_once_and_a_head_already_built_or_without_the_workflow_is_not() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    // bikes: never built; events: built at its head; maps: no build workflow; old: retired.
    app_repo(&server, "bikes", COMMIT, Some(NODE_WORKFLOW)).await;
    app_repo(&server, "events", COMMIT, Some(NODE_WORKFLOW)).await;
    app_repo(&server, "maps", COMMIT, None).await;
    let mut retired = app_manifest("old", None);
    retired.spec["lifecycle"] = json!("retired");
    let state = state(
        &server,
        &db,
        work.path(),
        &[
            app_manifest("bikes", None),
            app_manifest("events", Some(COMMIT)),
            app_manifest("maps", None),
            retired,
        ],
    );
    let forge = state.shako.clone().expect("a forge");

    let queued = trigger_once(&forge, &db, &state.mirror)
        .await
        .expect("the pass");
    assert_eq!(queued, [("bikes".to_owned(), COMMIT.to_owned())]);
    let build = build_records::next_pending(&db)
        .await
        .expect("read")
        .expect("queued");
    assert_eq!(
        (build.class(), build.trigger.as_str()),
        (Class::Node, "push")
    );

    // The same head is never queued twice, even after its build failed.
    build_records::finish(&db, build.id, Err("tests failed"))
        .await
        .expect("ended");
    assert!(trigger_once(&forge, &db, &state.mirror)
        .await
        .expect("the pass")
        .is_empty());
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        requests
            .iter()
            .all(|r| !r.url.path().contains("Name='ovzdusie_old'")),
        "a retired App's repository is not even read"
    );
}

#[tokio::test]
async fn a_build_whose_job_failed_or_whose_token_expired_fails_with_the_reason() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("bikes", None), app_manifest("events", None)],
    );
    let failed = build_records::enqueue(
        &db,
        PROJECT,
        "bikes",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("q");
    build_records::start(&db, failed, "t1", chrono::Duration::minutes(25))
        .await
        .expect("s");
    let expired = build_records::enqueue(
        &db,
        PROJECT,
        "events",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("q");
    build_records::start(&db, expired, "t2", chrono::Duration::seconds(-1))
        .await
        .expect("s");
    // A build the Portal was verifying when it stopped.
    let stuck = build_records::enqueue(
        &db,
        PROJECT,
        "maps",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("q");
    build_records::start(&db, stuck, "t3", chrono::Duration::minutes(-31))
        .await
        .expect("s");
    assert!(build_records::freeze(&db, stuck).await.expect("frozen"));
    cluster(
        &server,
        json!([
            { "metadata": { "name": format!("build-bikes-{failed}") },
              "status": { "conditions": [{ "type": "Failed", "status": "True", "reason": "DeadlineExceeded" }] } },
            { "metadata": { "name": format!("build-events-{expired}") }, "status": { "active": 1 } },
        ]),
    )
    .await;
    let pass = dispatch_once(
        &kube(&server),
        &db,
        &state.mirror,
        state.config.build_pods.as_ref().expect("lane"),
    )
    .await
    .expect("the pass");
    assert_eq!(pass.ended.len(), 3, "{:?}", pass.ended);
    let reason = |id| {
        let db = db.clone();
        async move {
            build_records::get(&db, id)
                .await
                .expect("read")
                .expect("there")
                .reason
                .unwrap_or_default()
        }
    };
    assert!(reason(failed).await.contains("DeadlineExceeded"));
    assert!(reason(expired).await.contains("token expired"));
    assert!(reason(stuck).await.contains("Portal stopped"));
}

#[tokio::test]
async fn the_listener_answers_a_builds_own_token_and_refuses_every_other() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    cluster(&server, json!([])).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("bikes", None), app_manifest("events", None)],
    );
    let (bikes, bikes_token) = started(&server, &db, &state, "bikes", COMMIT).await;
    build_records::finish(&db, bikes, Err("ended for this test"))
        .await
        .expect("ended");
    let (events, events_token) = started(&server, &db, &state, "events", COMMIT).await;

    let put = |id: i64, token: Option<&str>| {
        let state = state.clone();
        let token = token.map(str::to_owned);
        async move {
            listener(
                &state,
                "PUT",
                &format!("/builds/{id}/artifacts/sbom"),
                token.as_deref(),
                Body::from(SBOM),
            )
            .await
            .0
        }
    };
    assert_eq!(
        put(events, None).await,
        StatusCode::UNAUTHORIZED,
        "no token"
    );
    assert_eq!(
        put(events, Some("not-a-token")).await,
        StatusCode::NOT_FOUND,
        "a wrong token"
    );
    assert_eq!(
        put(events, Some(&bikes_token)).await,
        StatusCode::NOT_FOUND,
        "another build's token"
    );
    assert_eq!(
        put(9_999_999, Some(&events_token)).await,
        StatusCode::NOT_FOUND,
        "no such build"
    );
    assert_eq!(
        put(bikes, Some(&bikes_token)).await,
        StatusCode::CONFLICT,
        "a build that ended"
    );
    assert_eq!(
        put(events, Some(&events_token)).await,
        StatusCode::NO_CONTENT,
        "its own token"
    );
    let (status, _) = listener(
        &state,
        "PUT",
        &format!("/builds/{events}/artifacts/token"),
        Some(&events_token),
        Body::from("x"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "only bundle, image and sbom are uploaded"
    );

    // An expired token admits nothing.
    sqlx::query("UPDATE app_builds SET expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(events)
        .execute(&db)
        .await
        .expect("expired");
    assert_eq!(
        put(events, Some(&events_token)).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_upload_over_its_cap_is_refused_before_more_of_it_is_written() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    cluster(&server, json!([])).await;
    let state = state(&server, &db, work.path(), &[app_manifest("bikes", None)]);
    let (id, token) = started(&server, &db, &state, "bikes", COMMIT).await;
    let over = vec![b'x'; 16 * 1024 * 1024 + 1];
    let (status, text) = listener(
        &state,
        "PUT",
        &format!("/builds/{id}/artifacts/sbom"),
        Some(&token),
        Body::from(over),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
    // Streamed with no length: the same, and nothing is left behind.
    let chunks = futures_util::stream::iter(
        (0..17).map(|_| Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'y'; 1024 * 1024]))),
    );
    let (status, _) = listener(
        &state,
        "PUT",
        &format!("/builds/{id}/artifacts/sbom"),
        Some(&token),
        Body::from_stream(chunks),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let left: Vec<_> = std::fs::read_dir(work.path().join(id.to_string()))
        .map(|dir| dir.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    let (status, _) = listener(
        &state,
        "PUT",
        &format!("/builds/{id}/log"),
        Some(&token),
        Body::from(vec![b'l'; 1024 * 1024 + 1]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a log is at most 1 MiB"
    );
}

#[tokio::test]
async fn the_source_is_the_builds_commit_of_its_apps_repository_and_nothing_else() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = MockServer::start().await;
    cluster(&server, json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{}/Shako.ArchiveLink(Ref=@r,Format=@f)", repo("bikes"))))
        .and(query_param("@r", format!("'{COMMIT}'")))
        .and(query_param("@f", "'tar.gz'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": format!("https://public.example/git/api/v1/repos/test-owner/{PROJECT}_bikes/archive/{COMMIT}.tar.gz")
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/{PROJECT}_bikes/archive/{COMMIT}.tar.gz"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"source of bikes".to_vec()))
        .mount(&server)
        .await;
    let state = state(&server, &db, work.path(), &[app_manifest("bikes", None)]);
    let (id, token) = started(&server, &db, &state, "bikes", COMMIT).await;
    let (status, text) = listener(
        &state,
        "GET",
        &format!("/builds/{id}/source"),
        Some(&token),
        Body::empty(),
    )
    .await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "source of bikes"));
    let asked: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().contains("archive") || r.url.path().contains("Archive"))
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        asked
            .iter()
            .all(|p| p.contains(&format!("{PROJECT}_bikes"))),
        "{asked:?}"
    );
}

/// The forge the Portal publishes to and writes `status.build` into: the configuration
/// repository of `common::forge` and the App's repository at `head`.
async fn publishing_forge(app: &str, head: &str) -> MockServer {
    let server = common::forge().await;
    app_repo(&server, app, head, Some(NODE_WORKFLOW)).await;
    Mock::given(method("POST"))
        .and(path("/odata/PublishPackage"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    cluster(&server, json!([])).await;
    server
}

/// What the configuration repository was given, decoded.
async fn committed(server: &MockServer) -> Vec<String> {
    use base64::Engine;
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/Shako.CommitFiles"))
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .flat_map(|body| body["Files"].as_array().cloned().unwrap_or_default())
        .filter_map(|file| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(file.get("content")?.as_str()?)
                .ok()?;
            String::from_utf8(bytes).ok()
        })
        .collect()
}

async fn published(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/odata/PublishPackage")
        .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .collect()
}

async fn upload_both(state: &AppState, id: i64, token: &str) {
    for (name, bytes) in [("bundle", BUNDLE), ("sbom", SBOM)] {
        let (status, text) = listener(
            state,
            "PUT",
            &format!("/builds/{id}/artifacts/{name}"),
            Some(token),
            Body::from(bytes),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{name}: {text}");
    }
}

fn finished() -> Body {
    Body::from(json!({ "succeeded": true, "sdkVersion": "0.4.1" }).to_string())
}

#[tokio::test]
async fn a_finished_build_is_hashed_by_the_portal_published_and_written_as_status_build() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = publishing_forge("air-quality", COMMIT).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("air-quality", None)],
    );
    let (id, token) = started(&server, &db, &state, "air-quality", COMMIT).await;
    upload_both(&state, id, &token).await;
    let (status, text) = listener(
        &state,
        "POST",
        &format!("/builds/{id}/finish"),
        Some(&token),
        finished(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let outcome: Value = serde_json::from_str(&text).expect("json");
    let digest = format!("sha256:{:x}", Sha256::digest(BUNDLE));
    assert_eq!(outcome["state"], "succeeded", "{outcome}");
    assert_eq!(outcome["digest"], digest.as_str());

    let files: Vec<String> = published(&server)
        .await
        .iter()
        .map(|p| {
            format!(
                "{}/{}/{}",
                p["Package"].as_str().unwrap_or(""),
                p["Version"].as_str().unwrap_or(""),
                p["File"].as_str().unwrap_or("")
            )
        })
        .collect();
    let version = format!("{COMMIT}-{}", &digest[7..19]);
    assert_eq!(
        files,
        [
            format!("app-air-quality/{version}/bundle.tar.gz"),
            format!("app-air-quality/{version}/sbom.cdx.json")
        ]
    );
    let written = committed(&server).await;
    assert!(
        written
            .iter()
            .any(|f| f.contains(&digest) && f.contains(COMMIT) && f.contains("sdkVersion: 0.4.1")),
        "status.build of the digest the Portal computed: {written:?}"
    );
    let build = build_records::get(&db, id)
        .await
        .expect("read")
        .expect("the build");
    assert_eq!(
        (build.state.as_str(), build.digest.as_deref()),
        ("succeeded", Some(digest.as_str()))
    );
    assert!(
        !work.path().join(id.to_string()).exists(),
        "the uploads go once published"
    );

    // The build is over: its token admits nothing more, not even another finish.
    let (status, _) = listener(
        &state,
        "POST",
        &format!("/builds/{id}/finish"),
        Some(&token),
        finished(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_token_of_one_build_never_writes_another_apps_status() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = publishing_forge("events", COMMIT).await;
    app_repo(&server, "bikes", COMMIT, Some(NODE_WORKFLOW)).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("bikes", None), app_manifest("events", None)],
    );
    let (bikes, bikes_token) = started(&server, &db, &state, "bikes", COMMIT).await;
    build_records::finish(&db, bikes, Err("ended for this test"))
        .await
        .expect("ended");
    let (events, events_token) = started(&server, &db, &state, "events", COMMIT).await;
    upload_both(&state, events, &events_token).await;

    for (status, _) in [
        listener(
            &state,
            "POST",
            &format!("/builds/{events}/finish"),
            Some(&bikes_token),
            finished(),
        )
        .await,
        listener(
            &state,
            "PUT",
            &format!("/builds/{events}/artifacts/bundle"),
            Some(&bikes_token),
            Body::from("evil"),
        )
        .await,
    ] {
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert!(
        committed(&server).await.is_empty(),
        "nothing written for events"
    );
    assert!(
        published(&server).await.is_empty(),
        "nothing published for events"
    );
    assert_eq!(
        build_records::get(&db, events)
            .await
            .expect("r")
            .expect("b")
            .state,
        "running"
    );
}

#[tokio::test]
async fn an_upload_admitted_before_the_freeze_never_lands_after_it() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = publishing_forge("air-quality", COMMIT).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("air-quality", None)],
    );
    let (id, _) = started(&server, &db, &state, "air-quality", COMMIT).await;
    // An upload the listener admitted while the build ran lands only if the build still runs
    // when its rename comes: the row lock orders it against the freeze.
    let mut landed = false;
    assert!(build_records::while_running(&db, id, || {
        landed = true;
        Ok(())
    })
    .await
    .expect("held"));
    assert!(landed);
    assert!(build_records::freeze(&db, id).await.expect("frozen"));
    let mut late = false;
    assert!(!build_records::while_running(&db, id, || {
        late = true;
        Ok(())
    })
    .await
    .expect("held"));
    assert!(!late, "a rename after the freeze never runs");
}

#[tokio::test]
async fn an_upload_after_the_finish_never_lands_and_a_moved_head_is_not_published() {
    let db = db_or_skip!();
    let work = Work::new();
    // The head moved on while the build ran.
    let server = publishing_forge("air-quality", NEWER).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("air-quality", None)],
    );
    let (id, token) = started(&server, &db, &state, "air-quality", COMMIT).await;
    upload_both(&state, id, &token).await;
    assert!(build_records::freeze(&db, id).await.expect("frozen"));
    let (status, _) = listener(
        &state,
        "PUT",
        &format!("/builds/{id}/artifacts/bundle"),
        Some(&token),
        Body::from("swapped"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "frozen: no upload lands once the Portal hashes"
    );
    assert_eq!(
        std::fs::read(work.path().join(id.to_string()).join("bundle")).expect("kept"),
        BUNDLE
    );

    // A second build of the same App, whose head moved: refused, nothing published.
    sqlx::query("UPDATE app_builds SET state = 'failed' WHERE id = $1")
        .bind(id)
        .execute(&db)
        .await
        .expect("ended");
    let (again, token) = started(&server, &db, &state, "air-quality", COMMIT).await;
    upload_both(&state, again, &token).await;
    let (status, text) = listener(
        &state,
        "POST",
        &format!("/builds/{again}/finish"),
        Some(&token),
        finished(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        text.contains("\"failed\"") && text.contains(NEWER),
        "{text}"
    );
    assert!(published(&server).await.is_empty());
    assert!(committed(&server).await.is_empty());
}

#[tokio::test]
async fn a_build_that_failed_or_left_no_sbom_publishes_nothing() {
    let db = db_or_skip!();
    let work = Work::new();
    let server = publishing_forge("air-quality", COMMIT).await;
    let state = state(
        &server,
        &db,
        work.path(),
        &[app_manifest("air-quality", None)],
    );
    let (id, token) = started(&server, &db, &state, "air-quality", COMMIT).await;
    let failed =
        Body::from(json!({ "succeeded": false, "reason": "vitest: 2 failed" }).to_string());
    let (status, text) = listener(
        &state,
        "POST",
        &format!("/builds/{id}/finish"),
        Some(&token),
        failed,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        build_records::get(&db, id)
            .await
            .expect("r")
            .expect("b")
            .reason
            .as_deref(),
        Some("vitest: 2 failed")
    );

    sqlx::query("UPDATE app_builds SET commit = 'x' WHERE id = $1")
        .bind(id)
        .execute(&db)
        .await
        .expect("moved");
    let (again, token) = started(&server, &db, &state, "air-quality", COMMIT).await;
    let (status, _) = listener(
        &state,
        "PUT",
        &format!("/builds/{again}/artifacts/bundle"),
        Some(&token),
        Body::from(BUNDLE),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, text) = listener(
        &state,
        "POST",
        &format!("/builds/{again}/finish"),
        Some(&token),
        finished(),
    )
    .await;
    assert!(text.contains("no sbom"), "{text}");
    assert!(published(&server).await.is_empty());
}
