// SPDX-License-Identifier: EUPL-1.2
//! `antares-jsonstat` against a broker (T-4490): the view narrows what a dataset holds, the
//! Policy decides each read, the surface writes nothing, and the ŠÚ SR table `om7014rr`
//! (`tests/jsonstat/om7014rr.json`, as `DATAcube` answered it) comes back with the same values in
//! the same dimension order after the pipeline's mapping turned it into Entities.
//!
//! The Tenant `city` holds the cells of `om7014rr`, the sparse table `sparse` (two cells of a
//! 2 x 2 x 2 cube, names in sk and en, a unit) and the hidden `Secret` `s`. The view `city` shows
//! the cells without `isPartOf`; `secret` shows `Secret` alone; `writes` allows Create Entity
//! alone; `void` reads the empty Tenant `void`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::Surface as _;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

async fn send(app: &Router, method: &str, uri: &str, body: &str) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if uri.starts_with("/ngsi-ld") {
        req = req.header("NGSILD-Tenant", "city");
    }
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("infallible");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf-8"))
}

/// Denies every Query Entities (clause 5.7) once locked, as a Policy granting no read would.
#[derive(Default)]
struct NoReads(AtomicBool);

impl PolicyEngine for NoReads {
    fn name(&self) -> &'static str {
        "no-reads"
    }

    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let locked = self.0.load(Ordering::SeqCst);
        Box::pin(std::future::ready(
            if locked && op.clause.starts_with("5.7.") {
                Decision::Deny("no read".into())
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

const TABLE: &str = "/x/views/city/jsonstat/v1/om7014rr";

/// The Entities the pipeline's `datacube` step (jc-core `datacube.blobl`) writes of a `DATAcube`
/// answer: one per cell with a number, its territory, period and indicator by category code.
fn cells(cube: &Value) -> Vec<Value> {
    let ids: Vec<&str> = cube["id"]
        .as_array()
        .expect("id")
        .iter()
        .map(|d| d.as_str().expect("name"))
        .collect();
    let sizes: Vec<usize> = cube["size"]
        .as_array()
        .expect("size")
        .iter()
        .map(|n| usize::try_from(n.as_u64().expect("n")).expect("size"))
        .collect();
    let codes: Vec<Vec<String>> = ids
        .iter()
        .map(|d| {
            let index = cube["dimension"][d]["category"]["index"]
                .as_object()
                .expect("index");
            let mut list: Vec<(&String, u64)> = index
                .iter()
                .map(|(k, v)| (k, v.as_u64().expect("position")))
                .collect();
            list.sort_by_key(|(_, p)| *p);
            list.into_iter().map(|(k, _)| k.clone()).collect()
        })
        .collect();
    let code = cube["href"]
        .as_str()
        .expect("href")
        .split('/')
        .nth(7)
        .expect("code")
        .to_owned();
    let mut out = Vec::new();
    for (p, v) in cube["value"].as_array().expect("value").iter().enumerate() {
        if !v.is_number() {
            continue;
        }
        let (mut rest, mut at) = (p, vec![String::new(); ids.len()]);
        for k in (0..ids.len()).rev() {
            at[k].clone_from(&codes[k][rest.checked_rem(sizes[k]).expect("size")]);
            rest = rest.checked_div(sizes[k]).expect("size");
        }
        let label = cube["dimension"][ids[2]]["category"]["label"][&at[2]].clone();
        out.push(json!({
            "id": format!("urn:ngsi-ld:KeyPerformanceIndicator:{code}-{}-{}-{}", at[0], at[1], at[2]),
            "type": "KeyPerformanceIndicator",
            "dataSet": {"type": "Property", "value": code},
            "indicator": {"type": "Property", "value": at[2]},
            "name": {"type": "Property", "value": label},
            "refArea": {"type": "Property", "value": at[0]},
            "refPeriod": {"type": "Property", "value": at[1]},
            "kpiValue": {"type": "Property", "value": v, "observedAt": "2026-03-31T00:00:00Z"},
            "source": {"type": "Property", "value": "https://data.statistics.sk/api/v2/dataset/om7014rr/all/all/all?lang=sk&type=json"},
            "isPartOf": {"type": "Property", "value": "hidden-page"},
        }));
    }
    out
}

fn original() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/jsonstat/om7014rr.json"
    ))
    .expect("fixture");
    serde_json::from_str(&text).expect("json")
}

fn sparse() -> Vec<Value> {
    let kpi = |id: &str, area: &str, year: &str, value: f64| {
        json!({
        "id": format!("urn:ngsi-ld:KeyPerformanceIndicator:sparse-{id}"), "type": "KeyPerformanceIndicator",
        "dataSet": {"type": "Property", "value": "sparse"}, "indicator": {"type": "Property", "value": if id == "a" { "U1" } else { "U2" }},
        "name": {"type": "LanguageProperty", "languageMap": {"sk": format!("Ukazovateľ {id}"), "en": format!("Indicator {id}")}},
        "refArea": {"type": "Property", "value": area}, "refPeriod": {"type": "Property", "value": year},
        "unitText": {"type": "Property", "value": "osoba"},
        "kpiValue": {"type": "Property", "value": value}})
    };
    vec![kpi("a", "SK0", "2024", 1.5), kpi("b", "SK010", "2023", 0.0)]
}

async fn fixture_with(broker: TestBroker) -> Router {
    let app = broker.serve(antares_jsonstat::JsonStat).expect("mounted");
    let reads = json!(["query_entities"]);
    let ty = "KeyPerformanceIndicator";
    let shown = json!([
        "dataSet",
        "indicator",
        "name",
        "refArea",
        "refPeriod",
        "dimensionKey",
        "unitText",
        "kpiValue",
        "source"
    ]);
    let views = [
        json!({"id": "city", "tenant": "city", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}]}),
        json!({"id": "secret", "tenant": "city", "selector": ["Secret"], "operations": reads,
               "rules": [{"types": ["Secret"], "attributes": ["name"]}]}),
        json!({"id": "writes", "tenant": "city", "selector": [ty], "operations": ["create_entity"],
               "rules": [{"types": [ty], "attributes": ["name"]}]}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads,
               "rules": [{"types": ["*"], "attributes": ["*"]}]}),
        json!({"id": "void", "tenant": "void", "selector": [ty], "operations": reads,
               "rules": [{"types": [ty], "attributes": shown}]}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let (status, body) = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(status, StatusCode::CREATED, "view {id}: {body}");
    }
    let mut entities = cells(&original());
    entities.extend(sparse());
    entities.push(json!({"id": "urn:ngsi-ld:Secret:s", "type": "Secret", "name": {"type": "Property", "value": "hidden-secret"}}));
    let batch = Value::Array(entities).to_string();
    let (status, body) = send(&app, "POST", "/ngsi-ld/v1/entityOperations/create", &batch).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    app
}

async fn fixture() -> Router {
    fixture_with(TestBroker::new().await.expect("broker")).await
}

#[tokio::test]
async fn a_ds_sr_table_round_trips_with_its_values_and_dimension_order() {
    let app = fixture().await;
    let (status, body) = send(&app, "GET", TABLE, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !body.contains("hidden-page"),
        "a hidden Attribute leaked: {body}"
    );
    let got: Value = serde_json::from_str(&body).expect("json");
    let cube = original();
    // DATAcube's order: territory, period, indicator; its one-value `*_data` dimension names nothing
    assert_eq!(got["id"], json!(["refArea", "refPeriod", "indicator"]));
    assert_eq!(got["size"], json!([1, 30, 3]));
    assert_eq!(
        got["value"], cube["value"],
        "the same values in the same order"
    );
    for (ours, theirs) in [
        ("refArea", "om7014rr_obc"),
        ("refPeriod", "om7014rr_obd"),
        ("indicator", "om7014rr_ukaz"),
    ] {
        let index = cube["dimension"][theirs]["category"]["index"]
            .as_object()
            .expect("index");
        let mut codes: Vec<(&String, u64)> = index
            .iter()
            .map(|(k, v)| (k, v.as_u64().expect("p")))
            .collect();
        codes.sort_by_key(|(_, p)| *p);
        let codes: Vec<&String> = codes.into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            got["dimension"][ours]["category"]["index"],
            json!(codes),
            "{ours}"
        );
    }
    assert_eq!(
        got["dimension"]["indicator"]["category"]["label"],
        cube["dimension"]["om7014rr_ukaz"]["category"]["label"]
    );
    assert_eq!(got["updated"], "2026-03-31T00:00:00Z");
    assert!(got["source"].as_str().expect("source").contains("om7014rr"));
    let (status, every) = send(&app, "GET", "/x/views/every/jsonstat/v1/om7014rr", "").await;
    assert_eq!(status, StatusCode::OK, "{every}");
    assert!(
        every.contains("hidden-page"),
        "a view of every Attribute shows isPartOf: {every}"
    );
    let (status, body) = send(&app, "HEAD", TABLE, "").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, ""));
    let info = antares_jsonstat::JsonStat.version_info();
    assert_eq!(info["endpoint"], "/x/views/{view}/jsonstat/v1/{table}");
}

#[tokio::test]
async fn a_sparse_cube_holds_null_for_a_missing_cell_and_labels_follow_lang() {
    let app = fixture().await;
    let (status, body) = send(&app, "GET", "/x/views/city/jsonstat/v1/sparse?lang=en", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        got["id"],
        json!(["refArea", "refPeriod", "indicator", "unitText"])
    );
    assert_eq!(got["size"], json!([2, 2, 2, 1]));
    // SK0 2024 U1 is 1.5 and SK010 2023 U2 is 0; the six cells no Entity holds are null, never 0
    assert_eq!(
        got["value"],
        json!([1.5, null, null, null, null, null, null, 0.0])
    );
    assert_eq!(
        got["dimension"]["indicator"]["category"]["label"],
        json!({"U1": "Indicator a", "U2": "Indicator b"})
    );
    let (_, sk) = send(&app, "GET", "/x/views/city/jsonstat/v1/sparse?lang=sk", "").await;
    assert!(sk.contains("Ukazovateľ a"), "{sk}");
}

#[tokio::test]
async fn a_hidden_type_an_unknown_view_or_table_and_a_view_without_reads_are_404() {
    let app = fixture().await;
    for uri in [
        "/x/views/secret/jsonstat/v1/om7014rr",
        "/x/views/nowhere/jsonstat/v1/om7014rr",
        "/x/views/writes/jsonstat/v1/om7014rr",
        "/x/views/city/jsonstat/v1/nothing",
        "/x/views/city/jsonstat/v1/a%22b",
        "/x/views/void/jsonstat/v1/om7014rr",
    ] {
        let (status, body) = send(&app, "GET", uri, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(!body.contains("hidden-secret"), "{body}");
    }
}

#[tokio::test]
async fn the_policy_decides_every_read() {
    let engine = Arc::new(NoReads::default());
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(engine.clone());
    let app = fixture_with(broker).await;
    engine.0.store(true, Ordering::SeqCst);
    let (status, body) = send(&app, "GET", TABLE, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        !body.contains("IN010049"),
        "a denied read served data: {body}"
    );
}

#[tokio::test]
async fn the_surface_writes_nothing() {
    let app = fixture().await;
    let (status, _) = send(
        &app,
        "POST",
        TABLE,
        r#"{"id":"urn:ngsi-ld:KeyPerformanceIndicator:c","type":"KeyPerformanceIndicator"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(&app, "OPTIONS", TABLE, "").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:KeyPerformanceIndicator:c",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bad_parameters_are_400() {
    let app = fixture().await;
    for query in ["limit=1", "lang=sk&lang=en", "q=refArea==%22SK0%22"] {
        let (status, body) = send(&app, "GET", &format!("{TABLE}?{query}"), "").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
}

#[tokio::test]
async fn a_request_naming_another_tenant_is_refused() {
    let app = fixture().await;
    let req = Request::get(TABLE)
        .header("NGSILD-Tenant", "void")
        .body(Body::empty())
        .expect("request");
    let resp = app.oneshot(req).await.expect("infallible");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
