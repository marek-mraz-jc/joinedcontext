//! The seam between the Portal's chrono instants and the jc-time instants jc-core and the
//! gateway moved to first (T-7252, T-7253; T-7233 plan). It goes when the Portal's own slices
//! move it off chrono (T-7254 and the `src/api` slice after it).

use chrono::{DateTime, Utc};
use jc_time::TimestampTz;
use std::time::SystemTime;

/// A chrono instant as jc-time's, to the nanosecond; past jiff's range (years -9999..=9999)
/// the nearest end of that range, which no clock or stored grant reaches.
pub(crate) fn tz(at: DateTime<Utc>) -> TimestampTz {
    TimestampTz(jc_time::from_system_time(at.into()))
}

/// A jc-time instant as chrono's, to the nanosecond.
pub(crate) fn utc(at: TimestampTz) -> DateTime<Utc> {
    SystemTime::from(at).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_crosses_the_seam_unchanged() {
        let at: DateTime<Utc> = "2026-10-25T01:30:00.987654321Z"
            .parse()
            .expect("an instant");
        assert_eq!(tz(at).to_rfc3339(), at.to_rfc3339());
        assert_eq!(utc(tz(at)), at);
    }
}
