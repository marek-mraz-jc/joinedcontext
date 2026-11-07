// SPDX-License-Identifier: EUPL-1.2
//! Capabilities annotations (odata-linkml-mapping section 4, C11 to C13,
//! C20; surfaces-threat-model O25): `$metadata` names the members `$filter`
//! and `$orderby` refuse, and the annotations are advisory, never a grant:
//! a request the engine denies is 403 whatever they say.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const ROOT: &str = "/x/views/cars/odata/v4";

const SCHEMA: &str = r#"{
  "id": "https://example.org/city", "name": "city",
  "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": "https://example.org/city/"}},
  "default_prefix": "city",
  "classes": {"Car": {"slots": ["speed", "tags", "doc", "brand", "location"]}},
  "slots": {"speed": {"range": "float"}, "doc": {}, "brand": {"range": "string"},
            "location": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}},
            "tags": {"annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "ListProperty"}}}}
}"#;

/// Allows everything but the reads of 5.7.1 and 5.7.2 when `deny` is set.
struct Reads {
    deny: bool,
}

impl PolicyEngine for Reads {
    fn name(&self) -> &'static str {
        "reads"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let denied = self.deny && matches!(op.clause, "5.7.1" | "5.7.2");
        Box::pin(std::future::ready(if denied {
            Decision::Deny("reads".into())
        } else {
            Decision::Allow
        }))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(b) = body {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", b.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(
            req.body(Body::from(body.unwrap_or_default().to_owned()))
                .unwrap_or_default(),
        )
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn broker(deny: bool) -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::new()
        .await?
        .with_policy(Arc::new(Reads { deny }));
    let app = b.serve(antares_odata::OData)?;
    let view = json!({
        "id": "cars", "tenant": "city", "@context": {"city": "https://example.org/city/"},
        "selector": ["city:Car"],
        "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:tags", "city:doc", "city:brand", "city:location"]}],
        "operations": ["query_entities", "retrieve_entity"]
    })
    .to_string();
    for (uri, body) in [
        ("/q/views/cars", view.as_str()),
        ("/q/schemas/city", SCHEMA),
    ] {
        let (status, answer) = send(&app, Method::PUT, uri, Some(body)).await;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    let car = json!({
        "id": "urn:ngsi-ld:Car:1", "type": "https://example.org/city/Car",
        "https://example.org/city/speed": {"type": "Property", "value": 3.0}
    })
    .to_string();
    let req = Request::post("/ngsi-ld/v1/entities")
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "application/json")
        .header("Content-Length", car.len())
        .body(Body::from(car))
        .map_err(|e| e.to_string())?;
    let Ok(made) = app.clone().oneshot(req).await;
    if made.status() != StatusCode::CREATED {
        return Err(format!("car: {}", made.status()));
    }
    Ok((b, app))
}

/// The `PropertyPath`s of `member` in the annotation `term` of the CSDL XML.
fn paths(xml: &str, term: &str, member: &str) -> Vec<String> {
    let Some(at) = xml.find(&format!(r#"Term="Capabilities.{term}""#)) else {
        return Vec::new();
    };
    let rest = xml.get(at..).unwrap_or_default();
    let annotation = rest.split("</Annotation>").next().unwrap_or_default();
    let Some(from) = annotation.find(&format!(r#"Property="{member}""#)) else {
        return Vec::new();
    };
    let value = annotation
        .get(from..)
        .unwrap_or_default()
        .split("</PropertyValue>")
        .next()
        .unwrap_or_default();
    value
        .split("<PropertyPath>")
        .skip(1)
        .filter_map(|p| p.split("</PropertyPath>").next())
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_restrictions_name_what_filter_and_orderby_refuse() {
    let (_b, app) = broker(false).await.expect("broker");
    let (status, xml) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    let unfilterable = paths(&xml, "FilterRestrictions", "NonFilterableProperties");
    let unsortable = paths(&xml, "SortRestrictions", "NonSortableProperties");
    // the surface sorts on the system members (T-3758) and filters on none
    let system = |p: &String| p == "createdAt" || p == "modifiedAt";
    assert_eq!(
        unfilterable
            .iter()
            .filter(|p| !system(p))
            .collect::<Vec<_>>(),
        unsortable.iter().collect::<Vec<_>>(),
        "{xml}"
    );
    assert!(unfilterable.iter().any(system), "{xml}");
    for name in ["tags", "_types", "scope"] {
        assert!(unfilterable.iter().any(|p| p == name), "{name}: {xml}");
    }
    for (option, served) in [
        ("$orderby", "speed%20desc"),
        ("$orderby", "createdAt"),
        ("$orderby", "modifiedAt%20desc"),
        ("$filter", "brand%20eq%20'x'"),
    ] {
        let (status, body) = send(
            &app,
            Method::GET,
            &format!("{ROOT}/Car?{option}={served}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{option}={served}: {body}");
    }
    for name in &unfilterable {
        for uri in [
            format!("{ROOT}/Car?$filter={name}%20eq%20null"),
            format!("{ROOT}/Car?$orderby={name}"),
        ] {
            if uri.contains("$orderby") && !unsortable.contains(name) {
                continue;
            }
            let (status, body) = send(&app, Method::GET, &uri, None).await;
            assert!(
                status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_IMPLEMENTED,
                "{uri}: annotated as refused, answered {status} {body}"
            );
        }
    }
    for (term, member, value) in [
        ("FilterRestrictions", "MaxLevels", r#"Int="1""#),
        ("ExpandRestrictions", "Expandable", r#"Bool="true""#),
        ("ExpandRestrictions", "MaxLevels", r#"Int="1""#),
        ("UpdateRestrictions", "Upsertable", r#"Bool="false""#),
    ] {
        let at = xml
            .find(&format!(r#"Term="Capabilities.{term}""#))
            .and_then(|at| xml.get(at..))
            .and_then(|rest| rest.split("</Annotation>").next())
            .unwrap_or_default();
        assert!(
            at.contains(&format!(r#"<PropertyValue Property="{member}" {value}/>"#)),
            "{term}/{member}: {at}"
        );
    }
    assert!(
        xml.contains(r#"<Annotation Term="Capabilities.QuerySegmentSupported" Bool="true"/>"#),
        "C20: POST /$query is served: {xml}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_capabilities_true_but_engine_denies_is_403() {
    let (_b, app) = broker(true).await.expect("broker");
    let (status, xml) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    // F-ANT4-003: a caller the engine refuses every read is shown a model
    // that names nothing it could not read (fail closed)
    assert!(!xml.contains(r#"Name="Car""#), "{xml}");
    assert!(!xml.contains("speed"), "{xml}");
    for path in [
        "Car",
        "Car/$count",
        "Car?$filter=speed%20gt%201",
        "Car?$orderby=speed",
        "Car?$count=true",
    ] {
        let (status, body) = send(&app, Method::GET, &format!("{ROOT}/{path}"), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert!(!body.contains("urn:ngsi-ld:Car:1"), "{path}: {body}");
    }
}

/// Status, headers and body of one request.
async fn fetch(app: &Router, method: Method, uri: &str) -> (StatusCode, HeaderMap, Bytes) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let (parts, body) = resp.into_parts();
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (parts.status, parts.headers, bytes)
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_is_declared_as_the_surface_serves_it() {
    let (_b, app) = broker(false).await.expect("broker");
    let (status, xml) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert!(
        xml.contains(r#"<Annotation Term="Capabilities.BatchSupported" Bool="true"/>"#),
        "C18: {xml}"
    );
    let support = xml
        .find(r#"Term="Capabilities.BatchSupport""#)
        .and_then(|at| xml.get(at..))
        .and_then(|rest| rest.split("</Annotation>").next())
        .unwrap_or_default();
    for claim in [
        r#"<PropertyValue Property="Supported" Bool="true"/>"#,
        r#"<PropertyValue Property="ContinueOnErrorSupported" Bool="true"/>"#,
        r#"<PropertyValue Property="ReferencesInRequestBodiesSupported" Bool="false"/>"#,
        "<String>multipart/mixed</String>",
        "<String>application/json</String>",
    ] {
        assert!(support.contains(claim), "{claim}: {support}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn head_is_the_get_without_its_body() {
    let (_b, app) = broker(false).await.expect("broker");
    for path in [
        "",
        "/$metadata",
        "/Car",
        "/Car/$count",
        "/Car('urn:ngsi-ld:Car:1')",
        "/Car?$top=1&$select=speed",
        "/Car('urn:ngsi-ld:Car:9')",
        "/Nothing",
    ] {
        let uri = format!("{ROOT}{path}");
        let (got, get_headers, body) = fetch(&app, Method::GET, &uri).await;
        let (head, head_headers, empty) = fetch(&app, Method::HEAD, &uri).await;
        assert_eq!(head, got, "{path}");
        assert!(empty.is_empty(), "{path}: {empty:?}");
        // One value: a second Content-Length makes hyper drop the answer (T-3737).
        let lengths: Vec<_> = head_headers
            .get_all("content-length")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(lengths, [body.len().to_string()], "{path}");
        for name in ["content-type", "odata-version"] {
            assert_eq!(
                head_headers.get(name),
                get_headers.get(name),
                "{path}: {name}"
            );
        }
    }
    let (status, _, _) = fetch(&app, Method::HEAD, "/x/views/none/odata/v4/Car").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn head_passes_the_gate_the_get_does() {
    let (_b, app) = broker(true).await.expect("broker");
    for path in ["Car", "Car/$count", "Car('urn:ngsi-ld:Car:1')"] {
        let uri = format!("{ROOT}/{path}");
        let (status, _, body) = fetch(&app, Method::HEAD, &uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert!(body.is_empty(), "{path}: {body:?}");
    }
}

/// The strings of the collection annotation `term` on the container.
fn strings(xml: &str, term: &str) -> Vec<String> {
    let Some(at) = xml.find(&format!(r#"Term="Capabilities.{term}""#)) else {
        return Vec::new();
    };
    let annotation = xml
        .get(at..)
        .unwrap_or_default()
        .split("</Annotation>")
        .next()
        .unwrap_or_default();
    annotation
        .split("<String>")
        .skip(1)
        .filter_map(|s| s.split("</String>").next())
        .map(str::to_owned)
        .collect()
}

/// `Capabilities.FilterFunctions` lists exactly what a `$filter` serves
/// (T-5907): each name, used once, is answered 200, and the list is this
/// one, so a function the evaluator gains turns this red until the list
/// and docs/src/odata.md name it.
#[tokio::test(flavor = "multi_thread")]
async fn filter_functions_are_the_served_operators_and_functions() {
    const SERVED: &[(&str, &str)] = &[
        ("eq", "speed eq 3"),
        ("ne", "speed ne 4"),
        ("gt", "speed gt 2"),
        ("ge", "speed ge 3"),
        ("lt", "speed lt 4"),
        ("le", "speed le 3"),
        ("and", "speed gt 2 and speed lt 4"),
        ("or", "speed gt 9 or speed lt 4"),
        ("not", "not (speed gt 4)"),
        ("in", "speed in (3,4)"),
        ("add", "speed add 1 eq 4"),
        ("sub", "speed sub 1 eq 2"),
        ("mul", "speed mul 2 eq 6"),
        ("div", "speed div 3 eq 1"),
        ("divby", "speed divby 3 eq 1"),
        ("mod", "speed mod 2 eq 1"),
        ("any", "tags/any(t:t eq 'a')"),
        ("all", "tags/all(t:t eq 'a')"),
        ("contains", "contains(brand,'a')"),
        ("startswith", "startswith(brand,'a')"),
        ("endswith", "endswith(brand,'a')"),
        (
            "geo.intersects",
            "geo.intersects(location,geography'SRID=4326;Polygon((0 0,1 0,1 1,0 1,0 0))')",
        ),
        ("concat", "concat(brand,'x') eq 'ax'"),
        ("indexof", "indexof(brand,'a') eq 0"),
        ("length", "length(brand) eq 1"),
        ("matchesPattern", "matchesPattern(brand,'%5Ea')"),
        ("substring", "substring(brand,1) eq 'a'"),
        ("tolower", "tolower(brand) eq 'a'"),
        ("toupper", "toupper(brand) eq 'A'"),
        ("trim", "trim(brand) eq 'a'"),
        ("year", "year(now()) gt 2000"),
        ("month", "month(now()) ge 1"),
        ("day", "day(now()) ge 1"),
        ("hour", "hour(now()) ge 0"),
        ("minute", "minute(now()) ge 0"),
        ("second", "second(now()) ge 0"),
        ("fractionalseconds", "fractionalseconds(now()) ge 0"),
        ("totaloffsetminutes", "totaloffsetminutes(now()) eq 0"),
        ("date", "date(now()) gt 2000-01-01"),
        ("time", "time(now()) ge 00:00:00"),
        ("now", "now() gt 2000-01-01T00:00:00Z"),
        ("mindatetime", "mindatetime() lt now()"),
        ("maxdatetime", "maxdatetime() gt now()"),
        ("round", "round(speed) eq 3"),
        ("floor", "floor(speed) eq 3"),
        ("ceiling", "ceiling(speed) eq 3"),
    ];
    let (_b, app) = broker(false).await.expect("broker");
    let (status, xml) = send(&app, Method::GET, &format!("{ROOT}/$metadata"), None).await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert_eq!(
        strings(&xml, "FilterFunctions"),
        SERVED.iter().map(|(f, _)| *f).collect::<Vec<_>>(),
        "{xml}"
    );
    for (function, filter) in SERVED {
        let uri = format!("{ROOT}/Car?$filter={}", filter.replace(' ', "%20"));
        let (status, body) = send(&app, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{function}: {uri}: {body}");
    }
    // a function the list leaves out is refused, named
    let uri = format!("{ROOT}/Car?$filter=geo.distance(location,location)%20lt%201");
    let (status, body) = send(&app, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert!(
        body.contains("method call: no NGSI-LD equivalent"),
        "{body}"
    );
    for annotation in [
        r#"<Annotation Term="Capabilities.IndexableByKey" Bool="true"/>"#,
        r#"<Annotation Term="Capabilities.TopSupported" Bool="true"/>"#,
        r#"<Annotation Term="Capabilities.SkipSupported" Bool="true"/>"#,
        r#"<Annotation Term="Capabilities.ConformanceLevel" EnumMember="Capabilities.ConformanceLevelType/Minimal"/>"#,
    ] {
        assert!(xml.contains(annotation), "{annotation}: {xml}");
    }
}
