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
    ] {
        let errs = lint(&with(extra.clone()));
        let e = errs.join("; ");
        assert!(e.contains(want), "{extra}: {e}");
        assert!(!e.contains("hunter2"), "{e}");
    }
    assert!(lint(&base()).is_empty(), "{:?}", lint(&base()));
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
