//! The store's reading of a 5.2.33 `idPattern` and of a stored `expiresAt`
//! (antares-store `filter`, T-7595): the pushdown helpers never panic, a
//! pattern they write as SQL is anchored, a split names its separator, and
//! a canonical DateTime stays canonical.
#![no_main]
use antares_store::filter::{
    canonical_datetime, expired_at, id_pattern_literal, id_pattern_required, id_pattern_segments,
    id_pattern_sql,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the API caps a query string far below this
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = id_pattern_literal(text);
    let _ = id_pattern_required(text);
    if let Some(sql) = id_pattern_sql(text) {
        assert!(sql.starts_with("^^") && sql.ends_with("$$"), "{sql}");
    }
    if let Some(s) = id_pattern_segments(text) {
        assert!(s.tail.starts_with(s.sep), "{:?} {:?}", s.tail, s.sep);
        assert!(s.sep.is_ascii());
    }
    let canonical = canonical_datetime(text);
    assert_eq!(canonical_datetime(&canonical), canonical);
    let _ = expired_at(&serde_json::json!({ "expiresAt": text }), "2026-08-08T12:00:00Z");
});
