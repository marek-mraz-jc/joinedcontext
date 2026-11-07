// SPDX-License-Identifier: EUPL-1.2
//! `multipart/mixed` batches (`odata_batch::parse_multipart`, threat model O10): never panics
//! under `Limits`, and what it accepts, written again in canonical framing, reads back to the
//! same members (T-3505).
#![no_main]
use libfuzzer_sys::fuzz_target;
use odata_batch::{parse_multipart, Limits, Request};

fn canonical(requests: &[Request]) -> Vec<u8> {
    let mut body = Vec::new();
    for request in requests {
        body.extend_from_slice(b"--b\r\nContent-Type: application/http\r\n");
        if let Some(id) = &request.id {
            body.extend_from_slice(format!("Content-ID: {id}\r\n").as_bytes());
        }
        body.extend_from_slice(format!("\r\n{} {} HTTP/1.1\r\n", request.method.as_str(), request.url).as_bytes());
        for (name, value) in &request.headers {
            body.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&request.body);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--b--\r\n");
    body
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits { max_body_bytes: 8192, max_requests: 16, max_part_header_bytes: 512, ..Limits::strict() };
    let Ok(batch) = parse_multipart(data, "b", &limits) else {
        return;
    };
    assert!(batch.requests.len() <= limits.max_requests);
    // Canonical framing adds `HTTP/1.1` and a blank after each colon: room for those bytes.
    let roomy = Limits { max_body_bytes: usize::MAX, max_part_header_bytes: usize::MAX, ..limits };
    let again = parse_multipart(&canonical(&batch.requests), "b", &roomy);
    assert_eq!(again, Ok(batch));
});
