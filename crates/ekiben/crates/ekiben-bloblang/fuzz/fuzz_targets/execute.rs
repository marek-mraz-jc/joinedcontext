//! Input: one selector byte, then a message's bytes. A fixed mapping over the methods that read
//! message text (JSON, YAML, CSV, XML, URLs, times, numbers, encodings) reads it or refuses it.
#![no_main]

use std::sync::OnceLock;

use ekiben_bloblang::executor::Executor;
use ekiben_bloblang::Environment;
use libfuzzer_sys::fuzz_target;

const MAPPINGS: &[&str] = &[
    "root = this",
    "root = content().string().parse_json()",
    "root = content().string().parse_yaml()",
    "root = content().string().parse_csv()",
    "root = content().string().parse_xml()",
    "root = content().string().parse_url()",
    "root = content().string().ts_parse(\"2006-01-02T15:04:05Z07:00\")",
    "root = content().string().ts_strptime(\"%Y-%m-%d %H:%M:%S\")",
    "root = content().string().number()",
    "root = content().string().int64()",
    "root = content().string().decode(\"base64\")",
    "root = content().string().decode(\"hex\")",
    "root = content().parse_msgpack()",
    "root = content().string().parse_duration()",
    "root = content().string().unescape_html().strip_html()",
    "root = content().string().format_json()",
];

fn compiled() -> &'static Vec<Executor> {
    static C: OnceLock<Vec<Executor>> = OnceLock::new();
    C.get_or_init(|| {
        let env = Environment::new();
        MAPPINGS
            .iter()
            .filter_map(|m| env.compile_mapping(m).ok())
            .collect()
    })
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let all = compiled();
    if let Some(exec) = all.get(usize::from(sel) % all.len().max(1)) {
        let _ = exec.execute_bytes(rest);
    }
});
