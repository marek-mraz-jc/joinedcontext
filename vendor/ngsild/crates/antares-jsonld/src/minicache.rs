// SPDX-License-Identifier: EUPL-1.2
//! wasm32 only: a bounded map with moka's call surface. moka's clock
//! calls `std::time::Instant::now()` unconditionally, which panics on
//! wasm32-unknown-unknown — so the browser build swaps in this FIFO-evicting
//! cache behind the same methods and builder the loader uses. Bounded is the
//! requirement, by the same budget as moka's: an entry count, or with a
//! weigher the sum of the entries' weights (the loader's byte and term
//! budgets, T-3698). Recency-optimal eviction is not a requirement.
// FIFO eviction is deliberate; port moka's TinyLFU here only if a browser
// workload ever shows cache thrash.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Mutex;

type Weigher<K, V> = Box<dyn Fn(&K, &V) -> u32 + Send + Sync>;

/// Eviction policy, in full: entries leave in the order they were FIRST
/// inserted, until the new entry fits the budget. `get` is a pure read — it
/// does not refresh recency — and re-inserting a resident key overwrites its
/// value without moving it back in the queue. The held weight (one per entry
/// without a weigher) never exceeds the capacity the cache was built with,
/// including a capacity of zero; an entry heavier than the whole budget is
/// not held.
pub struct Cache<K, V> {
    cap: u64,
    weigher: Option<Weigher<K, V>>,
    inner: Mutex<Inner<K, V>>,
}

/// The entries with their weights, the insertion queue and the weight held.
type Inner<K, V> = (HashMap<K, (V, u64)>, VecDeque<K>, u64);

/// moka's `Cache::builder()` surface: a capacity and an optional weigher.
pub struct CacheBuilder<K, V> {
    cap: u64,
    weigher: Option<Weigher<K, V>>,
}

impl<K: Eq + Hash + Clone, V: Clone> CacheBuilder<K, V> {
    pub fn max_capacity(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }

    pub fn weigher(mut self, weigh: impl Fn(&K, &V) -> u32 + Send + Sync + 'static) -> Self {
        self.weigher = Some(Box::new(weigh));
        self
    }

    pub fn build(self) -> Cache<K, V> {
        Cache {
            cap: self.cap,
            weigher: self.weigher,
            inner: Mutex::new((HashMap::new(), VecDeque::new(), 0)),
        }
    }
}

impl<K: Eq + Hash + Clone, V: Clone> Cache<K, V> {
    pub fn new(cap: u64) -> Self {
        Self::builder().max_capacity(cap).build()
    }

    pub fn builder() -> CacheBuilder<K, V> {
        CacheBuilder {
            cap: 0,
            weigher: None,
        }
    }

    /// A cache must not turn a panic elsewhere into a permanently poisoned
    /// broker: nothing under this lock can leave the map inconsistent, so a
    /// poisoned guard is taken as-is.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<K, V>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn get<Q>(&self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.lock().0.get(key).map(|(v, _)| v.clone())
    }

    pub fn insert(&self, key: K, value: V) {
        let weight = self
            .weigher
            .as_ref()
            .map_or(1, |w| u64::from(w(&key, &value)));
        let mut g = self.lock();
        let (map, order, held) = &mut *g;
        if let Some((_, old)) = map.remove(&key) {
            *held = held.saturating_sub(old);
        }
        if weight > self.cap {
            order.retain(|k| k != &key);
            return;
        }
        while held.saturating_add(weight) > self.cap {
            let Some(at) = order.iter().position(|k| k != &key) else {
                break;
            };
            if let Some(old) = order.remove(at) {
                if let Some((_, w)) = map.remove(&old) {
                    *held = held.saturating_sub(w);
                }
            }
        }
        if !order.contains(&key) {
            order.push_back(key.clone());
        }
        *held = held.saturating_add(weight);
        map.insert(key, (value, weight));
    }

    pub fn invalidate<Q>(&self, key: &Q)
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let mut g = self.lock();
        let (map, order, held) = &mut *g;
        if let Some((_, w)) = map.remove(key) {
            *held = held.saturating_sub(w);
        }
        order.retain(|k| map.contains_key::<K>(k));
    }

    /// Snapshot of the resident entries, in moka's `iter()` item shape — the
    /// lock is released before the caller walks them, so a callback may touch
    /// this cache again.
    pub fn iter(&self) -> std::vec::IntoIter<(std::sync::Arc<K>, V)> {
        let g = self.lock();
        g.0.iter()
            .map(|(k, (v, _))| (std::sync::Arc::new(k.clone()), v.clone()))
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn run_pending_tasks(&self) {}

    pub fn entry_count(&self) -> u64 {
        self.lock().0.len() as u64
    }

    /// The weight held, moka's `weighted_size()`.
    #[cfg(test)]
    pub fn weighted_size(&self) -> u64 {
        self.lock().2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(cap: u64, n: u32) -> Cache<u32, u32> {
        let c = Cache::new(cap);
        for i in 0..n {
            c.insert(i, i);
        }
        c
    }

    /// The entry count is a hard ceiling: inserting far past the capacity
    /// leaves exactly `cap` entries.
    #[test]
    fn capacity_is_a_hard_bound() {
        for cap in [1u64, 2, 7, 64] {
            let c = filled(cap, 1000);
            assert_eq!(c.entry_count(), cap, "cap {cap}");
        }
    }

    /// A zero capacity means "hold nothing" — it must not degrade into an
    /// unbounded (or one-entry) map.
    #[test]
    fn zero_capacity_holds_nothing() {
        let c = filled(0, 100);
        assert_eq!(c.entry_count(), 0);
        assert_eq!(c.get(&99), None);
    }

    /// Eviction is FIFO by first insertion and therefore deterministic: after
    /// filling past the bound exactly the last `cap` keys survive.
    #[test]
    fn eviction_is_deterministic_fifo() {
        let c = filled(4, 6);
        assert_eq!(c.entry_count(), 4);
        for gone in [0u32, 1] {
            assert_eq!(c.get(&gone), None, "key {gone} should have been evicted");
        }
        for kept in [2u32, 3, 4, 5] {
            assert_eq!(c.get(&kept), Some(kept), "key {kept} should be resident");
        }
    }

    /// Documented policy: `get` is a pure read — it does NOT refresh recency.
    /// A key read on every lookup is still evicted in insertion order.
    #[test]
    fn get_does_not_refresh_recency() {
        let c = filled(4, 4);
        for _ in 0..10 {
            assert_eq!(c.get(&0), Some(0));
        }
        c.insert(4, 4);
        assert_eq!(
            c.get(&0),
            None,
            "get must not protect an entry from FIFO eviction"
        );
        assert_eq!(c.get(&1), Some(1));
    }

    /// Re-inserting an existing key overwrites the value, keeps the count flat
    /// and does NOT move the key to the back of the eviction queue.
    #[test]
    fn reinsert_updates_value_without_reordering() {
        let c = filled(3, 3);
        c.insert(0, 99);
        assert_eq!(c.entry_count(), 3);
        assert_eq!(c.get(&0), Some(99));
        c.insert(3, 3);
        assert_eq!(c.get(&0), None, "re-insert must not refresh eviction order");
        assert_eq!(c.entry_count(), 3);
    }

    /// `invalidate` drops the entry and its queue slot, so the freed room is
    /// reused before anything else is evicted.
    #[test]
    fn invalidate_frees_a_slot() {
        let c = filled(2, 2);
        c.invalidate(&0);
        assert_eq!(c.entry_count(), 1);
        c.insert(2, 2);
        assert_eq!(c.entry_count(), 2);
        assert_eq!(
            c.get(&1),
            Some(1),
            "the surviving entry must not be evicted early"
        );
        c.insert(3, 3);
        assert_eq!(c.entry_count(), 2);
        assert_eq!(c.get(&1), None);
        assert_eq!(c.get(&3), Some(3));
    }

    /// Invalidating an absent key is a no-op, and the bound still holds after.
    #[test]
    fn invalidate_absent_key_is_a_noop() {
        let c = filled(3, 3);
        c.invalidate(&42);
        assert_eq!(c.entry_count(), 3);
        for i in 0..3 {
            assert_eq!(c.get(&i), Some(i));
        }
    }

    /// `iter` yields every resident entry exactly once — it is what the
    /// caller filters to invalidate a subset, so a missed entry would leave a
    /// stale one behind. Evicted keys must not appear.
    #[test]
    fn iter_yields_every_resident_entry() {
        let c = filled(4, 6);
        let mut seen: Vec<u32> = c
            .iter()
            .map(|(k, v)| {
                assert_eq!(*k, v, "key and value must belong to the same entry");
                *k
            })
            .collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            vec![2, 3, 4, 5],
            "the evicted keys must not be listed"
        );
        c.invalidate(&3);
        let mut seen: Vec<u32> = c.iter().map(|(k, _)| *k).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![2, 4, 5]);
    }

    fn weighed(cap: u64) -> Cache<u32, u32> {
        Cache::builder()
            .max_capacity(cap)
            .weigher(|_k: &u32, v: &u32| *v)
            .build()
    }

    /// With a weigher the budget is the sum of the weights, as moka's is:
    /// a heavy entry evicts as many of the oldest as it needs, never more,
    /// and the held weight never passes the capacity (T-3698).
    #[test]
    fn a_weigher_bounds_the_held_weight() {
        let c = weighed(10);
        for (k, w) in [(0, 3), (1, 3), (2, 3)] {
            c.insert(k, w);
        }
        assert_eq!((c.entry_count(), c.weighted_size()), (3, 9));
        c.insert(3, 5);
        assert_eq!(c.get(&0), None, "the oldest made room");
        assert_eq!(c.get(&1), None, "and the next, since 3 + 3 + 5 > 10");
        assert_eq!((c.get(&2), c.get(&3)), (Some(3), Some(5)));
        assert_eq!(c.weighted_size(), 8);
        for k in 10..1000 {
            c.insert(k, k % 7 + 1);
            assert!(c.weighted_size() <= 10, "bound breached at {k}");
        }
    }

    /// An entry heavier than the whole budget is not held, and it evicts
    /// nothing: a hostile 5 MiB document cannot flush the warm ones.
    #[test]
    fn an_entry_over_the_budget_is_not_held() {
        let c = weighed(10);
        c.insert(0, 4);
        c.insert(1, 11);
        assert_eq!(c.get(&1), None);
        assert_eq!((c.get(&0), c.weighted_size()), (Some(4), 4));
        // a resident key re-inserted over the budget leaves too
        c.insert(0, 11);
        assert_eq!(
            (c.get(&0), c.entry_count(), c.weighted_size()),
            (None, 0, 0)
        );
        c.insert(2, 10);
        assert_eq!(c.get(&2), Some(10), "the queue holds no stale key");
    }

    /// Re-inserting a key with a new weight charges the new weight, and
    /// invalidating returns it to the budget.
    #[test]
    fn reinsert_and_invalidate_keep_the_weight_exact() {
        let c = weighed(10);
        c.insert(0, 2);
        c.insert(1, 2);
        c.insert(0, 7);
        assert_eq!((c.weighted_size(), c.entry_count()), (9, 2));
        c.insert(2, 3);
        assert_eq!(
            (c.get(&0), c.get(&1)),
            (None, Some(2)),
            "FIFO: the re-inserted 0 kept its place at the head of the queue"
        );
        assert_eq!(c.weighted_size(), 5);
        c.invalidate(&1);
        assert_eq!(c.weighted_size(), 3);
    }

    /// `run_pending_tasks` exists only to match moka's call surface; it must
    /// be a no-op that leaves the occupancy unchanged.
    #[test]
    fn run_pending_tasks_is_a_noop() {
        let c = filled(3, 5);
        c.run_pending_tasks();
        assert_eq!(c.entry_count(), 3);
        assert_eq!(c.get(&4), Some(4));
    }

    /// Borrowed lookups: a `String`-keyed cache is queried with `&str`, which
    /// is how the loader uses it.
    #[test]
    fn borrowed_key_lookup() {
        let c: Cache<String, u32> = Cache::new(2);
        c.insert("a".to_owned(), 1);
        assert_eq!(c.get("a"), Some(1));
        c.invalidate("a");
        assert_eq!(c.get("a"), None);
        assert_eq!(c.entry_count(), 0);
    }

    /// The bound is enforced under the lock, so it holds no matter how the
    /// inserts interleave. (Native-only: wasm32 has no threads — the browser
    /// build reaches the cache from a single thread.)
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn bound_holds_under_concurrent_inserts() {
        use std::sync::Arc;
        let c: Arc<Cache<u32, u32>> = Arc::new(Cache::new(16));
        let mut handles = Vec::new();
        for t in 0..8u32 {
            let c = Arc::clone(&c);
            handles.push(std::thread::spawn(move || {
                for i in 0..500u32 {
                    let k = t * 500 + i;
                    c.insert(k, k);
                    let _ = c.get(&k);
                    if k % 97 == 0 {
                        c.invalidate(&k);
                    }
                    assert!(c.entry_count() <= 16, "bound breached: {}", c.entry_count());
                }
            }));
        }
        for h in handles {
            h.join().expect("worker thread");
        }
        assert!(c.entry_count() <= 16);
    }
}
