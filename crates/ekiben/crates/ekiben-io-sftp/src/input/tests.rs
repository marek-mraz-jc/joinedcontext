use std::collections::HashMap;

use ekiben_core::{OrgId, OrgQuotas, Registry, ResourceManager, StreamMetrics};
use serde_json::json;

use super::*;

pub(crate) fn ctx(quotas: OrgQuotas) -> StreamContext {
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(&OrgId::new("org-sftp"), quotas);
    StreamContext {
        stream_id: "sftp-test".into(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("sftp-test", "org-sftp"),
        label: String::new(),
        path: "root.input".into(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

fn base() -> JsonValue {
    json!({
        "address": "sftp.example.org:22",
        "host_key_fingerprint": "SHA256:Nh0Me49Zh9fDw/VYUfq43IJmI1T+XrjiYONPND8GzaM",
        "credentials": {"username": "city", "password": "hunter2"},
        "paths": ["/upload/*.csv"],
    })
}

fn with(extra: JsonValue) -> JsonValue {
    let mut v = base();
    let o = v.as_object_mut().expect("object");
    for (k, x) in extra.as_object().expect("object") {
        if x.is_null() {
            o.remove(k);
        } else {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

#[test]
fn config_refusals_name_the_field_and_never_the_password() {
    for (extra, want) in [
        (
            json!({"host_key_fingerprint": null}),
            "host_key_fingerprint",
        ),
        (
            json!({"host_key_fingerprint": "MD5:aa:bb"}),
            "host_key_fingerprint",
        ),
        (
            json!({"credentials": {"username": "city", "private_key_file": "/k"}}),
            "private_key_file",
        ),
        (
            json!({"credentials": {"username": "city"}}),
            "credentials needs",
        ),
        (
            json!({"credentials": {"username": "city", "private_key": "not a key"}}),
            "credentials.private_key",
        ),
        (
            json!({"credentials": {"username": "c", "password": "hunter2", "token": "x"}}),
            "credentials.token",
        ),
        (
            json!({"paths": ["upload/*.csv"]}),
            "paths[0] must be an absolute path",
        ),
        (json!({"paths": ["/up*/a.csv"]}), "file name only"),
        (json!({"paths": ["/a/../etc/passwd"]}), "file name only"),
        (json!({"paths": []}), "paths needs"),
        (json!({"codec": "lines"}), "codec"),
        (json!({"watcher": {"enabled": true}}), "watcher.cache"),
        (
            json!({"watcher": {"enabled": true, "cache": "c", "poll_interval": "10ms"}}),
            "poll_interval",
        ),
        (json!({"max_file_bytes": 0}), "max_file_bytes"),
        (json!({"address": "sftp.example.org:http"}), "address"),
        (json!({"frobnicate": 1}), "frobnicate"),
        (
            json!({"credentials": "city:hunter2"}),
            "credentials must be an object",
        ),
        (json!({"address": ""}), "address must be host:port"),
        (
            json!({"paths": ["/upload/[x"]}),
            "paths[0] is not a valid glob",
        ),
        (json!({"paths": ["/upload/"]}), "file name only"),
        (
            json!({"watcher": {"enabled": true, "cache": "c", "poll_interval": "often"}}),
            "watcher.poll_interval must be a duration, e.g. 1s",
        ),
        (
            json!({"watcher": {"enabled": true, "cache": "c", "minimum_age": "old"}}),
            "watcher.minimum_age must be a duration, e.g. 1s",
        ),
        (
            json!({"max_file_bytes": MAX_FILE_BYTES + 1}),
            "max_file_bytes must be 1 to",
        ),
        (
            json!({"scanner": {"bogus": {}}}),
            "scanner type 'bogus' was not recognised",
        ),
    ] {
        let errs = lint(&with(extra.clone()));
        let e = errs.join("; ");
        assert!(e.contains(want), "{extra}: {e}");
        assert!(!e.contains("hunter2"), "{e}");
    }
    assert!(lint(&base()).is_empty(), "{:?}", lint(&base()));
}

#[test]
fn an_address_without_a_port_dials_22_and_a_root_glob_lists_the_root() {
    let s = Settings::parse(&with(
        json!({"address": "sftp.example.org", "paths": ["/*.csv"]}),
    ))
    .expect("parses");
    assert_eq!(
        (s.target.host.as_str(), s.target.port),
        ("sftp.example.org", 22)
    );
    assert_eq!(s.paths[0].0, "/");
    let s = Settings::parse(&with(json!({"address": "[::1]:2222"}))).expect("parses");
    assert_eq!((s.target.host.as_str(), s.target.port), ("::1", 2222));
    // a watcher that is not enabled is no watcher, and max_file_bytes takes its ceiling
    let s = Settings::parse(&with(json!({
        "watcher": {"enabled": false, "cache": "c"},
        "max_file_bytes": MAX_FILE_BYTES,
    })))
    .expect("parses");
    assert!(s.watcher.is_none());
    assert_eq!(s.max_file_bytes, MAX_FILE_BYTES);
    assert!(!lint(&json!("sftp://x")).is_empty());
}

/// An input dialling a loopback port where `serve` answers each connection.
fn local_input(port: u16, extra: JsonValue) -> (SftpInput, StreamContext) {
    let c = ctx(OrgQuotas {
        allow_private_ips: true,
        ..Default::default()
    });
    let mut config = with(json!({"address": format!("127.0.0.1:{port}")}));
    config
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    let conf = ComponentConfig {
        label: "upload".into(),
        component_type: "sftp".into(),
        config,
        processors: Vec::new(),
    };
    (SftpInput::from_config(&conf, &c).expect("builds"), c)
}

#[tokio::test]
async fn a_server_that_is_not_ssh_fails_the_handshake_by_name() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Ok((mut sock, _)) = listener.accept().await {
            let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        }
    });
    let (mut i, c) = local_input(port, json!({}));
    assert_eq!((i.input_type(), i.label()), ("sftp", "upload"));
    let e = match i.read_batch(&c).await {
        Ok(_) => panic!("read from a web server"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("sftp input: ssh handshake"), "{e}");
    assert!(!i.is_connected());
    i.close().await.expect("nothing to close");
}

#[tokio::test]
async fn a_server_that_never_answers_times_out_the_handshake() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });
    let (mut i, c) = local_input(port, json!({}));
    let started = std::time::Instant::now();
    let e = match i.read_batch(&c).await {
        Ok(_) => panic!("read from a silent server"),
        Err(e) => e.to_string(),
    };
    assert!(
        e.contains("did not complete the SSH handshake in 15 s"),
        "{e}"
    );
    assert!(started.elapsed() >= CONNECT_TIMEOUT);
}

#[tokio::test]
async fn a_refused_connection_fails_a_listing_at_once_and_a_watcher_keeps_dialling() {
    // a bound socket that never listens refuses every connection
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = closed.local_addr().expect("addr").port();
    drop(closed);
    let (mut i, c) = local_input(port, json!({}));
    let e = match i.read_batch(&c).await {
        Ok(_) => panic!("read from a closed port"),
        Err(e) => e,
    };
    assert!(
        !matches!(e, CoreError::Config(_) | CoreError::AccessDenied { .. }),
        "{e}"
    );

    let cache: Arc<dyn Cache> = Arc::new(NoCache);
    let mut c = ctx(OrgQuotas {
        allow_private_ips: true,
        ..Default::default()
    });
    c.resources.insert_cache(c.org_id(), "seen", cache);
    let conf = ComponentConfig {
        label: String::new(),
        component_type: "sftp".into(),
        config: with(json!({
            "address": format!("127.0.0.1:{port}"),
            "watcher": {"enabled": true, "cache": "seen"},
        })),
        processors: Vec::new(),
    };
    let mut w = SftpInput::from_config(&conf, &c).expect("builds");
    c.label = "watcher".into();
    // it dials again with a growing pause and never gives up on its own
    let r = tokio::time::timeout(Duration::from_millis(800), w.read_batch(&c)).await;
    assert!(r.is_err(), "the watcher stopped dialling");
    assert!(!w.is_connected());
    assert!(w.backoff > Duration::from_millis(100), "{:?}", w.backoff);
}

/// A cache resource that holds nothing.
struct NoCache;

#[async_trait]
impl Cache for NoCache {
    async fn get(&self, key: &str) -> Result<bytes::Bytes, CoreError> {
        Err(CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(&self, _: &str, _: bytes::Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        Ok(())
    }
    async fn add(&self, _: &str, _: bytes::Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        Ok(())
    }
    async fn delete(&self, _: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

#[test]
fn a_watcher_names_a_cache_that_exists() {
    let conf = ComponentConfig {
        label: String::new(),
        component_type: "sftp".into(),
        config: with(json!({"watcher": {"enabled": true, "cache": "seen"}})),
        processors: Vec::new(),
    };
    match SftpInput::from_config(&conf, &ctx(OrgQuotas::default())) {
        Ok(_) => panic!("built without the cache resource"),
        Err(e) => assert!(e.to_string().contains("seen"), "{e}"),
    }
}

#[tokio::test]
async fn a_private_server_is_refused_before_any_connection() {
    let conf = ComponentConfig {
        label: String::new(),
        component_type: "sftp".into(),
        config: with(json!({"address": "10.11.12.13:22"})),
        processors: Vec::new(),
    };
    let c = ctx(OrgQuotas::default());
    let mut i = SftpInput::from_config(&conf, &c).expect("builds");
    let started = std::time::Instant::now();
    let e = match i.read_batch(&c).await {
        Ok(_) => panic!("read from a private address"),
        Err(e) => e,
    };
    assert!(matches!(e, CoreError::AccessDenied { .. }), "{e}");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!i.is_connected());
}
