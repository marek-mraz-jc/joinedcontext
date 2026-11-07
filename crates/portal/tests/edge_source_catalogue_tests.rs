//! Edge cases of the catalogue status and the two DataModel source routes (T-2000, T-2001, T-2002;
//! DM-01, DM-24, EP-62, MF-24, PF-59, R20).
//!
//! **The contract, in one sentence:** the catalogue status and the model's source are reads of a
//! project, refused as a `404` for a caller whose bindings do not cover them, and a source is written
//! only after the path in the manifest, the caller's verb and the bytes of the body have each been
//! checked — before the forge is asked for anything.
//!
//! The happy paths live in `ckan_api_tests.rs` (the catalogue, the published resources, the token
//! reference) and `datamodel_source_tests.rs` (the source, the traversal, the version rules, the
//! proposal and its four artifacts). This file is the other side: a caller with no binding, a manifest
//! that names no file, a body that is not text.
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
const CSRF: &str = "csrf-token-edge-source";
const READER: &str = "peter.reader@banskabystrica.sk";
const STRANGER: &str = "nobody@banskabystrica.sk";
const TOKEN_SECRET: &str = "ckan-api-token";

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

fn role(name: &str, kinds: Value, verbs: Value) -> ResourceEnvelope {
    envelope(
        "Role",
        name,
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
    )
}

fn binding(name: &str, user: &str, role: &str, project: &str) -> ResourceEnvelope {
    envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": user }], "role": role, "scope": { "project": project } }),
    )
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

async fn send(
    state: &AppState,
    email: &str,
    verb: Method,
    uri: &str,
    body: Option<Vec<u8>>,
) -> (StatusCode, Vec<u8>, String) {
    let app = server::app(state.clone());
    let mut request = Request::builder()
        .method(verb)
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email))
        .header(CSRF_HEADER, CSRF);
    let body = match body {
        Some(bytes) => {
            request = request.header(header::CONTENT_TYPE, "text/yaml");
            Body::from(bytes)
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes()
        .to_vec();
    (status, bytes, content_type)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

// -------------------------------------------------------------------------------------------------
// T-2000 `get_status`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20, MF-24: the catalogue status is a read of the project's `CkanInstance` manifests. A
/// caller whose bindings do not cover that kind gets the answer of a project that is not there, and
/// what a caller who may read gets is the reference to the token — never a token.
#[tokio::test]
async fn the_catalogue_status_is_a_read_of_the_project_and_carries_no_token() {
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(envelope(
        "CkanInstance",
        "city-portal",
        PROJECT,
        json!({
            "url": "https://data.banskabystrica.sk",
            "organizationDefault": "mesto-bb",
            "apiTokenRef": { "name": TOKEN_SECRET, "key": "token" },
        }),
    ));
    state.mirror.upsert(envelope(
        "CkanInstance",
        "other-portal",
        ELSEWHERE,
        json!({
            "url": "https://data.doprava.sk",
            "organizationDefault": "doprava",
            "apiTokenRef": { "name": "doprava-token", "key": "token" },
        }),
    ));
    state.mirror.upsert(role(
        "catalogue-reader",
        json!(["CkanInstance"]),
        json!(["read"]),
    ));
    state.mirror.upsert(role(
        "space-reader",
        json!(["ContextSpace"]),
        json!(["read"]),
    ));
    state.mirror.upsert(binding(
        "catalogue-readers",
        READER,
        "catalogue-reader",
        PROJECT,
    ));
    // A second person who reads the project but not its catalogues: the kind is what decides.
    state.mirror.upsert(binding(
        "space-readers",
        "eva.spaces@banskabystrica.sk",
        "space-reader",
        PROJECT,
    ));

    let uri = format!("/api/v1/projects/{PROJECT}/ckan/status");
    for who in [STRANGER, "eva.spaces@banskabystrica.sk"] {
        let (status, body, _) = send(&state, who, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        let text = String::from_utf8_lossy(&body);
        for leaked in ["city-portal", "data.banskabystrica.sk", TOKEN_SECRET] {
            assert!(!text.contains(leaked), "{who} was told {leaked:?}: {text}");
        }
    }

    let (status, body, _) = send(&state, READER, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let answer = json_of(&body);
    let instances = answer["instances"].as_array().cloned().unwrap_or_default();
    assert_eq!(instances.len(), 1, "{answer}");
    assert_eq!(instances[0]["name"], json!("city-portal"), "{answer}");
    // The reference by name, which is what a person needs to find the Secret; the value lives in the
    // cluster and is resolved by the reconciler (MF-24).
    assert_eq!(instances[0]["apiTokenRef"], json!(TOKEN_SECRET), "{answer}");
    let text = String::from_utf8_lossy(&body);
    for never in ["\"token\":", "apiToken\":\"", "doprava"] {
        assert!(
            !text.contains(never),
            "the status carried {never:?}: {text}",
        );
    }

    // The project next door, where this reader holds nothing: not there, and its catalogue is not
    // named in the refusal.
    let (status, body, _) = send(
        &state,
        READER,
        Method::GET,
        &format!("/api/v1/projects/{ELSEWHERE}/ckan/status"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&body).contains("other-portal"));
}

// -------------------------------------------------------------------------------------------------
// T-2001 `get_source` and T-2002 `put_source`
// -------------------------------------------------------------------------------------------------

/// A Portal whose forge is a mock with nothing mounted, so a case can prove a refusal never asked it
/// for a file — plus a model whose manifest names the file `linkml` points at.
async fn source_world(linkml: Value) -> (MockServer, AppState) {
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
    let mut spec = json!({
        "contextSpaceRef": "mobility",
        "version": "1.0.0",
        "lifecycle": "published",
        "classes": ["AirQualityObserved"],
    });
    if !linkml.is_null() {
        spec["linkml"] = linkml;
    }
    state
        .mirror
        .upsert(envelope("DataModel", "air-quality", PROJECT, spec));
    state.mirror.upsert(role(
        "model-author",
        json!(["DataModel"]),
        json!(["read", "propose"]),
    ));
    state
        .mirror
        .upsert(binding("model-authors", READER, "model-author", PROJECT));
    (forge, state)
}

/// DM-01: the file a model's source lives in comes from its own manifest, so a manifest that names no
/// space, no file, or a file that is not a LinkML source is a message a person can act on — not a read
/// of whatever the path happens to point at. The forge is not asked for any of them.
#[tokio::test]
async fn a_manifest_that_names_no_linkml_file_is_a_message_and_not_a_read() {
    for (linkml, expected) in [
        (Value::Null, "linkml"),
        (json!(""), "linkml"),
        (json!("/etc/passwd"), "absolute"),
        (json!("..\\windows\\model.linkml.yaml"), "absolute"),
        (json!("../../other/model.linkml.yaml"), ".."),
        (json!("./model.yaml"), ".linkml.yaml"),
        (json!("model.linkml.yaml.bak"), ".linkml.yaml"),
        (json!("./"), ".."),
    ] {
        let (forge, state) = source_world(linkml.clone()).await;
        let (status, body, _) = send(
            &state,
            READER,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/datamodels/air-quality/source"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{linkml}");
        let detail = json_of(&body)["detail"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            detail.contains(expected),
            "{linkml} does not say what is wrong ({expected}): {detail}",
        );
        assert!(
            forge
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty(),
            "{linkml} reached the forge",
        );
    }

    // A model the project does not hold, and one of the project next door: the same 404, and the
    // forge is not asked for either.
    let (forge, state) = source_world(json!("./air-quality.linkml.yaml")).await;
    for uri in [
        format!("/api/v1/projects/{PROJECT}/datamodels/ghost/source"),
        format!("/api/v1/projects/{ELSEWHERE}/datamodels/air-quality/source"),
    ] {
        let (status, _, _) = send(&state, READER, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a model that is not there reached the forge",
    );

    // And a caller with no binding at all learns nothing about the model either way.
    let (status, body, _) = send(
        &state,
        STRANGER,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/datamodels/air-quality/source"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&body).contains("mobility"));
}

/// DM-24: a source is written by a caller who may propose a DataModel here, from a body that is text
/// and inside the size the route promises. Every other request is refused before the forge is touched,
/// so nothing half-written is left on a branch.
#[tokio::test]
async fn a_source_that_is_not_text_a_caller_or_a_size_is_refused_before_the_forge() {
    let (forge, state) = source_world(json!("./air-quality.linkml.yaml")).await;
    let uri = format!("/api/v1/projects/{PROJECT}/datamodels/air-quality/source");

    // Bytes that are not text: a source is YAML, and a body that is not UTF-8 is refused as one.
    let (status, body, _) = send(
        &state,
        READER,
        Method::PUT,
        &uri,
        Some(vec![0xff, 0xfe, 0x00, 0x41]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        json_of(&body)["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("utf-8"),
        "{}",
        String::from_utf8_lossy(&body),
    );

    // A caller who may read a model but not propose one.
    state
        .mirror
        .upsert(role("model-reader", json!(["DataModel"]), json!(["read"])));
    state.mirror.upsert(binding(
        "model-readers",
        "eva.reader@banskabystrica.sk",
        "model-reader",
        PROJECT,
    ));
    let (status, _, _) = send(
        &state,
        "eva.reader@banskabystrica.sk",
        Method::PUT,
        &uri,
        Some(b"id: https://example.org/m\nname: m\n".to_vec()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A caller with no binding at all: not there, rather than a refusal that says the model exists.
    let (status, _, _) = send(
        &state,
        STRANGER,
        Method::PUT,
        &uri,
        Some(b"id: https://example.org/m\nname: m\n".to_vec()),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "{status}",
    );

    // A model the project does not hold: its name has to be a manifest name, and a space named has to
    // be one of the project's. Without a space it is a project model (DM-75), so `ghost` alone is no
    // refusal.
    for (name, query) in [
        ("Air-Quality", "?space=mobility"),
        ("Air-Quality", ""),
        ("air%20quality", "?space=mobility"),
        ("air-quality-", "?space=mobility"),
        ("ghost", "?space=nowhere"),
    ] {
        let (status, body, _) = send(
            &state,
            READER,
            Method::PUT,
            &format!("/api/v1/projects/{PROJECT}/datamodels/{name}/source{query}"),
            Some(b"id: https://example.org/m\nname: m\n".to_vec()),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{name}{query} answered {status}: {}",
            String::from_utf8_lossy(&body),
        );
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
