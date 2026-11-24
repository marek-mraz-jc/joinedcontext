//! Any body of `POST /invoke`: reading and checking it (`functions::prepare`) never panics for the
//! Portal's caller or the assistant's, a refusal is a 4xx/5xx problem, and the assistant is never
//! handed an endpoint, whatever `via` the body names (T-7774).
#![no_main]

use std::sync::{Arc, OnceLock};

use functions::AppState;
use jc_core::auth::token::Verifier;
use libfuzzer_sys::fuzz_target;
use tokio::sync::Semaphore;

fn state() -> &'static AppState {
    static STATE: OnceLock<AppState> = OnceLock::new();
    STATE.get_or_init(|| AppState {
        verifier: Arc::new(Verifier::new("https://idm.example/realms/fuzz")),
        audience: "jc-functions".to_owned(),
        caller: "portal-api".to_owned(),
        assistant_caller: Some("jc-assistant".to_owned()),
        gateway: "http://127.0.0.1:9".to_owned(),
        proxy: Some("http://127.0.0.1:9".to_owned()),
        http: reqwest::Client::new(),
        slots: Arc::new(Semaphore::new(functions::SLOTS)),
    })
}

fuzz_target!(|data: &[u8]| {
    for assistant in [false, true] {
        match functions::prepare(state(), data, assistant) {
            Ok(invocation) => {
                assert!(!(assistant && invocation.endpoint.is_some()));
            }
            Err(refusal) => assert!(refusal.status().as_u16() >= 400),
        }
    }
});
