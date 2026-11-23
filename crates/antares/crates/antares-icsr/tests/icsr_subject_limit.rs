// SPDX-License-Identifier: EUPL-1.2
//! A grant's subject names 1 to 16 subject headers: its own binary, since the
//! subject header list is read once per process (ANTARES_POLICY_SUBJECT_HEADERS).

use antares_icsr::ExportGrant;
use serde_json::{json, Map, Value};

fn grant(headers: usize) -> Vec<u8> {
    let subject: Map<String, Value> = (0..headers)
        .map(|i| (format!("x-s{i}"), json!("alice")))
        .collect();
    json!({"target": "b", "source": "a", "subject": subject})
        .to_string()
        .into_bytes()
}

#[test]
fn a_subject_of_sixteen_headers_is_the_limit() {
    let names: Vec<String> = (0..17).map(|i| format!("x-s{i}")).collect();
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", names.join(","));
    assert!(ExportGrant::parse(&grant(15)).is_ok());
    assert!(ExportGrant::parse(&grant(16)).is_ok());
    let over = ExportGrant::parse(&grant(17));
    assert!(
        over.as_ref().is_err_and(|e| e.contains("1 to 16")),
        "{over:?}"
    );
}
