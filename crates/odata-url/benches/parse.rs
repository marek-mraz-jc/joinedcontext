// SPDX-License-Identifier: EUPL-1.2
//! Parse throughput over a few URLs of the subset (ADR-0024 decision 8). Std-only timing
//! (criterion is not in the lock file); `cargo bench -p odata-url` prints the mean per parse.

#[path = "../tests/service/mod.rs"]
mod service;

use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use odata_url::{parse, Limits};

const URLS: [(&str, &str); 4] = [
    ("key", "Buildings('b1')"),
    ("filter", "Buildings?$filter=height gt 10 and contains(name,'tower') or address/city eq 'Bonn'&$top=50&$orderby=height desc"),
    ("lambda", "Buildings?$filter=rooms/any(r:r/area gt 20) and geo.intersects(location,geography'SRID=4326;Polygon((0 0,1 0,1 1,0 0))')"),
    ("expand", "Buildings?$select=name,address/city&$expand=sensors($select=value;$expand=owner;$levels=2),owner/$ref"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const RUNS: u32 = 20_000;
    let limits = Limits::strict();
    for (name, url) in URLS {
        parse(url, &service::Service, &limits)?;
        let started = Instant::now();
        for _ in 0..RUNS {
            black_box(parse(black_box(url), &service::Service, &limits)?);
        }
        let mean = started.elapsed().checked_div(RUNS).unwrap_or_default();
        writeln!(
            std::io::stdout().lock(),
            "parse/{name}: {mean:?} per parse, {} bytes",
            url.len()
        )?;
    }
    Ok(())
}
