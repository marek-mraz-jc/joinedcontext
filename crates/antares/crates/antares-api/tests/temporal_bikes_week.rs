// SPDX-License-Identifier: EUPL-1.2
//! T-4669: one Helsinki bike station's week replayed under `onChange` with a
//! one-hour heartbeat (platform ADR-N-054 2.2) gives the answers every written
//! value gives, from far fewer rows.
//!
//! The week is synthetic with the shape the supervisor measured on dev
//! (2026-10-18): 6,336 writes 86 s apart over 6.3 days, and 137, 135, 3 and 1
//! value changes of the four attributes. `every` keeps each write as a row
//! (temporal_on_change), so the written series is its answer; replaying it
//! through the memory store as well costs minutes, since that store scans an
//! Entity's whole history on each append.

use antares_api::{AppState, OnChange, RecordMode, Recording};
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use antares_store::TemporalDriver;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const CORE: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const ID: &str = "urn:ngsi-ld:BikeHireDockingStation:week";
const WRITES: usize = 6336;
const STEP_SECS: i64 = 86;
const NUMBERS: [&str; 3] = ["availableBikeNumber", "freeSlotNumber", "totalSlotNumber"];

fn start() -> DateTime<Utc> {
    "2026-10-11T00:00:00Z".parse().expect("an instant")
}

fn at(i: usize) -> DateTime<Utc> {
    start() + Duration::seconds(STEP_SECS * i as i64)
}

fn stamp(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// `changes` distinct write indices (never the first) from a fixed LCG, so
/// the week is the same on every run.
fn change_points(changes: usize, seed: u64) -> std::collections::BTreeSet<usize> {
    let mut picked = std::collections::BTreeSet::new();
    let mut x = seed;
    while picked.len() < changes {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        picked.insert(1 + (x >> 33) as usize % (WRITES - 1));
    }
    picked
}

/// The week: per write, the four values.
fn week() -> Vec<[Value; 4]> {
    let bikes = change_points(137, 1);
    let free = change_points(135, 2);
    let status = change_points(3, 3);
    let total = change_points(1, 4);
    let (mut b, mut f, mut s, mut t) = (10_i64, 14_i64, 0_usize, 24_i64);
    (0..WRITES)
        .map(|i| {
            if bikes.contains(&i) {
                b = (b + [1, -2, 3, -1][i % 4]).clamp(0, 30);
            }
            if free.contains(&i) {
                f = (f + [-1, 2, -3, 1][i % 4]).clamp(0, 30);
            }
            if status.contains(&i) {
                s += 1;
            }
            if total.contains(&i) {
                t = 30;
            }
            let state = ["working", "outOfService"][s % 2];
            [json!(b), json!(f), json!(t), json!(state)]
        })
        .collect()
}

fn names() -> [&'static str; 4] {
    [NUMBERS[0], NUMBERS[1], NUMBERS[2], "status"]
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
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
    let resp = app
        .clone()
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

/// A broker with the week written, recording `recording` (`None`: every),
/// and the rows its history holds.
async fn replayed(recording: Option<Recording>) -> (Router, u64) {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store.clone(), "memory");
    antares_api::wire(&mut st).await;
    if let Some(recording) = recording {
        st.history_off.write().expect("rules").recording.insert(
            (
                antares_model::TenantId::default().as_str().to_owned(),
                format!("{CORE}BikeHireDockingStation"),
            ),
            recording,
        );
    }
    let app = antares_api::router(st);
    for (i, values) in week().into_iter().enumerate() {
        let observed = stamp(at(i));
        let mut attrs = serde_json::Map::new();
        for (name, value) in names().into_iter().zip(values) {
            attrs.insert(
                name.into(),
                json!({"type": "Property", "value": value, "observedAt": observed}),
            );
        }
        let (s, b) = if i == 0 {
            attrs.insert("id".into(), json!(ID));
            attrs.insert("type".into(), json!("BikeHireDockingStation"));
            call(
                &app,
                "POST",
                "/ngsi-ld/v1/entities",
                Some(Value::Object(attrs)),
            )
            .await
        } else {
            let path = format!("/ngsi-ld/v1/entities/{ID}/attrs");
            call(&app, "PATCH", &path, Some(Value::Object(attrs))).await
        };
        assert!(s.is_success(), "write {i}: {s} {b}");
    }
    let rows = store
        .attr_instance_count(&antares_model::TenantId::default())
        .await
        .expect("a count");
    (app, rows)
}

/// The value `attr` held just before `t`, as the temporal API answers it.
async fn held_before(app: &Router, attr: &str, t: DateTime<Utc>) -> Value {
    let path = format!(
        "/ngsi-ld/v1/temporal/entities/{ID}?attrs={attr}&timerel=before&timeAt={}&lastN=1",
        stamp(t)
    );
    let (s, doc) = call(app, "GET", &path, None).await;
    assert_eq!(s, 200, "{doc}");
    match &doc[attr] {
        Value::Array(rows) => rows.last().expect("a row")["value"].clone(),
        row => row["value"].clone(),
    }
}

/// The hourly averages of `attr` the broker answers, by bucket start.
async fn hourly(app: &Router, attr: &str) -> Vec<(String, f64)> {
    let path = format!(
        "/ngsi-ld/v1/temporal/entities/{ID}?attrs={attr}&options=aggregatedValues\
         &aggrMethods=avg&aggrPeriodDuration=PT1H&timerel=between&timeAt={}&endTimeAt={}",
        stamp(start()),
        stamp(at(WRITES))
    );
    let (s, doc) = call(app, "GET", &path, None).await;
    assert_eq!(s, 200, "{doc}");
    doc[attr]["avg"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|b| {
            (
                b[1].as_str().expect("start").to_owned(),
                b[0].as_f64().expect("a mean"),
            )
        })
        .collect()
}

/// The time-weighted hourly average of every written value of `attr`, each
/// holding until the next write, the last until the window ends.
fn weighted_truth(column: usize) -> Vec<(String, f64)> {
    let values: Vec<f64> = week()
        .iter()
        .map(|row| row[column].as_f64().expect("a number"))
        .collect();
    let end = at(WRITES);
    let mut out = Vec::new();
    let mut bucket = start();
    while bucket < end {
        let next = (bucket + Duration::hours(1)).min(end);
        let (mut sum, mut span) = (0.0, 0.0);
        for (i, v) in values.iter().enumerate() {
            let (from, to) = (at(i).max(bucket), at(i + 1).min(next));
            if to > from {
                let secs = (to - from).num_seconds() as f64;
                sum += v * secs;
                span += secs;
            }
        }
        out.push((stamp(bucket), sum / span));
        bucket = next;
    }
    out
}

/// The value `column` held just before `t`: the last write before it.
fn held_truth(column: usize, t: DateTime<Utc>) -> Value {
    let week = week();
    let last = (0..WRITES)
        .rev()
        .find(|&i| at(i) < t)
        .expect("a write before");
    week[last][column].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_station_week_on_change_answers_as_every_from_far_fewer_rows() {
    let hour = Some(RecordMode::OnChange(OnChange {
        heartbeat: std::time::Duration::from_secs(3600),
        deadband: None,
        min_distance: None,
    }));
    let (changed, rows) = replayed(Some(Recording {
        default: hour,
        attributes: Default::default(),
    }))
    .await;

    let every = (WRITES * names().len()) as u64;
    let saved = 1.0 - rows as f64 / every as f64;
    eprintln!(
        "T-4669 week: every {every} rows, onChange+PT1H {rows} rows, {:.1} % fewer",
        saved * 100.0
    );
    assert!(saved > 0.95, "{rows} of {every}");

    // The value at any instant, and the last one, are the written ones.
    let instants = change_points(10, 9).into_iter().map(at).chain([at(WRITES)]);
    for t in instants {
        for (column, attr) in names().into_iter().enumerate() {
            assert_eq!(
                held_before(&changed, attr, t).await,
                held_truth(column, t),
                "{attr} before {t}"
            );
        }
    }

    // Each hour's mean, weighted by the time each value held, is the one
    // every written value gives, an hour with no row included.
    for (column, attr) in NUMBERS.into_iter().enumerate() {
        let truth = weighted_truth(column);
        let answered = hourly(&changed, attr).await;
        assert_eq!(answered.len(), truth.len(), "{attr}: an hour left out");
        for ((bucket, mean), (want_bucket, want)) in answered.iter().zip(&truth) {
            assert_eq!(bucket, want_bucket, "{attr}");
            assert!(
                (mean - want).abs() < 1e-9,
                "{attr} {bucket}: {mean} != {want}"
            );
        }
    }
}
