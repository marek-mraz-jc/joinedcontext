#![forbid(unsafe_code)]
//! The real `daiya` binary end to end (R2, R3, R8, R11): started with a configuration file,
//! it loads provisioning, accepts the configured API token, answers a query from a
//! provisioned datasource against a mock Prometheus, and (on Postgres) keeps what was written
//! across a restart; the CLI subcommands act on the same store.

use daiya_model::sha256_hex;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BIN: &str = env!("CARGO_BIN_EXE_daiya");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("daiya-bin-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, text: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    child: Child,
    base: String,
}

impl Server {
    /// Stops the server the way an operator does (SIGINT / Ctrl-C) and returns its exit code.
    fn interrupt(&mut self) -> Option<i32> {
        let status = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        for _ in 0..100 {
            if let Some(exit) = self.child.try_wait().unwrap() {
                return exit.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daiya did not stop on SIGINT");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.interrupt();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `daiya --config <cfg> serve` and waits for its "listening on" line.
fn serve(cfg: &Path) -> Server {
    serve_with(&["--config", cfg.to_str().unwrap(), "serve"]).0
}

/// Starts `daiya <args>` and waits for its "listening on" line; also returns what it wrote
/// to stderr before that line.
fn serve_with(args: &[&str]) -> (Server, Vec<String>) {
    let mut child = Command::new(BIN)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stderr).lines();
    let mut seen = Vec::new();
    let addr = loop {
        let Some(Ok(line)) = lines.next() else {
            let _ = child.kill();
            panic!("daiya exited before listening: {seen:?}");
        };
        if let Some(addr) = line.strip_prefix("daiya listening on ") {
            break addr.to_owned();
        }
        seen.push(line);
    };
    std::thread::spawn(move || for _ in lines {});
    (
        Server {
            child,
            base: format!("http://{addr}"),
        },
        seen,
    )
}

async fn prometheus_mock() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let body = json!({ "status": "success", "data": { "resultType": "matrix", "result": [
                { "metric": { "job": "edge" }, "values": [[1700000000, "41"], [1700000060, "42"]] }
            ]}})
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

fn dashboard(uid: &str) -> Value {
    json!({
        "uid": uid,
        "title": format!("Board {uid}"),
        "schemaVersion": 39,
        "panels": [{
            "id": 1, "type": "timeseries", "title": "Edge",
            "gridPos": { "x": 0, "y": 0, "w": 12, "h": 8 },
            "datasource": { "type": "prometheus", "uid": "prom" },
            "targets": [{ "refId": "A", "expr": "up", "datasource": { "type": "prometheus", "uid": "prom" } }]
        }]
    })
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

/// The store part of the configuration: Postgres on the application login role when the
/// test database is set, memory otherwise.
async fn store_config(scratch: &Scratch) -> (Value, bool) {
    let Ok(admin_url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP (Postgres part): DAIYA_TEST_DATABASE_URL is unset; running on memory");
        return (json!({ "kind": "memory" }), false);
    };
    let db = daiya_store_pg::connect_test_database(&admin_url)
        .await
        .unwrap();
    let pw = scratch.write("secrets/db", db.app_config.password.as_str());
    let c = &db.app_config;
    (
        json!({
            "kind": "postgres",
            "url": format!("postgres://{}@{}:{}/{}", c.app_role, c.host, c.port, c.database),
            "passwordFile": pw,
            "maxConnections": 2
        }),
        true,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_provisioned_dashboards_and_queries_with_a_configured_token() {
    let scratch = Scratch::new();
    let mock = prometheus_mock().await;
    let org = uuid::Uuid::now_v7();
    let token = format!("ci-{}", uuid::Uuid::new_v4());
    let tokens = scratch.write(
        "secrets/tokens.json",
        &json!([{ "tokenSha256": sha256_hex(token.as_bytes()), "actor": "ci", "role": "Admin" }])
            .to_string(),
    );
    scratch.write(
        "prov/datasources/prom.yaml",
        &format!("apiVersion: 1\ndatasources:\n  - name: Prometheus\n    uid: prom\n    type: prometheus\n    url: {mock}\n    isDefault: true\n"),
    );
    scratch.write(
        "prov/dashboards/p.yaml",
        "apiVersion: 1\nproviders:\n  - name: ops\n    folder: Ops\n    options:\n      path: boards\n",
    );
    scratch.write("prov/boards/edge.json", &dashboard("edge").to_string());
    let (store, persistent) = store_config(&scratch).await;
    let cfg = scratch.write(
        "daiya.json",
        &json!({
            "listen": "127.0.0.1:0",
            "mount": { "scheme": "http", "host": "127.0.0.1" },
            "organization": { "id": org, "name": "Edge City" },
            "store": store,
            "tokensFile": tokens,
            "provisioningDir": scratch.0.join("prov"),
            "outbound": { "allowHttp": true, "allowedNetworks": ["127.0.0.0/8"] }
        })
        .to_string(),
    );

    let server = serve(&cfg);
    let client = http();
    let auth = format!("Bearer {token}");

    let health = client
        .get(format!("{}/api/health", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    assert!(health.headers().contains_key("content-security-policy"));

    let search: Value = client
        .get(format!("{}/api/search?query=Board", server.base))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(search.to_string().contains("\"edge\""), "{search}");

    // Without the token the same dashboard does not exist.
    let anon = client
        .get(format!("{}/api/dashboards/uid/edge", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 404);

    // The provisioned datasource answers through its built driver.
    let answer = client
        .post(format!("{}/api/ds/query", server.base))
        .header("authorization", &auth)
        .json(&json!({
            "from": "1700000000000", "to": "1700000600000",
            "queries": [{ "refId": "A", "datasource": { "uid": "prom" }, "expr": "up" }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), 200);
    let answer: Value = answer.json().await.unwrap();
    assert_eq!(
        answer["results"]["A"]["frames"][0]["fields"][1]["values"],
        json!([41.0, 42.0]),
        "{answer}"
    );

    // OData and the org lookup see the configured organization.
    let odata: Value = client
        .get(format!("{}/odata/v4/Dashboards?$select=uid", server.base))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(odata.to_string().contains("edge"), "{odata}");
    let org_lookup = client
        .get(format!("{}/api/orgs/name/Edge%20City", server.base))
        .header("authorization", &auth)
        .send()
        .await
        .unwrap();
    assert_eq!(org_lookup.status(), 200);

    // A dashboard saved through the API.
    let saved = client
        .post(format!("{}/api/dashboards/db", server.base))
        .header("authorization", &auth)
        .json(&json!({ "dashboard": dashboard("saved-over-http"), "overwrite": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let mut server = server;
    assert_eq!(server.interrupt(), Some(0), "a graceful stop exits 0");
    drop(server);

    // The CLI acts on the configured store.
    let file = scratch.write("import/cli.json", &dashboard("from-cli").to_string());
    let import = Command::new(BIN)
        .args([
            "--config",
            cfg.to_str().unwrap(),
            "import",
            "-f",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        matches!(import.status.code(), Some(0 | 2)),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let export = Command::new(BIN)
        .args(["--config", cfg.to_str().unwrap(), "export", "--uid", "edge"])
        .output()
        .unwrap();
    assert_eq!(
        export.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let exported: Value = serde_json::from_slice(&export.stdout).unwrap();
    assert_eq!(exported["uid"], "edge");
    assert_eq!(exported["schemaVersion"], 42);

    if persistent {
        // Restarted on the same database: what the API and the CLI wrote is still there.
        let server = serve(&cfg);
        for uid in ["saved-over-http", "from-cli"] {
            let got = client
                .get(format!("{}/api/dashboards/uid/{uid}", server.base))
                .header("authorization", &auth)
                .send()
                .await
                .unwrap();
            assert_eq!(got.status(), 200, "{uid}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn serve_flags_override_the_mount_and_a_taken_port_is_a_start_failure() {
    let (mut server, early) = serve_with(&[
        "serve",
        "--listen",
        "127.0.0.1:0",
        "--scheme",
        "https",
        "--host",
        "dash.example.org",
        "--path-prefix",
        "/dash",
    ]);
    assert!(
        early.iter().any(|l| l.contains("no API tokens configured")),
        "{early:?}"
    );
    let info: Value = http()
        .get(format!("{}/mount-info", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        info["publicBase"], "https://dash.example.org/dash",
        "{info}"
    );
    assert_eq!(
        info["odataUrl"], "https://dash.example.org/dash/odata/v4",
        "{info}"
    );

    // Another daiya on the same address cannot bind it.
    let taken = server.base.trim_start_matches("http://").to_owned();
    let out = Command::new(BIN)
        .args(["serve", "--listen", &taken])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("failed to bind"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(server.interrupt(), Some(0));
}

#[test]
fn the_binary_refuses_to_start_on_a_bad_configuration() {
    let scratch = Scratch::new();
    let unknown = scratch.write("unknown.json", "{ \"tokenFile\": \"/x\" }");
    let out = Command::new(BIN)
        .args(["--config", unknown.to_str().unwrap(), "serve"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("tokenFile"));

    let missing_prov = scratch.write(
        "prov.json",
        &json!({ "provisioningDir": scratch.0.join("absent") }).to_string(),
    );
    let out = Command::new(BIN)
        .args(["--config", missing_prov.to_str().unwrap(), "serve"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot start"));

    let bad_mount = scratch.write("mount.json", "{ \"listen\": \"127.0.0.1:0\" }");
    let out = Command::new(BIN)
        .args([
            "--config",
            bad_mount.to_str().unwrap(),
            "serve",
            "--path-prefix",
            "x/../y",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));

    // An unreadable plan bundle is refused, not planned as an empty one.
    let bundle = scratch.write("bundle.json", "{ not json");
    let out = Command::new(BIN)
        .args(["plan", "-f", bundle.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid bundle"));
}
