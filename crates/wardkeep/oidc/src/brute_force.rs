#![forbid(unsafe_code)]
//! Brute-force protector matching RI (W7):
//! - `failureFactor`: 10 failures within `maxDeltaTimeSeconds` (43 200 s = 12 h) locks the account
//!   for `waitIncrementSeconds` (60 s); every further 10 add 60 s up to `maxFailureWaitSeconds` (900 s = 15 min).
//! - Retry faster than `quickLoginCheckMilliSeconds` (1000 ms) after the last failure locks for
//!   `minimumQuickLoginWaitSeconds` (60 s).
//! - `permanentLockout`: false (never permanent).
//! - Tracked both per person (`user:{id}`) and per source address (`ip:{addr}`). The address is the
//!   one the router resolved ([`crate::ClientIp`]); when it is unknown only the per-person counter
//!   runs, so failures never pile onto one shared placeholder address that would lock everyone out.

use chrono::{Duration, Utc};
use std::sync::Arc;
use wardkeep_core::{LoginFailureRecord, RealmConfig, Result, Store};

pub struct BruteForceProtector {
    store: Arc<dyn Store>,
}

impl BruteForceProtector {
    #[must_use]
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    pub async fn is_locked(
        &self,
        realm: &RealmConfig,
        user_id: Option<&str>,
        ip_address: Option<&str>,
    ) -> Result<bool> {
        if !realm.brute_force_protected {
            return Ok(false);
        }
        let now = Utc::now();
        if let Some(uid) = user_id
            && let Some(rec) = self
                .store
                .get_login_failure(&realm.id, &format!("user:{uid}"))
                .await?
            && rec.locked_until.is_some_and(|until| until > now)
        {
            return Ok(true);
        }
        if let Some(ip) = ip_address
            && let Some(rec) = self
                .store
                .get_login_failure(&realm.id, &format!("ip:{ip}"))
                .await?
            && rec.locked_until.is_some_and(|until| until > now)
        {
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn record_failure(
        &self,
        realm: &RealmConfig,
        user_id: Option<&str>,
        ip_address: Option<&str>,
    ) -> Result<Option<i64>> {
        if !realm.brute_force_protected {
            return Ok(None);
        }
        let ip_text = ip_address.unwrap_or("unknown");
        let mut lock_secs = None;
        if let Some(uid) = user_id {
            lock_secs = self
                .update_one(realm, &format!("user:{uid}"), ip_text)
                .await?;
        }
        let mut ip_lock = None;
        if let Some(ip) = ip_address {
            ip_lock = self.update_one(realm, &format!("ip:{ip}"), ip).await?;
        }
        Ok(lock_secs.or(ip_lock))
    }

    pub async fn record_success(&self, realm: &RealmConfig, user_id: &str) -> Result<()> {
        self.store
            .clear_login_failure(&realm.id, &format!("user:{user_id}"))
            .await
    }

    async fn update_one(
        &self,
        realm: &RealmConfig,
        target_key: &str,
        ip_address: &str,
    ) -> Result<Option<i64>> {
        let now = Utc::now();
        let now_ms = now.timestamp_millis();
        let factor = realm.failure_factor.max(1);
        let max_delta = realm.max_delta_time_seconds.max(60);

        let mut rec = match self.store.get_login_failure(&realm.id, target_key).await? {
            Some(existing)
                if now
                    .signed_duration_since(existing.window_started_at)
                    .num_seconds()
                    <= max_delta =>
            {
                existing
            }
            _ => LoginFailureRecord {
                id: uuid::Uuid::now_v7().to_string(),
                realm_id: realm.id.clone(),
                target_key: target_key.to_owned(),
                num_failures: 0,
                last_failure_ms: 0,
                last_ip_failure: ip_address.to_owned(),
                locked_until: None,
                window_started_at: now,
            },
        };

        let delta_ms = if rec.last_failure_ms > 0 {
            (now_ms - rec.last_failure_ms).max(0)
        } else {
            i64::MAX
        };

        rec.num_failures += 1;
        rec.last_failure_ms = now_ms;
        rec.last_ip_failure = ip_address.to_owned();

        let mut wait_secs = 0i64;
        if delta_ms < realm.quick_login_check_milli_seconds {
            wait_secs = realm.minimum_quick_login_wait_or_default();
        }
        if rec.num_failures >= factor && rec.num_failures % factor == 0 {
            let multiplier = i64::from(rec.num_failures / factor);
            let computed = (multiplier * realm.wait_increment_seconds.max(60))
                .min(realm.max_failure_wait_seconds.max(60));
            wait_secs = wait_secs.max(computed);
        }

        if wait_secs > 0 {
            rec.locked_until = Some(now + Duration::seconds(wait_secs));
        }
        self.store.upsert_login_failure(rec).await?;
        Ok((wait_secs > 0).then_some(wait_secs))
    }
}

trait RealmBruteForceExt {
    fn minimum_quick_login_wait_or_default(&self) -> i64;
}

impl RealmBruteForceExt for RealmConfig {
    fn minimum_quick_login_wait_or_default(&self) -> i64 {
        self.minimum_quick_login_wait_seconds.max(60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wardkeep_store_memory::MemoryStore;

    #[tokio::test]
    async fn locks_on_10th_failure_and_steps_up_to_15_min_never_permanent() {
        let store = Arc::new(MemoryStore::default());
        let realm = RealmConfig {
            quick_login_check_milli_seconds: 0, // disable quick login check to test factor=10
            ..RealmConfig::default()
        };
        store.upsert_realm(realm.clone()).await.unwrap();
        let bf = BruteForceProtector::new(store);

        for i in 1..=9 {
            let wait = bf
                .record_failure(&realm, Some("u1"), Some("198.51.100.1"))
                .await
                .unwrap();
            assert_eq!(wait, None, "unexpected lock on attempt {i}");
            assert!(
                !bf.is_locked(&realm, Some("u1"), Some("198.51.100.1"))
                    .await
                    .unwrap()
            );
        }

        // 10th failure -> 60 s lock
        let wait10 = bf
            .record_failure(&realm, Some("u1"), Some("198.51.100.1"))
            .await
            .unwrap();
        assert_eq!(wait10, Some(60));
        assert!(
            bf.is_locked(&realm, Some("u1"), Some("198.51.100.1"))
                .await
                .unwrap()
        );

        // 20th failure -> 120 s lock
        for _ in 11..=19 {
            let _ = bf
                .record_failure(&realm, Some("u1"), Some("198.51.100.1"))
                .await
                .unwrap();
        }
        let wait20 = bf
            .record_failure(&realm, Some("u1"), Some("198.51.100.1"))
            .await
            .unwrap();
        assert_eq!(wait20, Some(120));
    }

    #[tokio::test]
    async fn unknown_address_never_locks_other_people() {
        let store = Arc::new(MemoryStore::default());
        let realm = RealmConfig {
            quick_login_check_milli_seconds: 0,
            ..RealmConfig::default()
        };
        store.upsert_realm(realm.clone()).await.unwrap();
        let bf = BruteForceProtector::new(store);
        // An attacker hammering unknown names from an unresolved address.
        for _ in 0..30 {
            bf.record_failure(&realm, None, None).await.unwrap();
        }
        assert!(!bf.is_locked(&realm, Some("victim"), None).await.unwrap());
        // The per-address counter still locks a known address.
        for _ in 0..10 {
            bf.record_failure(&realm, None, Some("203.0.113.5"))
                .await
                .unwrap();
        }
        assert!(
            bf.is_locked(&realm, None, Some("203.0.113.5"))
                .await
                .unwrap()
        );
        assert!(
            !bf.is_locked(&realm, Some("victim"), Some("203.0.113.6"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn quick_retry_locks_for_minimum_wait_and_success_clears_person() {
        let store = Arc::new(MemoryStore::default());
        let realm = RealmConfig::default();
        store.upsert_realm(realm.clone()).await.unwrap();
        let bf = BruteForceProtector::new(store);
        assert_eq!(
            bf.record_failure(&realm, Some("u2"), None).await.unwrap(),
            None
        );
        // Second failure inside quickLoginCheckMilliSeconds (1000 ms) -> 60 s.
        assert_eq!(
            bf.record_failure(&realm, Some("u2"), None).await.unwrap(),
            Some(60)
        );
        assert!(bf.is_locked(&realm, Some("u2"), None).await.unwrap());
        bf.record_success(&realm, "u2").await.unwrap();
        assert!(!bf.is_locked(&realm, Some("u2"), None).await.unwrap());

        let off = RealmConfig {
            brute_force_protected: false,
            ..RealmConfig::default()
        };
        assert_eq!(
            bf.record_failure(&off, Some("u3"), None).await.unwrap(),
            None
        );
        assert!(!bf.is_locked(&off, Some("u3"), None).await.unwrap());
    }
}
