// SPDX-License-Identifier: EUPL-1.2
//! Writes the `$metadata` of one NGSI-LD view (the test fixture) as `metadata.xml` and
//! `metadata.json` into the directory given as the only argument. `dev/odata-csdl-conformance.py`
//! validates both against the pinned OASIS schemas (ADR-0024 decision 6).

#[path = "../tests/fixture/mod.rs"]
mod fixture;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use odata_csdl::{write_json, write_xml, Limits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: metadata <out-dir>")?,
    );
    let model = fixture::ngsi_ld_model();
    write_xml(
        &model,
        &Limits::strict(),
        BufWriter::new(File::create(dir.join("metadata.xml"))?),
    )?;
    write_json(
        &model,
        &Limits::strict(),
        BufWriter::new(File::create(dir.join("metadata.json"))?),
    )?;
    Ok(())
}
