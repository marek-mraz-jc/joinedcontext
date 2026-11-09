#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // Tests read their database URL and run the binaries they test; jc runs none of this.
//! The standalone configuration (R2, R8, R11): parsing, the tokens file, datasource secrets
//! from files, provisioning loaded at start, the Postgres store with its boot check, and the
//! per-address client identity the public rate limit needs.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use daiya_model::{CrError, OrgId, sha256_hex};
use daiya_query::{FileSecretResolver, SecretResolver};
use daiya_server::config::{
    CLI_ACTOR, DaiyaConfig, StoreConfig, build_state, cli_ctx, open_store, read_tokens_file,
    standalone_router,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("daiya-cfg-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(v: Value) -> Result<DaiyaConfig, serde_json::Error> {
    serde_json::from_value(v)
}

fn dash(uid: &str, ds_uid: &str) -> String {
    json!({
        "uid": uid,
        "title": format!("Dash {uid}"),
        "schemaVersion": 39,
        "panels": [{
            "id": 1, "type": "timeseries", "title": "Up",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": ds_uid },
            "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": ds_uid } }]
        }]
    })
    .to_string()
}

#[test]
fn configuration_defaults_and_refusals() {
    let cfg = config(json!({})).unwrap();
    assert_eq!(cfg.listen, "127.0.0.1:3000".parse::<SocketAddr>().unwrap());
    assert!(matches!(cfg.store, StoreConfig::Memory));
    assert_eq!(cfg.organization.id, OrgId::from_u128(1));
    assert_eq!(cfg.mount().unwrap().public_base(), "https://localhost");
    assert!(cfg.outbound_policy().unwrap().allowed_networks.is_empty());

    // Unknown keys anywhere are refused, so a typo never silently disables a setting.
    assert!(config(json!({ "tokenFile": "/x" })).is_err());
    assert!(config(json!({ "store": { "kind": "postgres", "url": "u", "passwordFile": "p", "pasword": 1 } })).is_err());
    assert!(config(json!({ "store": { "kind": "sqlite" } })).is_err());
    // A Postgres store without a pool size gets a small default pool, not one connection.
    let pg = config(
        json!({ "store": { "kind": "postgres", "url": "postgres://r@h/d", "passwordFile": "/p" } }),
    )
    .unwrap();
    assert!(
        matches!(
            pg.store,
            StoreConfig::Postgres {
                max_connections: 8,
                migrate: None,
                ..
            }
        ),
        "{:?}",
        pg.store
    );
    assert!(config(json!({ "outbound": { "allowHttp": true, "allowPrivate": true } })).is_err());

    let bad_cidr =
        config(json!({ "outbound": { "allowedNetworks": ["10.0.0.0/8", "nope"] } })).unwrap();
    let err = bad_cidr.outbound_policy().unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");
    let ok =
        config(json!({ "outbound": { "allowHttp": true, "allowedNetworks": [" 10.43.0.0/16 "] } }))
            .unwrap();
    let policy = ok.outbound_policy().unwrap();
    assert!(policy.allow_http);
    assert_eq!(policy.allowed_networks[0].to_string(), "10.43.0.0/16");

    let bad_mount = config(json!({ "mount": { "pathPrefix": "dash/../x" } })).unwrap();
    assert!(bad_mount.mount().is_err());
    let prefixed =
        config(json!({ "mount": { "scheme": "http", "host": "h:8080", "pathPrefix": "/daiya" } }))
            .unwrap();
    assert_eq!(
        prefixed.mount().unwrap().public_base(),
        "http://h:8080/daiya"
    );

    let scratch = Scratch::new("parse");
    let file = scratch.write("c.json", "{ \"listen\": \"0.0.0.0:9\" }");
    assert_eq!(DaiyaConfig::from_file(&file).unwrap().listen.port(), 9);
    let broken = scratch.write("b.json", "{ listen");
    assert_eq!(
        DaiyaConfig::from_file(&broken).unwrap_err().status_code(),
        400
    );
    assert!(DaiyaConfig::from_file(&scratch.0.join("absent.json")).is_err());
}

#[test]
fn the_tokens_file_holds_digests_only_and_reserves_the_cli_actor() {
    let scratch = Scratch::new("tokens");
    let digest = sha256_hex(b"t");
    let ok = scratch.write(
        "ok.json",
        &json!([{ "tokenSha256": digest, "actor": "ci", "role": "Editor", "folderUid": "ops", "readOnly": true }]).to_string(),
    );
    let entries = read_tokens_file(&ok).unwrap();
    assert_eq!(entries[0].actor, "ci");
    assert!(entries[0].read_only);
    for (name, body) in [
        (
            "raw",
            json!([{ "tokenSha256": "plain-token", "actor": "ci", "role": "Admin" }]),
        ),
        (
            "upper",
            json!([{ "tokenSha256": digest.to_uppercase(), "actor": "ci", "role": "Admin" }]),
        ),
        (
            "reserved",
            json!([{ "tokenSha256": digest, "actor": CLI_ACTOR, "role": "Admin" }]),
        ),
        (
            "empty",
            json!([{ "tokenSha256": digest, "actor": " ", "role": "Admin" }]),
        ),
        (
            "role",
            json!([{ "tokenSha256": digest, "actor": "ci", "role": "Owner" }]),
        ),
        (
            "token",
            json!([{ "token": "abc", "actor": "ci", "role": "Admin" }]),
        ),
    ] {
        let path = scratch.write(&format!("{name}.json"), &body.to_string());
        let err = read_tokens_file(&path).unwrap_err();
        assert_eq!(err.status_code(), 400, "{name}");
        assert!(!err.to_string().contains("plain-token") || name == "raw");
    }
    assert!(read_tokens_file(&scratch.0.join("absent.json")).is_err());
}

#[test]
fn file_secrets_are_per_organization_and_one_name_only() {
    let scratch = Scratch::new("secrets");
    let org_a = OrgId::from_u128(1);
    let org_b = OrgId::from_u128(2);
    scratch.write(&format!("{org_a}/prom-token"), "value-a\n");
    scratch.write(&format!("{org_a}/empty"), "\n");
    scratch.write("top-level", "outside any organization");
    let resolver = FileSecretResolver::new(scratch.0.clone());
    assert_eq!(
        resolver
            .resolve_secret(org_a, "prom-token")
            .unwrap()
            .as_str(),
        "value-a"
    );
    // Another organization cannot name it.
    assert!(resolver.resolve_secret(org_b, "prom-token").is_err());
    for bad in [
        "../top-level",
        "..",
        ".hidden",
        "a/b",
        "",
        "x y",
        &"a".repeat(201),
    ] {
        let err = resolver.resolve_secret(org_a, bad).unwrap_err();
        assert!(err.to_string().contains("secretRef"), "{bad}: {err}");
    }
    assert!(resolver.resolve_secret(org_a, "empty").is_err());
    assert!(resolver.resolve_secret(org_a, "missing").is_err());
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn build_state_loads_tokens_secrets_and_provisioning_into_a_serving_router() {
    let scratch = Scratch::new("build");
    let org = OrgId::from_u128(9);
    let token = format!("tok-{}", uuid::Uuid::new_v4());
    let tokens = scratch.write(
        "secrets/tokens.json",
        &json!([
            { "tokenSha256": sha256_hex(token.as_bytes()), "actor": "ops", "role": "Admin" },
            { "tokenSha256": sha256_hex(b"viewer"), "actor": "viewer", "role": "Viewer" }
        ])
        .to_string(),
    );
    scratch.write(&format!("secrets/ds/{org}/prom-token"), "bearer-from-file");
    scratch.write(
        "prov/datasources/ds.yaml",
        "apiVersion: 1\ndeleteDatasources:\n  - name: Old\n    orgId: 1\ndatasources:\n  - name: Prometheus\n    uid: prom\n    type: prometheus\n    url: https://prom.example.com\n    isDefault: true\n    secretRefs:\n      token: prom-token\n",
    );
    scratch.write(
        "prov/dashboards/providers.yaml",
        "apiVersion: 1\nproviders:\n  - name: platform\n    folder: Platform\n    allowUiUpdates: false\n    options:\n      path: boards/platform\n  - name: teams\n    options:\n      path: boards/teams\n      foldersFromFilesStructure: true\n",
    );
    scratch.write("prov/boards/platform/slo.json", &dash("slo", "prom"));
    scratch.write("prov/boards/platform/notes.txt", "not a dashboard");
    scratch.write("prov/boards/teams/root.json", &dash("team-root", "prom"));
    scratch.write("prov/boards/teams/alpha/a.json", &dash("team-a", "prom"));
    scratch.write(
        "prov/boards/teams/beta/deeper/b.json",
        &dash("team-b", "prom"),
    );

    let cfg = config(json!({
        "organization": { "id": org, "name": "City" },
        "tokensFile": tokens,
        "secretsDir": scratch.0.join("secrets/ds"),
        "provisioningDir": scratch.0.join("prov"),
    }))
    .unwrap();
    let built = build_state(&cfg).await.unwrap();
    let report = built.provisioning.clone().unwrap();
    assert_eq!(report.datasources_upserted, 1);
    assert_eq!(report.datasources_deleted, 0);
    assert_eq!(report.dashboards, 4);
    assert_eq!(report.folders, 3, "Platform, alpha and deeper");
    let state = built.state;

    // The provisioned datasource resolves its secretRef from the organization's directory.
    let ds = state
        .store
        .get_datasource(org, "prom")
        .await
        .unwrap()
        .unwrap();
    assert!(
        ds.read_only,
        "provisioned datasources are read-only to the UI"
    );
    assert_eq!(
        state
            .secret_resolver
            .resolve_secret(org, &ds.secret_refs["token"])
            .unwrap()
            .as_str(),
        "bearer-from-file"
    );
    let slo = state
        .store
        .get_dashboard(org, "slo")
        .await
        .unwrap()
        .unwrap();
    assert!(slo.is_provisioned);
    assert!(!slo.allow_ui_updates);
    assert_eq!(
        slo.provisioned_external_id.as_deref(),
        Some("boards/platform/slo.json")
    );
    let platform = state
        .store
        .get_folder(org, slo.folder_uid.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(platform.title, "Platform");
    assert_eq!(
        platform.managed_by.as_deref(),
        Some("provisioning:platform")
    );
    assert_eq!(
        state
            .store
            .get_dashboard(org, "team-root")
            .await
            .unwrap()
            .unwrap()
            .folder_uid,
        None
    );
    let team_a = state
        .store
        .get_dashboard(org, "team-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .store
            .get_folder(org, team_a.folder_uid.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap()
            .title,
        "alpha"
    );
    assert_eq!(state.org_registry.read().by_num.get(&1).unwrap().0, "City");

    // The CLI context is the organization's admin.
    let ctx = cli_ctx(&state);
    assert_eq!(ctx.subject.actor_id.as_deref(), Some(CLI_ACTOR));
    assert_eq!(ctx.org_id, org);

    // Through the router: the token works, a viewer cannot write, no token is anonymous.
    let app = standalone_router(state.clone(), cfg.mount().unwrap());
    let (st, hits) = send(&app, get("/api/search", Some(&token))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        hits.as_array()
            .unwrap()
            .iter()
            .filter(|h| h["type"] == "dash-db")
            .count(),
        4,
        "{hits}"
    );
    let (st, _) = send(&app, get("/api/dashboards/uid/slo", None)).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, get("/api/dashboards/uid/slo", Some("viewer"))).await;
    assert_eq!(st, StatusCode::OK);
    let del = Request::builder()
        .method("DELETE")
        .uri("/api/dashboards/uid/slo")
        .header("authorization", "Bearer viewer")
        .body(Body::empty())
        .unwrap();
    let (st, _) = send(&app, del).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A second start over the same store re-provisions in place (versions move on).
    let again = build_state(&cfg).await.unwrap();
    assert_eq!(again.provisioning.unwrap().dashboards, 4);
}

/// (tag, files under the provisioning root, text the refusal must contain)
type ProvisioningCase = (&'static str, Vec<(&'static str, String)>, &'static str);

#[tokio::test]
async fn provisioning_refuses_secrets_other_organizations_and_escapes() {
    let cases: Vec<ProvisioningCase> = vec![
        (
            "secure",
            vec![("datasources/d.yaml", "apiVersion: 1\ndatasources:\n  - name: P\n    type: prometheus\n    url: https://p.example\n    secureJsonData:\n      httpHeaderValue1: Bearer raw\n".into())],
            "secureJsonData",
        ),
        (
            "org2-ds",
            vec![("datasources/d.yaml", "apiVersion: 1\ndatasources:\n  - name: P\n    orgId: 2\n    type: prometheus\n    url: https://p.example\n".into())],
            "orgId 2",
        ),
        (
            "org2-dash",
            vec![
                ("dashboards/p.yaml", "apiVersion: 1\nproviders:\n  - name: p\n    orgId: 3\n    options:\n      path: b\n".into()),
                ("b/x.json", dash("x", "prom")),
            ],
            "orgId 3",
        ),
        (
            "escape",
            vec![("dashboards/p.yaml", "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: ../outside\n".into())],
            "traversal",
        ),
        (
            "missing",
            vec![("dashboards/p.yaml", "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: nowhere\n".into())],
            "not a directory",
        ),
        (
            "notjson",
            vec![
                ("dashboards/p.yaml", "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: b\n".into()),
                ("b/x.json", "{ not json".into()),
            ],
            "not JSON",
        ),
        (
            "refused",
            vec![
                ("datasources/d.yaml", "apiVersion: 1\ndatasources:\n  - name: P\n    uid: prom\n    type: prometheus\n    url: https://p.example\n".into()),
                ("dashboards/p.yaml", "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: b\n".into()),
                ("b/x.json", json!({ "uid": "x", "title": "X", "schemaVersion": 39, "panels": [
                    { "id": 1, "type": "timeseries", "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
                      "datasource": { "type": "prometheus", "uid": "absent" },
                      "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": "absent" } }] }
                ]}).to_string()),
            ],
            "refused features",
        ),
    ];
    for (tag, files, needle) in cases {
        let scratch = Scratch::new(tag);
        for (rel, text) in &files {
            scratch.write(&format!("prov/{rel}"), text);
        }
        std::fs::create_dir_all(scratch.0.join("outside")).unwrap();
        let cfg = config(json!({ "provisioningDir": scratch.0.join("prov") })).unwrap();
        let err = match build_state(&cfg).await {
            Ok(_) => panic!("{tag}: provisioning accepted"),
            Err(e) => e,
        };
        let text = match &err {
            CrError::Unprocessable { detail, errors } => format!("{detail} {errors:?}"),
            other => other.to_string(),
        };
        assert!(text.contains(needle), "{tag}: {text}");
    }

    // A dashboard file that is a symlink out of the root is refused, not read.
    let scratch = Scratch::new("symlink");
    let outside = Scratch::new("symlink-target");
    let secret = outside.write("secret.json", &dash("leak", "prom"));
    scratch.write(
        "prov/dashboards/p.yaml",
        "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: b\n",
    );
    std::fs::create_dir_all(scratch.0.join("prov/b")).unwrap();
    std::os::unix::fs::symlink(&secret, scratch.0.join("prov/b/leak.json")).unwrap();
    let cfg = config(json!({ "provisioningDir": scratch.0.join("prov") })).unwrap();
    let err = match build_state(&cfg).await {
        Ok(_) => panic!("symlink accepted"),
        Err(e) => e,
    };
    assert_eq!(err.status_code(), 403, "{err}");

    // A missing provisioning root is an error at start, not a silent empty instance.
    let cfg = config(json!({ "provisioningDir": "/nonexistent/daiya/prov" })).unwrap();
    assert!(build_state(&cfg).await.is_err());
    // So is a missing tokens file.
    let cfg = config(json!({ "tokensFile": "/nonexistent/daiya/tokens.json" })).unwrap();
    assert!(build_state(&cfg).await.is_err());
}

#[tokio::test]
async fn delete_datasources_removes_a_provisioned_read_only_datasource() {
    let scratch = Scratch::new("delete");
    scratch.write(
        "prov/datasources/a.yaml",
        "apiVersion: 1\ndatasources:\n  - name: Old\n    uid: old\n    type: prometheus\n    url: https://old.example\n",
    );
    let cfg = config(json!({ "provisioningDir": scratch.0.join("prov") })).unwrap();
    let built = build_state(&cfg).await.unwrap();
    let org = built.state.default_org_id;
    assert!(
        built
            .state
            .store
            .get_datasource(org, "old")
            .await
            .unwrap()
            .unwrap()
            .read_only
    );
    scratch.write(
        "prov/datasources/a.yaml",
        "apiVersion: 1\ndeleteDatasources:\n  - name: Old\n    orgId: 1\n",
    );
    let report = daiya_import::load_provisioning_dir(
        built.state.store.as_ref(),
        daiya_import::ProvisioningTarget {
            installation_id: built.state.installation_id,
            org_id: org,
            instance_id: built.state.default_instance_id,
        },
        &scratch.0.join("prov"),
        &built.state.limits,
        &std::collections::BTreeMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.datasources_deleted, 1);
    assert!(
        built
            .state
            .store
            .get_datasource(org, "old")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_standalone_router_rate_limits_public_dashboards_per_source_address() {
    let built = build_state(&DaiyaConfig::default()).await.unwrap();
    let app = standalone_router(built.state, DaiyaConfig::default().mount().unwrap());
    let token = "a".repeat(64);
    let from = |ip: [u8; 4]| {
        let mut req = Request::builder()
            .uri(format!("/api/public/dashboards/{token}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((ip, 40_000))));
        req
    };
    let mut limited = false;
    for _ in 0..130 {
        let (st, _) = send(&app, from([203, 0, 113, 7])).await;
        if st == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    assert!(limited, "one address is limited");
    // Another address is counted on its own.
    let (st, _) = send(&app, from([198, 51, 100, 9])).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Writes `secret` to a 0600 file under `dir` (removed with it).
fn secret_file(dir: &Path, name: &str, secret: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, secret).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

#[tokio::test]
async fn a_postgres_store_from_configuration_migrates_checks_rls_and_keeps_data() {
    let Ok(admin_url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return;
    };
    let db = daiya_store_pg::connect_test_database(&admin_url)
        .await
        .unwrap();
    let opts: sqlx::postgres::PgConnectOptions = admin_url.parse().unwrap();
    let scratch = Scratch::new("pg");
    let app_pw = secret_file(&scratch.0, "app", db.app_config.password.as_str());
    let parsed = url::Url::parse(&admin_url).unwrap();
    let admin_password = percent_encoding::percent_decode_str(parsed.password().unwrap_or(""))
        .decode_utf8()
        .unwrap()
        .into_owned();
    let admin_pw = secret_file(&scratch.0, "admin", &admin_password);
    let host = format!("{}:{}", opts.get_host(), opts.get_port());
    let database = opts.get_database().unwrap_or("postgres");
    let org = OrgId::new();
    scratch.write("prov/datasources/d.yaml", "apiVersion: 1\ndatasources:\n  - name: P\n    uid: prom\n    type: prometheus\n    url: https://prom.example.com\n");
    scratch.write(
        "prov/dashboards/p.yaml",
        "apiVersion: 1\nproviders:\n  - name: p\n    options:\n      path: b\n",
    );
    scratch.write("prov/b/x.json", &dash("pg-dash", "prom"));

    let cfg = config(json!({
        "organization": { "id": org },
        "store": {
            "kind": "postgres",
            "url": format!("postgres://daiya_app_login@{host}/{database}"),
            "passwordFile": app_pw,
            "maxConnections": 2,
            "migrate": { "url": format!("postgres://{}@{host}/{database}", opts.get_username()), "passwordFile": admin_pw }
        },
        "provisioningDir": scratch.0.join("prov"),
    }))
    .unwrap();
    let built = build_state(&cfg).await.unwrap();
    assert_eq!(built.provisioning.unwrap().dashboards, 1);
    drop(built.state);

    // A fresh start without provisioning still finds what the first one wrote.
    let mut plain = cfg.clone();
    plain.provisioning_dir = None;
    let state = build_state(&plain).await.unwrap().state;
    let d = state
        .store
        .get_dashboard(org, "pg-dash")
        .await
        .unwrap()
        .unwrap();
    assert!(d.is_provisioned);

    // The superuser as the application role is refused by the boot check.
    let as_admin = StoreConfig::Postgres {
        url: format!("postgres://{}@{host}/{database}", opts.get_username()),
        password_file: scratch.0.join("admin"),
        max_connections: 1,
        migrate: None,
    };
    let err = match open_store(&as_admin).await {
        Ok(_) => panic!("superuser accepted as the application role"),
        Err(e) => e,
    };
    assert_eq!(err.slug(), "cr.rls_not_enforced");
    // A password in the URL is refused before anything connects.
    let inline = StoreConfig::Postgres {
        url: format!("postgres://daiya_app_login:inline@{host}/{database}"),
        password_file: scratch.0.join("app"),
        max_connections: 1,
        migrate: None,
    };
    assert!(open_store(&inline).await.is_err());
}

/// `serverAdmin`: Grafana's local administrator from a password file, as `provision.sh` and
/// the Portal sign in; it creates a project organization. A missing or weak password refuses
/// the start without repeating the password.
#[tokio::test]
async fn the_server_admin_password_comes_from_its_file() {
    let scratch = Scratch::new("admin");
    let file = scratch.write("secrets/admin", "server-admin-password-0123\n");
    let cfg = config(json!({ "serverAdmin": { "user": "admin", "passwordFile": file } })).unwrap();
    let state = build_state(&cfg).await.unwrap().state;
    let app = standalone_router(state, cfg.mount().unwrap());
    let create = |auth: &str| {
        Request::post("/api/orgs")
            .header("authorization", auth)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"helsinki"}"#))
            .unwrap()
    };
    // base64("admin:server-admin-password-0123")
    let (status, body) = send(
        &app,
        create("Basic YWRtaW46c2VydmVyLWFkbWluLXBhc3N3b3JkLTAxMjM="),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["orgId"], 2);

    let weak = scratch.write("secrets/weak", "weak-password\n");
    let cfg = config(json!({ "serverAdmin": { "user": "admin", "passwordFile": weak } })).unwrap();
    let err = build_state(&cfg).await.err().expect("refused");
    assert!(!err.to_string().contains("weak-password"), "{err}");
    let cfg = config(json!({ "serverAdmin": { "user": "admin",
                                              "passwordFile": scratch.0.join("none") } }))
    .unwrap();
    assert!(build_state(&cfg).await.is_err());
}

#[test]
fn the_tokens_file_cannot_name_the_server_admin() {
    let scratch = Scratch::new("reserved");
    let tokens = scratch.write(
        "tokens.json",
        &json!([{ "tokenSha256": sha256_hex(b"x"), "actor": daiya_model::SERVER_ADMIN_ACTOR,
                  "role": "Admin" }])
        .to_string(),
    );
    assert!(read_tokens_file(&tokens).is_err());
}

/// Only `DAIYA_VAR_*` variables reach provisioning, without their prefix.
#[test]
fn provisioning_variables_come_only_from_the_daiya_var_prefix() {
    let vars = daiya_server::config::provisioning_vars_from([
        ("DAIYA_VAR_HOST".to_owned(), "prom".to_owned()),
        ("DAIYA_VAR_".to_owned(), "empty name".to_owned()),
        ("JC_SECRET".to_owned(), "no".to_owned()),
        ("HOME".to_owned(), "/root".to_owned()),
    ]);
    assert_eq!(vars.len(), 1);
    assert_eq!(vars["HOST"], "prom");
}
