//! Edge cases of the import and the three writes (T-2018, T-2019, T-2020, T-2021; CC-19, MF-04, MF-13,
//! MF-18, MF-20, MF-24, PF-50, R20).
//!
//! **The contract, in one sentence:** an import and a proposal are writes into one project, held to
//! `propose` on the kind, refused for a status block, a literal credential, a namespace that is not the
//! project and a bundle that is not one — and every refusal happens before the forge is asked for
//! anything.
//!
//! The happy paths live in `import_api_tests.rs` (the archive that becomes one merge request, the
//! provenance, the dry run) and `resource_mutate_tests.rs` (the 202, the lanes, the open change, the
//! quota, the files). This file is the other side: an import from a URL, a bundle that is noise, a
//! caller without the verb, a patch of what is not there.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";
const ELSEWHERE: &str = "doprava";
const CSRF: &str = "csrf-token-edge-writes";
const AUTHOR: &str = "jana.author@banskabystrica.sk";
const READER: &str = "peter.reader@banskabystrica.sk";

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

/// One project with a space and an endpoint, an author who may propose both kinds, a reader who may
/// not, and a forge with nothing mounted so a case can prove a refusal never asked it for a file.
async fn world() -> (MockServer, AppState) {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    state
        .mirror
        .upsert(envelope("ContextSpace", "mobility", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "Endpoint",
        "mobility-bikes",
        PROJECT,
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" },
            "slug": "si6epqkx364lprho5uaigutk274r5grb",
            "audience": "project",
            "enabledRepresentations": ["ngsi-ld"],
        }),
    ));
    state.mirror.upsert(org(
        "Role",
        "author",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "reader",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "authors",
        json!({ "subjects": [{ "user": AUTHOR }], "role": "author",
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "readers",
        json!({ "subjects": [{ "user": READER }], "role": "reader",
                "scope": { "project": PROJECT } }),
    ));
    (forge, state)
}

async fn send(
    state: &AppState,
    email: &str,
    verb: Method,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(verb)
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email))
        .header(CSRF_HEADER, CSRF);
    if !content_type.is_empty() {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::from(body)).expect("a request"))
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

fn a_space(name: &str, namespace: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": namespace },
        "spec": { "isSandbox": true }
    })
}

// -------------------------------------------------------------------------------------------------
// T-2018 `import`
// -------------------------------------------------------------------------------------------------

/// MF-18, MF-20: an import is a write of one project from a bundle the caller uploads. A URL is refused
/// by name — fetching one would be the Portal opening a connection to a host a caller chose, with no
/// egress policy behind it — a body that is not a bundle is refused as one, and a caller who may read
/// the project but not propose into it imports nothing. Nothing reaches the forge.
#[tokio::test]
async fn an_import_takes_an_uploaded_bundle_from_a_caller_who_may_propose_and_nothing_else() {
    let (forge, state) = world().await;
    let uri = format!("/api/v1/projects/{PROJECT}/import");

    // A URL instead of an upload: refused, and the refusal says why rather than fetching it (MF-20).
    let (status, body) = send(
        &state,
        AUTHOR,
        Method::POST,
        &uri,
        "application/json",
        json!({ "url": "http://169.254.169.254/latest/meta-data/" })
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("egress"),
        "the refusal does not say what is missing: {body}",
    );

    // A body that is not a bundle: noise, an empty upload, a zip header with nothing behind it, a
    // JSON document that is not a manifest.
    for (content_type, bytes) in [
        ("application/zip", b"not a zip at all".to_vec()),
        ("application/zip", Vec::new()),
        ("application/zip", vec![0x50, 0x4b, 0x03, 0x04]),
        ("application/json", b"{}".to_vec()),
        ("application/json", b"[]".to_vec()),
        ("application/json", b"garbage".to_vec()),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            Method::POST,
            &uri,
            content_type,
            bytes.clone(),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{content_type} {} bytes answered {status}: {answer}",
            bytes.len(),
        );
    }

    // A caller who reads the project but may not propose into it.
    let (status, _) = send(
        &state,
        READER,
        Method::POST,
        &uri,
        "application/json",
        serde_json::to_vec(&a_space("mobility-2", PROJECT)).expect("a manifest"),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "a reader imported: {status}",
    );

    // A project name that could not be one, and a bundle aimed at another project's namespace.
    let (status, _) = send(
        &state,
        AUTHOR,
        Method::POST,
        "/api/v1/projects/Ovzdusie/import",
        "application/json",
        serde_json::to_vec(&a_space("mobility-2", PROJECT)).expect("a manifest"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        &state,
        AUTHOR,
        Method::POST,
        &uri,
        "application/json",
        serde_json::to_vec(&a_space("mobility-2", ELSEWHERE)).expect("a manifest"),
    )
    .await;
    assert!(status.is_client_error(), "{status}: {body}");

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused import reached the forge",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2019 `create`, T-2020 `replace`, T-2021 `patch`
// -------------------------------------------------------------------------------------------------

/// PF-50, MF-04, MF-24: the three writes share one gate. A caller who may read but not propose is
/// refused on all three; a status block and a literal credential are refused on all three; and a plural
/// this platform does not serve is the 404 of a resource that is not there. None of it reaches the forge.
#[tokio::test]
async fn the_three_writes_share_one_gate_and_refuse_before_the_forge() {
    let (forge, state) = world().await;
    let create = format!("/api/v1/projects/{PROJECT}/spaces");
    let replace = format!("/api/v1/projects/{PROJECT}/spaces/mobility");

    // A reader, on each of the three.
    for (verb, uri, content_type, body) in [
        (
            Method::POST,
            create.clone(),
            "application/json",
            a_space("mobility-2", PROJECT),
        ),
        (
            Method::PUT,
            replace.clone(),
            "application/json",
            a_space("mobility", PROJECT),
        ),
        (
            Method::PATCH,
            replace.clone(),
            "application/merge-patch+json",
            json!({ "spec": { "isSandbox": false } }),
        ),
    ] {
        let (status, answer) = send(
            &state,
            READER,
            verb.clone(),
            &uri,
            content_type,
            serde_json::to_vec(&body).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{verb} {uri}: {answer}");
    }

    // A status block, which the platform computes (MF-04), and a literal credential (MF-24): refused
    // on the write that carries it, and the refusal never repeats the credential.
    let with_status = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility-2", "namespace": PROJECT },
        "spec": { "isSandbox": true },
        "status": { "phase": "Live" }
    });
    let with_secret = json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "mobility-feed", "namespace": PROJECT },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "mobility" },
            "audience": "project",
            "enabledRepresentations": ["ngsi-ld"],
            "upstream": { "token": "a-literal-token" }
        }
    });
    for (uri, body) in [
        (create.clone(), with_status.clone()),
        (create.clone(), with_secret.clone()),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            Method::POST,
            &uri,
            "application/json",
            serde_json::to_vec(&body).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert!(
            !answer.to_string().contains("a-literal-token"),
            "the refusal repeated the credential: {answer}",
        );
    }
    // The same two on a patch, where the body is a fragment rather than a manifest.
    for patch in [
        json!({ "status": { "phase": "Live" } }),
        json!({ "spec": { "upstream": { "token": "a-literal-token" } } }),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            Method::PATCH,
            &replace,
            "application/merge-patch+json",
            serde_json::to_vec(&patch).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{patch}: {answer}");
        assert!(!answer.to_string().contains("a-literal-token"), "{answer}");
    }

    // A plural this platform does not serve, on each of the three.
    for (verb, uri, content_type) in [
        (
            Method::POST,
            format!("/api/v1/projects/{PROJECT}/contextspaces"),
            "application/json",
        ),
        (
            Method::PUT,
            format!("/api/v1/projects/{PROJECT}/contextspaces/mobility"),
            "application/json",
        ),
        (
            Method::PATCH,
            format!("/api/v1/projects/{PROJECT}/contextspaces/mobility"),
            "application/merge-patch+json",
        ),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            verb.clone(),
            &uri,
            content_type,
            serde_json::to_vec(&a_space("mobility", PROJECT)).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb} {uri}: {answer}");
    }

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused write reached the forge",
    );
}

/// MF-13, CC-19: what each of the three answers for a body that is not what it takes. A `dryRun` value
/// the API does not define, a patch of a resource that is not there, a replace whose manifest names
/// another name, and a manifest aimed at another project are all refused before the forge — and the
/// dry run of a write that would go ahead costs the forge nothing either.
#[tokio::test]
async fn a_body_or_a_target_that_is_not_the_routes_own_is_refused_before_the_forge() {
    let (forge, state) = world().await;
    let create = format!("/api/v1/projects/{PROJECT}/spaces");
    let mobility = format!("/api/v1/projects/{PROJECT}/spaces/mobility");

    // `dryRun` takes one value on every write (MF-13).
    for value in ["all", "true", "1", ""] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            Method::POST,
            &format!("{create}?dryRun={value}"),
            "application/json",
            serde_json::to_vec(&a_space("mobility-2", PROJECT)).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value:?}: {answer}");
    }

    // A patch of a resource that is not there, and a patch that is not a patch.
    let (status, answer) = send(
        &state,
        AUTHOR,
        Method::PATCH,
        &format!("/api/v1/projects/{PROJECT}/spaces/ghost"),
        "application/merge-patch+json",
        serde_json::to_vec(&json!({ "spec": { "isSandbox": false } })).expect("a body"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    for (content_type, body) in [
        ("application/merge-patch+json", b"not json".to_vec()),
        ("application/merge-patch+json", Vec::new()),
        ("application/json", b"{}".to_vec()),
        ("text/yaml", b"spec: {}".to_vec()),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            Method::PATCH,
            &mobility,
            content_type,
            body.clone(),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{content_type} answered {status}: {answer}",
        );
    }

    // A replace whose manifest names another resource, and a manifest aimed at another project.
    for (uri, body) in [
        (mobility.clone(), a_space("something-else", PROJECT)),
        (mobility.clone(), a_space("mobility", ELSEWHERE)),
        (create.clone(), a_space("mobility-2", ELSEWHERE)),
    ] {
        let (status, answer) = send(
            &state,
            AUTHOR,
            if uri == create {
                Method::POST
            } else {
                Method::PUT
            },
            &uri,
            "application/json",
            serde_json::to_vec(&body).expect("a body"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }

    // The dry run of a create that would go ahead: the plan and the lane, and not one call to the
    // forge — a check costs nothing but the answer.
    let (status, plan) = send(
        &state,
        AUTHOR,
        Method::POST,
        &format!("{create}?dryRun=All"),
        "application/json",
        serde_json::to_vec(&a_space("mobility-2", PROJECT)).expect("a body"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["valid"], json!(true), "{plan}");
    assert_eq!(plan["plan"]["summary"]["create"], json!(1), "{plan}");

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "an answer given before the write reached the forge",
    );
}
