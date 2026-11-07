//! Replays tests/parity/cases.json: each input decoded by `parquet_decode` gives Bento 1.22.0's recorded output
//! (scripts/record-codec-parity.py), one JSON line per row, or Bento's error text.

use std::path::Path;

use bytes::Bytes;
use ekiben_core::Part;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

fn run(input: &[u8], list_format: bool) -> Result<Vec<u8>, String> {
    let rows = ekiben_parquet::decode(Bytes::copy_from_slice(input), list_format)?;
    let mut out = Vec::new();
    for row in rows {
        let mut p = Part::from_structured(row);
        out.extend_from_slice(&p.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

#[test]
fn parquet_decode_matches_bento() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let cases: Vec<Json> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let mut failed = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let conf = &case["processor"]["parquet_decode"];
        let list_format = conf
            .get("use_parquet_list_format")
            .and_then(Json::as_bool)
            .unwrap_or(true);
        let input = std::fs::read(dir.join(case["input"].as_str().unwrap())).unwrap();
        let got = run(&input, list_format);
        let want = &case["expected"];
        let ok = match (&got, want.get("error")) {
            (Err(e), Some(w)) => e == w.as_str().unwrap(),
            (Ok(out), None) => match want.get("text") {
                Some(t) => out.as_slice() == t.as_str().unwrap().as_bytes(),
                None => hex::encode(Sha256::digest(out)) == want["sha256"].as_str().unwrap(),
            },
            _ => false,
        };
        if !ok {
            let shown = match &got {
                Ok(o) => String::from_utf8_lossy(&o[..o.len().min(600)]).into_owned(),
                Err(e) => format!("error: {e}"),
            };
            failed.push(format!("{name}: want {want}\n  got {shown}"));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ from Bento:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}
