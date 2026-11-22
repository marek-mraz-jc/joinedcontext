#![forbid(unsafe_code)]
//! T-7524: a burst of moving time ranges leaves the cache when its time to live ends.

use daiya_model::{InstanceId, OrgId};
use daiya_query::{CacheKey, QueryCache};
use serde_json::json;
use std::time::Duration;

#[test]
fn cache_bytes_fall_back_after_a_burst_of_moving_ranges() {
    let cache = QueryCache::new(64 * 1024 * 1024);
    let (org, instance) = (OrgId::generate(), InstanceId::generate());
    let frames: Vec<daiya_model::FrameSet> = vec![Default::default(); 8];
    for step in 0..2000 {
        let from = step * 15_000;
        let key = CacheKey::new(
            org,
            instance,
            &["viewer"],
            "ds",
            "r1",
            &json!("up"),
            from,
            from + 60_000,
            15_000,
        );
        cache.insert(key, frames.clone(), Duration::from_millis(20));
    }
    assert!(cache.entries_held() > 0);
    std::thread::sleep(Duration::from_millis(60));
    // No use of the cache after the burst: only what the timer runs.
    cache.sweep();
    let (held, bytes) = (cache.entries_held(), cache.bytes_held());
    println!("after the burst: {held} entries held, {bytes} bytes");
    assert_eq!((held, bytes), (0, 0));
    assert!(cache.stats_for_org(org, instance).is_empty());
}
