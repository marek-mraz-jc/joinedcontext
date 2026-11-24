//! The credential headers a run presents and a token's issuer, any bytes: what is read is never
//! empty, a run id the door takes is a plain name, and an issuer never ends in `/` (T-7795).
#![no_main]

use agent_proxy::{auth, delegation};
use axum::http::{HeaderMap, HeaderValue};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(3, |b| *b == b'\n');
    let run = parts.next().unwrap_or_default();
    let ticket = parts.next().unwrap_or_default();
    let bearer = parts.next().unwrap_or_default();
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (auth::RUN_HEADER, run),
        (auth::TICKET_HEADER, ticket),
        ("authorization", bearer),
    ] {
        if let Ok(value) = HeaderValue::from_bytes(value) {
            headers.insert(name, value);
        }
    }
    auth::fuzzing::credentials(&headers);
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    auth::fuzzing::names(&text(run), &text(ticket));
    delegation::fuzzing::issuer(&text(bearer));
});
