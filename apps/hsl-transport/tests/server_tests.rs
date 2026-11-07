//! The app's own contract with its Endpoint (T-0309, AP-04, AP-28, AP-38, AP-41).
//!
//! Every case runs the real router and the real poll against a stubbed endpoint, so what is
//! asserted is the request that actually leaves the pod: which URL, which representation,
//! and which credential. The point of a public reference app is that it adds nothing at all
//! to the caller's authority, and that is only observable from outside the process.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hsl_transport::board::HSL_ALERTS;
use hsl_transport::{router, App, Config, PollError};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BASE: &str = "/apps/hsl-transport/";
const BUS: &str = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-01";

fn app_at(endpoint: &MockServer) -> Arc<App> {
    Arc::new(App::new(Config {
        base_path: BASE.to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        page_config: None,
    }))
}

/// One bus as the Endpoint's GeoJSON representation carries it.
fn feature(id: &str, longitude: f64, latitude: f64) -> Value {
    json!({
        "id": id,
        "type": "Feature",
        "geometry": { "type": "Point", "coordinates": [longitude, latitude] },
        "properties": {
            "type": "Vehicle",
            "heading": { "type": "Property", "value": 143.0 },
            "speed": { "type": "Property", "value": 8.5 },
            "route": { "type": "Property", "value": "550" }
        }
    })
}

fn collection(features: Vec<Value>) -> Value {
    json!({ "type": "FeatureCollection", "features": features })
}

/// Answers every fleet request with this collection.
async fn serving(endpoint: &MockServer, body: Value) {
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(endpoint)
        .await;
}

async fn text_of(response: axum::response::Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// AP-04, AP-38: the fleet comes from the Endpoint's GeoJSON representation, capped at 30.
#[tokio::test]
async fn the_fleet_is_read_from_the_endpoints_geojson_representation() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", "Vehicle"))
        .and(query_param("limit", "90"))
        .and(header("accept", "application/geo+json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(collection(vec![feature(BUS, 24.94, 60.17)])),
        )
        .expect(1)
        .mount(&endpoint)
        .await;

    let app = app_at(&endpoint);
    let changed = app.reload().await.expect("a fleet");

    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].id, BUS);
    assert_eq!(changed[0].coordinates, [24.94, 60.17]);
    assert_eq!(changed[0].ref_line.as_deref(), Some("550"));
}

/// AP-28: the Endpoint is public and the app never sees a user, so nothing it sends carries a
/// credential. An app that sent one would be lending an authority no caller asked for.
#[tokio::test]
async fn the_poll_carries_no_credential_at_all() {
    let endpoint = MockServer::start().await;
    serving(&endpoint, collection(vec![feature(BUS, 24.94, 60.17)])).await;

    app_at(&endpoint).reload().await.expect("a fleet");

    let requests = endpoint.received_requests().await.expect("the requests");
    assert_eq!(requests.len(), 1);
    for name in ["authorization", "cookie", "x-access-token"] {
        assert!(
            requests[0].headers.get(name).is_none(),
            "the poll sent a `{name}` header"
        );
    }
}

/// The Endpoint is the app's whole world: no broker, no database, no second host.
#[tokio::test]
async fn every_call_the_app_makes_goes_to_its_own_endpoint() {
    let endpoint = MockServer::start().await;
    serving(&endpoint, collection(vec![feature(BUS, 24.94, 60.17)])).await;
    let app = app_at(&endpoint);

    app.reload().await.expect("a fleet");
    // Serving a browser must not produce a call of its own; the fleet is already in memory.
    router(Arc::clone(&app))
        .oneshot(
            Request::builder()
                .uri(format!("{BASE}api/vehicles"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");

    let requests = endpoint.received_requests().await.expect("the requests");
    assert_eq!(requests.len(), 1, "a browser must not cost a query");
    assert!(requests[0].url.path().starts_with("/ngsi-ld/v1/"));
}

/// A hundred open maps are one query per interval, so only what moved travels.
#[tokio::test]
async fn a_bus_that_has_not_moved_is_not_sent_again() {
    let endpoint = MockServer::start().await;
    serving(&endpoint, collection(vec![feature(BUS, 24.94, 60.17)])).await;
    let app = app_at(&endpoint);

    assert_eq!(app.reload().await.expect("a fleet").len(), 1);
    assert!(
        app.reload().await.expect("a fleet").is_empty(),
        "an unchanged fleet produced a change"
    );
    assert_eq!(app.snapshot().len(), 1, "the bus left the map");
}

#[tokio::test]
async fn a_bus_that_moves_is_reported_once_and_the_others_stay_quiet() {
    let endpoint = MockServer::start().await;
    let other = "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-02";
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(200).set_body_json(collection(vec![
            feature(BUS, 24.94, 60.17),
            feature(other, 24.80, 60.20),
        ])))
        .up_to_n_times(1)
        .mount(&endpoint)
        .await;
    serving(
        &endpoint,
        collection(vec![
            feature(BUS, 24.95, 60.18),
            feature(other, 24.80, 60.20),
        ]),
    )
    .await;
    let app = app_at(&endpoint);

    assert_eq!(app.reload().await.expect("a fleet").len(), 2);
    let moved = app.reload().await.expect("a fleet");

    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].id, BUS);
    assert_eq!(moved[0].coordinates, [24.95, 60.18]);
}

/// AP-38, T-3893: the map draws at most ninety vehicles, the pipeline's caps summed, whatever the
/// space holds.
#[tokio::test]
async fn the_fleet_is_capped_at_ninety_vehicles() {
    let endpoint = MockServer::start().await;
    let features = (0..120)
        .map(|n| {
            feature(
                &format!("urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-{n:02}"),
                24.0 + f64::from(n) / 100.0,
                60.0,
            )
        })
        .collect();
    serving(&endpoint, collection(features)).await;

    let app = app_at(&endpoint);
    assert_eq!(app.reload().await.expect("a fleet").len(), 90);
    assert_eq!(app.snapshot().len(), 90);
}

/// A bus the map cannot place is a bus the map must not draw somewhere wrong.
#[tokio::test]
async fn a_feature_with_no_point_is_dropped_rather_than_placed_at_zero() {
    let endpoint = MockServer::start().await;
    serving(
        &endpoint,
        collection(vec![
            json!({ "id": "urn:ngsi-ld:Vehicle:hsl.fi:transport:bus-99", "type": "Feature", "geometry": Value::Null, "properties": {} }),
            feature(BUS, 24.94, 60.17),
        ]),
    )
    .await;

    let fleet = app_at(&endpoint).reload().await.expect("a fleet");
    assert_eq!(fleet.len(), 1);
    assert_eq!(fleet[0].id, BUS);
}

/// An attribute a Policy hides is absent from the app's own answer too, never a zero that a
/// reader would take for a measurement (R9).
#[tokio::test]
async fn an_attribute_the_grant_hides_is_absent_and_not_defaulted() {
    let endpoint = MockServer::start().await;
    serving(
        &endpoint,
        collection(vec![json!({
            "id": BUS,
            "type": "Feature",
            "geometry": { "type": "Point", "coordinates": [24.94, 60.17] },
            "properties": { "type": "Vehicle" }
        })]),
    )
    .await;
    let app = app_at(&endpoint);
    app.reload().await.expect("a fleet");

    let response = router(Arc::clone(&app))
        .oneshot(
            Request::builder()
                .uri(format!("{BASE}api/vehicles"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let fleet: Value = serde_json::from_str(&text_of(response).await).expect("json");

    assert_eq!(fleet[0]["coordinates"], json!([24.94, 60.17]));
    for hidden in ["bearing", "speed", "refLine"] {
        assert!(fleet[0].get(hidden).is_none(), "`{hidden}` was invented");
    }
}

/// Nothing polled yet is not an empty fleet, and a map should be told the difference.
#[tokio::test]
async fn the_fleet_route_says_so_before_the_first_poll_has_produced_anything() {
    let endpoint = MockServer::start().await;
    let response = router(app_at(&endpoint))
        .oneshot(
            Request::builder()
                .uri(format!("{BASE}api/vehicles"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}

/// AP-41: a browser that has just connected draws immediately, so the stream opens with the
/// whole fleet and every later event is a difference.
#[tokio::test]
async fn the_stream_opens_with_the_whole_fleet() {
    let endpoint = MockServer::start().await;
    serving(&endpoint, collection(vec![feature(BUS, 24.94, 60.17)])).await;
    let app = app_at(&endpoint);
    app.reload().await.expect("a fleet");

    let response = router(app)
        .oneshot(
            Request::builder()
                .uri(format!("{BASE}api/stream"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let frame = response
        .into_body()
        .frame()
        .await
        .expect("a frame")
        .expect("a data frame");
    let first = String::from_utf8_lossy(frame.data_ref().expect("data")).into_owned();
    assert!(first.contains("event: vehicles"), "got {first}");
    assert!(first.contains(BUS), "got {first}");
}

/// A poll that fails leaves the last known fleet on every open map: the buses stop moving
/// rather than disappearing, and the reason goes to the log rather than to the browser.
#[tokio::test]
async fn an_endpoint_that_refuses_leaves_the_last_known_fleet_in_place() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(collection(vec![feature(BUS, 24.94, 60.17)])),
        )
        .up_to_n_times(1)
        .mount(&endpoint)
        .await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&endpoint)
        .await;
    let app = app_at(&endpoint);

    app.reload().await.expect("a fleet");
    assert_eq!(app.reload().await, Err(PollError::Refused(403)));
    assert_eq!(app.snapshot().len(), 1, "a refusal emptied the map");
}

/// The Portal links the app with the trailing slash and a person types it without; they are
/// not different visitors.
#[tokio::test]
async fn the_front_page_answers_on_both_spellings_of_the_prefix() {
    let endpoint = MockServer::start().await;
    let app = app_at(&endpoint);
    for uri in [BASE.trim_end_matches('/'), BASE] {
        let response = router(Arc::clone(&app))
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(response.status(), StatusCode::OK, "{uri} did not answer");
    }
}

/// The pod's readiness probe: at the root, outside the base path, and never a query.
#[tokio::test]
async fn the_readiness_probe_answers_at_the_root_without_a_query() {
    let endpoint = MockServer::start().await;
    let response = router(app_at(&endpoint))
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the app answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text_of(response).await, "ok");
    assert!(endpoint
        .received_requests()
        .await
        .is_some_and(|r| r.is_empty()));
}

/// T-2909: on its own host the base path is `/`; the router must build there (it panicked at
/// start on dev with an empty route) and serve the fleet under it.
#[tokio::test]
async fn the_app_starts_on_its_own_host_at_the_root() {
    let endpoint = MockServer::start().await;
    let app = Arc::new(App::new(Config {
        base_path: "/".to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        page_config: None,
    }));
    for uri in ["/healthz", "/api/vehicles"] {
        let response = router(app.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .expect("the app answers");
        // The fleet answers 503 until its first poll: routed, not missing.
        assert!(
            [StatusCode::OK, StatusCode::SERVICE_UNAVAILABLE].contains(&response.status()),
            "{uri}: {}",
            response.status()
        );
    }
}

/// AP-67, AP-126: the front page carries the reconciler's `JC_APP_CONFIG` as `#jc-config`, so
/// the map finds the project's basemap; a value that spells `</script>` cannot end the element.
#[tokio::test]
async fn the_front_page_carries_the_reconcilers_configuration_with_its_basemap() {
    let endpoint = MockServer::start().await;
    let basemap = "https://portal.hel.fi/api/v1/projects/hel/basemap/default/style.json";
    let raw = json!({ "appName": "</script><script>alert(1)</script>", "basemap": basemap });
    let page = |page_config: Option<String>| {
        let app = Arc::new(App::new(Config {
            base_path: "/".to_owned(),
            endpoint_url: format!("{}/", endpoint.uri()),
            page_config,
        }));
        async move {
            let response = router(app)
                .oneshot(
                    Request::builder()
                        .uri("/")
                        .body(Body::empty())
                        .expect("a request"),
                )
                .await
                .expect("the app answers");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes();
            String::from_utf8_lossy(&body).into_owned()
        }
    };

    let served = page(Some(
        hsl_transport::page_config(&raw.to_string()).expect("an object"),
    ))
    .await;
    let open = r#"<script id="jc-config" type="application/json">"#;
    let start = served.find(open).expect("the page carries #jc-config") + open.len();
    let end = start
        + served[start..]
            .find("</script>")
            .expect("the element is closed");
    let config: Value = serde_json::from_str(&served[start..end]).expect("the element holds JSON");
    assert_eq!(config["basemap"], basemap);
    assert_eq!(
        config["appName"], raw["appName"],
        "read back as it was handed over"
    );

    // Without one the page is served as built, and anything but one JSON object is refused.
    assert!(!page(None).await.contains("jc-config"));
    for raw in ["not json", "[1]", "\"basemap\""] {
        assert!(
            hsl_transport::page_config(raw).is_err(),
            "{raw} was accepted"
        );
    }
}

/// Answers the board's reads of one type with these entities, simplified.
async fn board_serving(endpoint: &MockServer, kind: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", kind))
        .and(query_param("options", "keyValues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(endpoint)
        .await;
}

const SPACE: &str = "hel.fi:helsinki";

async fn board_endpoint() -> MockServer {
    let endpoint = MockServer::start().await;
    board_serving(&endpoint, "PublicTransportRoute", json!([
        { "id": "urn:ngsi-ld:PublicTransportRoute:x:550-1", "route": "2550", "directionId": 1, "headsign": "Itäkeskus" },
        { "id": "urn:ngsi-ld:PublicTransportRoute:x:550-2", "route": "2550", "directionId": "2", "headsign": "Westendinasema" }
    ])).await;
    // Only stops that carry departures are asked for; the endpoint is told so.
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", "GtfsStop"))
        .and(query_param("q", "departures"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": format!("urn:ngsi-ld:GtfsStop:{SPACE}:2222"), "name": { "fi": "Tapiola", "en": "Tapiola (M)" },
              "location": { "type": "Point", "coordinates": [24.80, 60.17] },
              "departures": [
                { "route": "x", "routeId": "2550", "directionId": 1, "expected": "2026-10-12T08:01:00Z", "status": "scheduled", "delay": 60 },
                { "routeId": "31M1", "directionId": 0, "expected": "2026-10-12T08:03:00Z", "status": "canceled" }
              ] },
            { "id": format!("urn:ngsi-ld:GtfsStop:{SPACE}:1111"), "name": "Aalto-yliopisto", "departures": [] }
        ])))
        .mount(&endpoint)
        .await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", "GtfsStop"))
        // Both stops the alerts name, in one ask; the space holds one of them.
        .and(query_param(
            "id",
            format!("urn:ngsi-ld:GtfsStop:{SPACE}:1392501,urn:ngsi-ld:GtfsStop:{SPACE}:9999"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": format!("urn:ngsi-ld:GtfsStop:{SPACE}:1392501"), "location": { "type": "Point", "coordinates": [25.03, 60.27] } }
        ])))
        .mount(&endpoint)
        .await;
    board_serving(&endpoint, "Alert", json!([
        { "id": format!("urn:ngsi-ld:Alert:{SPACE}:hsl-2"), "name": { "fi": "Poikkeus", "en": "<b>Detour</b>" },
          "description": "Line 550 takes a detour.", "severity": "medium",
          "refRoute": format!("urn:ngsi-ld:GtfsRoute:{SPACE}:2550"), "unknownReference": ["route:100HE", "stop:9999"],
          "validFrom": "2026-10-12T05:00:00Z", "source": HSL_ALERTS },
        { "id": format!("urn:ngsi-ld:Alert:{SPACE}:hsl-1"), "name": { "fi": "Pysäkki suljettu" }, "severity": "high",
          "refStop": [format!("urn:ngsi-ld:GtfsStop:{SPACE}:1392501")], "source": HSL_ALERTS },
        { "id": format!("urn:ngsi-ld:Alert:{SPACE}:digitraffic-7"), "name": "Road works",
          "source": "https://tie.digitraffic.fi/api/traffic-message/v1/messages" }
    ])).await;
    endpoint
}

/// T-3895: a stop's departures carry the headsign of their line and direction, and a stop with
/// none is not listed.
#[tokio::test]
async fn departures_name_their_headsign_and_only_stops_with_departures_are_listed() {
    let endpoint = board_endpoint().await;
    let app = app_at(&endpoint);
    let response = router(app.clone())
        .oneshot(
            Request::get(format!("{BASE}api/departures"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    app.poll_board().await.expect("a board");
    let response = router(app)
        .oneshot(
            Request::get(format!("{BASE}api/departures"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let stops: Value = serde_json::from_str(&text_of(response).await).expect("JSON");
    assert_eq!(
        stops,
        json!([{
            "id": format!("urn:ngsi-ld:GtfsStop:{SPACE}:2222"), "name": "Tapiola (M)", "coordinates": [24.80, 60.17],
            "departures": [
                { "line": "2550", "headsign": "Westendinasema", "expected": "2026-10-12T08:01:00Z", "status": "scheduled", "delay": 60 },
                { "line": "31M1", "expected": "2026-10-12T08:03:00Z", "status": "canceled" }
            ]
        }])
    );
}

/// T-3892, T-3895: HSL's alerts only, the most severe first, each with its lines and stops,
/// linked or kept as text, and where the stops are; the text goes out as HSL wrote it, for the
/// browser to render as text.
#[tokio::test]
async fn alerts_are_hsls_most_severe_first_with_their_lines_stops_and_places() {
    let endpoint = board_endpoint().await;
    let app = app_at(&endpoint);
    let board = app.poll_board().await.expect("a board");
    let alerts = serde_json::to_value(&board.alerts).expect("JSON");
    assert_eq!(
        alerts,
        json!([
            { "id": format!("urn:ngsi-ld:Alert:{SPACE}:hsl-1"), "title": "Pysäkki suljettu", "severity": "high",
              "lines": [], "stops": ["1392501"], "places": [{ "stop": "1392501", "coordinates": [25.03, 60.27] }] },
            { "id": format!("urn:ngsi-ld:Alert:{SPACE}:hsl-2"), "title": "<b>Detour</b>", "body": "Line 550 takes a detour.",
              "severity": "medium", "validFrom": "2026-10-12T05:00:00Z", "lines": ["100HE", "2550"], "stops": ["9999"], "places": [] }
        ])
    );
}

/// A board read that fails keeps the last board on every open page.
#[tokio::test]
async fn a_failed_board_read_keeps_the_last_board() {
    let endpoint = board_endpoint().await;
    let app = app_at(&endpoint);
    let first = app.poll_board().await.expect("a board");
    endpoint.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&endpoint)
        .await;
    assert_eq!(app.poll_board().await, Err(PollError::Refused(502)));
    assert_eq!(app.board(), Some(first));
}

/// T-3893: a vehicle says its mode, as the pipeline writes it.
#[tokio::test]
async fn a_vehicle_carries_its_mode() {
    let endpoint = MockServer::start().await;
    let mut tram = feature("urn:ngsi-ld:Vehicle:hel.fi:helsinki:40-3", 24.94, 60.17);
    tram["properties"]["transportMode"] = json!({ "type": "Property", "value": "tram" });
    serving(&endpoint, collection(vec![tram])).await;
    let app = app_at(&endpoint);
    let fleet = app.reload().await.expect("a fleet");
    assert_eq!(fleet[0].mode.as_deref(), Some("tram"));
}
