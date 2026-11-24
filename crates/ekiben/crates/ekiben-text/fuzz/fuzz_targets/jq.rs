//! Input: a jq query, a NUL byte, then the JSON message (`{"a":1}` without a NUL). The query runs on a
//! thread with the processor's stack, under its step, time, stack and size budget, as process_batch
//! runs it; it ends in values or an error.
#![no_main]

use ekiben_text::processor_jq::{compile, run, to_val, THREAD_STACK};
use libfuzzer_sys::fuzz_target;

fn eval(query: &str, message: &[u8]) {
    let Ok(filter) = compile(query) else {
        return;
    };
    let mut part = ekiben_core::Part::new(bytes::Bytes::copy_from_slice(message));
    let Ok(input) = part.as_structured() else {
        return;
    };
    let Ok(input) = to_val(&input) else {
        return;
    };
    let empty = ekiben_core::Part::from_json(serde_json::json!({}))
        .as_structured()
        .expect("an object");
    let meta = to_val(&empty).expect("an empty object converts");
    let _ = run(&filter, input, meta);
}

fuzz_target!(|data: &[u8]| {
    let (query, message) = match data.iter().position(|b| *b == 0) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &br#"{"a":1}"#[..]),
    };
    let Ok(query) = std::str::from_utf8(query) else {
        return;
    };
    let (query, message) = (query.to_string(), message.to_vec());
    std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(move || eval(&query, &message))
        .expect("thread starts")
        .join()
        .expect("jq ends without a panic");
});
