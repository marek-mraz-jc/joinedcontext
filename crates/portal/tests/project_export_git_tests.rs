//! Exporting a project of layout 2 (T-2644, MF-45, MF-42, MF-18, CC-87, CC-88): its manifests
//! are read from its own repository, and `format=git` is one bundle per repository with the
//! head each ends at, the tags beside it, and the registry entry with no parameter values.

mod common;

use std::io::Read;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, forge, person, state_on};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
const APP_HEAD: &str = "b47f8079d2df9e7986d2babf4539cec802f4e68e";
const TAGGED: &str = "3a386a2d5ff9fc1e18d3032677ae5bbf2ea5d4e7";

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                     name: ovzdusie\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n";
const OTHER_SPACE: &str =
    "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  \
                           name: hluk\n  namespace: ovzdusie\nspec:\n  isSandbox: false\n";

fn bundle(head: &str) -> Vec<u8> {
    let mut bytes =
        format!("# v2 git bundle\n{head} refs/heads/bundle\n{head} HEAD\n\n").into_bytes();
    bytes.extend_from_slice(b"PACK\0\0\0\x02\0\0\0\0");
    bytes
}

/// One repository of the forge: its default branch at `head`, its tree and files at that
/// commit, the forge's bundle of `main` ending at `bundled`, and its tags.
async fn repository(
    server: &MockServer,
    name: &str,
    head: &str,
    bundled: &str,
    files: &[(&str, &str)],
    tags: Value,
) {
    let base = format!("/api/v1/repos/test-owner/{name}");
    let json_at = |route: String, body: Value| {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
    };
    json_at(base.clone(), json!({ "default_branch": "main" }))
        .mount(server)
        .await;
    json_at(
        format!("{base}/branches/main"),
        json!({ "name": "main", "commit": { "id": head } }),
    )
    .mount(server)
    .await;
    let tree: Vec<_> = files
        .iter()
        .map(|(file, _)| json!({ "path": file, "type": "blob", "sha": format!("blob-{file}") }))
        .collect();
    json_at(
        format!("{base}/git/trees/{head}"),
        json!({ "sha": "tree", "truncated": false, "tree": tree }),
    )
    .mount(server)
    .await;
    for (file, body) in files {
        json_at(
            format!("{base}/contents/{file}"),
            json!({ "sha": format!("blob-{file}"), "content": STANDARD.encode(body) }),
        )
        .mount(server)
        .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{base}/archive/main.bundle")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bundle(bundled)))
        .mount(server)
        .await;
    json_at(format!("{base}/tags"), tags).mount(server).await;
}

/// A layout 2 organization: `ovzdusie` in its own repository with two spaces and a tag, and
/// its App `air-map` building from the repository `ovzdusie_air-map`.
async fn world(bundled: &str) -> (MockServer, AppState) {
    let server = forge().await;
    repository(
        &server,
        "ovzdusie",
        HEAD,
        bundled,
        &[
            ("project.yaml", "kind: Project\n"),
            ("spaces/ovzdusie/space.yaml", SPACE),
            ("spaces/hluk/space.yaml", OTHER_SPACE),
        ],
        json!([{ "name": "v1.0.0", "commit": { "sha": TAGGED } }]),
    )
    .await;
    repository(
        &server,
        "ovzdusie_air-map",
        APP_HEAD,
        APP_HEAD,
        &[],
        json!([]),
    )
    .await;

    let state = state_on(&server);
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "ovzdusie".to_owned(),
        "ovzdusie".to_owned(),
    )]));
    state.mirror.upsert(envelope(
        "Project",
        "ovzdusie",
        ORG_NAMESPACE,
        json!({
            "organizationRef": "bb",
            "repository": { "name": "ovzdusie" },
            "ref": "main",
            "parameters": { "audience": "organization" }
        }),
    ));
    state.mirror.upsert(envelope(
        "App",
        "air-map",
        "ovzdusie",
        json!({ "source": { "git": { "url": "https://forge.example/test-owner/ovzdusie_air-map.git", "ref": "main" } } }),
    ));
    (server, state)
}

fn steward() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

struct Answer {
    status: StatusCode,
    text: String,
    bytes: Vec<u8>,
}

/// A download as `who`: the archive's bytes as they came.
async fn export(state: &AppState, who: Identity, query: &str) -> Answer {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/v1/projects/ovzdusie/export{query}"))
                .header(
                    axum::http::header::COOKIE,
                    common::cookie(&state.config, who),
                )
                .body(axum::body::Body::empty())
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
        .to_bytes()
        .to_vec();
    Answer {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
        bytes,
    }
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

/// CC-87: a project of layout 2 is exported from its own repository, not from the
/// organization repository's `projects/{slug}/`, which no longer holds it.
#[tokio::test]
async fn the_yaml_export_reads_the_project_repository() {
    let (_server, state) = world(HEAD).await;
    let answer = export(&state, steward(), "").await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(answer.text.contains("name: hluk"), "{}", answer.text);
    assert!(answer.text.contains("name: ovzdusie"), "{}", answer.text);
}

/// MF-45, MF-42, CC-88: one bundle per repository with its head in the index and each file's
/// SHA-256, the project's tags beside its bundle, and the registry entry without the values.
#[tokio::test]
async fn the_git_export_is_a_bundle_per_repository_with_its_head() {
    let (_server, state) = world(HEAD).await;
    let answer = export(&state, steward(), "?format=git").await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let files = unzip(&answer.bytes);
    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "air-map.bundle",
            "bundle.yaml",
            "ovzdusie.bundle",
            "ovzdusie.tags",
            "project.yaml",
            "projects/ovzdusie.yaml",
        ]
    );
    assert_eq!(
        files["ovzdusie.bundle"],
        bundle(HEAD),
        "the forge's bundle as it is"
    );
    assert_eq!(
        String::from_utf8_lossy(&files["ovzdusie.tags"]),
        format!("{TAGGED} refs/tags/v1.0.0\n")
    );

    let index: jc_core::kinds::Bundle =
        serde_norway::from_slice(&files["bundle.yaml"]).expect("the platform's Bundle");
    index.validate().expect("a valid index");
    let heads: Vec<(&str, &str)> = index
        .spec
        .repositories
        .iter()
        .map(|repository| (repository.name.as_str(), repository.head.as_str()))
        .collect();
    assert_eq!(heads, [("ovzdusie", HEAD), ("air-map", APP_HEAD)]);
    assert_eq!(index.spec.source_revision, HEAD);
    for file in &index.spec.files {
        assert_eq!(
            file.sha256,
            format!("{:x}", Sha256::digest(&files[&file.path])),
            "{}",
            file.path
        );
    }
    assert_eq!(
        index.spec.files.len(),
        files.len() - 1,
        "every file but the index"
    );

    let entry: Value =
        serde_norway::from_slice(&files["projects/ovzdusie.yaml"]).expect("the entry");
    assert!(entry["spec"].get("parameters").is_none(), "{entry}");
    assert_eq!(entry["spec"]["repository"]["name"], "ovzdusie");
}

/// MF-46: a bundle that ends elsewhere than the head just read is not exported.
#[tokio::test]
async fn a_repository_that_moved_during_the_export_is_not_exported() {
    let (_server, state) = world(TAGGED).await;
    let answer = export(&state, steward(), "?format=git").await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("export again"), "{}", answer.text);
}

/// MF-18: a bundle leaves nothing out, so a caller who may read one space of the project and
/// not the other gets no bundle; the zip export still answers, without the other space.
#[tokio::test]
async fn a_caller_who_reads_part_of_the_project_gets_no_bundle() {
    let (_server, state) = world(HEAD).await;
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-reads-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "space-reader",
            "scope": { "contextSpace": "ovzdusie" }
        }),
    ));
    // The space the binding names, held by the project (T-4589).
    state
        .mirror
        .upsert(envelope("ContextSpace", "ovzdusie", "ovzdusie", json!({})));
    let answer = export(&state, person("jana"), "?format=git").await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    // What the caller names still comes, narrowed to what it reads (PF-59, UI-87).
    let partial = export(&state, person("jana"), "?format=zip&names=ovzdusie").await;
    assert_eq!(partial.status, StatusCode::OK, "{}", partial.text);
}

/// MF-45: layout 1 has no project repository to bundle, and a git export is the whole
/// repository, so filters and a revision are refused.
#[tokio::test]
async fn a_git_export_is_whole_and_of_layout_two() {
    let (_server, state) = world(HEAD).await;
    for query in ["?format=git&kinds=spaces", "?format=git&revision=main"] {
        let answer = export(&state, steward(), query).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{query}: {}",
            answer.text
        );
    }
    state.mirror.set_repositories(BTreeMap::new());
    state.mirror.set_layout(1);
    let answer = export(&state, steward(), "?format=git").await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
}

/// CC-87: a project's revisions in layout 2 are the history of its whole repository, asked for
/// with no path, since the project is that repository's root.
#[tokio::test]
async fn the_revisions_are_the_project_repository_history() {
    let (server, state) = world(HEAD).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/ovzdusie/commits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "sha": HEAD,
            "commit": { "message": "open", "author": { "name": "jana", "date": "2026-09-22T10:00:00Z" } }
        }])))
        .mount(&server)
        .await;
    let answer = common::send(
        &state,
        steward(),
        "GET",
        "/api/v1/projects/ovzdusie/revisions",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(answer.text.contains(HEAD), "{}", answer.text);
    let asked = server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .find(|r| r.url.path().ends_with("/ovzdusie/commits"))
        .expect("the project repository's history was read");
    assert!(
        !asked.url.query_pairs().any(|(key, _)| key == "path"),
        "{}",
        asked.url
    );
}

const ORG_MODEL: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: DataModel\nmetadata:\n  name: stations\n  namespace: org\nspec:\n  linkml: ./stations.linkml.yaml\n  version: 1.3.0\n  lifecycle: published\n  classes: [Station]\n";
const ORG_SOURCE: &str = "id: https://bb.sk/stations\nname: stations\nclasses:\n  Station: {}\n";
const PARKING_SOURCE: &str =
    "id: https://bb.sk/parking\nname: parking\nimports: [linkml:types, org.stations.v1]\n";

/// The project's model `parking` importing the organization's `stations` (DM-76), and the
/// organization repository holding `stations` at `version`.
async fn importing(server: &MockServer, state: &AppState, version: &str) {
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/ovzdusie/contents/spaces/ovzdusie/datamodels/parking.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "sha": "blob-parking", "content": STANDARD.encode(PARKING_SOURCE) }),
        ))
        .with_priority(1)
        .mount(server)
        .await;
    for (file, text) in [
        ("stations.yaml", ORG_MODEL),
        ("stations.linkml.yaml", ORG_SOURCE),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "{}/contents/datamodels/stations/{file}",
                common::REPO
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "sha": format!("blob-{file}"), "content": STANDARD.encode(text) }),
            ))
            .with_priority(1)
            .mount(server)
            .await;
    }
    state.mirror.upsert(envelope(
        "DataModel",
        "parking",
        "ovzdusie",
        json!({ "contextSpaceRef": "ovzdusie", "linkml": "./parking.linkml.yaml", "version": "1.0.0", "lifecycle": "published", "classes": ["ParkingSpot", "Station"] }),
    ));
    state.mirror.upsert(envelope(
        "DataModel",
        "stations",
        ORG_NAMESPACE,
        json!({ "linkml": "./stations.linkml.yaml", "version": version, "lifecycle": "published", "classes": ["Station"] }),
    ));
}

/// MF-49: the organization model a project imports travels with it, its manifest and source as
/// the organization holds them, listed in the index with version, checksum and origin.
#[tokio::test]
async fn an_imported_organization_model_travels_with_the_project() {
    let (server, state) = world(HEAD).await;
    importing(&server, &state, "1.3.0").await;

    let answer = export(&state, steward(), "?format=git").await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let files = unzip(&answer.bytes);
    assert_eq!(
        String::from_utf8_lossy(&files["models/stations.v1.linkml.yaml"]),
        ORG_SOURCE
    );
    assert_eq!(
        String::from_utf8_lossy(&files["models/stations.v1.yaml"]),
        ORG_MODEL
    );
    let index: jc_core::kinds::Bundle =
        serde_norway::from_slice(&files["bundle.yaml"]).expect("the platform's Bundle");
    index.validate().expect("a valid index");
    let [model] = index.spec.models.as_slice() else {
        panic!("one carried model: {:?}", index.spec.models);
    };
    assert_eq!(
        (model.name.as_str(), model.version.to_string()),
        ("stations", "1.3.0".to_owned())
    );
    assert_eq!(model.sha256, format!("{:x}", Sha256::digest(ORG_SOURCE)));
    assert_eq!(
        (
            model.origin.organization.as_str(),
            model.origin.name.as_str()
        ),
        ("bb", "stations")
    );
    // Schema files only: the two files of the model and nothing else of the organization.
    let carried: Vec<&str> = files
        .keys()
        .filter(|path| path.starts_with("models/"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        carried,
        ["models/stations.v1.linkml.yaml", "models/stations.v1.yaml"]
    );
}

/// MF-49: a project importing a major the organization no longer holds is not exported, and the
/// refusal names the import.
#[tokio::test]
async fn an_import_the_organization_no_longer_holds_is_not_exported() {
    let (server, state) = world(HEAD).await;
    importing(&server, &state, "2.0.0").await;
    let answer = export(&state, steward(), "?format=git").await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("org.stations.v1"), "{}", answer.text);
    assert!(answer.text.contains("2.0.0"), "{}", answer.text);
}
