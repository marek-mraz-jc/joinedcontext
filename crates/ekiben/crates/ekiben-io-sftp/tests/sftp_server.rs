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
