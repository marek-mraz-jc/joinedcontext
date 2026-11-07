//! Replays tests/parity/cases.json: each input converted by `protobuf` gives Bento 1.22.0's recorded output
//! (scripts/record-codec-parity.py), or fails as Bento's does. Error texts are compared whole for `to_json`; for
//! `from_json` only up to the message name, since the JSON parser's wording is prost-reflect's. Go's `proto.Marshal`
//! writes a dynamic message's fields in map order, different on every run, so a `from_json` case compares the
//! message converted back by `to_json`.

use std::path::Path;

use ekiben_core::ComponentConfig;
use ekiben_protobuf::ProtobufProcessor;
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

fn processor(config: Json) -> Result<ProtobufProcessor, ekiben_core::CoreError> {
    ProtobufProcessor::from_config(&ComponentConfig {
        label: String::new(),
        component_type: "protobuf".into(),
        config,
        processors: Vec::new(),
    })
}

#[test]
fn protobuf_matches_bento() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity");
    let cases: Vec<Json> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let mut failed = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let mut conf = case["processor"]["protobuf"].clone();
        let paths: Vec<Json> = conf["import_paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Json::String(dir.join(p.as_str().unwrap()).to_string_lossy().into_owned()))
            .collect();
        conf["import_paths"] = Json::Array(paths);
        let from_json = conf["operator"] == "from_json";
        let mut back = conf.clone();
        back["operator"] = "to_json".into();
        let proc = processor(conf).unwrap_or_else(|e| panic!("{name}: {e}"));
        let input = std::fs::read(dir.join(case["input"].as_str().unwrap())).unwrap();
        let mut got = proc.convert(&input);
        if from_json {
            got = got.and_then(|bin| processor(back).unwrap().convert(&bin));
        }
        let got = got.map(|mut o| {
            o.push(b'\n');
            o
        });
        let want = &case["expected"];
        let ok = match (&got, want.get("error").and_then(Json::as_str)) {
            (Err(e), Some(w)) if from_json => e.split(": ").next() == w.split(": ").next(),
            (Err(e), Some(w)) => e == w,
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
