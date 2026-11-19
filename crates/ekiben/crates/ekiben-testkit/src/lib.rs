#![forbid(unsafe_code)]
//! `ekiben-testkit` — Shared test helpers, mockable clock, and registry builders for `ekiben`.

use std::sync::{Arc, Mutex};

use ekiben_core::Registry;
use jiff::{SignedDuration, Timestamp};

/// Builds a `Registry` with all `ekiben-processors`, `ekiben-io-http`, `ekiben-io-file`,
/// and `ekiben-io-nats` components registered.
pub fn full_registry() -> Arc<Registry> {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_http::register_all(&reg);
    ekiben_io_file::register_all(&reg);
    ekiben_io_nats::register_all(&reg);
    ekiben_io_ngsild::register_all(&reg);
    reg
}

/// Mockable clock for deterministic cron and period scheduler testing (`Section 6`).
#[derive(Clone)]
pub struct MockClock {
    now: Arc<Mutex<Timestamp>>,
}

impl MockClock {
    pub fn new(initial: Timestamp) -> Self {
        Self {
            now: Arc::new(Mutex::new(initial)),
        }
    }

    pub fn from_ymd_hms(year: i16, month: i8, day: i8, hour: i8, min: i8, sec: i8) -> Self {
        let dt = jiff::civil::date(year, month, day)
            .at(hour, min, sec, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .expect("valid UTC datetime");
        Self::new(dt.timestamp())
    }

    pub fn now(&self) -> Timestamp {
        *self.now.lock().unwrap()
    }

    pub fn advance(&self, delta: SignedDuration) -> Timestamp {
        let mut guard = self.now.lock().unwrap();
        *guard = guard
            .checked_add(delta)
            .expect("a time within jiff's range");
        *guard
    }

    pub fn set(&self, dt: Timestamp) {
        *self.now.lock().unwrap() = dt;
    }
}
