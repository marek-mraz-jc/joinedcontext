//! T-3464: q / scopeQ / geoQ text → SQL. Whatever the bytes, a compiled
//! statement is template text only (the request travels in its binds), and
//! nothing panics. Mirrors `crates/antares-sql/tests/sql_injection.rs`.
#![no_main]
use antares_ql::geo::{GeoSpec, Rel};
use antares_sql::compile::{geo::compile_geo, q::compile_q, scope::compile_scope_q};
use libfuzzer_sys::fuzz_target;

const TEMPLATE_WORDS: &[&str] = &[
    "entity", "scopes", "location", "AND", "OR", "NOT", "EXISTS", "SELECT", "FROM", "unnest",
    "AS", "s", "WHERE", "jsonpath", "geography", "ST_SetSRID", "ST_GeomFromGeoJSON", "ST_DWithin",
    "ST_Distance", "ST_Within", "ST_Contains", "ST_Intersects", "ST_Disjoint", "ST_Overlaps",
    "ST_Equals", "IS", "NULL", "location_ambiguous", "1", "4326", "location_cell", "ANY", "ARRAY", "ST_MakeEnvelope",
];

fn assert_template_only(sql: &str) {
    let mut rest = sql;
    while let Some(i) = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        let (word, tail) = rest.split_at(i);
        assert!(word.is_empty() || TEMPLATE_WORDS.contains(&word), "request text {word:?} in {sql}");
        let mut chars = tail.chars();
        let c = chars.next().unwrap_or(' ');
        if c == '$' {
            let after = chars.as_str();
            let digits = after.len() - after.trim_start_matches(|d: char| d.is_ascii_digit()).len();
            assert!(digits > 0, "a bare $ in {sql}");
            rest = &after[digits..];
            continue;
        }
        let ordering = c == '=' && sql[..sql.len() - tail.len()].ends_with(['>', '<']);
        // the geo cover's cell list, `= ANY(ARRAY[$n, ...])`, and nothing else
        let cover = (c == '=' && tail.starts_with("= ANY(ARRAY["))
            || (c == '[' && word == "ARRAY")
            || (c == ']' && tail.starts_with("])"));
        // the near box's bounding-box overlap, ` && ST_MakeEnvelope(`: one pair, spaced
        let overlap = c == '&'
            && (tail.starts_with("&& ") || (sql[..sql.len() - tail.len()].ends_with(" &") && tail.starts_with("& ")));
        assert!(" ()~@?><,:".contains(c) || ordering || cover || overlap, "request text {c:?} in {sql}");
        rest = chars.as_str();
    }
    assert!(rest.is_empty() || TEMPLATE_WORDS.contains(&rest), "request text {rest:?} in {sql}");
}

fuzz_target!(|data: &[u8]| {
    // a cap like the API's own on a query string: an allocation never scales
    // past it, whatever the input
    if data.len() > 4096 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else { return };
    let mut parts = text.splitn(3, '\n');
    let q = parts.next().unwrap_or("");
    let scope = parts.next().unwrap_or("");
    let coordinates = parts.next().unwrap_or("");

    if let Ok(ast) = antares_ql::parse_q(q) {
        if let Some(c) = compile_q(&ast, "entity", 1, &|t| format!("https://example.org/{t}")) {
            assert_template_only(&c.sql);
        }
    }
    if let Some(c) = compile_scope_q(scope, "scopes", 1) {
        assert_template_only(&c.sql);
    }
    let coordinates = serde_json::from_str(coordinates).unwrap_or(serde_json::Value::String(coordinates.to_owned()));
    for rel in [Rel::Within, Rel::Near { max: Some(10.0), min: None }] {
        let spec = GeoSpec { rel, geometry: "Point", coordinates: &coordinates, geoproperty_iri: "" };
        if let Some(c) = compile_geo(&spec, "location", 1) {
            assert_template_only(&c.sql);
        }
    }
});
