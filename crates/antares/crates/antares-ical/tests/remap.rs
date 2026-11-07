// SPDX-License-Identifier: EUPL-1.2
//! `antares_ical::mapping::Plan`: a view's field mapping, its presets and its remaps
//! (platform API/02 §8b, T-4901), over the shared cases of `fixtures/cases.json`.

#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use antares_ical::mapping::{
    is_language_tag, is_output_name, reason, CompiledOutput, IcalMapping, Outcome, Output, Plan,
    Window,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
        .expect("time")
        .with_timezone(&Utc)
}

fn plan(ical: &Value) -> Result<Plan, String> {
    serde_json::from_value::<IcalMapping>(ical.clone())
        .map_err(|e| e.to_string())
        .and_then(|m| Plan::compile(&m))
}

/// The unfolded content lines between `BEGIN:VEVENT` and `END:VEVENT`.
fn lines(outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::Event(text) => {
            let all: Vec<String> = text
                .replace("\r\n ", "")
                .split("\r\n")
                .map(str::to_owned)
                .collect();
            assert_eq!(all.first().map(String::as_str), Some("BEGIN:VEVENT"));
            assert_eq!(all.last().map(String::as_str), Some(""), "ends in CRLF");
            all.get(1..all.len().saturating_sub(2))
                .unwrap_or_default()
                .to_vec()
        }
        other => vec![format!("{other:?}")],
    }
}

fn render(ical: &Value, entity: &Value) -> Vec<String> {
    let plan = plan(ical).expect("a mapping that holds");
    lines(&plan.vevent(entity, &Window::default(), now()))
}

#[test]
fn every_shared_case_renders_skips_or_is_refused_as_written() {
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/cases.json")).expect("cases.json");
    let entity = &cases["entity"];
    let all = cases["cases"].as_object().expect("cases");
    assert!(all.len() >= 18, "{}", all.len());
    for (name, case) in all {
        let ical = &case["ical"];
        if let Some(refused) = case["refused"].as_str() {
            let err = plan(ical)
                .err()
                .unwrap_or_else(|| panic!("{name}: accepted"));
            assert!(err.contains(refused), "{name}: {err}");
        } else if let Some(skip) = case["skip"].as_str() {
            assert_eq!(
                render(ical, entity),
                [format!("Skipped({skip:?})")],
                "{name}"
            );
        } else {
            let want: Vec<String> = serde_json::from_value(case["lines"].clone()).expect("lines");
            assert_eq!(render(ical, entity), want, "{name}");
        }
    }
}

fn entity(members: &Value) -> Value {
    let mut e = json!({"id": "urn:ngsi-ld:Event:2", "type": "Event",
                       "modifiedAt": "2026-10-01T08:00:00Z",
                       "startDate": {"type": "Property", "value": "2026-10-20"},
                       "name": {"type": "Property", "value": "Trh"}});
    for (k, v) in members.as_object().expect("members") {
        e[k] = v.clone();
    }
    e
}

fn has(lines: &[String], line: &str) -> bool {
    lines.iter().any(|l| l == line)
}

#[test]
fn a_language_a_sub_attribute_and_a_constant_category_are_read() {
    let ical = json!({"preset": "sdm-event", "mapping": {
        "SUMMARY": {"attribute": "title", "language": "en"},
        "DESCRIPTION": "name.note",
        "CATEGORIES": {"constant": "Kultúra"}}});
    let e = entity(&json!({
        "title": {"type": "LanguageProperty", "languageMap": {"sk": "Trh", "en": "Market"}},
        "name": {"type": "Property", "value": "Trh", "note": {"type": "Property", "value": "pod holým nebom"}}}));
    let l = render(&ical, &e);
    assert!(has(&l, "SUMMARY:Market"), "{l:?}");
    assert!(has(&l, "DESCRIPTION:pod holým nebom"), "{l:?}");
    assert!(has(&l, "CATEGORIES:Kultúra"), "{l:?}");
    // a language the entity lacks falls back to @none, then to the first tag
    let e = entity(&json!({"title": {"type": "LanguageProperty", "languageMap": {"sk": "Trh"}}}));
    assert!(has(&render(&ical, &e), "SUMMARY:Trh"));
    let e = entity(&json!({"title": {"type": "LanguageProperty",
        "languageMap": {"de": "Markt", "@none": "Jarmok"}}}));
    assert!(has(&render(&ical, &e), "SUMMARY:Jarmok"));
    // a regional tag reads its language and back; BCP 47 tags compile
    let e = entity(&json!({"title": {"type": "LanguageProperty",
        "languageMap": {"de": "Markt", "en-GB": "Fair", "sk": "Trh"}}}));
    assert!(has(&render(&ical, &e), "SUMMARY:Fair"));
    let regional = json!({"preset": "sdm-event", "mapping": {
        "SUMMARY": {"attribute": "title", "language": "sk-SK"}}});
    assert!(has(&render(&regional, &e), "SUMMARY:Trh"));
    assert!(plan(&json!({"preset": "sdm-event", "mapping": {
        "SUMMARY": {"attribute": "title", "language": "zh-Hant"}}}))
    .is_ok());
    assert!(plan(&json!({"preset": "sdm-event", "mapping": {
        "SUMMARY": {"attribute": "title", "language": "s k"}}}))
    .is_err());
}

#[test]
fn a_date_end_is_exclusive_unless_written_inclusive() {
    let e = entity(&json!({"endDate": {"type": "Property", "value": "2026-10-22"}}));
    let exclusive = json!({"preset": "sdm-event", "mapping": {"DTEND": "endDate"}});
    assert!(has(&render(&exclusive, &e), "DTEND;VALUE=DATE:20261022"));
    let inclusive = json!({"preset": "sdm-event", "mapping": {"DTEND": {"attribute": "endDate", "endInclusive": true}}});
    assert!(has(&render(&inclusive, &e), "DTEND;VALUE=DATE:20261023"));
    // an exclusive end on the start day ends nothing
    let same = entity(&json!({"endDate": {"type": "Property", "value": "2026-10-20"}}));
    assert_eq!(
        render(&exclusive, &same),
        [format!("Skipped({:?})", reason::END_NOT_AFTER_START)]
    );
}

#[test]
fn a_duration_must_be_an_rfc_5545_duration() {
    let ical =
        json!({"mapping": {"DTSTART": "startDate", "SUMMARY": "name", "DURATION": "length"}});
    for good in ["P1D", "PT1H30M", "P2W", "-PT15M", "P1DT12H"] {
        let e = entity(&json!({"length": {"type": "Property", "value": good}}));
        assert!(
            has(&render(&ical, &e), &format!("DURATION:{good}")),
            "{good}"
        );
    }
    for bad in ["1 hour", "P", "PT", "P1H", "PT1M1H", "P1W2D", "PDT1H", "P5"] {
        let e = entity(&json!({"length": {"type": "Property", "value": bad}}));
        assert_eq!(
            render(&ical, &e),
            [format!("Skipped({:?})", reason::UNPARSABLE_DURATION)],
            "{bad}"
        );
    }
}

#[test]
fn status_organizer_and_last_modified_follow_their_value_types() {
    let ical = json!({"preset": "sdm-event", "mapping": {
        "STATUS": "state", "ORGANIZER": "contact", "LAST-MODIFIED": "changed"}});
    let e = entity(&json!({
        "state": {"type": "Property", "value": "confirmed"},
        "contact": {"type": "Property", "value": "kultura@zvolen.sk"},
        "changed": {"type": "Property", "value": "2026-09-30T10:00:00+02:00"}}));
    let l = render(&ical, &e);
    assert!(has(&l, "STATUS:CONFIRMED"), "{l:?}");
    assert!(has(&l, "ORGANIZER:mailto:kultura@zvolen.sk"), "{l:?}");
    assert!(has(&l, "LAST-MODIFIED:20260930T080000Z"), "{l:?}");
    // a value that is no calendar status or address is left out; DTSTAMP stays modifiedAt
    let e = entity(&json!({
        "state": {"type": "Property", "value": "postponed"},
        "contact": {"type": "Property", "value": "Mestský úrad"}}));
    let l = render(&ical, &e);
    assert!(
        !l.iter()
            .any(|x| x.starts_with("STATUS") || x.starts_with("ORGANIZER")),
        "{l:?}"
    );
    assert!(!l.iter().any(|x| x.starts_with("LAST-MODIFIED")), "{l:?}");
    assert!(has(&l, "DTSTAMP:20261001T080000Z"), "{l:?}");
}

#[test]
fn a_custom_type_is_the_plan_type_and_a_bad_type_name_is_refused() {
    // owner 2026-10-22T09:25Z: a preset only over a type that has one
    for ty in ["Event", "CouncilMeeting", "OfficeHours"] {
        assert!(plan(&json!({"type": ty})).is_ok(), "{ty}");
    }
    let err = plan(&json!({"type": "Festival"})).expect_err("no preset");
    assert!(err.contains("no calendar preset"), "{err}");
    let err =
        plan(&json!({"type": "AirQualityObserved", "preset": "sdm-event"})).expect_err("no preset");
    assert!(err.contains("map its DTSTART and SUMMARY"), "{err}");
    let p =
        plan(&json!({"type": "Festival", "mapping": {"DTSTART": "startDate", "SUMMARY": "name"}}))
            .expect("its own time and title");
    assert_eq!(p.entity_type(), "Festival");
    assert_eq!(Plan::default().entity_type(), "Event");
    let err = plan(&json!({"type": "a/b"})).expect_err("refused");
    assert!(err.contains("not an NGSI-LD type name"), "{err}");
    assert!(plan(&json!({"preset": "outlook"})).is_err());
    assert!(plan(&json!({"extra": 1})).is_err(), "unknown member");
}

fn skipped(why: &str) -> Vec<String> {
    vec![format!("Skipped({why:?})")]
}

#[test]
fn a_date_mode_and_a_zone_read_a_value_as_asked_and_a_skipped_wall_time_is_unreadable() {
    let start = |remap: Value, value: &str| {
        let ical = json!({"preset": "sdm-event", "mapping": {"DTSTART": remap}});
        let e = entity(&json!({"startDate": {"type": "Property", "value": value}}));
        render(&ical, &e)
    };
    let zone = "Europe/Bratislava";
    let day = json!({"attribute": "startDate", "date": "date"});
    let day_in_zone = json!({"attribute": "startDate", "date": "date", "timezone": zone});
    let datetime = json!({"attribute": "startDate", "date": "datetime"});
    let datetime_in_zone = json!({"attribute": "startDate", "date": "datetime", "timezone": zone});
    for (remap, value, line) in [
        // the output's zone, Europe/Bratislava unless it names another
        (&day, "2026-10-20T23:30:00Z", "DTSTART;VALUE=DATE:20261021"),
        (
            &day_in_zone,
            "2026-10-20T23:30:00Z",
            "DTSTART;VALUE=DATE:20261021",
        ),
        (&day, "2026-10-20T18:00:00", "DTSTART;VALUE=DATE:20261020"),
        (&datetime, "2026-10-20", "DTSTART:20261019T220000Z"),
        (&datetime_in_zone, "2026-10-20", "DTSTART:20261019T220000Z"),
    ] {
        let l = start(remap.clone(), value);
        assert!(has(&l, line), "{remap} {value}: {l:?}");
    }
    // 02:30 on the last Sunday of March does not happen in Bratislava
    let l = start(
        json!({"attribute": "startDate", "timezone": zone}),
        "2026-03-29T02:30:00",
    );
    assert_eq!(l, skipped(reason::UNPARSABLE_DATE));
    let ical = json!({"preset": "sdm-event", "mapping": {"LAST-MODIFIED": "changed"}});
    let e = entity(&json!({"changed": {"type": "Property", "value": "yesterday"}}));
    assert_eq!(render(&ical, &e), skipped(reason::UNPARSABLE_DATE));
}

#[test]
fn values_of_every_kind_read_as_text_or_as_nothing() {
    let ical = json!({"mapping": {"DTSTART": "startDate", "SUMMARY": "count",
        "DESCRIPTION": {"attribute": "title.alt", "language": "en"},
        "LOCATION": "name.missing", "URL": "flag",
        "STATUS": {"attribute": "state", "map": {"done": "CONFIRMED"}},
        "ORGANIZER": "by", "GEO": "location"}});
    let e = entity(&json!({
        "count": {"type": "Property", "value": 42},
        "title": {"type": "Property", "value": "x", "alt": {"type": "LanguageProperty", "languageMap": {"sk": "iba po slovensky"}}},
        "flag": {"type": "Property", "value": [true]},
        "state": {"type": "VocabProperty", "vocab": "open"},
        "by": {"type": "Property", "value": "mailto:kultura@zvolen.sk"},
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": ["19.1", "48.7"]}}}));
    let l = render(&ical, &e);
    assert_eq!(
        l,
        [
            "UID:urn:ngsi-ld:Event:2",
            "DTSTAMP:20261001T080000Z",
            "DTSTART;VALUE=DATE:20261020",
            "SUMMARY:42",
            // no en text: the first tag's
            "DESCRIPTION:iba po slovensky",
            "ORGANIZER:mailto:kultura@zvolen.sk",
            "LAST-MODIFIED:20261001T080000Z"
        ]
    );
    let e = entity(&json!({"count": {"type": "Property", "value": false},
        "state": {"type": "VocabProperty", "vocab": "done"}, "by": {"type": "Property", "value": "MsÚ"}}));
    let l = render(&ical, &e);
    assert!(
        has(&l, "SUMMARY:false") && has(&l, "STATUS:CONFIRMED"),
        "{l:?}"
    );
    assert!(!l.iter().any(|x| x.starts_with("ORGANIZER")), "{l:?}");
    let count = json!({"type": "Property", "value": 1});
    let e = entity(&json!({"by": {"type": "Property", "value": "a@b:c"}, "count": count}));
    let l = render(&ical, &e);
    assert!(!l.iter().any(|x| x.starts_with("ORGANIZER")), "{l:?}");
    let off =
        json!({"type": "GeoProperty", "value": {"type": "Point", "coordinates": [190.0, 48.7]}});
    let e = entity(&json!({"location": off, "count": count}));
    assert_eq!(render(&ical, &e), skipped(reason::GEO_OUT_OF_RANGE));
    // a mapped STATUS the entity lacks, and an entity that is no object
    assert!(
        !render(&json!({"preset": "schema-org"}), &entity(&json!({})))
            .iter()
            .any(|x| x.starts_with("STATUS"))
    );
    assert_eq!(
        lines(&Plan::default().vevent(&json!("x"), &Window::default(), now())),
        skipped(reason::MISSING_DTSTART)
    );
}

#[test]
fn lists_relationships_and_an_organizer_object_are_read() {
    let ical = json!({"preset": "sdm-event", "mapping": {
        "CATEGORIES": "tags", "ORGANIZER": "organizer", "DESCRIPTION": "venue"}});
    let e = entity(&json!({
        "tags": {"type": "ListProperty", "valueList": ["koncert", "jazz"]},
        "venue": {"type": "Relationship", "object": "urn:ngsi-ld:Place:dom-kultury"},
        "organizer": {"type": "Property", "value": {"name": "Mestský \"úrad\"\r\nATTENDEE:x", "url": "https://zvolen.sk"}}}));
    let l = render(&ical, &e);
    assert!(has(&l, "CATEGORIES:koncert,jazz"), "{l:?}");
    assert!(
        has(&l, "DESCRIPTION:urn:ngsi-ld:Place:dom-kultury"),
        "{l:?}"
    );
    // CN is a quoted parameter: a quote and LF are caret escapes, CR is dropped
    assert!(
        has(
            &l,
            "ORGANIZER;CN=\"Mestský ^'úrad^'^nATTENDEE:x\":https://zvolen.sk"
        ),
        "{l:?}"
    );
    let e = entity(&json!({
        "tags": {"type": "ListRelationship", "objectList": [{"object": "urn:a"}, {"object": "urn:b"}]},
        "organizer": {"type": "Property", "value": {"name": "MsÚ", "email": "kultura@zvolen.sk"}}}));
    let l = render(&ical, &e);
    assert!(has(&l, "CATEGORIES:urn:a,urn:b"), "{l:?}");
    assert!(
        has(&l, "ORGANIZER;CN=\"MsÚ\":mailto:kultura@zvolen.sk"),
        "{l:?}"
    );
}

#[test]
fn a_value_cannot_inject_a_property_line() {
    let ical = json!({"preset": "sdm-event"});
    let e = entity(&json!({
        "name": {"type": "Property", "value": "Trh\r\nATTENDEE:mailto:x@y.sk\r\nEND:VEVENT"}}));
    // a URL is no TEXT to escape: one with a line break is no event at all
    let bad_url =
        entity(&json!({"url": {"type": "Property", "value": "https://a.sk/\r\nX-EVIL:1"}}));
    assert_eq!(render(&ical, &bad_url), skipped(reason::URL_NOT_HTTP));
    let plan = plan(&ical).expect("holds");
    let Outcome::Event(text) = plan.vevent(&e, &Window::default(), now()) else {
        panic!("an event")
    };
    let props: Vec<&str> = text
        .split("\r\n")
        .map(|l| l.split([':', ';']).next().unwrap_or(l))
        .collect();
    assert!(
        !props.contains(&"ATTENDEE") && !props.contains(&"X-EVIL"),
        "{text}"
    );
    assert_eq!(
        text.split("\r\n").filter(|l| *l == "END:VEVENT").count(),
        1,
        "{text}"
    );
    assert!(
        text.contains("SUMMARY:Trh\\nATTENDEE:mailto:x@y.sk\\nEND:VEVENT"),
        "{text}"
    );
}

#[test]
fn dtstamp_is_the_latest_revision_of_the_mapped_attributes() {
    let ical = json!({"preset": "sdm-event"});
    let at = |t: &str| json!({"type": "Property", "value": "Trh", "modifiedAt": t});
    // the Entity was rewritten today, the attributes the feed shows were not
    let stamped = |entity_stamp: &str| {
        let mut e = entity(&json!({"name": at("2026-09-01T10:00:00Z"),
            "startDate": {"type": "Property", "value": "2026-10-20", "modifiedAt": "2026-09-02T10:00:00Z"},
            "unrelated": at("2026-10-21T00:00:00Z")}));
        e["modifiedAt"] = json!(entity_stamp);
        e
    };
    let first = render(&ical, &stamped("2026-10-21T00:00:00Z"));
    let second = render(&ical, &stamped("2026-10-22T00:00:00Z"));
    assert_eq!(first, second);
    assert!(has(&first, "DTSTAMP:20260902T100000Z"), "{first:?}");
    assert!(has(&first, "LAST-MODIFIED:20260902T100000Z"), "{first:?}");
    // a mapped LAST-MODIFIED is the stamp
    let mapped = json!({"preset": "sdm-event", "mapping": {"LAST-MODIFIED": "changed"}});
    let mut x = stamped("2026-10-22T00:00:00Z");
    x["changed"] = json!({"type": "Property", "value": "2026-08-01T12:00:00"});
    let l = render(&mapped, &x);
    assert!(has(&l, "DTSTAMP:20260801T100000Z"), "{l:?}");
}

#[test]
fn a_constant_duration_is_only_the_length_of_an_event_without_end() {
    let ical = json!({"preset": "sdm-event", "mapping": {"DURATION": {"constant": "PT1H"}}});
    let timed = entity(&json!({"startDate": {"type": "Property", "value": "2026-10-20T18:00:00"}}));
    assert!(has(&render(&ical, &timed), "DURATION:PT1H"));
    let mut ended = timed.clone();
    ended["endDate"] = json!({"type": "Property", "value": "2026-10-20T21:00:00"});
    let l = render(&ical, &ended);
    assert!(
        has(&l, "DTEND:20261020T190000Z") && !l.iter().any(|x| x.starts_with("DURATION")),
        "{l:?}"
    );
    // a date is one day by RFC 5545 3.6.1, no hour
    let l = render(&ical, &entity(&json!({})));
    assert!(!l.iter().any(|x| x.starts_with("DURATION")), "{l:?}");
    // a DURATION read from an attribute still excludes DTEND
    assert!(plan(&json!({"preset": "sdm-event", "mapping": {"DURATION": "length"}})).is_err());
}

fn output(o: &Value) -> Result<CompiledOutput, String> {
    let written: Output = serde_json::from_value(o.clone()).map_err(|e| e.to_string())?;
    CompiledOutput::compile(&written)
}

#[test]
fn an_output_is_refused_with_its_reason() {
    let ok = json!({"name": "vsetky", "sources": [{}]});
    assert!(output(&ok).is_ok());
    let prague = json!({"name": "vsetky", "timezone": "Europe/Prague", "sources": [{}]});
    assert!(output(&prague).is_ok());
    let many: Vec<Value> = (0..11).map(|_| json!({})).collect();
    for (bad, why) in [
        (json!({"version": 2}), "version"),
        (json!({"name": "Všetky"}), "1 to 40"),
        (json!({"title": {"s k": "x"}}), "language tag"),
        (
            json!({"title": {"sk": "a\r\nX-EVIL:1"}}),
            "control characters",
        ),
        (json!({"lang": "slovensky"}), "language tag"),
        (json!({"limit": 0}), "limit"),
        (json!({"sort": "name"}), "start or -start"),
        (json!({"filter": {"q": "a==1\n"}}), "filter"),
        (json!({"sources": []}), "1 to 10"),
        (json!({"sources": many}), "1 to 10"),
        (json!({"timezone": "Mars/Olympus"}), "IANA"),
        (json!({"sources": [{}, {"type": "Event"}]}), "twice"),
        (
            json!({"sources": [{"type": "Festival"}]}),
            "no calendar preset",
        ),
    ] {
        let mut o = ok.clone();
        for (k, v) in bad.as_object().into_iter().flatten() {
            o[k] = v.clone();
        }
        let err = output(&o).expect_err("refused");
        assert!(err.contains(why), "{o}: {err}");
    }
    for name in ["", "preview", &"a".repeat(41), "a_b"] {
        assert!(!is_output_name(name), "{name:?}");
    }
    assert!(is_output_name("okres-zvolen-2"));
    for tag in ["@none", "sk", "sk-SK", "zh-Hant", "de-CH-1996"] {
        assert!(is_language_tag(tag), "{tag}");
    }
    for tag in [
        "s",
        "slov",
        "sk-",
        "sk_SK",
        "sk-abcdefghi",
        &"sk-a".repeat(10),
    ] {
        assert!(!is_language_tag(tag), "{tag}");
    }
}

#[test]
fn an_organizer_object_writes_its_name_as_cn_and_needs_a_url() {
    // T-5134: the sdm-event preset maps `organizer`, the Event model's `{name, url}`.
    let ical = json!({"preset": "sdm-event"});
    let organizer =
        |value: Value| entity(&json!({"organizer": {"type": "Property", "value": value}}));
    let l = render(
        &ical,
        &organizer(json!({"name": "Kultúra, šport; \"Zvolen\"", "url": "https://www.zvolen.sk"})),
    );
    assert!(
        has(
            &l,
            "ORGANIZER;CN=\"Kultúra, šport; ^'Zvolen^'\":https://www.zvolen.sk"
        ),
        "{l:?}"
    );
    let l = render(
        &ical,
        &organizer(json!({"name": "a^b\nc\td", "url": "mailto:info@zvolen.sk"})),
    );
    assert!(
        has(&l, "ORGANIZER;CN=\"a^^b^ncd\":mailto:info@zvolen.sk"),
        "{l:?}"
    );
    let l = render(&ical, &organizer(json!({"url": "https://www.zvolen.sk"})));
    assert!(has(&l, "ORGANIZER:https://www.zvolen.sk"), "{l:?}");
    // no url, or one that is no cal-address: no ORGANIZER, never the event's own URL instead
    for value in [
        json!({"name": "Mestský úrad"}),
        json!({"name": "Mestský úrad", "url": "zvolen.sk"}),
        json!({"name": "Mestský úrad", "url": "https://x.sk/a b"}),
    ] {
        let mut e = organizer(value);
        e["url"] = json!({"type": "Property", "value": "https://www.zvolen.sk/podujatie/1"});
        let l = render(&ical, &e);
        assert!(!l.iter().any(|x| x.starts_with("ORGANIZER")), "{l:?}");
        assert!(has(&l, "URL:https://www.zvolen.sk/podujatie/1"), "{l:?}");
    }
}
