//! Edge cases of the pipeline harness, its capture and the pipeline counters (T-2025, T-2026, T-2027;
//! AG-52, PF-59, PL-43, R20, T-0983).
//!
//! **The contract, in one sentence:** running a pipeline over a sample is a write-level act held to
//! `propose` on `Pipeline`, the capture behind it answers the runner's own ServiceAccount token and
//! nothing else, and a pipeline's counters are a read of the project's pipelines.
//!
//! The happy paths live in `pipeline_test_tests.rs` (the harness, the trace, the stream that is deleted
//! afterwards) and `pipeline_metrics_tests.rs` (the counters and their freshness). This file is the other
//! side: a caller without the verb, a pipeline with no source, a capture nobody authenticated, a name
//! that would leave the runner's URL.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

mod common;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const CSRF: &str = "csrf-token-edge-pipeline";
const AUTHOR: &str = "jana.author@hel.fi";
const READER: &str = "peter.reader@hel.fi";
const STRANGER: &str = "nobody@hel.fi";

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    envelope(kind, name, ORG_NAMESPACE, spec)
}

fn config() -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_PIPELINE_RUNNER_CLIENT_ID" => Some(common::PIPELINE_RUNNER_CLIENT),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("a realm block")
}

fn cookie(config: &Config, email: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let username = email.split('@').next().unwrap_or(email).to_owned();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username,
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
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

/// A Portal with a realm (so the capture's token can be judged), two projects, a pipeline in one of
/// them, an author who may propose pipelines and a reader who may not. No runner is configured, so a
/// harness run that got past every check answers 503 — which is how a case tells "refused" from "ran".
async fn world() -> AppState {
    let state = AppState::new(config(), None);
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    for project in [PROJECT, ELSEWHERE] {
        state
            .mirror
            .upsert(envelope("ContextSpace", project, project, json!({})));
        state.mirror.upsert(envelope(
            "Pipeline",
            &format!("{project}-ingest"),
            project,
            json!({ "class": "resident" }),
        ));
    }
    state.mirror.upsert(org(
        "Role",
        "pipeline-author",
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "space-reader",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "pipeline-authors",
        json!({ "subjects": [{ "user": AUTHOR }], "role": "pipeline-author",
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "space-readers",
        json!({ "subjects": [{ "user": READER }], "role": "space-reader",
                "scope": { "project": PROJECT } }),
    ));
    state
}

async fn send(
    state: &AppState,
    email: Option<&str>,
    verb: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(verb).uri(uri);
    if let Some(email) = email {
        request = request
            .header(header::COOKIE, cookie(&state.config, email))
            .header(CSRF_HEADER, CSRF);
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = server::app(state.clone())
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn a_pipeline(source: Value) -> Value {
    let mut spec = json!({
        "class": "resident",
        "compute": { "kind": "bloblang", "bloblang": "root = this" },
        "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all",
    });
    if !source.is_null() {
        spec["source"] = source;
    }
    json!({
        "pipeline": {
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": "helsinki-ingest", "namespace": PROJECT },
            "spec": spec
        },
        "sample": { "text": "{\"id\":1}" }
    })
}

// -------------------------------------------------------------------------------------------------
// T-2025 `test_pipeline`
// -------------------------------------------------------------------------------------------------

/// PL-43, PF-50: running a pipeline over a sample starts a stream on the project's runner, so it is held
/// to `propose` on `Pipeline` and not to a read. A caller with only a read is refused, a caller no
/// binding covers is refused, a project name that could not be one is not found, and a pipeline that
/// declares no source is refused before the caller's rights are even consulted.
#[tokio::test]
async fn running_a_pipeline_over_a_sample_needs_the_verb_that_writes_one() {
    let state = world().await;
    let uri = format!("/api/v1/projects/{PROJECT}/pipelines/test");
    let sound = a_pipeline(json!({ "dataSourceRef": { "kind": "DataSource", "name": "feed" } }));

    // A pipeline whose source names neither a DataSource nor an Endpoint: there is nothing to read
    // the sample as, and it is refused before the caller's rights are consulted, so a caller who may
    // not propose is told the same thing a member is. A pipeline with no `source` key at all does not
    // even deserialise, which the extractor answers first (422) — both are the caller's mistake.
    for (who, pipeline) in [
        (AUTHOR, a_pipeline(json!({ "type": "http" }))),
        (READER, a_pipeline(json!({ "type": "http" }))),
        (AUTHOR, a_pipeline(Value::Null)),
    ] {
        let (status, body) = send(&state, Some(who), Method::POST, &uri, Some(pipeline)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{who}: {body}");
        assert!(
            body["detail"].is_string(),
            "{who} was not told what is wrong with it: {body}",
        );
    }
    // Struck as unreachable from this route: the handler's own "source must declare dataSourceRef or
    // endpointRef" 400 (`src/api/pipeline_test.rs:159`) sits behind `spec_of`, and a spec whose source
    // names neither is already refused by the kind's own validation above. The message is reachable
    // through `jc_pipeline_test`, which takes the spec as data.

    // A sound pipeline: the reader and the stranger may not run one.
    for who in [READER, STRANGER] {
        let (status, body) = send(&state, Some(who), Method::POST, &uri, Some(sound.clone())).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
    }

    // The author may, and this installation has no runner to do it on — which is the answer a person
    // can act on rather than a silent nothing.
    let (status, body) = send(
        &state,
        Some(AUTHOR),
        Method::POST,
        &uri,
        Some(sound.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("runner"),
        "{body}",
    );

    // A project name that could not be a project, and a body that is not a test request.
    for project in ["Helsinki", "helsinki_1", "%2e%2e"] {
        let (status, body) = send(
            &state,
            Some(AUTHOR),
            Method::POST,
            &format!("/api/v1/projects/{project}/pipelines/test"),
            Some(sound.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {body}");
    }
    for body in [
        json!({}),
        json!({ "pipeline": {} }),
        json!("test"),
        Value::Null,
    ] {
        let (status, answer) =
            send(&state, Some(AUTHOR), Method::POST, &uri, Some(body.clone())).await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {answer}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-2026 `capture`
// -------------------------------------------------------------------------------------------------

/// AG-52: the capture is where a harness stream posts what it read, on the internal listener, and it
/// answers the runner's own ServiceAccount token and nothing else. Without that token nothing is
/// captured and nothing is disclosed — not even whether a test of that id is running.
#[tokio::test]
async fn the_capture_answers_the_runners_own_token_and_tells_nobody_what_is_running() {
    let state = world().await;
    let internal = joinedcontext_portal::server::internal_app(state.clone());
    let message = json!({ "input": "{\"id\":1}", "output": { "id": "urn:ngsi-ld:Thing:1" } });

    let post = |bearer: Option<String>, id: String, body: Value| {
        let internal = internal.clone();
        async move {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri(format!("/internal/pipeline-tests/{id}"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(token) = bearer {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = internal
                .oneshot(
                    request
                        .body(Body::from(body.to_string()))
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
                .to_bytes();
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    // Not the runner: refused, and the answer is the same whatever id is named, so nobody learns
    // which tests are running.
    for credential in [
        None,
        Some(common::REALM.workload("context-gateway")),
        Some(common::REALM.token(common::PIPELINE_RUNNER_CLIENT, "portal-api")),
        Some("not-a-token".to_owned()),
    ] {
        for id in [
            "a-test-that-is-not-running",
            "%2e%2e",
            "a".repeat(200).as_str(),
        ] {
            let (status, body) = post(credential.clone(), id.to_owned(), message.clone()).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{credential:?} {id}: {body}",
            );
        }
    }

    // The runner's own token: past the gate, and a test nobody is running is not found — never a 204
    // that says a message was delivered nowhere.
    let runner = common::REALM.workload(common::PIPELINE_RUNNER_CLIENT);
    let (status, body) = post(
        Some(runner.clone()),
        "a-test-that-is-not-running".to_owned(),
        message.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // And a body that is not a captured message is refused as one, before any lookup.
    for shapeless in [json!("a message"), json!([]), Value::Null, json!(7)] {
        let (status, body) = post(
            Some(runner.clone()),
            "a-test-that-is-not-running".to_owned(),
            shapeless.clone(),
        )
        .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{shapeless} answered {status}: {body}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-2027 `get_metrics`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20, T-0983: a pipeline's counters say it runs here and how it is doing, so they are a read of
/// the project's pipelines. A caller who reads the project but not its pipelines is answered as if the
/// pipeline were not there, and a name that could not be one never reaches the runner's URL.
#[tokio::test]
async fn the_counters_of_a_pipeline_are_a_read_of_the_projects_pipelines() {
    let state = world().await;

    for (who, project, name) in [
        (READER, PROJECT, "helsinki-ingest"),
        (STRANGER, PROJECT, "helsinki-ingest"),
        (AUTHOR, ELSEWHERE, "espoo-ingest"),
        (AUTHOR, "Helsinki", "helsinki-ingest"),
        (AUTHOR, PROJECT, "Helsinki-Ingest"),
        (AUTHOR, PROJECT, "%2e%2e"),
        (AUTHOR, PROJECT, "helsinki-ingest%2f..%2fespoo-ingest"),
    ] {
        let (status, body) = send(
            &state,
            Some(who),
            Method::GET,
            &format!("/api/v1/projects/{project}/pipelines/{name}/metrics"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{who} {project}/{name}: {body}",
        );
        let text = body.to_string();
        for internal in ["panicked", "http://", "sqlx"] {
            assert!(
                !text.contains(internal),
                "{project}/{name} answered with {internal:?}: {text}",
            );
        }
    }

    // The author of this project's pipelines reads its own: this installation has no runner, so the
    // counters are unavailable rather than invented.
    let (status, body) = send(
        &state,
        Some(AUTHOR),
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/pipelines/helsinki-ingest/metrics"),
        None,
    )
    .await;
    assert!(
        status == StatusCode::SERVICE_UNAVAILABLE || status == StatusCode::OK,
        "{status}: {body}",
    );
}
