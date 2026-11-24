//! A model call's body and the provider's stream, any bytes: every rewrite keeps its promise (the
//! run's model sent, the output cap within what is left, an object left alone off OpenRouter) and
//! the stream reader counts nothing down and holds no line past its cap (T-7795).
#![no_main]

use agent_proxy::routes::llm::fuzzing;
use libfuzzer_sys::fuzz_target;

const EFFORTS: [&str; 4] = ["low", "medium", "high", "max"];
const RESTS: [&str; 4] = [
    "v1/chat/completions",
    "v1/messages",
    "chat/completions",
    "messages",
];

fuzz_target!(|data: &[u8]| {
    let [knob, l0, l1, l2, body @ ..] = data else {
        return;
    };
    let left = u64::from_le_bytes([*l0, *l1, *l2, 0, 0, 0, 0, 0]) << (knob >> 5);
    let effort = EFFORTS[usize::from(knob & 3)];
    let rest = RESTS[usize::from((knob >> 2) & 3)];
    let model = if knob & 16 == 0 { "" } else { "m/run" };
    fuzzing::body(body, rest, effort, model, left, knob & 1 == 1);
    fuzzing::stream(body, usize::from(*l0));
});
