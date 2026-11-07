//! The Endpoint list over T-4497's generated catalogue (T-4544, EP-01): not a gate, a
//! measurement. 10,000 Endpoints in one project, titled the way the gateway's generator titles
//! them (its 14 SDM topics, the 518 places read from dev in `fixtures/places-dev.txt`, 2023 to
//! 2026, the same xorshift64* stream and seed 4497), asked 200 times each through the whole
//! router: the first page, a deep page and the whole list the Endpoints page asks for today.
//!
//! ```text
//! cargo test --release --test endpoint_list_scale_bench -- --ignored --nocapture
//! ```

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::ResourceEnvelope;
use serde_json::{json, Value};

const PROJECT: &str = "bbsk";
const N: usize = 10_000;
const ASKS: usize = 200;

/// (type, en topic, sk topic): the gateway generator's topics, in its order.
const TOPICS: &[(&str, &str, &str)] = &[
    ("AirQualityObserved", "Air quality", "Kvalita ovzdušia"),
    (
        "WeatherObserved",
        "Weather observations",
        "Meteorologické pozorovania",
    ),
    ("Event", "Events", "Podujatia"),
    ("BikeHireDockingStation", "Bike sharing", "Zdieľané bicykle"),
    ("ParkingSpot", "Parking spots", "Parkovacie miesta"),
    ("TrafficFlowObserved", "Traffic counts", "Sčítanie dopravy"),
    (
        "KeyPerformanceIndicator",
        "Population statistics",
        "Štatistika obyvateľstva",
    ),
    ("PointOfInterest", "Points of interest", "Body záujmu"),
    ("WasteContainer", "Waste containers", "Kontajnery na odpad"),
    ("Alert", "Weather warnings", "Výstrahy pred počasím"),
    ("NoiseLevelObserved", "Noise levels", "Hladina hluku"),
    ("WaterQualityObserved", "Water quality", "Kvalita vody"),
    ("Streetlight", "Street lighting", "Verejné osvetlenie"),
    ("Vehicle", "Public transport vehicles", "Vozidlá MHD"),
];
const YEARS: [u32; 4] = [2023, 2024, 2025, 2026];

fn next(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn pick(state: &mut u64, len: usize) -> usize {
    usize::try_from(next(state) % len as u64).unwrap_or_default()
}

/// The generated catalogue as Endpoint manifests, `d00000` to `d09999`.
fn catalogue(seed: u64) -> Vec<ResourceEnvelope> {
    let places: Vec<&str> = include_str!("fixtures/places-dev.txt")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let mut state = seed.max(1);
    (0..N)
        .map(|i| {
            let (class, en, sk) = TOPICS[pick(&mut state, TOPICS.len())];
            let at = places[pick(&mut state, places.len())];
            let year = YEARS[pick(&mut state, YEARS.len())];
            let manifest = json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Endpoint",
                "metadata": {
                    "name": format!("d{i:05}"),
                    "namespace": PROJECT,
                    "title": {"sk": format!("{sk} {at} {year}"), "en": format!("{en} {at} {year}")},
                    "description": {
                        "sk": format!("{sk}, {at}, rok {year}."),
                        "en": format!("{en} in {at}, {year}."),
                    },
                },
                "spec": {
                    "contextSpaceRef": "otvorene-data",
                    "slug": format!("{i:0>26}"),
                    "audience": "public",
                    "entityTypes": [class],
                    "enabledRepresentations": ["ngsi-ld", "ogc-features"],
                },
                "status": {"phase": "Live"},
            });
            serde_json::from_value(manifest).expect("an Endpoint manifest")
        })
        .collect()
}

fn reader() -> Identity {
    common::person("reader")
}

/// p50 and p95, in milliseconds.
fn percentiles(mut took: Vec<Duration>) -> (f64, f64) {
    took.sort();
    let at = |p: usize| took[(took.len() * p / 100).min(took.len() - 1)].as_secs_f64() * 1000.0;
    (at(50), at(95))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement: run in release"]
async fn ten_thousand_endpoints_list_fast() {
    let state = common::state_on_forge().await;
    for endpoint in catalogue(4497) {
        state.mirror.upsert(endpoint);
    }
    state.mirror.upsert(common::envelope(
        "Role",
        "reads-endpoints",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "reads-endpoints",
            "scope": { "project": PROJECT },
        }),
    ));
    let base = format!("/api/v1/projects/{PROJECT}/endpoints");
    let deep = format!("{base}?limit=50&continue=d09900");
    let first = format!("{base}?limit=50");
    for (label, uri, rows) in [
        ("first page", &first, 50),
        ("deep page", &deep, 50),
        ("whole list", &base, N),
    ] {
        let mut took = Vec::with_capacity(ASKS);
        let mut bytes = 0;
        for _ in 0..ASKS {
            let started = Instant::now();
            let answer = common::send(&state, reader(), "GET", uri, None).await;
            took.push(started.elapsed());
            assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
            bytes = answer.text.len();
            let list: Value = serde_json::from_str(&answer.text).expect("a list");
            let items = list["items"].as_array().map_or(0, Vec::len);
            assert_eq!(items, rows, "{label}");
        }
        let (p50, p95) = percentiles(took);
        println!(
            "portal_endpoint_list {label}: p50={p50:.1}ms p95={p95:.1}ms rows={rows} bytes={bytes}"
        );
    }
}
