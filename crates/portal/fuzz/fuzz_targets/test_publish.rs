//! An MQTT test-publish body against a broker spec, and topic filters against topics: a verdict
//! or a 422 naming the member, never a panic; a filter with no wildcard matches only itself.
#![no_main]

use jc_core::kinds::MqttBrokerSpec;
use joinedcontext_portal::api::mqtt_test_publish::{topic_matches, validate, TestPublish};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some((filter, topic)) = text.split_once('\n') {
        let matched = topic_matches(filter, topic);
        if !filter.contains(['+', '#']) {
            assert_eq!(matched, filter == topic, "{filter:?} on {topic:?}");
        }
        assert!(topic_matches("#", topic));
    }
    let Ok(Value::Array(pair)) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let [spec, body] = pair.as_slice() else {
        return;
    };
    let (Ok(spec), Ok(body)) = (
        serde_json::from_value::<MqttBrokerSpec>(spec.clone()),
        serde_json::from_value::<TestPublish>(body.clone()),
    ) else {
        return;
    };
    let _ = validate(&spec, &body);
});
