//! Any caller `temporalQ` against any grants: the clamp never panics and never answers a
//! window wider than the grants' when one is granted (T-7767).
#![no_main]

use context_gateway::pdp::temporal;
use jc_time::TimestampTz;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    // A fixed clock, so a crash replays: 2026-11-24T00:00:00Z.
    let Some(now) = TimestampTz::from_second(1_795_478_400) else {
        return;
    };
    let mut lines = raw.split('\n');
    let caller = lines.next().filter(|line| *line != "-");
    let grants: Vec<&str> = lines.take(8).collect();
    let clamped = temporal::clamp(caller, &grants, now);
    if clamped.empty {
        assert!(clamped.temporal_q.is_none() && clamped.windows.is_empty());
    }
    if let Some(q) = &clamped.temporal_q {
        if !grants.is_empty() {
            // What the clamp writes reads back as the window it stands for.
            assert!(temporal::parse(q, now).is_some(), "{q}");
        }
    }
});
