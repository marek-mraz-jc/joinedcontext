// SPDX-License-Identifier: EUPL-1.2
//! `/q/views` and the loader (ADR-0023 rules 8, 10 and 11, V23): what is
//! refused before it is stored, what a broken stored view does, what the
//! loader reports, and names expanded through the view's own `@context`.

use antares_ops::test_kit::TestBroker;
use antares_ops::views::{View, MAX_RULES, MAX_VIEW_BYTES};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// No surface: `/q` and the NGSI-LD API are the broker's own.
struct Nothing;

impl antares_ops::Surface for Nothing {
    fn name(&self) -> &'static str {
        "nothing"
    }
    fn prefix(&self) -> &'static str {
        "/x/nothing"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: antares_ops::Ops) -> Router {
        Router::new()
    }
}

async fn send_raw(
    app: &Router,
    method: Method,
    uri: &str,
    tenant: Option<&str>,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let bytes = body.unwrap_or_default();
    if !bytes.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn put(app: &Router, id: &str, v: &Value) -> (StatusCode, Value) {
    send_raw(
        app,
        Method::PUT,
        &format!("/q/views/{id}"),
        None,
        Some(v.to_string()),
    )
    .await
}

fn view(id: &str) -> Value {
    json!({
        "id": id, "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
}

async fn broker() -> Result<(TestBroker, Router), String> {
    let b = TestBroker::new().await?;
    let app = b.serve(Nothing)?;
    Ok((b, app))
}

#[tokio::test(flavor = "multi_thread")]
async fn views_a_broken_view_is_not_served() {
    let (b, app) = broker().await.expect("broker");
    let (s, _) = put(&app, "good", &view("good")).await;
    assert_eq!(s, StatusCode::CREATED);
    let mut bad = view("bad");
    bad["q"] = json!("speed>");
    b.store_view_doc("bad", bad).await.expect("stored");
    b.store_view_doc("nameless", json!({"tenant": "city"}))
        .await
        .expect("stored");

    let (_, h) = send_raw(&app, Method::GET, "/q/health", None, None).await;
    assert_eq!(h["views"]["good"]["state"], "loaded", "{h}");
    assert_eq!(h["views"]["bad"]["state"], "invalid", "{h}");
    assert!(
        h["views"]["bad"]["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("q:")),
        "{h}"
    );
    assert!(
        h["views"].get("nameless").is_none(),
        "a document without an id is skipped: {h}"
    );
    assert!(
        b.serve_view("bad").is_err(),
        "an invalid view serves nothing"
    );
    assert!(b.serve_view("good").is_ok());
    let (s, list) = send_raw(&app, Method::GET, "/q/views", None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        list, h["views"],
        "GET /q/views is the table /q/health lists"
    ); // the broker's own router, no surface mounted, serves the same table
    let (s, root) = send_raw(&b.router(), Method::GET, "/q/views", None, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(root, list);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_is_refused_before_it_is_stored() {
    let (_b, app) = broker().await.expect("broker");
    let with = |member: &str, value: Value| {
        let mut v = view("v");
        v[member] = value;
        v
    };
    let rule = json!({"types": ["Car"], "attributes": ["speed"]});
    let cases = [
        ("an id outside the grammar", "V", view("V")),
        ("an id the path does not name", "v", view("w")),
        (
            "a reserved tenant",
            "v",
            with("tenant", json!("antares-views")),
        ),
        (
            "a tenant outside the grammar",
            "v",
            with("tenant", json!("a b")),
        ),
        ("an unknown member", "v", with("allowAll", json!(true))),
        (
            "a linkml member: the schema is the Tenant's, /q/schemas (T-3658)",
            "v",
            with("linkml", json!({"id": "https://e.org/s", "name": "s"})),
        ),
        ("an unparsable q", "v", with("q", json!("(speed>1"))),
        ("an empty scopeQ", "v", with("scopeQ", json!(""))),
        (
            "a geoQ that is no area",
            "v",
            with(
                "geoQ",
                json!({"georel": "within", "geometry": "Point", "coordinates": "nope"}),
            ),
        ),
        (
            "an operation off the matrix",
            "v",
            with("operations", json!(["query_entities", "drop_tables"])),
        ),
        (
            "a type that expands to no IRI",
            "v",
            with("@context", json!({"Car": "@id"})),
        ),
        (
            "a rule with no type",
            "v",
            with("rules", json!([{"types": [], "attributes": ["speed"]}])),
        ),
        (
            "too many rules",
            "v",
            with("rules", Value::Array(vec![rule; MAX_RULES + 1])),
        ),
        (
            "a q over its bound",
            "v",
            with("q", json!(format!("speed=={}", "9".repeat(5000)))),
        ),
    ];
    for (what, id, doc) in cases {
        let (s, body) = put(&app, id, &doc).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{what}: {body}");
        let (s, _) = send_raw(&app, Method::GET, &format!("/q/views/{id}"), None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{what} was stored");
    }
    let big = "x".repeat(MAX_VIEW_BYTES + 1);
    let (s, _) = send_raw(&app, Method::PUT, "/q/views/v", None, Some(big)).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    let (s, _) = send_raw(&app, Method::PUT, "/q/views/v", None, Some("{".into())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_view_reads_back_and_goes() {
    let (_b, app) = broker().await.expect("broker");
    let (s, _) = put(&app, "cars", &view("cars")).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, doc) = send_raw(&app, Method::GET, "/q/views/cars", None, None).await;
    assert_eq!(
        (s, doc),
        (StatusCode::OK, view("cars")),
        "stored as written"
    );
    let (s, _) = put(&app, "cars", &view("cars")).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "a second write replaces");
    let (s, _) = send_raw(&app, Method::DELETE, "/q/views/cars", None, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, body) = send_raw(&app, Method::DELETE, "/q/views/cars", None, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["title"], "ResourceNotFound");
}

#[test]
fn the_loader_reports_what_a_view_discloses() {
    let mut v = view("zone");
    v["rules"] = json!([
        {"types": ["Car"], "q": "status==\"parked\"", "attributes": ["speed"]},
        {"types": ["Car"], "q": "speed>10", "attributes": ["speed"]}
    ]);
    let loaded = serde_json::from_value::<View>(v.clone())
        .expect("a view")
        .load()
        .expect("loads");
    assert_eq!(loaded.notes.len(), 1, "{:?}", loaded.notes);
    assert!(
        loaded.notes[0].starts_with("rules[0] reads"),
        "{:?}",
        loaded.notes
    );
    assert!(loaded.notes[0].contains("status"), "{:?}", loaded.notes);

    // ADR-0023 rule 8: a view that serves history and keys on a member
    // that changes says so.
    v["q"] = json!("speed>10");
    v["operations"] = json!(["temporal_query"]);
    let loaded = serde_json::from_value::<View>(v)
        .expect("a view")
        .load()
        .expect("loads");
    assert!(
        loaded.notes.iter().any(|n| n.contains("rule 8")),
        "{:?}",
        loaded.notes
    );
}

/// The same short name in two vocabularies is two Attributes: the view's
/// `@context` expands its names, and a rule allowing one never shows the
/// other (ADR-0023 rule 11).
#[tokio::test(flavor = "multi_thread")]
async fn same_short_name_two_vocabularies_are_different_attributes() {
    let (b, app) = broker().await.expect("broker");
    let mut v = view("names");
    v["@context"] = json!({"a": "https://a.example/", "b": "https://b.example/"});
    v["rules"] = json!([{"types": ["Car"], "attributes": ["a:name"]}]);
    let (s, body) = put(&app, "names", &v).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let car = json!({
        "id": "urn:ngsi-ld:Car:1", "type": "Car",
        "https://a.example/name": {"type": "Property", "value": "from a"},
        "https://b.example/name": {"type": "Property", "value": "from b"}
    });
    let (s, body) = send_raw(
        &app,
        Method::POST,
        "/ngsi-ld/v1/entities",
        Some("city"),
        Some(car.to_string()),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{body}");

    let through = b.serve_view("names").expect("loaded");
    let (s, got) = send_raw(
        &through,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
        Some("city"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{got}");
    assert_eq!(got["https://a.example/name"]["value"], "from a", "{got}");
    assert!(got.get("https://b.example/name").is_none(), "{got}");
}

/// A selected type no rule names is out of the view: not found, not a bare
/// frame (ADR-0023 rule 11 as amended a second time; T-3528's "id and type
/// only" was the first amendment's answer).
#[tokio::test(flavor = "multi_thread")]
async fn type_without_a_rule_is_out_of_the_view() {
    let (b, app) = broker().await.expect("broker");
    let mut v = view("two");
    v["selector"] = json!(["Car", "Bike"]);
    let (s, body) = put(&app, "two", &v).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    for (id, kind) in [("urn:ngsi-ld:Car:1", "Car"), ("urn:ngsi-ld:Bike:1", "Bike")] {
        let e = json!({"id": id, "type": kind, "speed": {"type": "Property", "value": 5}});
        let (s, body) = send_raw(
            &app,
            Method::POST,
            "/ngsi-ld/v1/entities",
            Some("city"),
            Some(e.to_string()),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }
    let through = b.serve_view("two").expect("loaded");
    let (s, car) = send_raw(
        &through,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
        Some("city"),
        None,
    )
    .await;
    assert_eq!(
        (s, &car["speed"]["value"]),
        (StatusCode::OK, &json!(5)),
        "{car}"
    );
    let (s, bike) = send_raw(
        &through,
        Method::GET,
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Bike:1",
        Some("city"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{bike}");
}

/// `/q/views` outlives nothing: once the runtime that loaded the table has
/// stopped, a request through a router kept from it is told so.
#[test]
fn the_view_routes_answer_503_once_their_runtime_has_stopped() {
    let first = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let app = first
        .block_on(async { broker().await.map(|(_, app)| app) })
        .expect("broker");
    drop(first);
    let second = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    second.block_on(async {
        for (method, uri, body) in [
            (Method::GET, "/q/views", None),
            (Method::GET, "/q/views/v", None),
            (Method::PUT, "/q/views/v", Some(view("v").to_string())),
            (Method::DELETE, "/q/views/v", None),
            (Method::GET, "/q/schemas/city", None),
            (Method::PUT, "/q/schemas/city", Some("{}".to_owned())),
            (Method::DELETE, "/q/schemas/city", None),
        ] {
            let (s, b) = send_raw(&app, method, uri, None, body).await;
            assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {b}");
        }
    });
}
