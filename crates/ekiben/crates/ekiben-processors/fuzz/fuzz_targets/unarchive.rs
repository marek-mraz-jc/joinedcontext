//! Input: one byte that picks the unarchive format, then the message.
#![no_main]

use libfuzzer_sys::fuzz_target;

const FORMATS: &[&str] = &[
    "json_array",
    "json_map",
    "json_documents",
    "lines",
    "binary",
    "tar",
    "zip",
    "csv",
    "csv:;",
    "xlsx:Sheet1",
    "xlsx",
    "ods",
    "shapefile",
    "xml_documents_to_json",
    "xml_documents_to_json:cast",
];

fuzz_target!(|data: &[u8]| {
    let Some((pick, message)) = data.split_first() else {
        return;
    };
    let format = FORMATS[usize::from(*pick) % FORMATS.len()];
    let ctx = ekiben_processors_fuzz::ctx();
    let config = serde_json::json!([{"unarchive": {"format": format}}]);
    let procs = ekiben_processors_fuzz::build(&ctx, &config).expect("every listed format builds");
    let out = ekiben_processors_fuzz::run(&ctx, &procs, message);
    // The org's in-flight quota (at least MAX_ARCHIVE_MEMBERS) caps the fan-out of one message.
    let cap = ctx
        .org
        .quotas
        .read()
        .map_or(0, |q| q.max_in_flight_messages);
    let cap = cap.max(ekiben_processors::archive::MAX_ARCHIVE_MEMBERS);
    assert!(out <= cap, "{format} gave {out} messages");
});
