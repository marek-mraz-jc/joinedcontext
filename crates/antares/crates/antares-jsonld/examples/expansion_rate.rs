// SPDX-License-Identifier: EUPL-1.2
//! The go/no-go threshold for a hand-rolled JSON-LD processor was ≥5k
//! expansions/s/core. Antares hand-rolled its processor from day one rather
//! than forking a `json-ld` crate — this measures it, and exits non-zero
//! below the gate.
//!
//!     cargo run -p ngsild-jsonld --release --example expansion_rate
use ngsild_jsonld::{core_context, expand_entity, ExpandOpts};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = core_context();
    let entity: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
        r#"{
        "id": "urn:ngsi-ld:Vehicle:bench-1", "type": "Vehicle",
        "speed": {"type": "Property", "value": 55.1,
                  "observedAt": "2026-08-04T12:00:00Z", "unitCode": "KMH",
                  "source": {"type": "Property", "value": "GPS"}},
        "heading": {"type": "Property", "value": 180},
        "isParked": {"type": "Relationship",
                      "object": "urn:ngsi-ld:OffStreetParking:p1",
                      "providedBy": {"type": "Relationship",
                                      "object": "urn:ngsi-ld:Person:bob"}},
        "location": {"type": "GeoProperty",
                      "value": {"type": "Point", "coordinates": [13.35, 52.51]}},
        "name": {"type": "LanguageProperty",
                  "languageMap": {"en": "car", "de": "Auto"}}
    }"#,
    )?;
    let n = 20_000u32;
    let start = std::time::Instant::now();
    for _ in 0..n {
        let out = expand_entity(&entity, &ctx, ExpandOpts::default())?;
        std::hint::black_box(out);
    }
    let secs = start.elapsed().as_secs_f64();
    let rate = f64::from(n) / secs;
    eprintln!("expansion rate: {rate:.0}/s/core ({n} iterations in {secs:.2}s)");
    if rate < 5_000.0 {
        return Err(
            format!("expansion rate {rate:.0}/s is below the 5k/s/core phase-0 gate").into(),
        );
    }
    Ok(())
}
