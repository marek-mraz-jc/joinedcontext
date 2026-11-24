//! What the catalogue wave reads from outside: a CKAN `package_search` page, a gateway problem
//! document and a broker multi-status answer. Any JSON yields datasets, a reason or nothing.
#![no_main]

use joinedcontext_portal::reconciler::catalog;
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(answer) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    if let Some((results, _count)) = catalog::ckan_page(&answer) {
        let _ = results
            .iter()
            .filter_map(catalog::Package::from_ckan)
            .count();
    }
    let said = catalog::explained(&answer);
    assert!(said.chars().count() <= 203, "a reason is cut to its cap");
    let _ = catalog::names_no_tenant(&answer);
    if let Some(errors) = answer.get("errors").and_then(Value::as_array) {
        let _ = catalog::refusals(errors);
    }
});
