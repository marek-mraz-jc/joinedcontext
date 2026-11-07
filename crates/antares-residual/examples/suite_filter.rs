// SPDX-License-Identifier: EUPL-1.2
//! Filters the ETSI NGSI-LD test suite's Entity fixtures with one residual expression,
//! `speed gt 50 or brandName eq 'Mercedes'`, and prints each Entity's three-valued answer.
//!
//!     cargo run -p antares-residual --example suite_filter [ngsi-ld-test-suite/data/entities]
#![allow(clippy::print_stdout)]
use ngsild_residual::{compile, eval, Cmp, DateTime, Expr, Lit, Path};
use serde_json::Value;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ngsi-ld-test-suite/data/entities"),
        PathBuf::from,
    );
    let cmp = |op, attr: &str, lit| {
        Expr::Cmp(
            op,
            Box::new(Expr::Path(Path::value(attr))),
            Box::new(Expr::Lit(lit)),
        )
    };
    let filter = Expr::Or(vec![
        cmp(Cmp::Gt, "speed", Lit::Int(50)),
        cmp(Cmp::Eq, "brandName", Lit::Str("Mercedes".into())),
    ]);
    let program = compile(
        &filter,
        DateTime::parse("2026-10-12T00:00:00Z").ok_or("now")?,
    )?;
    let mut files: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for path in files {
        let Ok(entity @ Value::Object(_)) =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        else {
            continue;
        };
        println!(
            "{}: {:?}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            eval(&program, &entity)?
        );
    }
    Ok(())
}
