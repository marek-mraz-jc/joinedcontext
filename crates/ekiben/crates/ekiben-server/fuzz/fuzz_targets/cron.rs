//! `schedule.cron` and `schedule.timezone` of a stream config: the first line is the expression,
//! the rest the zone. A parsed schedule's next run is strictly later than the instant asked about.
#![no_main]

use ekiben_server::CronSchedule;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (expr, tz) = text.split_once('\n').unwrap_or((text, ""));
    let Ok(schedule) = CronSchedule::parse(expr, Some(tz)) else {
        return;
    };
    let after = jiff::Timestamp::from_second(1_774_684_800).expect("2026-03-28");
    if let Some(next) = schedule.next_after(after) {
        assert!(next > after, "{expr:?} {tz:?}: {next} <= {after}");
    }
});
