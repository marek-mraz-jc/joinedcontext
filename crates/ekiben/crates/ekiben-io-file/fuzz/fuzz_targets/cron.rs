//! Input: a generate input's `interval` read as a cron expression (`TZ=` prefix, descriptors,
//! `@every`). Parsing answers a schedule or robfig's error; a schedule's next activation is
//! later than now or none within five years, and is found without a hang.
#![no_main]

use ekiben_io_file::cron;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(expr) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(schedule) = cron::parse(expr) {
        let now = jiff::Timestamp::from_second(1_790_000_000).expect("now");
        if let Some(next) = schedule.next(now) {
            assert!(next > now, "{expr}");
        }
    }
});
