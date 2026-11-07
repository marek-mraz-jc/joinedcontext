//! The workbench's sample of an MQTT source on the real runner and a real broker (T-4841): the
//! configuration `stream_sample` renders subscribes to the saved source's topic and posts each
//! message whole, a JSON message parsed, a text message as text and one past the per-message
//! limit as an error with its size; a quiet topic and a broker that refuses the source's login
//! post nothing. Runs against the pinned Bento in streams mode (see pipeline_runner_stop_tests.rs)
//! and two brokers it reaches by name, as a saved source names its broker: an anonymous one at
//! `JC_PORTAL_TEST_MQTT_URL` and one that takes `jc-sample` / `sample-pass` only at
//! `JC_PORTAL_TEST_MQTT_AUTH_URL` (ci-full.yml starts both and names them for the runner). The
//! messages are published by streams on the same runner; the capture route is a mock in this process.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::routing::post;
use axum::Router;
use jc_core::kinds::data_source::DataSourceSpec;
use serde_json::{json, Value};

struct Env {
    runner: String,
    anonymous: String,
    login: String,
}

fn env() -> Option<Env> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_owned())
            .filter(|v| !v.is_empty())
    };
    Some(Env {
        runner: var("JC_PORTAL_TEST_BENTO_URL")?,
        anonymous: var("JC_PORTAL_TEST_MQTT_URL")?,
        login: var("JC_PORTAL_TEST_MQTT_AUTH_URL")?,
    })
}

type Seen = Arc<Mutex<Vec<Value>>>;

/// The capture route: every envelope the sample stream posts.
async fn capture() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    let app = Router::new().route(
        "/capture",
        post(move |body: axum::body::Bytes| {
            let kept = Arc::clone(&kept);
            async move {
                if let Ok(message) = serde_json::from_slice(&body) {
                    kept.lock().expect("the envelopes").push(message);
                }
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("the address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/capture"), seen)
}

/// The test runner holds no token: the capture post goes out without one.
fn without_token(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if let Some(oauth) = fields.get_mut("oauth2").and_then(Value::as_object_mut) {
                oauth.insert("enabled".to_owned(), json!(false));
            }
            fields.values_mut().for_each(without_token);
        }
        Value::Array(items) => items.iter_mut().for_each(without_token),
        _ => {}
    }
}

fn unique(what: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!(
        "t4841-{what}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

async fn create(client: &reqwest::Client, runner: &str, name: &str, config: &Value) {
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(config)
        .send()
        .await
        .expect("the runner answers");
    let status = created.status();
    assert!(
        status.is_success(),
        "the runner takes {name}: {status} {}",
        created.text().await.unwrap_or_default()
    );
}

async fn remove(client: &reqwest::Client, runner: &str, names: impl Iterator<Item = &String>) {
    for name in names {
        let _ = client
            .delete(format!("{runner}/streams/{name}"))
            .send()
            .await;
    }
}

/// Listens to `topic` of the broker at `url` as a saved source would for five seconds, while
/// `publish` (Bloblang mappings, one message each) is published there; the envelopes posted.
async fn listen(
    env: &Env,
    url: &str,
    login: Option<&str>,
    topic: &str,
    publish: &[&str],
) -> Vec<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let (capture_url, seen) = capture().await;
    let mut mqtt = json!({ "urls": [url], "topics": [topic] });
    if let Some(user) = login {
        mqtt["username"] = json!(user);
    }
    let source: DataSourceSpec =
        serde_json::from_value(json!({ "type": "mqtt", "mqtt": mqtt })).expect("a source");
    // The client id carries the project and the capture path's last segment, the Portal's test id;
    // this mock's path is one for all, so the project keeps the reads of tests running at once
    // apart: a broker drops one of two sessions that share an id, with the messages it was sent.
    let project = unique("project");
    let mut config = joinedcontext_portal::api::datasource_test::stream_sample(
        "hsl-hfp",
        &project,
        &source,
        &capture_url,
    )
    .expect("a sample stream");
    without_token(&mut config);
    let listener = unique("listen");
    create(&client, &env.runner, &listener, &config).await;
    // Subscribed before anything is published: a clean session gets no message sent before it.
    // Where one is expected, a probe is published until it arrives; a busy runner took over 1.5 s.
    let mut publishers = Vec::new();
    if login.is_none() && !publish.is_empty() {
        let ready = Instant::now() + Duration::from_secs(20);
        while seen.lock().expect("envelopes").is_empty() && Instant::now() < ready {
            let name = unique("probe");
            let probe = json!({
                "input": { "generate": { "count": 1, "interval": "", "mapping": "root = \"probe\"" } },
                "output": { "mqtt": { "urls": [url], "topic": topic, "client_id": name, "qos": 1 } },
            });
            create(&client, &env.runner, &name, &probe).await;
            publishers.push(name);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let subscribed = !seen.lock().expect("envelopes").is_empty();
        if !subscribed {
            remove(&client, &env.runner, publishers.iter().chain([&listener])).await;
            panic!("the sample stream never subscribed");
        }
        seen.lock().expect("envelopes").clear();
    } else {
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    for mapping in publish {
        let name = unique("publish");
        let mut output = json!({ "urls": [url], "topic": topic, "client_id": name, "qos": 1 });
        // QoS 1: a stream that ends after its one message still waits for the broker's ack.
        if login.is_some() {
            // The publisher logs in properly; only the listening source's login is wrong.
            output["user"] = json!("jc-sample");
            output["password"] = json!("sample-pass");
        }
        let publisher = json!({
            "input": { "generate": { "count": 1, "interval": "", "mapping": mapping } },
            "output": { "mqtt": output },
        });
        create(&client, &env.runner, &name, &publisher).await;
        publishers.push(name);
    }
    // Nothing published: the Portal's five seconds. Something published: until it all arrived,
    // within a bound that a runner busy creating streams stays inside.
    let wait = if publish.is_empty() || login.is_some() {
        5
    } else {
        20
    };
    let deadline = Instant::now() + Duration::from_secs(wait);
    while Instant::now() < deadline
        && (publish.is_empty()
            || login.is_some()
            || seen.lock().expect("envelopes").len() < publish.len())
    {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    remove(&client, &env.runner, publishers.iter().chain([&listener])).await;
    let seen = seen.lock().expect("envelopes").clone();
    seen
}

#[tokio::test]
async fn each_message_of_the_topic_is_one_record_and_an_oversized_one_is_named() {
    let Some(env) = env() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL or JC_PORTAL_TEST_MQTT_* is not set");
        return;
    };
    let topic = format!("{}/vp", unique("hfp"));
    let anonymous = env.anonymous.clone();
    let seen = listen(
        &env,
        &anonymous,
        None,
        &topic,
        &[
            r#"root = { "VP": { "veh": 12, "lat": 60.17 } }"#,
            r#"root = "plain text""#,
            r#"root = range(0, 70000).map_each("x").join("")"#,
        ],
    )
    .await;
    assert_eq!(seen.len(), 3, "{seen:?}");
    let json = seen
        .iter()
        .find(|e| e["output"].is_object())
        .expect("the JSON message");
    assert_eq!(json["output"]["VP"]["veh"], 12, "{json}");
    assert!(json["error"].is_null(), "{json}");
    assert!(
        seen.iter().any(|e| e["output"] == "plain text"),
        "a text message stays text: {seen:?}"
    );
    let oversized = seen
        .iter()
        .find(|e| e["error"].is_string())
        .expect("the oversized message");
    assert!(
        oversized["error"]
            .as_str()
            .is_some_and(|m| m.contains("70000 bytes")),
        "{oversized}"
    );
    assert_eq!(oversized["input"], "", "its bytes are not posted");
    assert!(oversized.get("output").is_none(), "{oversized}");
}

#[tokio::test]
async fn a_quiet_topic_posts_nothing_in_five_seconds() {
    let Some(env) = env() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL or JC_PORTAL_TEST_MQTT_* is not set");
        return;
    };
    let topic = format!("{}/vp", unique("quiet"));
    let anonymous = env.anonymous.clone();
    assert_eq!(
        listen(&env, &anonymous, None, &topic, &[]).await,
        Vec::<Value>::new()
    );
}

#[tokio::test]
async fn a_broker_that_refuses_the_sources_login_gives_no_message() {
    let Some(env) = env() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL or JC_PORTAL_TEST_MQTT_* is not set");
        return;
    };
    let topic = format!("{}/vp", unique("refused"));
    let login = env.login.clone();
    // The source names a user and no password: the broker refuses it, while the publisher's own
    // login lands its message on the topic.
    let seen = listen(
        &env,
        &login,
        Some("jc-sample"),
        &topic,
        &[r#"root = { "VP": { "veh": 1 } }"#],
    )
    .await;
    assert_eq!(seen, Vec::<Value>::new());
}
