//! `/apps/{name}/api/services/files…`: an App that is not `wasm` keeps objects under its own
//! prefix `apps/<shard>/<id>/`, never outside it, each at most 25 MiB and all within `filesMiB`
//! (AP-170, AP-145, API/06 §3, T-3584). The store is a mock that answers like S3.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::envelope;
use joinedcontext_portal::apps::apps_db::{app_id, AppsDb};
use joinedcontext_portal::apps::wasm_apps::WasmApps;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;

const APP: &str = "road-defects";
const PROJECT: &str = "roads";

fn prefix() -> String {
    format!("apps/0/{}/", app_id(PROJECT, APP))
}

/// A Portal with App `road-defects` (a `ui` App with `files` and a 1 MiB quota) whose store is
/// `store`.
fn state(store: &MockServer, services: Value) -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_APPS_STORE_ORIGIN" => Some("https://files.dev.example"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let mut state = AppState::new(config, None);
    let connect: sqlx::postgres::PgConnectOptions = "postgres://nobody@127.0.0.1:1/none"
        .parse()
        .expect("options");
    state.wasm_apps = Some(std::sync::Arc::new(WasmApps {
        db: AppsDb {
            admin: sqlx::postgres::PgPoolOptions::new().connect_lazy_with(connect.clone()),
            connect,
            shards: 2,
        },
        store: std::sync::Arc::new(
            joinedcontext_portal::artifact_store::Client::new(
                joinedcontext_portal::artifact_store::Settings {
                    endpoint: store.uri(),
                    bucket: "jc-artifacts".into(),
                    region: "us-east-1".into(),
                    root_access_key: "root".into(),
                    root_secret_key: "root-secret".into(),
                },
            )
            .expect("store client"),
        ),
        bucket: "apps".into(),
    }));
    state.mirror.upsert(envelope(
        "Organization",
        "helsinki",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    state.mirror.upsert(envelope(
        "App",
        APP,
        PROJECT,
        json!({
            "kind": "ui", "source": { "path": "." }, "build": {}, "visibility": "project",
            "lifecycle": "published", "dataNeeds": [], "services": services,
            "limits": { "filesMiB": 1 },
        }),
    ));
    common::bind(&state.mirror, "jana", PROJECT);
    state
}

/// A store holding `held` bytes under the App's prefix in one object, `old.txt`.
async fn store(held: u64) -> MockServer {
    let store = MockServer::start().await;
    let listing = format!(
        "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{}old.txt</Key>\
         <Size>{held}</Size><LastModified>2026-10-10T20:00:00.000Z</LastModified></Contents>\
         </ListBucketResult>",
        prefix()
    );
    Mock::given(method("GET"))
        .and(path("/apps"))
        .and(query_param("list-type", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(listing))
        .mount(&store)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200).insert_header("content-type", "text/plain"))
        .mount(&store)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/apps/{}old.txt", prefix())))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string("held"),
        )
        .mount(&store)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&store)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&store)
        .await;
    store
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Vec<u8>,
) -> (StatusCode, Value, String) {
    let token = common::REALM.person_token(APP, "jana", &[]);
    let request = Request::builder()
        .method(method)
        .uri(format!("/apps/{APP}/api/services/files{uri}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(Body::from(body))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

/// The keys the store was asked for, as paths.
async fn asked(store: &MockServer, verb: &str) -> Vec<String> {
    store
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() == verb)
        .map(|request| request.url.path().to_owned())
        .collect()
}

#[tokio::test]
async fn an_object_goes_under_the_apps_prefix_and_comes_back_listed_and_read() {
    let store = store(4).await;
    let state = state(&store, json!(["files"]));

    let (status, body, text) = call(&state, "PUT", "/defects/1.jpg", b"jpeg".to_vec()).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    assert_eq!(
        (
            body["path"].as_str(),
            body["size"].as_u64(),
            body["contentType"].as_str()
        ),
        (Some("defects/1.jpg"), Some(4), Some("image/jpeg"))
    );
    assert_eq!(
        asked(&store, "PUT").await,
        vec![format!("/apps/{}defects/1.jpg", prefix())]
    );

    let (status, body, text) = call(&state, "GET", "?prefix=", Vec::new()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        body,
        json!([{ "path": "old.txt", "size": 4, "contentType": "text/plain",
                 "modifiedAt": "2026-10-10T20:00:00.000Z" }])
    );

    let (status, _, text) = call(&state, "GET", "/old.txt", Vec::new()).await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "held"));

    let (status, _, _) = call(&state, "DELETE", "/old.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        asked(&store, "DELETE").await,
        vec![format!("/apps/{}old.txt", prefix())]
    );
}

#[tokio::test]
async fn a_key_outside_the_prefix_never_reaches_the_store() {
    let store = store(0).await;
    let state = state(&store, json!(["files"]));
    for key in ["/..%2Fother%2Fx", "/a%2F..%2F..%2Fb", "/a%5Cb"] {
        let (status, _, text) = call(&state, "PUT", key, b"x".to_vec()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {text}");
    }
    let (status, _, text) = call(&state, "GET", "?prefix=../", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(asked(&store, "PUT").await.is_empty());
}

#[tokio::test]
async fn an_object_over_25_mib_or_past_files_mib_is_refused() {
    let store = store(1024 * 1024 - 2).await;
    let state = state(&store, json!(["files"]));
    let (status, body, text) = call(
        &state,
        "PUT",
        "/big.bin",
        vec![0; joinedcontext_portal::apps::files::MAX_OBJECT_BYTES + 1],
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
    assert_eq!(
        body["type"],
        "https://joinedcontext.com/errors/file-too-large"
    );

    let (status, body, text) = call(&state, "PUT", "/three.txt", b"abc".to_vec()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(
        (body["service"].as_str(), body["quota"].as_str()),
        (Some("files"), Some("filesMiB"))
    );
    assert!(
        body.get("resetAt").is_none(),
        "a held quota names no reset: {text}"
    );
    // Replacing the object that holds the bytes is within the quota.
    let (status, _, text) = call(&state, "PUT", "/old.txt", b"abc".to_vec()).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
}

#[tokio::test]
async fn a_url_names_one_key_and_one_method_on_the_public_origin_for_five_minutes() {
    let store = store(0).await;
    let state = state(&store, json!(["files"]));
    let (status, body, text) = call(
        &state,
        "POST",
        "/defects/1.jpg:url",
        json!({ "method": "PUT", "size": 1000 })
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let url = body["url"].as_str().unwrap_or_default();
    assert!(
        url.starts_with(&format!(
            "https://files.dev.example/apps/{}defects/1.jpg?",
            prefix()
        )) && url.contains("X-Amz-Expires=300")
            && url.contains("X-Amz-SignedHeaders=content-length%3Bhost"),
        "the PUT's length is signed: {url}"
    );
    let (status, body, text) = call(
        &state,
        "POST",
        "/defects/1.jpg:url",
        json!({ "method": "GET" }).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        body["url"]
            .as_str()
            .is_some_and(|url| url.contains("X-Amz-SignedHeaders=host&")),
        "{text}"
    );
    let (status, _, _) = call(
        &state,
        "POST",
        "/defects/1.jpg:url",
        json!({ "method": "DELETE" }).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_app_without_files_is_refused_naming_the_app() {
    let store = store(0).await;
    let state = state(&store, json!([]));
    let (status, body, text) = call(&state, "GET", "/old.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(
        (body["service"].as_str(), body["layer"].as_str()),
        (Some("files"), Some("app"))
    );
    assert!(store
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// T-3661: a PUT URL is issued only for a size the object and the App's `filesMiB` hold, and the
/// store, which the URL signs that size for, refuses any other.
#[tokio::test]
async fn a_put_url_is_held_to_25_mib_and_to_what_files_mib_has_left() {
    let store = store(1024 * 1024 - 100).await;
    let state = state(&store, json!(["files"]));
    let ask = |body: Value| {
        call(
            &state,
            "POST",
            "/defects/1.jpg:url",
            body.to_string().into_bytes(),
        )
    };

    let (status, body, text) = ask(json!({ "method": "PUT", "size": 25 * 1024 * 1024 + 1 })).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
    assert_eq!(
        body["type"],
        "https://joinedcontext.com/errors/file-too-large"
    );

    let (status, body, text) = ask(json!({ "method": "PUT", "size": 101 })).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(body["quota"], "filesMiB", "{text}");
    let (status, _, text) = ask(json!({ "method": "PUT", "size": 100 })).await;
    assert_eq!(status, StatusCode::OK, "the last 100 bytes fit: {text}");

    for (body, why) in [
        (json!({ "method": "PUT" }), "size"),
        (json!({ "method": "GET", "size": 1 }), "size"),
        (json!({ "method": "PUT", "size": -1 }), "body"),
    ] {
        let (status, _, text) = ask(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {text}");
        assert!(text.contains(why), "{body}: {text}");
    }
}

/// T-4910: a person of another project calling a project App's service gets the 404 of a name never
/// created, and the store is never asked.
#[tokio::test]
async fn a_member_of_another_project_reaches_no_service_of_a_project_app() {
    let store = store(4).await;
    let state = state(&store, json!(["files"]));
    // Jana's one binding now covers project `parks`, not the App's.
    state.mirror.upsert(envelope(
        "RoleBinding",
        &format!("jana-in-{PROJECT}"),
        "org",
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "app-readers",
                "scope": { "project": "parks" } }),
    ));

    let (status, _, text) = call(&state, "GET", "?prefix=", Vec::new()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert!(
        state.mirror.get(PROJECT, "App", APP).is_some(),
        "the App is there"
    );
    assert!(
        asked(&store, "GET").await.is_empty(),
        "the store is never asked"
    );
}

/// F-POR2-016: concurrent PUTs that each fit `filesMiB` alone but not together: the quota check
/// and the write run one App upload at a time, so the stored total stays within the quota.
#[tokio::test]
async fn concurrent_puts_never_pass_files_mib_together() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    let held: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
    let store = MockServer::start().await;
    let listed = held.clone();
    Mock::given(method("GET"))
        .and(path("/apps"))
        .and(query_param("list-type", "2"))
        .respond_with(move |_: &wiremock::Request| {
            let contents: String = listed
                .lock()
                .expect("held")
                .iter()
                .map(|(key, size)| {
                    format!("<Contents><Key>{key}</Key><Size>{size}</Size><LastModified>2026-10-10T20:00:00.000Z</LastModified></Contents>")
                })
                .collect();
            ResponseTemplate::new(200).set_body_string(format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
            ))
        })
        .mount(&store)
        .await;
    let stored = held.clone();
    Mock::given(method("PUT"))
        .respond_with(move |request: &wiremock::Request| {
            let key = request.url.path().trim_start_matches("/apps/").to_owned();
            stored.lock().expect("held").insert(key, request.body.len());
            ResponseTemplate::new(200).set_delay(std::time::Duration::from_millis(200))
        })
        .mount(&store)
        .await;
    let state = state(&store, json!(["files"]));
    // One call first: the person's token is verified once before the burst (see chyby.md).
    let (status, _, text) = call(&state, "PUT", "/warm.txt", b"a".to_vec()).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let part = vec![0; 400 * 1024];
    let (one, two, three) = tokio::join!(
        call(&state, "PUT", "/one.bin", part.clone()),
        call(&state, "PUT", "/two.bin", part.clone()),
        call(&state, "PUT", "/three.bin", part.clone())
    );
    let created = [&one, &two, &three]
        .iter()
        .filter(|answer| answer.0 == StatusCode::CREATED)
        .count();
    assert_eq!(created, 2, "{} / {} / {}", one.2, two.2, three.2);
    let total: usize = held.lock().expect("held").values().sum();
    assert!(total <= 1024 * 1024, "{total} bytes stored past filesMiB 1");
}
