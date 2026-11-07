// SPDX-License-Identifier: EUPL-1.2
//! `/x/views/{view}/linkml` against a broker over the built-in store
//! (T-3511, threats LK1 to LK8): what each path serves, what it never
//! names, and how it refuses.

use antares_ops::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

/// What a hidden name looks like: every test checks no answer carries one.
const HIDDEN: [&str; 4] = ["Person", "owner_secret", "Secret", "hidden.e.org"];

fn schema() -> Value {
    json!({
      "id": "https://e.org/city", "name": "city", "version": "2.1.0",
      "prefixes": {
        "city": {"prefix_prefix": "city", "prefix_reference": "https://e.org/city/"},
        "hidden": {"prefix_prefix": "hidden", "prefix_reference": "https://hidden.e.org/"}
      },
      "default_prefix": "city",
      "classes": {
        "Car": {"slots": ["speed", "owner", "fuel", "owner_secret"],
                "description": "A car."},
        "Person": {"class_uri": "hidden:Person", "slots": ["name"]}
      },
      "slots": {
        "speed": {"range": "float", "required": true},
        "owner": {"range": "Person"},
        "fuel": {"range": "Fuel"},
        "owner_secret": {"range": "Secret"},
        "name": {}
      },
      "enums": {"Fuel": {"permissible_values": {"diesel": {}}},
                "Secret": {"permissible_values": {"x": {}}}}
    })
}

fn view(operations: &[&str]) -> Value {
    json!({
      "id": "cars", "tenant": "city",
      "@context": {"city": "https://e.org/city/"},
      "selector": ["city:Car"],
      "rules": [{"types": ["city:Car"], "attributes": ["city:speed", "city:owner", "city:fuel"]}],
      "operations": operations
    })
}

/// The schema bound every broker here runs with.
const SCHEMA_MAX: usize = 64 * 1024;

const READS: [&str; 3] = [
    "retrieve_entity_type_details",
    "retrieve_entity_type_info",
    "retrieve_attribute_info",
];

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<&Value>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut req = Request::builder().method(method).uri(uri);
    let bytes = body.map(Value::to_string).unwrap_or_default();
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let (parts, body) = resp.into_parts();
    let body = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

async fn get(app: &Router, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    send(
        app,
        "GET",
        &format!("/x/views/cars/linkml/{path}"),
        &[],
        None,
    )
    .await
}

/// A broker asking `engine` (none when `None`) with view `doc` loaded and
/// `schema` stored for its Tenant (`/q/schemas/city`, T-3628).
async fn broker(
    engine: Option<Arc<dyn PolicyEngine>>,
    doc: &Value,
    schema: Option<&Value>,
) -> Result<Router, String> {
    // Read once when the broker is wired; every test here sets the same value.
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, SCHEMA_MAX.to_string());
    let mut broker = TestBroker::new().await?;
    if let Some(engine) = engine {
        broker = broker.with_policy(engine);
    }
    let app = broker.serve(antares_linkml::Linkml)?;
    let (status, _, body) = send(&app, "PUT", "/q/views/cars", &[], Some(doc)).await;
    if !status.is_success() {
        return Err(format!("the view does not load: {status} {body}"));
    }
    if let Some(schema) = schema {
        let (status, _, body) = send(&app, "PUT", "/q/schemas/city", &[], Some(schema)).await;
        if !status.is_success() {
            return Err(format!("the schema does not load: {status} {body}"));
        }
    }
    Ok(app)
}

fn assert_no_hidden_name(what: &str, body: &str) {
    for name in HIDDEN {
        assert!(!body.contains(name), "{what} names {name}: {body}");
    }
}

/// Narrows clause 5.7.6 by omitting `speed` while `narrow` is set.
struct OmitSpeed(AtomicBool);

impl PolicyEngine for OmitSpeed {
    fn name(&self) -> &'static str {
        "omit-speed"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        let d = if self.0.load(Ordering::SeqCst) {
            Decision::Filter(Filter {
                omit: vec!["https://e.org/city/speed".into()],
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Denies every discovery clause with a reason that names nothing hidden.
struct DenyAll;

impl PolicyEngine for DenyAll {
    fn name(&self) -> &'static str {
        "deny-all"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Deny("not for you".into())))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn linkml_prefixes_and_context_hold_only_visible_terms() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (s, h, yaml) = get(&app, "schema/v2/model.linkml.yaml").await;
    assert_eq!(s, StatusCode::OK, "{yaml}");
    assert_eq!(h[header::CONTENT_TYPE], "text/yaml");
    assert!(yaml.contains("\"https://e.org/city/views/cars\""), "{yaml}");
    assert!(
        yaml.contains("\"speed\":") && yaml.contains("\"Fuel\":"),
        "{yaml}"
    );
    assert_no_hidden_name("the YAML", &yaml);

    let (s, _, context) = get(&app, "schema/v2/context.jsonld").await;
    assert_eq!(s, StatusCode::OK);
    let context: Value = serde_json::from_str(&context).expect("json");
    let mut terms: Vec<&String> = context["@context"]
        .as_object()
        .expect("terms")
        .keys()
        .collect();
    terms.sort();
    assert_eq!(terms, ["Car", "fuel", "owner", "speed"]);

    for doc in [
        "model.schema.json",
        "model.shacl.ttl",
        "model.owl.ttl",
        "model.rdf.ttl",
        "model.md",
        "model",
    ] {
        let (s, _, body) = get(&app, &format!("schema/v2/{doc}")).await;
        assert_eq!(s, StatusCode::OK, "{doc}");
        assert_no_hidden_name(doc, &body);
    }
    let (s, _, index) = get(&app, "schema/index.json").await;
    assert_eq!(s, StatusCode::OK);
    assert_no_hidden_name("the index", &index);
    let index: Value = serde_json::from_str(&index).expect("json");
    assert_eq!(index["models"][0]["types"], json!(["Car"]));
    assert_eq!(index["models"][0]["redacted"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_class_and_a_slot_are_served_and_out_of_view_ones_are_404() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (s, _, car) = get(&app, "classes/Car").await;
    assert_eq!(s, StatusCode::OK, "{car}");
    let car: Value = serde_json::from_str(&car).expect("json");
    let slots: Vec<&str> = car["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(slots, ["speed", "owner", "fuel"]);
    assert_eq!(car["iri"], "https://e.org/city/Car");
    let (s, _, speed) = get(&app, "slots/speed").await;
    assert_eq!(s, StatusCode::OK);
    let speed: Value = serde_json::from_str(&speed).expect("json");
    assert_eq!(
        (speed["range"].as_str(), speed["classes"].clone()),
        (Some("float"), json!(["Car"]))
    );

    let (hidden, _, body) = get(&app, "classes/Person").await;
    let (absent, _, other) = get(&app, "classes/Nothing").await;
    assert_eq!(
        (hidden, absent),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND)
    );
    assert_eq!(body, other, "a hidden class answers as an absent one");
    for path in ["slots/owner_secret", "slots/name", "slots/nothing"] {
        assert_eq!(get(&app, path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn linkml_errors_carry_no_hidden_name() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    for path in [
        "classes/Person",
        "slots/owner_secret",
        "schema/v9/model.linkml.yaml",
        "schema/v2/model.qb.ttl",
        "schema/v2/qb",
        "schema/v2/Person",
        "other",
    ] {
        let (s, _, body) = get(&app, path).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
        assert_no_hidden_name(path, &body);
    }
    let (s, _, body) = send(
        &app,
        "GET",
        "/x/views/nothing/linkml/schema/index.json",
        &[],
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_no_hidden_name("an unknown view", &body);

    let denied = broker(Some(Arc::new(DenyAll)), &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (s, _, body) = get(&denied, "schema/index.json").await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert_no_hidden_name("a denial", &body);
}

#[tokio::test(flavor = "multi_thread")]
async fn linkml_is_read_only() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let (s, h, _) = send(
            &app,
            method,
            "/x/views/cars/linkml/schema/v2/model.linkml.yaml",
            &[],
            Some(&json!({})),
        )
        .await;
        assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_eq!(h[header::ALLOW], "GET, HEAD", "{method}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_etag_revalidates_and_head_carries_no_body() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (s, h, body) = get(&app, "schema/v2/model.shacl.ttl").await;
    assert_eq!(s, StatusCode::OK);
    let etag = h[header::ETAG].to_str().expect("an etag").to_owned();
    assert!(
        etag.starts_with('"') && etag.len() == 66,
        "strong sha-256: {etag}"
    );
    assert_eq!(h[header::CACHE_CONTROL], "private, no-cache");
    let uri = "/x/views/cars/linkml/schema/v2/model.shacl.ttl";
    let (s, _, empty) = send(&app, "GET", uri, &[("If-None-Match", &etag)], None).await;
    assert_eq!((s, empty.as_str()), (StatusCode::NOT_MODIFIED, ""));
    let (s, _, _) = send(&app, "GET", uri, &[("If-None-Match", "\"other\"")], None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, h, empty) = send(&app, "HEAD", uri, &[], None).await;
    assert_eq!((s, empty.as_str()), (StatusCode::OK, ""));
    assert_eq!(h[header::CONTENT_LENGTH], body.len().to_string().as_str());
    assert_eq!(h[header::ETAG], etag.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn each_path_is_an_operation_the_view_allows() {
    let app = broker(None, &view(&["retrieve_entity_type_info"]), Some(&schema()))
        .await
        .expect("a broker");
    assert_eq!(get(&app, "classes/Car").await.0, StatusCode::OK);
    for path in ["schema/index.json", "schema/v2/model", "slots/speed"] {
        let (s, _, body) = get(&app, path).await;
        assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{path}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn linkml_schema_is_cached_per_view_and_subject_and_never_served_across_them() {
    let engine = Arc::new(OmitSpeed(AtomicBool::new(false)));
    let app = broker(Some(engine.clone()), &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (_, wide_tag, wide) = get(&app, "schema/v2/model.linkml.yaml").await;
    assert!(wide.contains("\"speed\":"));
    engine.0.store(true, Ordering::SeqCst);
    let (_, narrow_tag, narrow) = get(&app, "schema/v2/model.linkml.yaml").await;
    assert!(
        !narrow.contains("\"speed\""),
        "the engine's narrowing holds: {narrow}"
    );
    assert_ne!(wide_tag[header::ETAG], narrow_tag[header::ETAG]);
    let (s, _, _) = send(
        &app,
        "GET",
        "/x/views/cars/linkml/schema/v2/model.linkml.yaml",
        &[(
            "If-None-Match",
            wide_tag[header::ETAG].to_str().expect("tag"),
        )],
        None,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a wider caller's tag does not revalidate"
    );
    engine.0.store(false, Ordering::SeqCst);
    assert_eq!(get(&app, "schema/v2/model.linkml.yaml").await.2, wide);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_schema_is_refused_and_the_surface_stays_closed() {
    let mut big = schema();
    big["classes"]["Car"]["description"] = json!("x".repeat(SCHEMA_MAX));
    let err = broker(None, &view(&READS), Some(&big))
        .await
        .expect_err("past the bound");
    assert!(err.contains("413"), "{err}");
    let app = broker(None, &view(&READS), None).await.expect("a broker");
    let (s, _, body) = get(&app, "schema/index.json").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    let (s, _, health) = send(&app, "GET", "/q/health", &[], None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(health.contains("/x/views/{view}/linkml"), "{health}");
    let (s, _, _) = send(&app, "GET", "/x/views/cars/linkml", &[], None).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "the surface root names no document"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_schema_outside_the_subset_or_none_closes_the_surface() {
    let mut outside = schema();
    outside["imports"] = json!(["https://w3id.org/biolink/biolink-model"]);
    let app = broker(None, &view(&READS), Some(&outside))
        .await
        .expect("a broker");
    assert_eq!(
        get(&app, "schema/index.json").await.0,
        StatusCode::NOT_FOUND
    );
    let app = broker(None, &view(&READS), None).await.expect("a broker");
    assert_eq!(
        get(&app, "schema/index.json").await.0,
        StatusCode::NOT_FOUND
    );
}

/// One LinkML source (T-3658): a view serves the schema stored for its own
/// Tenant, never another Tenant's, whichever schemas the broker holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_of_another_tenant_never_renders_this_tenants_schema() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let mut town = view(&READS);
    town["id"] = json!("trucks");
    town["tenant"] = json!("town");
    let (status, _, body) = send(&app, "PUT", "/q/views/trucks", &[], Some(&town)).await;
    assert!(status.is_success(), "the town view loads: {status} {body}");
    let trucks = |path: &str| format!("/x/views/trucks/linkml/{path}");
    let (status, _, body) = send(&app, "GET", &trucks("schema/index.json"), &[], None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "town holds no schema: {body}"
    );
    assert!(!body.contains("Car") && !body.contains("speed"), "{body}");

    let mut own = schema();
    own["id"] = json!("https://e.org/town/schema");
    own["name"] = json!("town");
    own["version"] = json!("9.0.0");
    let (status, _, body) = send(&app, "PUT", "/q/schemas/town", &[], Some(&own)).await;
    assert!(
        status.is_success(),
        "the town schema loads: {status} {body}"
    );
    let (status, _, body) = send(&app, "GET", &trucks("schema/index.json"), &[], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(r#""name":"town""#) && body.contains(r#""semver":"9.0.0""#),
        "town's own schema: {body}"
    );
    let (status, _, body) = get(&app, "schema/index.json").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(r#""name":"city""#) && !body.contains("9.0.0"),
        "city's view keeps city's schema: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn linkml_a_relationship_to_a_hidden_class_is_uriorcurie() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let (s, _, owner) = get(&app, "slots/owner").await;
    assert_eq!(s, StatusCode::OK, "{owner}");
    let owner: Value = serde_json::from_str(&owner).expect("json");
    assert_eq!(
        (owner["range"].as_str(), owner["kind"].as_str()),
        (Some("uriorcurie"), Some("Relationship"))
    );
    let (_, _, yaml) = get(&app, "schema/v2/model.linkml.yaml").await;
    assert!(yaml.contains("\"range\": \"uriorcurie\""), "{yaml}");
    assert_no_hidden_name("the YAML", &yaml);
}

/// Denies Retrieve Entity Type Information about `Car` and Retrieve
/// Attribute Information about `speed`, allows the rest.
struct NotCarNotSpeed;

impl PolicyEngine for NotCarNotSpeed {
    fn name(&self) -> &'static str {
        "not-car-not-speed"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let named = |names: &[String], iri: &str| names.iter().any(|n| n == iri);
        let d = if (op.clause == "5.7.7" && named(op.types, "https://e.org/city/Car"))
            || (op.clause == "5.7.10" && named(op.attrs, "https://e.org/city/speed"))
        {
            Decision::Deny("not this one".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// LK1: the engine is asked about the class or slot named, and its
/// refusal answers byte for byte as a name the schema lacks, so a 403
/// never tells a hidden name is there.
#[tokio::test(flavor = "multi_thread")]
async fn linkml_a_denied_class_or_slot_is_the_404_of_an_absent_one() {
    let app = broker(
        Some(Arc::new(NotCarNotSpeed)),
        &view(&READS),
        Some(&schema()),
    )
    .await
    .expect("a broker");
    for (denied, absent) in [
        ("classes/Car", "classes/Nothing"),
        ("slots/speed", "slots/nothing"),
    ] {
        let (s, _, body) = get(&app, denied).await;
        let (t, _, other) = get(&app, absent).await;
        assert_eq!(
            (s, t),
            (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND),
            "{denied}"
        );
        assert_eq!(body, other, "{denied} answers as {absent}");
    }
    assert_eq!(get(&app, "slots/fuel").await.0, StatusCode::OK);
    assert_eq!(get(&app, "schema/index.json").await.0, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn model_answers_the_formalism_accept_names() {
    let app = broker(None, &view(&READS), Some(&schema()))
        .await
        .expect("a broker");
    let uri = "/x/views/cars/linkml/schema/v2/model";
    for (accept, media_type) in [
        (
            "text/turtle; profile=\"owl\"",
            "text/turtle; profile=\"owl\"",
        ),
        ("text/yaml", "text/yaml"),
        ("application/json", "application/schema+json"),
    ] {
        let (s, h, _) = send(&app, "GET", uri, &[("Accept", accept)], None).await;
        assert_eq!(s, StatusCode::OK, "{accept}");
        assert_eq!(h[header::CONTENT_TYPE], media_type, "{accept}");
    }
}

/// ADR-0023 rule 12: a `*` view narrows by no rule, so its LinkML surface
/// serves every class and slot of the schema, as its NGSI-LD surface
/// serves every type and Attribute.
#[tokio::test(flavor = "multi_thread")]
async fn linkml_a_view_of_everything_serves_every_class_and_slot() {
    let all = json!({
      "id": "cars", "tenant": "city", "selector": ["*"],
      "rules": [{"types": ["*"], "attributes": ["*"]}],
      "operations": READS
    });
    let app = broker(None, &all, Some(&schema())).await.expect("a broker");
    let (s, _, car) = get(&app, "classes/Car").await;
    assert_eq!(s, StatusCode::OK, "{car}");
    let car: Value = serde_json::from_str(&car).expect("json");
    let slots: Vec<&str> = car["slots"]
        .as_array()
        .expect("slots")
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(slots, ["speed", "owner", "fuel", "owner_secret"]);
    for path in ["classes/Person", "slots/name", "slots/owner_secret"] {
        assert_eq!(get(&app, path).await.0, StatusCode::OK, "{path}");
    }
    let (s, _, index) = get(&app, "schema/index.json").await;
    let index: Value = serde_json::from_str(&index).expect("json");
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        index["models"][0].get("redacted"),
        None,
        "nothing is left out"
    );
}

/// The population cube (DM-60): `PopulationObservation` is a Data Structure
/// Definition with three dimensions and a measure; `District` is none.
fn population() -> Value {
    let role = |r: &str| json!({"qb_component": {"tag": "qb_component", "value": r}});
    json!({
      "id": "https://e.org/population", "name": "population",
      "prefixes": {"bb": {"prefix_prefix": "bb", "prefix_reference": "https://e.org/bb/"}},
      "default_prefix": "bb",
      "classes": {
        "PopulationObservation": {
          "slots": ["refDistrict", "ageBand", "year", "population", "source"],
          "annotations": {"qb_dsd": {"tag": "qb_dsd", "value": "true"}}
        },
        "District": {"slots": ["source"]}
      },
      "slots": {
        "refDistrict": {"range": "uriorcurie", "required": true, "annotations": role("dimension")},
        "ageBand": {"range": "string", "required": true, "annotations": role("dimension")},
        "year": {"range": "integer", "required": true, "annotations": role("dimension")},
        "population": {"range": "integer", "required": true, "annotations": role("measure")},
        "source": {"range": "string"}
      }
    })
}

/// A view `cars` of the population schema showing `types` with `slots`.
fn cube_view(types: &[&str], slots: &[&str]) -> Value {
    let bb = |names: &[&str]| -> Vec<String> { names.iter().map(|n| format!("bb:{n}")).collect() };
    json!({
      "id": "cars", "tenant": "city", "@context": {"bb": "https://e.org/bb/"},
      "selector": bb(types),
      "rules": [{"types": bb(types), "attributes": bb(slots)}],
      "operations": READS
    })
}

const COMPONENTS: [&str; 5] = ["refDistrict", "ageBand", "year", "population", "source"];

/// T-3805: a view that shows a Data Structure Definition serves its cube,
/// lists it in the index, and answers `qb` with the same bytes.
#[tokio::test(flavor = "multi_thread")]
async fn the_cube_of_a_shown_dsd_names_each_component_by_its_role() {
    let view = cube_view(&["PopulationObservation", "District"], &COMPONENTS);
    let app = broker(None, &view, Some(&population()))
        .await
        .expect("a broker");
    let (s, h, cube) = get(&app, "schema/v1/model.qb.ttl").await;
    assert_eq!(s, StatusCode::OK, "{cube}");
    assert_eq!(h[header::CONTENT_TYPE], "text/turtle");
    assert!(cube.contains(
        "<https://e.org/bb/PopulationObservationStructure> a qb:DataStructureDefinition"
    ));
    for dimension in ["refDistrict", "ageBand", "year"] {
        assert!(
            cube.contains(&format!("[ qb:dimension <https://e.org/bb/{dimension}> ]")),
            "{cube}"
        );
    }
    assert!(cube.contains("[ qb:measure <https://e.org/bb/population> ]"));
    assert!(
        !cube.contains("bb/source>") && !cube.contains("bb/District>"),
        "{cube}"
    );
    assert_eq!(get(&app, "schema/v1/qb").await.2, cube);
    let (_, _, index) = get(&app, "schema/index.json").await;
    assert!(index.contains("\"model.qb.ttl\""), "{index}");
}

/// T-3805, LK1/LK8: a dimension the view or the engine hides is no
/// component; a model whose shown classes hold no Data Structure Definition
/// has no cube, and its 404 is the one an unknown document gets.
#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_dimension_or_dsd_leaves_no_trace_in_the_cube() {
    let narrow = cube_view(&["PopulationObservation"], &["year", "population"]);
    let app = broker(None, &narrow, Some(&population()))
        .await
        .expect("a broker");
    let (s, _, cube) = get(&app, "schema/v1/model.qb.ttl").await;
    assert_eq!(s, StatusCode::OK, "{cube}");
    assert!(cube.contains("bb/year>") && cube.contains("bb/population>"));
    assert!(
        !cube.contains("ageBand") && !cube.contains("refDistrict"),
        "{cube}"
    );

    let all = cube_view(&["PopulationObservation"], &COMPONENTS);
    let engine = Arc::new(OmitAgeBand);
    let app = broker(Some(engine), &all, Some(&population()))
        .await
        .expect("a broker");
    let (_, _, cube) = get(&app, "schema/v1/model.qb.ttl").await;
    assert!(
        cube.contains("bb/year>") && !cube.contains("ageBand"),
        "{cube}"
    );

    let districts = cube_view(&["District"], &["source"]);
    let app = broker(None, &districts, Some(&population()))
        .await
        .expect("a broker");
    let (s, _, absent) = get(&app, "schema/v1/model.qb.ttl").await;
    let (_, _, unknown) = get(&app, "schema/v1/model.nothing").await;
    assert_eq!((s, &absent), (StatusCode::NOT_FOUND, &unknown));
    assert!(!absent.contains("PopulationObservation"));
    let (_, _, index) = get(&app, "schema/index.json").await;
    assert!(!index.contains("model.qb.ttl"), "{index}");
    let (_, _, owl) = get(&app, "schema/v1/model.owl.ttl").await;
    assert!(!owl.contains("qb:"), "{owl}");
}

/// Omits the `ageBand` dimension from every discovery answer, as a grant
/// without it would.
struct OmitAgeBand;

impl PolicyEngine for OmitAgeBand {
    fn name(&self) -> &'static str {
        "omit-age-band"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Filter(Filter {
            omit: vec!["https://e.org/bb/ageBand".into()],
            ..Filter::default()
        })))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Grants the whole Car Entity, as a Policy naming the type without
/// `propertyNames` does (`Rule::every_attribute`, T-3637).
struct WholeCar;

impl PolicyEngine for WholeCar {
    fn name(&self) -> &'static str {
        "whole-car"
    }
    fn decide<'a>(&'a self, _: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Filter(Filter {
            rules: vec![Rule {
                types: vec!["https://e.org/city/Car".into()],
                every_attribute: true,
                ..Rule::default()
            }],
            ..Filter::default()
        })))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// T-3798: under a grant over the whole Entity a view of `*` (ADR-0023
/// rule 12, no rule of its own) keeps every slot of the granted class, and
/// no class the grant does not name.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_over_the_whole_entity_keeps_every_slot_of_its_class() {
    let all = json!({
      "id": "cars", "tenant": "city", "selector": ["*"],
      "rules": [{"types": ["*"], "attributes": ["*"]}],
      "operations": READS
    });
    let app = broker(Some(Arc::new(WholeCar)), &all, Some(&schema()))
        .await
        .expect("a broker");
    let (s, _, car) = get(&app, "classes/Car").await;
    assert_eq!(s, StatusCode::OK, "{car}");
    let car: Value = serde_json::from_str(&car).expect("json");
    let slots: Vec<&str> = car["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(slots, ["speed", "owner", "fuel", "owner_secret"], "{car}");
    assert_eq!(get(&app, "classes/Person").await.0, StatusCode::NOT_FOUND);
}

const CORE: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// Denies exactly the core-context IRIs of `Car` and `speed`: the names a view written with
/// short names shows, so a refusal proves the engine was asked about those, not the schema's.
struct NotCoreCarNotCoreSpeed;

impl PolicyEngine for NotCoreCarNotCoreSpeed {
    fn name(&self) -> &'static str {
        "not-core-car-not-core-speed"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let named = |names: &[String], n: &str| names.iter().any(|x| *x == format!("{CORE}{n}"));
        let d = if (op.clause == "5.7.7" && named(op.types, "Car"))
            || (op.clause == "5.7.10" && named(op.attrs, "speed"))
        {
            Decision::Deny("not this one".into())
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// T-3645: a schema in another vocabulary (`sdm:`, as Model Tools renders helsinki's) and a view
/// written with short names, as the Portal writes them. The surface shows what the view names,
/// read under the core @context, serves the IRIs the view reads (the `OData` surface's, T-5206), hides the rest,
/// and asks the engine about the IRI the view shows.
#[tokio::test(flavor = "multi_thread")]
async fn a_schema_of_another_vocabulary_serves_what_a_short_named_view_shows() {
    let sdm = json!({
      "id": "https://e.org/city", "name": "city",
      "prefixes": {"sdm": {"prefix_prefix": "sdm",
                           "prefix_reference": "https://smartdatamodels.org/"}},
      "default_prefix": "sdm",
      "classes": {"Car": {"slots": ["speed", "owner_secret"]},
                  "Person": {"slots": ["name"]}},
      "slots": {"speed": {"range": "float"}, "owner_secret": {}, "name": {}}
    });
    let short = json!({
      "id": "cars", "tenant": "city",
      "selector": ["Car"],
      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
      "operations": READS
    });
    let app = broker(None, &short, Some(&sdm)).await.expect("a broker");
    let (s, _, car) = get(&app, "classes/Car").await;
    assert_eq!(s, StatusCode::OK, "{car}");
    let car: Value = serde_json::from_str(&car).expect("json");
    assert_eq!(
        car["iri"],
        "https://uri.etsi.org/ngsi-ld/default-context/Car"
    );
    assert_eq!(get(&app, "slots/speed").await.0, StatusCode::OK);
    for path in ["classes/Person", "slots/owner_secret", "slots/name"] {
        assert_eq!(get(&app, path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    let (s, _, index) = get(&app, "schema/index.json").await;
    assert_eq!(s, StatusCode::OK, "{index}");
    assert!(!index.contains("Person"), "{index}");

    let denied = broker(Some(Arc::new(NotCoreCarNotCoreSpeed)), &short, Some(&sdm))
        .await
        .expect("a broker");
    for path in ["classes/Car", "slots/speed"] {
        assert_eq!(get(&denied, path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
}
