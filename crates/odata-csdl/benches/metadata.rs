// SPDX-License-Identifier: EUPL-1.2
//! `$metadata` build: validation plus both writers over the fixture model (ADR-0024
//! decision 8). Std-only timing (criterion is not in the lock file); `cargo bench -p odata-csdl`
//! prints the mean per build.

#[path = "../tests/fixture/mod.rs"]
mod fixture;

use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use odata_csdl::{write_json, write_xml, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const RUNS: u32 = 2_000;
    let model = fixture::ngsi_ld_model();
    let limits = Limits::strict();
    let mut out = Vec::with_capacity(64 * 1024);
    for (name, json) in [("xml", false), ("json", true)] {
        let started = Instant::now();
        for _ in 0..RUNS {
            out.clear();
            if json {
                write_json(black_box(&model), &limits, &mut out)?;
            } else {
                write_xml(black_box(&model), &limits, &mut out)?;
            }
        }
        let mean = started.elapsed().checked_div(RUNS).unwrap_or_default();
        writeln!(
            std::io::stdout().lock(),
            "metadata/{name}: {mean:?} per build, {} bytes",
            out.len()
        )?;
    }
    Ok(())
}
