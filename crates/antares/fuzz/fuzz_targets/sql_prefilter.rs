//! T-7594: the antares-sql compilers `q_to_sql` does not reach: the temporal
//! q prefilter, the 4.11 instance window, the scopeQ index keys, the stored
//! location and the geohash. Whatever the bytes, nothing panics, every
//! placeholder a statement names has its bind (numbered densely from the
//! first), and the location written to `entities.location` is a GeoJSON
//! geometry. Mirrors the unit tests of `compile/qprefilter.rs`,
//! `compile/temporal.rs`, `compile/scope.rs` and `compile/geo.rs`.
#![no_main]
use antares_sql::compile::{
    geo::{extract_location, geohash},
    qprefilter::{compile_prefilter, prefilter_exact},
    scope::scope_keys,
    temporal::{column_range_bound, compile_instance_range, InstanceRange},
};
use libfuzzer_sys::fuzz_target;

const TIMERELS: [&str; 5] = ["any", "before", "after", "between", "during"];
const TIMEPROPERTIES: [&str; 5] = ["observedAt", "createdAt", "modifiedAt", "deletedAt", "x"];

/// The `$n` numbers `sql` names, in order of first use.
fn placeholders(sql: &str) -> Vec<usize> {
    let mut seen = Vec::new();
    let mut rest = sql;
    while let Some(i) = rest.find('$') {
        let after = &rest[i + 1..];
        let digits = after.len() - after.trim_start_matches(|d: char| d.is_ascii_digit()).len();
        assert!(digits > 0, "a bare $ in {sql}");
        let n: usize = after[..digits].parse().expect("placeholder number");
        if !seen.contains(&n) {
            seen.push(n);
        }
        rest = &after[digits..];
    }
    seen
}

/// Every placeholder lies in `first..first + binds` and every bind is named.
fn assert_dense(sql: &str, binds: usize, first: usize) {
    let mut named = placeholders(sql);
    named.sort_unstable();
    let want: Vec<usize> = (first..first + binds).collect();
    assert_eq!(named, want, "placeholders of {sql} for {binds} binds");
}

fuzz_target!(|data: &[u8]| {
    // a query string's cap: no allocation scales past it, whatever the input
    if data.len() > 4096 || data.len() < 2 {
        return;
    }
    let (sel, rest) = data.split_at(2);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let mut parts = text.splitn(5, '\n');
    let q = parts.next().unwrap_or("");
    let time_at = parts.next().unwrap_or("");
    let end = parts.next().unwrap_or("");
    let scope = parts.next().unwrap_or("");
    let doc = parts.next().unwrap_or("");

    let range = InstanceRange {
        timerel: TIMERELS[usize::from(sel[0]) % TIMERELS.len()],
        time_at,
        end_time_at: (sel[0] & 0x80 == 0).then_some(end),
        timeproperty: TIMEPROPERTIES[usize::from(sel[1]) % TIMEPROPERTIES.len()],
    };
    let first = usize::from(sel[1] >> 4) + 1;
    if let Some(c) = compile_instance_range(&range, "el", first) {
        assert_eq!(
            c.binds.first().map(String::as_str),
            Some(range.timeproperty)
        );
        assert_dense(&c.sql, c.binds.len(), first);
    }
    if let Some(sql) = column_range_bound(&range, "a", first) {
        assert!(
            placeholders(&sql)
                .iter()
                .all(|n| (first..first + 2).contains(n)),
            "{sql}"
        );
    }

    if let Ok(ast) = antares_ql::parse_q(q) {
        let expand = |t: &str| format!("https://example.org/{t}");
        let with_range = (sel[1] & 1 == 1).then_some(&range);
        let compiled = compile_prefilter(&ast, with_range, "entity", first, &expand);
        if let Some(c) = &compiled {
            assert_dense(&c.sql, c.binds.len(), first);
        }
        // exact is a property of a compiled filter, never of a refused one
        if prefilter_exact(&ast, with_range, &expand) {
            assert!(compiled.is_some(), "exact but not compiled: {q:?}");
        }
    }

    if let Some(keys) = scope_keys(scope) {
        for k in keys {
            for key in [k.exact, k.under].into_iter().flatten() {
                // a key is the literal head of one pattern, normalised to `/a/b` (`/a/b/` for
                // what lies under it): every segment comes from the request, none a wildcard
                assert!(
                    key.starts_with('/') && !key.contains('\0'),
                    "{key:?} of {scope:?}"
                );
                for seg in key.split('/').filter(|s| !s.is_empty()) {
                    assert!(
                        seg != "#" && seg != "+" && scope.contains(seg),
                        "{key:?} of {scope:?}"
                    );
                }
            }
        }
    }

    if let Ok(doc) = serde_json::from_str::<serde_json::Value>(doc) {
        if let Some(geojson) = extract_location(&doc) {
            let v: serde_json::Value = serde_json::from_str(&geojson).expect("stored GeoJSON");
            assert!(
                v.get("type").and_then(|t| t.as_str()).is_some(),
                "{geojson}"
            );
        }
    }

    let lon = f64::from(i16::from_le_bytes([sel[0], sel[1]])) / 100.0;
    let chars = usize::from(sel[1] % 13);
    let hash = geohash(lon, -lon / 2.0, chars);
    assert_eq!(hash.len(), chars);
    assert!(hash
        .bytes()
        .all(|b| b"0123456789bcdefghjkmnpqrstuvwxyz".contains(&b)));
});
