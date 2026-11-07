// SPDX-License-Identifier: EUPL-1.2
//! `antares_ical::mapping`: one Entity as one `VEVENT` (docs/analysis/ical-mapping.md).

#![allow(clippy::expect_used, clippy::panic)]

use antares_ical::mapping::{escape, fold, reason, Outcome, Plan, When, Window};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
        .expect("time")
        .with_timezone(&Utc)
}

fn prop(v: Value) -> Value {
    Value::Object(Map::from_iter([
        ("type".to_owned(), json!("Property")),
        ("value".to_owned(), v),
    ]))
}

/// An Event starting `2026-10-20` named `Koncert`, with `extra` members set or replaced.
fn event(extra: &[(&str, Value)]) -> Value {
    let mut e = Map::new();
    e.insert("id".into(), json!("urn:ngsi-ld:Event:1"));
    e.insert("type".into(), json!("Event"));
    e.insert("name".into(), prop(json!("Koncert")));
    e.insert("startDate".into(), prop(json!("2026-10-20")));
    for (k, v) in extra {
        e.insert((*k).to_owned(), v.clone());
    }
    Value::Object(e)
}

/// The unfolded lines of an `Outcome::Event`, or the outcome's name.
fn lines(outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::Event(text) => text
            .replace("\r\n ", "")
            .split("\r\n")
            .map(str::to_owned)
            .collect(),
        other => vec![format!("{other:?}")],
    }
}

fn feed(extra: &[(&str, Value)]) -> Vec<String> {
    lines(&vevent(&event(extra), &Window::default(), now()))
}

fn vevent(entity: &Value, window: &Window, now: DateTime<Utc>) -> Outcome {
    Plan::default().vevent(entity, window, now)
}

fn has(l: &[String], line: &str) -> bool {
    l.iter().any(|x| x == line)
}

#[test]
fn a_line_of_multibyte_letters_folds_at_75_octets_between_characters() {
    let mut out = String::new();
    fold(&format!("SUMMARY:{}", "ž".repeat(60)), &mut out);
    for line in out.trim_end_matches("\r\n").split("\r\n") {
        assert!(line.len() <= 75, "{} octets: {line}", line.len());
    }
    assert_eq!(
        out.replace("\r\n ", ""),
        format!("SUMMARY:{}\r\n", "ž".repeat(60))
    );
}

#[test]
fn text_escapes_what_rfc_5545_reserves_keeps_a_tab_and_drops_control_characters() {
    assert_eq!(escape("a,b;c\\d\ne\u{7}\tf"), "a\\,b\\;c\\\\d\\ne\tf");
}

#[test]
fn a_multi_day_event_ends_the_day_after_its_last_day() {
    let l = feed(&[("endDate", prop(json!("2026-10-22")))]);
    assert!(has(&l, "DTSTART;VALUE=DATE:20261020"), "{l:?}");
    assert!(has(&l, "DTEND;VALUE=DATE:20261023"), "{l:?}");
}

#[test]
fn a_date_start_with_a_time_end_ends_the_day_after_that_time() {
    let l = feed(&[("endDate", prop(json!("2026-10-21T10:00:00Z")))]);
    assert!(has(&l, "DTEND;VALUE=DATE:20261022"), "{l:?}");
}

#[test]
fn an_offset_is_written_in_utc_and_an_end_before_the_start_skips_the_event() {
    let l = feed(&[
        (
            "startDate",
            prop(json!({"@type": "DateTime", "@value": "2026-10-20T18:00:00+02:00"})),
        ),
        ("endDate", prop(json!("2026-10-20T17:00:00+02:00"))),
    ]);
    assert_eq!(l, [format!("Skipped({:?})", reason::END_NOT_AFTER_START)]);
    let l = feed(&[(
        "startDate",
        prop(json!({"@type": "DateTime", "@value": "2026-10-20T18:00:00+02:00"})),
    )]);
    assert!(has(&l, "DTSTART:20261020T160000Z"), "{l:?}");
}

#[test]
fn an_instant_end_equal_to_its_start_is_an_event_without_dtend() {
    // bbsk-podujatia:0ff29da973d56cc4 on dev (T-6596): startDate and endDate one instant.
    let at = "2026-11-15T17:00:00+01:00";
    let l = feed(&[("startDate", prop(json!(at))), ("endDate", prop(json!(at)))]);
    assert!(has(&l, "DTSTART:20261115T160000Z"), "{l:?}");
    assert!(!l.iter().any(|x| x.starts_with("DTEND")), "{l:?}");
}

#[test]
fn a_utc_start_with_a_date_or_floating_end_ends_in_utc_read_in_bratislava() {
    let start = ("startDate", prop(json!("2026-10-20T18:00:00Z")));
    // the last day 21 October ends at its local midnight, 22:00Z in CEST
    let l = feed(&[start.clone(), ("endDate", prop(json!("2026-10-21")))]);
    assert!(has(&l, "DTEND:20261021T220000Z"), "{l:?}");
    let l = feed(&[start, ("endDate", prop(json!("2026-10-20T22:00:00")))]);
    assert!(has(&l, "DTEND:20261020T200000Z"), "{l:?}");
}

#[test]
fn a_time_without_offset_is_read_in_bratislava_and_never_left_floating() {
    // RFC 5545 3.3.5: a floating 18:00 would be 18:00 in New York too
    let start = ("startDate", prop(json!("2026-10-20T18:00:00")));
    let l = feed(&[
        start.clone(),
        ("endDate", prop(json!("2026-10-20T20:00:00Z"))),
    ]);
    assert!(has(&l, "DTSTART:20261020T160000Z"), "{l:?}");
    assert!(has(&l, "DTEND:20261020T200000Z"), "{l:?}");
    // the reverse mix: an instant start, a local end two hours later
    let l = feed(&[
        ("startDate", prop(json!("2026-10-20T16:00:00Z"))),
        ("endDate", prop(json!("2026-10-20T20:00:00"))),
    ]);
    assert!(has(&l, "DTEND:20261020T180000Z"), "{l:?}");
    // winter time: CET is +01:00
    let l = feed(&[("startDate", prop(json!("2026-12-01T18:00:00")))]);
    assert!(has(&l, "DTSTART:20261201T170000Z"), "{l:?}");
    let l = feed(&[start, ("endDate", prop(json!("2026-10-20")))]);
    assert!(has(&l, "DTEND:20261020T220000Z"), "{l:?}");
    assert!(
        !l.iter()
            .any(|x| x.starts_with("DT") && !x.ends_with('Z') && !x.contains("VALUE=DATE")),
        "{l:?}"
    );
}

#[test]
fn without_start_or_name_there_is_no_event() {
    let mut e = event(&[]);
    e.as_object_mut().expect("object").remove("name");
    assert_eq!(
        vevent(&e, &Window::default(), now()),
        Outcome::Skipped(reason::MISSING_SUMMARY)
    );
    assert_eq!(
        feed(&[("startDate", prop(json!("soon")))]),
        [format!("Skipped({:?})", reason::UNPARSABLE_DATE)]
    );
    let mut e = event(&[]);
    e.as_object_mut().expect("object").remove("startDate");
    assert_eq!(
        vevent(&e, &Window::default(), now()),
        Outcome::Skipped(reason::MISSING_DTSTART)
    );
    assert_eq!(
        feed(&[("name", prop(json!("  ")))]),
        [format!("Skipped({:?})", reason::MISSING_SUMMARY)]
    );
}

#[test]
fn the_window_keeps_what_overlaps_it_and_its_last_day_inclusive() {
    // the 21st in Bratislava (CEST): 2026-10-20T22:00Z to 2026-10-21T22:00Z
    let day = When::parse("2026-10-21");
    let w = Window::read(day, day, antares_ical::mapping::DEFAULT_ZONE);
    let multi = event(&[("endDate", prop(json!("2026-10-22")))]);
    assert!(matches!(vevent(&multi, &w, now()), Outcome::Event(_)));
    // one day, the 20th: over before the 21st
    assert_eq!(vevent(&event(&[]), &w, now()), Outcome::Outside);
    let late = event(&[("startDate", prop(json!("2026-10-21T21:30:00Z")))]);
    assert!(matches!(vevent(&late, &w, now()), Outcome::Event(_)));
    let later = event(&[("startDate", prop(json!("2026-10-21T22:00:00Z")))]);
    assert_eq!(vevent(&later, &w, now()), Outcome::Outside);
    // a bound without offset is read in the zone too
    let evening = Window::read(
        When::parse("2026-10-21T20:00:00"),
        None,
        antares_ical::mapping::DEFAULT_ZONE,
    );
    let at_19 = event(&[("startDate", prop(json!("2026-10-21T17:00:00Z")))]);
    assert_eq!(vevent(&at_19, &evening, now()), Outcome::Outside);
}

#[test]
fn location_geo_and_url_come_from_the_entity_and_a_bad_url_skips_it() {
    let l = feed(&[
        (
            "address",
            prop(json!({"streetAddress": "Námestie SNP 1", "addressLocality": "Banská Bystrica"})),
        ),
        (
            "location",
            json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.146, 48.736]}}),
        ),
    ]);
    assert!(
        has(&l, "LOCATION:Námestie SNP 1\\, Banská Bystrica"),
        "{l:?}"
    );
    assert!(has(&l, "GEO:48.736;19.146"), "{l:?}");
    let skipped = [format!("Skipped({:?})", reason::URL_NOT_HTTP)];
    assert_eq!(
        feed(&[("url", prop(json!("javascript:alert(1)")))]),
        skipped
    );
    let l = feed(&[("url", prop(json!("http://bb.sk/e/1")))]);
    assert!(has(&l, "URL:http://bb.sk/e/1"), "{l:?}");
    assert_eq!(feed(&[("url", prop(json!("https://bb.sk/a b")))]), skipped);
}

#[test]
fn an_address_as_text_is_the_location_and_an_empty_one_none() {
    let l = feed(&[("address", prop(json!("Kapitulská 23")))]);
    assert!(has(&l, "LOCATION:Kapitulská 23"), "{l:?}");
    let l = feed(&[("address", prop(json!({"streetAddress": " "})))]);
    assert!(!l.iter().any(|x| x.starts_with("LOCATION")), "{l:?}");
    let l = feed(&[("address", prop(json!(" ")))]);
    assert!(!l.iter().any(|x| x.starts_with("LOCATION")), "{l:?}");
}

#[test]
fn a_geometry_other_than_a_point_has_no_geo_and_one_off_the_globe_skips_it() {
    let line = json!({"type": "GeoProperty", "value": {"type": "LineString", "coordinates": [[19.1, 48.7], [19.2, 48.8]]}});
    let l = feed(&[("location", line)]);
    assert!(!l.iter().any(|x| x.starts_with("GEO")), "{l:?}");
    let off =
        json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.1, 98.7]}});
    assert_eq!(
        feed(&[("location", off)]),
        [format!("Skipped({:?})", reason::GEO_OUT_OF_RANGE)]
    );
}

#[test]
fn the_default_instance_and_the_first_language_are_read() {
    let l = feed(&[
        (
            "name",
            json!([
                {"type": "Property", "value": "iné", "datasetId": "urn:ngsi-ld:Dataset:x"},
                {"type": "Property", "value": "Hlavný názov"}
            ]),
        ),
        (
            "description",
            json!({"type": "LanguageProperty", "languageMap": {"en": "Concert", "sk": "Koncert"}}),
        ),
    ]);
    assert!(has(&l, "SUMMARY:Hlavný názov"), "{l:?}");
    assert!(has(&l, "DESCRIPTION:Concert"), "{l:?}");
    let only = feed(&[(
        "name",
        json!([{"type": "Property", "value": "jediný", "datasetId": "urn:ngsi-ld:Dataset:x"}]),
    )]);
    assert!(has(&only, "SUMMARY:jediný"), "{only:?}");
    assert_eq!(
        feed(&[("name", json!([]))]),
        [format!("Skipped({:?})", reason::MISSING_SUMMARY)]
    );
}

#[test]
fn an_sdm_iri_names_the_same_attribute_and_modified_at_stamps_the_event() {
    let l = feed(&[
        (
            "https://smartdatamodels.org/dataModel.Event/url",
            prop(json!("https://bb.sk/e/1")),
        ),
        ("modifiedAt", json!("2026-10-01T08:00:00Z")),
    ]);
    assert!(has(&l, "URL:https://bb.sk/e/1"), "{l:?}");
    assert!(has(&l, "DTSTAMP:20261001T080000Z"), "{l:?}");
    assert!(has(&l, "LAST-MODIFIED:20261001T080000Z"), "{l:?}");
    // without modifiedAt the request time stamps it and nothing is last-modified
    let l = feed(&[]);
    assert!(has(&l, "DTSTAMP:20261018T000000Z"), "{l:?}");
    assert!(!l.iter().any(|x| x.starts_with("LAST-MODIFIED")), "{l:?}");
}

/// T-5207: an output of this contract version is read strictly (an unknown member or an
/// unknown version is refused, as the Portal's save check refuses it); one of a newer version
/// keeps its name, texts, filter and settings and serves each source's type over its preset,
/// so the calendar still answers.
#[test]
fn the_output_version_reads_strictly_and_a_newer_one_degrades_to_the_preset() {
    use antares_ical::mapping::{output_of, CompiledOutput, MAPPING_VERSION};
    assert_eq!(MAPPING_VERSION, 1);
    let v1 = json!({"name": "vsetky", "version": 1, "sources": [{"preset": "schema-org"}]});
    let (output, degraded) = output_of(&v1).expect("v1");
    assert!(!degraded);
    assert!(CompiledOutput::compile(&output).is_ok());
    let (absent, _) =
        output_of(&json!({"name": "vsetky", "sources": [{}]})).expect("version absent reads as 1");
    assert_eq!(
        absent.sources.first().map(|s| s.entity_type.as_str()),
        Some("Event")
    );
    for refused in [
        json!({"name": "a", "version": 1, "rrule": "eventSchedule", "sources": [{}]}),
        json!({"name": "a", "sources": [{"rrule": "eventSchedule"}]}),
        json!({"name": "a", "version": 3, "filter": {"near": "x"}, "sources": [{}]}),
    ] {
        assert!(output_of(&refused).is_err(), "{refused}");
    }
    let (old, _) = output_of(&json!({"name": "a", "version": 0, "sources": [{}]})).expect("reads");
    assert!(
        CompiledOutput::compile(&old).is_err(),
        "version 0 is refused"
    );
    let (source, _) = output_of(&json!({"name": "a", "sources": [{"version": 0}]})).expect("reads");
    assert!(
        CompiledOutput::compile(&source).is_err(),
        "a source of version 0 is refused"
    );
    let newer = json!({"name": "koncerty", "version": 2, "rrule": "x",
        "title": {"sk": "Koncerty"}, "lang": "sk", "limit": 50, "sort": "sideways",
        "timezone": "Mars/Olympus", "filter": {"q": "eventCategory==\"hudba\""},
        "sources": [{"type": "CouncilMeeting", "preset": "schema-org", "rrule": "eventSchedule",
            "mapping": {"SUMMARY": {"attribute": "name", "case": "upper"}}}, {"type": "x y"}]});
    let (output, degraded) = output_of(&newer).expect("a newer output degrades");
    assert!(degraded);
    assert_eq!(output.name, "koncerty");
    assert_eq!(output.title.get("sk").map(String::as_str), Some("Koncerty"));
    assert_eq!(output.filter.q.as_deref(), Some("eventCategory==\"hudba\""));
    assert_eq!(
        (output.limit, output.sort.clone(), output.timezone.clone()),
        (Some(50), None, None)
    );
    assert_eq!(
        output.sources.get(1).map(|s| s.entity_type.as_str()),
        Some("Event")
    );
    let compiled = CompiledOutput::compile(&output).expect("compiles");
    let plan = compiled.plans.first().expect("a plan");
    assert_eq!(plan.entity_type(), "CouncilMeeting");
    let concert = event(&[("type", json!("CouncilMeeting"))]);
    assert!(lines(&plan.vevent(&concert, &Window::default(), now()))
        .iter()
        .any(|l| l == "SUMMARY:Koncert"));
    let (bare, _) = output_of(&json!({"version": 2})).expect("degrades");
    assert!(bare.sources.is_empty() && bare.name.is_empty());
}
