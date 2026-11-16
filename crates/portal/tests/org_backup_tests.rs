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
    world_with(installation, versioning, None).await
}

async fn world_with(installation: &str, versioning: &str, db: Option<sqlx::PgPool>) -> World {
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

const DATA_DOOR: &str = "/api/v1/organization/backups/data";
const DATA_KEY: &str = "jc/alfa/data/archive.zip.age";

/// Rows of both Organizations in the Portal's tables, each with a mark the archive is searched
/// for, and secret material in the columns that must never leave.
const PORTAL_SEED: &str = "
INSERT INTO activity (time, project, kind, source, summary, severity) VALUES (now(), 'alfa-city', 'pipeline', 'portal', 'alfa-activity-mark', 'info');
INSERT INTO entity_comments (project, space, urn, author, author_name, body) VALUES ('alfa-city', 'air', 'urn:ngsi-ld:X:alfa-1', 'ada', 'Ada', 'alfa-comment-mark');
INSERT INTO agent_runs (id, project, app_name, endpoint_name, endpoint_slug, profile, app_class, visibility, prompt, prompt_digest, data_needs, branch, path_prefix, status, ticket_hash, created_by, expires_at) VALUES ('run-alfa-1', 'alfa-city', 'a', 'e', 's', 'p', 'static', 'private', 'alfa-prompt-mark', 'd', '[]', 'b', 'x/', 'queued', 'ALFA_TICKET_HASH_SECRET', 'ada', now());
INSERT INTO agent_run_events (run_id, seq, kind, payload) VALUES ('run-alfa-1', 1, 'step', '{}');
INSERT INTO app_builds (project, app, commit, class, trigger, state, token_sha256) VALUES ('alfa-city', 'app', 'c', 'node', 'push', 'pending', 'ALFA_BUILD_TOKEN_SECRET');
INSERT INTO workspaces (name, project, owner, base_revision, scope, expires_at, preview_salt) VALUES ('ws-alfa', 'alfa-city', 'ada', 'r', '{}', now(), 'ALFA_PREVIEW_SALT_SECRET');
INSERT INTO domain_verifications (organization, domain, challenge, state) VALUES ('alfa', 'alfa.example', 'ALFA_DNS_CHALLENGE_SECRET', 'pending');
";

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
    for statement in PORTAL_SEED.split(";\n").chain(bravo.split(";\n")) {
        if !statement.trim().is_empty() {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .expect(statement);
        }
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
    let world = world_with("alfa", "Enabled", Some(db)).await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let sealed = latest(&world.bucket, DATA_KEY);
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
    assert!(files["module/portal/agent_run_events.jsonl"].contains("run-alfa-1"));
    let index: Value = serde_norway::from_str(&files["index.yaml"]).expect("index");
    assert_eq!(index["formatVersion"], 1);
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
async fn a_data_archive_without_a_database_says_what_it_left_out() {
    let world = world("alfa", "Enabled").await;
    let ids = identities();
    let run = data_saved_and_run(&world, &ids).await;
    assert_eq!(run["ok"], true, "{run}");
    let files = unzip(&decrypt(&latest(&world.bucket, DATA_KEY), &ids[1]));
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
    on_configuration["recoveryRecipient"] = good["recoveryRecipient"].clone();
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
    sqlx::query(
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ('alfa-city', 'air', 'urn:ngsi-ld:X:alfa-2', 'ada', 'Ada', 'alfa-kept-mark')",
    )
    .execute(&db)
    .await
    .expect("a second comment");
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
    sqlx::query("DELETE FROM entity_comments WHERE project IN ('alfa-city', 'bravo-town')")
        .execute(&db)
        .await
        .expect("lost");
    sqlx::query(
        "INSERT INTO entity_comments (project, space, urn, author, author_name, body) \
         VALUES ('bravo-town', 'air', 'urn:ngsi-ld:X:bravo-9', 'bob', 'Bob', 'bravo-after-mark')",
    )
    .execute(&db)
    .await
    .expect("bravo writes on");
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
        version_ids(&world.bucket, DATA_KEY).len(),
        2,
        "the version stays behind its marker"
    );
    // The person behind urn:ngsi-ld:X:alfa-1 is erased since the backup.
    sqlx::query(
        "INSERT INTO erasures (id, project, space, subject, reason, actor, groups, stores, approval) \
         VALUES ('er-alfa-1', 'alfa-city', 'air', '0123456789abcdef0123456789abcdef', 'asked', 'ada', \
         '[{\"class\":\"X\",\"entities\":[\"urn:ngsi-ld:X:alfa-1\"],\"attrs\":null}]', '{}', 'approved')",
    )
    .execute(&db)
    .await
    .expect("an erasure");

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
        sqlx::query(statement).execute(&db).await.expect(statement);
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
        sqlx::query(statement).execute(&db).await.expect(statement);
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
    let swap = |bucket: &FakeS3, from: &str, to: &str| {
        let mut bucket = bucket.lock().expect("bucket");
        let versions = bucket.objects.get_mut(DATA_KEY).expect("archive");
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
        .get_mut(DATA_KEY)
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

    // Even copied under bravo's key, with a run of bravo's that names the version, the archive
    // says whose it is and is refused.
    let copied_bytes = {
        let mut bucket = alfa.bucket.lock().expect("bucket");
        let copied = bucket.objects[DATA_KEY].clone();
        let bytes = copied
            .iter()
            .find(|v| v.0 == version)
            .and_then(|v| v.1.clone())
            .expect("the version's bytes");
        bucket
            .objects
            .insert("jc/bravo/data/archive.zip.age".into(), copied);
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
            object_key: "jc/bravo/data/archive.zip.age",
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
