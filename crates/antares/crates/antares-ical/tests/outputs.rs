// SPDX-License-Identifier: EUPL-1.2
//! Named calendar outputs (owner 2026-10-22T09:25Z and 09:30Z, T-5208): a view without
//! `representations.ical` has no calendar, each output serves its own sources under its own
//! filter, `events.ics` is the first, and the calendar carries its titles, refresh interval,
//! zone and a strong `ETag`; an unchanged re-ingest leaves the feed byte for byte.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use icalendar::{Calendar, CalendarComponent, Component};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Answer {
    status: StatusCode,
    etag: Option<String>,
    body: String,
}

async fn send(app: &Router, method: &str, uri: &str, body: &str, etag: Option<&str>) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if uri.starts_with("/ngsi-ld") {
        req = req.header("NGSILD-Tenant", "town");
    }
    if let Some(tag) = etag {
        req = req.header("If-None-Match", tag);
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
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    Answer {
        status,
        etag,
        body: String::from_utf8(bytes.to_vec()).expect("utf-8"),
    }
}

fn prop(v: &Value) -> Value {
    json!({"type": "Property", "value": v})
}

fn event(id: &str, ty: &str, start: &str, category: &str) -> Value {
    json!({"id": format!("urn:ngsi-ld:{ty}:{id}"), "type": ty,
           "name": prop(&json!(format!("{ty} {id}"))), "startDate": prop(&json!(start)),
           "category": prop(&json!(category)), "dateModified": prop(&json!("2026-10-01T08:00:00Z")),
           "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.15, 48.74]}}})
}

/// The outputs of the town's calendar: everything, culture alone, council meetings, the
/// newest one, and one fixed to a circle around Banská Bystrica.
fn outputs() -> Value {
    json!({"ical": [
        {"name": "vsetky", "title": {"sk": "Všetky podujatia", "en": "All events"}, "lang": "sk",
         "description": {"sk": "Kalendár mesta"},
         "sources": [{"type": "Event", "preset": "sdm-event", "mapping": {"LAST-MODIFIED": "dateModified"}}]},
        {"name": "kultura", "title": {"sk": "Kultúra"},
         "filter": {"q": "category==\"kultura\""}, "sources": [{"type": "Event"}]},
        {"name": "zastupitelstvo", "sources": [{"type": "CouncilMeeting"}, {"type": "Event"}]},
        {"name": "najnovsie", "sort": "-start", "limit": 1, "sources": [{"type": "Event"}]},
        {"name": "chronologicky", "sort": "start", "sources": [{"type": "Event"}]},
        {"name": "prve", "limit": 2, "sources": [{"type": "Event"}]},
        {"name": "okolie", "filter": {"georel": "near;maxDistance==2000", "geometry": "Point",
            "coordinates": "[19.15,48.74]"}, "sources": [{"type": "Event"}]}
    ]})
}

async fn fixture() -> (TestBroker, Router) {
    let broker = TestBroker::new().await.expect("broker");
    let app = broker.serve(antares_ical::Ical).expect("mounted");
    let reads = json!(["query_entities"]);
    let rules = json!([{"types": ["Event", "CouncilMeeting"], "attributes": ["name", "startDate", "category", "dateModified", "location"]}]);
    for view in [
        json!({"id": "town", "tenant": "town", "selector": ["Event", "CouncilMeeting"],
               "operations": reads, "rules": rules, "representations": outputs()}),
        json!({"id": "plain", "tenant": "town", "selector": ["Event"], "operations": reads,
               "rules": rules}),
        json!({"id": "meetings", "tenant": "town", "selector": ["Event"], "operations": reads,
               "rules": rules, "representations": {"ical": [{"name": "rada", "sources": [{"type": "CouncilMeeting"}]}]}}),
        json!({"id": "empty", "tenant": "nobody", "selector": ["Event"], "operations": reads,
               "rules": rules, "representations": {"ical": [{"name": "nic", "sources": [{}]}]}}),
    ] {
        let id = view["id"].as_str().expect("id");
        let a = send(
            &app,
            "PUT",
            &format!("/q/views/{id}"),
            &view.to_string(),
            None,
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED, "view {id}: {}", a.body);
    }
    for e in [
        event("jarmok", "Event", "2026-09-10", "kultura"),
        event("beh", "Event", "2026-09-12T09:00:00", "sport"),
        event("koncert", "Event", "2026-09-11T19:30:00+02:00", "kultura"),
        event(
            "zasadnutie",
            "CouncilMeeting",
            "2026-09-15T15:00:00",
            "rada",
        ),
    ] {
        let a = send(&app, "POST", "/ngsi-ld/v1/entities", &e.to_string(), None).await;
        assert_eq!(a.status, StatusCode::CREATED, "{e}: {}", a.body);
    }
    (broker, app)
}

fn parse(text: &str) -> Calendar {
    text.parse::<Calendar>()
        .unwrap_or_else(|e| panic!("not RFC 5545: {e}\n{text}"))
}

fn uids(text: &str) -> Vec<String> {
    let mut all: Vec<String> = parse(text)
        .components
        .iter()
        .filter_map(|c| match c {
            CalendarComponent::Event(e) => e.get_uid().map(str::to_owned),
            _ => None,
        })
        .collect();
    all.sort();
    all
}

async fn get(app: &Router, path: &str) -> Answer {
    let a = send(app, "GET", path, "", None).await;
    assert_eq!(a.status, StatusCode::OK, "{path}: {}", a.body);
    a
}

#[tokio::test]
async fn a_view_without_outputs_has_no_calendar() {
    let (_broker, app) = fixture().await;
    for path in [
        "/x/views/plain/ical/v1/events.ics",
        "/x/views/town/ical/v1/sport.ics",
        "/x/views/town/ical/v1/vsetky",
        // the output maps a type the view does not show
        "/x/views/meetings/ical/v1/rada.ics",
    ] {
        let a = send(&app, "GET", path, "", None).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{path}: {}", a.body);
        assert!(a.body.contains("no such calendar"), "{}", a.body);
    }
}

#[tokio::test]
async fn each_output_serves_its_own_sources_under_its_filter_and_events_is_the_first() {
    let (_broker, app) = fixture().await;
    let all = get(&app, "/x/views/town/ical/v1/vsetky.ics").await.body;
    assert_eq!(
        uids(&all),
        [
            "urn:ngsi-ld:Event:beh",
            "urn:ngsi-ld:Event:jarmok",
            "urn:ngsi-ld:Event:koncert"
        ]
    );
    let alias = get(&app, "/x/views/town/ical/v1/events.ics").await.body;
    assert_eq!(alias, all, "events.ics is the first output");
    let culture = get(&app, "/x/views/town/ical/v1/kultura.ics").await.body;
    assert_eq!(
        uids(&culture),
        ["urn:ngsi-ld:Event:jarmok", "urn:ngsi-ld:Event:koncert"]
    );
    // a caller's q narrows the output's further and never widens it
    let sport = get(
        &app,
        "/x/views/town/ical/v1/kultura.ics?q=category==%22sport%22",
    )
    .await;
    assert!(uids(&sport.body).is_empty(), "{}", sport.body);
    // a q that closes its own group would OR past the filter: refused, never joined
    let escape = send(
        &app,
        "GET",
        "/x/views/town/ical/v1/kultura.ics?q=category==%22kultura%22%29%7C%28category==%22sport%22",
        "",
        None,
    )
    .await;
    assert_eq!(escape.status, StatusCode::BAD_REQUEST, "{}", escape.body);
    let council = get(&app, "/x/views/town/ical/v1/zastupitelstvo.ics")
        .await
        .body;
    assert_eq!(
        uids(&council).len(),
        4,
        "two types in one calendar: {council}"
    );
    assert!(council.contains("UID:urn:ngsi-ld:CouncilMeeting:zasadnutie"));
    // 15:00 in Bratislava (CEST) is 13:00Z: no floating time
    assert!(council.contains("DTSTART:20260915T130000Z"), "{council}");
    let ordered = get(&app, "/x/views/town/ical/v1/chronologicky.ics")
        .await
        .body;
    let at = |uid: &str| ordered.find(uid).unwrap_or(usize::MAX);
    assert!(at("Event:jarmok") < at("Event:koncert") && at("Event:koncert") < at("Event:beh"));
    let newest = get(&app, "/x/views/town/ical/v1/najnovsie.ics").await.body;
    assert_eq!(uids(&newest), ["urn:ngsi-ld:Event:beh"]);
    // a limit without sort keeps the earliest
    let first = get(&app, "/x/views/town/ical/v1/prve.ics").await.body;
    assert_eq!(
        uids(&first),
        ["urn:ngsi-ld:Event:jarmok", "urn:ngsi-ld:Event:koncert"]
    );
    let near = get(&app, "/x/views/town/ical/v1/okolie.ics").await.body;
    assert_eq!(uids(&near).len(), 3, "{near}");
    let a = send(
        &app,
        "GET",
        "/x/views/town/ical/v1/okolie.ics?georel=near;maxDistance==1&geometry=Point&coordinates=[0,0]",
        "",
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
    let a = send(
        &app,
        "GET",
        "/x/views/town/ical/v1/kultura.ics?type=CouncilMeeting",
        "",
        None,
    )
    .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.body);
}

#[tokio::test]
async fn the_calendar_names_itself_refreshes_and_shows_its_zone() {
    let (_broker, app) = fixture().await;
    let body = get(&app, "/x/views/town/ical/v1/vsetky.ics").await.body;
    for line in [
        "NAME;LANGUAGE=en:All events\r\n",
        "NAME;LANGUAGE=sk:Všetky podujatia\r\n",
        "DESCRIPTION;LANGUAGE=sk:Kalendár mesta\r\n",
        "X-WR-CALNAME:Všetky podujatia\r\n",
        "REFRESH-INTERVAL;VALUE=DURATION:PT6H\r\n",
        "X-WR-TIMEZONE:Europe/Bratislava\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Europe/Bratislava\r\n",
    ] {
        assert!(body.contains(line), "{line:?} in {body}");
    }
    // an output without a title is named by its view
    let body = get(&app, "/x/views/town/ical/v1/okolie.ics").await.body;
    assert!(body.contains("X-WR-CALNAME:town\r\n"), "{body}");
    assert!(!body.contains("NAME;"), "{body}");
}

#[tokio::test]
async fn an_empty_calendar_still_holds_a_component() {
    let (_broker, app) = fixture().await;
    let body = get(&app, "/x/views/empty/ical/v1/nic.ics").await.body;
    assert!(uids(&body).is_empty(), "{body}");
    // RFC 5545 3.6: a VCALENDAR holds one component at least, the VTIMEZONE here
    assert!(body.contains("BEGIN:VTIMEZONE"), "{body}");
    assert!(body.contains("BEGIN:STANDARD"), "{body}");
}

#[tokio::test]
async fn a_strong_etag_answers_304_and_an_unchanged_reingest_keeps_the_feed() {
    let (_broker, app) = fixture().await;
    let path = "/x/views/town/ical/v1/vsetky.ics";
    let first = get(&app, path).await;
    let tag = first.etag.clone().expect("ETag");
    assert!(tag.starts_with('"') && !tag.starts_with("W/"), "{tag}");
    let again = send(&app, "GET", path, "", Some(&tag)).await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert_eq!(again.body, "");
    let any = send(&app, "GET", path, "", Some("*")).await;
    assert_eq!(any.status, StatusCode::NOT_MODIFIED);
    let other = send(&app, "GET", path, "", Some("\"other\"")).await;
    assert_eq!(other.status, StatusCode::OK);
    // the nightly re-ingest, a second later: the same values written again
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let same = json!([
        event("jarmok", "Event", "2026-09-10", "kultura"),
        event("beh", "Event", "2026-09-12T09:00:00", "sport"),
        event("koncert", "Event", "2026-09-11T19:30:00+02:00", "kultura")
    ]);
    let a = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert",
        &same.to_string(),
        None,
    )
    .await;
    assert!(a.status.is_success(), "{}: {}", a.status, a.body);
    let after = get(&app, path).await;
    assert_eq!(after.body, first.body, "DTSTAMP from dateModified");
    assert_eq!(after.etag, first.etag);
    assert!(
        after.body.contains("DTSTAMP:20261001T080000Z"),
        "{}",
        after.body
    );
}
