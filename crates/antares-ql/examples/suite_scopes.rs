// SPDX-License-Identifier: EUPL-1.2
//! Runs the scopeQ rows of the ETSI NGSI-LD test suite's QueryEntities TPs (019_01_06, 019_34)
//! over the two Entities those TPs create, and prints how many each selects next to the count
//! the TP expects.
//!
//!     cargo run -p antares-ql --example suite_scopes [ngsi-ld-test-suite]
#![allow(clippy::print_stdout)]
use ngsild_ql::scope::{check_scope_q, scope_matches};
use serde_json::Value;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let suite = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite"),
        PathBuf::from,
    );
    let mut entities = Vec::new();
    for f in [
        "building-minimal-with-one-scope.json",
        "building-minimal-with-many-scopes.json",
    ] {
        entities.push(serde_json::from_str::<Value>(&std::fs::read_to_string(
            suite.join("data/entities").join(f),
        )?)?);
    }
    let tps = suite.join("TP/NGSI-LD/ContextInformation/Consumption/Entity/QueryEntities");
    for tp in ["019_01_06.robot", "019_34.robot"] {
        for line in std::fs::read_to_string(tps.join(tp))?.lines() {
            // a template row: four spaces, the scopeQ, four spaces, ${count}
            let mut cols = line.strip_prefix("    ").unwrap_or_default().split("    ");
            let (Some(q), Some(want)) = (cols.next(), cols.next()) else {
                continue;
            };
            if !q.starts_with(['/', '(']) || !want.starts_with("${") {
                continue;
            }
            let got = match check_scope_q(q) {
                Ok(()) => entities
                    .iter()
                    .filter(|e| scope_matches(q, e))
                    .count()
                    .to_string(),
                Err(e) => format!("refused {}", e.status()),
            };
            println!("{tp} {q:<70} selects {got}, TP expects {want}");
        }
    }
    Ok(())
}
