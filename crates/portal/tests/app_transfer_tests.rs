//! One App exported from a project and imported into another from the Administration page
//! (T-2879, UI-87, MF-42, MF-46, MF-17, AP-75, CC-85): the export is the App repository's bundle
//! beside its stripped manifest under a checked index, the import lands the bundle in a new
//! repository and proposes the App building from it; both are an organization administrator's.

mod common;
use common::csrf_echo::EchoCsrf;

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
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
const APP_HEAD: &str = "b47f8079d2df9e7986d2babf4539cec802f4e68e";
const TAGGED: &str = "3a386a2d5ff9fc1e18d3032677ae5bbf2ea5d4e7";
const SOURCE: &str = "/odata/Repositories(Owner='test-owner',Name='ovzdusie_air-map')";
const TARGET: &str = "/odata/Repositories(Owner='test-owner',Name='doprava')";
const LANDED: &str = "/odata/Repositories(Owner='test-owner',Name='doprava_mapa')";

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
    mount_repository(
        &server,
        "/odata/Repositories(Owner='test-owner',Name='ovzdusie')",
    )
    .await;
    at(
        "GET",
        "/odata/Repositories(Owner='test-owner',Name='ovzdusie')/Branches('main')".into(),
        200,
        json!({ "Name": "main", "HeadSha": HEAD }),
    )
    .mount(&server)
    .await;
    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='ovzdusie')/Shako.File(Path=@p,Ref=@r)",
        ))
        .and(query_param("@p", "'apps/air-map/app.yaml'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "blob-app", "Content": STANDARD.encode(APP) })),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    // The App's repository.
    at(
        "GET",
        SOURCE.into(),
        200,
        json!({ "DefaultBranch": "main" }),
    )
    .mount(&server)
    .await;
    at(
        "GET",
        format!("{SOURCE}/Branches('main')"),
        200,
        json!({ "Name": "main", "HeadSha": APP_HEAD }),
    )
    .mount(&server)
    .await;
    // `Shako.BundleLink` names the bundle's download, which the client fetches from the base it
    // dials.
    let download = "/api/v1/repos/test-owner/ovzdusie_air-map/archive/main.bundle";
    Mock::given(method("GET"))
        .and(path(format!("{SOURCE}/Shako.BundleLink(Ref=@r)")))
        .and(query_param("@r", "'main'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "value": format!("https://forge.example{download}") })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(download))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bundle(bundled)))
        .mount(&server)
        .await;
    at(
        "GET",
        format!("{SOURCE}/Tags"),
        200,
        json!({ "value": [{ "Name": "v1.0.0", "TargetSha": TAGGED }] }),
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
        "/odata/CreateRepository".into(),
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
        format!("{LANDED}/Branches('main')"),
        200,
        json!({ "Name": "main", "HeadSha": landed }),
    )
    .mount(&server)
    .await;
    for (verb, route) in [
        ("POST", format!("{LANDED}/Shako.Protect")),
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
    import_into(state, who, "doprava", query, file, fields).await
}

/// [`import`] into `project`.
async fn import_into(
    state: &AppState,
    who: Identity,
    project: &str,
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
                .uri(format!("/api/v1/projects/{project}/import{query}"))
                .header(
                    axum::http::header::COOKIE,
                    common::cookie(&state.config, who),
                )
                .echo_csrf()
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
    assert!(sent(server, "POST", "/odata/CreateRepository")
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
        ("jana", "/api/v1/projects/ovzdusie/apps/Not_A_Name/export"),
        ("jana", "/api/v1/projects/Not_A_Project/apps/air-map/export"),
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

/// UI-87: an App the project's repository does not hold, or holds as no manifest, is not
/// exported, and the answer names which.
#[tokio::test]
async fn an_app_whose_manifest_is_missing_or_garbled_is_not_exported() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    for name in ["bikes", "garbled"] {
        state.mirror.upsert(envelope(
            "App",
            name,
            "ovzdusie",
            json!({ "source": { "git": { "url": "https://forge.example/test-owner/ovzdusie_air-map.git", "ref": "main" } } }),
        ));
    }
    Mock::given(method("GET"))
        .and(path(
            "/odata/Repositories(Owner='test-owner',Name='ovzdusie')/Shako.File(Path=@p,Ref=@r)",
        ))
        .and(query_param("@p", "'apps/garbled/app.yaml'"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "Sha": "blob-garbled", "Content": STANDARD.encode("kind: [") }),
            ),
        )
        .with_priority(1)
        .mount(&server)
        .await;

    let missing = export(
        &state,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/bikes/export",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
    assert!(
        missing.text.contains("is not in the project's repository"),
        "{}",
        missing.text
    );

    let garbled = export(
        &state,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/garbled/export",
    )
    .await;
    assert_eq!(garbled.status, StatusCode::CONFLICT, "{}", garbled.text);
    assert!(
        garbled
            .text
            .contains("apps/garbled/app.yaml is not a manifest"),
        "{}",
        garbled.text
    );
}

/// MF-42: an export whose index would not check is not handed out: a session naming no
/// username writes no `exportedBy`, and the answer is a server error, not an archive.
#[tokio::test]
async fn an_export_with_no_exporter_is_not_handed_out() {
    let (_server, state) = world(APP_HEAD, APP_HEAD).await;
    let mut nameless = person("jana");
    nameless.username = " ".into();
    let answer = export(
        &state,
        nameless,
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        answer.text
    );
    assert!(
        !answer.text.starts_with("PK"),
        "no archive: {}",
        answer.text
    );
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

    let created: Vec<Value> = sent(&server, "POST", "/odata/CreateRepository")
        .await
        .iter()
        .map(|body| serde_json::from_slice(body).expect("json"))
        .collect();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["Name"], "doprava_mapa");
    assert_eq!(created[0]["Private"], true);
    assert_eq!(created[0]["AutoInit"], false);

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
    common::assert_portal_branch_rules(&server, LANDED).await;

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

/// UI-87, MF-46: an App repository with no tag exports without a tags file, and its import
/// pushes `main` and nothing else.
#[tokio::test]
async fn an_app_without_tags_exports_and_imports_without_them() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    at("GET", format!("{SOURCE}/Tags"), 200, json!({ "value": [] }))
        .with_priority(1)
        .mount(&server)
        .await;
    let file = exported(&state).await;
    assert_eq!(
        unzip(&file).keys().map(String::as_str).collect::<Vec<_>>(),
        ["air-map.bundle", "app.yaml", "bundle.yaml"]
    );

    let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
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
    assert!(!push.contains("refs/tags/"), "{push}");
}

/// PF-57: the dry run answers the repository it would create and creates nothing.
/// T-7837: an import into an organization of layout 1, a Portal without a forge, and a name
/// the applications organization cannot hold are each refused, and nothing is created.
#[tokio::test]
async fn an_import_with_no_place_to_land_is_refused_and_creates_nothing() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;

    let mut forgeless = state.clone();
    forgeless.shako = None;
    let answer = export(
        &forgeless,
        person("jana"),
        "/api/v1/projects/ovzdusie/apps/air-map/export",
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );
    let answer = import(&forgeless, person("jana"), "?format=app", &file, &[]).await;
    assert_eq!(
        answer.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        answer.text
    );

    let long = "x".repeat(70);
    let answer = import(
        &state,
        person("jana"),
        "?format=app",
        &file,
        &[("name", &long)],
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);

    state.mirror.set_layout(1);
    let answer = import(&state, person("jana"), "?format=app", &file, &[]).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("layout 1"), "{}", answer.text);
    nothing_created(&server).await;
}

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

/// T-7458: an App's dry run into a project with no Organization in the registry answers 409
/// naming the project and creates nothing.
#[tokio::test]
async fn the_dry_run_into_a_project_with_no_organization_answers_409_naming_it() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    let state = common::unplaced(state, "doprava").await;
    let answer = import(
        &state,
        person("jana"),
        "?format=app&dryRun=All",
        &file,
        &[("name", "mapa")],
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("doprava"), "{}", answer.text);
    assert!(
        answer.text.contains("no active Organization"),
        "{}",
        answer.text
    );
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
    let mut no_index = files.clone();
    no_index.remove("bundle.yaml");
    let mut garbled = files.clone();
    garbled.insert("bundle.yaml".into(), b"spec: [".to_vec());
    let index_edit = |edit: &dyn Fn(&mut Value)| {
        let mut edited = files.clone();
        let mut index: Value =
            serde_norway::from_slice(&edited["bundle.yaml"]).expect("bundle.yaml reads");
        edit(&mut index);
        let text = serde_norway::to_string(&index).expect("bundle.yaml writes");
        edited.insert("bundle.yaml".into(), text.into_bytes());
        edited
    };
    let unsummed = index_edit(&|index| {
        index["spec"]["files"]
            .as_array_mut()
            .expect("files")
            .retain(|file| file["path"] != "app.yaml");
    });
    let two_items = index_edit(&|index| {
        index["spec"]["items"][0]["kind"] = json!("Dashboard");
    });
    let unsigned = index_edit(&|index| {
        index["spec"]["exportedBy"] = json!("");
    });
    let mut unknown_field = files.clone();
    let mut parsed: Value = serde_norway::from_slice(&files["app.yaml"]).expect("app.yaml reads");
    parsed["spec"]["notAField"] = json!(1);
    let text = serde_norway::to_string(&parsed).expect("app.yaml writes");
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    unknown_field.insert("app.yaml".into(), text.into_bytes());
    let index = String::from_utf8_lossy(&unknown_field["bundle.yaml"]).replace(&old, &digest);
    unknown_field.insert("bundle.yaml".into(), index.into_bytes());
    let mut broken_app = files.clone();
    let digest = format!("{:x}", Sha256::digest(b"kind: [".as_slice()));
    broken_app.insert("app.yaml".into(), b"kind: [".to_vec());
    let index = String::from_utf8_lossy(&broken_app["bundle.yaml"]).replace(&old, &digest);
    broken_app.insert("bundle.yaml".into(), index.into_bytes());

    for (file, fields, status, needle) in [
        (
            zip_of(&no_index),
            vec![],
            StatusCode::BAD_REQUEST,
            "no bundle.yaml",
        ),
        (
            zip_of(&garbled),
            vec![],
            StatusCode::BAD_REQUEST,
            "is not a Bundle",
        ),
        (
            zip_of(&broken_app),
            vec![],
            StatusCode::BAD_REQUEST,
            "app.yaml is not a manifest",
        ),
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
            zip_of(&unsummed),
            vec![],
            StatusCode::BAD_REQUEST,
            "no checksum of app.yaml",
        ),
        (
            zip_of(&two_items),
            vec![],
            StatusCode::BAD_REQUEST,
            "one App as its item",
        ),
        (
            zip_of(&unsigned),
            vec![],
            StatusCode::BAD_REQUEST,
            "exportedBy must not be empty",
        ),
        (
            zip_of(&unknown_field),
            vec![],
            StatusCode::BAD_REQUEST,
            "app.yaml is not a valid App",
        ),
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
        "/odata/Repositories(Owner='test-owner',Name='doprava_air-map')".into(),
        200,
        json!({ "DefaultBranch": "main" }),
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
        "/odata/Repositories(Owner='test-owner',Name='doprava_air-map')"
    )
    .await
    .is_empty());
}

/// AP-75: beside a project whose name is long, an App name whose repository name would pass
/// the forge's limit is refused before anything is created.
#[tokio::test]
async fn a_repository_name_past_the_forges_limit_is_refused() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    let long = "a".repeat(60);
    state.mirror.upsert(envelope(
        "Project",
        &long,
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb", "repository": { "name": long }, "ref": "main" }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-reads-long",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "reader", "scope": { "project": long } }),
    ));
    let name = "x".repeat(40);
    let answer = import_into(
        &state,
        person("jana"),
        &long,
        "?format=app",
        &file,
        &[("name", &name)],
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(answer.text.contains("AP-75"), "{}", answer.text);
    nothing_created(&server).await;
}

/// PF-86: a repository of the name that appears in the forge between the check and the
/// creation is somebody else's: the import stops and removes nothing.
#[tokio::test]
async fn a_repository_that_appears_during_the_import_is_not_adopted() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let file = exported(&state).await;
    at(
        "POST",
        "/odata/CreateRepository".into(),
        409,
        json!({ "message": "already exists" }),
    )
    .with_priority(1)
    .mount(&server)
    .await;
    let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("appeared in the forge during the import"),
        "{}",
        answer.text
    );
    assert!(sent(&server, "DELETE", LANDED).await.is_empty());
}

/// One pkt-line of a `git-receive-pack` answer.
fn pkt(text: &str) -> String {
    format!("{:04x}{text}", text.len() + 4)
}

/// MF-46, CC-85: a forge that refuses the branch stops the import and the new repository goes
/// again; a tag it refuses is reported in the Change, and the import goes on.
#[tokio::test]
async fn a_refused_branch_stops_the_import_and_a_refused_tag_is_reported() {
    for (refused, expected) in [
        ("ng refs/heads/main denied\n", StatusCode::CONFLICT),
        ("ng refs/tags/v1.0.0 missing\n", StatusCode::ACCEPTED),
    ] {
        let (server, state) = world(APP_HEAD, APP_HEAD).await;
        let file = exported(&state).await;
        let answer = format!("{}{}0000", pkt("unpack ok\n"), pkt(refused));
        Mock::given(method("POST"))
            .and(path("/test-owner/doprava_mapa.git/git-receive-pack"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(answer.into_bytes()))
            .with_priority(1)
            .mount(&server)
            .await;
        let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
        assert_eq!(answer.status, expected, "{refused}: {}", answer.text);
        let pulls = sent(&server, "POST", &format!("{TARGET}/Shako.OpenPullRequest")).await;
        if expected == StatusCode::CONFLICT {
            assert!(
                answer
                    .text
                    .contains("the forge refused refs/heads/main denied of 'doprava_mapa'"),
                "{}",
                answer.text
            );
            assert_eq!(sent(&server, "DELETE", LANDED).await.len(), 1);
            assert!(pulls.is_empty());
        } else {
            assert!(sent(&server, "DELETE", LANDED).await.is_empty());
            assert_eq!(pulls.len(), 1);
            let body = String::from_utf8_lossy(&pulls[0]);
            assert!(
                body.contains("doprava_mapa: refs/tags/v1.0.0 missing"),
                "{body}"
            );
        }
    }
}

/// AP-02, CC-85: an App that builds from no repository cannot build from the new one: the
/// import stops after the push and the new repository goes again.
#[tokio::test]
async fn an_app_that_names_no_source_git_removes_the_new_repository() {
    let (server, state) = world(APP_HEAD, APP_HEAD).await;
    let mut files = unzip(&exported(&state).await);
    let old = format!("{:x}", Sha256::digest(&files["app.yaml"]));
    let mut app: Value = serde_norway::from_slice(&files["app.yaml"]).expect("app.yaml reads");
    app["spec"]["source"] = json!({ "path": "apps/air-map" });
    let text = serde_norway::to_string(&app).expect("app.yaml writes");
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    files.insert("app.yaml".into(), text.into_bytes());
    let index = String::from_utf8_lossy(&files["bundle.yaml"]).replace(&old, &digest);
    files.insert("bundle.yaml".into(), index.into_bytes());

    let answer = checked_import(&state, &zip_of(&files), &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer.text.contains("app.yaml names no source.git"),
        "{}",
        answer.text
    );
    assert_eq!(sent(&server, "DELETE", LANDED).await.len(), 1);
}

/// CC-85: a repository the forge will not remove after a failed import is left behind, and the
/// import still answers why it failed.
#[tokio::test]
async fn a_failed_import_whose_repository_stays_still_answers_why() {
    let (server, state) = world(APP_HEAD, TAGGED).await;
    let file = exported(&state).await;
    at("DELETE", LANDED.to_owned(), 500, json!({ "message": "no" }))
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = checked_import(&state, &file, &[("name", "mapa")]).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("MF-46"), "{}", answer.text);
    assert!(!sent(&server, "DELETE", LANDED).await.is_empty());
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
    assert!(
        sent(&server, "POST", &format!("{TARGET}/Shako.OpenPullRequest"))
            .await
            .is_empty()
    );
}
