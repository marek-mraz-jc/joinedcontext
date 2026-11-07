// SPDX-License-Identifier: EUPL-1.2
//! JSON batches (`odata_batch::parse_json`, threat model O10, O11): never panics under
//! `Limits`, and what it accepts, written again as a JSON batch, reads back to the same members
//! (T-3505).
#![no_main]
use libfuzzer_sys::fuzz_target;
use odata_batch::{parse_json, Limits, Request};
use serde_json::{json, Map, Value};

fn canonical(requests: &[Request]) -> Vec<u8> {
    let members: Vec<Value> = requests
        .iter()
        .map(|request| {
            let mut member = json!({
                "id": request.id,
                "method": request.method.as_str(),
                "url": request.url,
                "headers": request.headers.iter().cloned().map(|(n, v)| (n, Value::from(v))).collect::<Map<_, _>>(),
                "dependsOn": request.depends_on.iter().map(|&at| requests[at].id.clone()).collect::<Vec<_>>(),
            });
            if !request.body.is_empty() {
                let json = request
                    .headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
                    .is_none_or(|(_, t)| {
                        // The crate's order: a JSON essence first (`+json` wins over `text/`).
                        let essence = t.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
                        essence == "application/json" || essence.ends_with("+json")
                    });
                member["body"] = if json {
                    serde_json::from_slice(&request.body).expect("a JSON body reads back")
                } else {
                    Value::from(String::from_utf8(request.body.clone()).expect("a text body is UTF-8"))
                };
            }
            member
        })
        .collect();
    json!({ "requests": members }).to_string().into_bytes()
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits { max_body_bytes: 8192, max_requests: 16, ..Limits::strict() };
    let Ok(batch) = parse_json(data, &limits) else {
        return;
    };
    assert!(batch.requests.len() <= limits.max_requests);
    let roomy = Limits { max_body_bytes: usize::MAX, ..limits };
    assert_eq!(parse_json(&canonical(&batch.requests), &roomy), Ok(batch));
});
