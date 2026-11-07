//! One App exported from a project and imported into another from the Administration page
//! (T-2879, UI-87, MF-42, MF-46, MF-17, AP-75, CC-85): the export is the App repository's bundle
//! beside its stripped manifest under a checked index, the import lands the bundle in a new
//! repository and proposes the App building from it; both are an organization administrator's.

mod common;

use std::collections::BTreeMap;
use std::io::{Read, Write};

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, forge, mount_repository, person, state_on};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
const APP_HEAD: &str = "b47f8079d2df9e7986d2babf4539cec802f4e68e";
const TAGGED: &str = "3a386a2d5ff9fc1e18d3032677ae5bbf2ea5d4e7";
const SOURCE: &str = "/api/v1/repos/test-owner/ovzdusie_air-map";
const TARGET: &str = "/api/v1/repos/test-owner/doprava";
const LANDED: &str = "/api/v1/repos/test-owner/doprava_mapa";

/// The App as the project's repository holds it: a status and a built digest, which no export
/// carries (MF-17, AP-13a).
const APP: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: App\nmetadata:\n  \
                   name: air-map\n  namespace: ovzdusie\n  annotations:\n    \
                   joinedcontext.com/image: registry.example/air-map@sha256:abc\n    \
                   owner: jana\nspec:\n  kind: static\n  source:\n    git:\n      url: \
                   https://forge.example/test-owner/ovzdusie_air-map.git\n      ref: main\n  \
                   build: {}\n  dataNeeds:\n  - contextSpaceRef: ovzdusie\n    types: [AirQualityObserved]\n    operations: [queryEntity]\nstatus:\n  phase: Live\n";

fn bundle(head: &str) -> Vec<u8> {
    let mut bytes =
        format!("# v2 git bundle\n{head} refs/heads/bundle\n{head} HEAD\n\n").into_bytes();
    bytes.extend_from_slice(b"PACK\0\0\0\x02\0\0\0\0");
    bytes
}

fn at(verb: &str, route: String, status: u16, body: Value) -> Mock {
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
}

/// A layout 2 organization: `ovzdusie` holds the App `air-map` building from
/// `ovzdusie_air-map` (tagged, bundled at `bundled`); `doprava` is a second project that takes
/// every write; `doprava_mapa` is not in the forge until an import creates it, then ends at
/// `landed`. Jana administers the organization, Peter reads both projects and nothing more.
async fn world(bundled: &str, landed: &str) -> (MockServer, AppState) {
    let server = forge().await;
    // The source project's repository, its App manifest at HEAD.
    mount_repository(&server, "/api/v1/repos/test-owner/ovzdusie").await;
    at(
        "GET",
        "/api/v1/repos/test-owner/ovzdusie/branches/main".into(),
        200,
        json!({ "name": "main", "commit": { "id": HEAD } }),
    )
    .mount(&server)
    .await;
    at(
        "GET",
        "/api/v1/repos/test-owner/ovzdusie/contents/apps/air-map/app.yaml".into(),
        200,
        json!({ "sha": "blob-app", "content": STANDARD.encode(APP) }),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    // The App's repository.
    at(
        "GET",
        SOURCE.into(),
        200,
        json!({ "default_branch": "main" }),
    )
    .mount(&server)
    .await;
    at(
        "GET",
        format!("{SOURCE}/branches/main"),
        200,
        json!({ "name": "main", "commit": { "id": APP_HEAD } }),
    )
    .mount(&server)
    .await;
    Mock::given(method("GET"))
        .and(path(format!("{SOURCE}/archive/main.bundle")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bundle(bundled)))
        .mount(&server)
        .await;
    at(
        "GET",
        format!("{SOURCE}/tags"),
        200,
        json!([{ "name": "v1.0.0", "commit": { "sha": TAGGED } }]),
    )
    .mount(&server)
    .await;
    // The target project's repository, and the App repository an import creates.
    mount_repository(&server, TARGET).await;
    at("GET", LANDED.into(), 404, json!({ "message": "not found" }))
        .mount(&server)
        .await;
    at(
        "POST",
        "/api/v1/orgs/test-owner/repos".into(),
        201,
        json!({ "name": "created" }),
    )
    .mount(&server)
    .await;
    Mock::given(method("POST"))
        .and(path("/test-owner/doprava_mapa.git/git-receive-pack"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"000eunpack ok\n0017ok refs/heads/main\n0000".to_vec()),
        )
        .mount(&server)
        .await;
    at(
        "GET",
        format!("{LANDED}/branches/main"),
        200,
        json!({ "name": "main", "commit": { "id": landed } }),
    )
    .mount(&server)
    .await;
    for (verb, route) in [
        ("POST", format!("{LANDED}/branch_protections")),
        ("DELETE", LANDED.to_owned()),
    ] {
        at(verb, route, 201, json!({})).mount(&server).await;
    }

    let state = state_on(&server);
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([
        ("ovzdusie".to_owned(), "ovzdusie".to_owned()),
        ("doprava".to_owned(), "doprava".to_owned()),
    ]));
    for project in ["ovzdusie", "doprava"] {
        state.mirror.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": "bb", "repository": { "name": project }, "ref": "main" }),
        ));
    }
    state.mirror.upsert(envelope(
        "App",
        "air-map",
        "ovzdusie",
        json!({ "source": { "git": { "url": "https://forge.example/test-owner/ovzdusie_air-map.git", "ref": "main" } } }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "App", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "org-admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "org-admin", "scope": { "organization": "bb" } }),
    ));
    for project in ["ovzdusie", "doprava"] {
        for who in ["jana", "peter"] {
            state.mirror.upsert(envelope(
                "RoleBinding",
                &format!("{who}-reads-{project}"),
                ORG_NAMESPACE,
                json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": "reader", "scope": { "project": project } }),
            ));
        }
    }
    (server, state)
}

struct Answer {
    status: StatusCode,
    text: String,
    bytes: Vec<u8>,
}

async fn answer(response: axum::response::Response) -> Answer {
    use http_body_util::BodyExt;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    Answer {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
        bytes,
    }
}

async fn export(state: &AppState, who: Identity, route: &str) -> Answer {
    use tower::ServiceExt;
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri(route)
                .header(
                    axum::http::header::COOKIE,
                    common::cookie(&state.config, who),
                )
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    answer(response).await
}

async fn import(
    state: &AppState,
    who: Identity,
    query: &str,
    file: &[u8],
    fields: &[(&str, &str)],
) -> Answer {
    use tower::ServiceExt;
    let boundary = "jcappimport";
    let mut body = Vec::new();
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
             filename=\"air-map-app.zip\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/doprava/import{query}"))
                .header(
                    axum::http::header::COOKIE,
                    common::cookie(&state.config, who),
                )
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .header(
                    axum::http::header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(axum::body::Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    answer(response).await
}

/// The import as the Administration page runs it: the dry run PF-57 holds it to, then the same
/// archive and fields for real.
async fn checked_import(state: &AppState, file: &[u8], fields: &[(&str, &str)]) -> Answer {
    let check = import(
        state,
        person("jana"),
        "?format=app&dryRun=All",
        file,
        fields,
    )
    .await;
    assert_eq!(check.status, StatusCode::OK, "{}", check.text);
    import(state, person("jana"), "?format=app", file, fields).await
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip");
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).expect("an entry");
            let mut content = Vec::new();
            file.read_to_end(&mut content).expect("readable");
            (file.name().expect("a text name").into_owned(), content)
        })
        .collect()
}

fn zip_of(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, bytes) in files {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(bytes).expect("write");
    }
    writer.finish().expect("zip").into_inner()
}

/// The archive Jana's export of `air-map` answers.
async fn exported(state: &AppState) -> Vec<u8> {
    let answer = export(
        state,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    answer.bytes
}

/// Every request of `verb` to `route`, its body as bytes.
async fn sent(server: &MockServer, verb: &str, route: &str) -> Vec<Vec<u8>> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == route)
        .map(|r| r.body)
        .collect()
}

async fn nothing_created(server: &MockServer) {
    assert!(sent(server, "POST", "/api/v1/orgs/test-owner/repos")
        .await
        .is_empty());
}

/// UI-87, MF-42, MF-17: the export is the App repository's bundle as the forge made it, its
/// tags, the manifest without status, built digest or credential, and an index whose one
/// repository ends at the head read and whose checksums hold.
#[tokio::test]
async fn an_app_exports_as_its_bundle_beside_its_stripped_manifest() {
    let (_server, state) = world(APP_HEAD, APP_HEAD).await;
    let files = unzip(&exported(&state).await);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        ["air-map.bundle", "air-map.tags", "app.yaml", "bundle.yaml"]
    );
    assert_eq!(files["air-map.bundle"], bundle(APP_HEAD));
    assert_eq!(
        String::from_utf8_lossy(&files["air-map.tags"]),
        format!("{TAGGED} refs/tags/v1.0.0\n")
    );
    let app: Value = serde_norway::from_slice(&files["app.yaml"]).expect("yaml");
    assert!(app.get("status").is_none(), "{app}");
    assert!(
        app["metadata"]["annotations"]
            .get("joinedcontext.com/image")
            .is_none(),
        "{app}"
    );
    assert_eq!(app["metadata"]["annotations"]["owner"], "jana");

    let index: jc_core::kinds::Bundle =
        serde_norway::from_slice(&files["bundle.yaml"]).expect("the platform's Bundle");
    index.validate().expect("a valid index");
    assert_eq!(index.spec.repositories.len(), 1);
    assert_eq!(index.spec.repositories[0].head, APP_HEAD);
    assert_eq!(index.spec.items.len(), 1);
    for file in &index.spec.files {
        assert_eq!(
            file.sha256,
            format!("{:x}", Sha256::digest(&files[&file.path])),
            "{}",
            file.path
        );
    }
    assert_eq!(index.spec.files.len(), files.len() - 1);
}

/// UI-87: who reads the project and does not administer the organization is refused; who does
/// not read it learns nothing of the App; an App that is not there is 404.
#[tokio::test]
async fn only_an_administrator_exports_an_app() {
    let (_server, state) = world(APP_HEAD, APP_HEAD).await;
    let refused = export(
        &state,
        person("peter"),
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused.text.contains("organization administrators"),
        "{}",
        refused.text
    );
    for (who, route) in [
        ("eva", "/api/v1/projects/ovzdusie/apps/air-map/export"),
        ("jana", "/api/v1/projects/ovzdusie/apps/nothing/export"),
    ] {
        let hidden = export(&state, person(who), route).await;
        assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{who} {route}");
    }
}

/// MF-45, MF-46: an App that builds from outside the forge's applications organization, or
/// whose repository moved during the export, is not exported.
#[tokio::test]
async fn an_app_that_cannot_travel_whole_is_not_exported() {
    let (_server, state) = world(TAGGED, APP_HEAD).await;
    let moved = export(
        &state,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(moved.status, StatusCode::CONFLICT, "{}", moved.text);
    assert!(moved.text.contains("export again"), "{}", moved.text);

    state.mirror.upsert(envelope(
        "App",
        "air-map",
        "ovzdusie",
        json!({ "source": { "git": { "url": "https://github.com/someone/air-map.git", "ref": "main" } } }),
    ));
    let foreign = export(
        &state,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(foreign.status, StatusCode::CONFLICT, "{}", foreign.text);
    assert!(foreign.text.contains("github.com"), "{}", foreign.text);
}

/// UI-87, MF-46, AP-75: the export imports into another project under a new name. The new
/// repository is created empty and private, the bundle pushed as `main` with its tags, `main`
/// protected, and the project's red-lane Change adds the App building from it.
#[tokio::test]
async fn an_exported_app_imports_into_another_project_under_a_new_name() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(change["status"]["lane"], "red", "{change}");

    let created: Vec<Value> = sent(&server, "POST", "/api/v1/orgs/test-owner/repos")
        .await
        .iter()
        .map(|body| serde_json::from_slice(body).expect("json"))
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "doprava_mapa");
    assert_eq!(created[0]["private"], true);
    assert_eq!(created[0]["auto_init"], false);

    let pushed = sent(
        &server,
        "POST",
        "/test-owner/doprava_mapa.git/git-receive-pack",
    )
    .await;
    assert_eq!(pushed.len(), 1);
    let push = String::from_utf8_lossy(&pushed[0]);
    assert!(
        push.contains(&format!("{APP_HEAD} refs/heads/main")),
        "{push}"
    );
    assert!(
        push.contains(&format!("{TAGGED} refs/tags/v1.0.0")),
        "{push}"
    );
    assert_eq!(
        sent(&server, "POST", &format!("{LANDED}/branch_protections"))
            .await
            .len(),
        1
    );

    let written = common::committed_files(&server, TARGET).await;
    let (route, text) = written
        .iter()
        .find(|(route, _)| route.ends_with("/app.yaml") || route.ends_with("mapa.yaml"))
        .unwrap_or_else(|| panic!("the App manifest is proposed: {written:?}"));
    assert!(route.contains("mapa"), "{route}");
    let app: Value = serde_norway::from_str(text).expect("yaml");
    assert_eq!(app["metadata"]["name"], "mapa");
    assert_eq!(app["metadata"]["namespace"], "doprava");
    assert!(
        app["spec"]["source"]["git"]["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with("/test-owner/doprava_mapa.git"),
        "{app}"
    );
    assert!(sent(&server, "DELETE", LANDED).await.is_empty());
}

/// PF-57: the dry run answers the repository it would create and creates nothing.
#[tokio::test]
async fn the_dry_run_answers_the_repository_and_creates_nothing() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    let answer = import(
        &state,
        person("jana"),
        "?format=app&dryRun=All",
        &file,
        &[("name", "mapa")],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let plan: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(plan["repositories"][0]["repository"], "doprava_mapa");
    assert_eq!(plan["repositories"][0]["head"], APP_HEAD);
    nothing_created(&server).await;
}

/// UI-87: who does not administer the organization is refused before the upload is read, and
/// who does not read the project is told nothing about it.
#[tokio::test]
async fn only_an_administrator_imports_an_app() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let refused = import(
        &state,
        person("peter"),
        "?format=app",
        b"not a zip",
        &[("stray", "x")],
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    let hidden = import(&state, person("eva"), "?format=app", b"not a zip", &[]).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.text);
    nothing_created(&server).await;
}

/// MF-42, MF-45, MF-47: a tampered file, a project's archive, an App of another apiVersion, a
/// bad name and project parameters are refused before anything is created.
#[tokio::test]
async fn an_archive_that_does_not_check_creates_nothing() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let files = unzip(&exported(&state).await);
    let mut tampered = files.clone();
    tampered
        .get_mut("air-map.bundle")
        .expect("bundle")
        .extend_from_slice(b"more");
    let mut project = files.clone();
    let index = String::from_utf8_lossy(&project["bundle.yaml"]).replace("application", "project");
    project.insert("bundle.yaml".into(), index.into_bytes());
    let mut newer = files.clone();
    let app = String::from_utf8_lossy(&newer["app.yaml"]).replace("v1alpha1", "v1beta1");
    let digest = format!("{:x}", Sha256::digest(app.as_bytes()));
    let old = format!("{:x}", Sha256::digest(&files["app.yaml"]));
    newer.insert("app.yaml".into(), app.into_bytes());
    let index = String::from_utf8_lossy(&newer["bundle.yaml"]).replace(&old, &digest);
    newer.insert("bundle.yaml".into(), index.into_bytes());

    for (file, fields, status, needle) in [
        (
            zip_of(&tampered),
            vec![],
            StatusCode::BAD_REQUEST,
            "SHA-256",
        ),
        (
            zip_of(&project),
            vec![],
            StatusCode::BAD_REQUEST,
            "format=git",
        ),
        (zip_of(&newer), vec![], StatusCode::BAD_REQUEST, "v1beta1"),
        (
            zip_of(&files),
            vec![("name", "Not_A_Name")],
            StatusCode::BAD_REQUEST,
            "Not_A_Name",
        ),
        (
            zip_of(&files),
            vec![("parameters", r#"{"audience":"public"}"#)],
            StatusCode::BAD_REQUEST,
            "parameters",
        ),
    ] {
        let answer = import(&state, person("jana"), "?format=app", &file, &fields).await;
        assert_eq!(answer.status, status, "{needle}: {}", answer.text);
        assert!(answer.text.contains(needle), "{needle}: {}", answer.text);
    }
    nothing_created(&server).await;
}

/// PF-86: an App of that name in the project, or a repository of that name in the forge, is a
/// conflict before anything is created, and nobody's repository is removed.
#[tokio::test]
async fn a_name_that_is_taken_is_not_adopted() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    state.mirror.upsert(envelope(
        "App",
        "mapa",
        "doprava",
        json!({ "source": { "git": { "url": "https://forge.example/test-owner/doprava_mapa.git" } } }),
    ));
    let taken = import(
        &state,
        person("jana"),
        "?format=app",
        &file,
        &[("name", "mapa")],
    )
    .await;
    assert_eq!(taken.status, StatusCode::CONFLICT, "{}", taken.text);
    assert!(taken.text.contains("another name"), "{}", taken.text);

    at(
        "GET",
        "/api/v1/repos/test-owner/doprava_air-map".into(),
        200,
        json!({ "default_branch": "main" }),
    )
    .mount(&server)
    .await;
    let there = import(&state, person("jana"), "?format=app", &file, &[]).await;
    assert_eq!(there.status, StatusCode::CONFLICT, "{}", there.text);
    assert!(there.text.contains("doprava_air-map"), "{}", there.text);
    nothing_created(&server).await;
    assert!(sent(
        &server,
        "DELETE",
        "/api/v1/repos/test-owner/doprava_air-map"
    )
    .await
    .is_empty());
}

/// MF-46, CC-85: a head that is not the index's after the push removes the repository the
/// import created, and nothing is proposed.
#[tokio::test]
async fn a_head_that_is_not_the_bundles_removes_the_new_repository() {
    let (server, state) = world(APP_HEAD, TAGGED).await;
    let file = exported(&state).await;
    let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("MF-46"), "{}", answer.text);
    assert_eq!(sent(&server, "DELETE", LANDED).await.len(), 1);
    assert!(sent(&server, "POST", &format!("{TARGET}/pulls"))
        .await
        .is_empty());
}
