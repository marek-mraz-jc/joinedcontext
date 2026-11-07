// SPDX-License-Identifier: EUPL-1.2
//! Expands the ETSI NGSI-LD test suite's Entity fixtures with the suite's own `@context` and
//! prints what each one became: the stored attribute IRIs, or the 400 a broker would answer.
//!
//!     cargo run -p antares-jsonld --example suite_expand [ngsi-ld-test-suite]
#![allow(clippy::print_stdout)]
use ngsild_jsonld::{core_context, expand_entity, ExpandOpts};
use serde_json::Value;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let suite = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite"),
        PathBuf::from,
    );
    let terms: Value = serde_json::from_str(&std::fs::read_to_string(
        suite.join("resources/jsonld-contexts/ngsi-ld-test-suite.jsonld"),
    )?)?;
    let mut ctx = core_context();
    ctx.merge_object(
        terms["@context"]
            .as_object()
            .ok_or("the suite @context is not an object")?,
    )?;
    ctx.freeze();
    let mut files: Vec<_> = std::fs::read_dir(suite.join("data/entities"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let Ok(Value::Object(mut doc)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        doc.remove("@context");
        let opts = ExpandOpts {
            fragment: !doc.contains_key("id"),
            ..ExpandOpts::default()
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match expand_entity(&doc, &ctx, opts) {
            Ok(Value::Object(e)) => println!("{name}: {:?}", e.keys().collect::<Vec<_>>()),
            Ok(other) => println!("{name}: {other}"),
            Err(e) => println!("{name}: {} {e}", e.status()),
        }
    }
    Ok(())
}
