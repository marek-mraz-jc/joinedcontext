//! Edge cases of the four Model Tools routes (T-2077 … T-2080; DM-10, DM-18, DM-19, DM-55, PF-57).
//!
//! **The contract, in one sentence:** Model Tools is an internal service the Portal speaks to on the
//! caller's behalf, so the caller may steer what is compiled and never where it is fetched from — and
//! every refusal happens in the Portal, before the service is reached, and names neither its address
//! nor anything else about it.
//!
//! `model_tools_proxy_tests.rs` covers the routes working (a source compiles, a source that does not is
//! an answer rather than an error, a catalogue identifier travels verbatim, a URL never reaches the
//! fetcher, the payload limit, the sample cap, an anonymous caller). This file is what it leaves: the
//! whole grammar of a catalogue identifier, the one query parameter the catalogue route forwards, the
//! shapes of a multipart upload that carry no sample, and the four doors' behaviour with no session.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::tools::model_tools::MAX_SAMPLE_BYTES;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

const CSRF: &str = "test-csrf-token-model-tools";

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
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
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn config_for(server: &MockServer) -> Config {
    Config {
        model_tools_url: Some(server.uri()),
        ..Config::for_tests()
    }
}

/// One request through the whole router, signed in unless `anonymous`.
async fn send(
    config: &Config,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    anonymous: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if !content_type.is_empty() {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    if !anonymous {
        request = request
            .header(header::COOKIE, session_cookie(config))
            .header(CSRF_HEADER, CSRF);
    }
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(request.body(Body::from(body)).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A multipart body with the fields a case wants, in order.
fn multipart(fields: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "portal-edge-boundary";
    let mut body = Vec::new();
    for (name, file_name, content) in fields {
        let disposition = match file_name {
            Some(file_name) => {
                format!("form-data; name=\"{name}\"; filename=\"{file_name}\"")
            }
            None => format!("form-data; name=\"{name}\""),
        };
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: {disposition}\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Everything the service was asked, as `VERB /path?query`.
async fn asked(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| {
            format!(
                "{} {}?{}",
                request.method,
                request.url.path(),
                request.url.query().unwrap_or_default()
            )
        })
        .collect()
}

// -------------------------------------------------------------------------------------------------
// T-2078 import_sdm: the catalogue identifier's grammar
// -------------------------------------------------------------------------------------------------

/// DM-10: the caller names a model in the catalogue and never a place to fetch it from. This is the
/// whole grammar of what `is_catalogue_id` accepts (`src/tools/sdm_mirror.rs`) — two segments of
/// letters, digits, dot, dash and underscore, neither empty, neither starting with a dot, neither
/// longer than 128 — and every refusal leaves the service untouched, which is what makes this the
/// Portal's guard rather than Model Tools'.
#[tokio::test]
async fn only_a_two_segment_catalogue_identifier_reaches_the_fetcher() {
    let service = MockServer::start().await;
    let config = config_for(&service);

    let long = "a".repeat(128);
    let too_long = "a".repeat(129);
    let refused: Vec<(&str, String)> = vec![
        ("a URL", "https://example.org/model.yaml".into()),
        ("a scheme-relative URL", "//example.org/model".into()),
        ("a file URL", "file:///etc/passwd".into()),
        ("no separator at all", "AirQualityObserved".into()),
        ("an empty subject", "/AirQualityObserved".into()),
        ("an empty name", "dataModel.Environment/".into()),
        ("nothing", String::new()),
        ("only a separator", "/".into()),
        (
            "three segments",
            "dataModel.Environment/Air/Observed".into(),
        ),
        ("a walk upwards", "../../etc/passwd".into()),
        ("a subject starting with a dot", ".hidden/Air".into()),
        (
            "a name starting with a dot",
            "dataModel.Environment/.git".into(),
        ),
        (
            "a name that is only a dot",
            "dataModel.Environment/.".into(),
        ),
        (
            "a percent-encoded separator",
            "dataModel.Environment%2FAir".into(),
        ),
        ("a space inside", "dataModel.Environment/Air Quality".into()),
        ("a colon inside", "dataModel.Environment/Air:1".into()),
        ("a query string", "dataModel.Environment/Air?x=1".into()),
        ("unicode", "dataModel.Prostredie/Ovzdušie".into()),
        ("a newline", "dataModel.Environment/Air\nObserved".into()),
        (
            "a name one character too long",
            format!("dataModel.Environment/{too_long}"),
        ),
        (
            "a subject one character too long",
            format!("{too_long}/Air"),
        ),
    ];

    for (what, model) in refused {
        let (status, body) = send(
            &config,
            "POST",
            "/api/v1/tools/import-sdm",
            "application/json",
            json!({ "model": model }).to_string().into_bytes(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {body}");
        assert!(
            body["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Smart Data Models catalogue identifier"),
            "{what} does not say what a model is: {body}",
        );
        assert!(
            !body.to_string().contains(&service.uri()),
            "{what} named the service: {body}",
        );
    }

    // Nothing of that reached Model Tools: the guard is the Portal's.
    assert!(
        asked(&service).await.is_empty(),
        "a refused identifier reached the service: {:?}",
        asked(&service).await,
    );

    // The bound itself, and the shapes the catalogue really uses, are accepted — they reach the
    // service, which is not mounted here, so the answer is the service's absence and not a refusal.
    for accepted in [
        "dataModel.Environment/AirQualityObserved".to_owned(),
        "a/b".to_owned(),
        "data-model_1.2/Air_Quality-2".to_owned(),
        format!("{long}/Air"),
        format!("dataModel.Environment/{long}"),
    ] {
        let (status, body) = send(
            &config,
            "POST",
            "/api/v1/tools/import-sdm",
            "application/json",
            json!({ "model": accepted }).to_string().into_bytes(),
            false,
        )
        .await;
        assert_ne!(
            status,
            StatusCode::BAD_REQUEST,
            "{accepted} was refused as an identifier: {body}",
        );
    }
    assert!(
        !asked(&service).await.is_empty(),
        "an accepted identifier never reached the service",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2079 sdm_catalog
// -------------------------------------------------------------------------------------------------

/// DM-10: `refresh` is the only thing a caller steers on the catalogue route, and it is a boolean —
/// anything else is the caller's mistake, answered before the service is reached. Whatever else the
/// query carries is dropped rather than forwarded, so a caller cannot add a parameter of their own to
/// the Portal's outgoing request.
#[tokio::test]
async fn the_catalogue_route_forwards_one_boolean_and_nothing_else_of_the_query() {
    let service = MockServer::start().await;
    let config = config_for(&service);

    for query in ["?refresh=maybe", "?refresh=1", "?refresh=", "?refresh=TRUE"] {
        let (status, body) = send(
            &config,
            "GET",
            &format!("/api/v1/tools/sdm-catalog{query}"),
            "",
            Vec::new(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    assert!(
        asked(&service).await.is_empty(),
        "a query that is not a boolean reached the service: {:?}",
        asked(&service).await,
    );

    // A query the Portal does not read is dropped: the outgoing request carries `refresh` and
    // nothing else, whatever the caller put beside it.
    for query in [
        "",
        "?refresh=true",
        "?refresh=false",
        "?catalog=https://evil.example/index.json",
        "?refresh=true&catalog=https://evil.example&limit=9999",
    ] {
        let _ = send(
            &config,
            "GET",
            &format!("/api/v1/tools/sdm-catalog{query}"),
            "",
            Vec::new(),
            false,
        )
        .await;
    }
    for call in asked(&service).await {
        assert!(call.starts_with("GET /catalog?"), "{call}");
        let forwarded = call.split_once('?').map(|(_, q)| q).unwrap_or_default();
        assert!(
            forwarded == "refresh=true" || forwarded == "refresh=false",
            "the outgoing query is not one boolean: {call}",
        );
    }
}

/// T-3840: `subject` names catalogue subjects to fill, comma-separated, at most eight, each in the
/// catalogue's own `dataModel.<Name>` spelling. A valid list reaches Model Tools one subject per
/// request, the refresh only on the first; anything else is refused before the service is asked.
#[tokio::test]
async fn subjects_reach_the_catalogue_one_per_request_and_nothing_else_does() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    let service = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/catalog"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"subjects": []})))
        .mount(&service)
        .await;
    let config = config_for(&service);
    let nine = (0..9)
        .map(|i| format!("dataModel.S{i}"))
        .collect::<Vec<_>>()
        .join(",");
    for query in [
        "?subject=Parking".to_owned(),
        "?subject=dataModel.".to_owned(),
        "?subject=dataModel.Park/../x".to_owned(),
        "?subject=dataModel.Parking,https://evil.example".to_owned(),
        format!("?subject={nine}"),
    ] {
        let (status, body) = send(
            &config,
            "GET",
            &format!("/api/v1/tools/sdm-catalog{query}"),
            "",
            Vec::new(),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    assert!(
        asked(&service).await.is_empty(),
        "a refused subject reached the service"
    );

    let (status, body) = send(
        &config,
        "GET",
        "/api/v1/tools/sdm-catalog?refresh=true&subject=dataModel.Parking,%20dataModel.Environment,",
        "",
        Vec::new(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        asked(&service).await,
        vec![
            "GET /catalog?refresh=true&subject=dataModel.Parking".to_owned(),
            "GET /catalog?refresh=false&subject=dataModel.Environment".to_owned(),
        ],
    );
}

// -------------------------------------------------------------------------------------------------
// T-2080 infer_schema
// -------------------------------------------------------------------------------------------------

/// DM-55: a sample is a file under `file`, and an upload that carries none is the caller's mistake —
/// a field of another name, an empty file, no fields at all, and a body that is not multipart. None of
/// them reaches Model Tools, so a person who picks the wrong thing in the browser learns it here.
#[tokio::test]
async fn an_upload_that_carries_no_sample_never_reaches_model_tools() {
    let service = MockServer::start().await;
    let config = config_for(&service);

    let cases: Vec<(&str, (String, Vec<u8>))> = vec![
        ("no fields at all", multipart(&[])),
        (
            "a field of another name",
            multipart(&[("sample", Some("a.csv"), b"id,name\n1,x\n")]),
        ),
        ("an empty file", multipart(&[("file", Some("a.csv"), b"")])),
        ("only a format", multipart(&[("format", None, b"csv")])),
    ];

    for (what, (content_type, body)) in cases {
        let (status, answer) = send(
            &config,
            "POST",
            "/api/v1/tools/infer-schema",
            &content_type,
            body,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {answer}");
        assert!(
            !answer.to_string().contains(&service.uri()),
            "{what} named the service: {answer}",
        );
    }

    // A body that is not multipart at all: refused by the extractor, before the handler.
    let (status, answer) = send(
        &config,
        "POST",
        "/api/v1/tools/infer-schema",
        "application/json",
        json!({ "file": "id,name" }).to_string().into_bytes(),
        false,
    )
    .await;
    assert!(status.is_client_error(), "{status}: {answer}");

    // A sample past the cap: refused by the route's own limit, which is set closest to the handler
    // so it wins over the router's (DM-55).
    let (content_type, body) = multipart(&[(
        "file",
        Some("big.csv"),
        &vec![b'x'; MAX_SAMPLE_BYTES + 128 * 1024],
    )]);
    let (status, answer) = send(
        &config,
        "POST",
        "/api/v1/tools/infer-schema",
        &content_type,
        body,
        false,
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::PAYLOAD_TOO_LARGE,
        "an oversized sample answered {status}: {answer}",
    );

    assert!(
        asked(&service).await.is_empty(),
        "an upload with no sample reached the service: {:?}",
        asked(&service).await,
    );
}

// -------------------------------------------------------------------------------------------------
// T-2077 generate, and the four doors together
// -------------------------------------------------------------------------------------------------

/// PF-57: all four routes are a signed-in person's, and the session is read before the body — so an
/// anonymous caller cannot make the Portal compile, fetch or forward anything, whatever they send.
/// With no service configured the answer is that there is none, and neither answer names an address.
#[tokio::test]
async fn no_session_and_no_service_are_two_answers_that_name_nothing() {
    let service = MockServer::start().await;
    let config = config_for(&service);

    let doors: Vec<(&str, &str, &str, Vec<u8>)> = vec![
        (
            "POST",
            "/api/v1/tools/generate",
            "application/json",
            json!({ "source": "id: x" }).to_string().into_bytes(),
        ),
        (
            "POST",
            "/api/v1/tools/import-sdm",
            "application/json",
            json!({ "model": "dataModel.Environment/AirQualityObserved" })
                .to_string()
                .into_bytes(),
        ),
        ("GET", "/api/v1/tools/sdm-catalog", "", Vec::new()),
    ];

    for (method, uri, content_type, body) in &doors {
        let (status, answer) = send(
            &config,
            method,
            uri,
            content_type,
            body.clone(),
            true, // anonymous
        )
        .await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{uri} let an anonymous caller in: {status} {answer}",
        );
        assert!(
            !answer.to_string().contains(&service.uri()),
            "{uri} named the service to an anonymous caller: {answer}",
        );
    }
    assert!(
        asked(&service).await.is_empty(),
        "an anonymous caller reached the service: {:?}",
        asked(&service).await,
    );

    // With no service configured, a signed-in caller is told there is none — and not what its
    // address would have been.
    let none = Config {
        model_tools_url: None,
        ..Config::for_tests()
    };
    for (method, uri, content_type, body) in &doors {
        let (status, answer) = send(&none, method, uri, content_type, body.clone(), false).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {answer}",);
        assert!(
            answer["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("no model tools service is configured"),
            "{uri}: {answer}",
        );
    }
}

/// A body with a field the request type does not have is accepted and the field ignored: neither
/// `GenerateRequest` nor `ImportSdmRequest` denies unknown fields (`src/tools/model_tools.rs:39`,
/// `:92`), against the rule that every request type does. Today's behaviour, written down in
/// `/workspace/chyby.md`: a caller who misspells `source` is told nothing about it, and gets the
/// compile of an empty source instead — which is an answer, not an error.
#[tokio::test]
async fn a_misspelled_field_is_ignored_rather_than_refused() {
    let service = MockServer::start().await;
    let config = config_for(&service);

    // The required field missing is still refused by the extractor: the type needs it.
    let (status, answer) = send(
        &config,
        "POST",
        "/api/v1/tools/generate",
        "application/json",
        json!({ "sources": "id: x" }).to_string().into_bytes(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");

    // But a field beside the required one is dropped in silence, and the request goes on.
    let (status, answer) = send(
        &config,
        "POST",
        "/api/v1/tools/generate",
        "application/json",
        json!({ "source": "id: x", "target": "https://evil.example", "version": 9 })
            .to_string()
            .into_bytes(),
        false,
    )
    .await;
    assert_ne!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");

    // Whatever it carried, only the source went out: the extra members are not in the body the
    // Portal sends, so they steer nothing.
    let sent: Vec<String> = service
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(!sent.is_empty(), "the source never reached the service");
    for body in sent {
        assert!(
            !body.contains("evil.example"),
            "a member the Portal does not read was forwarded: {body}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-1495 infer_schema: the format a caller may name
// -------------------------------------------------------------------------------------------------

/// The `format` of every request the service received, `None` when a body carried none.
async fn formats_sent(server: &MockServer) -> Vec<Option<String>> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .ok()
                .and_then(|body| body["format"].as_str().map(str::to_owned))
        })
        .collect()
}

/// DM-17: Model Tools switches its parser on `format`, so the Portal lets through one of the four
/// it documents and refuses anything else with the four named, before the service is reached.
#[tokio::test]
async fn an_unknown_format_is_refused_with_the_four_it_may_be() {
    let service = MockServer::start().await;
    let config = config_for(&service);
    for format in ["xml", "CSV", "csv;rm -rf", "../pdf", "parquet"] {
        let (content_type, body) = multipart(&[
            ("file", Some("sample.csv"), b"id,pm10\n1,22\n"),
            ("format", None, format.as_bytes()),
        ]);
        let (status, answer) = send(
            &config,
            "POST",
            "/api/v1/tools/infer-schema",
            &content_type,
            body,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{format}: {answer}");
        let text = answer.to_string();
        for known in ["csv", "xlsx", "json", "pdf"] {
            assert!(
                text.contains(known),
                "{format}: the refusal names {known}: {text}"
            );
        }
    }
    assert!(
        asked(&service).await.is_empty(),
        "a refused format reached Model Tools: {:?}",
        asked(&service).await,
    );
}

/// DM-17: each of the four documented formats travels to Model Tools as it was written, trimmed.
#[tokio::test]
async fn each_documented_format_reaches_model_tools() {
    let service = MockServer::start().await;
    let config = config_for(&service);
    for format in ["csv", "xlsx", " json ", "pdf"] {
        let (content_type, body) = multipart(&[
            ("file", Some("sample"), b"id,pm10\n1,22\n"),
            ("format", None, format.as_bytes()),
        ]);
        let (status, answer) = send(
            &config,
            "POST",
            "/api/v1/tools/infer-schema",
            &content_type,
            body,
            false,
        )
        .await;
        assert_ne!(status, StatusCode::BAD_REQUEST, "{format}: {answer}");
    }
    assert_eq!(
        formats_sent(&service).await,
        ["csv", "xlsx", "json", "pdf"].map(|f| Some(f.to_owned())),
    );
}

/// DM-17: an empty format is no format, so Model Tools goes by the file name.
#[tokio::test]
async fn an_empty_format_lets_the_file_name_decide() {
    let service = MockServer::start().await;
    let config = config_for(&service);
    for format in ["", "   "] {
        let (content_type, body) = multipart(&[
            ("file", Some("sample.csv"), b"id,pm10\n1,22\n"),
            ("format", None, format.as_bytes()),
        ]);
        let (status, answer) = send(
            &config,
            "POST",
            "/api/v1/tools/infer-schema",
            &content_type,
            body,
            false,
        )
        .await;
        assert_ne!(status, StatusCode::BAD_REQUEST, "{format:?}: {answer}");
    }
    assert_eq!(formats_sent(&service).await, [None, None]);
}
