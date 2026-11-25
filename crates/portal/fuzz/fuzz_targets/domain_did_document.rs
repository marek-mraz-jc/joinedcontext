//! A domain's `did.json` and TXT strings, which the domain's own server and resolver answer: any
//! bytes read as a document name a host or none, and a TXT string proves a challenge only when it
//! is that challenge's record whole.
#![no_main]

use joinedcontext_portal::domain_verification::{did_names_host, txt_proves};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The document, then a NUL, then the challenge the TXT strings are held to.
    let (document, rest) = data.split_at(data.iter().position(|b| *b == 0).unwrap_or(data.len()));
    if let Ok(document) = serde_json::from_slice::<serde_json::Value>(document) {
        let _ = did_names_host(&document, "portal.hel.fi");
        let _ = did_names_host(&document, "");
    }
    let Ok(rest) = std::str::from_utf8(rest.get(1..).unwrap_or_default()) else {
        return;
    };
    let (challenge, records) = rest.split_once('\n').unwrap_or((rest, ""));
    let records: Vec<String> = records.lines().map(str::to_owned).collect();
    if txt_proves(&records, challenge) {
        let wanted = format!("jc-verify={challenge}");
        assert!(
            records.iter().any(|text| text.trim() == wanted),
            "{records:?} proves {challenge:?} without carrying it"
        );
    }
});
