//! One small in-process cache bounded by bytes, not entries: the least recently used entries
//! leave first once the held bytes pass the cap, and an entry older than the time to live is
//! never answered (T-4353).

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::time::{Duration, Instant};

/// A least-recently-used map whose entries weigh what the caller says they weigh.
pub struct ByteLru<K, V> {
    max_bytes: usize,
    ttl: Option<Duration>,
    bytes: usize,
    tick: u64,
    entries: HashMap<K, Entry<V>>,
    /// Use order: the oldest tick first.
    order: BTreeMap<u64, K>,
}

struct Entry<V> {
    value: V,
    bytes: usize,
    tick: u64,
    at: Instant,
}

impl<K: Eq + Hash + Clone, V: Clone> ByteLru<K, V> {
    pub fn new(max_bytes: usize, ttl: Option<Duration>) -> Self {
        ByteLru {
            max_bytes,
            ttl,
            bytes: 0,
            tick: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// The value under `key`, unless it expired; a hit makes it the most recently used.
    pub fn get<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let (held, entry) = self.entries.get_key_value(key)?;
        if self.ttl.is_some_and(|ttl| entry.at.elapsed() >= ttl) {
            self.remove(key);
            return None;
        }
        let held = held.clone();
        self.tick += 1;
        let entry = self.entries.get_mut(key)?;
        self.order.remove(&entry.tick);
        entry.tick = self.tick;
        self.order.insert(self.tick, held);
        Some(entry.value.clone())
    }

    /// Keeps `value` as weighing `bytes`; one heavier than the whole cap is not kept.
    pub fn insert(&mut self, key: K, value: V, bytes: usize) {
        self.remove(&key);
        if bytes > self.max_bytes {
            return;
        }
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                tick: self.tick,
                at: Instant::now(),
            },
        );
        self.bytes += bytes;
        while self.bytes > self.max_bytes {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(gone) = self.entries.remove(&oldest) {
                self.bytes -= gone.bytes;
            }
        }
    }

    fn remove<Q>(&mut self, key: &Q)
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        if let Some(gone) = self.entries.remove(key) {
            self.order.remove(&gone.tick);
            self.bytes -= gone.bytes;
        }
    }

    /// The bytes the held entries weigh.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_used_leave_first_once_the_bytes_pass_the_cap() {
        let mut cache = ByteLru::new(10, None);
        cache.insert("a", 1, 4);
        cache.insert("b", 2, 4);
        assert_eq!(cache.get(&"a"), Some(1));
        cache.insert("c", 3, 4);
        assert_eq!(cache.get(&"b"), None, "b was the least recently used");
        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get(&"c"), Some(3));
        assert_eq!(cache.bytes(), 8);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn a_replaced_entry_is_weighed_once_and_one_over_the_cap_is_not_kept() {
        let mut cache = ByteLru::new(10, None);
        cache.insert("a", 1, 6);
        cache.insert("a", 2, 3);
        assert_eq!((cache.get(&"a"), cache.bytes()), (Some(2), 3));
        cache.insert("a", 3, 11);
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
        cache.insert("z", 0, 0);
        assert_eq!(cache.get(&"z"), Some(0));
    }

    #[test]
    fn an_expired_entry_is_not_answered_and_frees_its_bytes() {
        let mut cache = ByteLru::new(10, Some(Duration::ZERO));
        cache.insert("a", 1, 5);
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.get(&"missing"), None);
    }
}
