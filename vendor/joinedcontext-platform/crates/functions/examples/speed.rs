//! The runtime's speed (T-7774 item 6): `cargo run --release -p functions --example speed`.
//!
//! One invocation of an empty script in a fresh runtime (p50 and p99 over 1000 calls), then
//! `JSON.parse` and one group-by over 1,000 and 10,000 register entities.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use functions::sandbox::{run, Invocation};
use serde_json::{json, Value};

fn invocation(code: &str, data: Value) -> Invocation {
    Invocation {
        files: BTreeMap::from([(
            "script.js".to_owned(),
            format!("export default async (request) => {{ const data = request.body; const result = await (async () => {{ {code} }})(); return {{ body: result }}; }};"),
        )]),
        entry: "script.js".to_owned(),
        request: json!({ "method": "POST", "query": {}, "body": data, "user": null }),
        config: json!({}),
        endpoint: None,
    }
}

fn bridges(count: usize) -> Value {
    let classes = ["I", "II", "III", "IV", "V", "VI", "VII"];
    Value::Array(
        (0..count)
            .map(|i| {
                json!({
                    "id": format!("urn:ngsi-ld:Bridge:bbsk:{i:06}"), "type": "Bridge",
                    "name": format!("Most cez potok {i} pri obci Šťavnička"),
                    "conditionClass": classes[i % classes.len()], "length": 4.5 + (i % 97) as f64 * 1.25,
                    "district": "Banská Bystrica", "road": format!("III/{:05}", 50000 + i % 900),
                    "location": { "type": "Point", "coordinates": [19.1, 48.7] },
                })
            })
            .collect(),
    )
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

async fn measure(label: &str, calls: usize, code: &str, data: &Value) {
    let mut took = Vec::with_capacity(calls);
    for _ in 0..calls {
        let invocation = invocation(code, data.clone());
        let started = Instant::now();
        let outcome = run(invocation).await;
        took.push(started.elapsed());
        assert_eq!(outcome.status, 200, "{outcome:?}");
    }
    took.sort();
    println!(
        "{label}: p50 {:.2} ms, p99 {:.2} ms over {calls} calls",
        percentile(&took, 0.5).as_secs_f64() * 1e3,
        percentile(&took, 0.99).as_secs_f64() * 1e3
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let group = "const out = {}; for (const b of data) out[b.conditionClass] = (out[b.conditionClass] ?? 0) + 1; return out;";
    measure("empty script", 1000, "return null;", &Value::Null).await;
    for count in [1_000, 10_000] {
        let data = bridges(count);
        let bytes = serde_json::to_vec(&data).map_or(0, |b| b.len());
        measure(
            &format!("parse + group {count} entities ({bytes} bytes)"),
            100,
            group,
            &data,
        )
        .await;
    }
}
