//! The Portal in front of Model Tools: what a session may compile, what a caller may steer, and
//! what happens when the compiler is absent (DM-10, DM-18, DM-19).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::tools::model_tools::{MAX_REQUEST_BYTES, MAX_SAMPLE_BYTES};
use tower::ServiceExt;
use wiremock::matchers::{body_json_string, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

const COMPILED: &str = r#"{
  "jsonSchema": {"title": "AirQualityObserved"},
  "context": {"@context": {"temperature": "https://example.org/aq/temperature"}},
  "shacl": "@prefix sh: <http://www.w3.org/ns/shacl#> .",
  "generatorVersion": "linkml-1.8.0"
}"#;

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

/// Posts a body to a Portal route with a session cookie and the CSRF token that goes with it.
async fn post(config: Config, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let cookie = session_cookie(&config);
    let app = server::app(AppState::new(config, None));
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &cookie)
        .header(CSRF_HEADER, TEST_CSRF_TOKEN);
    let response = app
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Posts one file as `multipart/form-data`, the way the models page uploads a sample.
async fn upload(
    config: Config,
    uri: &str,
    file_name: &str,
    content: &[u8],
) -> (StatusCode, serde_json::Value) {
    let cookie = session_cookie(&config);
    let app = server::app(AppState::new(config, None));
    let boundary = "portal-test-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, &cookie)
        .header(CSRF_HEADER, TEST_CSRF_TOKEN);
    let response = app
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn config_for(server: &MockServer) -> Config {
    Config {
        model_tools_url: Some(server.uri()),
        ..Config::for_tests()
    }
}

#[tokio::test]
async fn a_linkml_source_comes_back_as_compiled_artifacts() {
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPILED, "application/json"))
        .mount(&tools)
        .await;

    let (status, body) = post(
        config_for(&tools),
        "/api/v1/tools/generate",
        r#"{"source":"id: https://example.org/aq\nname: aq\n"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jsonSchema"]["title"], "AirQualityObserved");
    assert_eq!(body["generatorVersion"], "linkml-1.8.0");
    assert!(
        body["owl"].is_null(),
        "an artifact this run did not render is absent"
    );
}

#[tokio::test]
async fn a_source_that_does_not_compile_is_an_answer_not_an_error() {
    // The editor asks on every pause in typing, so a half-written model must come back as
    // messages the view can show, never as a 5xx.
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"errors":["slot 'temperature' has no range"]}"#,
            "application/json",
        ))
        .mount(&tools)
        .await;

    let (status, body) = post(
        config_for(&tools),
        "/api/v1/tools/generate",
        r#"{"source":"name: aq\n"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["errors"][0], "slot 'temperature' has no range");
}

#[tokio::test]
async fn a_catalogue_identifier_reaches_model_tools_verbatim() {
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/import-sdm"))
        .and(body_json_string(
            r#"{"model":"dataModel.Environment/AirQualityObserved"}"#,
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(COMPILED, "application/json"))
        .mount(&tools)
        .await;

    let (status, body) = post(
        config_for(&tools),
        "/api/v1/tools/import-sdm",
        r#"{"model":"dataModel.Environment/AirQualityObserved"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["jsonSchema"]["title"], "AirQualityObserved");
}

#[tokio::test]
async fn a_caller_supplied_url_never_reaches_the_fetcher() {
    // DM-10: the allowlist lives in Model Tools, and the Portal refuses anything that is not a
    // catalogue identifier so no request is built from a caller's host at all. The mock has no
    // route mounted: a forwarded call would fail the test by answering 503 instead of 400.
    let tools = MockServer::start().await;

    for steered in [
        r#"{"model":"https://evil.example/schema.json"}"#,
        r#"{"model":"http://169.254.169.254/latest/meta-data"}"#,
        r#"{"model":"dataModel.Environment/../../../etc/passwd"}"#,
    ] {
        let (status, body) = post(config_for(&tools), "/api/v1/tools/import-sdm", steered).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{steered} was not refused");
        assert!(
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("catalogue identifier"),
            "{body}"
        );
    }
    assert_eq!(tools.received_requests().await.unwrap_or_default().len(), 0);
}

#[tokio::test]
async fn a_source_past_the_payload_limit_is_refused_before_it_is_forwarded() {
    let tools = MockServer::start().await;
    let body = format!(r#"{{"source":"{}"}}"#, "x".repeat(MAX_REQUEST_BYTES + 1));

    let (status, _) = post(config_for(&tools), "/api/v1/tools/generate", &body).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        tools.received_requests().await.unwrap_or_default().len(),
        0,
        "an oversized source never reaches the shared stateless service (DM-18)"
    );
}

#[tokio::test]
async fn without_a_configured_service_the_preview_is_unavailable_not_broken() {
    let (status, body) = post(
        Config::for_tests(),
        "/api/v1/tools/generate",
        r#"{"source":"name: aq\n"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["title"], "Service Unavailable");
}

#[tokio::test]
async fn a_refusing_or_silent_service_never_leaks_its_address() {
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&tools)
        .await;

    let (status, body) = post(
        config_for(&tools),
        "/api/v1/tools/generate",
        r#"{"source":"name: aq\n"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let rendered = body.to_string();
    assert!(
        !rendered.contains("127.0.0.1") && !rendered.contains(&tools.uri()),
        "the internal address stays in the log: {rendered}"
    );
}

#[tokio::test]
async fn an_anonymous_caller_cannot_compile() {
    let tools = MockServer::start().await;
    let app = server::app(AppState::new(config_for(&tools), None));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tools/generate")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::from(r#"{"source":"name: aq\n"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(tools.received_requests().await.unwrap_or_default().len(), 0);
}

const INFERRED: &str = r#"{
  "linkml": "name: Sensors\n",
  "operations": [{"op": "addClass", "name": "Sensors", "is_a": "Entity"}],
  "detectedTypes": {"pm10": "integer"},
  "matches": {},
  "untyped": [],
  "rows": 2,
  "errors": [],
  "generatorVersion": "linkml-1.11.1"
}"#;

#[tokio::test]
async fn a_sample_file_reaches_model_tools_as_base64_and_the_draft_comes_back_verbatim() {
    // T-0599, DM-54: the file goes as JSON, named as uploaded; a sample past the 512 KiB the
    // other tools routes take is still within this route's own 10 MiB.
    let tools = MockServer::start().await;
    let sample = format!("id,pm10\n{}", "1,12\n".repeat(150_000)).into_bytes();
    assert!(sample.len() > MAX_REQUEST_BYTES);
    use base64::Engine as _;
    let expected = serde_json::json!({
        "name": "sensors.csv",
        "content": base64::engine::general_purpose::STANDARD.encode(&sample),
    });
    Mock::given(method("POST"))
        .and(path("/infer-schema"))
        .and(body_json_string(expected.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_raw(INFERRED, "application/json"))
        .mount(&tools)
        .await;

    let (status, body) = upload(
        config_for(&tools),
        "/api/v1/tools/infer-schema",
        "sensors.csv",
        &sample,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["operations"][0]["name"], "Sensors");
    assert_eq!(body["rows"], 2);
    assert_eq!(body["generatorVersion"], "linkml-1.11.1");
}

#[tokio::test]
async fn a_sample_model_tools_cannot_read_is_the_persons_400_with_the_reason() {
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/infer-schema"))
        .respond_with(ResponseTemplate::new(400).set_body_raw(
            r#"{"errors":["the file is not a JSON document"]}"#,
            "application/json",
        ))
        .mount(&tools)
        .await;

    let (status, body) = upload(
        config_for(&tools),
        "/api/v1/tools/infer-schema",
        "x.json",
        b"nope",
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["detail"], "the file is not a JSON document");
}

#[tokio::test]
async fn a_sample_past_the_cap_or_without_a_file_never_reaches_model_tools() {
    // DM-55: 10 MiB at most, refused here; and an upload with no file is nothing to infer from.
    let tools = MockServer::start().await;
    let huge = vec![b'x'; MAX_SAMPLE_BYTES + 1];

    let (status, body) = upload(
        config_for(&tools),
        "/api/v1/tools/infer-schema",
        "huge.csv",
        &huge,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("larger than"),
        "{body}"
    );

    let (status, body) = upload(
        config_for(&tools),
        "/api/v1/tools/infer-schema",
        "empty.csv",
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(tools.received_requests().await.unwrap_or_default().len(), 0);
}
