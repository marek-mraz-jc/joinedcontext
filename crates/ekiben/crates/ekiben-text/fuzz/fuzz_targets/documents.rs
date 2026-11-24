//! Input: one selector byte, then a message. 0: msgpack to_json; 1: msgpack from_json and back;
//! otherwise a JMESPath query, a NUL byte and a JSON message. Each ends in a document or an error.
#![no_main]

use ekiben_core::ComponentConfig;
use ekiben_text::msgpack_processor::MsgpackProcessor;
use ekiben_text::processor_jmespath::JmespathProcessor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let part = || ekiben_core::Part::new(bytes::Bytes::copy_from_slice(rest));
    match sel {
        0 => {
            let _ = MsgpackProcessor::new("to_json")
                .expect("op")
                .process(&mut part());
        }
        1 => {
            let mut p = part();
            if MsgpackProcessor::new("from_json")
                .expect("op")
                .process(&mut p)
                .is_ok()
            {
                MsgpackProcessor::new("to_json")
                    .expect("op")
                    .process(&mut p)
                    .expect("msgpack written here decodes");
            }
        }
        _ => {
            let (query, message) = match rest.iter().position(|b| *b == 0) {
                Some(at) => (&rest[..at], &rest[at + 1..]),
                None => (rest, &b"{}"[..]),
            };
            let Ok(query) = std::str::from_utf8(query) else {
                return;
            };
            let conf = serde_json::json!({"jmespath": {"query": query}});
            let conf = ComponentConfig::from_json_value(&conf, "noop", None).expect("config");
            let Ok(p) = JmespathProcessor::from_config(&conf) else {
                return;
            };
            let mut m = ekiben_core::Part::new(bytes::Bytes::copy_from_slice(message));
            let _ = p.process(&mut m);
        }
    }
});
