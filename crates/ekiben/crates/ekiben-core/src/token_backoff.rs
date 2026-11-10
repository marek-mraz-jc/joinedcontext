//! When a client-credentials token may be fetched again after the token endpoint refused it.
//!
//! On jc-prod one project's pipelines asked the realm again at once after every 429: 1,262 token
//! requests in 5 minutes kept the edge's shared bucket empty, and the Portal's sign-ins failed
//! behind them. After a failed fetch the next one waits [`TokenBackoff::FIRST`], doubling up to
//! [`TokenBackoff::MAX`], half of each wait random so the clients of one realm spread out; a
//! longer `Retry-After` wins, up to [`TokenBackoff::MAX_RETRY_AFTER`]. A fetch in its wait is
//! refused without a request.

use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct TokenBackoff {
    failures: u32,
    until: Option<Instant>,
}

impl TokenBackoff {
    pub const FIRST: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(60);
    pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

    /// How long the next fetch must still wait, when it must.
    #[must_use]
    pub fn wait(&self) -> Option<Duration> {
        self.until
            .and_then(|until| until.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    /// Records a failed fetch; `retry_after` is the endpoint's own `Retry-After`.
    pub fn failed(&mut self, retry_after: Option<Duration>) {
        self.failures = self.failures.saturating_add(1);
        let doubled = Self::FIRST.saturating_mul(1 << (self.failures - 1).min(16));
        let base = doubled.min(Self::MAX);
        let fraction = (uuid::Uuid::new_v4().as_u128() % 1001) as u32;
        let wait = base / 2 + (base / 2) * fraction / 1000;
        let asked = retry_after.unwrap_or_default().min(Self::MAX_RETRY_AFTER);
        self.until = Some(Instant::now() + wait.max(asked));
    }

    /// Records a fetched token: the next failure waits [`Self::FIRST`] again.
    pub fn succeeded(&mut self) {
        *self = Self::default();
    }
}

/// A `Retry-After` value in seconds (the HTTP-date form is not read).
#[must_use]
pub fn retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_failure_waits_longer_with_jitter_and_a_success_resets() {
        let mut b = TokenBackoff::default();
        assert_eq!(b.wait(), None);
        let mut last = Duration::ZERO;
        for n in 0..8u32 {
            b.failed(None);
            let base = TokenBackoff::FIRST
                .saturating_mul(1 << n)
                .min(TokenBackoff::MAX);
            let wait = b.wait().expect("a failure waits");
            assert!(
                wait <= base && wait + Duration::from_millis(50) >= base / 2,
                "{n}: {wait:?}"
            );
            assert!(base >= last);
            last = base;
        }
        b.failed(None);
        assert!(b.wait().expect("capped") <= TokenBackoff::MAX);
        b.succeeded();
        assert_eq!(b.wait(), None);
    }

    #[test]
    fn a_longer_retry_after_wins_up_to_its_cap() {
        let mut b = TokenBackoff::default();
        b.failed(retry_after(Some(" 30 ")));
        assert!(b.wait().expect("wait") > Duration::from_secs(29));
        b.failed(retry_after(Some("86400")));
        assert!(b.wait().expect("wait") <= TokenBackoff::MAX_RETRY_AFTER);
        assert_eq!(retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(retry_after(None), None);
    }
}
