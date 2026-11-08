#![forbid(unsafe_code)]
//! The per-organization query cache (R3, R7, §4.3 CVE-2022-23498): keys never shared across
//! organizations or permission scopes, expiry, the per-organization byte ceiling with oldest-first
//! eviction, and the statistics of the QueryCacheStats entity set.

use daiya_model::{FieldColumn, Frame, FrameSet, InstanceId, OrgId};
use daiya_query::{CacheKey, PublicCacheKey, QueryCache};
use serde_json::json;
use std::time::Duration;

const ORG: OrgId = OrgId::from_u128(1);
const OTHER: OrgId = OrgId::from_u128(2);
const INST: InstanceId = InstanceId::from_u128(9);
const TTL: Duration = Duration::from_secs(60);

fn key(org: OrgId, principals: &[&str], ds: &str, expr: &str) -> CacheKey {
    CacheKey::new(
        org,
        INST,
        principals,
        ds,
        &json!({ "expr": expr }),
        1_700_000_000_000,
        1_700_000_600_000,
        15_000,
    )
}

fn pkey(uid: &str, version: i32) -> PublicCacheKey {
    PublicCacheKey::new(ORG, INST, uid, version, 1, 0, 60_000, 30_000, 100)
}

/// A result whose JSON form is about `n` bytes.
fn sized(n: usize) -> Vec<FrameSet> {
    vec![FrameSet::new(
        "A",
        vec![Frame {
            name: "A".into(),
            fields: vec![FieldColumn::strings("s", vec![Some("x".repeat(n))])],
            ..Frame::default()
        }],
    )]
}

#[test]
fn keys_separate_organizations_scopes_queries_and_ranges() {
    let a = key(ORG, &["user:a", "group:x"], "prom", "up");
    // Principal order and JSON member order do not matter.
    assert_eq!(a, key(ORG, &["group:x", "user:a"], "prom", "up"));
    let reordered = CacheKey::new(
        ORG,
        INST,
        &["user:a", "group:x"],
        "prom",
        &json!({ "b": 1, "expr": "up" }),
        1_700_000_000_000,
        1_700_000_600_000,
        15_000,
    );
    let ordered = CacheKey::new(
        ORG,
        INST,
        &["user:a", "group:x"],
        "prom",
        &json!({ "expr": "up", "b": 1 }),
        1_700_000_000_000,
        1_700_000_600_000,
        15_000,
    );
    assert_eq!(reordered, ordered);
    assert_ne!(a, key(OTHER, &["user:a", "group:x"], "prom", "up"));
    assert_ne!(a, key(ORG, &["user:a"], "prom", "up"));
    assert_ne!(a, key(ORG, &["user:a", "group:x"], "prom2", "up"));
    assert_ne!(a, key(ORG, &["user:a", "group:x"], "prom", "down"));
    // The range is aligned to the interval, never below one second.
    let k = CacheKey::new(
        ORG,
        INST,
        &[],
        "p",
        &json!({}),
        1_700_000_007_123,
        1_700_000_014_999,
        10,
    );
    assert_eq!(
        (k.aligned_from_ms, k.aligned_to_ms),
        (1_700_000_007_000, 1_700_000_014_000)
    );
    let k = CacheKey::new(ORG, INST, &[], "p", &json!({}), -1, 29_999, 15_000);
    assert_eq!((k.aligned_from_ms, k.aligned_to_ms), (-15_000, 15_000));

    let p = PublicCacheKey::new(ORG, INST, "d", 3, 2, 61_000, 119_999, 30_000, 500);
    assert_eq!(
        (p.from_bucket_ms, p.to_bucket_ms, p.interval_ms),
        (60_000, 90_000, 30_000)
    );
    let p = PublicCacheKey::new(ORG, INST, "d", 3, 2, 1_500, 2_500, 0, 500);
    assert_eq!((p.from_bucket_ms, p.interval_ms), (1_000, 1_000));
    assert_ne!(
        pkey("d", 1),
        pkey("d", 2),
        "a new dashboard version is a new key"
    );
}

#[test]
fn hits_misses_and_statistics_per_datasource() {
    let cache = QueryCache::default();
    assert!(cache.stats_for_org(ORG, INST).is_empty());
    let k = key(ORG, &["user:a"], "prom", "up");
    assert!(cache.get(&k).is_none());
    cache.insert(k.clone(), sized(10), TTL);
    assert_eq!(cache.get(&k).unwrap(), sized(10));
    assert!(cache.get(&k).is_some());

    // Another organization never sees it, even with an otherwise equal key.
    let mut foreign = k.clone();
    foreign.org_id = OTHER;
    assert!(cache.get(&foreign).is_none());

    let stats = cache.stats_for_org(ORG, INST);
    assert_eq!(stats.len(), 1);
    let s = &stats[0];
    assert_eq!((s.hits, s.misses, s.entries), (2, 1, 1));
    assert_eq!(s.datasource_uid, "prom");
    assert_eq!(s.id, format!("{ORG}:prom"));
    assert!(s.bytes_used > 10);
    let other = cache.stats_for_org(OTHER, INST);
    assert_eq!(
        (other[0].hits, other[0].misses, other[0].entries),
        (0, 1, 0)
    );
    assert!(
        cache
            .stats_for_org(ORG, InstanceId::from_u128(8))
            .is_empty()
    );

    // Public entries count under their dashboard.
    let p = pkey("pub", 1);
    assert!(cache.get_public(&p).is_none());
    cache.insert_public(p.clone(), sized(5), TTL);
    assert_eq!(cache.get_public(&p).unwrap(), sized(5));
    let stats = cache.stats_for_org(ORG, INST);
    let public = stats
        .iter()
        .find(|s| s.datasource_uid == "public:pub")
        .unwrap();
    assert_eq!((public.hits, public.misses, public.entries), (1, 1, 1));
}

#[test]
fn expired_entries_are_misses_and_free_their_bytes() {
    let cache = QueryCache::new(2_000);
    let k = key(ORG, &[], "prom", "up");
    cache.insert(k.clone(), sized(600), Duration::ZERO);
    assert!(cache.get(&k).is_none());
    let p = pkey("pub", 1);
    cache.insert_public(p.clone(), sized(600), Duration::ZERO);
    assert!(cache.get_public(&p).is_none());
    // Both expired entries gave their bytes back: two fresh ones fit without evicting.
    let a = key(ORG, &[], "prom", "a");
    let b = key(ORG, &[], "prom", "b");
    cache.insert(a.clone(), sized(800), TTL);
    cache.insert(b.clone(), sized(800), TTL);
    assert!(cache.get(&a).is_some() && cache.get(&b).is_some());

    // A key that expired and came back is the newest, not the next to go.
    cache.insert(k.clone(), sized(10), Duration::ZERO);
    assert!(cache.get(&k).is_none());
    cache.insert(k.clone(), sized(10), TTL);
    let c = key(ORG, &[], "prom", "c");
    cache.insert(c.clone(), sized(800), TTL);
    assert!(cache.get(&k).is_some(), "the refreshed key stayed");
    assert!(cache.get(&a).is_none(), "the oldest went");
}

#[test]
fn the_ceiling_evicts_the_oldest_and_refuses_what_never_fits() {
    let cache = QueryCache::new(10); // floors at 1024 bytes
    let huge = key(ORG, &[], "prom", "huge");
    cache.insert(huge.clone(), sized(2_000), TTL);
    assert!(
        cache.get(&huge).is_none(),
        "larger than the ceiling: not stored"
    );
    cache.insert_public(pkey("huge", 1), sized(2_000), TTL);
    assert!(cache.get_public(&pkey("huge", 1)).is_none());

    let cache = QueryCache::new(2_000);
    let a = key(ORG, &[], "prom", "a");
    let b = key(ORG, &[], "prom", "b");
    let c = key(ORG, &[], "prom", "c");
    cache.insert(a.clone(), sized(700), TTL);
    cache.insert(b.clone(), sized(700), TTL);
    // Writing `a` again makes it the newest: `b` goes first.
    cache.insert(a.clone(), sized(700), TTL);
    cache.insert(c.clone(), sized(700), TTL);
    assert!(cache.get(&a).is_some(), "the rewritten entry stayed");
    assert!(cache.get(&b).is_none(), "the oldest went");
    assert!(cache.get(&c).is_some());

    // One organization filling its bucket never evicts another's.
    let foreign = key(OTHER, &[], "prom", "a");
    cache.insert(foreign.clone(), sized(700), TTL);
    for i in 0..5 {
        cache.insert(key(ORG, &[], "prom", &format!("n{i}")), sized(700), TTL);
    }
    assert!(cache.get(&foreign).is_some());

    // Public rewrite keeps its place too.
    let cache = QueryCache::new(2_000);
    cache.insert_public(pkey("p", 1), sized(700), TTL);
    cache.insert_public(pkey("p", 2), sized(700), TTL);
    cache.insert_public(pkey("p", 1), sized(700), TTL);
    cache.insert_public(pkey("p", 3), sized(700), TTL);
    assert!(cache.get_public(&pkey("p", 1)).is_some());
    assert!(cache.get_public(&pkey("p", 2)).is_none());
}

#[test]
fn public_and_authenticated_entries_share_one_ceiling() {
    let cache = QueryCache::new(2_000);
    let p = pkey("pub", 1);
    cache.insert_public(p.clone(), sized(700), TTL);
    let a = key(ORG, &[], "prom", "a");
    cache.insert(a.clone(), sized(700), TTL);
    // A third entry fits only when the oldest of either kind goes.
    let b = key(ORG, &[], "prom", "b");
    cache.insert(b.clone(), sized(700), TTL);
    assert!(cache.get(&a).is_none(), "authenticated entries go first");
    assert!(cache.get_public(&p).is_some());
    // With no authenticated entry left to evict, the public one makes room.
    let cache = QueryCache::new(2_000);
    cache.insert_public(p.clone(), sized(1_200), TTL);
    cache.insert(a.clone(), sized(1_200), TTL);
    assert!(cache.get(&a).is_some());
    assert!(cache.get_public(&p).is_none(), "the ceiling held");
    let p2 = pkey("pub", 2);
    cache.insert_public(p2.clone(), sized(1_200), TTL);
    assert!(cache.get_public(&p2).is_some());
    assert!(cache.get(&a).is_none());
}
