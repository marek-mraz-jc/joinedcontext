//! Input: a stream config in YAML or JSON as posted to the API. Parsing answers a config or an
//! error; a parsed config goes through the lint, its metrics section is read, and the paging
//! cuts cover every pipeline processor.
#![no_main]

use ekiben_core::paging::page_cuts;
use ekiben_core::prometheus::{MetricMapping, MetricsConfig};
use ekiben_core::{Registry, StreamConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = StreamConfig::peek_org(text);
    let Ok(conf) = StreamConfig::from_yaml_or_json(text) else {
        return;
    };
    let _ = Registry::new().lint_stream_config(&conf, Vec::new());
    assert_eq!(page_cuts(&conf.pipeline.processors).len(), conf.pipeline.processors.len());
    if let Some(section) = conf.raw_source.get("metrics") {
        if let Ok(m) = MetricsConfig::from_json(section) {
            let _ = MetricMapping::new(&m.mapping);
        }
    }
});
