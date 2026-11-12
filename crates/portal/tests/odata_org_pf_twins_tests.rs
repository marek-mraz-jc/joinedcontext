//! PF-52, PF-58, PF-70 and PF-71 through the Organization's OData actions (T-6827, ADR-N-070
//! §2.4): each case proposes through `POST /api/v1/<entity>/JoinedContext.Portal.propose_*` and
//! through the REST door, and both answer the same verdict, so the OData face adds no way past
//! the role model, the lane or the four eyes.
//!
//! - PF-52: a reader proposes no access change; a grant above the proposer's own rights is
//!   refused; an access change is red lane.
//! - PF-58: an administrator's own access change waits for another approver; the author's
//!   approve is `403 self-approval`; a second administrator merges it.
//! - PF-70: a project steward's own Role change waits for another approver.
//! - PF-71: making an Endpoint public is a right of its own: a `propose` constrained to
//!   `spec.audience` not in `[public]` plans an internal Endpoint and is refused a public one.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
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
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const CSRF: &str = "test-csrf-token-6827";
const PROJECT: &str = "ovzdusie";
const ADMIN: &str = "jana.kovacova@banskabystrica.sk";
const SECOND: &str = "eva.hruba@banskabystrica.sk";
const VIEWER: &str = "ivan.novy@banskabystrica.sk";
const STEWARD: &str = "maria.cierna@banskabystrica.sk";
const DEVELOPER: &str = "peter.biely@banskabystrica.sk";
const BINDER: &str = "olga.zelena@banskabystrica.sk";
const BINDING: &str = "air-readers";
const ROLE: &str = "air-steward";

fn person(email: &str) -> Identity {
    let user = email.split('@').next().unwrap_or_default().to_owned();
    Identity {
        client: None,
        subject: format!("sub-{user}"),
        username: user.clone(),
        email: Some(email.into()),
        name: Some(user),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn cookies(config: &Config, email: &str) -> String {
    let now = session::now_unix();
    let s = Session {
        identity: person(email),
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let session: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().to_owned())
        .collect();
    format!("{}; {CSRF_COOKIE}={CSRF}", session.join("; "))
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

fn binding(name: &str, subject: &str, role: &str, scope: Value) -> ResourceEnvelope {
    envelope(
        "RoleBinding",
        name,
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": subject }], "role": role, "scope": scope }),
    )
}

/// The manifests the cases change, as the forge holds them: (namespace, kind, name, spec).
fn manifests() -> Vec<ResourceEnvelope> {
    vec![
        binding(
            BINDING,
            "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14",
            "access-viewer",
            json!({ "project": PROJECT }),
        ),
        envelope(
            "Role",
            ROLE,
            PROJECT,
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
        ),
        envelope(
            "Endpoint",
            "air",
            PROJECT,
            json!({ "contextSpaceRef": PROJECT, "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
                    "audience": "internal", "enabledRepresentations": ["ngsi-ld"] }),
        ),
    ]
}

fn yaml_of(env: &ResourceEnvelope) -> (String, String) {
    let ns = env.metadata.namespace.clone().unwrap_or_default();
    let info = resource::by_kind(&env.kind).expect("a served kind");
    let space = env.spec.get("contextSpaceRef").and_then(Value::as_str);
    let path = resource::repository_path(info, &ns, space, &env.metadata.name).expect("a path");
    let yaml = format!(
        "apiVersion: {API_VERSION}\nkind: {}\nmetadata:\n  name: {}\n  namespace: {ns}\nspec: {}\n",
        env.kind, env.metadata.name, env.spec
    );
    (path, yaml)
}

/// A forge that holds [`manifests`], takes a proposal as pull request 1 authored by `author`
/// and touching `touched`, and merges it; a mirror of the Organization `bb` (the installation),
/// its project, the manifests and five people:
/// - ADMIN and SECOND administer every access kind across `bb`;
/// - VIEWER reads them; BINDER proposes RoleBindings across `bb`; STEWARD proposes and
///   approves Roles and RoleBindings in the project;
/// - DEVELOPER proposes Endpoints that are not public (PF-71).
async fn forge(author: &str, touched: &str) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let repo = "/odata/Repositories(Owner='test-owner',Name='test-repo')";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    Mock::given(method("GET"))
        .and(path(repo))
        .respond_with(ok(json!({ "DefaultBranch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    let mut tree = Vec::new();
    for env in manifests() {
        let (file, yaml) = yaml_of(&env);
        Mock::given(method("GET"))
            .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .respond_with(ok(
                json!({ "Sha": "blob-1", "Content": STANDARD.encode(&yaml) }),
            ))
            .mount(&server)
            .await;
        tree.push(json!({ "Path": file, "Type": "blob", "Sha": "blob-1", "Mode": "100644" }));
    }
    Mock::given(method("GET"))
        .and(path(format!(
            "{repo}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .respond_with(ok(
            json!({ "Sha": "tree-1", "Truncated": false, "Tree": tree }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/Shako.CommitFiles")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c1" })))
        .mount(&server)
        .await;
    let user = author.split('@').next().unwrap_or_default();
    let pull = json!({
        "Number": 1,
        "HtmlUrl": "https://shako.example.sk/pulls/1",
        "State": "open",
        "Title": "update a manifest",
        "Head": "portal/update-rolebinding-air-readers-11111111",
        "HeadSha": "head-1",
        "Base": "main",
        "CreatedAt": "2026-11-12T09:14:22Z",
        "Author": user,
        "AuthorName": user,
        "AuthorEmail": author,
        "Mergeable": true,
        "Merged": false
    });
    Mock::given(method("POST"))
        .and(path(format!("{repo}/Shako.OpenPullRequest")))
        .respond_with(ResponseTemplate::new(201).set_body_json(pull.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)")))
        .respond_with(ok(pull))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)/Files")))
        .respond_with(ok(
            json!({ "value": [{ "Filename": touched, "Status": "modified" }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/PullRequests(1)/Shako.Merge")))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;

    let client = ShakoClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_shako(Arc::new(client));
    let m = &state.mirror;
    m.upsert(envelope(
        "Organization",
        "bb",
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    m.upsert(envelope(
        "Project",
        PROJECT,
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb" }),
    ));
    m.upsert(envelope("ContextSpace", PROJECT, PROJECT, json!({})));
    let access = json!([
        "Project",
        "ContextSpace",
        "Endpoint",
        "Role",
        "RoleBinding",
        "Group"
    ]);
    for (role, rules) in [
        (
            "access-admin",
            json!([{ "kinds": access, "verbs": ["read", "propose", "approve", "delete"] }]),
        ),
        (
            "access-viewer",
            json!([{ "kinds": access, "verbs": ["read"] }]),
        ),
        (
            "project-steward",
            json!([{ "kinds": ["Project", "ContextSpace", "Role", "RoleBinding"],
                     "verbs": ["read", "propose", "approve"] }]),
        ),
        (
            "binder",
            json!([{ "kinds": ["Project", "RoleBinding"], "verbs": ["read", "propose"] }]),
        ),
        (
            "endpoint-developer",
            json!([
                { "kinds": ["Project", "ContextSpace", "Endpoint"], "verbs": ["read"] },
                { "kinds": ["Endpoint"], "verbs": ["propose"],
                  "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
            ]),
        ),
    ] {
        m.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
    }
    let org = json!({ "organization": "bb" });
    let project = json!({ "project": PROJECT });
    for (name, who, role, scope) in [
        ("admin", ADMIN, "access-admin", &org),
        ("second", SECOND, "access-admin", &org),
        ("viewer", VIEWER, "access-viewer", &org),
        ("binder", BINDER, "binder", &org),
        ("steward", STEWARD, "project-steward", &project),
        ("developer", DEVELOPER, "endpoint-developer", &project),
    ] {
        m.upsert(binding(
            &format!("{name}-binding"),
            who,
            role,
            scope.clone(),
        ));
    }
    for env in manifests() {
        m.upsert(env);
    }
    (server, state)
}

struct Answer {
    status: StatusCode,
    body: Value,
    text: String,
}

async fn send(
    state: &AppState,
    who: &str,
    method: &str,
    uri: impl AsRef<str>,
    if_match: Option<&str>,
    content_type: &str,
    body: impl std::borrow::Borrow<Value>,
) -> Answer {
    let body = body.borrow();
    let mut request = Request::builder()
        .method(method)
        .uri(uri.as_ref())
        .header(header::COOKIE, cookies(&state.config, who))
        .header(CSRF_HEADER, CSRF)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(tag) = if_match {
        request = request.header(header::IF_MATCH, tag);
    }
    let body = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let response = server::app(state.clone())
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Answer {
        status,
        body: serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    }
}

/// One proposal through both doors: the OData action on `entity` and the REST `PATCH` of
/// `rest`, the same merge patch, with the manifest's ETag as If-Match. A proposal is checked
/// first (PF-57), as the Portal's form does.
async fn both(
    state: &AppState,
    who: &str,
    (entity, rest): (&str, &str),
    (namespace, kind, name): (&str, &str, &str),
    patch: &Value,
    dry_run: bool,
) -> (Answer, Answer) {
    let etag = state.mirror.get(namespace, kind, name).expect(name).etag();
    let odata = |dry: bool| {
        send(
            state,
            who,
            "POST",
            format!("/api/v1/{entity}/JoinedContext.Portal.propose_update"),
            Some(&etag),
            "application/json",
            json!({ "patch": patch, "dryRun": dry }),
        )
    };
    let rest = |dry: bool| {
        send(
            state,
            who,
            "PATCH",
            format!("{rest}{}", if dry { "?dryRun=All" } else { "" }),
            Some(&etag),
            "application/merge-patch+json",
            patch.clone(),
        )
    };
    if dry_run {
        return (odata(true).await, rest(true).await);
    }
    // Each door's check is the verdict its own proposal spends.
    odata(true).await;
    let proposed = odata(false).await;
    rest(true).await;
    (proposed, rest(false).await)
}

const BINDING_DOORS: (&str, &str) = (
    "rolebindings('air-readers')",
    "/api/v1/projects/org/rolebindings/air-readers",
);
const BINDING_ID: (&str, &str, &str) = (ORG_NAMESPACE, "RoleBinding", BINDING);

async fn merged(server: &MockServer) -> bool {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|r| r.url.path().ends_with("/PullRequests(1)/Shako.Merge"))
}

fn binding_file() -> String {
    yaml_of(&manifests()[0]).0
}

/// PF-52: a reader of RoleBindings proposes none (403 in both doors); a grant above the
/// proposer's own rights is refused alike; an administrator's binding change opens a red-lane
/// Change in both.
#[tokio::test]
async fn pf52_access_changes_keep_the_role_model_through_odata() {
    let (_server, state) = forge(ADMIN, &binding_file()).await;
    let widen = json!({ "spec": { "role": "access-admin" } });
    let (odata, rest) = both(&state, VIEWER, BINDING_DOORS, BINDING_ID, &widen, true).await;
    assert_eq!(odata.status, StatusCode::FORBIDDEN, "{}", odata.text);
    assert_eq!(odata.status, rest.status, "REST: {}", rest.text);

    // A project steward's binding reads no Organization set (T-6822): their binding proposals
    // keep to the REST door and MCP, and the OData row is not there for them.
    let (odata, _) = both(&state, STEWARD, BINDING_DOORS, BINDING_ID, &widen, true).await;
    assert_eq!(odata.status, StatusCode::NOT_FOUND, "{}", odata.text);

    // The binder proposes RoleBindings but holds no `approve`, `delete` or Endpoint right: a
    // binding to the admin Role grants more than they hold.
    let (odata, rest) = both(
        &state,
        BINDER,
        BINDING_DOORS,
        BINDING_ID,
        &json!({ "spec": { "role": "access-admin" } }),
        true,
    )
    .await;
    assert_eq!(odata.status, StatusCode::FORBIDDEN, "{}", odata.text);
    assert!(odata.text.contains("PF-52"), "{}", odata.text);
    assert_eq!(odata.status, rest.status, "REST: {}", rest.text);

    let rename = json!({ "metadata": { "title": "Air readers" } });
    let (odata, rest) = both(&state, ADMIN, BINDING_DOORS, BINDING_ID, &rename, false).await;
    for (door, answer) in [("OData", &odata), ("REST", &rest)] {
        assert_eq!(
            answer.status,
            StatusCode::ACCEPTED,
            "{door}: {}",
            answer.text
        );
        let lane = &answer.body["status"]["lane"];
        assert!(
            lane.as_str().is_some_and(|l| l.eq_ignore_ascii_case("red")),
            "{door}: {}",
            answer.text
        );
    }
}

/// PF-58: an administrator's own access change proposed through OData waits; the author's
/// approval is `403 self-approval` and merges nothing; a second administrator merges it.
#[tokio::test]
async fn pf58_the_author_of_an_access_change_never_approves_it() {
    let (server, state) = forge(ADMIN, &binding_file()).await;
    let rename = json!({ "metadata": { "title": "Air readers" } });
    let (odata, rest) = both(&state, ADMIN, BINDING_DOORS, BINDING_ID, &rename, false).await;
    for (door, answer) in [("OData", &odata), ("REST", &rest)] {
        assert_eq!(
            answer.status,
            StatusCode::ACCEPTED,
            "{door}: {}",
            answer.text
        );
        assert_eq!(
            answer.body["status"]["phase"], "PendingApproval",
            "{door}: {}",
            answer.text
        );
    }
    assert!(!merged(&server).await, "proposing merged an access change");
    assert_eq!(
        odata.body["metadata"]["name"], "chg-00000001",
        "{}",
        odata.text
    );
    // The Organization's Changes are read by project `org`, pull request 1.
    let approve = "/api/v1/projects/org/changes/chg-org-00000001/approve";
    let confirm = json!({ "confirm": BINDING });
    let own = send(
        &state,
        ADMIN,
        "POST",
        approve,
        None,
        "application/json",
        &confirm,
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    assert!(own.text.contains("self-approval"), "{}", own.text);
    assert!(
        !merged(&server).await,
        "the author merged their own access change"
    );
    let second = send(
        &state,
        SECOND,
        "POST",
        approve,
        None,
        "application/json",
        &confirm,
    )
    .await;
    assert!(second.status.is_success(), "{}", second.text);
    assert!(
        merged(&server).await,
        "a second administrator did not merge it"
    );
}

/// PF-70: a project steward approves Roles in the red lane, but their own Role change waits for
/// another approver, through OData as through REST.
#[tokio::test]
async fn pf70_a_stewards_own_role_change_waits() {
    let file = yaml_of(&manifests()[1]).0;
    let (server, state) = forge(STEWARD, &file).await;
    let doors = (
        "roles(project='ovzdusie',name='air-steward')",
        "/api/v1/projects/ovzdusie/roles/air-steward",
    );
    let rename = json!({ "metadata": { "title": "Air stewards" } });
    let (odata, rest) = both(
        &state,
        STEWARD,
        doors,
        (PROJECT, "Role", ROLE),
        &rename,
        false,
    )
    .await;
    for (door, answer) in [("OData", &odata), ("REST", &rest)] {
        assert_eq!(
            answer.status,
            StatusCode::ACCEPTED,
            "{door}: {}",
            answer.text
        );
        assert_eq!(
            answer.body["status"]["phase"], "PendingApproval",
            "{door}: {}",
            answer.text
        );
    }
    assert!(!merged(&server).await, "a steward's own change merged");
}

/// PF-71: a `propose` constrained to `spec.audience` not in `[public]` plans an internal
/// Endpoint and is refused making it public, in both doors.
#[tokio::test]
async fn pf71_making_an_endpoint_public_is_a_right_of_its_own() {
    let (_server, state) = forge(DEVELOPER, &yaml_of(&manifests()[2]).0).await;
    let doors = (
        "endpoints(project='ovzdusie',name='air')",
        "/api/v1/projects/ovzdusie/endpoints/air",
    );
    let id = (PROJECT, "Endpoint", "air");
    for (audience, status) in [
        ("internal", StatusCode::OK),
        ("public", StatusCode::FORBIDDEN),
    ] {
        let patch = json!({ "metadata": { "title": "Air" }, "spec": { "audience": audience } });
        let (odata, rest) = both(&state, DEVELOPER, doors, id, &patch, true).await;
        assert_eq!(odata.status, status, "{audience}: {}", odata.text);
        assert_eq!(odata.status, rest.status, "{audience} REST: {}", rest.text);
    }
}
