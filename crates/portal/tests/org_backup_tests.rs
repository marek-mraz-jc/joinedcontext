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
use joinedcontext_portal::org_backup;
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
    world_with(installation, versioning, None).await
}

async fn world_with(installation: &str, versioning: &str, db: Option<sqlx::PgPool>) -> World {
    world_configured(installation, versioning, db, |_| {}).await
}

async fn world_configured(
    installation: &str,
    versioning: &str,
    db: Option<sqlx::PgPool>,
    tweak: impl FnOnce(&mut Config),
) -> World {
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
    tweak(&mut config);
    let forge = forge().await;
    let (endpoint, bucket) = fake_s3(versioning).await;
    let shako =
        ShakoClient::new(forge.uri().parse().expect("url"), "jc", "org", "token").expect("shako");
    let mut state = AppState::new(config, None);
    if let Some(db) = db {
        state = state.with_db(db);
    }
    let state = state
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

/// The working and recovery identities every configuration target of these tests encrypts to
/// (T-7268), one pair for the whole binary so a bundle of one world opens in another.
fn keys() -> &'static [age::x25519::Identity; 2] {
    static KEYS: std::sync::OnceLock<[age::x25519::Identity; 2]> = std::sync::OnceLock::new();
    KEYS.get_or_init(identities)
}

/// The working identity, as an administrator pastes it into a restore.
fn identity() -> String {
    secret_of(&keys()[0])
}

fn target(endpoint: &str) -> Value {
    json!({
        "endpoint": endpoint, "region": "eu-central-1", "bucket": "backups", "prefix": "jc/",
        "accessKeyId": "AKIDBACKUP", "secretAccessKey": SECRET,
        "enabled": true, "everyHours": 24, "keepDays": 90,
        "workingRecipient": keys()[0].to_public().to_string(),
        "recoveryRecipient": keys()[1].to_public().to_string()
    })
}

/// A bundle as the bucket holds it, opened with the working identity.
fn opened(sealed: &[u8]) -> Vec<u8> {
    decrypt(sealed, &keys()[0])
}

/// `plain` encrypted to both test recipients, as a run uploads a bundle.
fn sealed(plain: &[u8]) -> Vec<u8> {
    let recipients: Vec<_> = keys().iter().map(|k| k.to_public()).collect();
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .expect("recipients");
    let mut out = Vec::new();
    let mut writer = encryptor.wrap_output(&mut out).expect("encrypt");
    std::io::Write::write_all(&mut writer, plain).expect("write");
    writer.finish().expect("finish");
    out
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

/// Two due backups: alfa's configuration target as ada saves it, and the same target copied to
/// bravo, whose run fails, its secret being sealed for alfa's record (T-7268).
async fn two_due(world: &World) {
    let saved = send(
        &world.state,
        person("ada"),
        "PUT",
        DOOR,
        Some(target(&world.endpoint)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let alfa = world
        .state
        .org_backups
        .target("alfa", "configuration")
        .await
        .expect("read")
        .expect("saved");
    let bravo = org_backup::Target {
        organization: "bravo".into(),
        ..alfa
    };
    world.state.org_backups.save(&bravo).await.expect("saved");
}

async fn last_run(world: &World, organization: &str) -> Option<String> {
    world
        .state
        .org_backups
        .target(organization, "configuration")
        .await
        .expect("read")
        .expect("saved")
        .last_run_at
}

/// Two due backups start together, each on its own slot, and only the one that ran is stamped
/// (S1-24, CMP-09, LOP-07): one slow archive no longer holds the other Organization back.
#[tokio::test]
async fn two_due_backups_run_at_once_within_the_bound() {
    let world = world("alfa", "Enabled").await;
    two_due(&world).await;
    let slots = Arc::new(tokio::sync::Semaphore::new(2));
    let started = org_backup::schedule(&world.state, &slots).await;
    assert_eq!(started.len(), 2, "both due backups start in one turn");
    assert_eq!(slots.available_permits(), 0, "both run at the same time");
    assert!(
        last_run(&world, "alfa").await.is_none(),
        "a run is stamped when it has run, not when it is claimed"
    );
    for run in started {
        run.await.expect("the run ends");
    }
    assert_eq!(slots.available_permits(), 2);
    assert!(
        last_run(&world, "alfa").await.is_some(),
        "a run that succeeded"
    );
    assert!(
        last_run(&world, "bravo").await.is_none(),
        "a run that failed"
    );
    assert!(
        org_backup::schedule(&world.state, &slots).await.is_empty(),
        "a backup that ran is not due again, one that failed waits for the backoff"
    );
}

/// The bound holds: with one slot one backup runs and the other waits for the next turn, due
/// and unstamped.
#[tokio::test]
async fn a_due_backup_beyond_the_bound_waits_for_the_next_turn() {
    let world = world("alfa", "Enabled").await;
    two_due(&world).await;
    let slots = Arc::new(tokio::sync::Semaphore::new(1));
    let first = org_backup::schedule(&world.state, &slots).await;
    assert_eq!(first.len(), 1, "one slot runs one backup");
    assert!(org_backup::schedule(&world.state, &slots).await.is_empty());
    for run in first {
        run.await.expect("the run ends");
    }
    let second = org_backup::schedule(&world.state, &slots).await;
    assert_eq!(second.len(), 1, "the other backup runs at the next turn");
    for run in second {
        run.await.expect("the run ends");
    }
    assert!(org_backup::schedule(&world.state, &slots).await.is_empty());
}

/// A run that fails is not stamped as run: the schedule leaves it for the backoff and tries it
/// again after, and its administrator may still run it by hand at once (S1-24).
#[tokio::test]
async fn a_failed_backup_is_not_stamped_as_run() {
    let world = world("alfa", "Enabled").await;
    two_due(&world).await;
    world.bucket.lock().expect("bucket").versioning = "Suspended".into();
    let slots = Arc::new(tokio::sync::Semaphore::new(2));
    for run in org_backup::schedule(&world.state, &slots).await {
        run.await.expect("the run ends");
    }
    assert!(
        last_run(&world, "alfa").await.is_none(),
        "a failed run is not stamped"
    );
    assert!(
        org_backup::schedule(&world.state, &slots).await.is_empty(),
        "a failed backup waits for the backoff, not the whole interval"
    );
    let entries = world
        .state
        .org_backups
        .entries("alfa", "configuration", 1)
        .await
        .expect("log");
    assert!(!entries[0].ok, "the failure is logged");
    let manual = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(manual.status, StatusCode::CONFLICT, "{}", manual.text);
    assert!(manual.text.contains("versioning"), "{}", manual.text);
}

#[tokio::test]
async fn the_bundle_holds_only_its_organization_and_no_secret_or_code() {
    let world = world("alfa", "Enabled").await;
    let run = saved_and_run(&world, "ada").await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&opened(&latest(
        &world.bucket,
        &key_of(&world, "alfa", "configuration").await,
    )));

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
async fn the_bundle_leaves_the_portal_only_encrypted_to_both_recipients() {
    let world = world("alfa", "Enabled").await;
    let run = saved_and_run(&world, "ada").await;
    assert_eq!(run["ok"], true, "{run}");
    let raw = latest(
        &world.bucket,
        &key_of(&world, "alfa", "configuration").await,
    );
    // A zip names its files in clear: none of them, nor the Organization, shows in the object.
    assert!(zip::ZipArchive::new(std::io::Cursor::new(&raw)).is_err());
    let text = String::from_utf8_lossy(&raw);
    for clear in [
        "index.yaml",
        "people.yaml",
        "organization/organizations/alfa.yaml",
        "alfa-city",
    ] {
        assert!(!text.contains(clear), "{clear} in the uploaded bundle");
    }
    assert!(raw.starts_with(b"age-encryption.org/v1"));
    assert_eq!(decrypt(&raw, &keys()[0]), decrypt(&raw, &keys()[1]));
    assert!(unzip(&opened(&raw)).contains_key("index.yaml"));

    let download = send(
        &world.state,
        person("ada"),
        "GET",
        &format!(
            "{DOOR}/versions/{}",
            run["versionId"].as_str().expect("version")
        ),
        None,
    )
    .await;
    assert_eq!(download.status, StatusCode::OK, "{}", download.text);
}

#[tokio::test]
async fn a_configuration_target_without_recipients_is_refused_naming_the_field() {
    let world = world("alfa", "Enabled").await;
    let mut input = target(&world.endpoint);
    input
        .as_object_mut()
        .expect("object")
        .remove("workingRecipient");
    input
        .as_object_mut()
        .expect("object")
        .remove("recoveryRecipient");
    let refused = send(&world.state, person("ada"), "PUT", DOOR, Some(input)).await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("workingRecipient"),
        "{}",
        refused.text
    );

    // A target saved before T-7268 holds no recipients: its run is refused, nothing uploads.
    saved_and_run(&world, "ada").await;
    let mut old = world
        .state
        .org_backups
        .target("alfa", "configuration")
        .await
        .expect("read")
        .expect("saved");
    old.recipients.clear();
    world.state.org_backups.save(&old).await.expect("saved");
    let key = key_of(&world, "alfa", "configuration").await;
    let before = world.bucket.lock().expect("bucket").objects[&key].len();
    let run = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::CONFLICT, "{}", run.text);
    assert!(run.text.contains("workingRecipient"), "{}", run.text);
    assert_eq!(
        world.bucket.lock().expect("bucket").objects[&key].len(),
        before
    );
}

#[tokio::test]
async fn a_plain_bundle_from_before_encryption_is_refused_with_its_date() {
    let world = world("alfa", "Enabled").await;
    saved_and_run(&world, "ada").await;
    let key = key_of(&world, "alfa", "configuration").await;
    let plain = opened(&latest(&world.bucket, &key));
    let taken = serde_norway::from_str::<Value>(&unzip(&plain)["index.yaml"]).expect("index")
        ["takenAt"]
        .as_str()
        .expect("takenAt")
        .to_owned();
    world
        .bucket
        .lock()
        .expect("bucket")
        .objects
        .get_mut(&key)
        .expect("alfa's bundle")
        .push((
            "plain".into(),
            Some(plain),
            "2026-11-16T11:00:00.000Z".into(),
        ));
    let refused = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/plain/restore"),
        Some(json!({ "identity": identity(), "apply": true })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(refused.text.contains("unencrypted"), "{}", refused.text);
    assert!(refused.text.contains(&taken), "{}", refused.text);
    // It still downloads as the plain zip it was uploaded as.
    let download = send(
        &world.state,
        person("ada"),
        "GET",
        &format!("{DOOR}/versions/plain"),
        None,
    )
    .await;
    assert_eq!(download.status, StatusCode::OK, "{}", download.text);
    assert!(download.text.starts_with("PK"), "a zip");

    // The wrong identity opens nothing and says which field.
    let other = secret_of(&age::x25519::Identity::generate());
    let version = world.bucket.lock().expect("bucket").objects[&key][0]
        .0
        .clone();
    let wrong = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/{version}/restore"),
        Some(json!({ "identity": other })),
    )
    .await;
    assert_eq!(
        wrong.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        wrong.text
    );
    assert!(
        wrong.text.contains(r#""path":"identity""#),
        "{}",
        wrong.text
    );
    assert!(!wrong.text.contains(&other), "the identity is never echoed");
}

#[tokio::test]
async fn a_seal_opens_only_for_the_organization_and_kind_it_was_sealed_for() {
    let world = world("alfa", "Enabled").await;
    saved_and_run(&world, "ada").await;
    let alfa = world
        .state
        .org_backups
        .target("alfa", "configuration")
        .await
        .expect("read")
        .expect("saved");
    assert!(joinedcontext_portal::org_backup::bucket(&world.state, &alfa).is_ok());
    // alfa's seal copied into bravo's row, or into alfa's data row, opens for neither.
    for (organization, kind) in [("bravo", "configuration"), ("alfa", "data")] {
        let copied = joinedcontext_portal::org_backup::Target {
            organization: organization.into(),
            kind: kind.into(),
            ..alfa.clone()
        };
        let refused = joinedcontext_portal::org_backup::bucket(&world.state, &copied)
            .err()
            .expect("refused");
        assert!(
            refused.to_string().contains("not sealed for this record"),
            "{organization}/{kind}: {refused}"
        );
        assert!(!refused.to_string().contains(SECRET));
    }
}

/// A secret sealed as the Portal sealed one before T-7268: the bare value, no binding.
fn unbound_seal(state: &AppState, secret: &str) -> String {
    let Some(joinedcontext_portal::pipeline_secrets::Backend::Sops { age_key_file }) =
        &state.config.pipeline_secrets
    else {
        panic!("the test Portal has an age key");
    };
    let text = std::fs::read_to_string(age_key_file).expect("key file");
    let own: age::x25519::Identity = text.trim().parse().expect("identity");
    let recipient = own.to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .expect("recipient");
    let mut out = Vec::new();
    let armor = age::armor::ArmoredWriter::wrap_output(&mut out, age::armor::Format::AsciiArmor)
        .expect("armor");
    let mut writer = encryptor.wrap_output(armor).expect("encrypt");
    std::io::Write::write_all(&mut writer, secret.as_bytes()).expect("write");
    writer
        .finish()
        .and_then(|armor| armor.finish())
        .expect("finish");
    String::from_utf8(out).expect("armored")
}

#[tokio::test]
async fn a_seal_from_before_the_binding_is_bound_to_its_row_once() {
    use joinedcontext_portal::org_backup::{rebound, reseal_unbound, target_context, unseal};
    let db = seeded_portal_db().await;
    let with_db = db.is_some();
    let world = world_with("alfa", "Enabled", db).await;
    saved_and_run(&world, "ada").await;
    let context = target_context("alfa", "configuration");
    let legacy = unbound_seal(&world.state, SECRET);
    assert!(unseal(&world.state, &context, &legacy).is_err());
    let bound = rebound(&world.state, &context, &legacy)
        .expect("rebound")
        .expect("moved");
    assert_eq!(
        unseal(&world.state, &context, &bound)
            .expect("opens")
            .as_str(),
        SECRET
    );
    assert!(rebound(&world.state, &context, &bound)
        .expect("rebound")
        .is_none());

    if !with_db {
        return;
    }
    let mut saved = world
        .state
        .org_backups
        .target("alfa", "configuration")
        .await
        .expect("read")
        .expect("saved");
    saved.sealed_secret = legacy;
    world.state.org_backups.save(&saved).await.expect("saved");
    assert_eq!(reseal_unbound(&world.state).await.expect("moved"), 1);
    assert_eq!(reseal_unbound(&world.state).await.expect("again"), 0);
    let moved = world
        .state
        .org_backups
        .target("alfa", "configuration")
        .await
        .expect("read")
        .expect("saved");
    assert!(joinedcontext_portal::org_backup::bucket(&world.state, &moved).is_ok());
}

/// An operator Change's SMTP password sealed before the binding (T-7268) is bound to its realm
/// once; a bound one and one that does not open stay as they are.
#[tokio::test]
async fn an_smtp_password_sealed_before_the_binding_is_bound_to_its_realm_once() {
    use joinedcontext_portal::org_backup::{reseal_unbound, seal, smtp_context, unseal};
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let world = world_with("alfa", "Enabled", Some(db.clone())).await;
    let legacy = unbound_seal(&world.state, "smtp-legacy-pass");
    let bound = seal(&world.state, &smtp_context("mail.bravo"), "smtp-bound-pass").expect("sealed");
    for (target, sealed) in [
        ("mail.alfa", legacy.as_str()),
        ("mail.bravo", bound.as_str()),
        ("mail.charlie", "not a seal"),
    ] {
        sqlx::query(
            "INSERT INTO operator_changes (kind, target, payload, proposed_by, sealed_secret) \
             VALUES ('SmtpSet', $1, '{}', 'ada', $2)",
        )
        .bind(target)
        .bind(sealed)
        .execute(&db)
        .await
        .expect("an open Change");
    }
    assert_eq!(reseal_unbound(&world.state).await.expect("moved"), 1);
    assert_eq!(reseal_unbound(&world.state).await.expect("again"), 0);
    let stored = |target: &'static str| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT sealed_secret FROM operator_changes WHERE target = $1",
            )
            .bind(target)
            .fetch_one(&db)
            .await
            .expect("the Change")
        }
    };
    let moved = stored("mail.alfa").await;
    assert_eq!(
        unseal(&world.state, &smtp_context("mail.alfa"), &moved)
            .expect("opens for its realm")
            .as_str(),
        "smtp-legacy-pass"
    );
    assert!(
        unseal(&world.state, &smtp_context("mail.bravo"), &moved).is_err(),
        "bound to its own realm only"
    );
    assert_eq!(stored("mail.bravo").await, bound);
    assert_eq!(stored("mail.charlie").await, "not a seal");
}

fn store_target(organization: &str, kind: &str) -> org_backup::Target {
    org_backup::Target {
        organization: organization.into(),
        kind: kind.into(),
        endpoint: "https://s3.example".into(),
        region: "eu-central-1".into(),
        bucket: "backups".into(),
        prefix: "jc/".into(),
        access_key_id: "AKID".into(),
        sealed_secret: String::new(),
        enabled: true,
        every_hours: 24,
        keep_days: 30,
        recipients: Vec::new(),
        updated_by: "ada@hel.fi".into(),
        updated_at: String::new(),
        last_run_at: None,
        key: None,
    }
}

fn entry<'a>(organization: &'a str) -> org_backup::NewEntry<'a> {
    org_backup::NewEntry {
        organization,
        kind: org_backup::CONFIGURATION,
        action: "run",
        actor: "ada@hel.fi",
        ok: true,
        bytes: 1,
        object_key: "jc/x",
        version_id: "v1",
        message: "ran",
        sha256: "",
    }
}

/// The Postgres store (ARCH/STO-07): each Organization's targets in its own keyed transaction,
/// claimed by one replica at a time, a run fenced by its lease token, stamped only on success;
/// an Organization outside the registry has no row.
#[tokio::test]
async fn the_postgres_store_claims_fences_and_stamps_each_organization_apart() {
    use org_backup::{BackupStore, CONFIGURATION, DATA};
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let store = BackupStore::new(Some(db.clone()));
    {
        let outside = "zulu";
        assert_eq!(
            store.target(outside, CONFIGURATION).await.expect("read"),
            None
        );
        let refused = store
            .save(&store_target(outside, CONFIGURATION))
            .await
            .expect_err("unregistered");
        assert!(
            matches!(refused, joinedcontext_portal::error::ApiError::NotFound(_)),
            "{refused}"
        );
        assert!(
            refused.to_string().contains("Organizations page"),
            "{refused}"
        );
        let refused = store
            .claim(outside, CONFIGURATION)
            .await
            .expect_err("unregistered");
        assert!(
            matches!(refused, joinedcontext_portal::error::ApiError::NotFound(_)),
            "{refused}"
        );
        let refused = store.log(entry(outside)).await.expect_err("unregistered");
        assert!(
            matches!(refused, joinedcontext_portal::error::ApiError::NotFound(_)),
            "{refused}"
        );
        assert!(store
            .entries(outside, CONFIGURATION, 10)
            .await
            .expect("read")
            .is_empty());
    }

    for (organization, kind) in [
        ("alfa", CONFIGURATION),
        ("alfa", DATA),
        ("bravo", CONFIGURATION),
    ] {
        store
            .save(&store_target(organization, kind))
            .await
            .expect("saved");
    }
    let first = store.claim_due(1).await.expect("claim");
    assert_eq!(first.len(), 1, "a claim takes no more than its limit");
    assert_eq!(
        (first[0].0.organization.as_str(), first[0].0.kind.as_str()),
        ("alfa", CONFIGURATION)
    );
    assert!(first[0].0.key.is_some(), "a claimed target carries its key");
    let rest = store.claim_due(10).await.expect("claim");
    assert_eq!(rest.len(), 2, "{rest:?}");
    assert!(
        store.claim_due(10).await.expect("claim").is_empty(),
        "every due target is held"
    );
    let refused = store.claim("alfa", CONFIGURATION).await.expect_err("held");
    assert!(
        matches!(refused, joinedcontext_portal::error::ApiError::Conflict(_)),
        "{refused}"
    );

    let (_, lease) = &first[0];
    assert!(store.renew(lease).await.expect("renew"));
    store.finished(lease, true).await.expect("ended");
    assert!(
        !store.renew(lease).await.expect("renew"),
        "an ended lease is fenced out"
    );
    let stamped = store
        .target("alfa", CONFIGURATION)
        .await
        .expect("read")
        .expect("saved");
    assert!(
        stamped.last_run_at.is_some(),
        "a run that succeeded is stamped"
    );
    for (target, lease) in &rest {
        store.finished(lease, false).await.expect("ended");
        let read = store
            .target(&target.organization, &target.kind)
            .await
            .expect("read")
            .expect("saved");
        assert!(read.last_run_at.is_none(), "a failed run is not stamped");
    }
    assert!(
        store.claim_due(10).await.expect("claim").is_empty(),
        "backoff and not due"
    );
    let manual = store
        .claim("bravo", CONFIGURATION)
        .await
        .expect("a backoff holds only the schedule back");
    store.finished(&manual, true).await.expect("ended");

    // A lease of an Organization the registry does not hold neither renews nor writes.
    let memory = BackupStore::new(None);
    memory
        .save(&store_target("zulu", CONFIGURATION))
        .await
        .expect("saved");
    let foreign = memory.claim("zulu", CONFIGURATION).await.expect("claimed");
    assert!(!store.renew(&foreign).await.expect("renew"));
    store
        .finished(&foreign, true)
        .await
        .expect("nothing to end");

    store.log(entry("alfa")).await.expect("logged");
    let entries = store
        .entries("alfa", CONFIGURATION, 10)
        .await
        .expect("read");
    assert_eq!(entries.len(), 1);
    assert!(store
        .entries("bravo", CONFIGURATION, 10)
        .await
        .expect("read")
        .is_empty());
}

/// A store whose database is gone answers an error a person can act on, and the schedule,
/// the reseal at start and the restore requests skip a turn instead of failing the Portal.
#[tokio::test]
async fn a_store_whose_database_is_gone_says_so_and_the_schedule_skips_a_turn() {
    use org_backup::{restore, BackupStore, CONFIGURATION};
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let gone = sqlx::PgPool::connect_with((*db.connect_options()).clone())
        .await
        .expect("a pool of its own");
    gone.close().await;
    let store = BackupStore::new(Some(gone.clone()));
    let refused = store.claim_due(2).await.expect_err("closed");
    assert!(
        refused.to_string().contains("the backup store failed"),
        "{refused}"
    );
    let world = world_with("alfa", "Enabled", Some(gone)).await;
    let slots = Arc::new(tokio::sync::Semaphore::new(org_backup::SCHEDULED_RUNS));
    assert!(org_backup::schedule(&world.state, &slots).await.is_empty());
    let none = Arc::new(tokio::sync::Semaphore::new(0));
    assert!(
        org_backup::schedule(&world.state, &none).await.is_empty(),
        "no free slot"
    );
    assert!(org_backup::reseal_unbound(&world.state).await.is_err());
    let refused = restore::requests(&world.state, "alfa")
        .await
        .expect_err("closed");
    assert!(
        refused.to_string().contains("backup store failed"),
        "{refused}"
    );
    assert!(world
        .state
        .org_backups
        .target("alfa", CONFIGURATION)
        .await
        .is_err());
    // The schedule's own task logs the failures and keeps ticking.
    org_backup::spawn_schedule(world.state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

/// Restore requests of an Organization outside the registry, or of a Portal without a
/// database, are empty; deciding one that does not exist is a 404 naming it.
#[tokio::test]
async fn a_restore_request_that_does_not_exist_is_named_and_an_unregistered_one_is_empty() {
    use org_backup::restore;
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let world = world_with("alfa", "Enabled", Some(db)).await;
    let state = &world.state;
    assert!(restore::requests(state, "zulu")
        .await
        .expect("read")
        .is_empty());
    assert!(!restore::any_pending(state, "zulu").await.expect("read"));
    assert!(restore::pending(state, "alfa", 999)
        .await
        .expect("read")
        .is_none());
    assert!(restore::decide(state, "alfa", 999, "ada", "declined", "no")
        .await
        .expect("decided")
        .is_none());
    let refused = restore::decide(state, "zulu", 1, "ada", "declined", "no")
        .await
        .expect_err("unregistered");
    assert!(
        refused.to_string().contains("not an active Organization"),
        "{refused}"
    );
    let declined = send(
        state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/restores/999/decline"),
        None,
    )
    .await;
    assert_eq!(declined.status, StatusCode::NOT_FOUND, "{}", declined.text);
    assert!(declined.text.contains("999"), "{}", declined.text);

    let bare = world_with("alfa", "Enabled", None).await;
    assert!(restore::requests(&bare.state, "alfa")
        .await
        .expect("read")
        .is_empty());
    assert!(!restore::any_pending(&bare.state, "alfa")
        .await
        .expect("read"));
    let refused = restore::decide(&bare.state, "alfa", 1, "ada", "declined", "no")
        .await
        .expect_err("no database");
    assert!(
        matches!(refused, joinedcontext_portal::error::ApiError::Conflict(_)),
        "{refused}"
    );
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
            Some(json!({ "identity": identity() })),
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

const DATA_DOOR: &str = "/api/v1/organization/backups/data";
/// Where a saved target's runs write: the Organization's reserved project key, never its name
/// (T-7207).
async fn key_of(world: &World, organization: &str, kind: &str) -> String {
    world
        .state
        .org_backups
        .target(organization, kind)
        .await
        .expect("read")
        .expect("saved")
        .object_key()
        .expect("keyed")
}

/// Rows of both Organizations in the Portal's tables, each with a mark the archive is searched
/// for, and secret material in the columns that must never leave. A project's rows are written
/// under its key, as `db::tenant_tx` writes them (the tables are forced since 0080, T-7201).
const PORTAL_SEED: &str = "
INSERT INTO organizations (name, host, state, created_by) VALUES ('alfa', 'alfa.example', 'active', 'seed');
INSERT INTO project_ids (id, name, organization) VALUES (gen_random_uuid(), 'alfa-city', 'alfa');
SELECT set_config('jc.org', (SELECT key::text FROM organizations WHERE name = 'alfa'), true), set_config('jc.project', (SELECT id::text FROM project_ids WHERE name = 'alfa-city' AND NOT reserved), true);
INSERT INTO activity (time, project, kind, source, summary, severity) VALUES (now(), 'alfa-city', 'pipeline', 'portal', 'alfa-activity-mark', 'info');
INSERT INTO entity_comments (project, space, urn, author, author_name, body) VALUES ('alfa-city', 'air', 'urn:ngsi-ld:X:alfa-1', 'ada', 'Ada', 'alfa-comment-mark');
INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, app_class, visibility, prompt, prompt_digest, data_needs, branch, path_prefix, status, ticket_hash, created_by, expires_at) VALUES ('run-alfa-1', 'alfa-city', 'a', 'e', 's', 'p', 'static', 'private', 'alfa-prompt-mark', 'd', '[]', 'b', 'x/', 'queued', 'ALFA_TICKET_HASH_SECRET', 'ada', now());
INSERT INTO agent_run_events (run_id, seq, kind, payload) VALUES ('run-alfa-1', 1, 'step', '{}');
INSERT INTO app_builds (project, app, commit, class, trigger, state, token_sha256) VALUES ('alfa-city', 'app', 'c', 'node', 'push', 'pending', 'ALFA_BUILD_TOKEN_SECRET');
INSERT INTO workspaces (name, project, owner, base_revision, scope, expires_at, preview_salt) VALUES ('ws-alfa', 'alfa-city', 'ada', 'r', '{}', now(), 'ALFA_PREVIEW_SALT_SECRET');
INSERT INTO domain_verifications (organization, domain, challenge, state) VALUES ('alfa', 'alfa.example', 'ALFA_DNS_CHALLENGE_SECRET', 'pending');
";

/// Runs `sql` under `project`'s key, as `db::tenant_tx` does: the project tables are forced
/// since 0080 (T-7201), so a row is written and read only in its project's key.
async fn as_project(db: &sqlx::PgPool, project: &str, sql: &str) {
    let key = joinedcontext_portal::project_ids::recorded(db, project)
        .await
        .expect("the registry")
        .expect("a seeded project");
    let mut tx = joinedcontext_portal::db::tenant_tx(db, key)
        .await
        .expect("tx");
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *tx)
        .await
        .expect(sql);
    tx.commit().await.expect("commit");
}

/// A database of its own for one test, migrated and seeded with both Organizations' rows; `None`
/// (the test says it skipped) without `JC_PORTAL_TEST_DATABASE_URL`.
async fn seeded_portal_db() -> Option<sqlx::PgPool> {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    let name = format!("org_data_{}", common::unique::unique().replace('-', "_"));
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    let mut own = url::Url::parse(&url).expect("url");
    own.set_path(&format!("/{name}"));
    let pool = joinedcontext_portal::db::connect(own.as_str())
        .await
        .expect("migrated");
    let bravo = PORTAL_SEED
        .replace("alfa-city", "bravo-town")
        .replace("alfa", "bravo")
        .replace("ALFA", "BRAVO");
    for seed in [PORTAL_SEED, bravo.as_str()] {
        let mut tx = pool.begin().await.expect("begin");
        for statement in seed.split(";\n") {
            if !statement.trim().is_empty() {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(&mut *tx)
                    .await
                    .expect(statement);
            }
        }
        tx.commit().await.expect("seeded");
    }
    Some(pool)
}

/// Two age identities: the working one and the recovery one.
fn identities() -> [age::x25519::Identity; 2] {
    [
        age::x25519::Identity::generate(),
        age::x25519::Identity::generate(),
    ]
}

fn data_target(endpoint: &str, ids: &[age::x25519::Identity; 2]) -> Value {
    let mut body = target(endpoint);
    body["workingRecipient"] = json!(ids[0].to_public().to_string());
    body["recoveryRecipient"] = json!(ids[1].to_public().to_string());
    body
}

fn decrypt(sealed: &[u8], identity: &age::x25519::Identity) -> Vec<u8> {
    let decryptor = age::Decryptor::new(sealed).expect("an age file");
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .expect("the identity opens it");
    let mut plain = Vec::new();
    reader.read_to_end(&mut plain).expect("read");
    plain
}

async fn data_saved_and_run(world: &World, ids: &[age::x25519::Identity; 2]) -> Value {
    let saved = send(
        &world.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&world.endpoint, ids)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let run = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::OK, "{}", run.text);
    body(&run)
}

#[tokio::test]
async fn the_data_archive_holds_only_its_organizations_rows_and_no_secret() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    // An Organization spelled like alfa's project: its reserved row carries the name `alfa-city`
    // (0067), and alfa's archive keeps only the project's own id.
    let city = joinedcontext_portal::project_ids::ensure(&db, &["alfa-city".to_string()])
        .await
        .expect("minted")["alfa-city"]
        .to_string();
    sqlx::query(
        "INSERT INTO organizations (name, host, state, created_by) \
         VALUES ('alfa-city', 'alfa-city.example', 'active', 'seed')",
    )
    .execute(&db)
    .await
    .expect("an Organization named like the project");
    let reserved: String = sqlx::query_scalar(
        "SELECT id::text FROM project_ids WHERE reserved AND name = 'alfa-city'",
    )
    .fetch_one(&db)
    .await
    .expect("its reserved project");
    let world = world_with("alfa", "Enabled", Some(db)).await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let sealed = latest(&world.bucket, &key_of(&world, "alfa", "data").await);
    // The data archive downloads as stored, still encrypted.
    let download = send(
        &world.state,
        person("ada"),
        "GET",
        &format!(
            "{DATA_DOOR}/versions/{}",
            run["versionId"].as_str().expect("version")
        ),
        None,
    )
    .await;
    assert_eq!(download.status, StatusCode::OK, "{}", download.text);
    assert!(
        download.text.starts_with("age-encryption.org/v1"),
        "encrypted"
    );
    assert!(
        zip::ZipArchive::new(std::io::Cursor::new(&sealed)).is_err(),
        "the stored archive is encrypted"
    );
    // Either key opens it: losing the working one loses no backup.
    assert_eq!(decrypt(&sealed, &ids[0]), decrypt(&sealed, &ids[1]));
    let files = unzip(&decrypt(&sealed, &ids[0]));

    for (path, text) in &files {
        for foreign in ["bravo", "BRAVO", "bob@"] {
            assert!(
                !path.contains(foreign) && !text.contains(foreign),
                "{foreign} in {path}"
            );
        }
        for secret in [
            "ALFA_TICKET_HASH_SECRET",
            "ALFA_BUILD_TOKEN_SECRET",
            "ALFA_PREVIEW_SALT_SECRET",
            "ALFA_DNS_CHALLENGE_SECRET",
            "ticket_hash",
            "token_sha256",
            "preview_salt",
            "challenge",
            "sealed_secret",
            SECRET,
        ] {
            assert!(!text.contains(secret), "{secret} in {path}");
        }
    }
    let all: String = files.values().cloned().collect();
    for mark in [
        "alfa-activity-mark",
        "alfa-comment-mark",
        "alfa-prompt-mark",
        "ws-alfa",
        "alfa.example",
    ] {
        assert!(all.contains(mark), "{mark} is missing");
    }
    // Format 2 (T-7208): each project's rows in the folder named by its id, the Organization's
    // own in its reserved project's.
    let segment = jc_types::ProjectId::parse_verified(&city)
        .expect("an id")
        .path_segment();
    let folder = format!("module/portal/project/{segment}/");
    assert!(files[&format!("{folder}agent_run_events.jsonl")].contains("run-alfa-1"));
    assert!(files[&format!("{folder}entity_comments.jsonl")].contains("alfa-comment-mark"));
    assert!(
        !files
            .keys()
            .any(|p| p.starts_with("module/portal/") && !p.starts_with("module/portal/project/")),
        "no flat Portal file beside the folders"
    );
    let registry = &files[&format!("{folder}project_ids.jsonl")];
    assert!(
        registry.contains(&city),
        "alfa-city's id is kept: {registry}"
    );
    assert!(
        !registry.contains(&reserved),
        "another Organization's reserved id left: {registry}"
    );
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    assert_eq!(index["formatVersion"], 2);
    assert_eq!(
        index["modules"]["portal"]["projects"][&segment],
        "alfa-city"
    );
    assert_eq!(
        segment.len(),
        26,
        "the folder names the id in its path form"
    );
    let own = index["modules"]["portal"]["organizationProject"]
        .as_str()
        .expect("the Organization's folder");
    assert!(files.contains_key(&format!(
        "module/portal/project/{own}/organization_backup_log.jsonl"
    )));
    assert_eq!(index["organization"], "alfa");
    assert_eq!(index["modules"]["portal"]["exported"], true);
    assert_eq!(
        index["modules"]["portal"]["keys"]["projects"],
        json!(["alfa-city"])
    );
    assert_eq!(index["modules"]["portal"]["tables"]["activity"], 1);
    assert!(index["modules"]["portal"]["snapshot"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert!(files["README.md"].contains("consistent in itself"));
}

/// T-7201: the Portal serves as a role without BYPASSRLS (T-7179), so the forced project tables
/// answer only the transaction's key. The archive still holds each of the Organization's project
/// rows once, read in that project's key, and none of another Organization's.
#[tokio::test]
async fn the_serving_role_exports_each_forced_project_row_once_in_its_key() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("the test database");
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&db)
        .await
        .expect("its name");
    sqlx::raw_sql(
        "DO $$ BEGIN CREATE ROLE pk_serving LOGIN NOBYPASSRLS PASSWORD 'pk-serving'; \
         EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$",
    )
    .execute(&db)
    .await
    .expect("the serving role");
    let mut owner = url::Url::parse(&url).expect("url");
    owner.set_path(&format!("/{name}"));
    let mut serving = owner.clone();
    serving.set_username("pk_serving").expect("user");
    serving.set_password(Some("pk-serving")).expect("password");
    let pool = joinedcontext_portal::db::connect_as(serving.as_str(), Some(owner.as_str()))
        .await
        .expect("served as a role without BYPASSRLS");
    let world = world_with("alfa", "Enabled", Some(pool)).await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&decrypt(
        &latest(&world.bucket, &key_of(&world, "alfa", "data").await),
        &ids[0],
    ));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    for table in ["activity", "entity_comments", "agent_runs", "workspaces"] {
        assert_eq!(
            index["modules"]["portal"]["tables"][table], 1,
            "{table}: alfa-city's one row, once: {}",
            index["modules"]["portal"]["tables"]
        );
    }
    let all: String = files.values().cloned().collect();
    assert!(all.contains("alfa-comment-mark") && all.contains("alfa-prompt-mark"));
    assert!(!all.contains("bravo"), "another Organization's rows left");
}

/// The modules' own schemas (their migrations as they ship), each with a row of alfa and of
/// bravo, read only through the `<module>_backup` role with the grants the deployment gives it
/// (deploy/components/jc/databases.yaml, which must name the Portal's column lists exactly).
const MODULE_SEED: &[(&str, &str)] = &[
    (
        "antares",
        "INSERT INTO entities (tenant_id, id, entity, version, types, location_ambiguous,
           created_at, modified_at)
         VALUES
           ('alfa-city-air', 'urn:ngsi-ld:Air:alfa-entity-mark', '{\"id\": \"alfa-value-mark\"}', 1,
            '{Air}', false, now(), now()),
           ('bravo-town-air', 'urn:ngsi-ld:Air:b1', '{\"id\": \"bravo-value\"}', 1, '{Air}', false,
            now(), now()),
           ('alfa-city-air-x', 'urn:ngsi-ld:Air:b2', '{\"id\": \"bravo-lookalike\"}', 1, '{Air}',
            false, now(), now());
         INSERT INTO iri_ids (tenant_id, iri) VALUES ('alfa-city-air', 'https://alfa.example/temperature-mark'),
           ('bravo-town-air', 'https://bravo.example/humidity');
         INSERT INTO attr_versions (tenant_id, entity_id, attr_id, instance_id, observed_at, version,
           data, superseded_at)
         VALUES ('alfa-city-air', 'urn:ngsi-ld:Air:alfa-entity-mark', 'temperature', 'i1', now(), 1,
           '{\"value\": \"alfa-history-mark\"}', now()),
           ('bravo-town-air', 'urn:ngsi-ld:Air:b1', 'humidity', 'i1', now(), 1,
           '{\"value\": \"bravo-history\"}', now())",
    ),
    (
        "kaisatsu",
        "INSERT INTO kaisatsu.realms (realm_id, name, doc, smtp_password_enc) VALUES
           ('alfa', 'alfa', '{}', '\x01'), ('bravo', 'bravo', '{}', '\x02');
         INSERT INTO kaisatsu.people (realm_id, id, username_lower, email_lower, doc) VALUES
           ('alfa', 'p-1', 'ada-alfa-mark', 'ada@alfa.example', '{}'),
           ('bravo', 'p-2', 'bob-bravo', 'bob@bravo.example', '{}');
         INSERT INTO kaisatsu.applications (realm_id, id, client_id, doc, secret_enc) VALUES
           ('alfa', 'a-1', 'alfa-client-mark', '{}', 'ALFA_CLIENT_SECRET_ENC'::bytea),
           ('bravo', 'a-2', 'bravo-client', '{}', 'BRAVO_CLIENT_SECRET_ENC'::bytea);
         INSERT INTO kaisatsu.credentials (realm_id, id, user_id, credential_type, doc) VALUES
           ('alfa', 'c-1', 'p-1', 'password', '{\"hash\": \"ALFA_PASSWORD_HASH\"}')",
    ),
    (
        "shako",
        "INSERT INTO shako.repo (id, instance_id, org_id, project_id, owner, name, description,
           default_branch, private, archived, size_bytes, next_pr_number, created_at, updated_at)
         VALUES
           ('00000000-0000-0000-0000-0000000000a1', '00000000-0000-0000-0000-000000000001',
            '00000000-0000-0000-0000-000000000001', 'alfa-city-config', 'jc', 'alfa-city-config',
            'alfa-repo-mark', 'main', true, false, 0, 1, now(), now()),
           ('00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-000000000001',
            '00000000-0000-0000-0000-000000000001', 'bravo-town-config', 'jc', 'bravo-town-config',
            'bravo-repo', 'main', true, false, 0, 1, now(), now()),
           ('00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-000000000001',
            '00000000-0000-0000-0000-000000000001', 'alfa-city-config', 'jc-apps',
            'alfa-city-config', 'bravo-app-named-like-alfa', 'main', true, false, 0, 1, now(), now());
         INSERT INTO shako.webhook (id, instance_id, org_id, repo_id, url, events, active,
           secret_ref, created_at)
         VALUES ('00000000-0000-0000-0000-0000000000a2', '00000000-0000-0000-0000-000000000001',
           '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000a1',
           'https://hook.example', '[]', true, 'ALFA_WEBHOOK_SECRET_REF', now())",
    ),
    (
        "daiya",
        "INSERT INTO cr_folder (installation_id, org_id, instance_id, project_id, uid, title,
           version, created_by, updated_by, created_at, updated_at)
         VALUES
           ('00000000-0000-0000-0000-000000000001', '{alfa_daiya}',
            '00000000-0000-0000-0000-000000000001', 'alfa-city', 'f-1', 'alfa-folder-mark', 1,
            'ada', 'ada', now(), now()),
           ('00000000-0000-0000-0000-000000000001', '{bravo_daiya}',
            '00000000-0000-0000-0000-000000000001', 'bravo-town', 'f-2', 'bravo-folder', 1,
            'bob', 'bob', now(), now()),
           ('00000000-0000-0000-0000-000000000001', '{retired_daiya}',
            '00000000-0000-0000-0000-000000000001', 'alfa-old', 'f-3', 'retired-folder', 1,
            'ada', 'ada', now(), now()),
           ('00000000-0000-0000-0000-000000000001', '{replaced_daiya}',
            '00000000-0000-0000-0000-000000000001', 'alfa-again', 'f-4', 'retired-before', 1,
            'ada', 'ada', now(), now()),
           ('00000000-0000-0000-0000-000000000001', '{recreated_daiya}',
            '00000000-0000-0000-0000-000000000001', 'alfa-again', 'f-5', 'recreated-folder-mark',
            1, 'ada', 'ada', now(), now());
         INSERT INTO cr_public_dashboard (installation_id, org_id, instance_id, uid,
           dashboard_uid, access_token_hash, is_enabled, time_selection_enabled,
           annotations_enabled, show_query_text, share, created_by, updated_by, created_at,
           updated_at)
         VALUES ('00000000-0000-0000-0000-000000000001', '{alfa_daiya}',
           '00000000-0000-0000-0000-000000000001', 'pd-1', 'd-1', 'ALFA_SHARE_TOKEN_HASH', true,
           false, false, false, 'public', 'ada', 'ada', now(), now())",
    ),
    (
        "dataroom",
        "INSERT INTO dataroom.cat_catalogue (org_id, id, name, host, title, default_language,
           main_profile, modified, doc)
         VALUES
           ('{alfa_org}', '{alfa_catalogue}', 'alfa', 'data.alfa.example', 'alfa-catalogue-mark',
            'en', 'dcat-ap', now(), '{{}}'),
           ('{bravo_org}', '{bravo_catalogue}', 'bravo', 'data.bravo.example', 'bravo-catalogue',
            'en', 'dcat-ap', now(), '{{}}');
         INSERT INTO dataroom.cat_harvest_source (org_id, catalogue_id, id, name, kind, url,
           publisher_id, enabled, modified, doc)
         VALUES ('{alfa_org}', '{alfa_catalogue}', 'h-1', 'alfa-harvest-mark', 'ckan',
           'https://ckan.example', 'pub-1', true, now(), '{{\"token\": \"ALFA_HARVEST_TOKEN\"}}')",
    ),
];

const ONE: &str = "00000000-0000-0000-0000-000000000001";

fn module_migrations(module: &str) -> Vec<&'static str> {
    match module {
        "antares" => vec![
            include_str!("../../antares/crates/antares-sql/migrations/0001_init.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0002_dead_letters.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0003_comma_seconds_fraction.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0004_drop_entity_maps.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0005_service_escape_by_command.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0006_context_tenant.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0007_outbox_claim_check.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0008_views.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0009_dataset_instances.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0010_history_retention_index.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0011_versions_provenance.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0012_entity_types.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0013_type_page_indexes.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0014_attrs_expire_at.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0015_rls_index_paths.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0016_entity_type_statistics.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0017_history_one_index.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0018_typed_location_paths.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0019_drop_entities_jsonb.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0020_history_integer_keys.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0021_name_key.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0022_iri_no_plpgsql.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0023_breaking_short_history_unlogged.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0024_entity_values.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0025_entity_attrs.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0026_views_digest.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0027_entities_id_bytes.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0028_offset_until.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0029_entity_values_diff.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0030_temporal_bare.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0031_versions_never_rewritten.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0032_entity_scope_keys.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0033_entity_id_remint.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0034_remint_batched.sql"),
            include_str!("../../antares/crates/antares-sql/migrations/0035_remint_ranges_bounded.sql"),
        ],
        "kaisatsu" => vec![
            include_str!("../../kaisatsu/store-pg/migrations/0001_init.sql"),
            include_str!("../../kaisatsu/store-pg/migrations/0002_k_items.sql"),
            include_str!("../../kaisatsu/store-pg/migrations/0003_device_code.sql"),
        ],
        "shako" => vec![
            include_str!("../../shako/store-pg/migrations/0001_init.sql"),
            include_str!("../../shako/store-pg/migrations/0002_store_log.sql"),
        ],
        "daiya" => vec![include_str!("../../daiya/store-pg/migrations/0001_initial.sql")],
        _ => vec![include_str!("../../dataroom/store-pg/migrations/0001_init.sql")],
    }
}

/// The deployment's grants of `<module>_backup`, as the role Job applies them.
fn deployed_grants(module: &str) -> Vec<(String, String, Vec<String>)> {
    let text = include_str!("../../../deploy/components/jc/databases.yaml");
    let databases: Value = serde_norway::from_str(text).expect("databases.yaml");
    let role = databases[format!("jc_{module}")]["additionalRoles"]
        .as_array()
        .and_then(|roles| {
            roles
                .iter()
                .find(|r| r["name"] == format!("{module}_backup").as_str())
        })
        .unwrap_or_else(|| panic!("databases.yaml has no {module}_backup role"))
        .clone();
    assert_eq!(role["type"], "backup");
    role["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .map(|g| {
            (
                g["schema"].as_str().expect("schema").to_owned(),
                g["table"].as_str().expect("table").to_owned(),
                g["columns"]
                    .as_array()
                    .expect("columns")
                    .iter()
                    .map(|c| c.as_str().expect("column").to_owned())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn the_deployment_grants_each_backup_reader_exactly_the_columns_the_portal_exports() {
    use joinedcontext_portal::org_backup::modules::MODULE_TABLES;
    for (module, schema, tables) in MODULE_TABLES {
        let portal: Vec<(String, String, Vec<String>)> = tables
            .iter()
            .map(|t| {
                (
                    (*schema).to_owned(),
                    t.name.to_owned(),
                    t.columns.iter().map(|c| (*c).to_owned()).collect(),
                )
            })
            .collect();
        assert_eq!(deployed_grants(module), portal, "{module}");
    }
}

/// One database per module, migrated as the module migrates it, seeded with alfa's and
/// bravo's rows, the backup reader granted as the deployment grants it; the reader's URL by
/// module. `None` without `JC_PORTAL_TEST_DATABASE_URL`.
async fn seeded_module_dbs(
    seed: &BTreeMap<&str, String>,
) -> Option<BTreeMap<&'static str, String>> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())?;
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    // Roles are the cluster's: made once, kept for the next run.
    sqlx::raw_sql(
        "DO $$ DECLARE r text; BEGIN
           FOREACH r IN ARRAY ARRAY['kaisatsu_app', 'shako_migrate', 'shako_app', 'daiya_migrate',
             'daiya_app', 'dataroom_app'] LOOP
             IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = r) THEN
               EXECUTE format('CREATE ROLE %I NOLOGIN', r);
             END IF;
           END LOOP;
           FOREACH r IN ARRAY ARRAY['antares_backup', 'kaisatsu_backup', 'shako_backup',
             'daiya_backup', 'dataroom_backup'] LOOP
             IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = r) THEN
               EXECUTE format('CREATE ROLE %I LOGIN NOBYPASSRLS', r);
             END IF;
             EXECUTE format('ALTER ROLE %I PASSWORD %L', r, 'backup-reader');
           END LOOP;
         END $$",
    )
    .execute(&admin)
    .await
    .expect("roles");
    let mut readers = BTreeMap::new();
    for (module, _) in MODULE_SEED {
        let name = format!(
            "org_mod_{module}_{}",
            common::unique::unique().replace('-', "_")
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("a fresh database");
        let mut own = url::Url::parse(&url).expect("url");
        own.set_path(&format!("/{name}"));
        let db = sqlx::PgPool::connect(own.as_str())
            .await
            .expect("module db");
        for migration in module_migrations(module) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(migration))
                .execute(&db)
                .await
                .unwrap_or_else(|e| panic!("{module} migration: {e}"));
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(seed[module].clone()))
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("{module} seed: {e}"));
        let role = format!("{module}_backup");
        let mut grants = format!("GRANT CONNECT ON DATABASE {name} TO {role};");
        let grants_of = deployed_grants(module);
        let schemas: std::collections::BTreeSet<&String> =
            grants_of.iter().map(|(s, _, _)| s).collect();
        for schema in schemas {
            grants.push_str(&format!(
                "REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM {role};
                 GRANT USAGE ON SCHEMA {schema} TO {role};"
            ));
        }
        for (schema, table, columns) in &grants_of {
            grants.push_str(&format!(
                "GRANT SELECT ({}) ON {schema}.{table} TO {role};",
                columns.join(", ")
            ));
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(grants))
            .execute(&db)
            .await
            .unwrap_or_else(|e| panic!("{module} grants: {e}"));
        own.set_username(&role).expect("user");
        own.set_password(Some("backup-reader")).expect("password");
        readers.insert(*module, own.to_string());
    }
    Some(readers)
}

#[tokio::test]
async fn each_modules_rows_leave_only_for_their_organization_and_no_secret_column_does() {
    use joinedcontext_portal::org_backup::modules::{daiya_org, DataroomTenant, Tenants};
    let one: uuid::Uuid = ONE.parse().expect("uuid");
    // daiya never held alfa-city's name, retired alfa-old's, and holds alfa-again's again.
    const RECREATED: &str = "0190a000-0000-7000-8000-0000000000aa";
    let daiya = MockServer::start().await;
    for (project, status, answer) in [
        ("alfa-city", 404, json!({ "error": { "status": 404 } })),
        (
            "alfa-old",
            404,
            json!({ "error": { "status": 404 }, "retired": true }),
        ),
        ("alfa-again", 200, json!({ "id": 5, "uuid": RECREATED })),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/orgs/name/{project}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(answer))
            .mount(&daiya)
            .await;
    }
    let tenant = |org: u128, catalogue: u128| DataroomTenant {
        org: uuid::Uuid::from_u128(org),
        catalogue: uuid::Uuid::from_u128(catalogue),
    };
    let dataroom = BTreeMap::from([
        ("alfa".to_owned(), tenant(0xa0, 0xa1)),
        ("bravo".to_owned(), tenant(0xb0, 0xb1)),
    ]);
    let fill = |text: &str| {
        text.replace("{alfa_daiya}", &daiya_org(one, "alfa-city").to_string())
            .replace("{bravo_daiya}", &daiya_org(one, "bravo-town").to_string())
            .replace("{retired_daiya}", &daiya_org(one, "alfa-old").to_string())
            .replace(
                "{replaced_daiya}",
                &daiya_org(one, "alfa-again").to_string(),
            )
            .replace("{recreated_daiya}", RECREATED)
            .replace("{alfa_org}", &dataroom["alfa"].org.to_string())
            .replace("{bravo_org}", &dataroom["bravo"].org.to_string())
            .replace("{alfa_catalogue}", &dataroom["alfa"].catalogue.to_string())
            .replace(
                "{bravo_catalogue}",
                &dataroom["bravo"].catalogue.to_string(),
            )
            .replace("{{", "{")
            .replace("}}", "}")
    };
    let seed: BTreeMap<&str, String> = MODULE_SEED.iter().map(|(m, s)| (*m, fill(s))).collect();
    let Some(readers) = seeded_module_dbs(&seed).await else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let mut world = world_configured("alfa", "Enabled", None, |config| {
        config.backup_modules.readers = readers;
        config.grafana = Some(joinedcontext_portal::config::GrafanaConfig {
            url: daiya.uri(),
            public_url: "https://dashboards.alfa.example".into(),
            user: "admin".into(),
            password: "unused".into(),
        });
        config.backup_modules.tenants = Tenants {
            shako: Some([one, one]),
            daiya_installation: Some(one),
            dataroom,
        };
    })
    .await;
    // alfa-city's space `air` is tenant alfa-city-air; bravo's project alfa-city-air (named like
    // it) has space `x`, tenant alfa-city-air-x, which is bravo's.
    let m = &world.state.mirror;
    m.upsert(org(
        "Project",
        "alfa-city-air",
        json!({ "organizationRef": "bravo" }),
    ));
    for project in ["alfa-old", "alfa-again"] {
        m.upsert(org(
            "Project",
            project,
            json!({ "organizationRef": "alfa" }),
        ));
    }
    for (project, space) in [
        ("alfa-city", "air"),
        ("bravo-town", "air"),
        ("alfa-city-air", "x"),
    ] {
        m.upsert(common::envelope("ContextSpace", space, project, json!({})));
    }
    world.state.mirror.set_repositories(BTreeMap::from([
        ("alfa-city".to_owned(), "alfa-city-config".to_owned()),
        ("bravo-town".to_owned(), "bravo-town-config".to_owned()),
    ]));
    // alfa signs in through a realm of its own; bravo's realm is never read for alfa.
    let oidc = joinedcontext_portal::config::OidcConfig {
        issuer: "https://idm.example/realms/jc".parse().expect("url"),
        client_id: "portal".into(),
        auth: joinedcontext_portal::auth::client_auth::ClientAuth::Secret("s".into()),
        extra_ca_pem: None,
        admin_url: None,
    };
    let setting = joinedcontext_portal::auth::realms::RealmSetting {
        organization: "alfa".into(),
        issuer: "https://idm.alfa.example/realms/alfa".parse().expect("url"),
        jwks_url: "http://127.0.0.1:9/realms/alfa/protocol/openid-connect/certs"
            .parse()
            .expect("url"),
    };
    world.state.organization_realms =
        joinedcontext_portal::auth::realms::OrganizationRealms::new(BTreeMap::from([(
            "alfa".to_owned(),
            Arc::new(joinedcontext_portal::auth::realms::OrganizationRealm::new(
                &setting,
                &oidc,
                &"https://portal.alfa.example".parse().expect("url"),
                None,
            )),
        )]));

    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&decrypt(
        &latest(&world.bucket, &key_of(&world, "alfa", "data").await),
        &ids[0],
    ));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    for module in ["antares", "kaisatsu", "shako", "daiya", "dataroom"] {
        assert_eq!(
            index["modules"][module]["exported"], true,
            "{module}: {}",
            index["modules"][module]
        );
    }
    for (path, text) in files.iter().filter(|(p, _)| p.starts_with("module/")) {
        for foreign in ["bravo", "bob", "lookalike", "alfa-city-air-x", "retired-"] {
            assert!(!text.contains(foreign), "{foreign} in {path}: {text}");
        }
        for secret in [
            "SECRET_ENC",
            "secret_enc",
            "smtp_password_enc",
            "PASSWORD_HASH",
            "WEBHOOK_SECRET_REF",
            "secret_ref",
            "SHARE_TOKEN_HASH",
            "access_token_hash",
            "HARVEST_TOKEN",
        ] {
            assert!(!text.contains(secret), "{secret} in {path}");
        }
    }
    for (path, mark) in [
        ("module/antares/entities.jsonl", "alfa-value-mark"),
        ("module/antares/iri_ids.jsonl", "temperature-mark"),
        ("module/antares/attr_versions.jsonl", "alfa-history-mark"),
        ("module/kaisatsu/people.jsonl", "ada-alfa-mark"),
        ("module/kaisatsu/applications.jsonl", "alfa-client-mark"),
        ("module/shako/repo.jsonl", "alfa-repo-mark"),
        ("module/daiya/cr_folder.jsonl", "alfa-folder-mark"),
        ("module/daiya/cr_folder.jsonl", "recreated-folder-mark"),
        ("module/dataroom/cat_catalogue.jsonl", "alfa-catalogue-mark"),
        (
            "module/dataroom/cat_harvest_source.jsonl",
            "alfa-harvest-mark",
        ),
    ] {
        assert!(files[path].contains(mark), "{mark} missing from {path}");
    }
    assert_eq!(
        index["modules"]["antares"]["keys"]["tenants"],
        json!(["alfa-city-air"])
    );
    assert_eq!(index["modules"]["antares"]["tables"]["entities"], 1);
    // A repository of the Apps' owner named like alfa's project repository is not alfa's.
    assert_eq!(index["modules"]["shako"]["tables"]["repo"], 1);
    assert!(!files
        .keys()
        .any(|p| p.contains("credentials") || p.contains("webhook")));
}

#[tokio::test]
async fn every_portal_table_is_exported_or_left_out_on_purpose() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema = 'public'",
    )
    .fetch_all(&db)
    .await
    .expect("tables");
    use joinedcontext_portal::org_backup::data::{PORTAL_LEFT_OUT, PORTAL_TABLES};
    for table in &tables {
        assert!(
            PORTAL_TABLES.iter().any(|t| t.name == table)
                || PORTAL_LEFT_OUT.iter().any(|(t, _)| t == table),
            "jc_portal table {table} is neither exported nor left out on purpose: decide it in \
             org_backup/data.rs"
        );
    }
}

#[tokio::test]
async fn a_backup_that_cannot_ask_daiya_exports_no_dashboards() {
    use joinedcontext_portal::org_backup::modules::Tenants;
    let world = world_configured("alfa", "Enabled", None, |config| {
        config.backup_modules.readers =
            BTreeMap::from([("daiya", "postgres://nobody@127.0.0.1:9/none".to_owned())]);
        config.backup_modules.tenants = Tenants {
            daiya_installation: Some(uuid::Uuid::from_u128(1)),
            ..Tenants::default()
        };
    })
    .await;
    let ids = identities();
    data_saved_and_run(&world, &ids).await;
    let files = unzip(&decrypt(
        &latest(&world.bucket, &key_of(&world, "alfa", "data").await),
        &ids[0],
    ));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    let daiya = &index["modules"]["daiya"];
    assert_eq!(daiya["exported"], false, "{daiya}");
    // Without daiya nobody can tell a retired name from one never held.
    assert!(
        daiya.to_string().contains("JC_PORTAL_GRAFANA_URL"),
        "{daiya}"
    );
}

/// An Organization the registry no longer holds as active has no keyed rows: its archive keeps
/// one flat file per table, so no unkeyed project row goes missing.
#[tokio::test]
async fn an_organization_outside_the_registry_gets_one_flat_file_per_table() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    sqlx::query("UPDATE organizations SET state = 'removed' WHERE name = 'alfa'")
        .execute(&db)
        .await
        .expect("removed");
    let mut world = world_with("alfa", "Enabled", Some(db)).await;
    // The registry refuses a removed Organization's targets; a memory store holds them.
    world.state.org_backups = Arc::new(org_backup::BackupStore::new(None));
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&decrypt(
        &latest(&world.bucket, &key_of(&world, "alfa", "data").await),
        &ids[0],
    ));
    assert!(
        files.keys().all(|name| !name.contains("/project/")),
        "{:?}",
        files.keys()
    );
    assert!(
        files.contains_key("module/portal/activity.jsonl"),
        "{:?}",
        files.keys()
    );
}

/// A module database the reader reaches but cannot read fails the run with the module named,
/// and nothing is uploaded or stamped.
#[tokio::test]
async fn a_module_database_that_cannot_be_read_fails_the_run_naming_it() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").expect("the test database");
    let world = world_configured("alfa", "Enabled", Some(db), |config| {
        config.backup_modules = joinedcontext_portal::org_backup::modules::BackupModules::from_lookup(|k| match k {
            "JC_PORTAL_BACKUP_DATAROOM_DATABASE_URL" => Some(url.clone()),
            "JC_PORTAL_BACKUP_TENANTS" => Some(
                r#"{"dataroom":{"alfa":{"org":"00000000-0000-0000-0000-000000000002","catalogue":"00000000-0000-0000-0000-000000000003"}}}"#
                    .into(),
            ),
            _ => None,
        })
        .expect("modules");
    })
    .await;
    let ids = identities();
    let saved = send(
        &world.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&world.endpoint, &ids)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let run = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::SERVICE_UNAVAILABLE, "{}", run.text);
    assert!(
        run.text.contains("the dataroom database could not be read"),
        "{}",
        run.text
    );
    let target = world
        .state
        .org_backups
        .target("alfa", "data")
        .await
        .expect("read")
        .expect("saved");
    assert!(target.last_run_at.is_none(), "a failed run is not stamped");
    let logged = world
        .state
        .org_backups
        .entries("alfa", "data", 1)
        .await
        .expect("log");
    assert!(!logged[0].ok, "the failure is in the log");
}

#[tokio::test]
async fn a_data_archive_without_a_database_says_what_it_left_out() {
    let world = world("alfa", "Enabled").await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&decrypt(
        &latest(&world.bucket, &key_of(&world, "alfa", "data").await),
        &ids[1],
    ));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    assert_eq!(index["modules"]["portal"]["exported"], false);
    assert_eq!(index["modules"]["antares"]["exported"], false);
    assert!(
        files["README.md"].contains("`portal`: not exported: this Portal runs without a database")
    );
    let overview = send(&world.state, person("ada"), "GET", DATA_DOOR, None).await;
    assert_eq!(overview.status, StatusCode::OK, "{}", overview.text);
    let fingerprints = &body(&overview)["fingerprints"];
    assert_eq!(
        fingerprints.as_array().map(Vec::len),
        Some(2),
        "{fingerprints}"
    );
    assert!(!overview.text.contains(SECRET));
}

#[tokio::test]
async fn only_the_organizations_administrators_reach_its_data_backup() {
    let world = world("alfa", "Enabled").await;
    let ids = identities();
    for who in ["mia", "bob"] {
        for (http, door, payload) in [
            ("GET", DATA_DOOR.to_owned(), None),
            (
                "PUT",
                DATA_DOOR.to_owned(),
                Some(data_target(&world.endpoint, &ids)),
            ),
            ("POST", format!("{DATA_DOOR}/run"), None),
            ("GET", format!("{DATA_DOOR}/versions"), None),
            ("GET", format!("{DATA_DOOR}/versions/v1"), None),
            ("DELETE", format!("{DATA_DOOR}/archive"), None),
            (
                "POST",
                format!("{DATA_DOOR}/versions/v1/restore"),
                Some(json!({ "identity": "AGE-SECRET-KEY-1" })),
            ),
            ("GET", format!("{DATA_DOOR}/restores"), None),
            (
                "POST",
                format!("{DATA_DOOR}/restores/1/approve"),
                Some(json!({ "identity": "AGE-SECRET-KEY-1" })),
            ),
            ("POST", format!("{DATA_DOOR}/restores/1/decline"), None),
        ] {
            let answer = send(&world.state, person(who), http, &door, payload).await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{who} {http} {door}: {}",
                answer.text
            );
        }
    }
    assert!(world.bucket.lock().expect("bucket").objects.is_empty());
}

#[tokio::test]
async fn a_recipient_that_is_not_an_age_key_is_refused_naming_the_field() {
    let world = world("alfa", "Enabled").await;
    let ids = identities();
    let good = data_target(&world.endpoint, &ids);
    let mut not_age = good.clone();
    not_age["workingRecipient"] = json!("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA");
    let mut missing = good.clone();
    missing["recoveryRecipient"] = Value::Null;
    let mut twice = good.clone();
    twice["recoveryRecipient"] = good["workingRecipient"].clone();
    let mut on_configuration = target(&world.endpoint);
    // The configuration bundle is encrypted too since T-7268: it needs both recipients.
    on_configuration["recoveryRecipient"] = Value::Null;
    for (door, payload, field) in [
        (DATA_DOOR, not_age, "workingRecipient"),
        (DATA_DOOR, missing, "recoveryRecipient"),
        (DATA_DOOR, twice, "recoveryRecipient"),
        (DOOR, on_configuration, "recoveryRecipient"),
    ] {
        let answer = send(&world.state, person("ada"), "PUT", door, Some(payload)).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(
            body(&answer)["errors"][0]["path"] == field,
            "{door} {field}: {}",
            answer.text
        );
    }
    let unversioned = world_with("alfa", "Suspended", None).await;
    let refused = send(
        &unversioned.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&unversioned.endpoint, &ids)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
}

/// Every log line written while the guard lives, at every level.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn secret_of(identity: &age::x25519::Identity) -> String {
    identity.to_string().expose_secret().to_owned()
}

fn version_ids(bucket: &FakeS3, key: &str) -> Vec<String> {
    bucket.lock().expect("bucket").objects[key]
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect()
}

#[tokio::test]
async fn a_deleted_backups_version_restores_with_a_second_admin_and_an_erased_person_stays_erased()
{
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    // A second comment, kept; the seeded one names the Entity an erasure removes later.
    as_project(
        &db,
        "alfa-city",
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ('alfa-city', 'air', 'urn:ngsi-ld:X:alfa-2', 'ada', 'Ada', 'alfa-kept-mark')",
    )
    .await;
    let world = world_with("alfa", "Enabled", Some(db.clone())).await;
    world.state.mirror.upsert(org(
        "RoleBinding",
        "cid-admin",
        json!({ "subjects": [{ "user": "cid@hel.fi" }], "role": "org-admin", "scope": { "organization": "alfa" } }),
    ));
    let log = Captured::default();
    let writer = log.clone();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let version = run["versionId"].as_str().expect("a version").to_owned();

    // Data lost after the backup, then the backup soft-deleted.
    for project in ["alfa-city", "bravo-town"] {
        as_project(
            &db,
            project,
            "DELETE FROM entity_comments WHERE project IN ('alfa-city', 'bravo-town')",
        )
        .await;
    }
    as_project(
        &db,
        "bravo-town",
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ('bravo-town', 'air', 'urn:ngsi-ld:X:bravo-9', 'bob', 'Bob', 'bravo-after-mark')",
    )
    .await;
    let deleted = send(
        &world.state,
        person("ada"),
        "DELETE",
        &format!("{DATA_DOOR}/archive"),
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text);
    assert_eq!(
        version_ids(&world.bucket, &key_of(&world, "alfa", "data").await).len(),
        2,
        "the version stays behind its marker"
    );
    // The person behind urn:ngsi-ld:X:alfa-1 is erased since the backup.
    as_project(
        &db,
        "alfa-city",
        "INSERT INTO erasures (id, project, space, subject, reason, actor, groups, stores, approval) \
         VALUES ('er-alfa-1', 'alfa-city', 'air', '0123456789abcdef0123456789abcdef', 'asked', 'ada', \
         '[{\"class\":\"X\",\"entities\":[\"urn:ngsi-ld:X:alfa-1\"],\"attrs\":null}]', '{}', 'approved')",
    )
    .await;

    let door = format!("{DATA_DOOR}/versions/{version}/restore");
    let wrong = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&age::x25519::Identity::generate()) })),
    )
    .await;
    assert_eq!(
        wrong.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        wrong.text
    );
    assert!(wrong.text.contains("identity"), "{}", wrong.text);

    let check = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[0]) })),
    )
    .await;
    assert_eq!(check.status, StatusCode::OK, "{}", check.text);
    let plan = &body(&check)["plan"];
    assert_eq!(
        plan["modules"]["portal"]["entity_comments"]["create"], 2,
        "{plan}"
    );
    assert_eq!(
        plan["modules"]["portal"]["activity"]["unchanged"], 1,
        "{plan}"
    );
    assert!(plan["notRestored"]["portal/access_requests"].is_string());
    let still_lost: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entity_comments WHERE project = 'alfa-city'")
            .fetch_one(&db)
            .await
            .expect("count");
    assert_eq!(still_lost, 0, "a check writes nothing");

    let asked = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[0]), "request": true })),
    )
    .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text);
    let id = body(&asked)["request"]["id"].as_i64().expect("a request");
    let approve = format!("{DATA_DOOR}/restores/{id}/approve");
    let own = send(
        &world.state,
        person("ada"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[0]) })),
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);
    let foreign = send(
        &world.state,
        person("bob"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN, "{}", foreign.text);

    let approved = send(
        &world.state,
        person("cid"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text);
    assert_eq!(body(&approved)["state"], "applied", "{}", approved.text);
    assert_eq!(body(&approved)["plan"]["erasuresReplayed"], 1);

    let bodies: Vec<String> = sqlx::query_scalar("SELECT body FROM entity_comments ORDER BY body")
        .fetch_all(&db)
        .await
        .expect("comments");
    assert_eq!(
        bodies,
        ["alfa-kept-mark", "bravo-after-mark"],
        "the kept comment is back, the erased one stays gone, bravo's is untouched"
    );
    let again = send(
        &world.state,
        person("cid"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND, "{}", again.text);

    let overview = send(&world.state, person("ada"), "GET", DATA_DOOR, None).await;
    let requests = send(
        &world.state,
        person("ada"),
        "GET",
        &format!("{DATA_DOOR}/restores"),
        None,
    )
    .await;
    let audit: Vec<String> = sqlx::query_scalar("SELECT message FROM organization_backup_log")
        .fetch_all(&db)
        .await
        .expect("audit");
    let logged = String::from_utf8_lossy(&log.0.lock().expect("log")).into_owned();
    for id in &ids {
        let secret = secret_of(id);
        for (place, text) in [
            ("overview", &overview.text),
            ("requests", &requests.text),
            ("log lines", &logged),
            ("audit", &audit.join("\n")),
        ] {
            assert!(!text.contains(&secret), "the identity is in the {place}");
            // The prefix alone may name the format in a message; a key is the prefix and its body.
            assert!(
                !text.split("AGE-SECRET-KEY-1").skip(1).any(|rest| rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())),
                "an identity is in the {place}"
            );
        }
    }
    for action in ["restore-plan", "restore-request", "restore"] {
        assert!(
            body(&overview)["log"]
                .as_array()
                .is_some_and(|log| log.iter().any(|e| e["action"] == action)),
            "{action} is audited"
        );
    }
}

/// T-7208: one project's folder restores into a new project: a new id whose parent is the
/// source, the rows under the new name and new ids, the source's erasures applied again; the
/// source's rows stay as they were. A taken name, an id the archive does not hold and a project
/// deleted since the backup are refused.
#[tokio::test]
async fn a_project_folder_restores_into_a_new_project_and_leaves_the_source_untouched() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    as_project(
        &db,
        "alfa-city",
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ('alfa-city', 'air', 'urn:ngsi-ld:X:alfa-2', 'ada', 'Ada', 'alfa-kept-mark')",
    )
    .await;
    let world = world_with("alfa", "Enabled", Some(db.clone())).await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let version = run["versionId"].as_str().expect("a version").to_owned();
    // The person behind urn:ngsi-ld:X:alfa-1 is erased since the backup.
    as_project(
        &db,
        "alfa-city",
        "INSERT INTO erasures (id, project, space, subject, reason, actor, groups, stores, approval) \
         VALUES ('er-alfa-1', 'alfa-city', 'air', '0123456789abcdef0123456789abcdef', 'asked', 'ada', \
         '[{\"class\":\"X\",\"entities\":[\"urn:ngsi-ld:X:alfa-1\"],\"attrs\":null}]', '{}', 'approved')",
    )
    .await;
    let source_rows = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM entity_comments WHERE project = 'alfa-city'",
        )
        .fetch_one(&db)
        .await
        .expect("count")
    };
    let before = source_rows().await;
    let source_id = joinedcontext_portal::project_ids::recorded(&db, "alfa-city")
        .await
        .expect("registry")
        .expect("alfa-city's key")
        .project;
    // Doors, folders and the log name an id in its 26-character path form (TEN-01).
    let source = source_id.path_segment();
    let identity = secret_of(&ids[0]);

    let listed = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/versions/{version}/projects"),
        Some(json!({ "identity": identity })),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    assert_eq!(
        body(&listed)["projects"],
        json!([{ "id": source, "name": "alfa-city" }])
    );

    let door = format!("{DATA_DOOR}/versions/{version}/copy");
    let ask = |project: &str, name: &str, apply: bool| json!({ "identity": identity, "project": project, "name": name, "apply": apply });
    let check = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask(&source, "alfa-copy", false)),
    )
    .await;
    assert_eq!(check.status, StatusCode::OK, "{}", check.text);
    let plan = body(&check);
    assert_eq!(plan["tables"]["entity_comments"]["create"], 2, "{plan}");
    assert_eq!(plan["projectId"], "", "a check mints no id");
    let minted: i64 =
        sqlx::query_scalar("SELECT count(*) FROM project_ids WHERE name = 'alfa-copy'")
            .fetch_one(&db)
            .await
            .expect("count");
    assert_eq!(minted, 0, "a check writes nothing");

    let applied = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask(&source, "alfa-copy", true)),
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text);
    let plan = body(&applied);
    assert_eq!(plan["erasuresReplayed"], 1, "{plan}");
    let copy_id =
        jc_types::ProjectId::from_path_segment(plan["projectId"].as_str().expect("the new id"))
            .expect("the path form");
    assert_ne!(copy_id, source_id);
    let copy = copy_id.to_string();
    let (parent, organization): (Option<String>, String) =
        sqlx::query_as("SELECT parent::text, organization FROM project_ids WHERE id = $1::uuid")
            .bind(&copy)
            .fetch_one(&db)
            .await
            .expect("the copy's registry row");
    assert_eq!((parent, organization.as_str()), (None, "alfa"));
    // The copy is the live project of its name: every reader by name enters its key, and the
    // first request for a Project of that name gets no second id.
    let by_name = joinedcontext_portal::project_ids::recorded(&db, "alfa-copy")
        .await
        .expect("registry")
        .expect("the copy's key by its name");
    assert_eq!(by_name.project, copy_id);
    let ensured = joinedcontext_portal::project_ids::ensure(&db, &["alfa-copy".to_owned()])
        .await
        .expect("ensure");
    assert_eq!(ensured.get("alfa-copy"), Some(&copy_id));
    let keyed = joinedcontext_portal::project_ids::key(&db, "alfa", "alfa-copy")
        .await
        .expect("key")
        .expect("the copy's key in its Organization");
    assert_eq!(keyed, by_name);
    let lineage: (String, String) =
        sqlx::query_as("SELECT source::text, version_id FROM project_copies WHERE id = $1::uuid")
            .bind(&copy)
            .fetch_one(&db)
            .await
            .expect("the lineage");
    assert_eq!(lineage, (source_id.to_string(), version.clone()));
    // The copy's rows answer in its own key only, under the new name, the erased one gone.
    let key = joinedcontext_portal::project_ids::recorded(&db, "alfa-city")
        .await
        .expect("registry")
        .expect("key");
    let copy_key = jc_types::ProjectKey::new(key.org, copy_id);
    let mut tx = joinedcontext_portal::db::tenant_tx(&db, copy_key)
        .await
        .expect("tx");
    let bodies: Vec<(String, String)> = sqlx::query_as(
        "SELECT project, body FROM entity_comments WHERE project_id = $1::uuid ORDER BY body",
    )
    .bind(&copy)
    .fetch_all(&mut *tx)
    .await
    .expect("the copy's comments");
    tx.rollback().await.expect("rollback");
    assert_eq!(
        bodies,
        vec![("alfa-copy".to_owned(), "alfa-kept-mark".to_owned())],
        "the erased person's comment stays erased in the copy"
    );
    assert_eq!(
        source_rows().await,
        before,
        "the source project is untouched"
    );

    let again = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask(&source, "alfa-copy", true)),
    )
    .await;
    assert_eq!(
        again.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        again.text
    );
    assert!(again.text.contains("name:"), "{}", again.text);
    let unknown = jc_types::ProjectId::mint().path_segment();
    let foreign = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask(&unknown, "alfa-other", false)),
    )
    .await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text);

    // alfa-city deleted since the backup: its id is retired and its folder never restored.
    world
        .state
        .mirror
        .remove(&joinedcontext_portal::resource::ResourceKey {
            namespace: ORG_NAMESPACE.to_owned(),
            kind: "Project".to_owned(),
            name: "alfa-city".to_owned(),
        });
    let gone = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask(&source, "alfa-other", false)),
    )
    .await;
    assert_eq!(
        gone.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        gone.text
    );
    assert!(
        gone.text.contains("project:") && gone.text.contains("deleted"),
        "{}",
        gone.text
    );
    let listed = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/versions/{version}/projects"),
        Some(json!({ "identity": identity })),
    )
    .await;
    assert_eq!(
        body(&listed),
        json!({ "projects": [], "deleted": ["alfa-city"] })
    );
}

/// A copy rewrites each row's id that names another copied row (a notice's subscription, a
/// notification's comment) to the copy's id of that row, mints a new text id where a row is
/// addressed by one, and refuses a row that would point at a row it does not hold; a name that
/// is no DNS label is refused naming the field.
#[tokio::test]
async fn a_project_copys_rows_point_at_each_other_and_never_at_the_source() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    as_project(
        &db,
        "bravo-town",
        "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
         VALUES ('bob', 'bravo-town', 'pipeline', 'bravo-feed', '{failure}', 'portal')",
    )
    .await;
    let foreign: i64 =
        sqlx::query_scalar("SELECT id FROM alert_subscriptions WHERE project = 'bravo-town'")
            .fetch_one(&db)
            .await
            .expect("bravo's subscription");
    for sql in [
        "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
         VALUES ('ada', 'alfa-city', 'pipeline', 'alfa-feed', '{failure}', 'portal')"
            .to_owned(),
        "INSERT INTO alert_notices (recipient, subscription, project, pipeline, event, change) \
         SELECT 'ada', id, 'alfa-city', 'alfa-feed', 'failure', 'opened' FROM alert_subscriptions \
         WHERE target = 'alfa-feed'"
            .to_owned(),
        format!(
            "INSERT INTO alert_notices (recipient, subscription, project, pipeline, event, change) \
             VALUES ('ada', {foreign}, 'alfa-city', 'bravo-feed', 'failure', 'opened')"
        ),
        "INSERT INTO notifications (recipient, project, space, urn, comment_id, author, author_name, excerpt) \
         SELECT 'mia', 'alfa-city', 'air', urn, id, 'ada', 'Ada', 'see this' FROM entity_comments \
         WHERE body = 'alfa-comment-mark'"
            .to_owned(),
        "INSERT INTO grafana_visualizations (id, project, title, endpoint, entity_type, attribute, kind) \
         VALUES ('viz-alfa-1', 'alfa-city', 'Air', 'air', 'X', 'pm10', 'stat')"
            .to_owned(),
        "INSERT INTO data_views (id, project, space, entity_type, kind, mode, title, owner_subject, owner_name, config) \
         VALUES ('view-alfa-1', 'alfa-city', 'air', 'X', 'table', 'private', 'Air table', 'sub-ada', 'Ada', '{}')"
            .to_owned(),
    ] {
        as_project(&db, "alfa-city", &sql).await;
    }
    let world = world_with("alfa", "Enabled", Some(db.clone())).await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let version = run["versionId"].as_str().expect("a version").to_owned();
    let source = joinedcontext_portal::project_ids::recorded(&db, "alfa-city")
        .await
        .expect("registry")
        .expect("alfa-city's key");
    let door = format!("{DATA_DOOR}/versions/{version}/copy");
    let ask = |name: &str| json!({ "identity": secret_of(&ids[0]), "project": source.project.path_segment(), "name": name, "apply": true });
    let refused = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask("Alfa_Twin")),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("name: use lower-case letters"),
        "{}",
        refused.text
    );
    let taken = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask("bravo")),
    )
    .await;
    assert_eq!(
        taken.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        taken.text
    );
    assert!(taken.text.contains("is taken"), "{}", taken.text);

    let applied = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(ask("alfa-twin")),
    )
    .await;
    assert_eq!(applied.status, StatusCode::OK, "{}", applied.text);
    let plan = body(&applied);
    let tables = &plan["tables"];
    assert_eq!(
        tables["alert_subscriptions"],
        json!({ "create": 1, "refused": 0 }),
        "{plan}"
    );
    assert_eq!(
        tables["alert_notices"],
        json!({ "create": 1, "refused": 1 }),
        "the notice of bravo's subscription is refused: {plan}"
    );
    assert_eq!(tables["notifications"]["create"], 1, "{plan}");
    assert_eq!(tables["grafana_visualizations"]["create"], 1, "{plan}");
    assert_eq!(tables["data_views"]["create"], 1, "{plan}");

    let twin = jc_types::ProjectId::from_path_segment(plan["projectId"].as_str().expect("id"))
        .expect("path form");
    let mut tx =
        joinedcontext_portal::db::tenant_tx(&db, jc_types::ProjectKey::new(source.org, twin))
            .await
            .expect("the copy's key");
    let pointed: (i64, i64) = sqlx::query_as(
        "SELECT n.subscription, s.id FROM alert_notices n JOIN alert_subscriptions s \
         ON s.id = n.subscription WHERE n.project = 'alfa-twin' AND s.project = 'alfa-twin'",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("the copy's notice points at the copy's subscription");
    assert_eq!(pointed.0, pointed.1);
    let comment: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications n JOIN entity_comments c ON c.id = n.comment_id \
         WHERE n.project = 'alfa-twin' AND c.project = 'alfa-twin')",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("read");
    assert!(comment, "the copy's notification names the copy's comment");
    let viz: String =
        sqlx::query_scalar("SELECT id FROM grafana_visualizations WHERE project = 'alfa-twin'")
            .fetch_one(&mut *tx)
            .await
            .expect("the copy's visualization");
    assert_ne!(viz, "viz-alfa-1", "a text id is minted anew");
    let view: String = sqlx::query_scalar("SELECT id FROM data_views WHERE project = 'alfa-twin'")
        .fetch_one(&mut *tx)
        .await
        .expect("the copy's data view");
    assert_ne!(view, "view-alfa-1", "a text id is minted anew");
    tx.rollback().await.expect("rollback");
}

/// T-7208: the copy doors answer the Organization's administrators only.
#[tokio::test]
async fn only_administrators_reach_the_project_copy_doors() {
    let world = world("alfa", "Enabled").await;
    for door in ["projects", "copy"] {
        let refused = send(
            &world.state,
            person("bob"),
            "POST",
            &format!("{DATA_DOOR}/versions/v1/{door}"),
            Some(if door == "copy" {
                json!({ "identity": "AGE-SECRET-KEY-1X", "project": "p", "name": "n" })
            } else {
                json!({ "identity": "AGE-SECRET-KEY-1X" })
            }),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{door}: {}",
            refused.text
        );
    }
}

#[tokio::test]
async fn a_restore_request_holds_its_target_and_bytes_and_writes_tables_without_an_id() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    for statement in [
        "INSERT INTO drafts (project, kind, name, manifest, touched_by, touched_kind) \
         VALUES ('alfa-city', 'Pipeline', 'alfa-draft', '{}', 'ada', 'user')",
        "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
         VALUES ('sub-ada', 'alfa-city', 'pipeline', 'air', '{failed}', 'portal')",
    ] {
        as_project(&db, "alfa-city", statement).await;
    }
    let world = world_with("alfa", "Enabled", Some(db.clone())).await;
    world.state.mirror.upsert(org(
        "RoleBinding",
        "cid-admin",
        json!({ "subjects": [{ "user": "cid@hel.fi" }], "role": "org-admin", "scope": { "organization": "alfa" } }),
    ));
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    let version = run["versionId"].as_str().expect("a version").to_owned();

    // Lost since: the draft (a table keyed without an `id`), and a subscription made again under
    // a new id with the same subject, project, scope and target.
    for statement in [
        "DELETE FROM drafts WHERE project = 'alfa-city'",
        "DELETE FROM alert_subscriptions WHERE project = 'alfa-city'",
        "INSERT INTO alert_subscriptions (subject, project, scope, target, events, delivery) \
         VALUES ('sub-ada', 'alfa-city', 'pipeline', 'air', '{failed,stale}', 'digest')",
    ] {
        as_project(&db, "alfa-city", statement).await;
    }
    let door = format!("{DATA_DOOR}/versions/{version}/restore");
    let asked = send(
        &world.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[0]), "request": true })),
    )
    .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text);
    let plan = &body(&asked)["plan"]["modules"]["portal"];
    assert_eq!(plan["drafts"]["create"], 1, "{plan}");
    assert_eq!(plan["alert_subscriptions"]["create"], 0, "{plan}");
    assert_eq!(plan["alert_subscriptions"]["refused"], 1, "{plan}");
    let id = body(&asked)["request"]["id"].as_i64().expect("a request");
    let approve = format!("{DATA_DOOR}/restores/{id}/approve");

    // While it waits, the bucket and the keys it was checked on stay.
    let repointed = send(
        &world.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&world.endpoint, &identities())),
    )
    .await;
    assert_eq!(repointed.status, StatusCode::CONFLICT, "{}", repointed.text);
    assert!(repointed.text.contains("pending"), "{}", repointed.text);

    // The asker under another username is still the asker.
    let mut renamed = person("cid");
    renamed.subject = "sub-ada".into();
    let own = send(
        &world.state,
        renamed,
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(own.status, StatusCode::FORBIDDEN, "{}", own.text);

    // Other bytes under the same version id (a bucket another admin points at) are refused.
    let second = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DATA_DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.text);
    let data_key = key_of(&world, "alfa", "data").await;
    let swap = |bucket: &FakeS3, from: &str, to: &str| {
        let mut bucket = bucket.lock().expect("bucket");
        let versions = bucket.objects.get_mut(&data_key).expect("archive");
        let bytes = versions
            .iter()
            .find(|v| v.0 == from)
            .and_then(|v| v.1.clone())
            .expect("bytes");
        let slot = versions.iter_mut().find(|v| v.0 == to).expect("version");
        slot.1.replace(bytes)
    };
    let newer = body(&second)["versionId"]
        .as_str()
        .expect("a version")
        .to_owned();
    let original = swap(&world.bucket, &newer, &version);
    let checked = send(
        &world.state,
        person("cid"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(
        checked.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a check reads only the bytes the run uploaded: {}",
        checked.text
    );
    let swapped = send(
        &world.state,
        person("cid"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(
        swapped.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        swapped.text
    );
    assert!(swapped.text.contains("version"), "{}", swapped.text);
    world
        .bucket
        .lock()
        .expect("bucket")
        .objects
        .get_mut(&data_key)
        .and_then(|v| v.iter_mut().find(|v| v.0 == version))
        .expect("version")
        .1 = original;

    let approved = send(
        &world.state,
        person("cid"),
        "POST",
        &approve,
        Some(json!({ "identity": secret_of(&ids[1]) })),
    )
    .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.text);
    assert_eq!(body(&approved)["state"], "applied", "{}", approved.text);
    let drafts: Vec<String> =
        sqlx::query_scalar("SELECT name FROM drafts WHERE project = 'alfa-city'")
            .fetch_all(&db)
            .await
            .expect("drafts");
    assert_eq!(drafts, ["alfa-draft"], "the draft is back");
    let deliveries: Vec<String> =
        sqlx::query_scalar("SELECT delivery FROM alert_subscriptions WHERE project = 'alfa-city'")
            .fetch_all(&db)
            .await
            .expect("subscriptions");
    assert_eq!(deliveries, ["digest"], "today's subscription stands");
    let changed = send(
        &world.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&world.endpoint, &ids)),
    )
    .await;
    assert_eq!(
        changed.status,
        StatusCode::OK,
        "decided: the target is free again"
    );
}

#[tokio::test]
async fn an_archive_of_another_organization_is_not_restored_into_it() {
    let Some(db) = seeded_portal_db().await else {
        return;
    };
    let alfa = world_with("alfa", "Enabled", Some(db.clone())).await;
    let ids = identities();
    let run = data_saved_and_run(&alfa, &ids).await;
    let version = run["versionId"].as_str().expect("a version").to_owned();

    // bravo's administrator, on the same database, aims at the bucket holding alfa's archive.
    let bravo = world_with("bravo", "Enabled", Some(db)).await;
    let saved = send(
        &bravo.state,
        person("ada"),
        "PUT",
        DATA_DOOR,
        Some(data_target(&alfa.endpoint, &ids)),
    )
    .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text);
    let door = format!("{DATA_DOOR}/versions/{version}/restore");
    let refused = send(
        &bravo.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[0]) })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("not a backup of this Organization"),
        "{}",
        refused.text
    );

    let alfa_key = key_of(&alfa, "alfa", "data").await;
    let bravo_key = key_of(&bravo, "bravo", "data").await;
    // Even copied under bravo's key, with a run of bravo's that names the version, the archive
    // says whose it is and is refused.
    let copied_bytes = {
        let mut bucket = alfa.bucket.lock().expect("bucket");
        let copied = bucket.objects[&alfa_key].clone();
        let bytes = copied
            .iter()
            .find(|v| v.0 == version)
            .and_then(|v| v.1.clone())
            .expect("the version's bytes");
        bucket.objects.insert(bravo_key.clone(), copied);
        bytes
    };
    bravo
        .state
        .org_backups
        .log(joinedcontext_portal::org_backup::NewEntry {
            organization: "bravo",
            kind: "data",
            action: "run",
            actor: "ada",
            ok: true,
            bytes: 1,
            object_key: &bravo_key,
            version_id: &version,
            message: "uploaded",
            sha256: &joinedcontext_portal::org_backup::sha256_hex(&copied_bytes),
        })
        .await
        .expect("logged");
    let refused = send(
        &bravo.state,
        person("ada"),
        "POST",
        &door,
        Some(json!({ "identity": secret_of(&ids[0]), "request": true })),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("another Organization"),
        "{}",
        refused.text
    );
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
async fn a_bundle_from_before_the_project_key_is_read_until_its_retention_ends() {
    let world = world("alfa", "Enabled").await;
    let run = saved_and_run(&world, "ada").await;
    let version = run["versionId"].as_str().expect("version").to_owned();
    let key = key_of(&world, "alfa", "configuration").await;
    // Built from ids only (ARCH/STO-14): no Organization or project name in the path.
    assert!(
        key.starts_with("jc/org/") && key.ends_with("/backup/configuration/bundle.zip"),
        "{key}"
    );
    assert!(!key.contains("alfa"), "{key}");
    assert_eq!(run["objectKey"], key.as_str(), "{run}");

    // The bucket holds alfa's bundle from before T-7207 under its name, bravo's from then, and
    // another Organization's under its key.
    let bundle = latest(&world.bucket, &key);
    let old = |at: &str| vec![("old".to_owned(), Some(bundle.clone()), at.to_owned())];
    let other = key.replacen("jc/org/", "jc/org/0", 1);
    {
        let mut bucket = world.bucket.lock().expect("bucket");
        for planted in [
            "jc/alfa/configuration/bundle.zip",
            "jc/bravo/configuration/bundle.zip",
        ] {
            bucket
                .objects
                .insert(planted.into(), old("2020-01-01T00:00:00.000Z"));
        }
        bucket
            .objects
            .insert(other.clone(), old("2026-11-16T11:00:00.000Z"));
    }
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
    let ids: Vec<&str> = listed["versions"]
        .as_array()
        .expect("versions")
        .iter()
        .filter_map(|v| v["versionId"].as_str())
        .collect();
    assert_eq!(ids, [version.as_str(), "old"], "{listed}");

    // The old version still downloads, at the key it was written under (ARCH/TEN-15).
    let read = send(
        &world.state,
        person("ada"),
        "GET",
        &format!("{DOOR}/versions/old"),
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert_eq!(read.text, String::from_utf8_lossy(&bundle));

    // The next run removes it, written more than keepDays (90) ago, and nobody else's.
    let again = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/run"),
        None,
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text);
    {
        let bucket = world.bucket.lock().expect("bucket");
        assert!(bucket.objects["jc/alfa/configuration/bundle.zip"].is_empty());
        assert_eq!(bucket.objects["jc/bravo/configuration/bundle.zip"].len(), 1);
        assert_eq!(bucket.objects[&other].len(), 1);
    }
    let gone = send(
        &world.state,
        person("ada"),
        "GET",
        &format!("{DOOR}/versions/old"),
        None,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text);
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
        Some(json!({ "identity": identity() })),
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
    let key = key_of(&world, "alfa", "configuration").await;
    let mut files = unzip(&opened(&latest(&world.bucket, &key)));
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
        .get_mut(&key)
        .expect("alfa's bundle")
        .push((
            "crafted".into(),
            Some(sealed(&crafted)),
            "2026-11-16T11:00:00.000Z".into(),
        ));

    let refused = send(
        &world.state,
        person("ada"),
        "POST",
        &format!("{DOOR}/versions/crafted/restore"),
        Some(json!({ "identity": identity() })),
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
    let alfa_bundle = latest(&alfa.bucket, &key_of(&alfa, "alfa", "configuration").await);

    // bravo's bucket holds alfa's bundle under bravo's key.
    let bravo = world("bravo", "Enabled").await;
    saved_and_run(&bravo, "ada").await;
    let bravo_key = key_of(&bravo, "bravo", "configuration").await;
    let version = {
        let mut bucket = bravo.bucket.lock().expect("bucket");
        let versions = bucket.objects.get_mut(&bravo_key).expect("bravo's bundle");
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
        Some(json!({ "identity": identity(), "apply": true })),
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
