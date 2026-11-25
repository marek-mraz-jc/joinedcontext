//! The pipeline runner's metrics page the reconciler reads for each stream (T-7806): any text
//! and any stream name yield a sentence or none, and a sentence names a positive error count.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    // The first line names the stream, the rest is the page.
    let (stream, metrics) = text.split_once('\n').unwrap_or((text, ""));
    if let Some(said) = joinedcontext_portal::reconciler::daemon::failing(metrics, stream) {
        let count: u64 = said
            .split_once(": ")
            .and_then(|(_, rest)| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("the sentence names its error count");
        assert!(count > 0, "{said}");
    }
});
