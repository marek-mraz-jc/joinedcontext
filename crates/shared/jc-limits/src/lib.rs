#![forbid(unsafe_code)]
//! A weighted budget of one process, shared fairly between keys (an Organization): LIM-01,
//! LIM-03.
//!
//! A key alone may take the whole capacity with one permit. A key that holds something leaves
//! a quarter free, so a key that holds nothing finds room, and yields to a waiter holding less.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;

/// Waiters one budget holds before it refuses at once.
const MAX_WAITING: usize = 256;

/// Why a budget said no.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    /// More than the whole budget: waiting cannot help.
    #[error("the request is larger than the limit of {0} bytes")]
    TooLarge(u64),
    /// No room within the wait.
    #[error("the budget is in use")]
    Busy,
}

#[derive(Debug, Clone, Copy, Default)]
struct Share {
    held: u64,
    waiting: usize,
}

#[derive(Debug)]
struct State<K> {
    used: u64,
    waiting: usize,
    keys: HashMap<K, Share>,
}

#[derive(Debug)]
struct Inner<K> {
    capacity: u64,
    state: Mutex<State<K>>,
    changed: Notify,
}

impl<K: Copy + Eq + Hash> Inner<K> {
    fn lock(&self) -> MutexGuard<'_, State<K>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self, state: &mut State<K>, key: K, bytes: u64) -> bool {
        let held = state.keys.get(&key).map_or(0, |share| share.held);
        let free = self.capacity.saturating_sub(state.used);
        if bytes > free {
            return false;
        }
        if held > 0 {
            let headroom = free - bytes >= self.capacity / 4;
            let outwaited = state
                .keys
                .iter()
                .any(|(other, share)| *other != key && share.waiting > 0 && share.held < held);
            if !headroom || outwaited {
                return false;
            }
        }
        state.used += bytes;
        state.keys.entry(key).or_default().held += bytes;
        true
    }

    fn give_back(&self, key: K, bytes: u64) {
        let mut state = self.lock();
        state.used = state.used.saturating_sub(bytes);
        if let Some(share) = state.keys.get_mut(&key) {
            share.held = share.held.saturating_sub(bytes);
            if share.held == 0 && share.waiting == 0 {
                state.keys.remove(&key);
            }
        }
        drop(state);
        self.changed.notify_waiters();
    }
}

/// One budget of a process.
#[derive(Debug)]
pub struct FairBudget<K>(Arc<Inner<K>>);

impl<K> Clone for FairBudget<K> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<K: Copy + Eq + Hash> FairBudget<K> {
    #[must_use]
    pub fn new(capacity: u64) -> Self {
        Self(Arc::new(Inner {
            capacity: capacity.max(1),
            state: Mutex::new(State {
                used: 0,
                waiting: 0,
                keys: HashMap::new(),
            }),
            changed: Notify::new(),
        }))
    }

    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.0.capacity
    }

    /// Takes `bytes` for `key`, waiting at most `wait`.
    ///
    /// # Errors
    /// [`Refused`] when it is larger than the budget or found no room in time.
    pub async fn acquire(&self, key: K, bytes: u64, wait: Duration) -> Result<Permit<K>, Refused> {
        let bytes = bytes.max(1);
        if bytes > self.0.capacity {
            return Err(Refused::TooLarge(self.0.capacity));
        }
        let permit = || Permit {
            budget: Arc::clone(&self.0),
            key,
            bytes,
        };
        let waiter = {
            let mut state = self.0.lock();
            if self.0.take(&mut state, key, bytes) {
                return Ok(permit());
            }
            if state.waiting >= MAX_WAITING {
                return Err(Refused::Busy);
            }
            state.waiting += 1;
            state.keys.entry(key).or_default().waiting += 1;
            Waiter {
                inner: &*self.0,
                key,
            }
        };
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let taken = self.0.take(&mut self.0.lock(), key, bytes);
            if taken {
                // The permit holds the key's share before the waiter lets go of it.
                let permit = permit();
                drop(waiter);
                return Ok(permit);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(Refused::Busy);
            }
        }
    }
}

struct Waiter<'a, K: Copy + Eq + Hash> {
    inner: &'a Inner<K>,
    key: K,
}

impl<K: Copy + Eq + Hash> Drop for Waiter<'_, K> {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        state.waiting = state.waiting.saturating_sub(1);
        if let Some(share) = state.keys.get_mut(&self.key) {
            share.waiting = share.waiting.saturating_sub(1);
            if share.held == 0 && share.waiting == 0 {
                state.keys.remove(&self.key);
            }
        }
        drop(state);
        self.inner.changed.notify_waiters();
    }
}

/// Bytes of a budget, given back on drop.
#[derive(Debug)]
pub struct Permit<K: Copy + Eq + Hash> {
    budget: Arc<Inner<K>>,
    key: K,
    bytes: u64,
}

impl<K: Copy + Eq + Hash> Permit<K> {
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Grows the permit to `bytes` without waiting: a holder that waits for more of what it
    /// holds can wait for itself.
    ///
    /// # Errors
    /// [`Refused`] when there is no room now.
    pub fn grow_to(&mut self, bytes: u64) -> Result<(), Refused> {
        let more = bytes.saturating_sub(self.bytes);
        if more == 0 {
            return Ok(());
        }
        if bytes > self.budget.capacity {
            return Err(Refused::TooLarge(self.budget.capacity));
        }
        let mut state = self.budget.lock();
        let held = state.keys.get(&self.key).map_or(0, |share| share.held);
        // The only permit of its key may grow into the whole budget.
        let fits = if held == self.bytes {
            more <= self.budget.capacity.saturating_sub(state.used)
        } else {
            self.budget.take(&mut state, self.key, more)
        };
        if !fits {
            return Err(Refused::Busy);
        }
        if held == self.bytes {
            state.used += more;
            state.keys.entry(self.key).or_default().held += more;
        }
        self.bytes = bytes;
        Ok(())
    }
}

impl<K: Copy + Eq + Hash> Drop for Permit<K> {
    fn drop(&mut self) {
        self.budget.give_back(self.key, self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: Duration = Duration::from_millis(20);

    #[tokio::test]
    async fn a_key_alone_takes_all_and_a_busy_key_leaves_room_for_another() {
        let budget = FairBudget::new(100);
        let all = budget.acquire(1, 100, NOW).await.expect("a key alone");
        assert_eq!(budget.acquire(2, 1, NOW).await.err(), Some(Refused::Busy));
        drop(all);

        let mut held = Vec::new();
        while let Ok(permit) = budget.acquire(1, 10, NOW).await {
            held.push(permit);
        }
        assert_eq!(held.len(), 7, "a quarter stays free");
        let other = budget
            .acquire(2, 25, NOW)
            .await
            .expect("room for another key");
        assert_eq!(other.bytes(), 25);
    }

    #[tokio::test]
    async fn a_waiter_that_holds_less_goes_first() {
        let budget = FairBudget::new(100);
        let first = budget.acquire(1, 60, NOW).await.expect("first");
        let mine = budget.acquire(1, 10, NOW).await.expect("mine");
        let waiting = tokio::spawn({
            let budget = budget.clone();
            async move { budget.acquire(2, 40, Duration::from_secs(2)).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(budget.acquire(1, 1, NOW).await.err(), Some(Refused::Busy));
        drop(first);
        let theirs = waiting.await.expect("join").expect("the waiter");
        assert_eq!(theirs.bytes(), 40);
        drop(mine);
    }

    #[tokio::test]
    async fn growing_never_waits_and_every_byte_comes_back() {
        let budget = FairBudget::new(100);
        let mut mine = budget.acquire(1, 4, NOW).await.expect("mine");
        mine.grow_to(90).expect("alone, up to the whole budget");
        assert_eq!(mine.grow_to(101), Err(Refused::TooLarge(100)));
        let other = budget.acquire(2, 10, NOW).await.expect("other");
        assert_eq!(mine.grow_to(95), Err(Refused::Busy));
        assert_eq!(
            budget.acquire(3, 101, NOW).await.err(),
            Some(Refused::TooLarge(100))
        );
        drop((mine, other));
        let state = budget.0.lock();
        assert_eq!((state.used, state.waiting, state.keys.len()), (0, 0, 0));
    }

    #[tokio::test]
    async fn the_queue_is_bounded() {
        let budget = FairBudget::new(10);
        let _all = budget.acquire(1, 10, NOW).await.expect("all");
        budget.0.lock().waiting = MAX_WAITING;
        let started = tokio::time::Instant::now();
        let refused = budget.acquire(2, 1, Duration::from_secs(5)).await.err();
        assert_eq!(refused, Some(Refused::Busy));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
