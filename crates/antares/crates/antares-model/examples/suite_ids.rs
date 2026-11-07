// SPDX-License-Identifier: EUPL-1.2
//! Runs the entry checks over the ETSI NGSI-LD test suite's Entity fixtures: each id through
//! `EntityId::new`, each Attribute name through `check_attr_name`.
//!
//!     cargo run -p antares-model --example suite_ids [ngsi-ld-test-suite/data/entities]
#![allow(clippy::print_stdout)]
use ngsild_model::{check_attr_name, EntityId};
use serde_json::Value;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite/data/entities"),
        PathBuf::from,
    );
    let mut files: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let Ok(Value::Object(doc)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        let id = doc
            .get("id")
            .and_then(Value::as_str)
            .map(|id| match EntityId::new(id) {
                Ok(_) => "id ok".to_owned(),
                Err(e) => format!("id {}: {e}", e.status()),
            });
        let names = doc
            .keys()
            .filter(|k| !["id", "type", "scope", "@context"].contains(&k.as_str()));
        let refused: Vec<_> = names.filter(|k| check_attr_name(k).is_err()).collect();
        println!(
            "{}: {}, {} refused names {refused:?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            id.unwrap_or_else(|| "no id".into()),
            refused.len()
        );
    }
    Ok(())
}
