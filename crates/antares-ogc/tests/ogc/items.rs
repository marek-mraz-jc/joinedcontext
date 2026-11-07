// SPDX-License-Identifier: EUPL-1.2
//! Part 1 Core: `/items` and `/items/{featureId}` (mapping rows C8, C10 to C14, C16, C18,
//! C19), through the view and through a narrowing engine.

use crate::common::{
    fixture, fixture_with, get, href, ids, local, narrowed, schemas_on, send, SCHEMA,
};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use serde_json::{json, Value};

const CARS: &str = "/x/views/city/ogc/features/collections/Car/items";

#[tokio::test(flavor = "multi_thread")]
async fn items_are_the_view_answer() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, CARS).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let mut got = ids(&a.body);
    got.sort();
    assert_eq!(
        got,
        [
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:4"
        ]
    );
    assert!(
        !a.body.to_string().contains("secret") && !a.body.to_string().contains("\"s1\""),
        "a hidden Attribute appears nowhere"
    );
    let slow = get(&f.app, "/x/views/slow/ogc/features/collections/Car/items").await;
    let mut got = ids(&slow.body);
    got.sort();
    assert_eq!(
        got,
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "the view's q narrows"
    );
    assert_eq!(slow.body.get("numberMatched"), Some(&json!(2)));
    let stops = get(&f.app, "/x/views/empty/ogc/features/collections/Stop/items").await;
    assert_eq!(ids(&stops.body), ["urn:ngsi-ld:Stop:1"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_engine_narrows_items_counts_links_and_items_by_id() -> Result<(), String> {
    let f = narrowed().await?;
    let a = get(&f.app, &format!("{CARS}?limit=1")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        a.body.get("numberMatched"),
        Some(&json!(2)),
        "Cars 2 and 4 are not counted"
    );
    let mut seen = ids(&a.body);
    let next = href(&a.body, "next").ok_or("a next link")?;
    let b = get(&f.app, &local(&next)).await;
    seen.extend(ids(&b.body));
    seen.sort();
    assert_eq!(seen, ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"]);
    assert!(
        href(&b.body, "next").is_none(),
        "no link past the narrowed set"
    );
    let hidden = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Car:2")).await;
    let absent = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Car:99")).await;
    assert_eq!(
        (hidden.status, &hidden.body),
        (StatusCode::NOT_FOUND, &absent.body),
        "hidden reads as absent"
    );
    let boxed = get(&f.app, &format!("{CARS}?bbox=179,-1,-179,1")).await;
    assert_eq!(
        ids(&boxed.body),
        ["urn:ngsi-ld:Car:3"],
        "only the Car without location: 2 and 4 are hidden"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn limit_default_clamp_and_bounds() -> Result<(), String> {
    let f = fixture().await?;
    let one = get(&f.app, &format!("{CARS}?limit=1")).await;
    assert_eq!(
        (
            one.body.get("numberReturned"),
            one.body.get("numberMatched")
        ),
        (Some(&json!(1)), Some(&json!(4)))
    );
    let all = get(&f.app, &format!("{CARS}?limit=10000")).await;
    assert_eq!(
        all.status,
        StatusCode::OK,
        "10000 is the declared maximum: {}",
        all.body
    );
    assert_eq!(all.body.get("numberReturned"), Some(&json!(4)));
    assert!(href(&all.body, "next").is_none());
    Ok(())
}

/// T-4926: ETS 1.8 Core `fc-numberMatched` reads the first page, then follows at most four
/// `next` links with `limit` set to the `maximum` of the API definition (10000). A page of the declared maximum
/// holds that many Features, read from the store in pages of its `max_limit` (1000), so 4,109
/// Cars arrive whole, each once, as on prod's Contract collection.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_collection_pages_whole_at_the_declared_maximum() -> Result<(), String> {
    let f = fixture().await?;
    let total = 4109;
    let cars: Vec<Value> = (4..total)
        .map(|n| {
            json!({"id": format!("urn:ngsi-ld:Car:bulk-{n:05}"), "type": "Car",
                        "speed": {"type": "Property", "value": n}})
        })
        .collect();
    for chunk in cars.chunks(1000) {
        let a = send(
            &f.app,
            "POST",
            "/ngsi-ld/v1/entityOperations/create",
            &Value::from(chunk.to_vec()).to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    }
    for first in [1, 10, 100] {
        let a = get(&f.app, &format!("{CARS}?limit={first}")).await;
        assert_eq!(a.body.get("numberMatched"), Some(&json!(total)));
        let mut seen = ids(&a.body);
        let mut next = href(&a.body, "next");
        for _ in 0..4 {
            let Some(link) = next else { break };
            let at = local(&link).replace(&format!("limit={first}"), "limit=10000");
            let page = get(&f.app, &at).await;
            assert_eq!(page.status, StatusCode::OK, "{}", page.body);
            seen.extend(ids(&page.body));
            next = href(&page.body, "next");
        }
        assert!(
            next.is_none(),
            "limit={first}: the walk ends with the answer"
        );
        let distinct: std::collections::BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            (seen.len(), distinct.len()),
            (total, total),
            "limit={first}"
        );
    }
    let whole = get(&f.app, &format!("{CARS}?limit=10000")).await;
    assert_eq!(whole.body.get("numberReturned"), Some(&json!(total)));
    assert!(href(&whole.body, "next").is_none());
    let tail = get(&f.app, &format!("{CARS}?limit=2500")).await;
    let rest = href(&tail.body, "next").ok_or("a page of 2500 of 4109 links on")?;
    let rest = get(&f.app, &local(&rest)).await;
    assert_eq!(rest.body.get("numberReturned"), Some(&json!(total - 2500)));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bbox_2d_3d_and_antimeridian() -> Result<(), String> {
    let f = fixture().await?;
    let ask = |q: &str| format!("{CARS}?bbox={q}");
    let around = |body: &Value| {
        let mut v = ids(body);
        v.sort();
        v
    };
    let a = get(&f.app, &ask("17,48,18,49")).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        around(&a.body),
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "Car 1 inside, Car 3 has no geometry"
    );
    assert_eq!(
        around(&get(&f.app, &ask("17,48,-100,18,49,100")).await.body),
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "6 numbers"
    );
    assert_eq!(
        around(&get(&f.app, &ask("179,-1,-179,1")).await.body),
        [
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Car:3",
            "urn:ngsi-ld:Car:4"
        ],
        "across 180"
    );
    assert_eq!(
        around(&get(&f.app, &ask("0,-1,1,1")).await.body),
        ["urn:ngsi-ld:Car:3"]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bbox_keeps_features_without_geometry() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{CARS}?bbox=0,-1,1,1&limit=1")).await;
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Car:3"]);
    assert_eq!(a.body.get("numberMatched"), Some(&json!(1)));
    let stops = get(
        &f.app,
        "/x/views/city/ogc/features/collections/Stop/items?bbox=0,0,1,1",
    )
    .await;
    assert_eq!(
        ids(&stops.body),
        ["urn:ngsi-ld:Stop:1"],
        "a collection without geometry matches whole"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn datetime_instant_interval_open_ends() -> Result<(), String> {
    let f = fixture().await?;
    for (datetime, want) in [
        ("2026-01-01T00:00:00Z", vec!["urn:ngsi-ld:Car:1"]),
        (
            "2025-12-01T00:00:00Z/2026-02-01T00:00:00Z",
            vec!["urn:ngsi-ld:Car:1"],
        ),
        ("2026-03-01T00:00:00Z/..", vec!["urn:ngsi-ld:Car:2"]),
        ("/2026-03-01T00:00:00Z", vec!["urn:ngsi-ld:Car:1"]),
        ("2027-01-01T00:00:00Z/", vec![]),
    ] {
        let a = get(&f.app, &format!("{CARS}?datetime={datetime}")).await;
        assert_eq!(a.status, StatusCode::OK, "{datetime}: {}", a.body);
        let mut got: Vec<String> = ids(&a.body)
            .into_iter()
            .filter(|id| id != "urn:ngsi-ld:Car:3" && id != "urn:ngsi-ld:Car:4")
            .collect();
        got.sort();
        assert_eq!(got, want, "{datetime}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn datetime_keeps_features_without_time() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{CARS}?datetime=2027-01-01T00:00:00Z")).await;
    let mut got = ids(&a.body);
    got.sort();
    assert_eq!(
        got,
        ["urn:ngsi-ld:Car:3", "urn:ngsi-ld:Car:4"],
        "never seen: no temporal geometry"
    );
    let stops = get(
        &f.app,
        "/x/views/city/ogc/features/collections/Stop/items?datetime=2027-01-01T00:00:00Z",
    )
    .await;
    assert_eq!(
        ids(&stops.body),
        ["urn:ngsi-ld:Stop:1"],
        "a type with no temporal slot matches whole"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn next_token_bound_to_view_and_query() -> Result<(), String> {
    let f = fixture().await?;
    let mut seen = Vec::new();
    let mut uri = format!("{CARS}?bbox=17,48,180,49&limit=1");
    let mut pages = 0;
    loop {
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, StatusCode::OK, "{uri}: {}", a.body);
        assert!(a.body.get("numberReturned") == Some(&json!(1)) || pages > 0);
        seen.extend(ids(&a.body));
        pages += 1;
        match href(&a.body, "next") {
            Some(next) => uri = local(&next),
            None => break,
        }
        assert!(pages < 10, "the pages end");
    }
    seen.sort();
    assert_eq!(
        seen,
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:3"],
        "the box part, then the part without geometry"
    );
    let first = get(&f.app, &format!("{CARS}?limit=1")).await;
    let token = href(&first.body, "next")
        .and_then(|h| h.split("next=").nth(1).map(str::to_owned))
        .ok_or("a token")?;
    for other in [
        format!("/x/views/slow/ogc/features/collections/Car/items?limit=1&next={token}"),
        format!("{CARS}?limit=1&bbox=0,0,1,1&next={token}"),
        format!("{CARS}?limit=1&next={token}x"),
    ] {
        assert_eq!(
            get(&f.app, &other).await.status,
            StatusCode::BAD_REQUEST,
            "{other}"
        );
    }
    assert_eq!(
        get(&f.app, &format!("{CARS}?limit=1&next={token}"))
            .await
            .status,
        StatusCode::OK
    );
    // the page size is no part of the request a token belongs to (ETS
    // `validateFeaturesWithLimitResponse_NumberMatched` follows a token
    // with another `limit`)
    assert_eq!(
        get(&f.app, &format!("{CARS}?limit=10000&next={token}"))
            .await
            .status,
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_are_the_view_counts() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, &format!("{CARS}?bbox=179,-1,-179,1&limit=2")).await;
    assert_eq!(
        (a.body.get("numberMatched"), a.body.get("numberReturned")),
        (Some(&json!(3)), Some(&json!(2)))
    );
    assert_eq!(a.body.get("type"), Some(&json!("FeatureCollection")));
    assert!(href(&a.body, "self").is_some_and(|s| s.ends_with("?bbox=179,-1,-179,1&limit=2")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn feature_of_another_type_is_404() -> Result<(), String> {
    let f = fixture().await?;
    let car = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Car:1")).await;
    assert_eq!(car.status, StatusCode::OK, "{}", car.body);
    assert_eq!(car.body.get("id"), Some(&json!("urn:ngsi-ld:Car:1")));
    let encoded = get(&f.app, &format!("{CARS}/urn%3Angsi-ld%3ACar%3A1")).await;
    assert_eq!(
        encoded.body, car.body,
        "a percent-encoded id is the same Feature"
    );
    let stop = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Stop:1")).await;
    let absent = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Car:99")).await;
    let depot = get(&f.app, &format!("{CARS}/urn:ngsi-ld:Depot:1")).await;
    for other in [&stop, &absent, &depot] {
        assert_eq!(
            (other.status, &other.body),
            (StatusCode::NOT_FOUND, &absent.body)
        );
    }
    let bad = get(&f.app, &format!("{CARS}/a%zz")).await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_collection_is_an_empty_feature_collection() -> Result<(), String> {
    let f = fixture().await?;
    // `slow` narrows to speed<50, which no Stop has.
    let a = get(&f.app, "/x/views/slow/ogc/features/collections/Stop/items").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        (a.body.get("numberMatched"), a.body.get("features")),
        (Some(&json!(0)), Some(&json!([])))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_id_that_is_no_uri_is_404_and_a_view_without_queries_refuses() -> Result<(), String> {
    let f = fixture().await?;
    for id in ["abc", "not%20a%20urn"] {
        let a = get(&f.app, &format!("{CARS}/{id}")).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{id}: {}", a.body);
    }
    let reads = "/x/views/reads/ogc/features/collections/Car/items";
    for path in [reads.to_owned(), format!("{reads}/urn:ngsi-ld:Car:1")] {
        let a = get(&f.app, &path).await;
        assert_eq!(
            (a.status, a.body.get("detail")),
            (
                StatusCode::METHOD_NOT_ALLOWED,
                Some(&json!("the view refused the query"))
            ),
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_next_link_crosses_from_the_bbox_part_to_the_features_without_geometry(
) -> Result<(), String> {
    let f = fixture().await?;
    let first = get(&f.app, &format!("{CARS}?bbox=170,-1,-170,1&limit=2")).await;
    assert_eq!(ids(&first.body), ["urn:ngsi-ld:Car:2", "urn:ngsi-ld:Car:4"]);
    assert_eq!(first.body.get("numberMatched"), Some(&json!(3)));
    let next = href(&first.body, "next").ok_or("no next link")?;
    let second = get(&f.app, &local(&next)).await;
    assert_eq!(ids(&second.body), ["urn:ngsi-ld:Car:3"], "{}", second.body);
    assert_eq!(href(&second.body, "next"), None);
    let one = get(&f.app, &format!("{CARS}?bbox=170,-1,-170,1&limit=1")).await;
    assert_eq!(ids(&one.body), ["urn:ngsi-ld:Car:2"]);
    let next = href(&one.body, "next").ok_or("no next link")?;
    let rest = get(&f.app, &local(&next)).await;
    assert_eq!(ids(&rest.body), ["urn:ngsi-ld:Car:4"], "{}", rest.body);
    Ok(())
}

/// T-3795: a token into the part of the Features without a geometry keeps its offset, so
/// more of them than a page are read to their end, each once.
#[tokio::test(flavor = "multi_thread")]
async fn the_features_without_geometry_page_to_their_end() -> Result<(), String> {
    let f = fixture().await?;
    let car = json!({"id": "urn:ngsi-ld:Car:5", "type": "Car", "speed": {"type": "Property", "value": 5}});
    let a = send(
        &f.app,
        "POST",
        "/ngsi-ld/v1/entities",
        &car.to_string(),
        &[("NGSILD-Tenant", "city")],
    )
    .await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.body);
    let mut uri = format!("{CARS}?bbox=0,0,1,1&limit=1");
    let mut seen = Vec::new();
    for _ in 0..5 {
        let page = get(&f.app, &uri).await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        assert_eq!(
            page.body.get("numberMatched"),
            Some(&json!(2)),
            "{}",
            page.body
        );
        seen.extend(ids(&page.body));
        match href(&page.body, "next") {
            Some(next) => uri = local(&next),
            None => break,
        }
    }
    assert_eq!(seen, ["urn:ngsi-ld:Car:3", "urn:ngsi-ld:Car:5"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_that_holds_nothing_is_an_empty_collection() -> Result<(), String> {
    let f = fixture().await?;
    let schema = send(&f.app, "PUT", "/q/schemas/nobody", SCHEMA, &[]).await;
    assert_eq!(schema.status, StatusCode::CREATED, "{}", schema.body);
    let view = json!({"id": "ghost", "tenant": "nobody", "selector": ["Car"],
                      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                      "operations": ["query_entities"]});
    let put = send(&f.app, "PUT", "/q/views/ghost", &view.to_string(), &[]).await;
    assert_eq!(put.status, StatusCode::CREATED, "{}", put.body);
    let a = get(&f.app, "/x/views/ghost/ogc/features/collections/Car/items").await;
    assert_eq!(
        (
            a.status,
            a.body.get("numberMatched"),
            a.body.get("features")
        ),
        (StatusCode::OK, Some(&json!(0)), Some(&json!([])))
    );
    Ok(())
}

/// T-3773: an NGSI-LD answer past the broker's buffer bound is a 413 problem, never a 200
/// holding the whole body or an empty collection; `HEAD` measures its `GET` under the same bound.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_past_the_buffer_bound_is_413() -> Result<(), String> {
    schemas_on();
    let f = fixture_with(TestBroker::with_max_answer_bytes(128).await?).await?;
    let a = get(&f.app, CARS).await;
    assert_eq!(a.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", a.body);
    assert_eq!(a.body.get("status"), Some(&json!(413)), "{}", a.body);
    assert!(
        !a.body.to_string().contains("urn:ngsi-ld:Car"),
        "{}",
        a.body
    );
    let head = send(&f.app, "HEAD", "/x/views/city/ogc/features/api", "", &[]).await;
    assert_eq!(head.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(head.bytes, 0, "HEAD sends no body");
    Ok(())
}

/// The `href` of each link of `rel` on a Feature.
fn feature_links(feature: &Value, rel: &str) -> Vec<String> {
    feature
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|l| l.get("rel").and_then(Value::as_str) == Some(rel))
        .filter_map(|l| l.get("href").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_feature_links_its_entity_and_its_schema() -> Result<(), String> {
    let f = fixture().await?;
    let view = json!({"id": "linked", "tenant": "city", "selector": ["Car"],
                      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                      "operations": ["query_entities", "retrieve_entity"]});
    let put = send(&f.app, "PUT", "/q/views/linked", &view.to_string(), &[]).await;
    assert_eq!(put.status, StatusCode::CREATED, "{}", put.body);
    let root = "http://localhost:9090/x/views";
    let page = get(&f.app, "/x/views/linked/ogc/features/collections/Car/items").await;
    let features = page.body.get("features").and_then(Value::as_array).cloned();
    let features = features.ok_or("no features")?;
    assert_eq!(features.len(), 4, "{}", page.body);
    for feature in &features {
        let urn = feature.get("id").and_then(Value::as_str).ok_or("no id")?;
        let encoded = urn.replace(':', "%3A");
        assert_eq!(
            feature_links(feature, "alternate"),
            [format!("{root}/linked/ngsi-ld/v1/entities/{encoded}")],
            "EP-38: the Entity under the same view"
        );
        assert_eq!(
            feature_links(feature, "describedby"),
            [format!("{root}/linked/ogc/features/collections/Car/schema")]
        );
    }
    let one = get(
        &f.app,
        "/x/views/linked/ogc/features/collections/Car/items/urn:ngsi-ld:Car:1",
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.body);
    assert_eq!(
        feature_links(&one.body, "alternate"),
        [format!(
            "{root}/linked/ngsi-ld/v1/entities/urn%3Angsi-ld%3ACar%3A1"
        )]
    );
    assert_eq!(
        feature_links(&one.body, "self").len(),
        1,
        "the item keeps its own links"
    );
    let schema = get(
        &f.app,
        &local(&feature_links(&one.body, "describedby").concat()),
    )
    .await;
    assert_eq!(schema.status, StatusCode::OK, "the schema link answers");
    let city = get(&f.app, CARS).await;
    let any_alternate = city
        .body
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|f| !feature_links(f, "alternate").is_empty());
    assert!(
        !any_alternate,
        "a view without Retrieve Entity links no Entity it would refuse"
    );
    Ok(())
}

/// The CQL2 temporal operators over the cars of
/// `datetime_and_temporal_filters_read_offsets_as_instants`, with the speeds each selects.
fn temporal_cases(at: &str) -> Vec<(String, Vec<u64>)> {
    // Each operator is chosen so that text order of the stored values would answer wrong.
    let march = "INTERVAL('2026-03-01T00:00:00Z', '2026-04-01T00:00:00Z')";
    let february = "INTERVAL('2026-02-01T00:00:00Z', '2026-03-01T00:00:00Z')";
    let midnight = "INTERVAL('2026-02-28T23:30:00Z', '2026-03-01T00:30:00Z')";
    let to_year_end = "INTERVAL(seen, '2026-12-31T00:00:00Z')";
    vec![
        (format!("T_EQUALS(seen, TIMESTAMP('{at}'))"), vec![5, 10]),
        ("T_EQUALS(seen, TIMESTAMP('2026-03-02T18:00:00Z'))".into(), vec![8]),
        ("T_EQUALS(seen, DATE('2026-03-01'))".into(), vec![7]),
        (format!("T_AFTER(seen, TIMESTAMP('{at}'))"), vec![7, 8, 60]),
        (
            "T_BEFORE(seen, TIMESTAMP('2026-01-01T00:00:00.1Z'))".into(),
            vec![5, 10],
        ),
        (
            "T_BEFORE(seen, TIMESTAMP('2026-03-01T01:00:00Z'))".into(),
            vec![5, 7, 10],
        ),
        (
            "T_INTERSECTS(seen, INTERVAL('2025-12-31T23:00:00Z', '2026-01-01T00:00:00Z'))".into(),
            vec![5, 10],
        ),
        (
            "T_INTERSECTS(seen, INTERVAL('..', '2026-03-01T00:00:00Z'))".into(),
            vec![5, 7, 10],
        ),
        (format!("T_DURING(seen, {midnight})"), vec![7]),
        (
            "T_DURING(seen, INTERVAL('2026-03-01T12:00:00Z', '2026-03-03T00:00:00Z'))".into(),
            vec![8],
        ),
        (
            "T_DISJOINT(seen, INTERVAL('2026-03-01T00:30:00Z', '2026-05-01T00:00:00Z'))".into(),
            vec![5, 7, 10, 60],
        ),
        (format!("T_MEETS(seen, {march})"), vec![7]),
        (format!("T_METBY(seen, {february})"), vec![7]),
        (format!("T_STARTS(seen, {march})"), vec![7]),
        (format!("T_FINISHES(seen, {february})"), vec![7]),
        (format!("T_STARTEDBY({march}, seen)"), vec![7]),
        (format!("T_FINISHEDBY({february}, seen)"), vec![7]),
        (format!("T_CONTAINS({midnight}, seen)"), vec![7]),
        (
            format!(
                "T_OVERLAPS({to_year_end}, INTERVAL('2026-03-01T00:30:00Z', '2027-01-01T00:00:00Z'))"
            ),
            vec![5, 7, 10],
        ),
        (format!("T_OVERLAPPEDBY({to_year_end}, {midnight})"), vec![7]),
    ]
}

/// T-5204: a time written with an offset is the instant it names, for `datetime` and for the
/// CQL2 temporal functions alike (the live Events were written `+01:00`).
#[tokio::test(flavor = "multi_thread")]
async fn datetime_and_temporal_filters_read_offsets_as_instants() -> Result<(), String> {
    let f = fixture().await?;
    // Car 5 is 2026-01-01T00:00:00Z (+01:00, 9 fraction digits), Car 7 2026-03-01T00:00:00Z
    // (+02:00), Car 8 the whole day 2026-03-02; Car 10 is January 1st, Car 60 June 1st (Z).
    for (speed, at) in [
        (5, "2026-01-01T01:00:00.000000001+01:00"),
        (7, "2026-03-01T02:00:00+02:00"),
        (8, "2026-03-02"),
    ] {
        let car = json!({"type": "Feature", "geometry": null,
                         "properties": {"speed": speed, "seen": at}});
        let made = send(
            &f.app,
            "POST",
            "/x/views/fleet/ogc/features/collections/Car/items",
            &car.to_string(),
            &[("Content-Type", "application/geo+json")],
        )
        .await;
        assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    }
    let seen = |query: String| {
        let app = f.app.clone();
        async move {
            let a = get(&app, &format!("{CARS}?{query}")).await;
            assert_eq!(a.status, StatusCode::OK, "{query}: {}", a.body);
            let mut got: Vec<u64> = a
                .body
                .get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|f| f.pointer("/properties/seen").is_some())
                .filter_map(|f| f.pointer("/properties/speed").and_then(Value::as_u64))
                .collect();
            got.sort_unstable();
            got
        }
    };
    let at = "2026-01-01T00:00:00Z";
    for (query, want) in [
        (format!("datetime={at}"), vec![5, 10]),
        ("datetime=2026-01-01T00:00:00.5Z/..".into(), vec![7, 8, 60]),
        ("datetime=../2025-12-31T23:59:59%2B00:00".into(), Vec::new()),
        ("datetime=../2026-03-01T00:00:00Z".into(), vec![5, 7, 10]),
        (
            "datetime=2026-02-28T23:30:00Z/2026-03-01T00:30:00Z".into(),
            vec![7],
        ),
        ("datetime=2026-03-02T12:00:00Z".into(), vec![8]),
        ("datetime=2026-03-01T12:00:00Z/..".into(), vec![8, 60]),
    ] {
        assert_eq!(seen(query.clone()).await, want, "{query}");
    }
    for (filter, want) in temporal_cases(at) {
        let query = format!("filter={}", antares_ops::percent::encode(&filter));
        assert_eq!(seen(query).await, want, "{filter}");
    }
    Ok(())
}

/// T-5169: `id = '…'` and `id IN (…)`, alone or joined by `AND`, select by the NGSI-LD `id` list, so a
/// collection larger than the residual's scan budget still answers them, counted exactly.
#[tokio::test(flavor = "multi_thread")]
async fn an_id_filter_reads_by_id_past_the_scan_budget() -> Result<(), String> {
    let f = fixture().await?;
    let budget = antares_ops::RESIDUAL_SCAN;
    let mut next = 0;
    while next <= budget {
        let batch: Vec<Value> = (next..(next + 1000).min(budget + 1))
            .map(|i| json!({"id": format!("urn:ngsi-ld:Car:b{i}"), "type": "Car"}))
            .collect();
        next += batch.len();
        let a = send(
            &f.app,
            "POST",
            "/ngsi-ld/v1/entityOperations/create",
            &Value::Array(batch).to_string(),
            &[("NGSILD-Tenant", "city")],
        )
        .await;
        assert!(a.status.is_success(), "{} {}", a.status, a.body);
    }
    let last = format!("urn:ngsi-ld:Car:b{budget}");
    let cases = [
        (format!("id = '{last}'"), vec![last.clone()]),
        (format!("'{last}' = id"), vec![last.clone()]),
        (
            format!("id IN ('urn:ngsi-ld:Car:b7', '{last}', 'urn:ngsi-ld:Car:none')"),
            vec![last.clone(), "urn:ngsi-ld:Car:b7".to_owned()],
        ),
        (
            format!("id IN ('urn:ngsi-ld:Car:b7', '{last}') AND id = 'urn:ngsi-ld:Car:b7'"),
            vec!["urn:ngsi-ld:Car:b7".to_owned()],
        ),
        (
            "id = 'urn:ngsi-ld:Car:b7' AND id = 'urn:ngsi-ld:Car:b8'".to_owned(),
            Vec::new(),
        ),
    ];
    for (filter, want) in cases {
        let uri = format!(
            "{CARS}?filter-lang=cql2-text&filter={}",
            crate::filter::enc(&filter)
        );
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, StatusCode::OK, "{filter}: {}", a.body);
        let mut got = ids(&a.body);
        got.sort();
        assert_eq!(got, want, "{filter}");
        assert_eq!(
            a.body.get("numberMatched"),
            Some(&json!(want.len())),
            "{filter}"
        );
    }
    Ok(())
}
