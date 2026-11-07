// SPDX-License-Identifier: EUPL-1.2
//! The core request paths every query pays, without the HTTP and store
//! around them (T-4853): `q`, `geoQ`, type and `scopeQ` parsing, their SQL
//! compilation, and JSON-LD compaction of a 100-entity page. Each case
//! prints p50 and p99 nanoseconds per call; `BENCH_JSON=<file>` also writes
//! them as JSON, which `dev/perf/gate.py bench` compares with the committed
//! baseline in `ci/perf-baseline/bench-core.json`.
//!
//! Every case is measured right after `calibrate`, a fixed CPU workload: the
//! gate compares each case's ratio to it, so a slower or faster runner moves
//! the case and its yardstick together.
//!
//!     cargo bench -p antares-sql --bench core_paths

use antares_jsonld::{compact_entity, core_context, expand_entity, ExpandOpts};
use antares_ql::geo::{GeoQuery, GeoSpec, Rel};
use antares_ql::{check_type_selection, parse_q, sql::compile_q};
use antares_sql::compile::{geo::compile_geo, scope::compile_scope_q};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::hint::black_box;
use std::io::Write as _;
use std::time::{Duration, Instant};

const SAMPLES: usize = 300;
/// One sample lasts at least this long, so the clock's resolution is noise.
const SAMPLE: Duration = Duration::from_micros(200);

const Q: &str = r#"speed>100;brand=="Tesla"|(mileage==1000..9000;address[addressLocality]~="Ban.*";colour!="red")"#;
const TYPES: &str = "(Vehicle;Car)|Device,Building";
const SCOPE: &str = "/city/#,/region/+/sensors";

fn iri(t: &str) -> String {
    format!("https://uri.etsi.org/ngsi-ld/default-context/{t}")
}

fn vehicle(n: u32) -> Map<String, Value> {
    let v = json!({
        "id": format!("urn:ngsi-ld:Vehicle:bench:{n}"),
        "type": "Vehicle",
        "brand": {"type": "Property", "value": "Tesla"},
        "speed": {"type": "Property", "value": n % 130, "observedAt": "2026-10-20T12:00:00Z"},
        "mileage": {"type": "Property", "value": n * 1000, "unitCode": "KMT"},
        "colour": {"type": "Property", "value": "red"},
        "owner": {"type": "Relationship", "object": format!("urn:ngsi-ld:Person:{n}")},
        "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.1, 48.7]}}
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// p50 and p99 nanoseconds per call of `f`, over `SAMPLES` samples.
fn measure(mut f: impl FnMut()) -> (f64, f64) {
    // calls per sample, so one sample lasts at least SAMPLE
    let mut calls = 1u32;
    loop {
        let start = Instant::now();
        for _ in 0..calls {
            f();
        }
        if start.elapsed() >= SAMPLE || calls >= 1 << 20 {
            break;
        }
        calls *= 2;
    }
    let mut per_call: Vec<f64> = (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..calls {
                f();
            }
            start.elapsed().as_nanos() as f64 / f64::from(calls)
        })
        .collect();
    per_call.sort_by(f64::total_cmp);
    (per_call[SAMPLES / 2], per_call[SAMPLES * 99 / 100])
}

/// A fixed integer workload: the yardstick the cases are read against.
fn calibrate() {
    let mut x = black_box(0x9E37_79B9_7F4A_7C15u64);
    for _ in 0..1000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    black_box(x);
}

fn main() -> std::io::Result<()> {
    let ctx = core_context();
    let ast = parse_q(Q).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    let point = json!([19.1, 48.7]);
    let polygon = json!([[
        [19.0, 48.6],
        [19.3, 48.6],
        [19.3, 48.9],
        [19.0, 48.9],
        [19.0, 48.6]
    ]]);
    let near: HashMap<String, String> = [
        ("georel", "near;maxDistance==500"),
        ("geometry", "Point"),
        ("coordinates", "[19.1,48.7]"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let page: Vec<Value> = (0..100)
        .map(|n| expand_entity(&vehicle(n), &ctx, ExpandOpts::default()))
        .collect::<Result<_, _>>()
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;

    type Case<'a> = (&'a str, Box<dyn FnMut() + 'a>);
    let cases: Vec<Case> = vec![
        ("calibrate", Box::new(calibrate)),
        (
            "ql_parse_q",
            Box::new(|| drop(black_box(parse_q(black_box(Q))))),
        ),
        (
            "ql_check_type_selection",
            Box::new(|| drop(black_box(check_type_selection(black_box(TYPES))))),
        ),
        (
            "ql_geo_from_params",
            Box::new(|| drop(black_box(GeoQuery::from_params(black_box(&near))))),
        ),
        (
            "sql_compile_q",
            Box::new(|| drop(black_box(compile_q(black_box(&ast), "entity", 1, &iri)))),
        ),
        (
            "sql_compile_geo_near",
            Box::new(|| {
                let spec = GeoSpec {
                    rel: Rel::Near {
                        max: Some(500.0),
                        min: None,
                    },
                    geometry: "Point",
                    coordinates: &point,
                    geoproperty_iri: "",
                };
                drop(black_box(compile_geo(black_box(&spec), "location", 1)));
            }),
        ),
        (
            "sql_compile_geo_within",
            Box::new(|| {
                let spec = GeoSpec {
                    rel: Rel::Within,
                    geometry: "Polygon",
                    coordinates: &polygon,
                    geoproperty_iri: "",
                };
                drop(black_box(compile_geo(black_box(&spec), "location", 1)));
            }),
        ),
        (
            "sql_compile_scope_q",
            Box::new(|| drop(black_box(compile_scope_q(black_box(SCOPE), "scopes", 1)))),
        ),
        (
            "jsonld_compact_page_100",
            Box::new(|| {
                for e in &page {
                    drop(black_box(compact_entity(black_box(e), &ctx)));
                }
            }),
        ),
    ];

    let mut out = std::io::stdout().lock();
    let mut results = Map::new();
    for (name, mut f) in cases {
        // the yardstick again right before each case, so a clock that drifts
        // during the run (frequency, a neighbour) moves both sides of a ratio
        let (yardstick, _) = measure(calibrate);
        let (p50, p99) = measure(&mut f);
        writeln!(out, "{name:<28} p50 {p50:>12.0} ns   p99 {p99:>12.0} ns")?;
        results.insert(
            name.to_owned(),
            json!({"p50_ns": p50.round(), "p99_ns": p99.round(), "calibrate_p50_ns": yardstick.round()}),
        );
    }
    if let Ok(path) = std::env::var("BENCH_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"cases": results}))?)?;
    }
    Ok(())
}
