//! The ways a download's reads end that a healthy broker never shows: a refused connection, a
//! hang-up mid-answer, a body larger than the gateway holds or one that is no JSON (EP-26, T-2340).
//! Each answer is the gateway's own problem document; the broker's words are never passed on.

use crate::raw_broker::{spawn, Reply};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use context_gateway::app::{router, Gateway};
use context_gateway::pdp::PolicyPdp;
use context_gateway::proxy::Broker;
use context_gateway::resolver::Endpoint;
use jc_core::kinds::{Audience, FeedOutput, Representation};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

const SLUG: &str = "mluyob4nz52lok3ssk7pgn5vwt";
const PUBLIC_READ: &str = r#"contextSpaceRef: ovzdusie
assigner: did:web:banskabystrica.sk
assignee: { kind: role, id: public }
operations: [queryEntity, retrieveEntity]
"#;

fn endpoint() -> Endpoint {
    Endpoint {
        declared_types: None,
        roles: Default::default(),
        slug: SLUG.to_owned(),
        title: Default::default(),
        description: Default::default(),
        space: "ovzdusie".to_owned(),
        tenant: context_gateway::middleware::tenancy::Tenant::segment("ovzdusie").ok(),
        project: "ovzdusie".to_owned(),
        audience: Audience::Public,
        allowed_projects: Vec::new(),
        representations: vec![
            Representation::NgsiLd,
            Representation::Csv,
            Representation::Feed,
        ],
        rate_limit: None,
        creates: None,
        file_limits: None,
        hidden_attributes: Default::default(),
        projection: None,
        base_path: format!("/api/endpoint/{SLUG}"),
        models: Vec::new(),
        view_mapping: None,
        catalog: None,
        deprecation: None,
        feeds: vec![serde_norway::from_str::<FeedOutput>(
            "name: kultura\ntitle: { sk: Kultúra }\nsources: [{ type: Event, preset: event }]\n",
        )
        .expect("the output parses")],
        calendars: Vec::new(),
        map_defaults: Default::default(),
        page: None,
        open_data: false,
        policy_names: Vec::new(),
        manifest_name: String::new(),
        policies: vec![serde_norway::from_str(PUBLIC_READ).expect("the policy parses")],
    }
}

async fn ask(url: &str, endpoint: Endpoint, path: &str) -> (StatusCode, String, bool) {
    let app = router(Arc::new(
        Gateway::new(Broker::new(url), Box::new(PolicyPdp), "banskabystrica.sk").serve([endpoint]),
    ));
    let request = Request::builder()
        .uri(format!("/api/endpoint/{SLUG}{path}"))
        .body(Body::empty())
        .expect("a request");
    let response = app.oneshot(request).await.expect("the gateway answers");
    let status = response.status();
    let disposition = response.headers().contains_key("content-disposition");
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("a readable body");
    (
        status,
        String::from_utf8_lossy(&body).into_owned(),
        disposition,
    )
}

/// A port nobody listens on.
async fn closed() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    drop(listener);
    url
}

#[tokio::test]
async fn a_broker_nobody_answers_for_fails_a_download_with_and_without_a_selector() {
    let url = closed().await;
    for path in ["/file.csv", "/file.csv?type=AirQualityObserved"] {
        let (status, body, _) = ask(&url, endpoint(), path).await;
        assert!(status.is_server_error(), "{path}: {status} {body}");
        assert!(!body.contains("127.0.0.1"), "{path}: {body}");
    }
}

#[tokio::test]
async fn a_type_list_the_gateway_cannot_hold_or_read_is_a_500() {
    let oversized = spawn(|request, _| {
        if request.path().ends_with("/types") {
            Reply::Bytes(200, "application/json", vec![b' '; 9 * 1024 * 1024])
        } else {
            Reply::Json(200, json!([]))
        }
    })
    .await;
    let (status, body, _) = ask(&oversized.url, endpoint(), "/file.csv").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let cut = spawn(|_, _| Reply::Cut).await;
    let (status, body, _) = ask(&cut.url, endpoint(), "/file.csv").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let garbled = spawn(|_, _| Reply::Bytes(200, "application/json", b"not json".to_vec())).await;
    let (status, body, _) = ask(&garbled.url, endpoint(), "/file.csv").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

#[tokio::test]
async fn a_page_the_gateway_cannot_read_or_that_breaks_off_is_a_500() {
    let garbled = spawn(|_, _| Reply::Bytes(200, "application/json", b"not json".to_vec())).await;
    let (status, body, _) = ask(
        &garbled.url,
        endpoint(),
        "/file.csv?type=AirQualityObserved",
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");

    let cut = spawn(|_, _| Reply::Cut).await;
    let (status, body, _) = ask(&cut.url, endpoint(), "/file.csv?type=AirQualityObserved").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

#[tokio::test]
async fn an_endpoint_without_a_tenant_reads_nothing_for_a_download() {
    let broker = spawn(|_, _| Reply::Json(200, json!([]))).await;
    let mut untenanted = endpoint();
    untenanted.tenant = None;
    let (status, body, _) = ask(&broker.url, untenanted, "/file.csv?type=AirQualityObserved").await;
    assert!(!status.is_success(), "{status} {body}");
    assert_eq!(broker.count("GET "), 0, "{:?}", broker.calls);
}

// --- feeds/{output}.csv -----------------------------------------------------------------------

#[tokio::test]
async fn a_feed_output_is_a_csv_named_for_it_and_a_failed_read_names_no_file() {
    let broker = spawn(|_, _| Reply::Json(200, json!([]))).await;
    let (status, body, named) = ask(&broker.url, endpoint(), "/feeds/kultura.csv").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(named, "the file is named for the output");

    let failing = spawn(|_, _| Reply::Json(500, json!({}))).await;
    let (status, body, named) = ask(&failing.url, endpoint(), "/feeds/kultura.csv").await;
    assert!(status.is_server_error(), "{status} {body}");
    assert!(!named, "a refusal is no file");
}

#[tokio::test]
async fn a_feed_csv_path_with_more_than_a_filter_and_an_output_is_not_found() {
    let broker = spawn(|_, _| Reply::Json(200, json!([]))).await;
    let (status, body, _) = ask(&broker.url, endpoint(), "/feeds/a/b/kultura.csv").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // An output name that is no `[A-Za-z0-9_-]` is no output.
    let (status, body, _) = ask(&broker.url, endpoint(), "/feeds/bad%20name.csv").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(broker.count("GET "), 0, "{:?}", broker.calls);
}
