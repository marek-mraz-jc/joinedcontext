// SPDX-License-Identifier: EUPL-1.2
//! How a type records history (T-4665, platform ADR-N-054 2.2): `every`
//! writes a row per new `observedAt`, even with the same value; `onChange`
//! writes one when the value moves past its deadband or minimum distance,
//! when a value of the same instant is corrected, and once per heartbeat.

use antares_api::{AppState, OnChange, RecordMode, Recording};
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const CORE: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

async fn state(recording: Option<Recording>) -> AppState {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store, "memory");
    antares_api::wire(&mut st).await;
    if let Some(recording) = recording {
        st.history_off.write().expect("rules").recording.insert(
            (
                antares_model::TenantId::default().as_str().to_owned(),
                format!("{CORE}Spot"),
            ),
            recording,
        );
    }
    st
}

async fn req(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("req"))
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Writes `writes` (value, minute) of one attribute to a fresh Spot and
/// answers the values its history recorded.
async fn recorded(st: &AppState, id: &str, attr: &str, writes: &[(Value, u32)]) -> Vec<Value> {
    let at = |m: u32| format!("2026-10-18T{:02}:{:02}:00Z", 7 + m / 60, m % 60);
    recorded_at(st, id, attr, writes, at).await
}

/// [`recorded`] with the instants `at` makes of each write's number.
async fn recorded_at(
    st: &AppState,
    id: &str,
    attr: &str,
    writes: &[(Value, u32)],
    at: impl Fn(u32) -> String,
) -> Vec<Value> {
    let (first, m0) = &writes[0];
    let (s, b) = req(
        st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": id, "type": "Spot",
                    attr: {"type": "Property", "value": first, "observedAt": at(*m0)}})),
    )
    .await;
    assert_eq!(s, 201, "{b}");
    for (value, m) in &writes[1..] {
        let (s, b) = req(
            st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{id}/attrs"),
            Some(json!({attr: {"type": "Property", "value": value, "observedAt": at(*m)}})),
        )
        .await;
        assert_eq!(s, 204, "{b}");
    }
    let (s, doc) = req(
        st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{doc}");
    let rows = match doc.get(attr) {
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
        None => vec![],
    };
    rows.iter().map(|r| r["value"].clone()).collect()
}

fn on_change(heartbeat_minutes: u64, deadband: Option<f64>) -> Option<RecordMode> {
    Some(RecordMode::OnChange(OnChange {
        heartbeat: Duration::from_secs(heartbeat_minutes * 60),
        deadband,
        min_distance: None,
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn every_records_the_same_value_at_each_new_instant() {
    let st = state(None).await;
    let writes = [(json!("free"), 0), (json!("free"), 1), (json!("free"), 2)];
    assert_eq!(
        recorded(&st, "urn:ngsi-ld:Spot:every", "status", &writes)
            .await
            .len(),
        3
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn on_change_records_a_change_and_a_heartbeat_and_nothing_in_between() {
    let st = state(Some(Recording {
        default: on_change(60, None),
        ..Default::default()
    }))
    .await;
    let writes = [
        (json!("free"), 0),
        (json!("free"), 1),      // unchanged: no row
        (json!("occupied"), 2),  // changed: a row
        (json!("occupied"), 30), // unchanged inside the heartbeat: no row
        (json!("occupied"), 62), // the heartbeat passed: a row
    ];
    let rows = recorded(&st, "urn:ngsi-ld:Spot:status", "status", &writes).await;
    assert_eq!(rows, [json!("free"), json!("occupied"), json!("occupied")]);
}

/// T-5046's Vehicle rule: a writer of every second whose value moves every
/// second, under a 30 s heartbeat and a deadband no real move crosses,
/// leaves one history row per 30 s (T-5050).
#[tokio::test(flavor = "multi_thread")]
async fn a_one_second_writer_records_one_row_per_30_seconds() {
    let st = state(Some(Recording {
        default: Some(RecordMode::OnChange(OnChange {
            heartbeat: Duration::from_secs(30),
            deadband: Some(1e6),
            min_distance: None,
        })),
        ..Default::default()
    }))
    .await;
    let writes: Vec<(Value, u32)> = (0..=90).map(|s| (json!(s), s)).collect();
    let at = |s: u32| format!("2026-10-18T07:{:02}:{:02}Z", s / 60, s % 60);
    let rows = recorded_at(&st, "urn:ngsi-ld:Spot:speed", "speed", &writes, at).await;
    assert_eq!(rows, [json!(0), json!(30), json!(60), json!(90)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deadband_holds_back_small_moves_and_an_attribute_may_record_every() {
    let recording = Recording {
        default: on_change(60, Some(1.0)),
        attributes: [(format!("{CORE}count"), None)].into_iter().collect(),
    };
    let st = state(Some(recording)).await;
    let level = [
        (json!(10.0), 0),
        (json!(10.6), 1),
        (json!(11.2), 2),
        (json!(10.9), 3),
    ];
    // 10.6 and 10.9 stay within 1.0 of the last row; 11.2 is 1.2 past 10.0.
    assert_eq!(
        recorded(&st, "urn:ngsi-ld:Spot:level", "occupancy", &level).await,
        [json!(10.0), json!(11.2)]
    );
    // A count per interval is never left out (its own `every`).
    let count = [(json!(3), 0), (json!(3), 1), (json!(3), 2)];
    assert_eq!(
        recorded(&st, "urn:ngsi-ld:Spot:count", "count", &count)
            .await
            .len(),
        3
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_correction_of_the_same_instant_is_recorded_whatever_the_deadband() {
    let st = state(Some(Recording {
        default: on_change(60, Some(5.0)),
        ..Default::default()
    }))
    .await;
    let writes = [(json!(10.0), 0), (json!(10.5), 0)];
    let rows = recorded(&st, "urn:ngsi-ld:Spot:fix", "occupancy", &writes).await;
    assert_eq!(
        rows,
        [json!(10.5)],
        "the instant's row now holds the corrected value"
    );
}

/// The Spot `id` written `bikes` (onChange) and `count` (its own `every`)
/// with the same minute series, as a replayed bike station.
async fn replayed(st: &AppState, id: &str, series: &[(f64, u32)]) {
    let writes: Vec<(Value, u32)> = series.iter().map(|&(v, m)| (json!(v), m)).collect();
    recorded(st, id, "bikes", &writes).await;
    for (value, m) in series {
        let (s, b) = req(
            st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{id}/attrs"),
            Some(json!({"count": {"type": "Property", "value": value,
                         "observedAt": format!("2026-10-18T07:{m:02}:00Z")}})),
        )
        .await;
        assert_eq!(s, 204, "{b}");
    }
}

fn station() -> Option<Recording> {
    Some(Recording {
        default: on_change(60, None),
        attributes: [(format!("{CORE}count"), None)].into_iter().collect(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_starts_at_the_level_an_on_change_attribute_held() {
    let st = state(station()).await;
    let id = "urn:ngsi-ld:Spot:carry";
    replayed(&st, id, &[(5.0, 0), (5.0, 1), (7.0, 2)]).await;
    let (s, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{id}?timerel=after&timeAt=2026-10-18T07:01:30Z"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{doc}");
    let at = |attr: &str| -> Vec<Value> {
        match &doc[attr] {
            Value::Array(a) => a.iter().map(|r| r["observedAt"].clone()).collect(),
            v => vec![v["observedAt"].clone()],
        }
    };
    // bikes held 5 from 07:00 when the window opened; count has a row of its own
    assert_eq!(
        at("bikes"),
        [json!("2026-10-18T07:00:00Z"), json!("2026-10-18T07:02:00Z")]
    );
    assert_eq!(at("count"), [json!("2026-10-18T07:02:00Z")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_time_weighted_average_of_on_change_rows_is_the_average_of_every_row() {
    let st = state(station()).await;
    let id = "urn:ngsi-ld:Spot:bikes";
    let series = [5.0, 5.0, 5.0, 7.0, 7.0, 3.0, 3.0, 3.0, 3.0, 6.0];
    let minutes: Vec<(f64, u32)> = series.iter().copied().zip(0..).collect();
    replayed(&st, id, &minutes).await;
    let window = "timerel=between&timeAt=2026-10-18T07:00:00Z&endTimeAt=2026-10-18T07:10:00Z";
    let aggregated = |methods: &str| {
        format!(
            "/ngsi-ld/v1/temporal/entities/{id}?options=aggregatedValues\
             &aggrPeriodDuration=PT0S&aggrMethods={methods}&{window}"
        )
    };
    let (s, doc) = req(&st, "GET", &aggregated("avg,min,max"), None).await;
    assert_eq!(s, 200, "{doc}");
    // bikes keeps 4 of the 10 rows; weighted by the minutes each held, its
    // mean is the plain mean of all 10 that count kept
    assert_eq!(doc["bikes"]["avg"][0][0], json!(4.7), "{doc}");
    assert_eq!(doc["count"]["avg"][0][0], json!(4.7), "{doc}");
    assert_eq!(doc["bikes"]["min"][0][0], json!(3.0));
    assert_eq!(doc["bikes"]["max"][0][0], json!(7.0));

    // A left-out row is not a zero: sum and totalCount mean nothing on bikes.
    for method in ["sum", "totalCount"] {
        let (s, doc) = req(&st, "GET", &aggregated(method), None).await;
        assert_eq!(s, 400, "{method}: {doc}");
        let detail = doc["detail"].as_str().unwrap_or_default();
        assert!(detail.starts_with("aggregation-not-meaningful"), "{doc}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_attribute_fills_no_period_past_now_and_refuses_too_many() {
    let st = state(station()).await;
    let id = "urn:ngsi-ld:Spot:far";
    replayed(&st, id, &[(5.0, 0), (7.0, 2)]).await;
    let aggregated = |period: &str| {
        format!(
            "/ngsi-ld/v1/temporal/entities/{id}?options=aggregatedValues&attrs=bikes\
             &aggrPeriodDuration={period}&aggrMethods=avg&timerel=between\
             &timeAt=2026-10-18T07:00:00Z&endTimeAt=9999-12-31T00:00:00Z"
        )
    };
    // a day per period: the held value fills up to now, not to the year 9999
    let started = std::time::Instant::now();
    let (s, doc) = req(&st, "GET", &aggregated("P1D"), None).await;
    assert_eq!(s, 200, "{doc}");
    let days = doc["bikes"]["avg"].as_array().map_or(0, Vec::len);
    let first = chrono::DateTime::parse_from_rfc3339("2026-10-18T07:00:00Z").expect("instant");
    let since = chrono::Utc::now()
        .fixed_offset()
        .signed_duration_since(first)
        .num_days();
    assert!(
        days >= 1 && days <= usize::try_from(since + 2).unwrap_or(usize::MAX),
        "{days}: {doc}"
    );
    // a second per period from then to now is more periods than one answer holds
    let (s, doc) = req(&st, "GET", &aggregated("PT1S"), None).await;
    assert_eq!(s, 400, "{doc}");
    assert!(
        doc["detail"].as_str().unwrap_or_default().contains("10000"),
        "{doc}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

/// T-5056: under `ANTARES_TEMPORAL_RECORD=ruled` a type no retention rule
/// opts in keeps no history, and its temporal Entity answers empty.
#[tokio::test(flavor = "multi_thread")]
async fn ruled_keeps_no_history_for_a_type_no_rule_opts_in() {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store, "memory");
    // before wire: the write path records through the state wire hands it
    st.temporal_record = antares_api::TemporalRecord::Ruled;
    antares_api::wire(&mut st).await;
    let writes = [(json!(1), 0), (json!(2), 1), (json!(3), 2)];
    let rows = recorded(&st, "urn:ngsi-ld:Spot:off", "occupancy", &writes).await;
    assert!(rows.is_empty(), "{rows:?}");
    let (s, doc) = req(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Spot:none",
        None,
    )
    .await;
    assert_eq!(s, 404, "an Entity that does not exist stays a 404: {doc}");
    st.history_off.write().expect("rules").ruled.insert((
        antares_model::TenantId::default().as_str().to_owned(),
        format!("{CORE}Spot"),
    ));
    let rows = recorded(&st, "urn:ngsi-ld:Spot:on", "occupancy", &writes).await;
    assert_eq!(
        rows,
        [json!(1), json!(2), json!(3)],
        "a rule's duration opts in"
    );
}

/// T-5056: `record: observed` adds a row only for a newer `observedAt`; a
/// value re-sent or changed under its old `observedAt` adds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn observed_records_a_row_per_new_observation_only() {
    let st = state(Some(Recording {
        default: Some(RecordMode::Observed),
        ..Default::default()
    }))
    .await;
    let writes = [
        (json!(5), 0),
        (json!(5), 0),
        (json!(6), 0),
        (json!(6), 1),
        (json!(7), 1),
    ];
    let rows = recorded(&st, "urn:ngsi-ld:Spot:obs", "occupancy", &writes).await;
    assert_eq!(rows, [json!(5), json!(6)], "one row per observedAt");
}
