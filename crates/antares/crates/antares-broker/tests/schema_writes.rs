// SPDX-License-Identifier: EUPL-1.2
//! T-5912: a write that breaks the Tenant's `LinkML` schema is refused on
//! every write surface with the same 400, in `enforce` mode; `report` mode
//! writes it and refuses nothing. NGSI-LD on the Tenant root and through a
//! view (create, merge, append, partial update, replace, batch, temporal),
//! OData and OGC API Features Part 4 (both write through the NGSI-LD
//! routes). A value that keeps to the model reads back unchanged through
//! OData and OGC.
#![cfg(any(feature = "odata", feature = "ogc"))]

use antares_api::schema_writes::Mode;
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::Service;

const TENANT: &str = "model";

/// `Sensor` under the core context's default vocabulary: `pm10` a float
/// the model requires, `level` an enum, `temperature` in degrees Celsius,
/// `seen` a date-time.
const SCHEMA: &str = r#"{
  "id": "https://example.org/model", "name": "model",
  "prefixes": {"d": {"prefix_prefix": "d", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
  "default_prefix": "d",
  "classes": {"Sensor": {"slots": ["pm10", "level", "temperature", "seen", "location"]}},
  "enums": {"Level": {"permissible_values": {"low": {}, "high": {}}}},
  "slots": {
    "pm10": {"range": "float", "required": true},
    "level": {"range": "Level"},
    "temperature": {"range": "float", "unit": {"ucum_code": "Cel", "exact_mappings": ["unece:CEL"]}},
    "seen": {"range": "datetime"},
    "location": {"slot_uri": "https://uri.etsi.org/ngsi-ld/location",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}
  }
}"#;

const VIEW: &str = "/x/views/sensors";

async fn broker(mode: Mode) -> (Router, antares_ops::views::Reload) {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let mut state = AppState::new("schema-writes".into());
    antares_api::wire(&mut state).await;
    state.views.schema_writes.set_mode(mode);
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
        #[cfg(feature = "odata")]
        antares_ops::mount(antares_odata::OData),
        #[cfg(feature = "ogc")]
        antares_ops::mount(antares_ogc::Ogc),
    ];
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    let reload = antares_ops::views::install(&state)
        .await
        .expect("the view table");
    let app = antares_api::router(state);
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let view = json!({"id": "sensors", "tenant": TENANT, "selector": ["Sensor"],
        "rules": [{"types": ["Sensor"], "attributes": ["pm10", "level", "temperature", "seen", "location"]}],
        "operations": ops});
    let (s, b) = send(&app, "PUT", "/q/views/sensors", Some(&view)).await;
    assert!(s.is_success(), "view: {s} {b}");
    let schema: Value = serde_json::from_str(SCHEMA).expect("the schema");
    let (s, b) = send(&app, "PUT", &format!("/q/schemas/{TENANT}"), Some(&schema)).await;
    assert!(s.is_success(), "schema: {s} {b}");
    (app, reload)
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<&Value>) -> (StatusCode, Value) {
    let ct = if method == "PATCH" && uri.contains("/ogc/") {
        "application/merge-patch+json"
    } else {
        "application/json"
    };
    send_as(app, method, uri, body, ct).await
}

async fn send_as(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&Value>,
    content_type: &str,
) -> (StatusCode, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT);
    if body.is_some() {
        req = req
            .header("Content-Type", content_type)
            .header("Content-Length", text.len());
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("request"))
        .await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn sensor(id: &str, pm10: Value) -> Value {
    json!({"id": id, "type": "Sensor",
           "pm10": {"type": "Property", "value": pm10},
           "level": {"type": "Property", "value": "low"},
           "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [17.1, 48.1]}}})
}

/// The 400 the model answers: Bad Request Data naming the attribute and
/// the rule, never the value.
fn refused_by_model(status: StatusCode, body: &Value, attribute: &str, secret: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let text = body.to_string();
    assert!(text.contains("breaks the data model of Sensor"), "{text}");
    assert!(text.contains(attribute), "{text}");
    assert!(!text.contains(secret), "the value is echoed: {text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_ngsi_ld_write_route_refuses_what_breaks_the_model() {
    let (app, _reload) = broker(Mode::Enforce).await;
    for root in ["", VIEW] {
        let base = format!("{root}/ngsi-ld/v1");
        let id = format!("urn:ngsi-ld:Sensor:ok{}", root.len());
        let (s, b) = send(
            &app,
            "POST",
            &format!("{base}/entities"),
            Some(&sensor(&id, json!(12.5))),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{root}: {b}");

        // create: a range, an enum, a unit, a required slot
        let (s, b) = send(
            &app,
            "POST",
            &format!("{base}/entities"),
            Some(&sensor("urn:ngsi-ld:Sensor:bad", json!("abc-secret"))),
        )
        .await;
        refused_by_model(s, &b, "pm10", "abc-secret");
        let mut wrong_enum = sensor("urn:ngsi-ld:Sensor:bad", json!(1));
        wrong_enum["level"]["value"] = json!("medium-secret");
        let (s, b) = send(&app, "POST", &format!("{base}/entities"), Some(&wrong_enum)).await;
        refused_by_model(s, &b, "level", "medium-secret");
        let mut no_pm10 = sensor("urn:ngsi-ld:Sensor:bad", json!(1));
        no_pm10.as_object_mut().expect("object").remove("pm10");
        let (s, b) = send(&app, "POST", &format!("{base}/entities"), Some(&no_pm10)).await;
        refused_by_model(s, &b, "pm10", "\"value\"");

        let at = format!("{base}/entities/{id}");
        // merge, append, update attributes, partial attribute update
        let frag = json!({"temperature": {"type": "Property", "value": 70, "unitCode": "FAH"}});
        let (s, b) = send(&app, "PATCH", &at, Some(&frag)).await;
        refused_by_model(s, &b, "temperature", "FAH");
        let frag = json!({"seen": {"type": "Property", "value": "yesterday-secret"}});
        let (s, b) = send(&app, "POST", &format!("{at}/attrs"), Some(&frag)).await;
        refused_by_model(s, &b, "seen", "yesterday-secret");
        let frag = json!({"pm10": {"type": "Property", "value": true}});
        let (s, b) = send(&app, "PATCH", &format!("{at}/attrs"), Some(&frag)).await;
        refused_by_model(s, &b, "pm10", "true");
        let (s, b) = send(
            &app,
            "PATCH",
            &format!("{at}/attrs/pm10"),
            Some(&json!({"value": "x-secret"})),
        )
        .await;
        refused_by_model(s, &b, "pm10", "x-secret");
        // a kind other than the model's
        let frag = json!({"pm10": {"type": "Relationship", "object": "urn:ngsi-ld:X:1"}});
        let (s, b) = send(&app, "PATCH", &at, Some(&frag)).await;
        refused_by_model(s, &b, "pm10", "urn:ngsi-ld:X:1");
        // replace: the whole Entity, so the required slot counts
        let mut replacement = no_pm10.clone();
        replacement["id"] = json!(id);
        let (s, b) = send(&app, "PUT", &at, Some(&replacement)).await;
        refused_by_model(s, &b, "pm10", "\"value\"");

        // what keeps to the model is written
        let frag = json!({"pm10": {"type": "Property", "value": 40},
                          "temperature": {"type": "Property", "value": 21, "unitCode": "CEL"},
                          "seen": {"type": "Property", "value": "2026-10-28T10:00:00Z"}});
        let (s, b) = send(&app, "PATCH", &at, Some(&frag)).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{root}: {b}");
        let (s, b) = send(
            &app,
            "PATCH",
            &format!("{at}/attrs/pm10"),
            Some(&json!({"value": 41})),
        )
        .await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{root}: {b}");
        let (_, read) = send(&app, "GET", &format!("{at}?options=keyValues"), None).await;
        assert_eq!(read["pm10"], json!(41), "{read}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_refuses_only_the_member_that_breaks_the_model() {
    let (app, _reload) = broker(Mode::Enforce).await;
    for op in ["create", "upsert"] {
        let good = sensor(&format!("urn:ngsi-ld:Sensor:{op}-good"), json!(3));
        let bad = sensor(&format!("urn:ngsi-ld:Sensor:{op}-bad"), json!("n/a-secret"));
        let (s, b) = send(
            &app,
            "POST",
            &format!("/ngsi-ld/v1/entityOperations/{op}"),
            Some(&json!([good, bad])),
        )
        .await;
        assert_eq!(s, StatusCode::MULTI_STATUS, "{op}: {b}");
        assert_eq!(
            b["success"],
            json!([format!("urn:ngsi-ld:Sensor:{op}-good")]),
            "{op}: {b}"
        );
        let errors = b["errors"].as_array().expect("errors");
        assert_eq!(errors.len(), 1, "{op}: {b}");
        assert_eq!(
            errors[0]["entityId"],
            format!("urn:ngsi-ld:Sensor:{op}-bad"),
            "{b}"
        );
        let detail = errors[0]["error"].to_string();
        assert!(
            detail.contains("pm10") && !detail.contains("n/a-secret"),
            "{detail}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_write_is_held_to_the_model() {
    let (app, _reload) = broker(Mode::Enforce).await;
    let mut e = sensor("urn:ngsi-ld:Sensor:t", json!(1));
    e["pm10"] = json!([{"type": "Property", "value": 1, "observedAt": "2026-10-28T10:00:00Z"},
                       {"type": "Property", "value": "high-secret", "observedAt": "2026-10-28T11:00:00Z"}]);
    let (s, b) = send(&app, "POST", "/ngsi-ld/v1/temporal/entities", Some(&e)).await;
    refused_by_model(s, &b, "pm10", "high-secret");
    let (s, b) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(&sensor("urn:ngsi-ld:Sensor:t", json!(1))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let frag = json!({"pm10": [{"type": "Property", "value": "x-secret", "observedAt": "2026-10-28T12:00:00Z"}]});
    let (s, b) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Sensor:t/attrs",
        Some(&frag),
    )
    .await;
    refused_by_model(s, &b, "pm10", "x-secret");
}

#[cfg(feature = "ogc")]
#[tokio::test(flavor = "multi_thread")]
async fn ogc_writes_meet_the_same_refusal_and_read_back_what_keeps() {
    let (app, _reload) = broker(Mode::Enforce).await;
    // OGC API Features Part 4: no type check of its own, so the 400 is the model's
    let items = format!("{VIEW}/ogc/features/collections/Sensor/items");
    let feature = |pm10: Value, level: &str| {
        json!({"type": "Feature", "id": "urn:ngsi-ld:Sensor:ogc",
        "geometry": {"type": "Point", "coordinates": [17.0, 48.0]},
        "properties": {"pm10": pm10, "level": level}})
    };
    let (s, b) = send_as(
        &app,
        "POST",
        &items,
        Some(&feature(json!("abc-secret"), "low")),
        "application/geo+json",
    )
    .await;
    refused_by_model(s, &b, "pm10", "abc-secret");
    let (s, b) = send_as(
        &app,
        "POST",
        &items,
        Some(&feature(json!(5), "medium-secret")),
        "application/geo+json",
    )
    .await;
    refused_by_model(s, &b, "level", "medium-secret");
    let (s, b) = send_as(
        &app,
        "POST",
        &items,
        Some(&feature(json!(5), "high")),
        "application/geo+json",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, b) = send(
        &app,
        "PATCH",
        &format!("{items}/urn:ngsi-ld:Sensor:ogc"),
        Some(&json!({"properties": {"seen": "noon-secret"}})),
    )
    .await;
    refused_by_model(s, &b, "seen", "noon-secret");

    // any other 400 of the NGSI-LD route keeps the surface's own fixed text
    let mut spaced = feature(json!(5), "low");
    spaced["id"] = json!("urn:ngsi-ld:Sensor:has space");
    let (s, b) = send_as(&app, "POST", &items, Some(&spaced), "application/geo+json").await;
    assert_eq!(
        (s, b["detail"].as_str()),
        (StatusCode::BAD_REQUEST, Some("the view refused the write")),
        "{b}"
    );
    // written through NGSI-LD, read through OGC: the same values
    let (s, b) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(&sensor("urn:ngsi-ld:Sensor:rt", json!(33.5))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, f) = send(&app, "GET", &format!("{items}/urn:ngsi-ld:Sensor:rt"), None).await;
    assert_eq!(s, StatusCode::OK, "{f}");
    assert_eq!(f.pointer("/properties/pm10"), Some(&json!(33.5)), "{f}");
}

#[cfg(feature = "odata")]
#[tokio::test(flavor = "multi_thread")]
async fn odata_writes_meet_the_same_refusal_and_read_back_what_keeps() {
    let (app, _reload) = broker(Mode::Enforce).await;
    // OData: its own Edm check or the model's, a 400 either way, nothing written
    let set = format!("{VIEW}/odata/v4/Sensor");
    let (s, b) = send(
        &app,
        "POST",
        &set,
        Some(&json!({"id": "urn:ngsi-ld:Sensor:od", "pm10": "abc-secret"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(!b.to_string().contains("abc-secret"), "{b}");
    let (s, b) = send(
        &app,
        "POST",
        &set,
        Some(&json!({"id": "urn:ngsi-ld:Sensor:od", "pm10": 7, "level": "medium-secret"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let message = b["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("the attribute level breaks the data model of Sensor"),
        "{b}"
    );
    assert!(!message.contains("medium-secret"), "{b}");
    let (s, _) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Sensor:od",
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "a refused OData write stored nothing"
    );

    // any other 400 of the NGSI-LD route keeps the surface's own fixed text
    let (s, b) = send(
        &app,
        "POST",
        &set,
        Some(&json!({"id": "urn:ngsi-ld:Sensor:has space", "pm10": 7})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert_eq!(b["error"]["message"], "the request was refused", "{b}");

    // written through NGSI-LD, read through OData: the same values
    let (s, b) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(&sensor("urn:ngsi-ld:Sensor:rt", json!(33.5))),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, row) = send(
        &app,
        "GET",
        &format!("{set}('urn:ngsi-ld:Sensor:rt')"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{row}");
    assert_eq!(
        (row["pm10"].as_f64(), row["level"].as_str()),
        (Some(33.5), Some("low")),
        "{row}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn report_mode_writes_and_off_checks_nothing() {
    for mode in [Mode::Report, Mode::Off] {
        let (app, _reload) = broker(mode).await;
        let (s, b) = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(&sensor("urn:ngsi-ld:Sensor:r", json!("abc"))),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{mode:?}: {b}");
    }
}
