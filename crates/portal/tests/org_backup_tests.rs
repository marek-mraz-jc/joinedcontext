//! An Organization's configuration backup (T-7154): only that Organization's settings, only for
//! its administrators, only into a versioned bucket, restored only into itself.
//!
//! The bucket is an in-process S3 (`FakeS3`) that answers the calls the backup makes, with its
//! versioning switchable; the forge holds both Organizations' projects in one repository, so a
//! bundle that reached past its own Organization would show it.

mod common;

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex};

use age::secrecy::ExposeSecret;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DOOR: &str = "/api/v1/organization/backups/configuration";
const HEAD: &str = "5d1e0c3b2a19f8e7d6c5b4a3928170615f4e3d2c";
const SECRET: &str = "wJalrXUtnFEMI-K7MDENG-bravoNEVERshown";

/// One object's versions, oldest first: `(version id, body or None for a delete marker, time)`.
type Versions = Vec<(String, Option<Vec<u8>>, String)>;

#[derive(Default)]
struct Bucket {
    versioning: String,
    objects: BTreeMap<String, Versions>,
    counter: u32,
}

type FakeS3 = Arc<Mutex<Bucket>>;

fn xml(body: String) -> axum::response::Response {
    axum::response::IntoResponse::into_response((
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        body,
    ))
}

fn error(status: StatusCode, code: &str) -> axum::response::Response {
    let mut response = xml(format!("<Error><Code>{code}</Code></Error>"));
    *response.status_mut() = status;
    response
}

/// The S3 calls the backup makes, path-style, on bucket `backups`. Every call must be signed.
async fn s3(
    State(bucket): State<FakeS3>,
    method: Method,
    uri: Uri,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let signed = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 Credential=AKIDBACKUP/"));
    if !signed {
        return error(StatusCode::FORBIDDEN, "AccessDenied");
    }
    let query: BTreeMap<String, String> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_owned(), v.replace("%2F", "/"))
        })
        .collect();
    let path = uri.path().trim_start_matches('/');
    let (name, key) = path.split_once('/').unwrap_or((path, ""));
    if name != "backups" {
        return error(StatusCode::NOT_FOUND, "NoSuchBucket");
    }
    let mut bucket = bucket.lock().expect("bucket");
    bucket.counter += 1;
    let tick = bucket.counter;
    let now = format!("2026-11-16T10:{:02}:{:02}.000Z", tick / 60, tick % 60);
    match (method.as_str(), key.is_empty()) {
        ("GET", true) if query.contains_key("versioning") => {
            let status = match bucket.versioning.as_str() {
                "" => String::new(),
                s => format!("<Status>{s}</Status>"),
            };
            xml(format!(
                "<VersioningConfiguration>{status}</VersioningConfiguration>"
            ))
        }
        ("GET", true) if query.contains_key("object-lock") => error(
            StatusCode::NOT_FOUND,
            "ObjectLockConfigurationNotFoundError",
        ),
        ("GET", true) if query.contains_key("versions") => {
            let prefix = query.get("prefix").cloned().unwrap_or_default();
            let mut out = String::from("<ListVersionsResult>");
            for (key, versions) in bucket
                .objects
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
            {
                for (i, (id, body, at)) in versions.iter().enumerate() {
                    let latest = i + 1 == versions.len();
                    let tag = if body.is_some() {
                        "Version"
                    } else {
                        "DeleteMarker"
                    };
                    let size = body.as_ref().map_or(0, Vec::len);
                    out.push_str(&format!(
                        "<{tag}><Key>{key}</Key><VersionId>{id}</VersionId><IsLatest>{latest}</IsLatest><LastModified>{at}</LastModified><Size>{size}</Size></{tag}>"
                    ));
                }
            }
            out.push_str("</ListVersionsResult>");
            xml(out)
        }
        ("PUT", false) => {
            let id = format!("v{tick}");
            let enabled = bucket.versioning == "Enabled";
            let versions = bucket.objects.entry(key.to_owned()).or_default();
            if !enabled {
                versions.clear();
            }
            versions.push((id.clone(), Some(body.to_vec()), now));
            let mut response = xml(String::new());
            response
                .headers_mut()
                .insert("x-amz-version-id", id.parse().expect("header"));
            response
        }
        ("GET", false) => {
            let wanted = query.get("versionId").cloned().unwrap_or_default();
            bucket
                .objects
                .get(key)
                .and_then(|versions| versions.iter().find(|(id, _, _)| *id == wanted))
                .and_then(|(_, body, _)| body.clone())
                .map_or_else(
                    || error(StatusCode::NOT_FOUND, "NoSuchVersion"),
                    axum::response::IntoResponse::into_response,
                )
        }
        ("DELETE", false) => {
            let versions = bucket.objects.entry(key.to_owned()).or_default();
            match query.get("versionId") {
                Some(id) => versions.retain(|(v, _, _)| v != id),
                None => versions.push((format!("m{tick}"), None, now)),
            }
            let mut response = xml(String::new());
            *response.status_mut() = StatusCode::NO_CONTENT;
            response
        }
        _ => error(StatusCode::BAD_REQUEST, "InvalidRequest"),
    }
}

async fn fake_s3(versioning: &str) -> (String, FakeS3) {
    let bucket: FakeS3 = Arc::new(Mutex::new(Bucket {
        versioning: versioning.to_owned(),
        ..Default::default()
    }));
    let app = axum::Router::new().fallback(s3).with_state(bucket.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}"), bucket)
}

const ALFA_ENDPOINT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: alfa-air
  namespace: alfa-city
spec:
  contextSpaceRef: air
  slug: alfaairslug
  audience: public
  publish:
    apiKey: "jc_alfa_literal_api_key"
"#;

const ALFA_SCRIPT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Script
metadata:
  name: convert
  namespace: alfa-city
spec:
  language: javascript
  description: Converts
  versions:
    - version: 1.0.0
      state: published
      code: "ALFA_SCRIPT_CODE_MARK()"
      sha256: "00"
"#;

const BRAVO_ENDPOINT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: bravo-bus
  namespace: bravo-town
spec:
  contextSpaceRef: transit
  slug: bravobusslug
  audience: public
"#;

fn repository() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "projects/alfa-city/spaces/air/endpoints/alfa-air.yaml",
            ALFA_ENDPOINT,
        ),
        ("projects/alfa-city/scripts/convert.yaml", ALFA_SCRIPT),
        (
            "projects/alfa-city/pipelines/ingest/bento.yaml",
            "input:\n  generate:\n    mapping: ALFA_BENTO_MARK\n",
        ),
        (
            "projects/bravo-town/spaces/transit/endpoints/bravo-bus.yaml",
            BRAVO_ENDPOINT,
        ),
    ]
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    let repo = "/odata/Repositories(Owner='jc',Name='org')";
    Mock::given(method("GET"))
        .and(path(repo))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{repo}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Tree": repository().iter().map(|(p, _)| json!({ "Path": p, "Type": "blob" })).collect::<Vec<_>>(),
            "Truncated": false
        })))
        .mount(&server)
        .await;
    for (file, content) in repository() {
        Mock::given(method("GET"))
            .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "b1ob",
                "Content": STANDARD.encode(content)
            })))
            .mount(&server)
            .await;
    }
    server
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    envelope(kind, name, ORG_NAMESPACE, spec)
}

struct World {
    state: AppState,
    bucket: FakeS3,
    endpoint: String,
    _forge: MockServer,
}

/// Two Organizations: `alfa` (the installation's, project `alfa-city`) and `bravo` (project
/// `bravo-town`). ada administers `installation`, bob administers the other one, mia only reads
/// alfa-city.
async fn world(installation: &str, versioning: &str) -> World {
    let identity = age::x25519::Identity::generate();
    let dir = std::env::temp_dir().join(format!(
        "org-backup-{}-{}",
        std::process::id(),
        common::unique::unique()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let key_file = dir.join("keys.txt");
    std::fs::write(
        &key_file,
        format!("{}\n", identity.to_string().expose_secret()),
    )
    .expect("key");
    let mut config = Config::for_tests();
    config.pipeline_secrets = Some(joinedcontext_portal::pipeline_secrets::Backend::Sops {
        age_key_file: key_file,
    });
    let forge = forge().await;
    let (endpoint, bucket) = fake_s3(versioning).await;
    let shako =
        ShakoClient::new(forge.uri().parse().expect("url"), "jc", "org", "token").expect("shako");
    let state = AppState::new(config, None)
        .with_shako(Arc::new(shako))
        .with_org_backup_egress(reqwest::Client::new());
    let other = if installation == "alfa" {
        "bravo"
    } else {
        "alfa"
    };
    let m = &state.mirror;
    m.upsert(org(
        "Organization",
        "alfa",
        json!({ "domain": "alfa.example", "installation": installation == "alfa" }),
    ));
    m.upsert(org(
        "Organization",
        "bravo",
        json!({ "domain": "bravo.example", "installation": installation == "bravo" }),
    ));
    m.upsert(org(
        "Project",
        "alfa-city",
        json!({ "organizationRef": "alfa" }),
    ));
    m.upsert(org(
        "Project",
        "bravo-town",
        json!({ "organizationRef": "bravo" }),
    ));
    m.upsert(org(
        "Portal",
        "bravo-portal",
        json!({ "organizationRef": "bravo", "host": "portal.bravo.example" }),
    ));
    m.upsert(org(
        "Role",
        "org-admin",
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding", "Project", "Endpoint"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    m.upsert(org(
        "Role",
        "reader",
        json!({ "rules": [{ "kinds": ["*"], "verbs": ["read"] }] }),
    ));
    for (name, who, role, scope) in [
        (
            "ada-admin",
            "ada@hel.fi",
            "org-admin",
            json!({ "organization": installation }),
        ),
        (
            "bob-admin",
            "bob@hel.fi",
            "org-admin",
            json!({ "organization": other }),
        ),
        (
            "mia-reader",
            "mia@hel.fi",
            "reader",
            json!({ "project": "alfa-city" }),
        ),
    ] {
        m.upsert(org(
            "RoleBinding",
            name,
            json!({ "subjects": [{ "user": who }], "role": role, "scope": scope }),
        ));
    }
    // The live manifests of the projects, as the reconciler's mirror holds them.
    for (_, content) in repository() {
        if let Some(mut live) = joinedcontext_portal::store::envelope_of(content) {
            // A write door refuses a literal credential, so the live Endpoint holds none.
            if live.kind == "Endpoint" {
                live.spec["publish"]["apiKey"] = json!("");
            }
            m.upsert(live);
        }
    }
    World {
        state,
        bucket,
        endpoint,
        _forge: forge,
    }
}

fn target(endpoint: &str) -> Value {
    json!({
        "endpoint": endpoint, "region": "eu-central-1", "bucket": "backups", "prefix": "jc/",
        "accessKeyId": "AKIDBACKUP", "secretAccessKey": SECRET,
        "enabled": true, "everyHours": 24, "keepDays": 90
    })
}

fn body(answer: &common::Answer) -> Value {
    serde_json::from_str(&answer.text).unwrap_or(Value::Null)
}

/// Every file of a zip, by path, as text.
fn unzip(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).expect("entry");
            let mut text = String::new();
            file.read_to_string(&mut text).expect("text");
            (file.name().expect("name").to_string(), text)
        })
        .collect()
}

fn latest(bucket: &FakeS3, key: &str) -> Vec<u8> {
    let bucket = bucket.lock().expect("bucket");
    bucket.objects[key]
        .iter()
        .rev()
        .find_map(|(_, body, _)| body.clone())
        .expect("a version")
}

async fn saved_and_run(world: &World, who: &str) -> Value {
    let saved = send(
        &world.state,
        person(who),
        "PUT",
        DOOR,
        Some(target(&world.endpoint)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let run = send(
        &world.state,
        person(who),
        "POST",
        &format!("{DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::OK, "{}", run.text);
    body(&run)
}

#[tokio::test]
async fn the_bundle_holds_only_its_organization_and_no_secret_or_code() {
    let world = world("alfa", "Enabled").await;
    let run = saved_and_run(&world, "ada").await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&latest(&world.bucket, "jc/alfa/configuration/bundle.zip"));

    for (path, text) in &files {
        for foreign in ["bravo", "bob@", "AKIDBACKUP", SECRET] {
            assert!(
                !path.contains(foreign) && !text.contains(foreign),
                "{foreign} in {path}"
            );
        }
        for code in [
            "ALFA_SCRIPT_CODE_MARK",
            "ALFA_BENTO_MARK",
            "jc_alfa_literal_api_key",
        ] {
            assert!(!text.contains(code), "{code} in {path}");
        }
        assert!(!path.ends_with("bento.yaml"), "{path}");
    }
    assert!(
        files.contains_key("organization/organizations/alfa.yaml"),
        "{:?}",
        files.keys()
    );
    assert!(files.contains_key("projects/alfa-city/spaces/air/endpoints/alfa-air.yaml"));
    assert!(files
        .keys()
        .any(|p| p.starts_with("schemas/kinds/Organization")));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    assert_eq!(index["organization"], "alfa");
    assert!(index["items"].as_array().is_some_and(|items| items
        .iter()
        .any(|i| i["id"] == "alfa-air" && i["revision"] == HEAD)));
    assert!(files["README.md"].contains("Left out on purpose"));
}

#[tokio::test]
async fn only_the_organizations_administrators_reach_its_backup() {
    let world = world("alfa", "Enabled").await;
    for (who, http, door, payload) in [
        ("mia", "GET", DOOR.to_owned(), None),
        ("mia", "PUT", DOOR.to_owned(), Some(target(&world.endpoint))),
        ("mia", "POST", format!("{DOOR}/run"), None),
        ("bob", "GET", DOOR.to_owned(), None),
        ("bob", "PUT", DOOR.to_owned(), Some(target(&world.endpoint))),
        ("bob", "GET", format!("{DOOR}/versions"), None),
        (
            "bob",
            "POST",
            format!("{DOOR}/versions/v1/restore"),
            Some(json!({})),
        ),
        ("bob", "DELETE", format!("{DOOR}/bundle"), None),
    ] {
        let answer = send(&world.state, person(who), http, &door, payload).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{who} {http} {door}: {}",
            answer.text
        );
    }
    assert!(world.bucket.lock().expect("bucket").objects.is_empty());
}

#[tokio::test]
async fn an_unversioned_bucket_is_refused_until_versioning_is_on() {
    let world = world("alfa", "").await;
    let refused = send(
        &world.state,
        person("ada"),
        "PUT",
        DOOR,
        Some(target(&world.endpoint)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("versioning"), "{}", refused.text);
    world.bucket.lock().expect("bucket").versioning = "Suspended".into();
    let refused = send(
        &world.state,
        person("ada"),
        "PUT",
        DOOR,
        Some(target(&world.endpoint)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);

    world.bucket.lock().expect("bucket").versioning = "Enabled".into();
    let saved = send(
        &world.state,
        person("ada"),
        "PUT",
        DOOR,
        Some(target(&world.endpoint)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    // The secret goes in and never comes back out.
    assert!(!saved.text.contains(SECRET), "{}", saved.text);
    let read = send(&world.state, person("ada"), "GET", DOOR, None).await;
    assert!(
        !read.text.contains(SECRET) && read.text.contains("AKIDBACKUP"),
        "{}",
        read.text
    );

    // Versioning turned off later: the run is refused and logged, nothing is written.
    world.bucket.lock().expect("bucket").versioning = "Suspended".into();
    let run = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::CONFLICT, "{}", run.text);
    assert!(world.bucket.lock().expect("bucket").objects.is_empty());
    let log = body(&send(&world.state, person("ada"), "GET", DOOR, None).await);
    assert_eq!(log["log"][0]["ok"], false, "{log}");
    assert_eq!(log["log"][0]["actor"], "ada");
}

#[tokio::test]
async fn a_field_out_of_range_is_named() {
    let world = world("alfa", "Enabled").await;
    for (field, value) in [
        ("endpoint", json!("not a url")),
        ("bucket", json!("No_Such")),
        ("prefix", json!("../up/")),
        ("everyHours", json!(0)),
        ("keepDays", json!(5000)),
        ("region", json!("EU CENTRAL")),
    ] {
        let mut input = target(&world.endpoint);
        input[field] = value;
        let answer = send(&world.state, person("ada"), "PUT", DOOR, Some(input)).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {}",
            answer.text
        );
        assert!(answer.text.contains(field), "{field}: {}", answer.text);
    }
}

#[tokio::test]
async fn a_deleted_bundle_keeps_its_versions_and_restores_with_no_change() {
    let world = world("alfa", "Enabled").await;
    let run = saved_and_run(&world, "ada").await;
    let version = run["versionId"].as_str().expect("version").to_owned();

    let deleted = send(
        &world.state,
        person("ada"),
        "DELETE",
        &format!("{DOOR}/bundle"),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text);
    let listed = body(
        &send(
            &world.state,
            person("ada"),
            "GET",
            &format!("{DOOR}/versions"),
            None,
        )
        .await,
    );
    assert_eq!(listed["versioning"], "Enabled");
    assert_eq!(listed["versions"][0]["deleteMarker"], true, "{listed}");
    assert_eq!(
        listed["versions"][1]["versionId"],
        version.as_str(),
        "{listed}"
    );

    let plan = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/{version}/restore"),
        Some(json!({})),
    )
    .await;
    assert_eq!(plan.status, StatusCode::OK, "{}", plan.text);
    let plan = body(&plan);
    assert_eq!(plan["changes"], 0, "{plan}");
    assert!(plan["groups"].as_array().is_some_and(|g| g
        .iter()
        .any(|g| g["name"] == "alfa-city" && g["unchanged"] == 1)));

    let log = body(&send(&world.state, person("ada"), "GET", DOOR, None).await);
    let actions: Vec<&str> = log["log"]
        .as_array()
        .expect("log")
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    assert_eq!(actions, ["restore-plan", "delete", "run"]);
}

#[tokio::test]
async fn a_bundle_item_naming_another_organizations_namespace_is_not_planned() {
    let world = world("alfa", "Enabled").await;
    saved_and_run(&world, "ada").await;
    let mut files = unzip(&latest(&world.bucket, "jc/alfa/configuration/bundle.zip"));
    // An item in alfa's own project folder that names bravo's Endpoint by namespace.
    let planted = "projects/alfa-city/spaces/transit/endpoints/bravo-bus.yaml";
    files.insert(planted.into(), BRAVO_ENDPOINT.into());
    let mut index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    index["items"].as_array_mut().expect("items").push(json!({
        "kind": "Endpoint", "id": "bravo-bus", "path": planted, "revision": HEAD
    }));
    files.insert(
        "index.yaml".into(),
        serde_norway::to_string(&index).expect("yaml"),
    );
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, text) in &files {
        writer
            .start_file(path.as_str(), zip::write::SimpleFileOptions::default())
            .expect("entry");
        std::io::Write::write_all(&mut writer, text.as_bytes()).expect("write");
    }
    let crafted = writer.finish().expect("zip").into_inner();
    world
        .bucket
        .lock()
        .expect("bucket")
        .objects
        .get_mut("jc/alfa/configuration/bundle.zip")
        .expect("alfa's bundle")
        .push((
            "crafted".into(),
            Some(crafted),
            "2026-11-16T11:00:00.000Z".into(),
        ));

    let refused = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/crafted/restore"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains(r#""path":"version""#),
        "{}",
        refused.text
    );
    assert!(
        !refused.text.contains("bravo-town") && !refused.text.contains("unchanged"),
        "{}",
        refused.text
    );
}

#[tokio::test]
async fn a_bundle_of_another_organization_is_not_restored() {
    let alfa = world("alfa", "Enabled").await;
    saved_and_run(&alfa, "ada").await;
    let alfa_bundle = latest(&alfa.bucket, "jc/alfa/configuration/bundle.zip");

    // bravo's bucket holds alfa's bundle under bravo's key.
    let bravo = world("bravo", "Enabled").await;
    saved_and_run(&bravo, "ada").await;
    let version = {
        let mut bucket = bravo.bucket.lock().expect("bucket");
        let versions = bucket
            .objects
            .get_mut("jc/bravo/configuration/bundle.zip")
            .expect("bravo's bundle");
        versions.push((
            "planted".into(),
            Some(alfa_bundle),
            "2026-11-16T11:00:00.000Z".into(),
        ));
        "planted"
    };
    let refused = send(
        &bravo.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/{version}/restore"),
        Some(json!({ "apply": true })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("only into its own Organization"),
        "{}",
        refused.text
    );
}
