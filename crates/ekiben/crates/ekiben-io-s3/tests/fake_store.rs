//! aws_s3 against a scripted store on loopback: the registry, config refusals, paging, size caps,
//! error answers, a cut-off body and delete-after-ack. The store answers whatever the test's
//! script returns for a request and records each request line and its headers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ekiben_bloblang::Environment;
use ekiben_core::{
    BatchError, ComponentConfig, CoreError, OrgId, OrgQuotas, Part, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use serde_json::{json, Value as JsonValue};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

type Script = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// One recorded request: its request line and its lowercased header lines.
type Seen = Arc<Mutex<Vec<String>>>;

/// Serves `script(method, path)` raw HTTP answers, one connection per request.
async fn store(script: Script) -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen: Seen = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let script = script.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let line = head.lines().next().unwrap_or("").to_string();
                let mut words = line.split(' ');
                let method = words.next().unwrap_or("").to_uppercase();
                let path = words.next().unwrap_or("").to_string();
                log.lock().expect("lock").push(head);
                let answer = script(&method, &path);
                let _ = sock.write_all(answer.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nLast-Modified: Fri, 24 May 2013 00:00:00 GMT\r\nx-amz-meta-origin: fake\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn ctx() -> StreamContext {
    let resources = ResourceManager::new();
    let org_id = OrgId::new("org-fake-s3");
    // the store listens on loopback, which only an organization with allow_private_ips reaches
    let org = resources.set_org_quotas(
        &org_id,
        OrgQuotas {
            allow_private_ips: true,
            ..Default::default()
        },
    );
    let registry = Registry::new();
    ekiben_io_s3::register_all(&registry);
    StreamContext {
        stream_id: "fake-s3".to_string(),
        org,
        resources,
        registry: Arc::new(registry),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("fake-s3", "org-fake-s3"),
        label: String::new(),
        path: "root.output".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: watch::channel(false).1,
    }
}

fn conf(label: &str, config: JsonValue) -> ComponentConfig {
    ComponentConfig {
        label: label.to_string(),
        component_type: "aws_s3".to_string(),
        config,
        ..Default::default()
    }
}

fn base(endpoint: &str, extra: JsonValue) -> JsonValue {
    let mut c = json!({
        "bucket": "raw",
        "endpoint": endpoint,
        "force_path_style_urls": true,
        "credentials": {"id": "AK", "secret": "SK", "token": "session"},
    });
    if let (Some(c), Some(e)) = (c.as_object_mut(), extra.as_object()) {
        c.extend(e.clone());
    }
    c
}

fn refusal(r: Result<impl Sized, CoreError>) -> String {
    match r {
        Ok(_) => panic!("expected a refusal"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn the_registry_builds_both_and_refuses_bad_config_by_field() {
    let ctx = ctx();
    let reg = ctx.registry.clone();
    let input = reg
        .build_input(&conf("in", base("http://127.0.0.1:9", json!({}))), &ctx)
        .expect("input");
    assert_eq!((input.input_type(), input.label()), ("aws_s3", "in"));
    let output = reg
        .build_output(&conf("out", base("http://127.0.0.1:9", json!({}))), &ctx)
        .expect("output");
    assert_eq!((output.output_type(), output.label()), ("aws_s3", "out"));
    // credentials without id and secret sign nothing and still build
    assert!(reg
        .build_input(&conf("", json!({"bucket": "raw", "credentials": {}})), &ctx)
        .is_ok());

    let input_errors = [
        (json!("raw"), "aws_s3 input config must be an object"),
        (
            json!({"region": "eu-west-1"}),
            "aws_s3 input: field bucket is required",
        ),
        (
            json!({"bucket": "raw", "sqs": {"url": "https://sqs.example.org/q"}}),
            "sqs is not supported",
        ),
        (
            json!({"bucket": "raw", "max_object_bytes": 0}),
            "max_object_bytes must be 1 to",
        ),
        (
            json!({"bucket": "raw", "max_object_bytes": u64::MAX}),
            "max_object_bytes must be 1 to",
        ),
        (
            json!({"bucket": "raw", "endpoint": "not a url"}),
            "endpoint must be an http(s) URL",
        ),
    ];
    for (config, want) in input_errors {
        let e = refusal(reg.build_input(&conf("", config.clone()), &ctx));
        assert!(e.contains(want), "{config}: {e}");
    }
    let output_errors = [
        (json!(["raw"]), "aws_s3 output config must be an object"),
        (
            json!({"bucket": "raw", "tags": {"team": 7}}),
            "aws_s3 output: tags.team must be a string",
        ),
        (json!({"bucket": "raw", "timeout": "soon"}), "soon"),
    ];
    for (config, want) in output_errors {
        let e = refusal(reg.build_output(&conf("", config.clone()), &ctx));
        assert!(e.contains(want), "{config}: {e}");
    }
}

#[tokio::test]
async fn the_output_sends_headers_tags_and_kept_metadata_and_names_what_fails() {
    let (endpoint, seen) = store(Arc::new(|_, _| ok(""))).await;
    let ctx = ctx();
    let out = ctx
        .registry
        .build_output(
            &conf(
                "",
                base(
                    &endpoint,
                    json!({
                        "path": "${! meta(\"name\") }",
                        "content_type": "text/csv",
                        "server_side_encryption": "aws:kms",
                        "tags": {"team": "city ops"},
                        "metadata": {"exclude_prefixes": ["skip_"]},
                    }),
                ),
            ),
            &ctx,
        )
        .expect("output");
    let mut part = Part::from_str("a,b");
    part.meta_set_str("name", "x/y.csv");
    part.meta_set_str("keep", "yes");
    part.meta_set_str("skip_me", "no");
    part.meta_set_str("bell", "a\u{7}b");
    out.write_batch(&ctx, vec![part]).await.expect("put");
    let head = seen.lock().expect("lock").join("\n");
    assert!(head.starts_with("put /raw/x/y.csv "), "{head}");
    for want in [
        "content-type: text/csv",
        "x-amz-storage-class: standard",
        "x-amz-server-side-encryption: aws:kms",
        "x-amz-tagging: team=city%20ops",
        "x-amz-meta-keep: yes",
        "x-amz-security-token: session",
        "authorization: aws4-hmac-sha256",
    ] {
        assert!(head.contains(want), "{want} missing:\n{head}");
    }
    // excluded by prefix, a value no header carries, and unset kms_key_id stay off the PUT
    for absent in ["x-amz-meta-skip_me", "x-amz-meta-bell", "kms-key-id"] {
        assert!(!head.contains(absent), "{absent}:\n{head}");
    }

    let mut root = Part::from_str("x");
    root.meta_set_str("name", "/");
    let e = refusal(out.write_batch(&ctx, vec![root]).await);
    assert_eq!(e, "aws_s3 output: path resolved to an empty key");
    let bad = ctx
        .registry
        .build_output(
            &conf(
                "",
                base(&endpoint, json!({"path": "${! throw(\"no key\") }"})),
            ),
            &ctx,
        )
        .expect("output");
    let e = refusal(bad.write_batch(&ctx, vec![Part::from_str("x")]).await);
    assert!(e.contains("aws_s3 output: path interpolation error"), "{e}");
}

#[tokio::test]
async fn the_input_pages_through_the_listing_and_deletes_only_what_was_acked() {
    let deletes: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = deletes.clone();
    let (endpoint, _) = store(Arc::new(move |method, path| {
        match (method, path) {
            ("GET", p) if p.contains("continuation-token=t1") => ok(
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>b.txt</Key></Contents><Contents><Key>c.txt</Key></Contents></ListBucketResult>",
            ),
            ("GET", p) if p.contains("list-type=2") => ok(
                "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>dir/</Key></Contents><Contents><Key>a.txt</Key></Contents><Contents><Key>empty.txt</Key></Contents><NextContinuationToken>t1</NextContinuationToken></ListBucketResult>",
            ),
            ("GET", "/raw/empty.txt") => ok(""),
            ("GET", p) => ok(&format!("body of {p}")),
            ("DELETE", p) => {
                log.lock().expect("lock").push(p.to_string());
                if p.ends_with("c.txt") {
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                } else {
                    "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string()
                }
            }
            _ => "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
        }
    }))
    .await;
    let ctx = ctx();
    let mut input = ctx
        .registry
        .build_input(
            // lines: the empty object gives no message and is passed over
            &conf(
                "",
                base(
                    &endpoint,
                    json!({"delete_objects": true, "scanner": {"lines": {}}}),
                ),
            ),
            &ctx,
        )
        .expect("input");
    let mut keys = Vec::new();
    for i in 0..3 {
        let (parts, ack) = input.read_batch(&ctx).await.expect("read");
        let p = &parts[0];
        assert_eq!(p.meta_get_str("s3_bucket"), "raw");
        assert_eq!(p.meta_get_str("s3_last_modified_unix"), "1369353600");
        assert_eq!(p.meta_get_str("origin"), "fake");
        let key = p.meta_get_str("s3_key");
        assert_eq!(
            String::from_utf8_lossy(&p.as_bytes_ref()),
            format!("body of /raw/{key}")
        );
        keys.push(key);
        // the first object is nacked and stays; the others are acked and deleted
        if i == 0 {
            ack.nack(BatchError::new(parts, "downstream refused"));
        } else {
            ack.ack();
        }
    }
    assert_eq!(keys, ["a.txt", "b.txt", "c.txt"]);
    assert!(matches!(
        input.read_batch(&ctx).await,
        Err(CoreError::EndOfInput)
    ));
    let mut gone = Vec::new();
    for _ in 0..50 {
        gone = deletes.lock().expect("lock").clone();
        if gone.len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    gone.sort();
    assert_eq!(gone, ["/raw/b.txt", "/raw/c.txt"]);
}

#[tokio::test]
async fn an_anonymous_read_is_unsigned_and_an_ack_without_a_runtime_deletes_nothing() {
    let (endpoint, seen) = store(Arc::new(|method, path| match (method, path) {
        ("GET", p) if p.contains("list-type=2") => ok(
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>a.txt</Key></Contents></ListBucketResult>",
        ),
        ("GET", _) => ok("a1"),
        _ => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string(),
    }))
    .await;
    let ctx = ctx();
    let mut config = base(&endpoint, json!({"delete_objects": true}));
    config
        .as_object_mut()
        .expect("object")
        .remove("credentials");
    let mut input = ctx
        .registry
        .build_input(&conf("", config), &ctx)
        .expect("input");
    let (parts, ack) = input.read_batch(&ctx).await.expect("read");
    assert_eq!(String::from_utf8_lossy(&parts[0].as_bytes_ref()), "a1");
    // an ack answered on a thread with no runtime cannot send the delete
    std::thread::spawn(move || ack.ack()).join().expect("ack");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let seen = seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen.iter().all(|r| r.starts_with("get ")), "{seen:?}");
    assert!(
        seen.iter().all(|r| !r.contains("authorization:")),
        "{seen:?}"
    );
}

#[tokio::test]
async fn caps_error_answers_and_cut_bodies_are_refused_with_the_object_named() {
    let (endpoint, _) = store(Arc::new(|_, path| match path {
        p if p.contains("list-type=2") => ok(
            "<ListBucketResult><Contents><Key>big.txt</Key></Contents><Contents><Key>cut.txt</Key></Contents><Contents><Key>gone.txt</Key></Contents></ListBucketResult>",
        ),
        "/raw/big.txt" => ok(&"x".repeat(64)),
        "/raw/cut.txt" => {
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nonly ten b".to_string()
        }
        _ => {
            let body = "<Error><Code>NoSuchKey</Code></Error>";
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    }))
    .await;
    let ctx = ctx();
    let mut input = ctx
        .registry
        .build_input(
            &conf("", base(&endpoint, json!({"max_object_bytes": 16}))),
            &ctx,
        )
        .expect("input");
    let e = refusal(input.read_batch(&ctx).await);
    assert!(
        e.contains("s3://raw/big.txt") && e.contains("over the 16 bytes"),
        "{e}"
    );
    let e = refusal(input.read_batch(&ctx).await);
    assert!(
        e.contains("s3://raw/cut.txt") && e.contains("reading the answer"),
        "{e}"
    );
    let e = refusal(input.read_batch(&ctx).await);
    assert!(
        e.contains("GET s3://raw/gone.txt answered 404 Not Found NoSuchKey"),
        "{e}"
    );

    // a header value HTTP cannot carry is refused before anything is sent
    let out = ctx
        .registry
        .build_output(
            &conf(
                "",
                base(&endpoint, json!({"path": "k", "cache_control": "a\u{7}b"})),
            ),
            &ctx,
        )
        .expect("output");
    let e = refusal(out.write_batch(&ctx, vec![Part::from_str("x")]).await);
    assert!(
        e.contains("header cache-control has a value HTTP cannot carry"),
        "{e}"
    );

    // nothing listens on the discard port: the send fails and names the object
    let closed = ctx
        .registry
        .build_output(
            &conf("", base("http://127.0.0.1:9", json!({"path": "k"}))),
            &ctx,
        )
        .expect("output");
    let e = refusal(closed.write_batch(&ctx, vec![Part::from_str("x")]).await);
    assert!(e.starts_with("aws_s3: PUT k: "), "{e}");
}

/// The read caps hold at their sizes: an object of exactly the cap, a 2 MiB listing page, a
/// 4 KiB error answer and the 256 MiB max_object_bytes ceiling; a page that says it is not
/// truncated ends the listing even with a continuation token.
#[tokio::test]
async fn read_caps_hold_at_their_exact_sizes() {
    let padding = " ".repeat(2 << 20);
    let (endpoint, _) = store(Arc::new(move |_, path| match path {
        p if p.contains("continuation-token") => ok(
            "<ListBucketResult><Contents><Key>never.txt</Key></Contents></ListBucketResult>",
        ),
        p if p.contains("list-type=2") => ok(&format!(
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>exact.txt</Key></Contents><Contents><Key>gone.txt</Key></Contents><NextContinuationToken>t9</NextContinuationToken>{padding}</ListBucketResult>"
        )),
        "/raw/exact.txt" => ok(&"x".repeat(16)),
        _ => {
            let body = format!("<Error>{}<Code>NoSuchKey</Code></Error>", " ".repeat(4096));
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    }))
    .await;
    let ctx = ctx();
    let mut input = ctx
        .registry
        .build_input(
            &conf("", base(&endpoint, json!({"max_object_bytes": 16}))),
            &ctx,
        )
        .expect("input");
    let (parts, _) = input
        .read_batch(&ctx)
        .await
        .expect("exactly the cap is read");
    assert_eq!(parts[0].as_bytes_ref().len(), 16);
    let e = refusal(input.read_batch(&ctx).await);
    assert!(e.ends_with("answered 404 Not Found NoSuchKey"), "{e}");
    assert!(matches!(
        input.read_batch(&ctx).await,
        Err(CoreError::EndOfInput)
    ));

    for (bytes, ok) in [(256u64 << 20, true), ((256 << 20) + 1, false)] {
        let built = ctx.registry.build_input(
            &conf("", base(&endpoint, json!({"max_object_bytes": bytes}))),
            &ctx,
        );
        assert_eq!(built.is_ok(), ok, "{bytes}");
    }
}
