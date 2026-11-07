//! Edge cases of the four change routes (T-1996, T-1997, T-1998, T-1999; PF-50, PF-59, R20, CC-32).
//!
//! **The contract, in one sentence:** a change belongs to one project and to the kinds a caller's
//! bindings read, its id is `chg-` and eight hex digits, and every refusal of an approval or a
//! rejection happens before the forge is asked to merge or close anything.
//!
//! The happy paths and the lane rules live in `changes_tests.rs` (2 400 lines of them: the listing's
//! metadata, the redacted diff, self-approval, the Red lane's confirmation, bundles, the merge and the
//! rejection that follow). This file is the other side: a change of the project next door, a change
//! whose manifest the forge no longer has, an id that is not one, and an approval nobody may press.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
const CSRF: &str = "csrf-token-edge-changes";
const BRANCH: &str = "portal/create-contextspace-mobility-11111111";
const GHOST_BRANCH: &str = "portal/create-contextspace-ghost-22222222";
const AUTHOR: &str = "jana.kovacova@banskabystrica.sk";

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

fn pull_request(number: u64, branch: &str, title: &str) -> Value {
    json!({
        "number": number,
        "html_url": format!("https://gitea.example.sk/pulls/{number}"),
        "state": "open",
        "title": title,
        "head": { "ref": branch },
        "base": { "ref": "main" },
        "created_at": "2026-09-06T09:14:22Z",
        "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová", "email": AUTHOR },
        "mergeable": true,
        "merged": false
    })
}

/// A forge with two open changes of `ovzdusie`: one whose manifest it serves, and one whose manifest
/// it does not have any more. Nothing that merges or closes is mounted, so a case can prove a refusal
/// never asked for one.
async fn world() -> (MockServer, AppState) {
    let forge = MockServer::start().await;
    let repo = "/api/v1/repos/test-owner/test-repo";

    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            pull_request(1, BRANCH, "create ContextSpace mobility"),
            pull_request(2, GHOST_BRANCH, "create ContextSpace ghost"),
        ])))
        .mount(&forge)
        .await;
    for (number, branch, title) in [
        (1, BRANCH, "create ContextSpace mobility"),
        (2, GHOST_BRANCH, "create ContextSpace ghost"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{repo}/pulls/{number}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(pull_request(number, branch, title)),
            )
            .mount(&forge)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{repo}/pulls/{number}/files")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": format!("projects/{PROJECT}/spaces/mobility/space.yaml"),
                  "status": "added" }
            ])))
            .mount(&forge)
            .await;
    }
    // The first change's manifest. The second one's is nowhere: wiremock answers 404 for every path
    // nobody mounted, which is what a manifest deleted from the branch looks like.
    Mock::given(method("GET"))
        .and(path(format!(
            "{repo}/contents/projects/{PROJECT}/spaces/mobility/space.yaml"
        )))
        .and(query_param("ref", BRANCH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": STANDARD.encode(
                "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                 name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n",
            ),
        })))
        .mount(&forge)
        .await;

    let client = GiteaClient::new(
        forge.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));

    // A reader of spaces everywhere in the organization, and an approver of them.
    state.mirror.upsert(org(
        "Role",
        "space-reader",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "space-approver",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read", "approve"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "readers",
        json!({ "subjects": [{ "user": "peter.reader@banskabystrica.sk" }],
                "role": "space-reader", "scope": { "organization": "bb" } }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "approvers",
        json!({ "subjects": [{ "user": "eva.approver@banskabystrica.sk" }],
                "role": "space-approver", "scope": { "organization": "bb" } }),
    ));
    (forge, state)
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

async fn call(
    state: &AppState,
    email: &str,
    verb: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let app = server::app(state.clone());
    let mut request = Request::builder()
        .method(verb)
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email))
        .header(CSRF_HEADER, CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
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

/// Whether the forge was asked to change anything: a merge, a close, a review or a write.
async fn mutated(forge: &MockServer) -> Vec<String> {
    forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| !request.method.to_string().eq_ignore_ascii_case("get"))
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

// -------------------------------------------------------------------------------------------------
// T-1996 `list_changes`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: the list is one project's. The same open merge request is in the list of the project
/// whose manifests it changes and in no other — the paths in the change decide, not the caller's
/// rights — and a project name that could not be a project is the 404 of a project that is not there.
#[tokio::test]
async fn a_change_is_in_the_list_of_the_project_it_changes_and_of_no_other() {
    let (forge, state) = world().await;

    let (status, mine) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/changes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(
        mine["items"].as_array().map(Vec::len),
        Some(1),
        "the project's own change is not the only one listed: {mine}",
    );

    // The same reader, the same forge, the project next door: the change belongs to `ovzdusie`, so
    // there is nothing here — not a copy of it under another project's name.
    let (status, theirs) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{ELSEWHERE}/changes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    assert_eq!(theirs["items"], json!([]), "{theirs}");
    assert!(
        !theirs.to_string().contains("isSandbox"),
        "another project's list carried the manifest: {theirs}",
    );

    // A name that could not be a project: this reader's binding is scoped to the organization, so
    // it covers every project name in it and the answer is an empty list rather than a 404 — the
    // listing tells nobody which names exist either way, because every name that is not a project's
    // reads the same.
    for project in ["Ovzdusie", "ovzdusie-1", "%2e%2e"] {
        let (status, body) = call(
            &state,
            "peter.reader@banskabystrica.sk",
            Method::GET,
            &format!("/api/v1/projects/{project}/changes"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{project}: {body}");
        assert_eq!(body["items"], json!([]), "{project}: {body}");
    }

    assert!(mutated(&forge).await.is_empty(), "a read changed something");
}

/// CC-32: one change whose manifest the forge no longer serves does not take the listing with it. The
/// broken one is absent, the sound one is there, and the answer is a list rather than a 5xx.
#[tokio::test]
async fn a_change_whose_manifest_the_forge_has_lost_leaves_the_list_standing() {
    let (_forge, state) = world().await;

    let (status, list) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/changes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<&str> = list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["metadata"]["name"].as_str())
        .collect();
    assert_eq!(ids, vec!["chg-00000001"], "{list}");

    // And asked for by its own id, the one the forge lost is not found rather than half a change.
    let (status, body) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000002"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

// -------------------------------------------------------------------------------------------------
// T-1997 `get_change`
// -------------------------------------------------------------------------------------------------

/// A change id is `chg-` and eight lowercase hex digits (T-0832). Anything else is a 400 that says so
/// — but only for a caller who reads the project: the read is checked first, so a stranger learns
/// nothing about which ids are well formed.
#[tokio::test]
async fn a_change_id_is_chg_and_eight_lowercase_hex_digits_and_nothing_else() {
    let (forge, state) = world().await;
    let malformed = [
        "chg-1",
        "chg-000000001",
        "chg-0000000g",
        "chg-0000000F",
        "CHG-00000001",
        "chg-0000_001",
        "chg-%20000001",
        "chg-",
        "00000001",
        "chg-00000001x",
    ];

    for id in malformed {
        let (status, body) = call(
            &state,
            "peter.reader@banskabystrica.sk",
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/changes/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{id}: {body}");
        let detail = body["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("chg-"),
            "{id} does not say what an id looks like: {detail}",
        );

        // The same id, asked by somebody with no binding at all: the project is not there, and the
        // shape of the id is none of their business.
        let (status, body) = call(
            &state,
            "nobody@banskabystrica.sk",
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/changes/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}: {body}");
    }

    // A well-formed id nobody minted: the forge has no such pull request.
    let (status, body) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/changes/chg-ffffffff"),
        None,
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::SERVICE_UNAVAILABLE,
        "{status}: {body}",
    );
    assert!(mutated(&forge).await.is_empty(), "a read changed something");
}

/// PF-59, R20: a change is read through the path of the project it changes. Asked for through another
/// project's path it is not found, and the 404 carries nothing of the manifest — not the spec, and not
/// the project the change really belongs to.
#[tokio::test]
async fn a_change_is_not_readable_through_another_projects_path() {
    let (_forge, state) = world().await;

    let (status, mine) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000001"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(mine["metadata"]["name"], json!("chg-00000001"), "{mine}");

    let (status, theirs) = call(
        &state,
        "peter.reader@banskabystrica.sk",
        Method::GET,
        &format!("/api/v1/projects/{ELSEWHERE}/changes/chg-00000001"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{theirs}");
    let text = theirs.to_string();
    for leaked in ["isSandbox", "ovzdusie/spaces", "blob-1"] {
        assert!(!text.contains(leaked), "the 404 carried {leaked:?}: {text}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-1998 `approve_change` and T-1999 `reject_change`
// -------------------------------------------------------------------------------------------------

/// PF-50: approving and rejecting need `approve` on the change's kind in the project. A reader and a
/// person no binding covers are each refused both, and the change's own author without the verb is
/// refused the approval — and the forge is asked for nothing: no merge, no close, no review. The
/// author's own reject is a withdrawal (T-6535), covered in tests/changes_tests.rs.
#[tokio::test]
async fn an_approval_nobody_may_press_asks_the_forge_for_nothing() {
    let (forge, state) = world().await;
    let approve = format!("/api/v1/projects/{PROJECT}/changes/chg-00000001/approve");
    let reject = format!("/api/v1/projects/{PROJECT}/changes/chg-00000001/reject");

    for (who, uris) in [
        ("peter.reader@banskabystrica.sk", vec![&approve, &reject]),
        ("nobody@banskabystrica.sk", vec![&approve, &reject]),
        (AUTHOR, vec![&approve]),
    ] {
        for uri in uris {
            let (status, body) = call(&state, who, Method::POST, uri, Some(json!({}))).await;
            assert!(
                status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
                "{who} on {uri} answered {status}: {body}",
            );
        }
    }

    let touched = mutated(&forge).await;
    assert!(
        touched.is_empty(),
        "a refused approval reached the forge: {touched:?}",
    );
}

/// PF-50, R20: an approver's own refusals come before the forge too — an id that is not one, a change
/// of another project, and a change whose manifest the forge has lost. Nothing is merged or closed for
/// any of them, and a body that is not a verdict is refused as a body.
#[tokio::test]
async fn an_approvers_own_refusals_also_come_before_the_forge() {
    let (forge, state) = world().await;
    let approver = "eva.approver@banskabystrica.sk";

    // An id that is not an id: refused after the caller's right to approve here, before the forge.
    for id in ["chg-1", "CHG-00000001", "chg-0000000g", ""] {
        let (status, body) = call(
            &state,
            approver,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/changes/{id}/approve"),
            Some(json!({})),
        )
        .await;
        assert!(status.is_client_error(), "{id:?} answered {status}: {body}",);
    }

    // The change of a project this path does not name, and the change whose manifest is gone.
    for uri in [
        format!("/api/v1/projects/{ELSEWHERE}/changes/chg-00000001/approve"),
        format!("/api/v1/projects/{PROJECT}/changes/chg-00000002/approve"),
        format!("/api/v1/projects/{ELSEWHERE}/changes/chg-00000001/reject"),
        format!("/api/v1/projects/{PROJECT}/changes/chg-00000002/reject"),
    ] {
        let (status, body) = call(&state, approver, Method::POST, &uri, Some(json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // A body that is not a verdict: refused as a body, whoever sends it.
    // `json!([])` is absent on purpose: serde reads a struct from a sequence, so an empty array is a
    // body with both fields defaulted and the approval goes on to the forge like any other.
    for body in [
        json!("approve"),
        json!({ "confirm": 7 }),
        json!(7),
        json!(""),
    ] {
        let (status, answer) = call(
            &state,
            approver,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/changes/chg-00000001/approve"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
    }

    let touched = mutated(&forge).await;
    assert!(
        touched.is_empty(),
        "a refused verdict reached the forge: {touched:?}",
    );
}
