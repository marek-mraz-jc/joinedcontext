#![forbid(unsafe_code)]
//! `ekiben-testkit` — Shared test helpers, mockable clock, and registry builders for `ekiben`.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use ekiben_core::Registry;

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
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl MockClock {
    pub fn new(initial: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(initial)),
        }
    }

    pub fn from_ymd_hms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Self {
        let dt = Utc
            .with_ymd_and_hms(year, month, day, hour, min, sec)
            .single()
            .expect("valid UTC datetime");
        Self::new(dt)
    }

    pub fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }

    pub fn advance(&self, delta: Duration) -> DateTime<Utc> {
        let mut guard = self.now.lock().unwrap();
        *guard += delta;
        *guard
    }

    pub fn set(&self, dt: DateTime<Utc>) {
        *self.now.lock().unwrap() = dt;
    }
}
