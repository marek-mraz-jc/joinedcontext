//! What the stream deployer reads from the pipeline runner: a refusal body, and a stream's config
//! as the runner answers it. Any bytes yield a bounded reason, and a config always holds itself.
#![no_main]

use axum::http::StatusCode;
use joinedcontext_portal::reconciler::streams;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let body = String::from_utf8_lossy(data);
    let said = streams::runner_refusal(StatusCode::BAD_REQUEST, &body);
    assert!(
        said.len() <= 540,
        "a refusal is cut to its cap: {}",
        said.len()
    );
    let Ok(config) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    assert!(
        streams::held_as_rendered(&config, &config),
        "a config holds itself"
    );
    let empty = Value::Object(Default::default());
    let _ = streams::held_as_rendered(&config, &empty);
    let _ = streams::held_as_rendered(&empty, &config);
});
