//! Importing a bundle into a project against a mocked forge (T-0193, MF-20…MF-26).
//!
//! An import is the one write that touches many resources at once, so what these tests check
//! is what an operator cannot see by reading the diff: that the bundle was refused whole when
//! any part of it was wrong, that the conflict policy did what it says, and that a rename
//! carried every reference with it. A bundle that half-imports is worse than one that does
//! not, because the half that landed is now a project nobody described.

mod common;

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request as MockRequest, ResponseTemplate};

const PROJECT: &str = "banskabystrica";
const SOURCE: &str = "helsinki";
const CSRF: &str = "test-csrf-token-12345";

const SPACE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: ovzdusie
  namespace: helsinki
  title: { en: Air quality }
spec:
  defaultLocale: en
"#;

const ENDPOINT: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: public-air
  namespace: helsinki
spec:
  contextSpaceRef: ovzdusie
  slug: mluyob4nz52lok3ssk7pgn5vwt
  audience: public
  enabledRepresentations: [ngsi-ld]
"#;

/// Where an entity selector legitimately lives (R6): the Policy of the endpoint's space. The ids
/// and the anchored pattern are what an import has to move from one organisation to another
/// (MF-22, PF-43).
const POLICY: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Policy
metadata:
  name: public-air
  namespace: helsinki
spec:
  contextSpaceRef: ovzdusie
  assigner: did:web:hel.fi
  assignee: { kind: role, id: public }
  operations: [queryEntity, retrieveEntity]
  information:
    - entities:
        - type: AirQualityObserved
          id: urn:ngsi-ld:AirQualityObserved:hel.fi:ovzdusie:sever-01
        - type: AirQualityObserved
          id: urn:ngsi-ld:AirQualityObserved:tampere.fi:ilma:keskusta-01
        - type: AirQualityObserved
          idPattern: "^urn:ngsi-ld:AirQualityObserved:hel\\.fi:ovzdusie:.*$"
"#;

const ROLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Role
metadata:
  name: air-steward
  namespace: org
spec:
  rules:
    - kinds: [Endpoint]
      verbs: [read]
"#;

const PROJECT_MANIFEST: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: helsinki
  namespace: org
  title: { en: Helsinki }
spec:
  organizationRef: hel
"#;

const BENTO: &str = "input:\n  mqtt:\n    urls: [ mqtts://mqtt.hsl.fi:8883 ]\n";

const BUNDLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Bundle
metadata:
  name: helsinki
  namespace: org
spec:
  exportedAt: "2026-09-06T16:30:00Z"
  exportedBy: aino.virtanen
  sourceRevision: 8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8
  items:
    - { kind: ContextSpace, namespace: helsinki, name: ovzdusie, path: projects/helsinki/spaces/ovzdusie/space.yaml }
    - { kind: Endpoint, namespace: helsinki, name: public-air, path: projects/helsinki/endpoints/public-air.yaml }
  nativeFiles:
    - projects/helsinki/pipelines/aq/bento.yaml
  omitted: 0
"#;

fn archive(files: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files {
        writer.start_file(*name, options).expect("archive entry");
        writer.write_all(content.as_bytes()).expect("archive write");
    }
    writer.finish().expect("archive closes").into_inner()
}

fn bundle_archive() -> Vec<u8> {
    archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("bundle.yaml", BUNDLE),
    ])
}

/// A multipart body with one file part and the wizard's option fields.
fn multipart(file: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "jcimportboundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"bundle.zip\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "portal/import" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/contents"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "content": { "sha": "b1ob" },
            "commit": { "sha": "c0mm1t" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 412,
            "html_url": "https://git.example.sk/bb/org/pulls/412",
            "title": "import",
            "state": "open",
            "head": { "ref": "portal/import" },
            "base": { "ref": "main" }
        })))
        .mount(&server)
        .await;
    server
}

fn session_cookie(config: &Config, groups: &[&str]) -> String {
    session_cookie_of(config, "jana.kovacova", groups)
}

fn session_cookie_of(config: &Config, username: &str, groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The session and the double-submit CSRF cookie, which every UI mutation carries.
fn cookies(config: &Config, groups: &[&str]) -> String {
    format!("{}; {CSRF_COOKIE}={CSRF}", session_cookie(config, groups))
}

/// The bootstrap administrator of `Config::for_tests`, who may propose every kind.
fn state(server: &MockServer, existing: Vec<&str>) -> (AppState, String) {
    state_as(server, existing, &["portal-approver"], vec![])
}

/// A person in no group, whose rights are exactly the organization's `Role`s and
/// `RoleBinding`s in `org` (T-0798, PF-50).
fn state_as(
    server: &MockServer,
    existing: Vec<&str>,
    groups: &[&str],
    org: Vec<Value>,
) -> (AppState, String) {
    state_with(Config::for_tests(), server, existing, groups, org)
}

fn state_with(
    config: Config,
    server: &MockServer,
    existing: Vec<&str>,
    groups: &[&str],
    org: Vec<Value>,
) -> (AppState, String) {
    let cookie = cookies(&config, groups);
    let gitea = Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "bb",
            "org",
            "token",
        )
        .expect("gitea client"),
    );
    let mirror = Arc::new(Mirror::new());
    for yaml in existing {
        let mut envelope: ResourceEnvelope = serde_norway::from_str(yaml).expect("manifest");
        envelope.metadata.namespace = Some(PROJECT.to_string());
        mirror.upsert(envelope);
    }
    // The organization the import lands in: its domain is the one every imported id is
    // rewritten to (PF-10, PF-43).
    mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION,
            "kind": "Organization",
            "metadata": { "name": "bb", "namespace": ORG_NAMESPACE },
            "spec": { "displayName": { "en": "Banska Bystrica" }, "domain": "banskabystrica.sk" },
        }))
        .expect("organization manifest"),
    );
    for manifest in org {
        mirror.upsert(serde_json::from_value(manifest).expect("organization manifest"));
    }
    let state = AppState::new(config, None)
        .with_gitea(gitea)
        .with_mirror(mirror);
    (state, cookie)
}

/// An import the way the pages send it: the dry run first, which is the bundle's check (PF-57,
/// T-1460), then the same request.
async fn post(
    state: AppState,
    cookie: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let uri = format!("/api/v1/projects/{PROJECT}/import");
    send(
        state.clone(),
        cookie,
        content_type,
        body.clone(),
        &format!("{uri}?dryRun=All"),
    )
    .await;
    send(state, cookie, content_type, body, &uri).await
}

async fn send(
    state: AppState,
    cookie: &str,
    content_type: &str,
    body: Vec<u8>,
    uri: &str,
) -> (StatusCode, Value) {
    let app = server::app(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, content_type)
                .header(CSRF_HEADER, CSRF)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the request completes");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn upload(
    server: &MockServer,
    existing: Vec<&str>,
    fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (state, cookie) = state(server, existing);
    let (content_type, body) = multipart(&bundle_archive(), fields);
    post(state, &cookie, &content_type, body).await
}

async fn written(server: &MockServer) -> Vec<String> {
    common::committed_files(server, "/api/v1/repos/bb/org")
        .await
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

async fn put_bodies(server: &MockServer) -> Vec<String> {
    common::committed_files(server, "/api/v1/repos/bb/org")
        .await
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

/// A `Role` of the organization repository with the given rules.
fn role(name: &str, rules: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Role",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": { "rules": rules },
    })
}

/// The signed-in person holds `role` on this project.
fn binding(role: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": format!("jana-{role}"), "namespace": ORG_NAMESPACE },
        "spec": {
            "subjects": [{ "user": "jana.kovacova" }],
            "role": role,
            "scope": { "project": PROJECT },
        },
    })
}

/// Everything the forge saw, as `METHOD path`.
async fn forge_calls(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

// --- the happy path ---------------------------------------------------------------------

/// TS-19: a bundle is one commit in the person's name, whatever its size, and no file is read
/// or written on its own: seven files one at a time passed the edge's 60 s on a busy repository.
#[tokio::test]
async fn a_bundle_lands_as_one_commit() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![], &[("conflictPolicy", "fail")]).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let requests = server.received_requests().await.unwrap_or_default();
    let to_contents: Vec<&MockRequest> = requests
        .iter()
        .filter(|r| r.url.path().contains("/contents"))
        .collect();
    assert_eq!(to_contents.len(), 1, "{to_contents:?}");
    assert_eq!(to_contents[0].method.as_str(), "POST");
    let commit: Value = serde_json::from_slice(&to_contents[0].body).expect("json");
    assert!(
        commit["files"].as_array().is_some_and(|f| f.len() > 1),
        "{commit}"
    );
    assert!(
        commit["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("import ")),
        "{commit}"
    );
    assert_eq!(commit["author"]["name"], commit["committer"]["name"]);
}

/// All or nothing: a commit the forge refuses leaves no Change behind, and nobody approves half
/// a bundle.
#[tokio::test]
async fn a_commit_the_forge_refuses_opens_no_merge_request() {
    let server = forge().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/contents"))
        .respond_with(ResponseTemplate::new(500))
        .with_priority(1)
        .mount(&server)
        .await;
    let (status, body) = upload(&server, vec![], &[("conflictPolicy", "fail")]).await;

    assert!(status.is_server_error(), "{status}: {body}");
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        !requests.iter().any(|r| r.url.path().ends_with("/pulls")),
        "{requests:?}"
    );
}

#[tokio::test]
async fn an_archive_becomes_one_merge_request_for_the_whole_bundle() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![], &[("conflictPolicy", "fail")]).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["kind"], "Change");
    assert_eq!(
        body["status"]["mergeRequest"],
        "https://git.example.sk/bb/org/pulls/412"
    );
    // MF-21, CC-63: one merge request, one lane, one diff for the bundle.
    assert_eq!(body["status"]["plan"]["create"], 2);

    let paths = written(&server).await;
    // An Endpoint lives under the space it serves, and the space segment travels with it.
    assert!(
        paths
            .iter()
            .any(|p| p == "projects/banskabystrica/spaces/ovzdusie/endpoints/public-air.yaml"),
        "{paths:?}"
    );
    // The native file travels beside its manifest, reprojected into this project.
    assert!(
        paths
            .iter()
            .any(|p| p == "projects/banskabystrica/pipelines/aq/bento.yaml"),
        "{paths:?}"
    );
    // The bundle index describes the bundle and is not a resource of the project (MF-17).
    assert!(
        !paths.iter().any(|p| p.ends_with("bundle.yaml")),
        "{paths:?}"
    );
}

#[tokio::test]
async fn every_imported_manifest_says_where_it_came_from() {
    let server = forge().await;
    upload(&server, vec![], &[]).await;

    let manifests: Vec<String> = put_bodies(&server)
        .await
        .into_iter()
        .filter(|body| body.contains("kind: Endpoint"))
        .collect();
    assert_eq!(manifests.len(), 1);
    // MF-20: provenance travels with the object, and the bundle index is what names the source.
    assert!(
        manifests[0].contains("joinedcontext.com/imported-from")
            && manifests[0].contains("helsinki@8c56954"),
        "{}",
        manifests[0]
    );
}

/// The Policy is where the entity selector lives (R6), so it is the manifest whose URNs an import
/// has to move; the Endpoint moves its namespace and its references (T-2253 corrected the fixture:
/// an Endpoint has no `entitySelector`, and a manifest that carries one is refused at every door).
#[tokio::test]
async fn the_namespace_the_typed_references_and_the_urns_all_move() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
            (
                "projects/helsinki/spaces/ovzdusie/policies/public-air.yaml",
                POLICY,
            ),
        ]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let written = put_bodies(&server).await;
    let endpoint = written
        .iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    let policy = written
        .iter()
        .find(|body| body.contains("kind: Policy"))
        .expect("the policy was written");
    assert!(
        endpoint.contains(&format!("namespace: {PROJECT}")),
        "{endpoint}"
    );
    // MF-22, PF-43: the organisation segment of an id moves to the organisation that now owns
    // the space, or the gateway refuses every write the imported endpoint selects. The space
    // segment is a ContextSpace name and keeps it.
    assert!(
        policy.contains("urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:sever-01"),
        "{policy}"
    );
    // An id in a space this bundle does not carry is another city's, and a federated
    // registration that pointed there still does (MF-22).
    assert!(
        policy.contains("urn:ngsi-ld:AirQualityObserved:tampere.fi:ilma:keskusta-01"),
        "{policy}"
    );
    // An anchored idPattern is a URN prefix with the domain's dots escaped (R33, T-0826).
    assert!(
        policy.contains(r"^urn:ngsi-ld:AirQualityObserved:banskabystrica\.sk:ovzdusie:.*$"),
        "{policy}"
    );
    // The one place the source survives is the provenance annotation (MF-20).
    assert!(
        !endpoint.contains(&format!("namespace: {SOURCE}")),
        "{endpoint}"
    );
    assert!(!endpoint.contains("hel.fi"), "{endpoint}");
    assert!(
        policy.contains(&format!("namespace: {PROJECT}")),
        "{policy}"
    );
    // CC-82, R6, T-2256: the grant is this organisation's now, so its assigner is the placeholder
    // the loader renders for whoever owns the file, and the source's DID is gone.
    assert!(policy.contains("assigner: did:web:{orgDomain}"), "{policy}");
    assert!(!policy.contains("did:web:hel.fi"), "{policy}");
}

/// R6, CC-82, T-2256: an assigner the bundle carried from another organisation is rewritten to
/// `did:web:{orgDomain}`, and the plan a person approves lists each one it rewrote with the DID it
/// carried; a Policy already naming the placeholder is not listed, and the Change body says it too.
#[tokio::test]
async fn an_imported_policy_is_signed_by_this_organisation_and_the_plan_says_so() {
    let own = POLICY
        .replace("name: public-air", "name: own-air")
        .replace("assigner: did:web:hel.fi", "assigner: did:web:{orgDomain}");
    let files = [
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        (
            "projects/helsinki/spaces/ovzdusie/policies/public-air.yaml",
            POLICY,
        ),
        (
            "projects/helsinki/spaces/ovzdusie/policies/own-air.yaml",
            own.as_str(),
        ),
    ];

    let server = forge().await;
    let (checked, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&archive(&files), &[("dryRun", "true")]);
    let (status, report) = post(checked, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["reassigned"],
        json!({ "Policy/public-air": "did:web:hel.fi" }),
        "{report}"
    );

    let server = forge().await;
    let (imported, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&archive(&files), &[]);
    let (status, answer) = post(imported, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    let pull = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|request| request.method.as_str() == "POST" && request.url.path().ends_with("/pulls"))
        .expect("a merge request");
    let pull: Value = serde_json::from_slice(&pull.body).expect("json");
    let body = pull["body"].as_str().unwrap_or_default();
    assert!(
        body.contains("Policy/public-air: assigner did:web:hel.fi is now did:web:{orgDomain}"),
        "{body}"
    );
}

#[tokio::test]
async fn an_organization_scoped_kind_lands_in_org_and_the_source_project_manifest_is_dropped() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let bundle = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("users/roles/air-steward.yaml", ROLE),
        ("projects/helsinki/project.yaml", PROJECT_MANIFEST),
        ("bundle.yaml", BUNDLE),
    ]);
    let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let paths = written(&server).await;
    // PF-22: an organization-scoped kind is stored at `users/roles/{name}.yaml`, namespace
    // `org`; jc-core refuses it under a project's namespace.
    assert!(
        paths.iter().any(|p| p == "users/roles/air-steward.yaml"),
        "{paths:?}"
    );
    // The destination project is the one in the URL, so the bundle's own Project manifest
    // never writes a `projects/helsinki/` directory into this repository.
    assert!(
        !paths.iter().any(|p| p.starts_with("projects/helsinki/")),
        "{paths:?}"
    );
    let role = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Role"))
        .expect("the role was written");
    assert!(role.contains("namespace: org"), "{role}");
    assert!(!role.contains(&format!("namespace: {PROJECT}")), "{role}");
}

#[tokio::test]
async fn a_dry_run_reports_what_it_would_do_and_writes_nothing() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&bundle_archive(), &[("dryRun", "true")]);
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["created"], json!(["ovzdusie", "public-air"]));
    assert_eq!(report["nativeFiles"], 1);
    // A space and an endpoint carry nothing that stays behind (CC-84).
    assert_eq!(report["needs"], json!([]), "{report}");
    assert_eq!(
        report["source"],
        "helsinki@8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8"
    );
    assert!(
        written(&server).await.is_empty(),
        "a dry run must not write"
    );
}

#[tokio::test]
async fn the_readme_and_schemas_of_a_complete_export_are_never_written_into_the_project() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let complete = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("bundle.yaml", BUNDLE),
        ("README.md", "# Project helsinki\n"),
        (
            "schemas/kinds/Endpoint.schema.json",
            "{\"type\": \"object\"}",
        ),
        (
            "schemas/models/air/air.linkml.yaml",
            "id: https://example.org/air\n",
        ),
    ]);
    let (content_type, body) = multipart(&complete, &[("dryRun", "true")]);
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["created"], json!(["ovzdusie", "public-air"]));
    assert_eq!(
        report["nativeFiles"], 1,
        "only bento.yaml is the project's own native file (MF-41): {report}"
    );
}

#[tokio::test]
async fn a_manifest_posted_as_json_is_imported_too() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let manifest: Value = serde_norway::from_str(SPACE).expect("the space parses");
    let body = json!({ "manifests": { "apiVersion": "joinedcontext.com/v1alpha1",
                                      "kind": "List", "items": [manifest] } });
    let (status, answer) = post(
        state,
        &cookie,
        "application/json",
        serde_json::to_vec(&body).expect("body"),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    assert_eq!(written(&server).await.len(), 1);
}

/// T-1226: `?dryRun=All` is the spelling the OpenAPI document, the endpoint form's **Check** and
/// the generated client all use. `ImportQuery` bound `dry_run` instead, so the parameter was
/// silently ignored: a button that says "check" committed a change and opened a merge request,
/// and the next check answered `409 … is already open`. The multipart field was always parsed,
/// which is why the dry-run tests above never saw it.
#[tokio::test]
async fn a_dry_run_asked_for_in_the_query_writes_nothing_either() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let manifest: Value = serde_norway::from_str(SPACE).expect("the space parses");
    let body = json!({ "manifests": { "apiVersion": "joinedcontext.com/v1alpha1",
                                      "kind": "List", "items": [manifest] } });
    let app = server::app(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{PROJECT}/import?dryRun=All"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header(CSRF_HEADER, CSRF)
                .body(Body::from(serde_json::to_vec(&body).expect("body")))
                .unwrap(),
        )
        .await
        .expect("response");
    let status = response.status();
    let answer: Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .unwrap_or(Value::Null);

    assert_eq!(
        status,
        StatusCode::OK,
        "a dry run answers the report: {answer}"
    );
    assert_eq!(answer["created"], json!(["ovzdusie"]), "{answer}");
    assert!(
        answer.get("status").is_none(),
        "a dry run answers a report, never a Change: {answer}"
    );
    assert!(
        written(&server).await.is_empty(),
        "the check wrote to the repository"
    );
}

// --- the conflict policy (MF-23) ---------------------------------------------------------

#[tokio::test]
async fn fail_refuses_the_whole_bundle_when_one_resource_already_exists() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![ENDPOINT], &[("conflictPolicy", "fail")]).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("public-air"),
        "{body}"
    );
    // The refusal is whole: the space that had no conflict must not have landed either.
    assert!(
        written(&server).await.is_empty(),
        "a refused import must write nothing"
    );
}

#[tokio::test]
async fn skip_leaves_the_existing_resource_alone_and_imports_the_rest() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![ENDPOINT]);
    let (content_type, body) = multipart(
        &bundle_archive(),
        &[("conflictPolicy", "skip"), ("dryRun", "true")],
    );
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["skipped"], json!(["public-air"]));
    assert_eq!(report["created"], json!(["ovzdusie"]));
}

#[tokio::test]
async fn replace_overwrites_the_existing_resource() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![ENDPOINT], &[("conflictPolicy", "replace")]).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"]["plan"]["update"], 1);
    assert!(written(&server)
        .await
        .iter()
        .any(|p| p.contains("public-air.yaml")));
}

#[tokio::test]
async fn rename_imports_under_a_new_name_and_carries_the_references_with_it() {
    let server = forge().await;
    // The space is the one that collides, and the endpoint points at it by name.
    let (state, cookie) = state(&server, vec![SPACE]);
    let (content_type, body) = multipart(&bundle_archive(), &[("conflictPolicy", "rename")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|written| written.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    // MF-26: a rename that does not move the references produces a bundle that imports and
    // then dangles, which is the failure mode project duplication exists to avoid.
    assert!(
        endpoint.contains("contextSpaceRef: ovzdusie-helsinki"),
        "{endpoint}"
    );
}

// --- the gates (MF-24) --------------------------------------------------------------------

#[tokio::test]
async fn a_literal_secret_anywhere_in_the_bundle_refuses_it() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let leaky = ENDPOINT.replace(
        "  audience: public\n",
        "  audience: public\n  publish:\n    apiKey: jc_dead_beef\n",
    );
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/endpoints/public-air.yaml", &leaky),
        ]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("apiKey"),
        "{answer}"
    );
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_reference_to_a_resource_nobody_has_refuses_the_bundle() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    // The endpoint alone: the space it names is in neither the bundle nor the project.
    let (content_type, body) = multipart(
        &archive(&[("projects/helsinki/endpoints/public-air.yaml", ENDPOINT)]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("ContextSpace 'ovzdusie'"), "{detail}");
}

#[tokio::test]
async fn a_reference_the_project_already_holds_resolves() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![SPACE]);
    let (content_type, body) = multipart(
        &archive(&[("projects/helsinki/endpoints/public-air.yaml", ENDPOINT)]),
        &[("conflictPolicy", "skip")],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
}

#[tokio::test]
async fn a_status_block_is_refused_because_the_platform_computes_it() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let with_status = format!("{ENDPOINT}status:\n  phase: Live\n");
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/endpoints/public-air.yaml", &with_status),
        ]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("MF-04"),
        "{answer}"
    );
}

#[tokio::test]
async fn a_kind_this_platform_does_not_serve_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let alien = SPACE.replace("kind: ContextSpace", "kind: DeploymentConfig");
    let (content_type, body) = multipart(&archive(&[("projects/helsinki/x.yaml", &alien)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("DeploymentConfig"),
        "{answer}"
    );
}

#[tokio::test]
async fn an_unsupported_api_version_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let old = SPACE.replace("joinedcontext.com/v1alpha1", "joinedcontext.com/v1beta9");
    let (content_type, body) = multipart(&archive(&[("projects/helsinki/x.yaml", &old)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("v1beta9"),
        "{answer}"
    );
}

#[tokio::test]
async fn an_archive_that_escapes_its_root_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&archive(&[("../../etc/passwd.yaml", SPACE)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn importing_from_a_url_says_it_is_not_implemented_rather_than_fetching() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let body = json!({ "url": "http://169.254.169.254/latest/meta-data" });
    let (status, answer) = post(
        state,
        &cookie,
        "application/json",
        serde_json::to_vec(&body).expect("body"),
    )
    .await;

    // MF-20 allows a URL and the Portal has no egress policy for one; fetching whatever a
    // caller names would be inventing an SSRF surface, so it is refused in the open.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
}

#[tokio::test]
async fn an_anonymous_import_is_refused_before_anything_is_parsed() {
    let server = forge().await;
    let (state, _) = state(&server, vec![]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let app = server::app(state);
    // A matching double-submit pair, so the CSRF guard lets the request through and the
    // missing session is what refuses it.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{PROJECT}/import"))
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the request completes");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(written(&server).await.is_empty());
}

// --- who may import (T-0798, PF-50) -----------------------------------------------------

#[tokio::test]
async fn an_import_by_someone_with_no_binding_is_refused_before_anything_is_planned() {
    let server = forge().await;
    for fields in [&[][..], &[("dryRun", "All")][..]] {
        let (state, cookie) = state_as(&server, vec![], &[], vec![]);
        let (content_type, body) = multipart(&bundle_archive(), fields);
        let (status, problem) = post(state, &cookie, &content_type, body).await;

        assert_eq!(status, StatusCode::FORBIDDEN, "{fields:?}: {problem}");
        let detail = problem["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("PF-50"), "{detail}");
    }
    // Not a branch, not a file, not a merge request: the forge was never asked.
    let calls = forge_calls(&server).await;
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("POST") || c.starts_with("PUT")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_role_that_grants_propose_on_every_kind_in_the_bundle_imports_it() {
    let server = forge().await;
    let importer = role(
        "importer",
        json!([{ "kinds": ["ContextSpace", "Endpoint", "Pipeline"], "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(&server, vec![], &[], vec![importer, binding("importer")]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, change) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{change}");
    assert_eq!(change["status"]["plan"]["create"], 2);
    assert!(written(&server)
        .await
        .iter()
        .any(|p| p == "projects/banskabystrica/pipelines/aq/bento.yaml"));
}

#[tokio::test]
async fn a_role_that_misses_one_kind_of_the_bundle_refuses_the_whole_bundle_naming_it() {
    let server = forge().await;
    // ContextSpace and Endpoint, but not the Pipeline whose bento.yaml the bundle carries.
    let partial = role(
        "partial",
        json!([{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(&server, vec![], &[], vec![partial, binding("partial")]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("Pipeline"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_constraint_the_bundles_manifest_violates_refuses_it_naming_the_field() {
    let server = forge().await;
    // The bundle's Endpoint is public; this role may propose internal ones only.
    let internal = role(
        "internal-editor",
        json!([
            { "kinds": ["ContextSpace", "Pipeline"], "verbs": ["propose"] },
            { "kinds": ["Endpoint"], "verbs": ["propose"],
              "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
        ]),
    );
    let (state, cookie) = state_as(
        &server,
        vec![],
        &[],
        vec![internal, binding("internal-editor")],
    );
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("spec.audience"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_binding_on_another_project_grants_nothing_here() {
    let server = forge().await;
    let importer = role(
        "importer",
        json!([{ "kinds": ["ContextSpace", "Endpoint", "Pipeline"], "verbs": ["propose"] }]),
    );
    let mut elsewhere = binding("importer");
    elsewhere["spec"]["scope"] = json!({ "project": SOURCE });
    let (state, cookie) = state_as(&server, vec![], &[], vec![importer, elsewhere]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_native_file_under_a_directory_that_names_no_kind_is_refused_even_with_every_role() {
    let server = forge().await;
    // Every kind the catalogue has, and still no role can cover `projects/x/notes/`.
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let everything = role(
        "everything",
        json!([{ "kinds": kinds, "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(
        &server,
        vec![],
        &[],
        vec![everything, binding("everything")],
    );
    let file = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/notes/todo.txt", "remember the milk\n"),
    ]);
    let (content_type, body) = multipart(&file, &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("notes/todo.txt"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_bundle_carrying_a_built_digest_is_refused_whole() {
    // AP-11, AP-13a (T-0822): an App that names an image is an App that deploys it. The image
    // a bundle carries was built somewhere else, and this cluster runs what it built itself.
    const APP: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: air-map
  namespace: helsinki
  annotations:
    joinedcontext.com/image: "ghcr.io/hel/air-map@sha256:9f2b1c0d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
spec:
  kind: static
  source:
    path: apps/air-map
  build:
    node: "22"
    pnpm: "9"
  visibility: project
  dataNeeds:
    - contextSpaceRef: ovzdusie
      types: [AirQualityObserved]
      operations: [queryEntity]
"#;
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let bundle = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/apps/air-map.yaml", APP),
    ]);
    let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("joinedcontext.com/image"),
        "{answer}"
    );
    // Nothing of the bundle was written: a refusal is whole (MF-24).
    assert!(written(&server).await.is_empty());
}

/// AP-87 (T-2600): a bundle that publishes a static App on a folder of the configuration
/// repository is refused whole, and so is one claiming a shipped bundle this Portal does not hold:
/// the shipped mark of the instance it came from says nothing about this one.
#[tokio::test]
async fn a_bundle_publishing_a_static_app_nothing_can_build_is_refused_whole() {
    let published = |annotation: &str| {
        format!(
            r#"apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: air-map
  namespace: helsinki{annotation}
spec:
  kind: static
  source:
    path: apps/air-map
  build:
    node: "22"
  visibility: project
  lifecycle: published
  dataNeeds:
    - contextSpaceRef: ovzdusie
      types: [AirQualityObserved]
      operations: [queryEntity]
"#
        )
    };
    for (annotation, named) in [
        ("", "spec.source.git"),
        (
            "\n  annotations:\n    joinedcontext.com/shipped-with: portal",
            "joinedcontext.com/shipped-with",
        ),
    ] {
        let server = forge().await;
        let (state, cookie) = state(&server, vec![]);
        let app = published(annotation);
        let bundle = archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/apps/air-map.yaml", &app),
        ]);
        let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
        let (status, answer) = post(state, &cookie, &content_type, body).await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
        let detail = answer["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains(named) && detail.contains("AP-87"),
            "{detail}"
        );
        assert!(written(&server).await.is_empty());
    }
}

/// PF-68, T-0872: a role the bundle wrote inside a project lands inside the destination project,
/// while the organization's own role keeps `users/` — the namespace decides, not the kind alone.
#[tokio::test]
async fn a_role_of_a_project_lands_in_the_destination_project() {
    const PROJECT_ROLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Role
metadata:
  name: air-analyst
  namespace: helsinki
spec:
  rules:
    - kinds: [DataSource]
      verbs: [propose]
"#;

    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let bundle = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("users/roles/air-steward.yaml", ROLE),
        ("projects/helsinki/roles/air-analyst.yaml", PROJECT_ROLE),
        ("bundle.yaml", BUNDLE),
    ]);
    let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let paths = written(&server).await;
    assert!(
        paths.iter().any(|p| p == "users/roles/air-steward.yaml"),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p == &format!("projects/{PROJECT}/roles/air-analyst.yaml")),
        "{paths:?}"
    );
    let analyst = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("air-analyst"))
        .expect("the project role was written");
    assert!(
        analyst.contains(&format!("namespace: {PROJECT}")),
        "{analyst}"
    );
}

// ---------------------------------------------------------------------------
// Verifying a transfer (MF-42)
// ---------------------------------------------------------------------------

/// The lowercase hexadecimal SHA-256, as the bundle index carries it.
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// A manifest as the exporter writes it: parsed and serialised back by the platform's own type,
/// which is the form both sides hash.
fn exported(manifest: &str) -> String {
    let envelope: ResourceEnvelope = serde_norway::from_str(manifest).expect("the fixture parses");
    serde_norway::to_string(&envelope).expect("serialises")
}

/// The index of `bundle_archive()` with a checksum per file, as the current exporter writes it.
fn bundle_with_checksums(space: &str) -> String {
    format!(
        "{BUNDLE}  files:\n\
         {}{}{}",
        format_args!(
            "    - {{ path: projects/helsinki/spaces/ovzdusie/space.yaml, sha256: {} }}\n",
            sha256(exported(space).as_bytes())
        ),
        format_args!(
            "    - {{ path: projects/helsinki/endpoints/public-air.yaml, sha256: {} }}\n",
            sha256(exported(ENDPOINT).as_bytes())
        ),
        format_args!(
            "    - {{ path: projects/helsinki/pipelines/aq/bento.yaml, sha256: {} }}\n",
            sha256(BENTO.as_bytes())
        ),
    )
}

async fn verify_archive(files: &[(&str, &str)]) -> (StatusCode, Value) {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&archive(files), &[("dryRun", "true")]);
    post(state, &cookie, &content_type, body).await
}

#[tokio::test]
async fn a_round_trip_reports_every_file_equal() {
    let index = bundle_with_checksums(SPACE);
    let (status, report) = verify_archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("bundle.yaml", &index),
    ])
    .await;

    assert_eq!(status, StatusCode::OK, "{report}");
    let verified = report["verified"].as_array().expect("verified");
    assert_eq!(verified.len(), 3, "{report}");
    assert!(
        verified.iter().all(|file| file["equal"] == json!(true)),
        "{report}"
    );
}

#[tokio::test]
async fn a_file_edited_on_the_way_is_reported_unequal_and_the_rest_is_not() {
    // The index still carries the checksum of the manifest as it was exported.
    let index = bundle_with_checksums(SPACE);
    let tampered = SPACE.replace("Air quality", "Air quality (edited)");
    let (status, report) = verify_archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", &tampered),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("bundle.yaml", &index),
    ])
    .await;

    assert_eq!(status, StatusCode::OK, "{report}");
    let verified = report["verified"].as_array().expect("verified");
    let space = verified
        .iter()
        .find(|file| file["path"] == "projects/helsinki/spaces/ovzdusie/space.yaml")
        .expect("the space is listed");
    assert_eq!(space["equal"], json!(false), "{report}");
    assert_eq!(
        verified
            .iter()
            .filter(|file| file["equal"] == json!(true))
            .count(),
        2,
        "the other two arrived whole: {report}"
    );
}

#[tokio::test]
async fn a_file_the_index_lists_and_the_bundle_lost_is_unequal_and_not_silently_absent() {
    let index = bundle_with_checksums(SPACE);
    let (status, report) = verify_archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("bundle.yaml", &index),
    ])
    .await;

    assert_eq!(status, StatusCode::OK, "{report}");
    let verified = report["verified"].as_array().expect("verified");
    let lost = verified
        .iter()
        .find(|file| file["path"] == "projects/helsinki/pipelines/aq/bento.yaml")
        .expect("the missing file is listed");
    assert_eq!(lost["equal"], json!(false), "{report}");
}

#[tokio::test]
async fn a_bundle_from_an_older_exporter_says_nothing_about_verification() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&bundle_archive(), &[("dryRun", "true")]);
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(
        report.get("verified").is_none(),
        "no checksums means nothing verified, never everything equal: {report}"
    );
}

// --- the slug is minted here, never carried in (T-0821, EP-02, EP-75, CC-74) ---------------

/// The source's 26 characters are the source's capability URL. A bundle that kept them would
/// make this instance answer at the address an old link already names, and two imports of one
/// bundle would answer at one address — where the gateway's table keeps only the last.
#[tokio::test]
async fn an_imported_endpoint_is_minted_a_slug_of_this_instance() {
    let server = forge().await;
    upload(&server, vec![], &[]).await;

    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    let slug = endpoint
        .lines()
        .find_map(|line| line.trim().strip_prefix("slug: "))
        .expect("the endpoint carries a slug")
        .trim()
        .to_owned();

    assert_ne!(
        slug, "mluyob4nz52lok3ssk7pgn5vwt",
        "the source's slug travelled"
    );
    assert!(slug.len() >= 26, "a slug is 26 characters or more: {slug}");
    assert!(
        slug.chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
        "base32 without the confusable digits (EP-02): {slug}"
    );
}

/// Updating a project by re-importing its bundle must not move its endpoints under the people
/// using them: the one slug that survives an import is the one this instance minted before.
#[tokio::test]
async fn re_importing_over_an_endpoint_keeps_the_address_this_instance_gave_it() {
    let server = forge().await;
    let existing = ENDPOINT.replace(
        "slug: mluyob4nz52lok3ssk7pgn5vwt",
        "slug: qqqqqqqqqqqqqqqqqqqqqqqqqq",
    );
    let (state, cookie) = state(&server, vec![&existing]);
    let (content_type, body) = multipart(&bundle_archive(), &[("conflictPolicy", "replace")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    assert!(
        endpoint.contains("slug: qqqqqqqqqqqqqqqqqqqqqqqqqq"),
        "the endpoint kept the address it already had: {endpoint}"
    );
}

/// A rename is a second endpoint beside the first, so it cannot answer at the first's address.
#[tokio::test]
async fn a_renamed_endpoint_is_minted_its_own_slug() {
    let server = forge().await;
    let existing = ENDPOINT.replace(
        "slug: mluyob4nz52lok3ssk7pgn5vwt",
        "slug: qqqqqqqqqqqqqqqqqqqqqqqqqq",
    );
    let (state, cookie) = state(&server, vec![&existing]);
    let (content_type, body) = multipart(&bundle_archive(), &[("conflictPolicy", "rename")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    assert!(
        !endpoint.contains("slug: qqqqqqqqqqqqqqqqqqqqqqqqqq")
            && !endpoint.contains("slug: mluyob4nz52lok3ssk7pgn5vwt"),
        "a renamed endpoint answers at neither address: {endpoint}"
    );
}

// --- the import door's check (PF-57, T-1460) --------------------------------------------

fn import_uri() -> String {
    format!("/api/v1/projects/{PROJECT}/import")
}

async fn import_unchecked(
    state: AppState,
    cookie: &str,
    fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (content_type, body) = multipart(&bundle_archive(), fields);
    send(state, cookie, &content_type, body, &import_uri()).await
}

async fn check(state: AppState, cookie: &str, archive: &[u8], fields: &[(&str, &str)]) {
    let (content_type, body) = multipart(archive, fields);
    let (status, report) = send(
        state,
        cookie,
        &content_type,
        body,
        &format!("{}?dryRun=All", import_uri()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
}

fn refusal(status: StatusCode, body: &Value, reason: &str) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "verdict_required", "{body}");
    assert_eq!(body["check"], "jc_project_import", "{body}");
    assert_eq!(body["reason"], reason, "{body}");
}

#[tokio::test]
async fn an_unchecked_bundle_is_refused_and_nothing_reaches_the_forge() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (status, body) = import_unchecked(state, &cookie, &[]).await;

    refusal(status, &body, "verdict_absent");
    assert!(
        forge_calls(&server).await.is_empty(),
        "{:?}",
        forge_calls(&server).await
    );
}

#[tokio::test]
async fn a_bundle_changed_after_its_check_is_stale() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let other = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("bundle.yaml", BUNDLE),
    ]);
    check(state.clone(), &cookie, &other, &[]).await;
    let (status, body) = import_unchecked(state, &cookie, &[]).await;

    refusal(status, &body, "stale");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn another_conflict_policy_than_the_checked_one_is_stale() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![ENDPOINT]);
    check(
        state.clone(),
        &cookie,
        &bundle_archive(),
        &[("conflictPolicy", "skip")],
    )
    .await;
    let (status, body) = import_unchecked(state, &cookie, &[("conflictPolicy", "replace")]).await;

    refusal(status, &body, "stale");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn an_import_uses_its_check_once() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    check(state.clone(), &cookie, &bundle_archive(), &[]).await;
    let (status, body) = import_unchecked(state.clone(), &cookie, &[]).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let (status, body) = import_unchecked(state, &cookie, &[]).await;
    refusal(status, &body, "verdict_absent");
}

#[tokio::test]
async fn a_lax_installation_imports_an_unchecked_bundle() {
    let server = forge().await;
    let branding = std::env::temp_dir().join(format!("jc-import-lax-{}.yaml", std::process::id()));
    std::fs::write(&branding, "validation: lax\n").expect("branding file");
    let mut config = Config::for_tests();
    config.branding_file = Some(branding.to_string_lossy().to_string());
    let (state, cookie) = state_with(config, &server, vec![], &["portal-approver"], vec![]);
    let (status, body) = import_unchecked(state, &cookie, &[]).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn one_persons_check_does_not_open_the_door_for_another() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    check(state.clone(), &cookie, &bundle_archive(), &[]).await;
    let other = format!(
        "{}; {CSRF_COOKIE}={CSRF}",
        session_cookie_of(&state.config, "peter.novak", &["portal-approver"])
    );
    let (status, body) = import_unchecked(state, &other, &[]).await;

    refusal(status, &body, "verdict_absent");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_bundle_whose_dry_run_is_refused_records_no_check() {
    // A conflict under `fail` refuses the dry run; the import then answers the same conflict,
    // never a door opened by a check that did not pass.
    let server = forge().await;
    let (state, cookie) = state(&server, vec![ENDPOINT]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, _) = send(
        state.clone(),
        &cookie,
        &content_type,
        body,
        &format!("{}?dryRun=All", import_uri()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = import_unchecked(state, &cookie, &[]).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_ne!(body["error"], "verdict_required", "{body}");
    assert!(written(&server).await.is_empty());
}

// --- Save as into another project: the endpoint's space (T-1441) --------------------------

/// The target project already holds the space `air`, readable by a steward.
const TARGET_SPACE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: air
  namespace: banskabystrica
spec:
  isSandbox: false
"#;

fn endpoint_only(mapping: Value) -> Vec<u8> {
    let endpoint: Value = serde_norway::from_str(ENDPOINT).expect("the endpoint parses");
    serde_json::to_vec(&json!({ "manifests": [endpoint], "spaceMapping": mapping })).expect("body")
}

/// The endpoint with the Policy that selects its entities, which is where the URNs a mapping has
/// to move actually live (R6).
fn endpoint_and_policy(mapping: Value) -> Vec<u8> {
    let endpoint: Value = serde_norway::from_str(ENDPOINT).expect("the endpoint parses");
    let policy: Value = serde_norway::from_str(POLICY).expect("the policy parses");
    serde_json::to_vec(&json!({ "manifests": [endpoint, policy], "spaceMapping": mapping }))
        .expect("body")
}

#[tokio::test]
async fn an_endpoint_mapped_onto_a_space_of_the_target_keeps_its_name_and_gets_a_fresh_slug() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![TARGET_SPACE]);
    let body = endpoint_and_policy(json!([{ "from": "ovzdusie", "to": "air" }]));
    let (status, answer) = post(state, &cookie, "application/json", body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let written = put_bodies(&server).await;
    let endpoint = written
        .iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint is written");
    let policy = written
        .iter()
        .find(|body| body.contains("kind: Policy"))
        .expect("the policy is written");
    assert!(
        endpoint.contains("name: public-air"),
        "the name is kept: {endpoint}"
    );
    assert!(endpoint.contains("contextSpaceRef: air"), "{endpoint}");
    assert!(
        !endpoint.contains("mluyob4nz52lok3ssk7pgn5vwt"),
        "a fresh slug: {endpoint}"
    );
    assert!(
        policy.contains(
            "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:banskabystrica-air:sever-01"
        ),
        "its ids land on the target space: {policy}"
    );
    assert!(
        policy.contains("tampere.fi:ilma"),
        "another city's id is not ours to move: {policy}"
    );
    assert!(
        !written
            .iter()
            .any(|body| body.contains("kind: ContextSpace")),
        "no space is copied"
    );
}

#[tokio::test]
async fn a_mapping_onto_a_space_the_caller_cannot_read_says_nothing_of_whether_it_exists() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![TARGET_SPACE]);
    let (missing, absent) = post(
        state.clone(),
        &cookie,
        "application/json",
        endpoint_only(json!([{ "from": "ovzdusie", "to": "nowhere" }])),
    )
    .await;
    assert_eq!(missing, StatusCode::BAD_REQUEST, "{absent}");

    // A person who may import but not read spaces meets the same words for a space that exists.
    let (state, cookie) = state_as(
        &server,
        vec![TARGET_SPACE],
        &[],
        vec![
            role(
                "importer",
                json!([{ "kinds": ["Endpoint"], "verbs": ["propose", "read"] }]),
            ),
            binding("importer"),
        ],
    );
    let (hidden, there) = post(
        state,
        &cookie,
        "application/json",
        endpoint_only(json!([{ "from": "ovzdusie", "to": "air" }])),
    )
    .await;
    assert_eq!(hidden, StatusCode::BAD_REQUEST, "{there}");
    let said = |v: &Value| {
        v["detail"]
            .as_str()
            .unwrap_or_default()
            .replace("nowhere", "X")
            .replace("air", "X")
    };
    assert_eq!(said(&absent), said(&there));
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_dangling_space_names_the_three_ways_out() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (status, answer) = post(state, &cookie, "application/json", endpoint_only(json!([]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    for way in ["copy the space", "spaceMapping", "SharedSpaceReference"] {
        assert!(detail.contains(way), "{way}: {detail}");
    }
}

#[tokio::test]
async fn a_name_taken_in_the_target_is_a_conflict_that_never_offers_a_prefixed_name() {
    let server = forge().await;
    let taken = ENDPOINT.replace("namespace: helsinki", "namespace: banskabystrica");
    let (state, cookie) = state(&server, vec![TARGET_SPACE, &taken]);
    let (status, answer) = post(
        state,
        &cookie,
        "application/json",
        endpoint_only(json!([{ "from": "ovzdusie", "to": "air" }])),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("public-air"), "{detail}");
    assert!(
        !detail.contains("banskabystrica-public-air") && !detail.contains("public-air-helsinki"),
        "{detail}"
    );
}

/// The real manifest that went through the door on dev on 2026-09-18 (T-2253): `1.1.0` where
/// DM-22 allows only the served major.
const BAD_PROJECTION: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ModelProjection
metadata:
  name: helsinki-bikes-mobility
  namespace: helsinki
spec:
  contextSpaceRef: ovzdusie
  classes:
    - name: BikeHireDockingStation
      slots: [name, availableBikeNumber]
  dataModelRef:
    kind: DataModel
    name: helsinki
    version: 1.1.0
"#;

/// CC-08, DM-22, T-2253: a bundle's manifests meet their kind's own invariants at the door.
///
/// Before this, `import` asked only who may propose. A ModelProjection naming a semver
/// `dataModelRef` was written into a Change, approved by a person reading an ordinary plan, and
/// then refused by the reconciler on every tick for ever — the resource never existed and the
/// catalogue showed it as restricted, which reads like a permission decision and is not one.
#[tokio::test]
async fn an_imported_manifest_that_fails_its_kind_is_refused_at_the_door() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let archive = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        (
            "projects/helsinki/spaces/ovzdusie/projections/helsinki-bikes-mobility.yaml",
            BAD_PROJECTION,
        ),
    ]);

    // The dry run is the bundle's check (PF-57), and it must say the same thing as the import.
    for uri in [
        format!("/api/v1/projects/{PROJECT}/import?dryRun=All"),
        format!("/api/v1/projects/{PROJECT}/import"),
    ] {
        let (content_type, body) = multipart(&archive, &[("dryRun", "true")]);
        let (status, answer) = send(state.clone(), &cookie, &content_type, body, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {answer}");
        let detail = answer["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("helsinki-bikes-mobility"),
            "the refusal names the file it came from: {detail}"
        );
        assert!(
            detail.contains("dataModelRef"),
            "and the field that is wrong: {detail}"
        );
    }
    assert!(
        written(&server).await.is_empty(),
        "nothing of a bundle that holds a refused manifest is written"
    );
}

/// The neighbour: the same bundle with the version DM-22 allows is imported as before.
#[tokio::test]
async fn a_projection_that_names_the_served_major_is_imported() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let good = BAD_PROJECTION.replace("version: 1.1.0", "version: \"1\"");
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            (
                "projects/helsinki/spaces/ovzdusie/projections/helsinki-bikes-mobility.yaml",
                &good,
            ),
        ]),
        &[("dryRun", "true")],
    );
    let (status, report) = post(state, &cookie, &content_type, body).await;
    // The kind's own validation lets it through; what answers now is the bundle's reference
    // check, which is a different refusal and names the model rather than the field.
    let detail = report["detail"].as_str().unwrap_or_default();
    assert!(
        !detail.contains("not a valid ModelProjection"),
        "the served major is valid: {report}"
    );
    assert_eq!(status, StatusCode::BAD_REQUEST, "{report}");
    assert!(
        detail.contains("DataModel 'helsinki' is in neither the bundle nor project"),
        "{detail}"
    );
}

/// A project's own DataModel resolves: the Endpoint form's check names one and was refused
/// with "in neither the bundle nor project" although the project listed the model (T-6449).
#[tokio::test]
async fn a_projection_of_a_model_the_project_holds_itself_resolves() {
    let server = forge().await;
    let model = r#"apiVersion: joinedcontext.com/v1alpha1
kind: DataModel
metadata:
  name: helsinki
  namespace: helsinki
spec:
  version: "1"
"#;
    let (state, cookie) = state(&server, vec![model]);
    let good = BAD_PROJECTION.replace("version: 1.1.0", "version: \"1\"");
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            (
                "projects/helsinki/spaces/ovzdusie/projections/helsinki-bikes-mobility.yaml",
                &good,
            ),
        ]),
        &[("dryRun", "true")],
    );
    let (_, report) = post(state, &cookie, &content_type, body).await;
    let detail = report["detail"].as_str().unwrap_or_default();
    assert!(!detail.contains("is in neither"), "{report}");
}

/// T-2870 (ADR-N-035): the organization's `spec.limits.data.uploadMegabytes` is the import's
/// limit. A bundle past it is refused before anything is read or written, naming its size, the
/// limit and the setting; the same bundle under a larger limit is imported.
async fn post_padded_space(upload_megabytes: u32) -> (StatusCode, Value, MockServer) {
    let server = forge().await;
    let (state, cookie) = state_as(
        &server,
        vec![],
        &["portal-approver"],
        vec![json!({
            "apiVersion": API_VERSION,
            "kind": "Organization",
            "metadata": { "name": "bb", "namespace": ORG_NAMESPACE },
            "spec": {
                "displayName": { "en": "Banska Bystrica" },
                "domain": "banskabystrica.sk",
                "limits": { "data": { "uploadMegabytes": upload_megabytes } },
            },
        })],
    );
    let manifest: Value = serde_norway::from_str(SPACE).expect("the space parses");
    let mut body = serde_json::to_vec(&json!({ "manifests": { "apiVersion": API_VERSION,
                                      "kind": "List", "items": [manifest] } }))
    .expect("body");
    // One and a half MiB, most of it the whitespace JSON allows after the value.
    body.resize(3 * 512 * 1024, b' ');
    let (status, answer) = post(state, &cookie, "application/json", body).await;
    (status, answer, server)
}

#[tokio::test]
async fn an_import_past_the_organizations_upload_size_is_refused_naming_the_setting() {
    let (status, answer, server) = post_padded_space(1).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    for part in [
        "1.5 MiB",
        "at most 1 MiB",
        "spec.limits.data.uploadMegabytes",
    ] {
        assert!(detail.contains(part), "{part}: {detail}");
    }
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn an_import_inside_the_organizations_upload_size_is_imported() {
    let (status, answer, server) = post_padded_space(2).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    assert_eq!(written(&server).await.len(), 1);
}
