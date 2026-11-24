//! Input: one byte that picks a scanner, then the bytes of a file or stream, which that scanner
//! splits into messages. Every scanner must answer messages or an error; a decompressing one
//! stops at its cap; a byte scanner never answers more bytes than the stream held.
#![no_main]

use ekiben_io_file::inputs::Scanner;
use libfuzzer_sys::fuzz_target;

const SCANNERS: &[&str] = &[
    r#"{"lines": {}}"#,
    r#"{"lines": {"custom_delimiter": "\r\n--", "omit_empty": true, "max_buffer_size": 64}}"#,
    r#"{"csv": {}}"#,
    r#"{"csv": {"parse_header_row": false, "continue_on_error": true, "lazy_quotes": true, "custom_delimiter": ";"}}"#,
    r#"{"csv": {"expected_number_of_fields": 3, "continue_on_error": true}}"#,
    r#"{"json_documents": {}}"#,
    r#"{"xml_documents": {"operator": "to_json"}}"#,
    r#"{"tar": {}}"#,
    r#"{"chunker": {"size": 7}}"#,
    r#"{"to_the_end": {}}"#,
    r#"{"re_match": {"pattern": "(?m)^\\d+", "max_buffer_size": 256}}"#,
    r#"{"skip_bom": {"into": {"lines": {}}}}"#,
    r#"{"decompress": {"algorithm": "gzip", "into": {"lines": {}}}}"#,
    r#"{"decompress": {"algorithm": "zlib"}}"#,
    r#"{"decompress": {"algorithm": "flate"}}"#,
    r#"{"decompress": {"algorithm": "bzip2"}}"#,
    r#"{"decompress": {"algorithm": "lz4"}}"#,
    r#"{"decompress": {"algorithm": "snappy"}}"#,
    r#"{"decompress": {"algorithm": "zstd", "into": {"csv": {}}}}"#,
    r#"{"switch": [{"re_match_name": "\\.csv$", "scanner": {"csv": {}}}, {"scanner": {"json_documents": {}}}]}"#,
];

fuzz_target!(|data: &[u8]| {
    let Some((pick, raw)) = data.split_first() else {
        return;
    };
    let conf = SCANNERS[*pick as usize % SCANNERS.len()];
    let obj = serde_json::json!({ "scanner": serde_json::from_str::<serde_json::Value>(conf).expect("seed config") });
    let scanner = Scanner::from_config(obj.as_object().expect("object")).expect("seed scanner");
    let name = if pick % 2 == 0 {
        "upload/a.csv"
    } else {
        "upload/a.json"
    };
    if let Ok(parts) = scanner.scan(raw, name) {
        // a byte scanner hands on slices of the stream: never more bytes than it read. The
        // structured ones (csv rows repeat the header names, JSON escapes) and decompression grow.
        let structured = [
            "csv",
            "json_documents",
            "xml_documents",
            "decompress",
            "switch",
        ]
        .iter()
        .any(|k| conf.contains(k));
        if !structured {
            let total: usize = parts.iter().map(|p| p.as_bytes_ref().len()).sum();
            assert!(total <= raw.len(), "{conf}");
        }
    }
});
