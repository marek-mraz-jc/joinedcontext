// SPDX-License-Identifier: EUPL-1.2
//! `ViewOps::run` on a view of another model (T-3817, gap G18): whichever
//! surface asks, the request is inverted into the source model and the
//! answer rebuilt in the target model, without the view root.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::routing::any;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// `/x/mp/{view}/ngsi-ld/v1/...` runs the request through
/// `Ops::mapped_view`; an `x-raw-path` header replaces the path and query,
/// as a surface that builds its own request may.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "mp"
    }
    fn prefix(&self) -> &'static str {
        "/x/mp"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/ngsi-ld/v1/{*rest}",
            any(move |caller: Caller, req: Request| {
                let ops = ops.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let mut segments = parts.uri.path().splitn(3, '/').skip(1);
                    let id = segments.next().unwrap_or_default();
                    let path = format!("/{}", segments.next().unwrap_or_default());
                    let Some(view) = ops.mapped_view(id) else {
                        return antares_ops::Refusal::NotFound.ngsi_ld();
                    };
                    let Some(op) = OperationId::of(parts.method.as_str(), &path, parts.uri.query())
                    else {
                        return antares_ops::Refusal::NotFound.ngsi_ld();
                    };
                    let target = match (parts.headers.get("x-raw-path"), parts.uri.query()) {
                        (Some(raw), _) => raw.to_str().unwrap_or_default().to_owned(),
                        (None, Some(q)) => format!("{path}?{q}"),
                        (None, None) => path,
                    };
                    let mut inner = OpsRequest::new(parts.method, target).body(body);
                    for (name, value) in &parts.headers {
                        if name != "x-raw-path" {
                            inner = inner.header(name.clone(), value.clone());
                        }
                    }
                    match view.run(op, &caller, inner).await {
                        Ok(r) => r,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                }
            }),
        )
    }
}

const ROOT: &str = "/x/mp/vehicles/ngsi-ld/v1";

struct Answer {
    status: StatusCode,
    content_type: String,
    count: Option<String>,
    text: String,
    json: Value,
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> Answer {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let content_type = header("content-type").unwrap_or_default();
    let count = header("ngsild-results-count");
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    Answer {
        status,
        content_type,
        count,
        text: String::from_utf8_lossy(&bytes).into_owned(),
        json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn get(app: &Router, uri: &str) -> Answer {
    send(app, Method::GET, uri, &[], None).await
}

/// Car:1 (speed 10, red) and Car:2 (speed 50, blue) in `city`, and
/// `vehicles` serving them as `Vehicle`s.
async fn fixture() -> Result<(TestBroker, Router), String> {
    fixture_on(TestBroker::new().await?).await
}

async fn fixture_on(b: TestBroker) -> Result<(TestBroker, Router), String> {
    let app = b.serve(Probe)?;
    for (id, speed, colour) in [
        ("urn:ngsi-ld:Car:1", 10, "red"),
        ("urn:ngsi-ld:Car:2", 50, "blue"),
    ] {
        let car = json!({"id": id, "type": "Car",
            "speed": {"type": "Property", "value": speed},
            "colour": {"type": "Property", "value": colour}});
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            &[("NGSILD-Tenant", "city")],
            Some(&car),
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    }
    let view = json!({
        "id": "vehicles", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "colour"]}],
        "operations": antares_ops::mapping::READS.iter().map(|op| op.id()).collect::<Vec<_>>(),
        "mapping": {"version": 2, "sourceClass": "Car", "targetClass": "Vehicle", "slots": [
            {"target": "velocity", "source": "speed", "kind": "unitConversion", "factor": 2, "filterable": true},
            {"target": "paint", "source": "colour", "kind": "valueMappings",
             "forward": {"red": "rot", "blue": "blau"}},
            {"target": "maker", "kind": "constant", "value": "acme"}
        ]}
    });
    let a = send(&app, Method::PUT, "/q/views/vehicles", &[], Some(&view)).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    Ok((b, app))
}

/// No answer names a source Attribute or the source class.
fn in_the_target_model(a: &Answer) {
    for leak in ["speed", "colour", "Car\"", "jcSrc"] {
        assert!(!a.text.contains(leak), "{leak} in {}", a.text);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_is_inverted_and_its_answer_rebuilt() {
    let (_b, app) = fixture().await.expect("fixture");
    let a = get(
        &app,
        &format!("{ROOT}/entities?type=Vehicle&q=velocity%3C50"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(
        a.json,
        json!([{"id": "urn:ngsi-ld:Car:1", "type": "Vehicle",
                "velocity": {"type": "Property", "value": 20.0},
                "paint": {"type": "Property", "value": "rot"},
                "maker": {"type": "Property", "value": "acme"}}])
    );
    in_the_target_model(&a);

    let a = get(&app, &format!("{ROOT}/entities/urn:ngsi-ld:Car:2")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json["velocity"]["value"], json!(100.0), "{}", a.text);
    in_the_target_model(&a);

    let a = get(
        &app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1/attrs/velocity"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json["value"], json!(20.0), "{}", a.text);
    in_the_target_model(&a);

    let a = get(&app, &format!("{ROOT}/temporal/entities/urn:ngsi-ld:Car:1")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json["type"], json!("Vehicle"), "{}", a.text);
    assert!(a.json.get("velocity").is_some(), "{}", a.text);
    in_the_target_model(&a);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_type_the_view_does_not_serve_selects_nothing() {
    let (_b, app) = fixture().await.expect("fixture");
    let a = get(&app, &format!("{ROOT}/entities?type=Truck&count=true")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json, json!([]));
    assert_eq!(a.count.as_deref(), Some("0"));
    assert_eq!(a.content_type, "application/json");
    let a = send(
        &app,
        Method::GET,
        &format!("{ROOT}/entities?type=Truck"),
        &[("Accept", "application/ld+json")],
        None,
    )
    .await;
    assert_eq!(a.content_type, "application/ld+json");
    assert_eq!(a.count, None);
    let a = get(
        &app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1?type=Truck"),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_names_the_target_model() {
    let (_b, app) = fixture().await.expect("fixture");
    let a = get(&app, &format!("{ROOT}/types/Vehicle")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    in_the_target_model(&a);
    let a = get(&app, &format!("{ROOT}/attributes/velocity")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert!(a.text.contains("velocity"), "{}", a.text);
    in_the_target_model(&a);
    let a = get(&app, &format!("{ROOT}/attributes")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    in_the_target_model(&a);
    let a = send(
        &app,
        Method::GET,
        &format!("{ROOT}/types?details=true"),
        &[("Accept", "application/json")],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert!(a.text.contains("Vehicle"), "{}", a.text);
    in_the_target_model(&a);
    let a = get(&app, &format!("{ROOT}/attributes?limit=1")).await;
    assert_eq!(
        a.status,
        StatusCode::BAD_REQUEST,
        "a parameter discovery does not take: {}",
        a.text
    );
    let a = get(&app, &format!("{ROOT}/types/Truck")).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    let a = get(&app, &format!("{ROOT}/attributes/nosuch")).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_mapped_view_cannot_serve_is_refused() {
    let (_b, app) = fixture().await.expect("fixture");
    let entities = format!("{ROOT}/entities?type=Vehicle");
    for (headers, detail) in [
        (
            vec![(
                "Link",
                "<https://example.org/c.jsonld>; rel=\"http://www.w3.org/ns/json-ld#context\"",
            )],
            "a mapped view names its attributes by its own @context; send no Link header",
        ),
        (
            vec![("Accept", "application/geo+json")],
            "a mapped view answers JSON and JSON-LD, not GeoJSON",
        ),
        (
            vec![("x-raw-path", "/ngsi-ld/v1/entities?q=a b")],
            "the query is not form encoded",
        ),
    ] {
        let a = send(&app, Method::GET, &entities, &headers, None).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{headers:?}: {}", a.text);
        assert_eq!(a.json["detail"], json!(detail), "{}", a.text);
    }
    // a filter on a slot that does not say it is filterable
    let a = get(
        &app,
        &format!("{ROOT}/entities?type=Vehicle&q=paint%3D%3D%22rot%22"),
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    in_the_target_model(&a);
    // an Attribute the mapping does not name
    let a = get(
        &app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1/attrs/nosuch"),
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_of_the_inner_request_is_worded_in_the_target_model() {
    let (_b, app) = fixture().await.expect("fixture");
    let a = get(&app, &format!("{ROOT}/entities/urn:ngsi-ld:Car:9")).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    in_the_target_model(&a);
    let a = send(
        &app,
        Method::HEAD,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:1"),
        &[],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.text, "", "HEAD: no body to rebuild");
}

/// The view's own refusals reach the caller as they are: a second tenant,
/// an answer past the broker's buffer bound.
#[tokio::test(flavor = "multi_thread")]
async fn the_views_own_refusals_pass() {
    let (_b, app) = fixture().await.expect("fixture");
    let entities = format!("{ROOT}/entities?type=Vehicle");
    let a = send(
        &app,
        Method::GET,
        &entities,
        &[("Accept", "application/json")],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    let a = send(
        &app,
        Method::GET,
        &entities,
        &[("NGSILD-Tenant", "other")],
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    assert_eq!(a.json["detail"], json!("a view serves one tenant"));
    let (_b, app) = fixture_on(TestBroker::with_max_answer_bytes(16).await.expect("broker"))
        .await
        .expect("fixture");
    let a = get(&app, &entities).await;
    assert_eq!(a.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", a.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_geoquery_without_geoproperty_filters_the_target_location() {
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve(Probe).expect("serve");
    // the source `location` of each Car lies where the other Car's
    // `position` is: filtering the source `location` answers the wrong Car
    for (id, position, location) in [
        ("urn:ngsi-ld:Car:1", [17.10, 48.10], [19.10, 48.70]),
        ("urn:ngsi-ld:Car:2", [19.10, 48.70], [17.10, 48.10]),
    ] {
        let car = json!({"id": id, "type": "Car",
            "position": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": position}},
            "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": location}}});
        let a = send(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            &[("NGSILD-Tenant", "city")],
            Some(&car),
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    }
    let view = json!({
        "id": "vehicles", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["position", "location"]}],
        "operations": ["query_entities"],
        "mapping": {"version": 2, "sourceClass": "Car", "targetClass": "Vehicle", "slots": [
            {"target": "location", "source": "position", "kind": "rename", "filterable": true},
            {"target": "registeredAt", "source": "location", "kind": "rename", "filterable": true}
        ]}
    });
    let a = send(&app, Method::PUT, "/q/views/vehicles", &[], Some(&view)).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    let near = "georel=near%3BmaxDistance%3D%3D1000&geometry=Point&coordinates=%5B17.1%2C48.1%5D";
    let ids = |a: &Answer| -> Vec<Value> {
        a.json
            .as_array()
            .map(|e| e.iter().map(|e| e["id"].clone()).collect())
            .unwrap_or_default()
    };
    let a = get(&app, &format!("{ROOT}/entities?type=Vehicle&{near}")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(ids(&a), vec![json!("urn:ngsi-ld:Car:1")], "{}", a.text);
    let a = get(
        &app,
        &format!("{ROOT}/entities?type=Vehicle&{near}&geoproperty=registeredAt"),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(ids(&a), vec![json!("urn:ngsi-ld:Car:2")], "{}", a.text);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_a_slot_cannot_translate_is_not_served() {
    let (_b, app) = fixture().await.expect("fixture");
    let car = json!({"id": "urn:ngsi-ld:Car:3", "type": "Car",
        "colour": {"type": "LanguageProperty", "languageMap": {"sk": "červená"}}});
    let a = send(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        &[("NGSILD-Tenant", "city")],
        Some(&car),
    )
    .await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    let a = get(
        &app,
        &format!("{ROOT}/entities/urn:ngsi-ld:Car:3/attrs/paint"),
    )
    .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    let a = get(&app, &format!("{ROOT}/entities/urn:ngsi-ld:Car:3")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.json.get("paint"), None, "{}", a.text);
    in_the_target_model(&a);
}
