// SPDX-License-Identifier: EUPL-1.2
//! What one page of a `{Set}_history` set costs (T-7518): the rows it judges and the bytes it
//! holds are bounded, a page past either bound answers a next link that resumes at that row,
//! anonymous and signed-in callers alike, and what it builds is charged to the answer pool.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Map, Value};
use tower::ServiceExt;

const CITY: &str = "https://example.org/city/";
const PAST: &str = "/x/views/past/odata/v4";
const SUBJECT: &str = "x-test-subject";
/// The instances of each of the 30 Attributes of Wide:1, each at a time of its own.
const EACH: u32 = 400;

fn second(n: u32) -> String {
    format!(
        "2026-09-01T{:02}:{:02}:{:02}Z",
        n / 3_600,
        n % 3_600 / 60,
        n % 60
    )
}

/// Wide:1 (30 Attributes, 12 000 rows) and Heavy:1 (a 256 KiB text valid over 50 rows).
async fn fixture() -> Result<(TestBroker, Router), String> {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", SUBJECT);
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_odata::OData)?;
    let wide: Vec<String> = (0..30).map(|i| format!("a{i}")).collect();
    let mut slots: Map<String, Value> = wide
        .iter()
        .map(|name| (name.clone(), json!({"range": "float"})))
        .collect();
    slots.insert("blob".into(), json!({"range": "string"}));
    slots.insert("tick".into(), json!({"range": "float"}));
    let schema = json!({"id": "https://example.org/city", "name": "city",
        "prefixes": {"city": {"prefix_prefix": "city", "prefix_reference": CITY}},
        "default_prefix": "city", "slots": slots,
        "classes": {"Wide": {"slots": wide}, "Heavy": {"slots": ["blob", "tick"]}}});
    let named =
        |names: &[String]| -> Vec<String> { names.iter().map(|n| format!("city:{n}")).collect() };
    let view = json!({"id": "past", "tenant": "city", "@context": {"city": CITY},
        "selector": ["city:Wide", "city:Heavy"],
        "rules": [{"types": ["city:Wide"], "attributes": named(&wide)},
                  {"types": ["city:Heavy"], "attributes": ["city:blob", "city:tick"]}],
        "operations": ["query_entities", "retrieve_entity", "temporal_query"]});
    for (uri, body) in [("/q/schemas/city", schema), ("/q/views/past", view)] {
        let (status, answer) = send(&app, Method::PUT, uri, &body).await?;
        if status != StatusCode::CREATED {
            return Err(format!("{uri}: {status} {answer}"));
        }
    }
    let at =
        |value: Value, n: u32| json!({"type": "Property", "value": value, "observedAt": second(n)});
    let mut histories = Vec::new();
    for (i, name) in (0u32..).zip(&wide) {
        let list: Vec<Value> = (0..EACH)
            .map(|k| k.saturating_mul(30).saturating_add(i))
            .map(|n| at(json!(f64::from(n)), n))
            .collect();
        histories.push(("Wide", name.clone(), list));
    }
    let blob = at(json!("x".repeat(256 * 1024)), 0);
    histories.push(("Heavy", "blob".to_owned(), vec![blob]));
    let ticks = (0..50).map(|n| at(json!(f64::from(n)), n)).collect();
    histories.push(("Heavy", "tick".to_owned(), ticks));
    for (class, name, list) in histories {
        let mut body = Map::new();
        body.insert("id".into(), json!(format!("urn:ngsi-ld:{class}:1")));
        body.insert("type".into(), json!(format!("{CITY}{class}")));
        body.insert(format!("{CITY}{name}"), Value::Array(list));
        let uri = "/ngsi-ld/v1/temporal/entities";
        let (status, answer) = send(&app, Method::POST, uri, &Value::Object(body)).await?;
        if !status.is_success() {
            return Err(format!("{class} {name}: {status} {answer}"));
        }
    }
    Ok((broker, app))
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: &Value,
) -> Result<(StatusCode, String), String> {
    let body = body.to_string();
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .map_err(|e| e.to_string())?;
    answered(app, request).await
}

async fn answered(app: &Router, request: Request<Body>) -> Result<(StatusCode, String), String> {
    let Ok(response) = app.clone().oneshot(request).await;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

async fn get(app: &Router, uri: &str, subject: Option<&str>) -> Result<Value, String> {
    let mut request = Request::get(uri)
        .header("NGSILD-Tenant", "city")
        .header("OData-MaxVersion", "4.01");
    if let Some(subject) = subject {
        request = request.header(SUBJECT, subject);
    }
    let request = request.body(Body::empty()).map_err(|e| e.to_string())?;
    let (status, body) = answered(app, request).await?;
    if status != StatusCode::OK {
        return Err(format!("{status} {body}"));
    }
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// Each page's `observedAt`s, the next links followed to the end.
async fn pages(
    app: &Router,
    query: &str,
    subject: Option<&str>,
) -> Result<Vec<Vec<String>>, String> {
    let mut uri = format!("{PAST}/{query}");
    let mut out = Vec::new();
    while out.len() < 200 {
        let body = get(app, &uri, subject).await?;
        let rows = body.get("value").and_then(Value::as_array);
        out.push(
            rows.into_iter()
                .flatten()
                .filter_map(|r| r.get("observedAt")?.as_str().map(str::to_owned))
                .collect(),
        );
        let link = body
            .get("@nextLink")
            .or_else(|| body.get("@odata.nextLink"))
            .and_then(Value::as_str);
        let Some(link) = link else { return Ok(out) };
        let from = link.find("/x/").ok_or_else(|| link.to_owned())?;
        link.get(from..).unwrap_or_default().clone_into(&mut uri);
    }
    Err("the next links do not end".to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_past_the_judged_bound_answers_a_next_link_that_resumes_there() {
    let (_broker, app) = fixture().await.expect("fixture");
    for subject in [None, Some("someone")] {
        let last = EACH * 30 - 1;
        let got = pages(
            &app,
            &format!("Wide_history?$filter=a29%20eq%20{last}"),
            subject,
        )
        .await
        .expect("walk");
        assert_eq!(got, [vec![], vec![second(last)]], "{subject:?}");

        let none = pages(&app, "Wide_history?$filter=a0%20eq%20-1", subject)
            .await
            .expect("walk");
        assert_eq!(none, [Vec::<String>::new(), vec![]], "{subject:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_value_ends_a_page_at_its_bytes_and_the_links_reach_every_row_once() {
    let (_broker, app) = fixture().await.expect("fixture");
    for subject in [None, Some("someone")] {
        let got = pages(&app, "Heavy_history", subject).await.expect("walk");
        for page in &got {
            assert!(
                page.len() * 256 * 1024 <= 8 * 1024 * 1024,
                "{subject:?}: a page of {} rows",
                page.len()
            );
        }
        let rows: Vec<String> = got.into_iter().flatten().collect();
        let all: Vec<String> = (0..50).map(second).collect();
        assert_eq!(rows, all, "{subject:?}");
        let skipped = pages(&app, "Heavy_history?$skip=3&$top=44", subject)
            .await
            .expect("walk");
        let rows: Vec<String> = skipped.into_iter().flatten().collect();
        let part: Vec<String> = (3..47).map(second).collect();
        assert_eq!(rows, part, "{subject:?}: $skip and $top over the links");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wide_page_charges_the_rows_it_builds_and_no_others() {
    let (broker, app) = fixture().await.expect("fixture");
    let pool = broker.answer_pool();
    let mut peaks = Vec::new();
    for top in [1, 100] {
        pool.take_peak();
        let uri = format!("{PAST}/Wide_history?$top={top}");
        get(&app, &uri, None).await.expect("page");
        peaks.push(pool.take_peak());
    }
    let [one, hundred] = peaks[..] else {
        panic!("two pages")
    };
    assert!(
        hundred > one,
        "the rows built are charged: {one}, {hundred}"
    );
    // 12 000 rows of 30 members would be tens of MB: a page holds its own 100.
    assert!(hundred - one < 100 * 8 * 1024, "{one}, {hundred}");
}
