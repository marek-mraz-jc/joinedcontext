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
    state_at(store.uri(), services)
}

/// [`state`] with its store at `endpoint`.
fn state_at(endpoint: String, services: Value) -> AppState {
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
                    endpoint,
                    bucket: "jc-artifacts".into(),
                    region: "us-east-1".into(),
                    root_access_key: "root".into(),
                    root_secret_key: "root-secret".into(),
                },
            )
            .expect("store client"),
        ),
        bucket: "apps".into(),
        keys: None,
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
    // A delete and a URL of such a key are refused alike.
    let (status, _, text) = call(&state, "DELETE", "/a%5Cb", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    let (status, _, text) = call(
        &state,
        "POST",
        "/a%5Cb:url",
        br#"{"method":"GET"}"#.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(asked(&store, "PUT").await.is_empty());
    assert!(asked(&store, "DELETE").await.is_empty());
}

/// A store nobody answers at is the 503 of a store that did not answer, never a missing file
/// (T-7921).
#[tokio::test]
async fn a_store_that_is_not_there_is_unavailable() {
    // A socket bound and never listening: every connection to it is refused.
    let closed = tokio::net::TcpSocket::new_v4().expect("a socket");
    closed
        .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("bind");
    let port = closed.local_addr().expect("address").port();
    let state = state_at(format!("http://127.0.0.1:{port}"), json!(["files"]));
    let (status, body, text) = call(&state, "GET", "/old.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert_eq!(body["detail"], "the file store did not answer", "{text}");
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

/// A store that fails answers 503 on every verb, without the store's own words; a key the
/// store does not hold is 404 naming it; a key with a backslash is 400 before the store is
/// asked; an action other than `:url` is 404 naming the URL action.
#[tokio::test]
async fn a_failing_store_answers_503_and_a_missing_key_404() {
    let failing = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(
            ResponseTemplate::new(500).set_body_string("<Error>bucket secret-name</Error>"),
        )
        .mount(&failing)
        .await;
    let state = state(&failing, json!(["files"]));
    for (verb, uri, body) in [
        ("GET", "", Vec::new()),
        ("GET", "/old.txt", Vec::new()),
        ("PUT", "/new.jpg", b"jpeg".to_vec()),
        ("DELETE", "/old.txt", Vec::new()),
        (
            "POST",
            "/new.jpg:url",
            br#"{"method":"PUT","size":4}"#.to_vec(),
        ),
    ] {
        let (status, body, text) = call(&state, verb, uri, body).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{verb} {uri}: {text}"
        );
        assert_eq!(
            body["detail"], "the file store did not answer",
            "{verb} {uri}"
        );
        assert!(!text.contains("secret-name"), "{text}");
    }

    let typeless = MockServer::start().await;
    let listing = format!(
        "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{}old.txt</Key>\
         <Size>4</Size><LastModified>2026-10-10T20:00:00.000Z</LastModified></Contents>\
         </ListBucketResult>",
        prefix()
    );
    Mock::given(method("GET"))
        .and(path("/apps"))
        .respond_with(ResponseTemplate::new(200).set_body_string(listing))
        .mount(&typeless)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&typeless)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&typeless)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&typeless)
        .await;
    let state = self::state(&typeless, json!(["files"]));
    // A type the store does not tell (its HEAD refused) is listed as octet-stream.
    let (status, body, text) = call(&state, "GET", "", Vec::new()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body[0]["contentType"], "application/octet-stream");
    let (status, body, _) = call(&state, "GET", "/gone.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["detail"], "no file 'gone.txt'");
    let (status, body, _) = call(&state, "PUT", "/new.jpg", b"jpeg".to_vec()).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a refused write: {body}"
    );

    let asked_before = typeless.received_requests().await.unwrap_or_default().len();
    let (status, body, _) = call(&state, "GET", "/a%5Cb.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body, _) = call(&state, "POST", "/new.jpg:copy", Vec::new()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["detail"],
        "no action on 'new.jpg:copy'; a URL is asked at 'new.jpg:copy:url'"
    );
    assert_eq!(
        typeless.received_requests().await.unwrap_or_default().len(),
        asked_before,
        "neither reached the store"
    );
}

/// A write that carries the edge's cookie token needs the CSRF pair beside it, and a call off
/// the App's own host is sent there before any service runs (AP-84, RTE-12).
#[tokio::test]
async fn a_write_with_the_edges_token_needs_csrf_and_a_foreign_host_is_redirected() {
    let store = store(0).await;
    let mut state = state(&store, json!(["files"]));
    let token = common::REALM.person_token(APP, "jana", &[]);
    let mut config = (*state.config).clone();
    config.trust_edge_token = true;
    state.config = std::sync::Arc::new(config.clone());
    let request = Request::builder()
        .method("PUT")
        .uri(format!("/apps/{APP}/api/services/files/new.jpg"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("x-access-token", token.as_str())
        .body(Body::from("jpeg"))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(asked(&store, "PUT").await.is_empty());

    config.apps_url = Some("https://dev.example".parse().expect("a url"));
    state.config = std::sync::Arc::new(config);
    let request = Request::builder()
        .uri(format!("/apps/{APP}/api/services/files/old.txt"))
        .header(header::HOST, "portal.dev.example")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("a request");
    let response = joinedcontext_portal::server::app(state)
        .oneshot(request)
        .await
        .expect("a response");
    assert!(response.status().is_redirection(), "{}", response.status());
    assert_eq!(
        response.headers()[header::LOCATION],
        format!("https://{APP}.apps.dev.example/api/services/files/old.txt")
    );
}

/// An installation without a public store origin hands out no URL, and says so.
#[tokio::test]
async fn an_installation_without_a_store_origin_hands_out_no_url() {
    let store = store(0).await;
    let mut state = state(&store, json!(["files"]));
    let mut config = (*state.config).clone();
    config.apps_store_origin = None;
    state.config = std::sync::Arc::new(config);
    let (status, body, _) = call(
        &state,
        "POST",
        "/old.txt:url",
        br#"{"method":"GET"}"#.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["detail"], "this installation hands out no file URLs");
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

/// An installation with no App store answers 503 in words, and a project registry that does
/// not answer is a 503 too: neither guesses a prefix to write under.
#[tokio::test]
async fn no_store_or_no_registry_is_unavailable() {
    let store = store(0).await;
    let mut bare = state(&store, json!(["files"]));
    bare.wasm_apps = None;
    let (status, body, _) = call(&bare, "GET", "/old.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["detail"], "this installation stores no App files");

    let mut dead = state(&store, json!(["files"]));
    let connect: sqlx::postgres::PgConnectOptions = "postgres://nobody@127.0.0.1:1/none"
        .parse()
        .expect("options");
    dead.db = Some(
        sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(300))
            .connect_lazy_with(connect),
    );
    let (status, _, text) = call(&dead, "GET", "/old.txt", Vec::new()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(
        asked(&store, "GET").await.is_empty(),
        "the store is not asked"
    );
}

/// TEN-15: a keyed project's App writes under the keyed prefix and reads the old one until its
/// files moved, so a GET URL names the old key while only that prefix holds the file.
#[tokio::test]
async fn a_keyed_project_writes_keyed_and_hands_out_the_old_key_it_still_reads() {
    let store = store(4).await;
    let state = common::on_database(state(&store, json!(["files"]))).await;
    let pool = state.db.clone().expect("a pool");
    common::keyed_project(&state, &pool, PROJECT, "helsinki").await;
    let key = joinedcontext_portal::project_ids::key_of(&pool, &state.mirror, PROJECT)
        .await
        .expect("the registry")
        .expect("a keyed project");
    let keyed = joinedcontext_portal::apps::files::keyed_prefix(&key, &app_id(PROJECT, APP));

    let (status, body, text) = call(
        &state,
        "POST",
        "/old.txt:url",
        json!({ "method": "GET" }).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        body["url"]
            .as_str()
            .is_some_and(|url| url.contains(&format!("/apps/{}old.txt?", prefix()))),
        "{text}"
    );
    let (status, _, text) = call(&state, "PUT", "/new.txt", b"new".to_vec()).await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    assert!(
        asked(&store, "PUT")
            .await
            .contains(&format!("/apps/{keyed}new.txt")),
        "{:?}",
        asked(&store, "PUT").await
    );
}

/// A store on loopback that answers the listing of `old.txt` and closes the connection on every
/// other request, as a store that dies mid-call does.
async fn dropping_store() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let addr = listener.local_addr().expect("addr");
    let listing = format!(
        "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{}old.txt</Key>\
         <Size>4</Size><LastModified>2026-10-10T20:00:00.000Z</LastModified></Contents>\
         </ListBucketResult>",
        prefix()
    );
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let listing = listing.clone();
            tokio::spawn(async move {
                let mut head = vec![0u8; 8192];
                let read = socket.read(&mut head).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&head[..read]).into_owned();
                if request.starts_with("GET /apps?") {
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{listing}",
                        listing.len()
                    );
                    let _ = socket.write_all(answer.as_bytes()).await;
                }
            });
        }
    });
    format!("http://{addr}")
}

/// A store that lists and then drops the connection of the type's HEAD or of the write is a
/// 503 naming no part of the store, and nothing is reported stored.
#[tokio::test]
async fn a_store_that_drops_the_connection_mid_call_answers_503() {
    let state = state_at(dropping_store().await, json!(["files"]));
    let (status, body, text) = call(&state, "GET", "", Vec::new()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert_eq!(body["detail"], "the file store did not answer");
    let (status, body, text) = call(&state, "PUT", "/new.jpg", b"jpeg".to_vec()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert_eq!(body["detail"], "the file store did not answer");
}

/// A `wasm` App's files are under the shard its status records, every other App's under 0.
#[tokio::test]
async fn an_apps_files_are_listed_under_the_shard_its_status_records() {
    let store = store(4).await;
    let state = state(&store, json!(["files"]));
    let mut app = state.mirror.get(PROJECT, "App", APP).expect("the App");
    app.status = Some(serde_json::from_value(json!({ "shard": 1 })).expect("a status"));
    state.mirror.upsert(app);
    let (status, _, text) = call(&state, "GET", "", Vec::new()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let shard = format!("apps/1/{}/", app_id(PROJECT, APP));
    let listed = store
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() == "GET" && request.url.path() == "/apps")
        .any(|request| {
            request
                .url
                .query_pairs()
                .any(|(key, value)| key == "prefix" && value == shard)
        });
    assert!(listed, "the listing names shard 1's prefix");
}

/// For a keyed project a GET URL names the keyed key of a file the keyed prefix holds and of one
/// the store does not list, and a store that cannot list is a 503.
#[tokio::test]
async fn a_keyed_projects_get_url_names_the_key_the_file_is_under() {
    let store = store(4).await;
    let state = common::on_database(state(&store, json!(["files"]))).await;
    let pool = state.db.clone().expect("a pool");
    common::keyed_project(&state, &pool, PROJECT, "helsinki").await;
    let key = joinedcontext_portal::project_ids::key_of(&pool, &state.mirror, PROJECT)
        .await
        .expect("the registry")
        .expect("a keyed project");
    let keyed = joinedcontext_portal::apps::files::keyed_prefix(&key, &app_id(PROJECT, APP));
    let listing = format!(
        "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{keyed}fresh.txt</Key>\
         <Size>4</Size><LastModified>2026-10-10T20:00:00.000Z</LastModified></Contents>\
         </ListBucketResult>"
    );
    Mock::given(method("GET"))
        .and(path("/apps"))
        .respond_with(ResponseTemplate::new(200).set_body_string(listing))
        .with_priority(1)
        .mount(&store)
        .await;
    let get = json!({ "method": "GET" }).to_string().into_bytes();
    for file in ["fresh.txt", "unlisted.txt"] {
        let (status, body, text) = call(&state, "POST", &format!("/{file}:url"), get.clone()).await;
        assert_eq!(status, StatusCode::OK, "{file}: {text}");
        assert!(
            body["url"]
                .as_str()
                .is_some_and(|url| url.contains(&format!("/apps/{keyed}{file}?"))),
            "{file}: {text}"
        );
    }
    let failing = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(500))
        .mount(&failing)
        .await;
    let broken = state_at(failing.uri(), json!(["files"])).with_db(pool);
    let (status, body, text) = call(&broken, "POST", "/fresh.txt:url", get).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert_eq!(body["detail"], "the file store did not answer");
}
