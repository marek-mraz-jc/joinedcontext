//! In-process locks by name: a check and the write it allows run one at a time per name, so two
//! requests never both pass a check the other's write would fail.
//!
//! ponytail: one Portal replica (owner 2026-10-23); a second replica needs a Postgres advisory
//! lock in place of this map.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::sync::OwnedMutexGuard;

/// Locks by name; a name nobody holds or waits on is dropped from the map.
#[derive(Default)]
pub struct Keyed(Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>);

impl Keyed {
    /// Waits for `key`'s turn; the turn ends when the guard drops.
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let gate = {
            let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            map.retain(|_, held| held.strong_count() > 0);
            match map.get(key).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(tokio::sync::Mutex::new(()));
                    map.insert(key.to_owned(), Arc::downgrade(&gate));
                    gate
                }
            }
        };
        gate.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_name_waits_another_does_not_and_a_free_name_is_dropped() {
        let keyed = Keyed::default();
        let held = keyed.lock("a").await;
        let other = keyed.lock("b").await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), keyed.lock("a"))
                .await
                .is_err(),
            "a second turn of one name waits"
        );
        drop((held, other));
        let again = keyed.lock("a").await;
        assert_eq!(keyed.0.lock().expect("map").len(), 1);
        drop(again);
    }
}
