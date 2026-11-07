//! One Helsinki server WASM App on the real host (T-3346..T-3350): its component built from
//! `apps/<name>/server` for wasm32-wasip2, placed twice on one shard of a fresh database (so a
//! test can show the second App sees nothing of the first), its migrations run as the reconciler
//! runs them, as the App's owner and never as the App, and each run twice to show a re-run changes
//! nothing. Its files go to RustFS under the shard's own key; the gateway is a mock, and so is the
//! Portal the host asks who a token's person is and which of the App's roles they hold
//! ([`token`], T-3826).
//!
//! ```text
//! JC_WASM_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres   # a superuser
//! JC_WASM_TEST_S3=http://127.0.0.1:19000 JC_WASM_TEST_S3_KEY=… JC_WASM_TEST_S3_SECRET=…   # RustFS root
//! ```
//!
//! A missing variable is a failure that says what to set, never a test that passes by skipping.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use time::OffsetDateTime;
use wasm_host::blob::S3Blob;
use wasm_host::host::Host;
use wasm_host::limits::Limits;
use wasm_host::placement::Placed;
use wasm_host::provision;
use wasm_host::s3::Bucket;
use wasm_host::source::Source;
use wasm_host::sql::{PgStore, SqlLimits};
use wasm_host::storage::Stores;
use wiremock::MockServer;

pub fn var(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("set {name} (see the top of tests/wasm-apps/src/lib.rs)"))
}

fn unique() -> String {
    format!(
        "{}{}",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos() % 1_000_000_000
    )
}

fn app_dir(app: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps")
        .join(app)
}

/// The App's component, built as the build lane builds it (AP-151), minus `--offline`.
pub fn build(app: &str) -> Vec<u8> {
    let server = app_dir(app).join("server");
    let manifest = std::fs::read_to_string(server.join("Cargo.toml")).expect("server/Cargo.toml");
    let name = manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix("name = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("a package name")
        .replace('-', "_");
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
        .join("wasm-apps");
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--release",
            "--locked",
            "--target",
            "wasm32-wasip2",
            "--manifest-path",
        ])
        .arg(server.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("cargo runs");
    assert!(status.success(), "{app}'s server builds for wasm32-wasip2");
    std::fs::read(target.join(format!("wasm32-wasip2/release/{name}.wasm"))).expect("the component")
}

/// The slug of the App's own Endpoint, as its committed grants carry it
/// (`grants/projects/*/spaces/*/endpoints/app-<name>.yaml`): what the reconciler places it with,
/// and the one Endpoint the host lets it call (AP-147, AP-157).
pub fn slug(app: &str) -> String {
    let projects = app_dir(app).join("grants/projects");
    let file = format!("app-{app}.yaml");
    let found = std::fs::read_dir(&projects)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|project| {
            std::fs::read_dir(project.path().join("spaces"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|space| space.path().join("endpoints").join(&file))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{app}: no grants/projects/*/spaces/*/endpoints/{file}"));
    std::fs::read_to_string(&found)
        .expect("the Endpoint")
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("slug:")
                .map(|s| s.trim().trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| panic!("{}: no slug", found.display()))
}

/// The App's migrations, in the order of their names (AP-149).
pub fn migrations(app: &str) -> Vec<(String, String)> {
    let dir = app_dir(app).join("migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("migrations/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("a migration");
            (
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                text,
            )
        })
        .collect()
}

/// The roles `app.yaml` declares, in its order (`spec.roles[].name`).
pub fn roles(app: &str) -> Vec<String> {
    let manifest = std::fs::read_to_string(app_dir(app).join("app.yaml")).expect("app.yaml");
    let mut roles = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        if line == "  roles:" {
            inside = true;
        } else if inside && line.starts_with("  ") && !line.starts_with("   ") {
            break;
        } else if let Some(name) = line.strip_prefix("    - name:").filter(|_| inside) {
            roles.push(name.trim().trim_matches('"').to_owned());
        }
    }
    roles
}

/// The token of a caller as a scenario names it: `anonymous` has none; `steward`,
/// `steward+viewer` or `viewer@u-2` is person `u-<first role>` (or the one after `@`) holding
/// those roles. The stand-in Portal reads the token back, so no test shares a table of people.
pub fn token(caller: &str) -> Option<String> {
    if caller == "anonymous" {
        return None;
    }
    let (roles, person) = caller.split_once('@').unwrap_or((caller, ""));
    let first = roles.split('+').next().unwrap_or_default();
    let person = if person.is_empty() {
        format!("u-{first}")
    } else {
        person.to_owned()
    };
    Some(format!("tok-{person}~{}", roles.replace('+', ",")))
}

/// The Portal's `GET /api/v1/projects/{project}/apps/{name}/me` for every token of [`token`]:
/// the person and roles it names, `401` for any other.
async fn portal() -> MockServer {
    let portal = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/projects/helsinki/apps/[a-z0-9-]+/me$",
        ))
        .respond_with(|request: &wiremock::Request| {
            let named = request
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer tok-"))
                .and_then(|v| v.split_once('~'));
            match named {
                Some((person, roles)) => {
                    wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "id": person,
                        "name": format!("Person {person}"),
                        "roles": roles.split(',').filter(|r| !r.is_empty()).collect::<Vec<_>>(),
                    }))
                }
                None => wiremock::ResponseTemplate::new(401),
            }
        })
        .mount(&portal)
        .await;
    portal
}

/// The App, twice, on one shard, with its own schema and files, and the gateway it reads.
pub struct Harness {
    pub host: Arc<Host>,
    /// The App's own Endpoint slug: a mock gateway answers below `/api/endpoint/<slug>/`.
    pub slug: String,
    pub first: Placed,
    pub second: Placed,
    pub gateway: MockServer,
    /// Who each token is, for the host's `identity::caller()`.
    pub portal: MockServer,
    database: String,
}

impl Harness {
    pub async fn new(app: &str, gateway: MockServer) -> Self {
        let suffix = unique();
        let shard = format!("s1_{suffix}");
        let admin = var("JC_WASM_TEST_DATABASE_URL");
        let name = format!("wasmapps_{suffix}");
        let mut conn = PgConnection::connect(&admin)
            .await
            .expect("the test server");
        sqlx::query(AssertSqlSafe(format!("create database {name}")))
            .execute(&mut conn)
            .await
            .expect("create database");
        let url = admin
            .rsplit_once('/')
            .map(|(base, _)| format!("{base}/{name}"))
            .expect("a URL with a database");
        let mut db = PgConnection::connect(&url).await.expect("the new database");
        let mut statements = provision::database(&name);
        statements.extend(provision::shard(&shard).expect("shard"));
        statements.push(format!(
            "alter role wasm_host_{shard} password 'pw-{shard}'"
        ));
        let ids = [format!("one_{suffix}"), format!("two_{suffix}")];
        for id in &ids {
            statements.extend(provision::app(&shard, id, "postgres").expect("app"));
        }
        for statement in statements {
            sqlx::raw_sql(AssertSqlSafe(statement.clone()))
                .execute(&mut db)
                .await
                .unwrap_or_else(|err| panic!("{statement}: {err}"));
        }
        // Each file as the App's owner in the App's schema, twice: a re-run must change nothing.
        for id in &ids {
            for round in 0..2 {
                for (file, sql) in migrations(app) {
                    let mut tx = db.begin().await.expect("a transaction");
                    for statement in [
                        format!("set local role {}", provision::owner_of(id)),
                        format!("set local search_path = app_{id}"),
                    ] {
                        sqlx::raw_sql(AssertSqlSafe(statement))
                            .execute(&mut *tx)
                            .await
                            .expect("as the owner");
                    }
                    sqlx::raw_sql(AssertSqlSafe(sql))
                        .execute(&mut *tx)
                        .await
                        .unwrap_or_else(|err| panic!("{file} (run {}): {err}", round + 1));
                    tx.commit().await.expect("commit");
                }
            }
        }

        let login = {
            let rest = url.split_once("://").map(|(_, rest)| rest).expect("url");
            let host_and_db = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
            format!("postgres://wasm_host_{shard}:pw-{shard}@{host_and_db}")
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&login)
            .await
            .expect("the shard logs in");
        let blob_shard = format!("s1{suffix}");
        let bucket = store(&blob_shard).await;
        let stores = Stores {
            sql: Some(PgStore::with_pool(pool, SqlLimits::default())),
            blob: Some(S3Blob::new(bucket, &blob_shard, 64 << 20)),
        };

        let bytes = build(app);
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        let dir = std::env::temp_dir().join(format!("wasm-apps-{suffix}"));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(format!("sha256-{}.wasm", &digest[7..])), &bytes)
            .expect("component");
        let portal = portal().await;
        let host = Host::new(
            Limits::default(),
            Source::Dir(dir),
            Arc::new(stores),
            Some(&gateway.uri()),
            Some(&portal.uri()),
        )
        .expect("host");
        let slug = slug(app);
        let placed = |id: &str| Placed {
            name: app.to_owned(),
            id: id.to_owned(),
            tenant: "helsinki".into(),
            digest: digest.clone(),
            endpoint: Some(slug.clone()),
            jobs: Vec::new(),
        };
        Self {
            host,
            slug: slug.clone(),
            first: placed(&ids[0]),
            second: placed(&ids[1]),
            gateway,
            portal,
            database: url,
        }
    }

    /// One request to `app`, as a signed-in caller with `token`; the answer's status and JSON.
    pub async fn call(
        &self,
        app: &Placed,
        method: &str,
        path: &str,
        body: &str,
        token: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://apps.test/apps/{}{path}", app.name))
            .header("content-type", "application/json")
            .body(Bytes::from(body.to_owned()))
            .expect("request");
        let response = self
            .host
            .serve(app, request, token.map(str::to_owned))
            .await
            .unwrap_or_else(|failure| panic!("{method} {path}: {failure:?}"));
        let status = response.status().as_u16();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map(|b| b.to_bytes())
            .unwrap_or_default();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// A count read straight from the App's schema, as the database's superuser.
    pub async fn count(&self, app: &Placed, table: &str) -> i64 {
        let mut db = PgConnection::connect(&self.database).await.expect("db");
        sqlx::query_scalar(AssertSqlSafe(format!(
            "select count(*) from app_{}.{table}",
            app.id
        )))
        .fetch_one(&mut db)
        .await
        .expect("count")
    }
}

/// What a presigned URL serves.
pub async fn fetch(url: &str) -> (u16, String) {
    let got = reqwest::get(url).await.expect("the store answers");
    (got.status().as_u16(), got.text().await.unwrap_or_default())
}

/// RustFS's `apps` bucket and a key for `shard` whose policy reaches `apps/<shard>/*` alone, as the
/// deployment provisions it.
async fn store(shard: &str) -> Bucket {
    let root = Bucket {
        endpoint: var("JC_WASM_TEST_S3"),
        bucket: "apps".into(),
        region: "us-east-1".into(),
        key_id: var("JC_WASM_TEST_S3_KEY"),
        secret: var("JC_WASM_TEST_S3_SECRET"),
        public_endpoint: None,
        http: reqwest::Client::new(),
    };
    let (status, _) = Bucket {
        bucket: String::new(),
        ..root.clone()
    }
    .object(reqwest::Method::PUT, "apps", None)
    .await
    .expect("bucket");
    assert!(status == 200 || status == 409, "the bucket: {status}");
    let policy = serde_json::json!({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"], "Resource": [format!("arn:aws:s3:::apps/apps/{shard}/*")]},
        {"Effect": "Allow", "Action": ["s3:ListBucket"], "Resource": ["arn:aws:s3:::apps"], "Condition": {"StringLike": {"s3:prefix": [format!("apps/{shard}/*")]}}}
    ]});
    admin(
        &root,
        "PUT",
        "/rustfs/admin/v3/add-user",
        &format!("accessKey=key{shard}"),
        &serde_json::json!({"secretKey": format!("secret-{shard}"), "status": "enabled"}),
    )
    .await;
    admin(
        &root,
        "PUT",
        "/rustfs/admin/v3/add-canned-policy",
        &format!("name=p{shard}"),
        &policy,
    )
    .await;
    admin(
        &root,
        "POST",
        "/rustfs/admin/v3/idp/builtin/policy/attach",
        "",
        &serde_json::json!({"policies": [format!("p{shard}")], "user": format!("key{shard}")}),
    )
    .await;
    Bucket {
        key_id: format!("key{shard}"),
        secret: format!("secret-{shard}"),
        ..root
    }
}

async fn admin(root: &Bucket, method: &str, path: &str, query: &str, body: &serde_json::Value) {
    let body = body.to_string();
    let sha = hex::encode(Sha256::digest(body.as_bytes()));
    let (authorization, amz_date) =
        root.authorization(method, path, query, &[], &sha, OffsetDateTime::now_utc());
    let url = format!(
        "{}{path}{}{query}",
        root.endpoint,
        if query.is_empty() { "" } else { "?" }
    );
    let response = root
        .http
        .request(method.parse().expect("method"), url)
        .header("authorization", authorization)
        .header("x-amz-date", amz_date)
        .header("x-amz-content-sha256", sha)
        .body(body)
        .send()
        .await
        .expect("admin");
    assert!(
        response.status().is_success(),
        "{path}: {} {}",
        response.status(),
        response.text().await.unwrap_or_default()
    );
}
