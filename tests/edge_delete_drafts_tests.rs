//! Edge cases of the deletion route and the five draft routes (T-2003, T-2004, T-2005, T-2006, T-2007,
//! T-2008; AG-61, CC-19, MF-07, MF-24, PF-50, PF-59, R20, UI-47).
//!
//! **The contract, in one sentence:** a deletion needs `delete` on the kind and is refused while
//! anything still references the target, and a draft is one project's shared scratch — named like a
//! manifest, versioned, never holding a literal secret, and never announced to a reader who may not
//! read its kind.
//!
//! The happy paths live in `resource_delete_tests.rs` (the Red-lane change, the dependents, the stale
//! branch, the owned files) and `drafts_tests.rs` (the store, the round trip, the sweep, the stream's
//! filter). This file is the other side: a caller without the verb, a dry run that must cost nothing,
//! a plural that is not one, a body that is not a draft, a project name that is not a name.
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
const CSRF: &str = "csrf-token-edge-delete";
const DELETER: &str = "eva.deleter@banskabystrica.sk";
const PROPOSER: &str = "peter.proposer@banskabystrica.sk";
const STRANGER: &str = "nobody@banskabystrica.sk";

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
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

fn cookie(config: &Config, email: &str, groups: &[&str]) -> String {
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
            groups: groups.iter().map(|group| group.to_string()).collect(),
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
    groups: &[&str],
    verb: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(verb)
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email, groups))
        .header(CSRF_HEADER, CSRF);
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

// -------------------------------------------------------------------------------------------------
// T-2003 `delete_resource`
// -------------------------------------------------------------------------------------------------

/// A project with a space, one endpoint of it, and the two bindings that tell `propose` from `delete`.
/// The forge is a mock with nothing mounted, so a case can prove a refusal never asked it for a file.
async fn delete_world() -> (MockServer, AppState) {
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
        "space-proposer",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "space-deleter",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint"],
                            "verbs": ["read", "propose", "delete"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "proposers",
        json!({ "subjects": [{ "user": PROPOSER }], "role": "space-proposer",
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "deleters",
        json!({ "subjects": [{ "user": DELETER }], "role": "space-deleter",
                "scope": { "project": PROJECT } }),
    ));
    (forge, state)
}

/// PF-50: proposing is not deleting. A caller who may propose the kind is refused, and the refusal is a
/// `403` because a write says what is missing rather than pretending the resource is not there — the
/// rule the operations registry applies to every write. Nothing reaches the forge.
#[tokio::test]
async fn a_caller_who_may_propose_a_kind_still_may_not_delete_it() {
    let (forge, state) = delete_world().await;
    let uri = format!("/api/v1/projects/{PROJECT}/spaces/mobility");

    let (status, body) = send(&state, PROPOSER, &[], Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let text = body.to_string();
    for leaked in ["projects/ovzdusie/spaces", "sha", "isSandbox"] {
        assert!(
            !text.contains(leaked),
            "the refusal carried {leaked:?}: {text}"
        );
    }

    // A caller no binding covers at all may not read the space, so it is not there to them (R20,
    // T-2563); and a project this space does not live in.
    let (status, _) = send(&state, STRANGER, &[], Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &state,
        DELETER,
        &[],
        Method::DELETE,
        &format!("/api/v1/projects/{ELSEWHERE}/spaces/mobility"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused deletion reached the forge",
    );
}

/// MF-07, CC-19: what a deletion answers before it writes anything — the plural has to be one this
/// platform serves, the resource has to be there, the `dryRun` value has to be the one the API
/// documents, and a resource something still references is a conflict that names what holds it. None of
/// these costs a call to the forge, and the dry run of a real deletion does not either.
#[tokio::test]
async fn every_answer_a_deletion_gives_before_it_writes_costs_the_forge_nothing() {
    let (forge, state) = delete_world().await;

    // A plural this platform does not serve, including one that is nearly right.
    for plural in [
        "contextspaces",
        "contextspace",
        "Spaces",
        "space",
        "%2e%2e",
        "endpoints%2f..",
    ] {
        let (status, body) = send(
            &state,
            DELETER,
            &[],
            Method::DELETE,
            &format!("/api/v1/projects/{PROJECT}/{plural}/mobility"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{plural}: {body}");
    }

    // A name nobody has, and a name that is not a manifest name.
    for name in ["ghost", "Mobility", "mobility%20", "%2e%2e"] {
        let (status, body) = send(
            &state,
            DELETER,
            &[],
            Method::DELETE,
            &format!("/api/v1/projects/{PROJECT}/spaces/{name}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
    }

    // `dryRun` takes one value. Anything else is a 400 that names it, so a client that meant to check
    // never finds out by having written (MF-13).
    for value in ["all", "true", "1", "yes", "", "All,All"] {
        let (status, body) = send(
            &state,
            DELETER,
            &[],
            Method::DELETE,
            &format!("/api/v1/projects/{PROJECT}/spaces/mobility?dryRun={value}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value:?}: {body}");
        assert!(
            body["detail"].as_str().unwrap_or_default().contains("All"),
            "{value:?} does not name the value it wanted: {body}",
        );
    }

    // The space is still referenced by the endpoint of it: a conflict that names what holds it, and
    // the Red lane is never entered (MF-07).
    let (status, body) = send(
        &state,
        DELETER,
        &[],
        Method::DELETE,
        &format!("/api/v1/projects/{PROJECT}/spaces/mobility"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("mobility-bikes"),
        "the conflict does not say what still holds it: {body}",
    );

    // The dry run of a deletion that would go ahead: the plan and the lane, and not one call to the
    // forge — a check must cost nothing but the answer.
    let (status, plan) = send(
        &state,
        DELETER,
        &[],
        Method::DELETE,
        &format!("/api/v1/projects/{PROJECT}/endpoints/mobility-bikes?dryRun=All"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["valid"], json!(true), "{plan}");
    assert_eq!(plan["lane"], json!("red"), "a deletion is not Red: {plan}");
    assert_eq!(plan["plan"]["summary"]["delete"], json!(1), "{plan}");

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "an answer given before the write reached the forge",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2004 … T-2008: the five draft routes
// -------------------------------------------------------------------------------------------------

/// A project whose `devs` read and propose everything the drafts in this file name.
fn drafts_state() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    state
        .mirror
        .upsert(envelope("ContextSpace", "mobility", PROJECT, json!({})));
    state.mirror.upsert(org(
        "Role",
        "drafter",
        json!({ "rules": [{ "kinds": ["DataSource", "Endpoint", "ContextSpace"],
                            "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "drafters",
        json!({ "subjects": [{ "group": "devs" }], "role": "drafter",
                "scope": { "project": PROJECT } }),
    ));
    state
}

fn a_draft(name: &str) -> Value {
    json!({
        "manifest": {
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": name, "namespace": PROJECT },
            "spec": { "type": "http", "http": { "url": "https://example.org/feed" } }
        }
    })
}

/// PF-59, R20: every draft route of a project starts with the project's name, and a name that could not
/// be one is the 404 of a project that is not there — the same answer on all five, so the shape of a
/// name is not a way to ask which projects exist.
#[tokio::test]
async fn a_project_name_that_is_not_one_is_the_same_answer_on_all_five_draft_routes() {
    let state = drafts_state();

    for project in ["Ovzdusie", "ovzdusie_1", "%2e%2e", "-ovzdusie"] {
        let base = format!("/api/v1/projects/{project}/drafts");
        let calls: [(Method, String, Option<Value>); 5] = [
            (Method::GET, base.clone(), None),
            (Method::GET, format!("{base}/DataSource/feed-1"), None),
            (
                Method::PUT,
                format!("{base}/DataSource/feed-1"),
                Some(a_draft("feed-1")),
            ),
            (Method::DELETE, format!("{base}/DataSource/feed-1"), None),
            (Method::GET, format!("{base}/events"), None),
        ];
        for (verb, uri, body) in calls {
            let (status, answer) =
                send(&state, "jana@hel.fi", &["devs"], verb.clone(), &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{verb} {uri}: {answer}");
        }
    }
}

/// AG-61, MF-24, UI-47: a draft is a manifest under a name, saved against the version the editor last
/// saw. A body that is not that is refused as a body, a literal credential is refused by name, and a
/// stale version is a conflict rather than a silent overwrite of somebody else's edit.
#[tokio::test]
async fn a_draft_is_a_manifest_a_version_and_never_a_literal_secret() {
    let state = drafts_state();
    let uri = format!("/api/v1/projects/{PROJECT}/drafts/DataSource/feed-1");

    for body in [
        json!({}),
        json!({ "manifest": { "kind": "DataSource" }, "expectedVersion": "1" }),
        json!({ "manifest": { "kind": "DataSource" }, "expectedVersion": [] }),
        // `deny_unknown_fields`: a field this route does not read is a mistake, not something to drop.
        json!({ "manifest": { "kind": "DataSource" }, "touchedBy": "somebody.else" }),
        json!([]),
        Value::Null,
    ] {
        let (status, answer) = send(
            &state,
            "jana@hel.fi",
            &["devs"],
            Method::PUT,
            &uri,
            Some(body.clone()),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {answer}",
        );
    }

    // A credential typed into the form instead of a reference (MF-24).
    let (status, answer) = send(
        &state,
        "jana@hel.fi",
        &["devs"],
        Method::PUT,
        &uri,
        Some(json!({
            "manifest": {
                "apiVersion": API_VERSION,
                "kind": "DataSource",
                "metadata": { "name": "feed-1", "namespace": PROJECT },
                "spec": { "type": "http", "http": { "url": "https://example.org/feed",
                                                    "token": "a-literal-token" } }
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("secretRef"), "{detail}");
    assert!(
        !detail.contains("a-literal-token"),
        "the refusal repeated the secret: {detail}",
    );

    // The first save, then a second against the version it no longer is.
    let (status, first) = send(
        &state,
        "jana@hel.fi",
        &["devs"],
        Method::PUT,
        &uri,
        Some(a_draft("feed-1")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["version"], json!(1), "{first}");

    let mut stale = a_draft("feed-1");
    stale["expectedVersion"] = json!(0);
    let (status, conflict) = send(
        &state,
        "jana@hel.fi",
        &["devs"],
        Method::PUT,
        &uri,
        Some(stale),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");

    // And a draft nobody saved is not an empty draft.
    let (status, answer) = send(
        &state,
        "jana@hel.fi",
        &["devs"],
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/drafts/DataSource/feed-9"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");

    // Dropping is idempotent and says which it was: a draft that was never saved answers `false`
    // rather than a 404, so a form that closes twice never shows an error for work already gone.
    let (status, answer) = send(
        &state,
        "jana@hel.fi",
        &["devs"],
        Method::DELETE,
        &format!("/api/v1/projects/{PROJECT}/drafts/DataSource/feed-9"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["dropped"], json!(false), "{answer}");
    // The one that is there is dropped once, and the second drop says `false` like any other draft
    // that is not there.
    let (status, dropped) =
        send(&state, "jana@hel.fi", &["devs"], Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{dropped}");
    assert_eq!(dropped["dropped"], json!(true), "{dropped}");
    let (status, again) = send(&state, "jana@hel.fi", &["devs"], Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["dropped"], json!(false), "{again}");
    // And the draft is gone from the read as well, not merely reported gone.
    let (status, _) = send(&state, "jana@hel.fi", &["devs"], Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// PF-59, AG-61: the kind in the path is what a draft is held to — a reader's bindings answer for the
/// kind, so a kind no binding of theirs grants is refused on the read, the write and the drop alike,
/// and a kind this platform does not have is granted by nothing.
///
/// The name is not held to MF-02 here on purpose-by-omission: the draft store takes the name as it
/// comes and the manifest as it comes, and it is the proposal that refuses a name no manifest could
/// have. That is written down in `/workspace/chyby.md` rather than asserted as wanted behaviour.
#[tokio::test]
async fn a_draft_of_a_kind_no_binding_grants_is_refused_on_every_route() {
    let state = drafts_state();

    for (kind, name) in [
        ("Spreadsheet", "feed-1"),
        ("datasource", "feed-1"),
        ("%2e%2e", "feed-1"),
        ("Policy", "feed-1"),
        ("RoleBinding", "feed-1"),
        ("Secret", "feed-1"),
    ] {
        let uri = format!("/api/v1/projects/{PROJECT}/drafts/{kind}/{name}");
        for (verb, body) in [
            (Method::GET, None),
            (Method::PUT, Some(a_draft("feed-1"))),
            (Method::DELETE, None),
        ] {
            let (status, answer) = send(
                &state,
                "jana@hel.fi",
                &["devs"],
                verb.clone(),
                &uri,
                body.clone(),
            )
            .await;
            assert!(
                status.is_client_error(),
                "{verb} {kind}/{name} answered {status}: {answer}",
            );
        }
    }
}
