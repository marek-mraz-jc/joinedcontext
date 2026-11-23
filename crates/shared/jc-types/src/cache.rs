//! The cache of a process (ARCH/ISO-04, PRF-01): one byte budget for the deployment, least
//! recently used out first, every entry with a time to live, every key with its Organization.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A cache key names the Organization it belongs to, so no key type works without one.
pub trait OrgKeyed: Clone + Eq + Hash {
    /// The Organization id type of the crate that owns the key.
    type Org: Copy + Eq + Hash;

    fn org_id(&self) -> Self::Org;

    /// What the key holds on the heap, in bytes.
    fn heap_bytes(&self) -> usize;
}

/// What the maps spend on one entry beside its three keys and its value.
const ENTRY_OVERHEAD: usize = 128;

struct Slot<V> {
    value: V,
    bytes: usize,
    used: u64,
    expires: (Instant, u64),
}

struct OrgUse<K> {
    bytes: usize,
    order: BTreeMap<u64, K>,
}

struct Inner<K: OrgKeyed, V> {
    entries: HashMap<K, Slot<V>>,
    expiry: BTreeMap<(Instant, u64), K>,
    orgs: HashMap<K::Org, OrgUse<K>>,
    bytes: usize,
    sequence: u64,
}

impl<K: OrgKeyed, V> Inner<K, V> {
    fn remove(&mut self, key: &K) -> Option<Slot<V>> {
        let slot = self.entries.remove(key)?;
        self.expiry.remove(&slot.expires);
        self.bytes = self.bytes.saturating_sub(slot.bytes);
        let org = key.org_id();
        if let Some(used) = self.orgs.get_mut(&org) {
            used.order.remove(&slot.used);
            used.bytes = used.bytes.saturating_sub(slot.bytes);
            if used.order.is_empty() {
                self.orgs.remove(&org);
            }
        }
        Some(slot)
    }

    fn expire(&mut self, now: Instant, gone: &mut Vec<Slot<V>>) {
        while let Some((&(at, _), key)) = self.expiry.first_key_value() {
            if at > now {
                break;
            }
            let key = key.clone();
            match self.remove(&key) {
                Some(slot) => gone.push(slot),
                None => break,
            }
        }
    }

    fn next(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// The least recently used key of the Organization that holds most once `incoming` is in.
    // ponytail: scans the Organizations with live entries on each eviction; an ordered share
    // index if thousands of them cache at once.
    fn victim(&self, inserting: K::Org, incoming: usize) -> Option<K> {
        let share = |org: &K::Org, used: &OrgUse<K>| {
            used.bytes + if *org == inserting { incoming } else { 0 }
        };
        // A tie goes against the Organization that inserts.
        let (_, used) = self
            .orgs
            .iter()
            .max_by_key(|(org, used)| (share(org, used), **org == inserting))?;
        used.order.first_key_value().map(|(_, key)| key.clone())
    }
}

/// A cache with a byte budget for the whole deployment. When it is full the Organization that
/// holds most gives up its least recently used entry, so one Organization's burst does not empty
/// another's share. Values are dropped outside the lock.
pub struct BoundedCache<K: OrgKeyed, V> {
    budget: usize,
    max_entry: usize,
    inner: Mutex<Inner<K, V>>,
}

impl<K: OrgKeyed, V> fmt::Debug for BoundedCache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedCache")
            .field("budget", &self.budget)
            .field("entries", &self.len())
            .field("bytes", &self.bytes())
            .finish()
    }
}

impl<K: OrgKeyed, V: Clone> BoundedCache<K, V> {
    /// The live value of `key`, now the most recently used one.
    pub fn get(&self, key: &K) -> Option<V> {
        let mut gone = Vec::new();
        let mut inner = self.lock();
        inner.expire(Instant::now(), &mut gone);
        let sequence = inner.next();
        let Inner { entries, orgs, .. } = &mut *inner;
        let found = entries.get_mut(key).map(|slot| {
            if let Some(used) = orgs.get_mut(&key.org_id())
                && let Some(stored) = used.order.remove(&slot.used)
            {
                used.order.insert(sequence, stored);
                slot.used = sequence;
            }
            slot.value.clone()
        });
        drop(inner);
        found
    }
}

impl<K: OrgKeyed, V> BoundedCache<K, V> {
    /// A cache of `budget` bytes. One entry takes half of it at most.
    #[must_use]
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            max_entry: budget / 2,
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                expiry: BTreeMap::new(),
                orgs: HashMap::new(),
                bytes: 0,
                sequence: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner<K, V>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `value`, which holds `value_bytes`, for `ttl`. A value over the entry cap is not
    /// stored, and it takes the older value of its key with it.
    pub fn insert(&self, key: K, value: V, value_bytes: usize, ttl: Duration) {
        let bytes = (size_of::<K>().saturating_add(key.heap_bytes()))
            .saturating_mul(3)
            .saturating_add(size_of::<Slot<V>>() + ENTRY_OVERHEAD)
            .saturating_add(value_bytes);
        let now = Instant::now();
        let mut gone = Vec::new();
        let mut inner = self.lock();
        inner.expire(now, &mut gone);
        gone.extend(inner.remove(&key));
        // A time to live past what the clock can count is not a time to live.
        if let Some(at) = now.checked_add(ttl)
            && !ttl.is_zero()
            && bytes <= self.max_entry
        {
            let org = key.org_id();
            while inner.bytes + bytes > self.budget {
                let Some(victim) = inner.victim(org, bytes) else {
                    break;
                };
                gone.extend(inner.remove(&victim));
            }
            let used = inner.next();
            let expires = (at, used);
            inner.expiry.insert(expires, key.clone());
            inner.bytes += bytes;
            let share = inner.orgs.entry(org).or_insert_with(|| OrgUse {
                bytes: 0,
                order: BTreeMap::new(),
            });
            share.bytes += bytes;
            share.order.insert(used, key.clone());
            inner.entries.insert(
                key,
                Slot {
                    value,
                    bytes,
                    used,
                    expires,
                },
            );
        }
        drop(inner);
    }

    /// Drops `key`'s entry, if any.
    pub fn remove(&self, key: &K) {
        let mut inner = self.lock();
        let gone = inner.remove(key);
        drop(inner);
        drop(gone);
    }

    /// Drops what has expired. The owner runs it on a timer, so an idle process gives it back.
    pub fn sweep(&self) {
        let mut gone = Vec::new();
        let mut inner = self.lock();
        inner.expire(Instant::now(), &mut gone);
        drop(inner);
    }

    /// What the entries hold, in bytes.
    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `org` has an entry.
    pub fn holds(&self, org: K::Org) -> bool {
        self.lock().orgs.contains_key(&org)
    }

    /// Every live entry of `org` and the bytes it holds.
    pub fn entries_of(&self, org: K::Org) -> Vec<(K, usize)> {
        let now = Instant::now();
        let inner = self.lock();
        let Some(used) = inner.orgs.get(&org) else {
            return Vec::new();
        };
        used.order
            .values()
            .filter_map(|key| {
                let slot = inner.entries.get(key)?;
                (slot.expires.0 > now).then(|| (key.clone(), slot.bytes))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrgId;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct Key(OrgId, u32);

    impl OrgKeyed for Key {
        type Org = OrgId;
        fn org_id(&self) -> OrgId {
            self.0
        }
        fn heap_bytes(&self) -> usize {
            0
        }
    }

    const TTL: Duration = Duration::from_secs(60);

    #[test]
    fn the_least_recently_used_entry_goes_first_and_the_budget_holds() {
        let cache = BoundedCache::new(10_000);
        let org = OrgId::mint();
        for n in 0..3 {
            cache.insert(Key(org, n), n, 2_500, TTL);
        }
        assert_eq!(cache.get(&Key(org, 0)), Some(0));
        cache.insert(Key(org, 3), 3, 2_500, TTL);
        assert_eq!(cache.get(&Key(org, 1)), None);
        assert_eq!(cache.get(&Key(org, 0)), Some(0));
        assert!(cache.bytes() <= 10_000);
        cache.insert(Key(org, 0), 9, 9_000, TTL);
        assert_eq!(cache.get(&Key(org, 0)), None, "over the entry cap");
    }

    #[test]
    fn a_removed_entry_gives_its_bytes_and_organization_back() {
        let cache = BoundedCache::new(10_000);
        let (org, other) = (OrgId::mint(), OrgId::mint());
        cache.insert(Key(org, 1), 1, 100, TTL);
        cache.insert(Key(other, 1), 2, 100, TTL);
        cache.remove(&Key(org, 1));
        cache.remove(&Key(org, 7));
        assert_eq!(cache.get(&Key(org, 1)), None);
        assert!(!cache.holds(org));
        assert_eq!((cache.len(), cache.get(&Key(other, 1))), (1, Some(2)));
    }

    #[test]
    fn expired_entries_leave_without_being_asked_for() {
        let cache = BoundedCache::new(1 << 20);
        let org = OrgId::mint();
        for n in 0..1_000 {
            cache.insert(Key(org, n), n, 16, Duration::from_millis(50));
        }
        assert!(!cache.is_empty());
        std::thread::sleep(Duration::from_millis(80));
        cache.sweep();
        assert_eq!((cache.len(), cache.bytes()), (0, 0));
        assert!(!cache.holds(org));
    }

    #[test]
    fn one_organizations_burst_leaves_anothers_entry() {
        let cache = BoundedCache::new(20_000);
        let (noisy, quiet) = (OrgId::mint(), OrgId::mint());
        cache.insert(Key(quiet, 0), 7, 1_000, TTL);
        for n in 0..200 {
            cache.insert(Key(noisy, n), n, 1_000, TTL);
        }
        assert_eq!(cache.get(&Key(quiet, 0)), Some(7));
        assert!(
            cache
                .entries_of(noisy)
                .iter()
                .all(|(key, _)| key.0 == noisy)
        );
        assert!(cache.bytes() <= 20_000);
    }
}
