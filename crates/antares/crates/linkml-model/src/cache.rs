// SPDX-License-Identifier: EUPL-1.2
//! One parse of a stored schema per revision and bounds, shared by every
//! surface of the pod (T-4354): the `OData` and `LinkML` surfaces asking for the
//! same revision read the same [`Model`].
//!
//! The cache is bounded by the bytes of the schemas it holds, as the JSON-LD
//! loader's is (`antares-jsonld` `loader.rs`), not by a count: 20 schemas of
//! 1 MiB weigh what 20 MiB do. A refused schema is kept with its error, so a
//! hostile one is not read again on every request.
//!
//! A revision is a 64-bit digest a Tenant can craft a collision for, so a hit
//! counts only when the bytes are the ones the entry was read from: one
//! Tenant's schema is never answered with another's model.
//!
//! ```
//! use linkml_model::cache;
//!
//! let json: std::sync::Arc<[u8]> =
//!     br#"{"id": "https://example.org/s", "name": "s", "classes": {"Room": {}}}"#[..].into();
//! let (first, fresh) = cache::read(7, &json, &linkml_model::SURFACE_LIMITS);
//! let (again, again_fresh) = cache::read(7, &json, &linkml_model::SURFACE_LIMITS);
//! assert!(fresh && !again_fresh);
//! assert!(std::sync::Arc::ptr_eq(&first.unwrap(), &again.unwrap()));
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use moka::sync::Cache;

use crate::{Error, Limits, Model};

/// The bytes of schema source the cache holds by default: 8 MiB, this
/// cache's share of the Antares broker's default 200 MiB memory budget
/// (T-5895); [`set_max_bytes`] sets another.
pub const MAX_BYTES: u64 = 8 << 20;

static CAPACITY: AtomicU64 = AtomicU64::new(MAX_BYTES);

/// Sets the bytes of schema source the cache holds. The cache is sized at
/// its first read, so a process calls this before it reads a schema.
pub fn set_max_bytes(bytes: u64) {
    CAPACITY.store(bytes, Ordering::Relaxed);
}

/// What one entry weighs at least, so tiny schemas cannot crowd the cache
/// past any sensible count.
const FLOOR: u32 = 4096;

/// A read: the model, or why the schema was refused.
pub type Read = Result<Arc<Model>, Arc<Error>>;

/// One read: the bytes it was made of (shared with the caller, not copied),
/// what they read as, and their weight.
#[derive(Clone)]
struct Entry {
    bytes: Arc<[u8]>,
    read: Read,
    weight: u32,
}

static CACHE: LazyLock<Cache<(u64, Limits), Entry>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(CAPACITY.load(Ordering::Relaxed))
        .weigher(|_key: &(u64, Limits), entry: &Entry| entry.weight)
        .build()
});

static PARSES: AtomicU64 = AtomicU64::new(0);

fn parse(bytes: &Arc<[u8]>, limits: &Limits) -> Entry {
    PARSES.fetch_add(1, Ordering::Relaxed);
    Entry {
        bytes: Arc::clone(bytes),
        read: Model::read(bytes, limits).map(Arc::new).map_err(Arc::new),
        weight: u32::try_from(bytes.len()).unwrap_or(u32::MAX).max(FLOOR),
    }
}

/// The schema `bytes` of `revision` read under `limits`, parsed once per
/// pair on this pod; the flag is `true` for the call that parsed it, so a
/// caller logs a refused schema once. `revision` names `bytes`: the caller
/// passes the digest its store gave them. Other bytes under a cached
/// revision (a collision) are parsed for this call and not kept.
pub fn read(revision: u64, bytes: &Arc<[u8]>, limits: &Limits) -> (Read, bool) {
    let entry = CACHE
        .entry((revision, *limits))
        .or_insert_with(|| parse(bytes, limits));
    let fresh = entry.is_fresh();
    let entry = entry.into_value();
    if Arc::ptr_eq(&entry.bytes, bytes) || *entry.bytes == **bytes {
        (entry.read, fresh)
    } else {
        (parse(bytes, limits).read, true)
    }
}

/// How many schemas this pod has parsed through [`read`] since it started.
#[must_use]
pub fn parses() -> u64 {
    PARSES.load(Ordering::Relaxed)
}

/// The bytes of schema source the cache holds now (after pending evictions).
#[must_use]
pub fn held_bytes() -> u64 {
    CACHE.run_pending_tasks();
    CACHE.weighted_size()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5895: the share a process sets is the capacity the cache takes.
    #[test]
    fn a_set_share_is_the_capacity() {
        set_max_bytes(MAX_BYTES);
        assert_eq!(CAPACITY.load(Ordering::Relaxed), MAX_BYTES);
    }

    fn schema(name: &str, pad: usize) -> Arc<[u8]> {
        let description = "x".repeat(pad);
        format!(r#"{{"id": "https://e.org/{name}", "name": "{name}", "classes": {{"Room": {{"description": "{description}"}}}}}}"#)
            .into_bytes()
            .into()
    }

    const BIG: Limits = Limits {
        max_bytes: 2 << 20,
        max_depth: 32,
        max_elements: 20_000,
    };

    #[test]
    fn a_revision_is_parsed_once_per_bounds() {
        let bytes = schema("once", 10);
        let before = parses();
        let (a, fresh) = read(1_001, &bytes, &BIG);
        let (b, again) = read(1_001, &bytes, &BIG);
        assert_eq!((fresh, again), (true, false));
        assert!(Arc::ptr_eq(&a.unwrap(), &b.unwrap()));
        // Other bounds are another read: a stricter caller is never answered
        // from a looser one's parse.
        let tight = Limits {
            max_bytes: 10,
            ..BIG
        };
        let (refused, fresh) = read(1_001, &bytes, &tight);
        assert_eq!((fresh, refused.is_err()), (true, true));
        assert!(parses() >= before + 2);
    }

    #[test]
    fn a_refused_schema_is_kept_with_its_error() {
        let bad: Arc<[u8]> = b"{not json"[..].into();
        let (first, fresh) = read(1_002, &bad, &BIG);
        let (second, again) = read(1_002, &bad, &BIG);
        assert_eq!((fresh, again), (true, false));
        assert_eq!((first.is_err(), second.is_err()), (true, true));
    }

    /// A Tenant crafting a schema whose revision collides with another's gets
    /// its own bytes read, never the other's model.
    #[test]
    fn other_bytes_under_a_cached_revision_are_read_as_themselves() {
        let theirs = schema("theirs", 10);
        let (cached, _) = read(1_003, &theirs, &BIG);
        // Equal bytes in another allocation are a hit.
        let copy: Arc<[u8]> = theirs.to_vec().into();
        let (same, again) = read(1_003, &copy, &BIG);
        assert!(Arc::ptr_eq(cached.as_ref().unwrap(), &same.unwrap()));
        assert!(!again);
        let mine = schema("mine", 10);
        let (read_mine, fresh) = read(1_003, &mine, &BIG);
        assert!(fresh);
        assert_eq!(read_mine.unwrap().schema().name, "mine");
        // The first entry stays: the colliding read was not kept.
        let (kept, _) = read(1_003, &theirs, &BIG);
        assert!(Arc::ptr_eq(&cached.unwrap(), &kept.unwrap()));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // 40 MiB parsed: past the weekly Miri job's 90 min (strict 39095897849, T-4564)
    fn forty_schemas_of_a_mebibyte_stay_under_the_byte_cap() {
        // 40 MiB asked for, 32 MiB held at most.
        for n in 0..40u64 {
            let bytes = schema(&format!("big{n}"), 1 << 20);
            let (read, _) = read(2_000 + n, &bytes, &BIG);
            assert!(read.is_ok());
        }
        assert!(held_bytes() <= MAX_BYTES, "{} > {MAX_BYTES}", held_bytes());
    }
}
