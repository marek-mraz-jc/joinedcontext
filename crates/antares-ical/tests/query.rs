// SPDX-License-Identifier: EUPL-1.2
//! The feed as an NGSI-LD Query Entities (T-4901): the view's `representations.ical` mapping
//! renders it, a changed mapping changes it, `q` selects what the NGSI-LD answer to the same
//! `q` holds, other parameters are refused, a refused read is the NGSI-LD 403, and the preview
//! answers the admin token alone.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::{AdminAuth, TestBroker};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const FEED: &str = "/x/views/events/ical/v1/events.ics";
const PREVIEW: &str = "/x/views/events/ical/v1/preview";

/// Denies Query Entities when `deny` is set.
struct Reads {
    deny: bool,
}

impl PolicyEngine for Reads {
    fn name(&self) -> &'static str {
        "reads"
    }
    fn decide<'a>(&'a self, _: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let denied = self.deny && op.clause == "5.7.2";
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

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    admin: bool,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if uri.starts_with("/ngsi-ld") {
        req = req.header("NGSILD-Tenant", "city");
    }
    if admin {
        req = req.header("Authorization", format!("Bearer {TOKEN}"));
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

/// The view `events`; `None`: one output `events` of the `sdm-event` preset.
fn view(representations: Option<Value>) -> Value {
    let mut v = json!({"id": "events", "tenant": "city", "selector": ["Event"],
        "operations": ["query_entities"],
        "rules": [{"types": ["Event"], "attributes": ["name", "title", "startDate", "address"]}]});
    v["representations"] =
        representations.unwrap_or_else(|| json!({"ical": [{"name": "events", "sources": [{}]}]}));
    v
}

/// `representations` of one output `events` over `source`.
fn one(source: &Value) -> Value {
    json!({"ical": [{"name": "events", "sources": [source]}]})
}

async fn put_view(app: &Router, doc: &Value) -> (StatusCode, String) {
    send(app, "PUT", "/q/views/events", &doc.to_string(), true).await
}

fn event(id: &str, town: &str, title: &str) -> Value {
    json!({"id": format!("urn:ngsi-ld:Event:{id}"), "type": "Event",
           "name": {"type": "Property", "value": format!("Podujatie {id}")},
           "title": {"type": "Property", "value": title},
           "startDate": {"type": "Property", "value": "2026-11-02"},
           "address": {"type": "Property", "value": {"addressLocality": town}}})
}

async fn fixture(deny: bool) -> (TestBroker, Router) {
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(Arc::new(Reads { deny }))
        .with_admin_auth(AdminAuth::new(TOKEN, true).expect("token"));
    let app = broker.serve(antares_ical::Ical).expect("mounted");
    let (status, body) = put_view(&app, &view(None)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for e in [
        event("zv1", "Zvolen", "Jarmok"),
        event("zv2", "Zvolen", "Koncert"),
        event("bb1", "Banská Bystrica", "Divadlo"),
    ] {
        let (status, body) =
            send(&app, "POST", "/ngsi-ld/v1/entities", &e.to_string(), false).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    (broker, app)
}

fn uids(ics: &str) -> Vec<String> {
    let mut all: Vec<String> = ics
        .split("\r\n")
        .filter_map(|l| l.strip_prefix("UID:"))
        .map(str::to_owned)
        .collect();
    all.sort();
    all
}

fn encoded(q: &str) -> String {
    antares_ops::percent::encode(q)
}

#[tokio::test]
async fn q_selects_what_the_ngsi_ld_answer_to_the_same_q_holds() {
    let (_broker, app) = fixture(false).await;
    for q in [
        r#"address[addressLocality]=="Zvolen""#,
        r#"address[addressLocality]=="Banská Bystrica""#,
        r#"title=="Jarmok"|title=="Divadlo""#,
        r#"address.addressLocality=="Zvolen""#,
    ] {
        let (status, ics) = send(&app, "GET", &format!("{FEED}?q={}", encoded(q)), "", false).await;
        assert_eq!(status, StatusCode::OK, "{q}: {ics}");
        let uri = format!("/ngsi-ld/v1/entities?type=Event&q={}", encoded(q));
        let (status, ngsi) = send(&app, "GET", &uri, "", false).await;
        assert_eq!(status, StatusCode::OK, "{ngsi}");
        let mut ids: Vec<String> = serde_json::from_str::<Vec<Value>>(&ngsi)
            .expect("entities")
            .iter()
            .map(|e| e["id"].as_str().expect("id").to_owned())
            .collect();
        ids.sort();
        assert!(!ids.is_empty(), "{q}");
        assert_eq!(uids(&ics), ids, "{q}");
    }
    let zvolen = encoded(r#"address[addressLocality]=="Zvolen""#);
    let (_, ics) = send(
        &app,
        "GET",
        &format!("{FEED}?q={zvolen}&type=Event&from=2026-11-01"),
        "",
        false,
    )
    .await;
    assert_eq!(
        uids(&ics),
        ["urn:ngsi-ld:Event:zv1", "urn:ngsi-ld:Event:zv2"]
    );
}

#[tokio::test]
async fn a_parameter_of_no_query_a_repeated_one_another_type_and_a_broken_q_are_400() {
    let (_broker, app) = fixture(false).await;
    for query in [
        "town=Zvolen",
        "category=music",
        "q=title%3D%3D%22a%22&q=title%3D%3D%22b%22",
        "type=Place",
        "limit=5",
    ] {
        let (status, body) = send(&app, "GET", &format!("{FEED}?{query}"), "", false).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }
    let (status, body) = send(
        &app,
        "GET",
        &format!("{FEED}?q={}", encoded("title==")),
        "",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("BadRequestData"),
        "the NGSI-LD answer: {body}"
    );
}

#[tokio::test]
async fn a_read_the_policy_refuses_is_the_ngsi_ld_403() {
    let (_broker, app) = fixture(true).await;
    let (status, body) = send(&app, "GET", FEED, "", false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("AccessDenied"), "{body}");
}

#[tokio::test]
async fn the_views_mapping_renders_the_feed_and_a_new_mapping_changes_it() {
    let (_broker, app) = fixture(false).await;
    let (_, before) = send(&app, "GET", FEED, "", false).await;
    assert!(before.contains("SUMMARY:Podujatie zv1"), "{before}");
    let mapping = one(&json!({"preset": "sdm-event", "mapping": {
        "SUMMARY": "title", "LOCATION": "address.addressLocality"}}));
    let (status, body) = put_view(&app, &view(Some(mapping))).await;
    assert!(status.is_success(), "{body}");
    let (_, after) = send(&app, "GET", FEED, "", false).await;
    assert!(after.contains("SUMMARY:Jarmok"), "{after}");
    assert!(after.contains("LOCATION:Banská Bystrica"), "{after}");
    assert!(!after.contains("Podujatie"), "{after}");
    // a representation the broker does not know, outputs that are no list (more than 20 load
    // and serve nobody: tests/outputs.rs)
    for bad in [
        json!({"icall": []}),
        json!({"ical": "sdm-event"}),
        json!({"ical": {"preset": "sdm-event"}}),
    ] {
        let (status, body) = put_view(&app, &view(Some(bad.clone()))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
}

#[tokio::test]
async fn a_stored_mapping_that_does_not_hold_serves_no_feed_and_says_so() {
    let (broker, app) = fixture(false).await;
    let no_start = one(&json!({"mapping": {"SUMMARY": "title"}}));
    broker
        .store_view_doc("events", view(Some(no_start)))
        .await
        .expect("stored");
    let (status, body) = send(&app, "GET", FEED, "", false).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.contains("published again"), "{body}");
    assert!(!body.contains("SUMMARY"), "names no field: {body}");
}

#[tokio::test]
async fn the_preview_answers_the_admin_token_alone_and_renders_a_candidate() {
    let (_broker, app) = fixture(false).await;
    let body =
        json!({"mapping": {"preset": "sdm-event", "mapping": {"SUMMARY": "missing"}}, "limit": 10})
            .to_string();
    let (status, _) = send(&app, "POST", PREVIEW, &body, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &app,
        "POST",
        "/x/views/nowhere/ical/v1/preview",
        &body,
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "names no view to a caller without the token"
    );
    let (status, answer) = send(&app, "POST", PREVIEW, &body, true).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let answer: Value = serde_json::from_str(&answer).expect("json");
    assert_eq!(answer["read"], 3);
    assert_eq!(answer["valid"], 0);
    assert_eq!(answer["skipped"]["missing-summary"], 3);
    let good =
        json!({"mapping": {"preset": "sdm-event", "mapping": {"SUMMARY": "title"}}}).to_string();
    let zvolen = encoded(r#"address[addressLocality]=="Zvolen""#);
    let (status, answer) = send(&app, "POST", &format!("{PREVIEW}?q={zvolen}"), &good, true).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let answer: Value = serde_json::from_str(&answer).expect("json");
    assert_eq!(
        (answer["read"].clone(), answer["valid"].clone()),
        (json!(2), json!(2))
    );
    let events = answer["events"].as_array().expect("events");
    assert!(
        events[0]
            .as_str()
            .expect("text")
            .starts_with("BEGIN:VEVENT\r\n"),
        "{events:?}"
    );
    // the feed did not change: nothing was stored
    let (_, feed) = send(&app, "GET", FEED, "", false).await;
    assert!(feed.contains("SUMMARY:Podujatie zv1"), "{feed}");
}

#[tokio::test]
async fn a_preview_over_its_bounds_or_with_a_bad_mapping_is_refused() {
    let (_broker, app) = fixture(false).await;
    for (body, want) in [
        (
            json!({"mapping": {}, "limit": 51}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"mapping": {}, "limit": 0}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"mapping": {"mapping": {"UID": "title"}}}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"mapping": {}, "store": true}).to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "x".repeat(antares_ical::MAX_PREVIEW_BYTES + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let (status, answer) = send(&app, "POST", PREVIEW, &body, true).await;
        assert_eq!(status, want, "{}: {answer}", &body[..body.len().min(60)]);
    }
    let (status, _) = send(&app, "GET", PREVIEW, "", true).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn attrs_select_what_the_feed_reads_and_a_bad_bound_or_view_segment_is_refused() {
    let (_broker, app) = fixture(false).await;
    let (status, ics) = send(
        &app,
        "GET",
        &format!("{FEED}?attrs=name,startDate"),
        "",
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ics}");
    assert_eq!(uids(&ics).len(), 3, "{ics}");
    assert!(!ics.contains("LOCATION"), "address not selected: {ics}");
    let (_, ics) = send(&app, "GET", &format!("{FEED}?attrs=startDate"), "", false).await;
    assert!(uids(&ics).is_empty(), "no SUMMARY without name: {ics}");
    let (status, _) = send(&app, "GET", &format!("{FEED}?to=later"), "", false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&app, "GET", "/x/views/%FF/ical/v1/events.ics", "", false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_stored_mapping_of_an_unknown_shape_serves_no_feed() {
    let (broker, app) = fixture(false).await;
    let unknown = one(&json!({"preset": "sdm-event", "template": "{{name}}"}));
    broker
        .store_view_doc("events", view(Some(unknown)))
        .await
        .expect("stored");
    let (status, body) = send(&app, "GET", FEED, "", false).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
}

#[tokio::test]
async fn a_page_past_the_answer_bound_is_the_ngsi_ld_refusal() {
    let broker = TestBroker::with_max_answer_bytes(64).await.expect("broker");
    let app = broker.serve(antares_ical::Ical).expect("mounted");
    let (status, body) = put_view(&app, &view(None)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let e = event("zv1", "Zvolen", "Jarmok");
    let (status, _) = send(&app, "POST", "/ngsi-ld/v1/entities", &e.to_string(), false).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = send(&app, "GET", FEED, "", false).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert!(body.contains("BadRequestData"), "{body}");
}

#[tokio::test]
async fn a_preview_takes_the_feeds_query_and_counts_what_falls_outside_its_window() {
    let (_broker, app) = fixture(false).await;
    let extra = event("zv3", "Zvolen", "Výstava");
    let (status, _) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entities",
        &extra.to_string(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let body = json!({"mapping": {}}).to_string();
    let (_, answer) = send(&app, "POST", PREVIEW, &body, true).await;
    let answer: Value = serde_json::from_str(&answer).expect("json");
    assert_eq!(
        (
            answer["valid"].clone(),
            answer["events"].as_array().map(Vec::len)
        ),
        (json!(4), Some(3))
    );
    let (_, answer) = send(
        &app,
        "POST",
        &format!("{PREVIEW}?from=2026-12-01"),
        &body,
        true,
    )
    .await;
    let answer: Value = serde_json::from_str(&answer).expect("json");
    assert_eq!(
        (answer["valid"].clone(), answer["outside"].clone()),
        (json!(0), json!(4))
    );
    for (uri, want) in [
        (format!("{PREVIEW}?town=Zvolen"), StatusCode::BAD_REQUEST),
        (
            format!("{PREVIEW}?q={}", encoded("title==")),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/x/views/nowhere/ical/v1/preview".to_owned(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, answer) = send(&app, "POST", &uri, &body, true).await;
        assert_eq!(status, want, "{uri}: {answer}");
    }
    let unknown = json!({"mapping": {"preset": "outlook"}}).to_string();
    let (status, answer) = send(&app, "POST", PREVIEW, &unknown, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(answer.contains("does not hold"), "{answer}");
}
