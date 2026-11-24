//! Input: one selector byte, then a JSON config for one processor (jq, jmespath, grok, msgpack,
//! parse_log). Every config builds the processor or is refused with a config error. grok's
//! pattern_paths is dropped: it names files, which a manifest's author picks, not the fuzzer.
#![no_main]

use ekiben_core::ComponentConfig;
use ekiben_text::msgpack_processor::MsgpackProcessor;
use ekiben_text::processor_grok::GrokProcessor;
use ekiben_text::processor_jmespath::JmespathProcessor;
use ekiben_text::processor_jq::JqProcessor;
use ekiben_text::processor_parse_log::ParseLogProcessor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(rest) else {
        return;
    };
    let ty = ["jq", "jmespath", "grok", "msgpack", "parse_log"][usize::from(sel) % 5];
    if let Some(m) = body.as_object_mut() {
        m.remove("pattern_paths");
    }
    let Ok(conf) = ComponentConfig::from_json_value(&serde_json::json!({ ty: body }), "noop", None)
    else {
        return;
    };
    let built = match ty {
        "jq" => JqProcessor::from_config(&conf).map(drop),
        "jmespath" => JmespathProcessor::from_config(&conf).map(drop),
        "grok" => GrokProcessor::from_config(&conf, None).map(drop),
        "msgpack" => MsgpackProcessor::from_config(&conf).map(drop),
        _ => ParseLogProcessor::from_config(&conf).map(drop),
    };
    let _ = built;
});
