//! The forge's `git-receive-pack` answer (pkt-lines with `report-status`): read strictly, never a
//! panic, and an answer is accepted only when it reported `unpack ok`.
#![no_main]

use joinedcontext_portal::forge::shako::receive_pack_status;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(refused) = receive_pack_status(data) {
        assert!(data.windows(9).any(|w| w == b"unpack ok"));
        for reason in refused {
            // A reason is read lossily: every run between replacement characters is from the answer.
            for run in reason.split('\u{fffd}').filter(|run| !run.is_empty()) {
                assert!(data.windows(run.len()).any(|w| w == run.as_bytes()));
            }
        }
    }
});
