#![forbid(unsafe_code)]
//! T-7524: a burst of moving time ranges leaves the cache when its time to live ends.

use daiya_model::{InstanceId, OrgId};
use daiya_query::{CacheKey, QueryCache};
use serde_json::json;
use std::time::Duration;

#[test]
fn cache_bytes_fall_back_after_a_burst_of_moving_ranges() {
    let cache = QueryCache::new(64 * 1024 * 1024);
    let (org, instance) = (OrgId::new(), InstanceId::new());
    let frames: Vec<daiya_model::FrameSet> = vec![Default::default(); 8];
    for step in 0..2000 {
        let from = step * 15_000;
        let key = CacheKey::new(
            org,
            instance,
            &["viewer"],
            "ds",
            &json!("up"),
            from,
            from + 60_000,
            15_000,
        );
        cache.insert(key, frames.clone(), Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(60));
    let later = CacheKey::new(org, instance, &["viewer"], "ds", &json!("up"), 0, 1, 1_000);
    cache.insert(later, Vec::new(), Duration::from_secs(60));
    let stats = cache.stats_for_org(org, instance);
    let (entries, bytes) = stats
        .iter()
        .fold((0, 0), |(e, b), s| (e + s.entries, b + s.bytes_used));
    let held = format!("{cache:?}").matches("expires_at").count();
    println!("after the burst: {held} entries held, stats say {entries} entries, {bytes} bytes");
    assert!(held <= 1, "{held} expired entries are still held");
}
