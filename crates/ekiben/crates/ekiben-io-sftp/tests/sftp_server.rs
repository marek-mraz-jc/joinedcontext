//! sftp input against a real OpenSSH server (atmoz/sftp in the CI ekiben job). Runs where
//! SFTP_TEST_ADDRESS (host:port), SFTP_TEST_USER and SFTP_TEST_PASSWORD name one whose user can
//! write /upload, as the S3 tests run where S3_TEST_ENDPOINT names a store. The host key is read
//! once from the refusal of a wrong pin: the server is a throwaway container.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_core::redact::SecretString;
use ekiben_core::{
    BatchError, Cache, ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Registry,
    ResourceManager, StreamContext, StreamMetrics,
};
use ekiben_io_sftp::{connect, SftpInput, SftpTarget};
use serde_json::{json, Value as JsonValue};
use tokio::io::AsyncWriteExt;

struct Server {
    address: String,
    user: String,
    password: String,
}

fn server() -> Option<Server> {
    let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    Some(Server {
        address: v("SFTP_TEST_ADDRESS")?,
        user: v("SFTP_TEST_USER")?,
        password: v("SFTP_TEST_PASSWORD")?,
    })
}

#[derive(Default)]
struct MemCache(Mutex<HashMap<String, Bytes>>);

#[async_trait]
impl Cache for MemCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        self.0
            .lock()
            .expect("lock")
            .get(key)
            .cloned()
            .ok_or_else(|| CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(&self, key: &str, value: Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        self.0.lock().expect("lock").insert(key.to_string(), value);
        Ok(())
    }
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        self.set(key, value, ttl).await
    }
    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.0.lock().expect("lock").remove(key);
        Ok(())
    }
}

fn ctx(cache: Arc<MemCache>) -> StreamContext {
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-sftp");
    // the server listens on loopback, which only an organization with allow_private_ips reaches
    let org = resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        },
    );
    resources.insert_cache(&org_id, "seen", cache);
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

fn target(s: &Server, pin: &str) -> SftpTarget {
    let (host, port) = s.address.rsplit_once(':').expect("host:port");
    SftpTarget {
        host: host.to_string(),
        port: port.parse().expect("port"),
        host_key_fingerprint: pin.to_string(),
        username: s.user.clone(),
        password: Some(SecretString::new(s.password.clone())),
        private_key: None,
        private_key_pass: None,
    }
}

/// The server's pin, from the error a wrong one gets.
async fn pin(s: &Server, c: &StreamContext) -> String {
    let wrong = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let e = match connect(&target(s, wrong), &c.org).await {
        Ok(_) => panic!("a wrong host key pin was accepted"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("does not match host_key_fingerprint"), "{e}");
    let at = e.find("presented host key ").expect("names the key") + "presented host key ".len();
    e[at..].split(',').next().expect("fingerprint").to_string()
}

fn input(s: &Server, pin: &str, extra: JsonValue, c: &StreamContext) -> SftpInput {
    let mut config = json!({
        "address": s.address,
        "host_key_fingerprint": pin,
        "credentials": {"username": s.user, "password": s.password},
        "scanner": {"lines": {}},
    });
    config
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    SftpInput::from_config(
        &ComponentConfig {
            label: String::new(),
            component_type: "sftp".into(),
            config,
            processors: Vec::new(),
        },
        c,
    )
    .expect("builds")
}

async fn put(s: &Server, pin: &str, c: &StreamContext, path: &str, body: &str) {
    let session = connect(&target(s, pin), &c.org).await.expect("connects");
    let mut f = session.sftp.create(path.to_string()).await.expect("create");
    f.write_all(body.as_bytes()).await.expect("write");
    f.shutdown().await.expect("close");
}

/// Every log message of this test binary: the warnings are the input's behaviour on failures it
/// rides out.
fn logs() -> &'static Mutex<Vec<String>> {
    static LOGS: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
    let logs = LOGS.get_or_init(Mutex::default);
    static SET: std::sync::Once = std::sync::Once::new();
    SET.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Recorder);
    });
    logs
}

fn logged(part: &str) -> bool {
    logs()
        .lock()
        .expect("lock")
        .iter()
        .any(|m| m.contains(part))
}

struct Recorder;

struct Message(String);

impl tracing::field::Visit for Message {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut m = Message(String::new());
        event.record(&mut m);
        logs().lock().expect("lock").push(m.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn lines(b: &ekiben_core::Batch) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_matching_files_deletes_them_after_ack_and_refuses_bad_logins() {
    let Some(s) = server() else {
        eprintln!("SFTP_TEST_ADDRESS unset: skipped");
        return;
    };
    let c = ctx(Arc::default());
    let pin = pin(&s, &c).await;
    let dir = format!("/upload/once-{}", std::process::id());
    let session = connect(&target(&s, &pin), &c.org).await.expect("connects");
    session.sftp.create_dir(dir.clone()).await.expect("mkdir");
    put(&s, &pin, &c, &format!("{dir}/a.csv"), "a1\na2\n").await;
    put(&s, &pin, &c, &format!("{dir}/b.csv"), "b1\n").await;
    put(&s, &pin, &c, &format!("{dir}/c.txt"), "not matched\n").await;

    let mut i = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/*.csv")], "delete_on_finish": true}),
        &c,
    );
    let (b, ack) = i.read_batch(&c).await.expect("a.csv");
    assert_eq!(lines(&b), ["a1", "a2"]);
    assert_eq!(b[0].meta_get_str("sftp_path"), format!("{dir}/a.csv"));
    ack.ack();
    let (b, ack) = i.read_batch(&c).await.expect("b.csv");
    assert_eq!(lines(&b), ["b1"]);
    // A nacked file stays on the server.
    ack.nack(BatchError::new(b, "output down".to_string()));
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut left: Vec<String> = session
        .sftp
        .read_dir(dir.clone())
        .await
        .expect("list")
        .map(|e| e.file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["b.csv", "c.txt"]);

    let wrong = Server {
        password: "not-the-password".into(),
        address: s.address.clone(),
        user: s.user.clone(),
    };
    let mut bad = input(&wrong, &pin, json!({"paths": [format!("{dir}/*.csv")]}), &c);
    let e = match bad.read_batch(&c).await {
        Ok(_) => panic!("logged in with a wrong password"),
        Err(e) => e.to_string(),
    };
    assert!(
        e.contains("refused the credentials") && !e.contains("not-the-password"),
        "{e}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watcher_takes_each_new_file_once() {
    let Some(s) = server() else {
        eprintln!("SFTP_TEST_ADDRESS unset: skipped");
        return;
    };
    let cache = Arc::new(MemCache::default());
    let c = ctx(cache.clone());
    let pin = pin(&s, &c).await;
    let dir = format!("/upload/watch-{}", std::process::id());
    let session = connect(&target(&s, &pin), &c.org).await.expect("connects");
    session.sftp.create_dir(dir.clone()).await.expect("mkdir");
    put(&s, &pin, &c, &format!("{dir}/d.csv"), "d1\n").await;

    let mut i = input(
        &s,
        &pin,
        json!({
            "paths": [format!("{dir}/*.csv")],
            "watcher": {"enabled": true, "minimum_age": "0s", "poll_interval": "200ms", "cache": "seen"},
        }),
        &c,
    );
    let (b, ack) = i.read_batch(&c).await.expect("d.csv");
    assert_eq!(lines(&b), ["d1"]);
    ack.ack();
    // Nothing new: the next read waits for a file.
    assert!(
        tokio::time::timeout(Duration::from_millis(700), i.read_batch(&c))
            .await
            .is_err()
    );
    put(&s, &pin, &c, &format!("{dir}/e.csv"), "e1\n").await;
    let (b, _ack) = tokio::time::timeout(Duration::from_secs(5), i.read_batch(&c))
        .await
        .expect("e.csv in time")
        .expect("e.csv");
    assert_eq!(lines(&b), ["e1"]);
    assert!(cache.exists(&format!("{dir}/d.csv")).await.expect("cache"));
}

fn private_key_target(s: &Server, pin: &str, key: Option<&str>) -> SftpTarget {
    SftpTarget {
        password: None,
        private_key: key.map(|k| SecretString::new(k.to_string())),
        ..target(s, pin)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_names_missing_undecodable_and_refused_credentials() {
    let Some(s) = server() else {
        eprintln!("SFTP_TEST_ADDRESS unset: skipped");
        return;
    };
    let c = ctx(Arc::default());
    let pin = pin(&s, &c).await;
    fn err<T>(r: Result<T, CoreError>) -> String {
        match r {
            Ok(_) => panic!("connected"),
            Err(e) => e.to_string(),
        }
    }
    let e = err(connect(&private_key_target(&s, &pin, None), &c.org).await);
    assert!(e.contains("no password or private_key"), "{e}");
    let e = err(connect(&private_key_target(&s, &pin, Some("not a key")), &c.org).await);
    assert!(e.contains("credentials.private_key does not decode"), "{e}");
    // a well-formed key the server does not know
    let dir = tempfile::TempDir::new().expect("tmp");
    let path = dir.path().join("id");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&path)
        .status()
        .expect("ssh-keygen runs");
    assert!(made.success());
    let key = std::fs::read_to_string(&path).expect("key");
    let e = err(connect(&private_key_target(&s, &pin, Some(&key)), &c.org).await);
    assert!(e.contains("refused the credentials of user"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_past_the_cap_are_skipped_or_refused_and_an_empty_file_is_passed_over() {
    let Some(s) = server() else {
        eprintln!("SFTP_TEST_ADDRESS unset: skipped");
        return;
    };
    logs();
    let c = ctx(Arc::default());
    let pin = pin(&s, &c).await;
    let dir = format!("/upload/cap-{}", std::process::id());
    let session = connect(&target(&s, &pin), &c.org).await.expect("connects");
    session.sftp.create_dir(dir.clone()).await.expect("mkdir");
    put(&s, &pin, &c, &format!("{dir}/a.csv"), "a1\n").await;
    put(&s, &pin, &c, &format!("{dir}/b.csv"), "b1\n").await;
    put(&s, &pin, &c, &format!("{dir}/big.csv"), "far too long\n").await;
    put(&s, &pin, &c, &format!("{dir}/empty.csv"), "").await;

    let mut i = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/*.csv")], "max_file_bytes": 4, "delete_on_finish": true}),
        &c,
    );
    assert_eq!(i.input_type(), "sftp");
    let (b, ack) = i.read_batch(&c).await.expect("a.csv");
    assert_eq!(lines(&b), ["a1"]);
    assert!(i.is_connected());
    // b.csv grows past the cap between the listing and its read
    put(&s, &pin, &c, &format!("{dir}/b.csv"), "b1 grew\n").await;
    let e = match i.read_batch(&c).await {
        Ok(_) => panic!("read a file past max_file_bytes"),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("is larger than max_file_bytes (4 bytes)"), "{e}");
    // acked from a thread without a runtime: the delete cannot run, the file stays
    std::thread::spawn(move || ack.ack()).join().expect("ack");
    // the empty file is deleted without a batch and the big one never listed
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut left: Vec<String> = session
        .sftp
        .read_dir(dir.clone())
        .await
        .expect("list")
        .map(|e| e.file_name())
        .collect();
    left.sort();
    assert_eq!(left, ["a.csv", "b.csv", "big.csv"]);
    assert!(logged(&format!(
        "{dir}/big.csv (13 bytes) is larger than max_file_bytes (4) and is skipped"
    )));
    i.close().await.expect("closes");
    assert!(!i.is_connected());

    // a listing of a missing directory fails a one-off read
    let mut gone = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/none/*.csv")]}),
        &c,
    );
    let e = match gone.read_batch(&c).await {
        Ok(_) => panic!("listed a missing directory"),
        Err(e) => e.to_string(),
    };
    assert!(
        e.contains(&format!("sftp input: listing {dir}/none")),
        "{e}"
    );

    // without delete_on_finish or a watcher an ack changes nothing on the server
    let mut keep = input(&s, &pin, json!({"paths": [format!("{dir}/a.csv")]}), &c);
    let (_, ack) = keep.read_batch(&c).await.expect("a.csv");
    ack.ack();
    // an ack whose file is gone already only logs the failed delete
    let mut del = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/a.csv")], "delete_on_finish": true}),
        &c,
    );
    let (_, ack) = del.read_batch(&c).await.expect("a.csv again");
    session
        .sftp
        .remove_file(format!("{dir}/a.csv"))
        .await
        .expect("removed");
    ack.ack();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(matches!(
        del.read_batch(&c).await,
        Err(CoreError::EndOfInput)
    ));
    assert!(logged(&format!("deleting {dir}/a.csv after ack failed")));
}

/// A cache whose deletes fail, to show a failed forget is only logged.
#[derive(Default)]
struct StickyCache(MemCache);

#[async_trait]
impl Cache for StickyCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        self.0.get(key).await
    }
    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        self.0.set(key, value, ttl).await
    }
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        self.0.add(key, value, ttl).await
    }
    async fn delete(&self, _: &str) -> Result<(), CoreError> {
        Err(CoreError::other("cache down"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watcher_rides_out_a_vanished_file_and_a_missing_directory() {
    let Some(s) = server() else {
        eprintln!("SFTP_TEST_ADDRESS unset: skipped");
        return;
    };
    logs();
    let cache = Arc::new(MemCache::default());
    let c = ctx(cache.clone());
    let pin = pin(&s, &c).await;
    let dir = format!("/upload/ride-{}", std::process::id());
    let session = connect(&target(&s, &pin), &c.org).await.expect("connects");
    session.sftp.create_dir(dir.clone()).await.expect("mkdir");
    put(&s, &pin, &c, &format!("{dir}/f.csv"), "f1\n").await;
    put(&s, &pin, &c, &format!("{dir}/g.csv"), "g1\n").await;
    let watcher =
        json!({"enabled": true, "minimum_age": "0s", "poll_interval": "100ms", "cache": "seen"});
    let mut i = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/*.csv")], "watcher": watcher}),
        &c,
    );
    let (b, ack) = i.read_batch(&c).await.expect("f.csv");
    assert_eq!(lines(&b), ["f1"]);
    // a nacked file leaves the cache, so a later poll reads it again
    ack.nack(BatchError::new(b, "output down".to_string()));
    // g.csv vanishes between the listing and its read: skipped, forgotten, read never
    session
        .sftp
        .remove_file(format!("{dir}/g.csv"))
        .await
        .expect("removed");
    let (b, _ack) = tokio::time::timeout(Duration::from_secs(5), i.read_batch(&c))
        .await
        .expect("in time")
        .expect("f.csv again");
    assert_eq!(lines(&b), ["f1"]);
    assert!(!cache.exists(&format!("{dir}/g.csv")).await.expect("cache"));
    assert!(logged(&format!("reading {dir}/g.csv")) && logged("reconnecting"));

    // a watched directory that does not exist is listed again and again, never an error
    let sticky: Arc<dyn Cache> = Arc::new(StickyCache::default());
    c.resources
        .insert_cache(&OrgId::new("org-sftp"), "sticky", sticky);
    let mut w = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/none/*.csv")], "watcher": {
            "enabled": true, "minimum_age": "0s", "poll_interval": "100ms", "cache": "sticky"}}),
        &c,
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(700), w.read_batch(&c))
            .await
            .is_err()
    );
    // a nack whose forget fails is only logged
    let mut f = input(
        &s,
        &pin,
        json!({"paths": [format!("{dir}/f.csv")], "watcher": {
            "enabled": true, "minimum_age": "0s", "poll_interval": "100ms", "cache": "sticky"}}),
        &c,
    );
    let (b, ack) = f.read_batch(&c).await.expect("f.csv");
    ack.nack(BatchError::new(b, "output down".to_string()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(400), f.read_batch(&c))
            .await
            .is_err(),
        "the file stays seen: the cache kept it"
    );
    assert!(logged(&format!("listing {dir}/none")));
    assert!(logged(&format!(
        "forgetting {dir}/f.csv after a nack: cache down"
    )));
}

#[tokio::test]
async fn register_all_builds_the_input_and_lints_its_config() {
    let registry = Registry::new();
    ekiben_io_sftp::register_all(&registry);
    let c = ctx(Arc::default());
    let conf = |config: JsonValue| ComponentConfig {
        label: String::new(),
        component_type: "sftp".into(),
        config,
        processors: Vec::new(),
    };
    let good = json!({
        "address": "127.0.0.1:22",
        "host_key_fingerprint": "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "credentials": {"username": "city", "password": "hunter2"},
        "paths": ["/upload/*.csv"],
    });
    assert!(registry
        .lint_component("input", &conf(good.clone()))
        .is_empty());
    let input = registry.build_input(&conf(good), &c).expect("builds");
    assert_eq!(input.input_type(), "sftp");
    let bad = json!({"address": "x", "paths": ["/upload/*.csv"]});
    let errs = registry
        .lint_component("input", &conf(bad.clone()))
        .join("; ");
    assert!(errs.contains("host_key_fingerprint"), "{errs}");
    match registry.build_input(&conf(bad), &c) {
        Ok(_) => panic!("built without a pin"),
        Err(e) => assert!(e.to_string().contains("host_key_fingerprint"), "{e}"),
    }
}

#[tokio::test]
async fn a_watcher_keeps_dialling_a_refused_server_with_a_growing_pause() {
    logs();
    // a port whose listener is gone refuses every connection
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = closed.local_addr().expect("addr").port();
    drop(closed);
    let s = Server {
        address: format!("127.0.0.1:{port}"),
        user: "city".into(),
        password: "hunter2".into(),
    };
    let c = ctx(Arc::default());
    let mut w = input(
        &s,
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        json!({"paths": ["/upload/*.csv"], "watcher": {"enabled": true, "cache": "seen"}}),
        &c,
    );
    let r = tokio::time::timeout(Duration::from_millis(800), w.read_batch(&c)).await;
    assert!(r.is_err(), "the watcher stopped dialling");
    assert!(!w.is_connected());
    assert!(
        logged("connect failed, retrying in 100ms") && logged("connect failed, retrying in 200ms")
    );
}
