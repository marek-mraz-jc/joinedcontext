// SPDX-License-Identifier: EUPL-1.2
//! `antares-ical` against a broker (docs/analysis/ical-mapping.md): the feed parses as RFC 5545
//! with the `icalendar` crate and round-trips through it, the window narrows it, and a type or
//! an Attribute the view hides is in no `VEVENT`.
//!
//! The Tenant `city` holds five Events and a `Meeting`. The view `city` shows `Event` without
//! its `organizer` and hides `Meeting`; `places` shows `Place` alone; `every` shows every type;
//! `void` reads the empty Tenant `void`; `writes` allows Create Entity alone.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use icalendar::{Calendar, CalendarComponent, Component, DatePerhapsTime, EventLike};
use serde_json::{json, Value};
use std::fmt::Write as _;
use tower::ServiceExt;

struct Answer {
    status: StatusCode,
    content_type: String,
    skipped: Option<String>,
    body: String,
}

async fn send(app: &Router, method: &str, uri: &str, body: &str) -> Answer {
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
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let (status, content_type, skipped) = (
        resp.status(),
        header("content-type").unwrap_or_default(),
        header(antares_ical::SKIPPED_HEADER),
    );
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    Answer {
        status,
        content_type,
        skipped,
        body: String::from_utf8(bytes.to_vec()).expect("utf-8"),
    }
}

fn prop(v: &str) -> Value {
    json!({"type": "Property", "value": v})
}

fn entities() -> Vec<Value> {
    vec![
        json!({"id": "urn:ngsi-ld:Event:jarmok", "type": "Event", "name": prop("Radvanský jarmok, 2026"),
               "startDate": prop("2026-09-10"), "endDate": prop("2026-09-12"),
               "address": {"type": "Property", "value": {"streetAddress": "Námestie SNP", "addressLocality": "Banská Bystrica"}},
               "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.1462, 48.7363]}},
               "url": prop("https://www.banskabystrica.sk/jarmok"),
               "organizer": prop("hidden organizer")}),
        json!({"id": "urn:ngsi-ld:Event:koncert", "type": "Event", "name": prop("Koncert"),
               "startDate": prop("2026-09-11T19:30:00+02:00"),
               "description": prop("Hrá orchester; vstup voľný\nDruhý riadok")}),
        json!({"id": "urn:ngsi-ld:Event:nostart", "type": "Event", "name": prop("Bez dátumu")}),
        json!({"id": "urn:ngsi-ld:Event:noname", "type": "Event", "startDate": prop("2026-09-11")}),
        json!({"id": "urn:ngsi-ld:Event:silvester", "type": "Event", "name": prop("Silvester"),
               "startDate": prop("2026-12-31T22:00:00Z"), "endDate": prop("2027-01-01T02:00:00Z")}),
        json!({"id": "urn:ngsi-ld:Meeting:1", "type": "Meeting", "name": prop("hidden meeting"),
               "startDate": prop("2026-09-11")}),
    ]
}

async fn fixture() -> (TestBroker, Router) {
    let broker = TestBroker::new().await.expect("broker");
    let app = broker.serve(antares_ical::Ical).expect("mounted");
    let reads = json!(["query_entities"]);
    let ical = json!({"ical": [{"name": "events", "sources": [{}]}]});
    let views = [
        json!({"id": "city", "tenant": "city", "selector": ["Event"], "operations": reads, "representations": ical,
               "rules": [{"types": ["Event"], "attributes": ["name", "startDate", "endDate", "description", "address", "location", "url"]}]}),
        json!({"id": "places", "tenant": "city", "selector": ["Place"], "operations": reads,
               "rules": [{"types": ["Place"], "attributes": ["name"]}]}),
        json!({"id": "every", "tenant": "city", "selector": ["*"], "operations": reads, "representations": ical,
               "rules": [{"types": ["*"], "attributes": ["*"]}]}),
        json!({"id": "void", "tenant": "void", "selector": ["Event"], "operations": reads, "representations": ical,
               "rules": [{"types": ["Event"], "attributes": ["name", "startDate"]}]}),
        json!({"id": "writes", "tenant": "city", "selector": ["Event"], "operations": ["create_entity"],
               "rules": [{"types": ["Event"], "attributes": ["name", "startDate"]}]}),
    ];
    for view in views {
        let id = view["id"].as_str().expect("id");
        let a = send(&app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        assert_eq!(a.status, StatusCode::CREATED, "view {id}: {}", a.body);
    }
    for entity in entities() {
        let a = send(&app, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
        assert_eq!(a.status, StatusCode::CREATED, "{entity}: {}", a.body);
    }
    (broker, app)
}

fn parse(text: &str) -> Calendar {
    text.parse::<Calendar>()
        .unwrap_or_else(|e| panic!("not RFC 5545: {e}\n{text}"))
}

/// The feed's `VEVENT`s by `UID`.
fn events(cal: &Calendar) -> Vec<icalendar::Event> {
    let mut all: Vec<icalendar::Event> = cal
        .components
        .iter()
        .filter_map(|c| match c {
            CalendarComponent::Event(e) => Some(e.clone()),
            _ => None,
        })
        .collect();
    all.sort_by(|a, b| a.get_uid().cmp(&b.get_uid()));
    all
}

fn uids(cal: &Calendar) -> Vec<String> {
    events(cal)
        .iter()
        .filter_map(|e| e.get_uid().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn the_feed_is_rfc_5545_and_round_trips_through_a_parser() {
    let (_broker, app) = fixture().await;
    let a = send(&app, "GET", "/x/views/city/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.content_type, "text/calendar; charset=utf-8");
    for line in a.body.split("\r\n") {
        assert!(line.len() <= 75, "a content line over 75 octets: {line}");
    }
    assert!(a.body.ends_with("END:VCALENDAR\r\n"));

    let cal = parse(&a.body);
    assert_eq!(
        uids(&cal),
        [
            "urn:ngsi-ld:Event:jarmok",
            "urn:ngsi-ld:Event:koncert",
            "urn:ngsi-ld:Event:silvester"
        ]
    );
    // the parser's own rendering parses to the same events
    let again = parse(&cal.to_string());
    assert_eq!(events(&again), events(&cal));

    let all = events(&cal);
    let (jarmok, koncert) = (&all[0], &all[1]);
    // a non-ASCII title with a comma, unescaped by the parser
    assert_eq!(jarmok.get_summary(), Some("Radvanský jarmok, 2026"));
    // a multi-day event: its last day inclusive in the SDM, the day after it in DTEND
    assert_eq!(
        jarmok.get_start(),
        Some(DatePerhapsTime::Date(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 10).expect("date")
        ))
    );
    assert_eq!(
        jarmok.get_end(),
        Some(DatePerhapsTime::Date(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 13).expect("date")
        ))
    );
    assert_eq!(jarmok.get_location(), Some("Námestie SNP, Banská Bystrica"));
    assert_eq!(
        jarmok.get_url(),
        Some("https://www.banskabystrica.sk/jarmok")
    );
    assert!(a.body.contains("GEO:48.7363;19.1462\r\n"), "{}", a.body);
    // an event without endDate has no DTEND; its offset is written in UTC
    assert_eq!(koncert.get_end(), None);
    assert!(
        a.body.contains("DTSTART:20260911T173000Z\r\n"),
        "{}",
        a.body
    );
    assert_eq!(
        koncert.get_description(),
        Some("Hrá orchester; vstup voľný\nDruhý riadok")
    );
    // every VEVENT is stamped from modifiedAt
    for e in &all {
        assert!(e.get_timestamp().is_some(), "{e:?}");
        assert!(e.get_last_modified().is_some(), "{e:?}");
    }
}

#[tokio::test]
async fn an_event_without_start_or_name_is_left_out_and_counted() {
    let (_broker, app) = fixture().await;
    let a = send(&app, "GET", "/x/views/city/ical/v1/events.ics", "").await;
    assert_eq!(a.skipped.as_deref(), Some("2"));
    assert!(!a.body.contains("nostart") && !a.body.contains("noname"));
}

#[tokio::test]
async fn a_hidden_type_and_a_hidden_attribute_are_in_no_event() {
    let (_broker, app) = fixture().await;
    let a = send(&app, "GET", "/x/views/city/ical/v1/events.ics", "").await;
    assert!(!a.body.contains("hidden"), "{}", a.body);
    assert!(!a.body.contains("Meeting"), "{}", a.body);
}

#[tokio::test]
async fn the_window_keeps_what_overlaps_it_with_to_inclusive() {
    let (_broker, app) = fixture().await;
    let get = |q: &'static str| {
        let app = app.clone();
        async move {
            let a = send(
                &app,
                "GET",
                &format!("/x/views/city/ical/v1/events.ics?{q}"),
                "",
            )
            .await;
            assert_eq!(a.status, StatusCode::OK, "{q}: {}", a.body);
            uids(&parse(&a.body))
        }
    };
    // the fair's last day is the 12th, the concert the evening of the 11th
    assert_eq!(
        get("from=2026-09-12").await,
        ["urn:ngsi-ld:Event:jarmok", "urn:ngsi-ld:Event:silvester"]
    );
    assert_eq!(
        get("from=2026-09-11&to=2026-09-11").await,
        ["urn:ngsi-ld:Event:jarmok", "urn:ngsi-ld:Event:koncert"]
    );
    // a night across the new year is in a window of either year
    assert_eq!(
        get("from=2027-01-01T01:00:00Z").await,
        ["urn:ngsi-ld:Event:silvester"]
    );
    assert_eq!(get("to=2026-09-09").await, Vec::<String>::new());
}

#[tokio::test]
async fn a_bad_window_a_view_without_events_and_a_write_are_refused() {
    let (_broker, app) = fixture().await;
    let feed = "/x/views/city/ical/v1/events.ics";
    for q in ["from=yesterday", "from=2026-09-12&to=2026-09-11"] {
        let a = send(&app, "GET", &format!("{feed}?{q}"), "").await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{q}");
        assert_eq!(a.content_type, "application/problem+json");
    }
    for view in ["places", "nowhere"] {
        let a = send(
            &app,
            "GET",
            &format!("/x/views/{view}/ical/v1/events.ics"),
            "",
        )
        .await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{view}");
        assert!(a.body.contains("no such calendar"), "{}", a.body);
    }
    let a = send(&app, "POST", feed, "BEGIN:VCALENDAR").await;
    assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED);
    let a = send(&app, "HEAD", feed, "").await;
    assert_eq!((a.status, a.body.as_str()), (StatusCode::OK, ""));
}

#[tokio::test]
async fn a_view_of_every_type_serves_its_events_alone() {
    let (_broker, app) = fixture().await;
    let a = send(&app, "GET", "/x/views/every/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        uids(&parse(&a.body)),
        [
            "urn:ngsi-ld:Event:jarmok",
            "urn:ngsi-ld:Event:koncert",
            "urn:ngsi-ld:Event:silvester"
        ]
    );
    assert!(!a.body.contains("Meeting"), "{}", a.body);
}

#[tokio::test]
async fn an_empty_tenant_is_an_empty_calendar_and_a_view_without_query_entities_none() {
    let (_broker, app) = fixture().await;
    let a = send(&app, "GET", "/x/views/void/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(events(&parse(&a.body)).is_empty(), "{}", a.body);
    let a = send(&app, "GET", "/x/views/writes/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.body);
    assert!(a.body.contains("no such calendar"), "{}", a.body);
    let a = send(&app, "OPTIONS", "/x/views/city/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_feed_reads_every_page_and_refuses_past_its_bound() {
    use antares_ops::Surface as _;
    assert_eq!(
        antares_ical::Ical.version_info()["endpoint"],
        "/x/views/{view}/ical/v1/events.ics"
    );
    let (_broker, app) = fixture().await;
    let add = |from: usize, to: usize| {
        let app = app.clone();
        async move {
            let batch: Vec<Value> = (from..to)
                .map(|i| {
                    json!({"id": format!("urn:ngsi-ld:Event:bulk-{i:05}"), "type": "Event",
                                "name": prop("Trh"), "startDate": prop("2026-10-20")})
                })
                .collect();
            let body = Value::Array(batch).to_string();
            let a = send(&app, "POST", "/ngsi-ld/v1/entityOperations/create", &body).await;
            assert!(a.status.is_success(), "{}", a.body);
        }
    };
    // past one upstream page of 100
    add(0, 250).await;
    let a = send(&app, "GET", "/x/views/city/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(events(&parse(&a.body)).len(), 253);
    let mut at = 250;
    while at < antares_ops::RESIDUAL_SCAN {
        let next = (at + 1000).min(antares_ops::RESIDUAL_SCAN);
        add(at, next).await;
        at = next;
    }
    let a = send(&app, "GET", "/x/views/city/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", a.body);
}

/// `DTSTAMP` and `LAST-MODIFIED` hold the time the fixture wrote the Entities, and the
/// `VTIMEZONE` spans the years around today: both out, the zone has its own tests.
fn stamped(text: &str) -> String {
    let mut inside = false;
    text.split_inclusive('\n')
        .filter_map(|line| {
            let was = inside;
            inside = (inside || line.starts_with("BEGIN:VTIMEZONE"))
                && !line.starts_with("END:VTIMEZONE");
            if was || inside {
                return line
                    .starts_with("BEGIN:VTIMEZONE")
                    .then(|| "VTIMEZONE\r\n".to_owned());
            }
            Some(match line.split_once(':') {
                Some((name @ ("DTSTAMP" | "LAST-MODIFIED"), rest)) => {
                    format!("{name}:STAMP{}", &rest[16..])
                }
                _ => line.to_owned(),
            })
        })
        .collect()
}

#[tokio::test]
async fn the_preset_output_serves_the_events_of_t_4487_under_the_calendar_properties() {
    // fixtures/sdm-event-default.txt: the VEVENTs as dev 53ab566e (T-4487) served them, under
    // the T-5208 calendar properties and VTIMEZONE
    let (_broker, app) = fixture().await;
    let mut all = String::new();
    for uri in [
        "/x/views/city/ical/v1/events.ics",
        "/x/views/every/ical/v1/events.ics",
        "/x/views/city/ical/v1/events.ics?from=2026-09-11&to=2026-09-11",
    ] {
        let a = send(&app, "GET", uri, "").await;
        writeln!(all, "# {uri} {} {:?}", a.status, a.skipped).expect("a String takes it");
        all.push_str(&a.body);
    }
    assert_eq!(
        stamped(&all),
        include_str!("fixtures/sdm-event-default.txt")
    );
}

/// F-ANT5-011: a calendar over a view of every type pushes the plans'
/// types, so a Tenant holding more Entities of another type than one read
/// may scan still serves its Events.
#[tokio::test]
async fn a_view_of_every_type_serves_a_large_tenant() {
    let (_broker, app) = fixture().await;
    let n = antares_ops::RESIDUAL_SCAN.saturating_add(1);
    let noise: Vec<serde_json::Value> = (0..n)
        .map(|i| json!({"id": format!("urn:ngsi-ld:Noise:n{i:06}"), "type": "Noise"}))
        .collect();
    for chunk in noise.chunks(1000) {
        let body = serde_json::Value::Array(chunk.to_vec()).to_string();
        let a = send(&app, "POST", "/ngsi-ld/v1/entityOperations/create", &body).await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    }
    let a = send(&app, "GET", "/x/views/every/ical/v1/events.ics", "").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(uids(&parse(&a.body)).len(), 3, "{}", a.body);
}
