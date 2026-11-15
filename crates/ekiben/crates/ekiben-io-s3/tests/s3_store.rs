//! aws_s3 output and input against a real S3-compatible store that checks SigV4 (versitygw in
//! the CI ekiben job). Runs where S3_TEST_ENDPOINT, S3_TEST_ID, S3_TEST_SECRET and S3_TEST_BUCKET
//! name one, as the sql parity cases run where PG_DSN names a database.

use std::collections::HashMap;
use std::sync::Arc;

use ekiben_bloblang::Environment;
use ekiben_core::{
    ComponentConfig, CoreError, Input, OrgId, OrgQuotas, Output, Part, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use ekiben_io_s3::{input::S3Input, output::S3Output};
use serde_json::{json, Value as JsonValue};
use tokio::sync::watch;

struct Store {
    endpoint: String,
    id: String,
    secret: String,
    bucket: String,
}

fn store() -> Option<Store> {
    let v = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    Some(Store {
        endpoint: v("S3_TEST_ENDPOINT")?,
        id: v("S3_TEST_ID")?,
        secret: v("S3_TEST_SECRET")?,
        bucket: v("S3_TEST_BUCKET")?,
    })
}

fn ctx() -> StreamContext {
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-s3");
    // the store listens on loopback, which only an organization with allow_private_ips reaches
    let org = resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        },
    );
    let (_tx, rx) = watch::channel(false);
    StreamContext {
        stream_id: "s3-test".to_string(),
        org,
        resources,
        registry: Arc::new(Registry::new()),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("s3-test", "org-s3"),
        label: String::new(),
        path: "root.output".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    }
}

fn conf(kind: &str, s: &Store, extra: JsonValue, secret: &str) -> ComponentConfig {
    let mut c = json!({
        "bucket": s.bucket,
        "endpoint": s.endpoint,
        "force_path_style_urls": true,
        "region": "us-east-1",
        "credentials": {"id": s.id, "secret": secret},
    });
    if let (Some(c), Some(e)) = (c.as_object_mut(), extra.as_object()) {
        c.extend(e.clone());
    }
    ComponentConfig {
        label: String::new(),
        component_type: kind.to_string(),
        config: c,
        ..Default::default()
    }
}

#[tokio::test]
async fn objects_written_by_the_output_are_read_back_and_deleted_by_the_input() {
    let Some(s) = store() else {
        eprintln!("S3_TEST_ENDPOINT unset: the CI ekiben job runs this against versitygw");
        return;
    };
    let ctx = ctx();
    let run = uuid::Uuid::new_v4().simple().to_string();
    let out = S3Output::from_config(
        &conf(
            "aws_s3",
            &s,
            json!({
                "path": format!("{run}/${{!meta(\"name\")}}.json"),
                "content_type": "application/json",
                "tags": {"source": "ekiben test"},
            }),
            &s.secret,
        ),
        &ctx,
    )
    .expect("output config");
    let mut a = Part::from_str(r#"{"n":1}"#);
    a.meta_set_str("name", "a b");
    let mut b = Part::from_str(r#"{"n":2}"#);
    b.meta_set_str("name", "c$d");
    out.write_batch(&ctx, vec![a, b]).await.expect("put");

    let mut input = S3Input::from_config(&conf(
        "aws_s3",
        &s,
        json!({"prefix": format!("{run}/"), "delete_objects": true, "scanner": {"lines": {}}}),
        &s.secret,
    ))
    .expect("input config");
    let mut seen = Vec::new();
    loop {
        match input.read_batch(&ctx).await {
            Ok((parts, ack)) => {
                for p in &parts {
                    seen.push((
                        p.meta_get_str("s3_key"),
                        p.meta_get_str("s3_content_type"),
                        String::from_utf8_lossy(&p.as_bytes_ref()).to_string(),
                    ));
                }
                ack.ack();
            }
            Err(CoreError::EndOfInput) => break,
            Err(e) => panic!("read: {e}"),
        }
    }
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (
                format!("{run}/a b.json"),
                "application/json".into(),
                r#"{"n":1}"#.into()
            ),
            (
                format!("{run}/c$d.json"),
                "application/json".into(),
                r#"{"n":2}"#.into()
            ),
        ]
    );

    // delete_objects ran after the acks: a second listing finds nothing
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut again = S3Input::from_config(&conf(
        "aws_s3",
        &s,
        json!({"prefix": format!("{run}/")}),
        &s.secret,
    ))
    .unwrap();
    assert!(matches!(
        again.read_batch(&ctx).await,
        Err(CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn a_wrong_secret_is_refused_by_the_store() {
    let Some(s) = store() else {
        eprintln!("S3_TEST_ENDPOINT unset: the CI ekiben job runs this against versitygw");
        return;
    };
    let ctx = ctx();
    let out = S3Output::from_config(
        &conf("aws_s3", &s, json!({"path": "x.txt"}), "not-the-secret"),
        &ctx,
    )
    .unwrap();
    let e = out
        .write_batch(&ctx, vec![Part::from_str("x")])
        .await
        .expect_err("refused");
    assert!(e.to_string().contains("403"), "{e}");
}
