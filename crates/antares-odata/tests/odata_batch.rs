// SPDX-License-Identifier: EUPL-1.2
//! `$batch` (odata-linkml-mapping section 6 rows W24 to W26,
//! surfaces-threat-model O8, O9, O11, O12): each member is served as a
//! request of its own and gated alone, under the outer request's tenant
//! and subject; a member outside this service root, a member header that is
//! not an OData one or a bound passed refuses the whole batch before any
//! member runs.

mod writes;

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;
use writes::{city, city_on, odata, stored, CITY, ROOT};

/// Records the tenant and the subject of every decision asked of it, and
/// allows.
#[derive(Default)]
struct Recording(Mutex<Vec<(String, String)>>);

impl Recording {
    fn take(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl PolicyEngine for Recording {
    fn name(&self) -> &'static str {
        "recording"
    }
    fn decide<'a>(&'a self, subject: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        let who = subject
            .headers
            .iter()
            .find(|(k, _)| k == "x-subject")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((subject.tenant.to_string(), who));
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn broker() -> Result<(TestBroker, Router, Arc<Recording>), String> {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-subject");
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let engine = Arc::new(Recording::default());
    let b = TestBroker::new().await?.with_policy(engine.clone());
    let (b, app) = city_on(b).await?;
    engine.take();
    Ok((b, app, engine))
}

/// A batch as user `alice` of Tenant `city`: status, headers, body.
async fn batch(
    app: &Router,
    content_type: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(format!("{ROOT}/$batch"))
        .header("Host", "broker:8080")
        .header("NGSILD-Tenant", "city")
        .header("X-Subject", "alice")
        .header("Content-Type", content_type)
        .header("Content-Length", body.len());
    if !headers.iter().any(|(k, _)| *k == "OData-MaxVersion") {
        req = req.header("OData-MaxVersion", "4.01");
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
        .await;
    let (parts, body) = resp.into_parts();
    let bytes = body
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        parts.status,
        parts.headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

/// A JSON batch of `members`, answered as JSON.
async fn json_batch(
    app: &Router,
    members: &Value,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let body = json!({ "requests": members }).to_string();
    let (status, headers, text) = batch(app, "application/json", &body, headers).await;
    (
        status,
        headers,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// The statuses of a JSON batch response, in order.
fn statuses(answer: &Value) -> Vec<u64> {
    answer["responses"]
        .as_array()
        .map(|r| r.iter().filter_map(|m| m["status"].as_u64()).collect())
        .unwrap_or_default()
}

/// A `multipart/mixed` body of `(Content-ID, request)` parts, boundary `b`.
fn multipart(parts: &[(&str, &str)]) -> String {
    let mut parts: Vec<String> = parts
        .iter()
        .map(|(id, request)| {
            format!(
                "--b\r\nContent-Type: application/http\r\nContent-ID: {id}\r\n\r\n{request}\r\n"
            )
        })
        .collect();
    parts.push("--b--\r\n".to_owned());
    parts.concat()
}

const MULTIPART: &str = "multipart/mixed; boundary=b";

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_members_are_gated_one_by_one() {
    let (_b, app, engine) = broker().await.expect("broker");
    let members = [
        ("1", Method::GET, "Car", None),
        ("2", Method::GET, "Car('urn:ngsi-ld:Car:1')", None),
        (
            "3",
            Method::PATCH,
            "Car('urn:ngsi-ld:Car:1')",
            Some(r#"{"label":"b"}"#),
        ),
        (
            "4",
            Method::PUT,
            "Car('urn:ngsi-ld:Car:1')/speed",
            Some(r#"{"value":3}"#),
        ),
        ("5", Method::DELETE, "Car('urn:ngsi-ld:Car:1')/label", None),
    ];
    let mut alone = Vec::new();
    for (_, method, url, body) in &members {
        let (status, answer) = odata(&app, method.clone(), url, *body).await;
        assert!(status.is_success(), "{method} {url}: {answer}");
        alone.push(engine.take().len());
    }
    let requests: Vec<Value> = members
        .iter()
        .map(|(id, method, url, body)| {
            let mut m = json!({"id": id, "method": method.as_str(), "url": url});
            if let Some(body) = body {
                m["body"] = serde_json::from_str(body).unwrap_or_default();
                m["headers"] = json!({"Content-Type": "application/json"});
            }
            m
        })
        .collect();
    let (status, headers, answer) = json_batch(&app, &Value::from(requests), &[]).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(statuses(&answer), [200, 200, 204, 204, 204], "{answer}");
    assert_eq!(answer["responses"][1]["body"]["speed"], 3, "{answer}");
    let decisions = engine.take();
    assert_eq!(
        decisions.len(),
        alone.iter().sum::<usize>(),
        "one gate per member, as each alone: {decisions:?}"
    );
    assert!(
        decisions.iter().all(|(t, s)| t == "city" && s == "alice"),
        "{decisions:?}"
    );
    let (_, car) = stored(&app, "urn:ngsi-ld:Car:1").await;
    assert_eq!(car[format!("{CITY}speed")]["value"], 3, "{car}");
    assert_eq!(car.get(format!("{CITY}label")), None, "{car}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_part_headers_cannot_change_tenant_or_subject() {
    let (_b, app, engine) = broker().await.expect("broker");
    let write = "PATCH Car('urn:ngsi-ld:Car:1') HTTP/1.1\r\nContent-Type: application/json\r\n";
    for (name, value) in [
        ("NGSILD-Tenant", "other"),
        ("NGSILD-Snapshot", "1"),
        ("X-Subject", "mallory"),
        ("Authorization", "Bearer x"),
        ("Link", "<https://example.org/c.jsonld>"),
        ("Prefer", "ngsi-ld=x"),
    ] {
        let part = format!("{write}{name}: {value}\r\n\r\n{{\"label\":\"x\"}}");
        let (status, _, answer) = batch(&app, MULTIPART, &multipart(&[("1", &part)]), &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {answer}");
        let member = json!([{"id": "1", "method": "patch", "url": "Car('urn:ngsi-ld:Car:1')",
                             "headers": {"Content-Type": "application/json", name: value},
                             "body": {"label": "x"}}]);
        let (status, _, answer) = json_batch(&app, &member, &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {answer}");
    }
    assert_eq!(engine.take(), [], "no member ran");
    let (_, car) = stored(&app, "urn:ngsi-ld:Car:1").await;
    assert_eq!(car[format!("{CITY}label")]["value"], "one", "{car}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_part_bodies_are_depth_checked() {
    let (_b, app, engine) = broker().await.expect("broker");
    let deep = format!("{}1{}", "[".repeat(17), "]".repeat(17));
    let part = format!(
        "PATCH Car('urn:ngsi-ld:Car:1') HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{{\"notes\":{deep}}}"
    );
    let first = "GET Car HTTP/1.1\r\n";
    let body = multipart(&[("1", first), ("2", &part)]);
    let (status, _, answer) = batch(&app, MULTIPART, &body, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer.contains("JSON nesting depth"), "{answer}");
    assert_eq!(engine.take(), [], "not even the member before it ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_part_count_is_capped() {
    let (_b, app, engine) = broker().await.expect("broker");
    // the surface's cap: the library's strict limit, never above the broker's batch bound
    let cap = odata_batch::Limits::strict()
        .max_requests
        .min(*antares_ops::bounds::MAX_BATCH_ITEMS);
    let members = |n: usize| -> Value {
        (0..n)
            .map(|i| json!({"id": i.to_string(), "method": "get", "url": "Car"}))
            .collect()
    };
    let (status, _, answer) = json_batch(&app, &members(cap + 1), &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(engine.take(), [], "no JSON member ran");

    let ids: Vec<String> = (0..=cap).map(|i| i.to_string()).collect();
    let parts: Vec<(&str, &str)> = ids
        .iter()
        .map(|id| (id.as_str(), "GET Car HTTP/1.1\r\n"))
        .collect();
    let (status, _, answer) = batch(&app, MULTIPART, &multipart(&parts), &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(engine.take(), [], "no multipart member ran");

    let (status, _, answer) = json_batch(&app, &members(cap), &[]).await;
    assert_eq!(status, StatusCode::OK, "the cap itself is served: {answer}");
    assert_eq!(engine.take().len(), cap, "every member is gated alone");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_refuses_a_part_outside_this_service_root() {
    let (_b, app, engine) = broker().await.expect("broker");
    for url in [
        "/x/views/reads/odata/v4/Car",
        "/q/views/cars",
        "/ex/anything",
        "/ngsi-ld/v1/entities",
        "../reads/odata/v4/Car",
        "Car/%2e%2e/%2E%2E/reads/odata/v4/Car",
        "Car('urn:ngsi-ld:Car:1%2F..')",
    ] {
        let part = format!("GET {url} HTTP/1.1\r\n");
        let body = multipart(&[("1", "GET Car HTTP/1.1\r\n"), ("2", &part)]);
        let (status, _, answer) = batch(&app, MULTIPART, &body, &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {answer}");
    }
    assert_eq!(engine.take(), [], "no member ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_member_for_another_host_is_400() {
    let (_b, app, engine) = broker().await.expect("broker");
    let url = |host: &str| format!("GET http://{host}{ROOT}/Car HTTP/1.1\r\n");
    let (status, _, answer) =
        batch(&app, MULTIPART, &multipart(&[("1", &url("evil:80"))]), &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(engine.take(), []);
    let (status, headers, answer) = batch(
        &app,
        MULTIPART,
        &multipart(&[("1", &url("BROKER:8080"))]),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let content_type = headers["content-type"].to_str().unwrap_or_default();
    let boundary = content_type
        .strip_prefix("multipart/mixed; boundary=")
        .unwrap_or_default();
    assert!(!boundary.is_empty(), "{content_type}");
    assert!(
        answer.starts_with(&format!("--{boundary}\r\nContent-Type: application/http\r\nContent-ID: 1\r\n\r\nHTTP/1.1 200 OK\r\n")),
        "{answer}"
    );
    assert!(answer.contains("urn:ngsi-ld:Car:1"), "{answer}");
    assert!(answer.ends_with(&format!("--{boundary}--\r\n")), "{answer}");
    // An absolute-form request names its host in the URI (HTTP/2 too).
    let body = multipart(&[("1", &url("broker:8080"))]);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("http://broker:8080{ROOT}/$batch"))
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", MULTIPART)
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_content_id_reference_is_501() {
    let (_b, app, engine) = broker().await.expect("broker");
    let members = json!([{"id": "1", "method": "get", "url": "Car('urn:ngsi-ld:Car:1')"},
                         {"id": "2", "method": "get", "url": "$1/speed"}]);
    let (status, _, answer) = json_batch(&app, &members, &[]).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
    assert_eq!(engine.take(), []);
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_json_goes_on_unless_asked_to_stop() {
    let (_b, app, _engine) = broker().await.expect("broker");
    // Car 2 is outside the view: its read fails as an absent one would.
    let members = json!([
        {"id": "1", "method": "get", "url": "Car('urn:ngsi-ld:Car:2')"},
        {"id": "2", "method": "patch", "url": "Car('urn:ngsi-ld:Car:1')", "dependsOn": ["1"],
         "headers": {"Content-Type": "application/json"}, "body": {"label": "x"}},
        {"id": "3", "method": "get", "url": "Car('urn:ngsi-ld:Car:1')"}
    ]);
    // JSON Format 4.01 section 19.4: without the preference, or with it true, every member runs.
    for prefer in [
        None,
        Some("odata.continue-on-error"),
        Some("continue-on-error=true"),
    ] {
        let headers: Vec<_> = prefer.map(|p| ("Prefer", p)).into_iter().collect();
        let (status, headers, answer) = json_batch(&app, &members, &headers).await;
        assert_eq!(
            (status, statuses(&answer)),
            (StatusCode::OK, vec![404, 424, 200]),
            "{prefer:?}: {answer}"
        );
        assert_eq!(answer["responses"][1]["id"], "2");
        assert_eq!(
            headers
                .get("preference-applied")
                .and_then(|v| v.to_str().ok()),
            prefer.map(|_| "odata.continue-on-error"),
            "{prefer:?}"
        );
    }
    let stop = [("Prefer", "continue-on-error=false")];
    let (status, headers, answer) = json_batch(&app, &members, &stop).await;
    assert_eq!(
        (status, statuses(&answer)),
        (StatusCode::OK, vec![404]),
        "{answer}"
    );
    assert!(headers.get("preference-applied").is_none());
    let (_, car) = stored(&app, "urn:ngsi-ld:Car:1").await;
    assert_eq!(
        car[format!("{CITY}label")]["value"],
        "one",
        "a failed dependency writes nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_multipart_stops_at_the_first_failure_unless_asked_to_go_on() {
    let (_b, app, _engine) = broker().await.expect("broker");
    let body = multipart(&[
        ("1", "GET Car('urn:ngsi-ld:Car:2') HTTP/1.1\r\n"),
        ("2", "GET Car('urn:ngsi-ld:Car:1') HTTP/1.1\r\n"),
    ]);
    // Protocol 4.01 section 11.7.7.4: processing stops on the first error.
    let (status, _, text) = batch(&app, MULTIPART, &body, &[]).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(text.matches("HTTP/1.1 ").count(), 1, "{text}");
    assert!(text.contains("HTTP/1.1 404"), "{text}");
    let go_on = [("Prefer", "odata.continue-on-error")];
    let (status, headers, text) = batch(&app, MULTIPART, &body, &go_on).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(
        text.contains("HTTP/1.1 404") && text.contains("HTTP/1.1 200"),
        "{text}"
    );
    assert_eq!(headers["preference-applied"], "odata.continue-on-error");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_json_under_odata_4_0_is_400() {
    let (_b, app, engine) = broker().await.expect("broker");
    let members = json!([{"id": "1", "method": "get", "url": "Car('urn:ngsi-ld:Car:1')"}]);
    let (status, _, answer) = json_batch(&app, &members, &[("OData-MaxVersion", "4.0")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["error"]["code"], "BadRequest", "{answer}");
    assert_eq!(engine.take(), [], "no member runs");
    // Without OData-MaxVersion a JSON batch reads.
    let body = json!({ "requests": members }).to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("{ROOT}/$batch"))
        .header("Host", "broker:8080")
        .header("NGSILD-Tenant", "city")
        .header("X-Subject", "alice")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // A multipart batch is a 4.0 format and still reads under 4.0.
    let body = multipart(&[("1", "GET Car('urn:ngsi-ld:Car:1') HTTP/1.1\r\n")]);
    let (status, _, text) = batch(&app, MULTIPART, &body, &[("OData-MaxVersion", "4.0")]).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(text.contains("HTTP/1.1 200"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_member_is_refused_like_the_request_alone() {
    let (_b, app) = city().await.expect("broker");
    let go_on = [("Prefer", "odata.continue-on-error")];
    let members = json!([
        {"id": "1", "method": "post", "url": "Car/$query",
         "headers": {"Content-Type": "text/plain"}, "body": "$select=speed"},
        {"id": "2", "method": "post", "url": "Car/$query",
         "headers": {"Content-Type": "text/csv"}, "body": "x"},
        {"id": "3", "method": "get", "url": "Car?$filter=label eq 'one'"},
        {"id": "4", "method": "get", "url": "$metadata"},
        {"id": "5", "method": "get", "url": "Car?$apply=expand(owner)"}
    ]);
    let (status, _, answer) = json_batch(&app, &members, &go_on).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(statuses(&answer), [200, 415, 400, 200, 501], "{answer}");
    assert_eq!(
        answer["responses"][0]["body"]["value"][0]["speed"], 1.0,
        "{answer}"
    );
}

/// T-3773: a member whose answer is past the broker's buffer bound answers
/// 413 inside the batch; the members under it still answer.
#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_member_past_the_buffer_bound_is_413() {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::with_max_answer_bytes(512)
        .await
        .expect("broker");
    let (_b, app) = city_on(b).await.expect("city");
    let members = json!([
        {"id": "1", "method": "get", "url": "$metadata"},
        {"id": "2", "method": "get", "url": "Car?$top=1&$select=speed"}
    ]);
    let go_on = [("Prefer", "odata.continue-on-error")];
    let (status, _, answer) = json_batch(&app, &members, &go_on).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(statuses(&answer), [413, 200], "{answer}");
    assert_eq!(
        answer["responses"][0]["body"]["error"]["code"], "PayloadTooLarge",
        "{answer}"
    );
}

/// T-5895: a member whose answer the broker's buffered-answer pool has no
/// room for answers 503 with Retry-After inside the batch, refused whole;
/// the members under it still answer.
#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_member_past_the_answer_pool_is_503() {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let b = TestBroker::with_answer_pool(512).await.expect("broker");
    let (_b, app) = city_on(b).await.expect("city");
    let members = json!([
        {"id": "1", "method": "get", "url": "$metadata"},
        {"id": "2", "method": "get", "url": "Car?$top=1&$select=speed"}
    ]);
    let go_on = [("Prefer", "odata.continue-on-error")];
    let (status, _, answer) = json_batch(&app, &members, &go_on).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(statuses(&answer), [503, 200], "{answer}");
    assert_eq!(
        answer["responses"][0]["body"]["error"]["code"], "ServiceUnavailable",
        "{answer}"
    );
    assert_eq!(
        answer["responses"][0]["headers"]["retry-after"], "1",
        "{answer}"
    );
}

/// F-ANT4-004: the member answers one batch buffers share the broker's
/// `max_answer_bytes`; the member that passes it and every later one answer
/// 413, and the batch framing stays valid.
#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_members_past_the_shared_answer_bound_are_413() {
    std::env::set_var(antares_ops::views::SCHEMA_MAX_ENV, "65536");
    let get = json!({"id": "1", "method": "get", "url": "Car?$select=speed"});
    let (_a, plain) = city_on(TestBroker::new().await.expect("broker"))
        .await
        .expect("city");
    let (status, _, one) = json_batch(&plain, &json!([get]), &[]).await;
    assert_eq!(status, StatusCode::OK, "{one}");
    let member = one["responses"][0]["body"].to_string().len();
    let bound = member.saturating_add(member.div_ceil(2));
    let b = TestBroker::with_max_answer_bytes(bound)
        .await
        .expect("broker");
    let (_b, app) = city_on(b).await.expect("city");
    let members = json!([
        {"id": "1", "method": "get", "url": "Car?$select=speed"},
        {"id": "2", "method": "get", "url": "Car?$select=speed"},
        {"id": "3", "method": "get", "url": "Car?$select=speed"}
    ]);
    let go_on = [("Prefer", "odata.continue-on-error")];
    let (status, _, answer) = json_batch(&app, &members, &go_on).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(statuses(&answer), [200, 413, 413], "{answer}");
    assert_eq!(
        answer["responses"][1]["body"]["error"]["code"], "PayloadTooLarge",
        "{answer}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn odata_batch_itself_is_refused_whole() {
    let (_b, app, engine) = broker().await.expect("broker");
    let get = multipart(&[("1", "GET Car HTTP/1.1\r\n")]);
    for (content_type, body, headers, want) in [
        ("text/plain", get.as_str(), &[][..], StatusCode::BAD_REQUEST),
        (
            MULTIPART,
            "--b\r\nnot a part",
            &[][..],
            StatusCode::BAD_REQUEST,
        ),
        (
            MULTIPART,
            get.as_str(),
            &[("OData-Version", "3.0")][..],
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _, answer) = batch(&app, content_type, body, headers).await;
        assert_eq!(status, want, "{content_type} {headers:?}: {answer}");
    }
    for (method, path) in [(Method::PUT, "$batch"), (Method::DELETE, "$batch")] {
        let (status, answer) = odata(&app, method.clone(), path, Some("{}")).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path}: {answer}"
        );
    }
    let entity = "$entity?$id=Car('urn:ngsi-ld:Car:1')";
    let (status, answer) = odata(&app, Method::DELETE, entity, None).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
    let change_set = "--b\r\nContent-Type: multipart/mixed; boundary=c\r\n\r\n--c--\r\n--b--\r\n";
    let (status, _, answer) = batch(&app, MULTIPART, change_set, &[]).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
    assert_eq!(engine.take(), []);
}
