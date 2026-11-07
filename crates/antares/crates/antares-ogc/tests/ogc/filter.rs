// SPDX-License-Identifier: EUPL-1.2
//! Part 3 and CQL2 (mapping rows F1, F2, F4 to F16, T-3745): queryables, `filter` in text and
//! JSON, the queryables as parameters, each conformance class answered over the fixture's Cars,
//! and the filter only ever narrowing what the view and its Policy admit.

use crate::common::{fixture, get, href, ids, local, narrowed, Fixture};
use axum::http::StatusCode;
use serde_json::{json, Value};
use std::fmt::Write as _;

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";

/// Percent-encodes a filter for a query string.
pub(crate) fn enc(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// The Car numbers `filter` selects in the `city` view, every page followed.
async fn cars(f: &Fixture, query: &str) -> Result<Vec<String>, String> {
    let mut uri = format!("{CARS}?{query}");
    let mut seen = Vec::new();
    for _ in 0..20 {
        let a = get(&f.app, &uri).await;
        if a.status != StatusCode::OK {
            return Err(format!("{uri}: {} {}", a.status, a.body));
        }
        seen.extend(
            ids(&a.body)
                .into_iter()
                .map(|id| id.trim_start_matches("urn:ngsi-ld:Car:").to_owned()),
        );
        if let Some(next) = href(&a.body, "next") {
            uri = local(&next);
        } else {
            seen.sort();
            return Ok(seen);
        }
    }
    Err(format!("{uri}: the pages do not end"))
}

async fn filtered(f: &Fixture, filter: &str) -> Result<Vec<String>, String> {
    cars(f, &format!("filter={}", enc(filter))).await
}

async fn check(f: &Fixture, cases: &[(&str, &[&str])]) -> Result<(), String> {
    for (filter, expected) in cases {
        let got = filtered(f, filter).await?;
        if got != *expected {
            return Err(format!("{filter}: {got:?}, expected {expected:?}"));
        }
    }
    Ok(())
}

async fn status(f: &Fixture, query: &str) -> (StatusCode, Value) {
    let a = get(&f.app, &format!("{CARS}?{query}")).await;
    (a.status, a.body)
}

#[tokio::test]
async fn queryables_are_the_shown_attributes() -> Result<(), String> {
    let f = fixture().await?;
    let base = "/x/views/city/ogc/features";
    let a = get(&f.app, &format!("{base}/collections/Car/queryables")).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(
        a.headers.get("content-type").and_then(|v| v.to_str().ok()),
        Some("application/schema+json")
    );
    assert_eq!(a.body.get("additionalProperties"), Some(&json!(false)));
    let names: Vec<&str> = a
        .body
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    for shown in ["id", "geometry", "speed", "seen", "colour", "tags", "at"] {
        assert!(names.contains(&shown), "{shown} in {names:?}");
    }
    assert!(!names.contains(&"secret"), "{names:?}");
    let c = get(&f.app, &format!("{base}/collections/Car")).await;
    let rel = "http://www.opengis.net/def/rel/ogc/1.0/queryables";
    assert!(href(&c.body, rel).is_some_and(|h| h.ends_with("/collections/Car/queryables")));
    let conf = get(&f.app, &format!("{base}/conformance"))
        .await
        .body
        .to_string();
    for class in [
        "ogcapi-features-3/1.0/conf/queryables",
        "ogcapi-features-3/1.0/conf/filter",
        "ogcapi-features-3/1.0/conf/features-filter",
        "cql2/1.0/conf/basic-cql2",
        "cql2/1.0/conf/cql2-json",
    ] {
        assert!(conf.contains(class), "{class}");
    }
    Ok(())
}

#[tokio::test]
async fn functions_list_is_empty_and_unknown_is_400() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, "/x/views/city/ogc/features/functions").await;
    assert_eq!(
        (a.status, a.body),
        (StatusCode::OK, json!({"functions": []}))
    );
    let (s, _) = status(
        &f,
        &format!("filter={}", enc("Buffer(geometry, 10) IS NULL")),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn basic_cql2_pushed_to_q() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("speed > 50", &["2", "4"]),
            ("speed >= 60 AND speed < 70", &["2"]),
            ("speed = 10 OR speed = 70", &["1", "4"]),
            ("speed <> 10", &["2", "3", "4"]),
            ("colour IS NULL", &["1", "3", "4"]),
            ("colour IS NOT NULL", &["2"]),
            ("colour = 'red'", &["2"]),
            ("TRUE", &["1", "2", "3", "4"]),
        ],
    )
    .await?;
    let json = enc(
        r#"{"op": "and", "args": [{"op": ">", "args": [{"property": "speed"}, 50]}, {"op": "<", "args": [{"property": "speed"}, 65]}]}"#,
    );
    assert_eq!(
        cars(&f, &format!("filter-lang=cql2-json&filter={json}")).await?,
        ["2"]
    );
    Ok(())
}

#[tokio::test]
async fn not_is_residual() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("NOT speed > 50", &["1", "3"]),
            ("NOT (speed > 50 OR speed = 10)", &["3"]),
            // Cars 1 and 3 have no colour: `false OR unknown` is unknown, and so is its NOT.
            ("NOT (speed > 50 OR colour = 'red')", &[]),
            // Car 1, 3 and 4 have no colour: unknown under NOT stays unknown.
            ("NOT colour = 'red'", &[]),
            ("speed > 20 AND NOT speed > 65", &["2", "3"]),
            ("NOT speed > 50 AND NOT speed < 20", &["3"]),
        ],
    )
    .await
}

#[tokio::test]
async fn like_between_in_pushed() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("colour LIKE 'r%'", &["2"]),
            ("colour LIKE '_e_'", &["2"]),
            ("colour LIKE 'R%'", &[]),
            ("speed BETWEEN 20 AND 65", &["2", "3"]),
            ("speed IN (10, 30)", &["1", "3"]),
            ("colour IN ('blue', 'red')", &["2"]),
        ],
    )
    .await
}

#[tokio::test]
async fn casei_pushed_as_regex_flag() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("CASEI(colour) = CASEI('RED')", &["2"]),
            ("CASEI(colour) LIKE CASEI('R%')", &["2"]),
            ("CASEI(colour) <> CASEI('RED')", &[]),
        ],
    )
    .await
}

#[tokio::test]
async fn accenti_is_residual() -> Result<(), String> {
    let f = fixture().await?;
    let stops = "/x/views/city/ogc/features/collections/Stop/items";
    for (filter, expected) in [
        ("ACCENTI(name) = ACCENTI('Hlavna')", 1),
        ("ACCENTI(CASEI(name)) LIKE ACCENTI(CASEI('hlav%'))", 1),
        ("name = 'Hlavna'", 0),
    ] {
        let a = get(&f.app, &format!("{stops}?filter={}", enc(filter))).await;
        assert_eq!(a.status, StatusCode::OK, "{filter}: {}", a.body);
        assert_eq!(ids(&a.body).len(), expected, "{filter}");
    }
    Ok(())
}

#[tokio::test]
async fn s_intersects_pushed_to_geoq() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("S_INTERSECTS(geometry, BBOX(17, 48, 18, 49))", &["1"]),
            ("S_INTERSECTS(geometry, POINT(17.1 48.1))", &["1"]),
            // Across the antimeridian: two polygons.
            (
                "S_INTERSECTS(geometry, BBOX(170, -10, -170, 10))",
                &["2", "4"],
            ),
        ],
    )
    .await
}

#[tokio::test]
async fn spatial_functions_pushed_or_residual() -> Result<(), String> {
    let f = fixture().await?;
    let east = "POLYGON((170 -10, 180 -10, 180 10, 170 10, 170 -10))";
    check(
        &f,
        &[
            (&format!("S_WITHIN(geometry, {east})"), &["2"]),
            (&format!("S_CONTAINS({east}, geometry)"), &["2"]),
            // Car 3 has no geometry: unknown, so not disjoint either.
            (&format!("S_DISJOINT(geometry, {east})"), &["1", "4"]),
            ("S_EQUALS(geometry, POINT(17.1 48.1))", &["1"]),
            (&format!("S_TOUCHES(geometry, {east})"), &[]),
            ("S_CROSSES(geometry, LINESTRING(17 48, 18 49))", &[]),
            // Two spatial conjuncts: one geoQ, one residual.
            (
                &format!("S_WITHIN(geometry, {east}) AND S_DISJOINT(geometry, POINT(0 0))"),
                &["2"],
            ),
            (
                &format!("S_WITHIN(geometry, {east}) OR speed = 10"),
                &["1", "2"],
            ),
        ],
    )
    .await
}

#[tokio::test]
async fn filter_and_bbox_and_datetime_are_anded() -> Result<(), String> {
    let f = fixture().await?;
    let speed = enc("speed > 50");
    assert_eq!(
        cars(&f, &format!("filter={speed}&bbox=170,-10,180,10")).await?,
        ["2"]
    );
    // Features without a geometry pass a bbox (C12) and still meet the filter.
    assert_eq!(
        cars(
            &f,
            &format!("filter={}&bbox=170,-10,180,10", enc("speed < 50"))
        )
        .await?,
        ["3"]
    );
    // A spatial filter beside bbox is the residual's: the geoQ is the bbox's.
    let disjoint = enc("S_DISJOINT(geometry, POINT(0 0))");
    assert_eq!(
        cars(&f, &format!("filter={disjoint}&bbox=170,-10,180,10")).await?,
        ["2"]
    );
    assert_eq!(
        cars(
            &f,
            &format!("filter={speed}&datetime=2026-05-01T00:00:00Z/..")
        )
        .await?,
        ["2", "4"]
    );
    assert_eq!(
        cars(&f, &format!("filter={speed}&ngsild.q=speed%3C65")).await?,
        ["2"]
    );
    Ok(())
}

/// F-ANT5-012: an `ngsild.q` that does not parse alone would close its group and OR past the
/// filter's conjunct; it is refused whether or not the collection has an output filter.
#[tokio::test]
async fn ngsild_q_cannot_escape_its_group() -> Result<(), String> {
    let f = fixture().await?;
    let speed = enc("speed > 50");
    let escaping = enc("speed<65)|(speed>0");
    let (code, body) = status(&f, &format!("filter={speed}&ngsild.q={escaping}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    let (whole, body) = status(
        &f,
        &format!("filter={speed}&ngsild.q={}", enc("(speed<65)|(speed>0)")),
    )
    .await;
    assert_eq!(whole, StatusCode::OK, "{body}");
    Ok(())
}

#[tokio::test]
async fn temporal_functions_pushed_or_residual() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("T_AFTER(seen, TIMESTAMP('2026-03-01T00:00:00Z'))", &["2"]),
            ("T_BEFORE(seen, TIMESTAMP('2026-03-01T00:00:00Z'))", &["1"]),
            ("T_EQUALS(seen, TIMESTAMP('2026-06-01T00:00:00Z'))", &["2"]),
            ("T_AFTER(seen, DATE('2026-03-01'))", &["2"]),
            (
                "T_DURING(seen, INTERVAL('2025-12-01T00:00:00Z', '2026-02-01T00:00:00Z'))",
                &["1"],
            ),
            ("T_INTERSECTS(seen, INTERVAL('..', '..'))", &["1", "2"]),
            // A missing time holds no relation, not even under NOT.
            ("NOT T_INTERSECTS(seen, INTERVAL('..', '..'))", &[]),
            (
                "T_BEFORE(seen, INTERVAL('2026-02-01T00:00:00Z', '..'))",
                &["1"],
            ),
            (
                "T_STARTS(seen, INTERVAL('2026-06-01T00:00:00Z', '2026-07-01T00:00:00Z'))",
                &["2"],
            ),
            (
                "T_STARTS(INTERVAL(seen, '2026-06-15T00:00:00Z'), INTERVAL('2026-06-01T00:00:00Z', '2026-07-01T00:00:00Z'))",
                &["2"],
            ),
            (
                "T_MEETS(INTERVAL('2026-05-01T00:00:00Z', seen), INTERVAL('2026-06-01T00:00:00Z', '..'))",
                &["2"],
            ),
            (
                "T_DISJOINT(seen, INTERVAL('2026-05-01T00:00:00Z', '2026-07-01T00:00:00Z'))",
                &["1"],
            ),
            (
                "T_CONTAINS(INTERVAL('2025-01-01T00:00:00Z', '..'), seen)",
                &["1", "2"],
            ),
        ],
    )
    .await
}

#[tokio::test]
async fn array_functions() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("A_OVERLAPS(tags, ('b', 'z'))", &["2"]),
            ("A_CONTAINS(tags, ('a', 'b'))", &["2"]),
            ("A_CONTAINS(tags, ('a', 'z'))", &[]),
            ("A_CONTAINEDBY(tags, ('a', 'b', 'c'))", &["2"]),
            ("A_EQUALS(tags, ('b', 'a'))", &["2"]),
            ("A_EQUALS(tags, ('a', 'z'))", &[]),
            ("A_CONTAINS(tags, tags)", &["2"]),
            ("NOT A_OVERLAPS(tags, ('z', 'y'))", &["2"]),
        ],
    )
    .await
}

#[tokio::test]
async fn property_property_is_residual() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("speed > speed", &[]),
            ("speed = speed", &["1", "2", "3", "4"]),
            ("60 = speed", &["2"]),
        ],
    )
    .await
}

#[tokio::test]
async fn arithmetic_is_residual() -> Result<(), String> {
    let f = fixture().await?;
    check(
        &f,
        &[
            ("speed * 2 > 100", &["2", "4"]),
            ("speed + 5 = 15", &["1"]),
            ("speed - 30 = 0", &["3"]),
            ("speed / 20 = 0.5", &["1"]),
            ("speed div 20 = 1", &["3"]),
            ("speed % 20 = 10", &["1", "3", "4"]),
            ("speed ^ 2 > 3000", &["2", "4"]),
        ],
    )
    .await
}

#[tokio::test]
async fn hidden_and_unknown_property_are_the_same_400() -> Result<(), String> {
    let f = fixture().await?;
    let hidden = status(&f, &format!("filter={}", enc("secret = 's1'"))).await;
    let unknown = status(&f, &format!("filter={}", enc("nosuch = 's1'"))).await;
    assert_eq!(hidden.0, StatusCode::BAD_REQUEST);
    assert_eq!(hidden, unknown);
    for filter in ["speed > 1 OR secret = 's1'", "NOT secret IS NULL"] {
        assert_eq!(
            status(&f, &format!("filter={}", enc(filter))).await,
            unknown,
            "{filter}"
        );
    }
    // As a parameter, a hidden name is any unknown parameter.
    let param = status(&f, "secret=s1").await;
    assert_eq!(param, status(&f, "nosuch=s1").await);
    assert_eq!(param.0, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn queryables_as_parameters() -> Result<(), String> {
    let f = fixture().await?;
    assert_eq!(cars(&f, "speed=60").await?, ["2"]);
    assert_eq!(cars(&f, "colour=red&speed=60").await?, ["2"]);
    assert_eq!(
        cars(&f, &format!("colour=red&filter={}", enc("speed < 50"))).await?,
        Vec::<String>::new()
    );
    assert_eq!(cars(&f, "id=urn:ngsi-ld:Car:3").await?, ["3"]);
    assert_eq!(status(&f, "speed=fast").await.0, StatusCode::BAD_REQUEST);
    // Not scalar: no parameter.
    assert_eq!(status(&f, "tags=a").await.0, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn filter_parameters_are_checked() -> Result<(), String> {
    let f = fixture().await?;
    let speed = enc("speed > 50");
    for query in [
        format!("filter={speed}&filter-lang=ecql"),
        format!(
            "filter={speed}&filter-crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326"
        ),
        format!("filter={}", enc("speed >")),
        format!("filter-lang=cql2-json&filter={speed}"),
        format!("filter={}", enc("speed = INTERVAL('..', '..')")),
        format!("filter={}", enc(&"NOT ".repeat(100))),
        "ngsild.pick=speed&ngsild.omit=colour".to_owned(),
    ] {
        assert_eq!(
            status(&f, &query).await.0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    let crs84 = enc("http://www.opengis.net/def/crs/OGC/1.3/CRS84");
    assert_eq!(
        cars(&f, &format!("filter={speed}&filter-crs={crs84}")).await?,
        ["2", "4"]
    );
    // A pattern that is no literal is a construct the residual does not evaluate.
    let (s, _) = status(&f, &format!("filter={}", enc("'red' LIKE colour"))).await;
    assert!(
        matches!(s, StatusCode::NOT_IMPLEMENTED | StatusCode::BAD_REQUEST),
        "{s}"
    );
    Ok(())
}

#[tokio::test]
async fn deep_nesting_is_bounded() -> Result<(), String> {
    let f = fixture().await?;
    let deep = format!("{}speed > 1{}", "(".repeat(200), ")".repeat(200));
    let (s, body) = status(&f, &format!("filter={}", enc(&deep))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let at_ceiling = format!("{}speed > 1{}", "(".repeat(20), ")".repeat(20));
    assert_eq!(filtered(&f, &at_ceiling).await?, ["1", "2", "3", "4"]);
    Ok(())
}

#[tokio::test]
async fn a_residual_page_selects_and_strips_what_it_read() -> Result<(), String> {
    let f = fixture().await?;
    // Paged one at a time: the pages concatenate to the whole answer.
    assert_eq!(
        cars(&f, &format!("limit=1&filter={}", enc("NOT speed > 50"))).await?,
        ["1", "3"]
    );
    let a = get(
        &f.app,
        &format!("{CARS}?properties=colour&filter={}", enc("NOT speed > 50")),
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let features = a
        .body
        .get("features")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(features.len(), 2);
    for feature in &features {
        assert!(feature.pointer("/properties/speed").is_none(), "{feature}");
    }
    // ngsild.omit of an Attribute the filter reads: the filter still reads it.
    let omit = get(
        &f.app,
        &format!("{CARS}?ngsild.omit=speed&filter={}", enc("NOT speed > 50")),
    )
    .await;
    assert_eq!(ids(&omit.body).len(), 2, "{}", omit.body);
    assert!(omit.body.pointer("/features/0/properties/speed").is_none());
    assert_eq!(a.body.get("numberMatched"), Some(&json!(2)));
    Ok(())
}

#[tokio::test]
async fn a_filter_never_widens_the_policy() -> Result<(), String> {
    // The Policy narrows reads to speed<50: no filter reaches Cars 2 and 4.
    let f = narrowed().await?;
    check(
        &f,
        &[
            ("speed > 0", &["1", "3"]),
            ("NOT speed < 50", &[]),
            ("speed > 50 OR speed < 50", &["1", "3"]),
            ("A_OVERLAPS(tags, ('a'  , 'b'))", &[]),
        ],
    )
    .await?;
    // The view's own q narrows the same way.
    let f = fixture().await?;
    let slow = "/x/views/slow/ogc/features/collections/Car/items";
    let a = get(&f.app, &format!("{slow}?filter={}", enc("NOT speed < 50"))).await;
    assert_eq!((a.status, ids(&a.body).len()), (StatusCode::OK, 0));
    Ok(())
}
