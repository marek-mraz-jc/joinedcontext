//! One-click export against a mocked forge (T-0192, CC-49, MF-16…MF-19).
//!
//! Everything an export hands out comes from the repository at one revision, so the forge is the
//! only thing these tests have to fake. What they check is what must never travel: live status,
//! a literal secret, or a file from another project.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8";

/// An Endpoint manifest with a status the Portal computed and a secret someone smuggled in.
const ENDPOINT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: public-air
  namespace: banskabystrica
spec:
  contextSpaceRef: ovzdusie
  slug: mluyob4nz52lok3ssk7pgn5vwt
  audience: public
  publish:
    apiKey: "jc_dead_beef_secret"
status:
  phase: Live
  sourceUrl: https://git.example.sk/bb/org/src/branch/main/x.yaml
"#;

const PIPELINE: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: aq-mqtt-ingest
  namespace: banskabystrica
spec:
  class: resident
  secretRefs:
    - name: mqtt-credentials
      key: password
"#;

/// An App whose image the build lane of this instance published (AP-13a).
const APP: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: air-map
  namespace: banskabystrica
  annotations:
    joinedcontext.com/image: "ghcr.io/bb/air-map@sha256:9f2b1c0d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
    joinedcontext.com/generated-by: "model-tools"
spec:
  endpointRefs: [public-air]
  visibility: project
"#;

/// A native file beside a manifest: Bento's own configuration, ours only to carry.
const BENTO: &str = "input:\n  mqtt:\n    urls: [ mqtts://mqtt.hsl.fi:8883 ]\n";

/// A data model whose LinkML source and generated JSON Schema are committed beside it (DM-02).
const AIR_MODEL: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: DataModel
metadata:
  name: air-quality
  namespace: banskabystrica
spec:
  contextSpaceRef: ovzdusie
  linkml: ./air-quality.linkml.yaml
  version: 1.0.0
  lifecycle: published
  classes: [AirQualityObserved]
  artifacts:
    jsonSchema: ./json-schema/air-quality.v1.json
"#;

const AIR_LINKML: &str =
    "id: https://example.org/air-quality\nclasses:\n  AirQualityObserved:\n    slots: [pm10]\n";

const AIR_SCHEMA: &str = r#"{"$schema": "http://json-schema.org/draft-07/schema#", "title": "AirQualityObserved", "properties": {"pm10": {"type": "number"}}}"#;

/// A model whose repository holds the source but no generated schema, on a Portal without Model
/// Tools: the export names what it could not include.
const NOISE_MODEL: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: DataModel
metadata:
  name: noise
  namespace: banskabystrica
spec:
  contextSpaceRef: hluk
  linkml: ./noise.linkml.yaml
  version: 0.1.0
  lifecycle: draft
"#;

const NOISE_LINKML: &str = "id: https://example.org/noise\n";

const OTHER_PROJECT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: doprava-public
  namespace: bb-doprava
spec:
  slug: zt4qm7ge2xdv6ksb3ncf5arw2y
"#;

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "projects/banskabystrica/endpoints/public-air.yaml",
            ENDPOINT,
        ),
        (
            "projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
            PIPELINE,
        ),
        (
            "projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml",
            BENTO,
        ),
        ("projects/banskabystrica/apps/air-map.yaml", APP),
        (
            "projects/banskabystrica/spaces/ovzdusie/datamodels/air-quality.yaml",
            AIR_MODEL,
        ),
        (
            "projects/banskabystrica/spaces/ovzdusie/datamodels/air-quality.linkml.yaml",
            AIR_LINKML,
        ),
        (
            "projects/banskabystrica/spaces/ovzdusie/datamodels/json-schema/air-quality.v1.json",
            AIR_SCHEMA,
        ),
        (
            "projects/banskabystrica/spaces/hluk/datamodels/noise.yaml",
            NOISE_MODEL,
        ),
        (
            "projects/banskabystrica/spaces/hluk/datamodels/noise.linkml.yaml",
            NOISE_LINKML,
        ),
        (
            "projects/bb-doprava/endpoints/doprava-public.yaml",
            OTHER_PROJECT,
        ),
    ]
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org/branches/main"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": REVISION } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/repos/bb/org/git/trees/{REVISION}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": files()
                .iter()
                .map(|(path, _)| json!({ "path": path, "type": "blob" }))
                .collect::<Vec<_>>(),
            "truncated": false
        })))
        .mount(&server)
        .await;
    for (file_path, content) in files() {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/repos/bb/org/contents/{file_path}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "b1ob",
                "content": STANDARD.encode(content),
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org/commits"))
        .and(query_param("path", "projects/banskabystrica"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "sha": REVISION,
                "commit": {
                    "message": "Endpoint public-air: add csv\n\nlonger body",
                    "author": { "name": "Jana Kováčová", "date": "2026-09-06T16:30:00Z" }
                }
            }
        ])))
        .mount(&server)
        .await;
    server
}

/// A `Role` that reads every kind an export carries and a `RoleBinding` that gives it to the
/// caller for one context space only, as `roles_matrix_tests::state_with` builds them, beside the
/// project's `ContextSpace` the binding names: a space binding holds in the project that holds the
/// space (T-4589).
fn bound_to(space: &str) -> Vec<ResourceEnvelope> {
    let in_namespace = |namespace: &str, kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    let manifest = |kind: &str, name: &str, spec: Value| in_namespace("org", kind, name, spec);
    vec![
        in_namespace("banskabystrica", "ContextSpace", space, json!({})),
        manifest(
            "Role",
            "space-reader",
            json!({ "rules": [{
                "kinds": ["DataModel", "Endpoint", "Pipeline", "App", "ContextSpace"],
                "verbs": ["read"]
            }] }),
        ),
        manifest(
            "RoleBinding",
            "space-reader-binding",
            json!({
                "subjects": [{ "user": "jana.kovacova" }],
                "role": "space-reader",
                "scope": { "contextSpace": space }
            }),
        ),
    ]
}

fn session_cookie(config: &Config) -> String {
    cookie_for(config, vec!["portal-approver".into()])
}

/// A signed-in person who is in no group the bootstrap names and holds no RoleBinding: what
/// PF-59 answers `404` (T-0819).
fn stranger_cookie(config: &Config) -> String {
    cookie_for(config, Vec::new())
}

fn cookie_for(config: &Config, groups: Vec<String>) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:jana".into(),
            username: "jana.kovacova".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups,
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

struct Answer {
    status: StatusCode,
    content_type: String,
    disposition: String,
    body: Vec<u8>,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn get(uri: &str, with_forge: bool) -> Answer {
    get_as(uri, with_forge, None).await
}

/// One export, optionally as a caller the mirror binds to a `Role` rather than as a bootstrap
/// administrator (PF-60, T-1206).
///
/// The bootstrap group answers every permission question with yes, so a fixture that uses it
/// cannot express a grant bound to one context space — which is exactly the boundary the export
/// filter draws. `bound` seeds the pair the binding needs and signs the caller in as its subject.
async fn get_as(uri: &str, with_forge: bool, bound: Option<&str>) -> Answer {
    let server = forge().await;
    let config = Config::for_tests();
    let cookie = match bound {
        None => session_cookie(&config),
        Some(_) => cookie_for(&config, Vec::new()),
    };
    let mut state = AppState::new(config, None);
    if let Some(space) = bound {
        for envelope in bound_to(space) {
            state.mirror.upsert(envelope);
        }
    }
    if with_forge {
        let client = GiteaClient::new(server.uri().parse().unwrap(), "bb", "org", "token")
            .expect("gitea client");
        state = state.with_gitea(Arc::new(client));
    }
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let header_of = |name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header_of(header::CONTENT_TYPE);
    let disposition = header_of(header::CONTENT_DISPOSITION);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        content_type,
        disposition,
        body: body.to_vec(),
    }
}

#[tokio::test]
async fn anonymous_callers_never_reach_the_repository() {
    let response = server::app(AppState::new(Config::for_tests(), None))
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/banskabystrica/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_a_forge_there_is_nothing_to_export() {
    let answer = get("/api/v1/projects/banskabystrica/export", false).await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.json()["status"], 503);
}

#[tokio::test]
async fn the_yaml_stream_carries_the_project_without_status_or_secrets() {
    let answer = get("/api/v1/projects/banskabystrica/export", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.content_type.starts_with("application/yaml"));
    assert!(
        answer.disposition.contains("banskabystrica-8c56954.yaml"),
        "the download names the project and the revision: {}",
        answer.disposition
    );

    let body = answer.text();
    assert!(body.contains("name: public-air"));
    assert!(body.contains("name: aq-mqtt-ingest"));
    assert!(
        !body.contains("jc_dead_beef_secret"),
        "MF-17: no secret value leaves in a bundle"
    );
    assert!(
        !body.contains("phase: Live") && !body.contains("sourceUrl"),
        "MF-17: status is the Portal's computation and is stripped"
    );
    assert!(
        body.contains("key: password"),
        "a secretRef names a key and must survive, or the bundle is not re-importable"
    );
    assert!(
        !body.contains("doprava-public"),
        "another project's manifest is not part of this export"
    );
}

#[tokio::test]
async fn kinds_and_names_narrow_the_stream() {
    let only_endpoints = get(
        "/api/v1/projects/banskabystrica/export?kinds=endpoints",
        true,
    )
    .await
    .text();
    assert!(only_endpoints.contains("public-air"));
    assert!(!only_endpoints.contains("aq-mqtt-ingest"));

    let by_name = get(
        "/api/v1/projects/banskabystrica/export?kinds=pipelines&names=aq-mqtt-ingest",
        true,
    )
    .await
    .text();
    assert!(by_name.contains("aq-mqtt-ingest"));
    assert!(!by_name.contains("public-air"));

    let nothing = get(
        "/api/v1/projects/banskabystrica/export?names=does-not-exist",
        true,
    )
    .await;
    assert_eq!(
        nothing.status,
        StatusCode::OK,
        "a filter that matches nothing is an empty export, not a 404"
    );
    assert!(nothing.text().trim().is_empty());
}

#[tokio::test]
async fn the_json_format_is_a_list_of_the_same_manifests() {
    let answer = get(
        "/api/v1/projects/banskabystrica/export?format=json&kinds=endpoints",
        true,
    )
    .await;
    assert!(answer.content_type.starts_with("application/json"));
    let body = answer.json();
    assert_eq!(body["kind"], "List");
    assert_eq!(body["metadata"]["revision"], REVISION);
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["metadata"]["name"], "public-air");
    assert!(items[0]["status"].is_null(), "status is stripped");
    assert_eq!(items[0]["spec"]["publish"]["apiKey"], "");
}

#[tokio::test]
async fn the_archive_is_a_zip_with_the_native_files_and_a_bundle_index() {
    let answer = get("/api/v1/projects/banskabystrica/export?format=zip", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.content_type, "application/zip");
    assert!(answer.disposition.contains("banskabystrica-8c56954.zip"));
    assert_eq!(&answer.body[..2], b"PK", "a zip starts with its magic");

    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(answer.body.clone())).expect("a readable zip");
    let names: Vec<String> = archive
        .file_names()
        .map(|n| n.expect("a text name").into_owned())
        .collect();
    assert!(names.contains(&"projects/banskabystrica/endpoints/public-air.yaml".to_string()));
    assert!(
        names.contains(&"projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml".to_string()),
        "the native file travels with its manifest (MF-17)"
    );
    assert!(!names.iter().any(|name| name.contains("bb-doprava")));

    let mut content = String::new();
    {
        use std::io::Read;
        archive
            .by_name("bundle.yaml")
            .expect("the bundle index at the root of the archive")
            .read_to_string(&mut content)
            .expect("readable");
    }
    assert!(content.contains("kind: Bundle"));
    assert!(content.contains(REVISION));
    assert!(content.contains("public-air"));
    assert!(content.contains("omitted: 0"));
    // T-0823: the index is the platform's own kind, so `jcctl validate` accepts the tree a
    // person unpacks instead of refusing the document the Portal just wrote.
    assert_eq!(
        jc_core::registry::validate_yaml("Bundle", &content),
        Some(Ok(())),
        "{content}"
    );

    // Deflate would hide a plain substring search, so every entry is read out and checked.
    for index in 0..archive.len() {
        use std::io::Read;
        let mut entry = archive.by_index(index).expect("entry");
        let name = entry.name().expect("a text name").into_owned();
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("utf-8 entry");
        assert!(
            !text.contains("jc_dead_beef_secret"),
            "a secret value survived into {name}"
        );
    }
}

/// MF-42: the index carries the SHA-256 of every file the archive holds, over the bytes as
/// exported, so an import can say whether the transfer arrived whole.
#[tokio::test]
async fn the_index_carries_the_checksum_of_every_file_in_the_archive() {
    use sha2::{Digest, Sha256};

    let answer = get("/api/v1/projects/banskabystrica/export?format=zip", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(answer.body.clone())).expect("a readable zip");

    let index: serde_json::Value =
        serde_norway::from_str(&entry(&mut archive, "bundle.yaml")).expect("the index parses");
    let files = index["spec"]["files"].as_array().expect("files");

    // Every file of the project, and nothing that only describes the bundle.
    let listed: Vec<&str> = files
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert!(
        listed
            .iter()
            .all(|path| path.starts_with("projects/banskabystrica/")),
        "{listed:?}"
    );
    assert!(
        listed.contains(&"projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml"),
        "the native file is checksummed too: {listed:?}"
    );

    for file in files {
        let path = file["path"].as_str().expect("a path");
        let content = entry(&mut archive, path);
        assert_eq!(
            file["sha256"].as_str().unwrap_or_default(),
            format!("{:x}", Sha256::digest(content.as_bytes())),
            "the checksum of {path} is of the bytes the archive carries"
        );
    }
}

fn entry(archive: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> String {
    use std::io::Read;
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap_or_else(|_| panic!("{name} is in the archive"))
        .read_to_string(&mut text)
        .expect("utf-8 entry");
    text
}

#[tokio::test]
async fn a_whole_project_archive_says_what_its_files_mean() {
    let answer = get("/api/v1/projects/banskabystrica/export?format=zip", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(answer.body.clone())).expect("a readable zip");
    let names: Vec<String> = archive
        .file_names()
        .map(|n| n.expect("a text name").into_owned())
        .collect();
    for expected in [
        "README.md",
        "schemas/kinds/Endpoint.schema.json",
        "schemas/kinds/Pipeline.schema.json",
        "schemas/kinds/DataModel.schema.json",
        "schemas/models/air-quality/air-quality.linkml.yaml",
        "schemas/models/air-quality/air-quality.schema.json",
        "schemas/models/noise/noise.linkml.yaml",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "MF-41: {expected} in {names:?}"
        );
    }
    assert!(
        !names.contains(&"schemas/models/noise/noise.schema.json".to_string()),
        "a schema nobody could generate is not invented"
    );

    // What the fields mean travels with the kind: the manifest model's descriptions.
    let endpoint: Value =
        serde_json::from_str(&entry(&mut archive, "schemas/kinds/Endpoint.schema.json"))
            .expect("the kind schema is JSON");
    assert!(
        endpoint.to_string().contains("\"description\""),
        "the kind schema carries its field descriptions"
    );
    let model: Value = serde_json::from_str(&entry(
        &mut archive,
        "schemas/models/air-quality/air-quality.schema.json",
    ))
    .expect("the model schema is JSON");
    assert_eq!(model["title"], "AirQualityObserved");
    assert_eq!(
        entry(
            &mut archive,
            "schemas/models/air-quality/air-quality.linkml.yaml"
        ),
        AIR_LINKML
    );

    let readme = entry(&mut archive, "README.md");
    assert!(readme.contains("# Project banskabystrica"), "{readme}");
    assert!(readme.contains(REVISION), "{readme}");
    assert!(
        readme.contains("| Endpoint | 1 (`public-air`) | A published door onto a space"),
        "{readme}"
    );
    assert!(
        readme.contains("`schemas/kinds/DataModel.schema.json`"),
        "{readme}"
    );
    assert!(
        readme.contains("| air-quality | AirQualityObserved |"),
        "{readme}"
    );
    assert!(readme.contains("## Missing"), "{readme}");
    assert!(readme.contains("- noise: JSON Schema"), "{readme}");
    assert!(!readme.contains("doprava-public"));
}

/// PF-60, T-0986: a grant bound to one context space exports that space's manifests and no
/// other. The export hands out manifests, so the binding is the boundary; before the filter
/// asked `may_read_manifest` it asked only about the kind, and this caller got both spaces.
#[tokio::test]
async fn an_export_leaves_out_the_spaces_the_grant_is_not_bound_to() {
    // The whole project is an administrator's (UI-87); this caller names what it wants.
    let whole = get_as(
        "/api/v1/projects/banskabystrica/export",
        true,
        Some("ovzdusie"),
    )
    .await;
    assert_eq!(whole.status, StatusCode::FORBIDDEN, "{}", whole.text());
    assert!(
        whole.text().contains("organization administrators"),
        "{}",
        whole.text()
    );

    let answer = get_as(
        "/api/v1/projects/banskabystrica/export?names=air-quality,noise,public-air",
        true,
        Some("ovzdusie"),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text());
    let exported = answer.text();

    assert!(
        exported.contains("name: air-quality"),
        "the space the binding names is exported: {exported}"
    );
    assert!(
        !exported.contains("name: noise"),
        "a model of another space is not: {exported}"
    );
    // The Endpoint of `ovzdusie` still travels, so this is the space boundary and not a
    // narrowing to one kind.
    assert!(exported.contains("name: public-air"), "{exported}");
}

/// The same caller with no binding at all reads nothing, which is what makes the test above
/// about the space and not about being signed in (PF-59, R20).
#[tokio::test]
async fn a_caller_the_project_does_not_bind_exports_nothing_of_it() {
    let answer = get_as("/api/v1/projects/banskabystrica/export", true, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    let exported = answer.text();
    assert!(exported.contains("name: air-quality"), "{exported}");
    assert!(exported.contains("name: noise"), "{exported}");
}

#[tokio::test]
async fn the_yaml_and_json_exports_carry_the_schemas_in_their_index() {
    let yaml = get("/api/v1/projects/banskabystrica/export", true)
        .await
        .text();
    let documents: Vec<Value> = serde_norway::Deserializer::from_str(&yaml)
        .map(|document| {
            use serde::Deserialize;
            Value::deserialize(document).expect("a YAML document")
        })
        .collect();
    let index = documents.last().expect("documents");
    assert_eq!(index["kind"], "Bundle", "the stream closes with its index");
    assert!(index["spec"]["schemas"]["kinds"]["Endpoint"].is_object());
    assert_eq!(
        index["spec"]["schemas"]["models"]["air-quality"]["linkml"],
        AIR_LINKML
    );
    assert!(index["spec"]["readme"]
        .as_str()
        .is_some_and(|readme| readme.contains("`schemas.kinds.Endpoint`")));
    assert!(!yaml.contains("jc_dead_beef_secret"));

    let json = get(
        "/api/v1/projects/banskabystrica/export?format=json&kinds=endpoints",
        true,
    )
    .await
    .json();
    let kinds = json["schemas"]["kinds"].as_object().expect("schemas.kinds");
    assert_eq!(
        kinds.keys().collect::<Vec<_>>(),
        ["Endpoint"],
        "a kinds filter keeps the schemas of the kinds it selects"
    );
    assert!(json["readme"].as_str().is_some());

    let one = get(
        "/api/v1/projects/banskabystrica/export?format=json&kinds=endpoints&names=public-air",
        true,
    )
    .await
    .json();
    assert!(
        one["schemas"].is_null() && one["readme"].is_null(),
        "an export that names resources is those manifests alone"
    );
}

#[tokio::test]
async fn a_format_or_revision_the_export_does_not_serve_is_refused() {
    let bad_format = get("/api/v1/projects/banskabystrica/export?format=tar.gz", true).await;
    assert_eq!(bad_format.status, StatusCode::BAD_REQUEST);

    let traversal = get(
        "/api/v1/projects/banskabystrica/export?revision=../../etc/passwd",
        true,
    )
    .await;
    assert_eq!(
        traversal.status,
        StatusCode::BAD_REQUEST,
        "a revision is a commit or a branch, never a path"
    );

    let bad_project = get("/api/v1/projects/Banska%20Bystrica/export", true).await;
    assert_eq!(bad_project.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_revision_picker_reads_the_history_of_this_project() {
    let answer = get("/api/v1/projects/banskabystrica/revisions", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    let items = answer.json()["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["sha"], REVISION);
    assert_eq!(
        items[0]["message"], "Endpoint public-air: add csv",
        "the picker shows the subject line, not the whole message"
    );
    assert_eq!(items[0]["author"], "Jana Kováčová");

    let too_many = get("/api/v1/projects/banskabystrica/revisions?limit=500", true).await;
    assert_eq!(too_many.status, StatusCode::BAD_REQUEST);
    let none = get("/api/v1/projects/banskabystrica/revisions?limit=0", true).await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_session_with_no_binding_in_the_project_is_answered_404_everywhere_it_reads() {
    let server = forge().await;
    let config = Config::for_tests();
    let cookie = stranger_cookie(&config);
    let client = GiteaClient::new(server.uri().parse().unwrap(), "bb", "org", "token")
        .expect("gitea client");
    let state = AppState::new(config, None).with_gitea(Arc::new(client));
    let app = server::app(state);

    for uri in [
        "/api/v1/projects/banskabystrica/export",
        "/api/v1/projects/banskabystrica/revisions",
        "/api/v1/projects/banskabystrica/pipelines",
        "/api/v1/projects/banskabystrica/endpoints/public-air",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{uri} answered a person with no binding in the project (PF-59, MF-18)"
        );
        // The refusal repeats what the caller typed and nothing else: no other name of the
        // project reaches a person who may not read it (R20).
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("aq-mqtt-ingest") && !text.contains("mluyob4nz52lok3ssk7pgn5vwt"),
            "the refusal disclosed a name the caller did not ask for: {text}"
        );
    }
}

#[tokio::test]
async fn the_digest_this_instance_built_never_leaves_in_a_bundle() {
    // AP-11, AP-13a (T-0822): the annotation is what one environment's build lane published.
    // Carried into another instance it would deploy an image that instance never built, so it
    // is stripped exactly like `status` — while the provenance annotations travel.
    let yaml = get("/api/v1/projects/banskabystrica/export", true)
        .await
        .text();
    assert!(yaml.contains("air-map"), "the app is in the export");
    assert!(
        !yaml.contains("joinedcontext.com/image"),
        "the built digest left in the bundle: {yaml}"
    );
    assert!(
        yaml.contains("joinedcontext.com/generated-by"),
        "provenance still travels: {yaml}"
    );
}

fn archived(answer: &Answer) -> Vec<String> {
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text());
    zip::ZipArchive::new(std::io::Cursor::new(answer.body.clone()))
        .expect("a readable zip")
        .file_names()
        .map(|n| n.expect("a text name").into_owned())
        .collect()
}

/// T-1405: a native file is read exactly when the manifest it belongs to is. A caller bound to
/// `ovzdusie` gets that space's LinkML source and schema, and not the `hluk` model's source it
/// could not read as a manifest either; before, every native file of the project travelled.
#[tokio::test]
async fn an_archive_carries_only_the_native_files_of_what_the_caller_may_read() {
    let answer = get_as(
        "/api/v1/projects/banskabystrica/export?format=zip&names=air-quality,noise",
        true,
        Some("ovzdusie"),
    )
    .await;
    let names = archived(&answer);
    let base = "projects/banskabystrica/spaces";
    assert!(
        names.contains(&format!(
            "{base}/ovzdusie/datamodels/air-quality.linkml.yaml"
        )),
        "{names:?}"
    );
    assert!(
        names.contains(&format!(
            "{base}/ovzdusie/datamodels/json-schema/air-quality.v1.json"
        )),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("/hluk/")),
        "another space's files travelled: {names:?}"
    );
}

/// T-1405: an archive that names a resource or a kind carries that resource's native files and no
/// other's; a whole-project archive still carries them all.
#[tokio::test]
async fn the_filters_of_an_archive_hold_for_its_native_files() {
    let pipeline = "projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml";
    let air = "projects/banskabystrica/spaces/ovzdusie/datamodels/air-quality.linkml.yaml";
    let noise = "projects/banskabystrica/spaces/hluk/datamodels/noise.linkml.yaml";

    let named = archived(
        &get(
            "/api/v1/projects/banskabystrica/export?format=zip&names=aq-mqtt-ingest",
            true,
        )
        .await,
    );
    assert!(named.contains(&pipeline.to_string()), "{named:?}");
    assert!(
        !named.contains(&air.to_string()) && !named.contains(&noise.to_string()),
        "{named:?}"
    );

    let models = archived(
        &get(
            "/api/v1/projects/banskabystrica/export?format=zip&kinds=datamodels",
            true,
        )
        .await,
    );
    assert!(
        models.contains(&air.to_string()) && models.contains(&noise.to_string()),
        "{models:?}"
    );
    assert!(!models.contains(&pipeline.to_string()), "{models:?}");

    let whole = archived(&get("/api/v1/projects/banskabystrica/export?format=zip", true).await);
    for file in [pipeline, air, noise] {
        assert!(whole.contains(&file.to_string()), "{file}: {whole:?}");
    }
}
