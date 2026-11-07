// SPDX-License-Identifier: EUPL-1.2
//! The T-3912 edge cases at the trust boundary: each answers a typed error, never a panic.
#![allow(clippy::unwrap_used, clippy::panic)]
use ngsild_ql::geo::GeoQuery;
use ngsild_ql::scope::check_scope_q;
use ngsild_ql::{parse_q, percent_decode};
use std::collections::HashMap;

#[test]
fn an_empty_q_is_bad_request() {
    assert_eq!(parse_q("").unwrap_err().status(), 400);
}

#[test]
fn a_ten_thousand_character_q_is_too_complex() {
    let q = format!("speed=={}", "9".repeat(10_000));
    assert_eq!(parse_q(&q).unwrap_err().status(), 403);
}

/// `%FF` is not UTF-8: the decoded name carries U+FFFD, which no Attribute name may hold.
#[test]
fn invalid_utf8_in_an_attribute_name_is_bad_request() {
    let q = percent_decode(b"sp%FFeed==1");
    assert_eq!(parse_q(&q).unwrap_err().status(), 400);
}

#[test]
fn a_self_intersecting_polygon_is_evaluated_not_refused() {
    let params: HashMap<String, String> = [
        ("georel", "intersects"),
        ("geometry", "Polygon"),
        ("coordinates", "[[[0,0],[2,2],[2,0],[0,2],[0,0]]]"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert!(GeoQuery::from_params(&params).is_ok_and(|g| g.is_some()));
}

#[test]
fn a_scope_q_with_an_empty_segment_is_bad_request() {
    for q in ["/a,,/b", "/a;;/b", "//a", ""] {
        assert_eq!(check_scope_q(q).unwrap_err().status(), 400, "{q:?}");
    }
}
