//! A writer's own branch in a project repository is a Change (T-3433, ADR-N-029, PF-87):
//! listed with its files and lane, approved through the Portal, never by its own author; a
//! fork's or a non-writer's merge request is neither listed nor approvable.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::api::changes::{ChangeList, ChangeProposal};
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::Lane;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-12345";
const REPO: &str = "/api/v1/repos/test-owner/ovzdusie-config";
const WRITER: (&str, &str) = ("wera.writer", "wera.writer@hel.fi");
const APPROVER: (&str, &str) = ("jana.approver", "jana.approver@hel.fi");
const OUTSIDER: (&str, &str) = ("olli.outsider", "olli.outsider@hel.fi");
const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";

fn cookies(config: &Config, (username, email): (&str, &str)) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: format!("sub-{username}"),
            username: username.to_owned(),
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
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("jar");
    let response = (jar, StatusCode::OK).into_response();
    let session: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    format!("{}; {CSRF_COOKIE}={CSRF}", session.join("; "))
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A merge request of the project repository: `head_repo` 7 is the repository itself, any other
/// a fork.
fn pull(number: u64, branch: &str, head_repo: u64, (login, email): (&str, &str)) -> Value {
    json!({
        "number": number,
        "html_url": format!("https://forge.example/test-owner/ovzdusie-config/pulls/{number}"),
        "state": "open",
        "title": format!("pushed {branch}"),
        "head": { "ref": branch, "sha": format!("head-{number}"), "repo_id": head_repo },
        "base": { "ref": "main", "repo_id": 7 },
        "created_at": format!("2026-10-10T0{number}:00:00Z", number = number % 10),
        "user": { "login": login, "full_name": "", "email": email },
        "mergeable": true,
        "merged": false
    })
}

/// Layout 2 with the project `ovzdusie` in its own repository; Wera may propose, Jana
/// administers the organization, Olli holds nothing. Merge requests 11 (Wera's `fix-bikes`),
/// 12 (Wera's, from a fork) and 13 (Olli's) are open, and 11's head commit claims to be Jana's.
async fn forge(writer_verbs: &[&str]) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "organization-config",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "ovzdusie".to_owned(),
        "ovzdusie-config".to_owned(),
    )]));
    let every = ["read", "propose", "approve", "delete"];
    state.mirror.upsert(org(
        "Role",
        "admin",
        json!({ "rules": [{ "kinds": ["ContextSpace", "RoleBinding"], "verbs": every }] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "writer",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": writer_verbs }] }),
    ));
    for (name, role, user) in [("jana", "admin", APPROVER.1), ("wera", "writer", WRITER.1)] {
        state.mirror.upsert(org(
            "RoleBinding",
            name,
            json!({ "subjects": [{ "user": user }], "role": role, "scope": { "organization": "hel" } }),
        ));
    }

    let pulls = [
        pull(11, "fix-bikes", 7, WRITER),
        pull(12, "fix-bikes", 8, WRITER),
        pull(13, "olli-bikes", 7, OUTSIDER),
    ];
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(pulls)))
        .mount(&server)
        .await;
    for pr in &pulls {
        let number = pr["number"].as_u64().expect("number");
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls/{number}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls/{number}/files")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!([{ "filename": "spaces/mobility/space.yaml", "status": "added" }]),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/spaces/mobility/space.yaml")))
            .and(query_param("ref", format!("head-{number}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "sha": "blob", "content": STANDARD.encode(SPACE) })),
            )
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "name": "ovzdusie-config", "default_branch": "main" })),
        )
        .mount(&server)
        .await;
    // What a commit says is whatever its author configured: here, that Jana wrote it.
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/commits")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "sha": "head-11",
            "commit": { "message": "fix bikes", "author": { "name": "Jana", "email": APPROVER.1, "date": "2026-10-10T01:00:00Z" } }
        }])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/commits/head-11/status")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "state": "failure", "total_count": 1, "statuses": [] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls/11/merge")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    (server, state)
}

async fn get(state: &AppState, who: (&str, &str), uri: &str) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookies(&state.config, who))
                .body(Body::empty())
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn approve(state: &AppState, who: (&str, &str), number: u64) -> StatusCode {
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/ovzdusie/changes/chg-{number:08x}/approve"
                ))
                .header(header::COOKIE, cookies(&state.config, who))
                .header(CSRF_HEADER, CSRF)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

async fn merged(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/merge"))
        .map(|r| r.url.path().to_owned())
        .collect()
}

#[tokio::test]
async fn a_writers_branch_is_listed_with_its_files_lane_and_forge_author() {
    let (_server, state) = forge(&["read", "propose"]).await;

    let (status, body) = get(&state, APPROVER, "/api/v1/projects/ovzdusie/changes").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let list: ChangeList = serde_json::from_value(body).expect("ChangeList");
    assert_eq!(list.items.len(), 1, "only Wera's own branch is a Change");
    let change = &list.items[0];
    assert_eq!(change.metadata.name, "chg-0000000b");
    assert_eq!(change.summary.params["kind"], "ContextSpace");
    assert_eq!(change.summary.params["name"], "mobility");
    assert_eq!(change.status.lane, Lane::Green);
    assert_eq!(change.file_count, Some(1));
    // The forge account that opened it, not the commit that claims to be Jana's.
    assert_eq!(change.author.email.as_deref(), Some(WRITER.1));
}

#[tokio::test]
async fn a_forks_and_a_non_writers_merge_requests_are_named_to_an_org_admin_alone() {
    let (_server, state) = forge(&["read", "propose"]).await;

    let (_, body) = get(&state, APPROVER, "/api/v1/projects/ovzdusie/changes").await;
    let list: ChangeList = serde_json::from_value(body).expect("ChangeList");
    let outside: Vec<(u64, &str)> = list
        .outside
        .iter()
        .map(|mr| (mr.number, mr.author.as_str()))
        .collect();
    assert_eq!(outside, [(12, WRITER.0), (13, OUTSIDER.0)]);
    assert!(
        list.outside[0].reason.contains("fork"),
        "{:?}",
        list.outside[0]
    );
    assert!(
        list.outside[1].reason.contains("not a writer"),
        "{:?}",
        list.outside[1]
    );

    let (_, body) = get(&state, WRITER, "/api/v1/projects/ovzdusie/changes").await;
    let list: ChangeList = serde_json::from_value(body).expect("ChangeList");
    assert_eq!(list.items.len(), 1);
    assert!(
        list.outside.is_empty(),
        "a writer is not told: {:?}",
        list.outside
    );
}

#[tokio::test]
async fn the_detail_shows_the_repository_ci_on_the_head_commit() {
    let (_server, state) = forge(&["read", "propose"]).await;

    let (status, body) = get(
        &state,
        APPROVER,
        "/api/v1/projects/ovzdusie/changes/chg-0000000b",
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let change: ChangeProposal = serde_json::from_value(body).expect("ChangeProposal");
    assert_eq!(change.ci.as_deref(), Some("failure"));
    assert_eq!(change.files.as_ref().map(Vec::len), Some(1));
}

#[tokio::test]
async fn an_approver_merges_a_writers_change() {
    let (server, state) = forge(&["read", "propose"]).await;

    assert_eq!(approve(&state, APPROVER, 11).await, StatusCode::ACCEPTED);
    assert_eq!(merged(&server).await, [format!("{REPO}/pulls/11/merge")]);
}

#[tokio::test]
async fn the_author_cannot_approve_their_own_branch_whatever_its_commits_claim() {
    // Wera may approve ContextSpaces, so only the authorship stops her (CC-34, PF-58).
    let (server, state) = forge(&["read", "propose", "approve"]).await;

    assert_eq!(approve(&state, WRITER, 11).await, StatusCode::FORBIDDEN);
    assert!(merged(&server).await.is_empty());
}

#[tokio::test]
async fn a_forks_or_a_non_writers_merge_request_is_not_found_and_never_merged() {
    let (server, state) = forge(&["read", "propose"]).await;

    for number in [12, 13] {
        assert_eq!(
            approve(&state, APPROVER, number).await,
            StatusCode::NOT_FOUND
        );
        let (status, _) = get(
            &state,
            APPROVER,
            &format!("/api/v1/projects/ovzdusie/changes/chg-{number:08x}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert!(merged(&server).await.is_empty());
}

#[tokio::test]
async fn a_writer_who_lost_the_binding_has_no_change_any_more() {
    let (server, state) = forge(&["read"]).await;

    let (_, body) = get(&state, APPROVER, "/api/v1/projects/ovzdusie/changes").await;
    let list: ChangeList = serde_json::from_value(body).expect("ChangeList");
    assert!(list.items.is_empty(), "{:?}", list.items);
    assert_eq!(approve(&state, APPROVER, 11).await, StatusCode::NOT_FOUND);
    assert!(merged(&server).await.is_empty());
}

#[tokio::test]
async fn a_forge_that_refuses_the_portals_token_is_a_503_that_says_what_to_do() {
    // dev 2026-10-21 (T-4966): Gitea answered every token 401 and the change list was a bare
    // 500, which the inbox showed as "Nothing waits for your decision".
    let (server, state) = forge(&["read", "propose"]).await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({ "message": "invalid username, password or token" })),
        )
        .with_priority(1)
        .mount(&server)
        .await;

    let (status, body) = get(&state, APPROVER, "/api/v1/projects/ovzdusie/changes").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("Git repository did not answer"), "{body}");
    assert!(!body.to_string().contains("token-xyz"), "{body}");
}
