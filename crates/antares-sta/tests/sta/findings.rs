// SPDX-License-Identifier: EUPL-1.2
//! The defects the STA mapping review of 2026-10-22 found live on dev (T-5205,
//! evidence/supervisor-2026-10-22-mapping-validation/sta.md), each as the answer it gives now.

use crate::common::{at, fixture, fixture_with, ids, local, sta};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use serde_json::{json, Value};

/// A link of this surface as the path below the `city` view's STA root, its `#` sent as `%23`.
fn path(link: &Value) -> String {
    let link = local(link.as_str().unwrap_or_default());
    link.strip_prefix("/x/views/city/sta/v1.1")
        .unwrap_or(&link)
        .replace('#', "%23")
}

/// Finding 1 and 3: an Entity with no temporal record still has its current Observation, an
/// untimed one too, under its Datastream.
#[tokio::test]
async fn a_datastream_without_history_lists_its_current_observation() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:2%2Fspeed')/Observations",
    )
    .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    let listed = sta(&f.app, "/Observations").await;
    let current: Vec<String> = ids(&listed.body)
        .into_iter()
        .filter(|i| i.starts_with("urn:ngsi-ld:Car:2/speed/"))
        .collect();
    assert_eq!(ids(&a.body), current);
    assert_eq!(at(&a.body, "/value/0/resultTime"), Value::Null);
    assert_eq!(at(&a.body, "/value/0/parameters/timeSource"), "modifiedAt");
    Ok(())
}

/// The fixture of the paging and id findings: an `https:` Car with a speed history of 250 behind
/// 120 Stops that measure nothing.
async fn long() -> Result<crate::common::Fixture, String> {
    let car = "https://example.org/car/9";
    let mut entities: Vec<Value> = (0..120)
        .map(|n| {
            json!({"id": format!("urn:ngsi-ld:Stop:{n:03}"), "type": "Stop",
                        "name": {"type": "Property", "value": format!("stop {n}")}})
        })
        .collect();
    entities.push(json!({"id": car, "type": "Car",
        "speed": {"type": "Property", "value": 250, "observedAt": "2026-01-01T04:10:00Z"}}));
    let history: Vec<Value> = (0..249)
        .map(|n| {
            json!({"type": "Property", "value": n,
                        "observedAt": format!("2026-01-01T{:02}:{:02}:00Z", n / 60, n % 60)})
        })
        .collect();
    let histories = [json!({"id": car, "type": "Car", "speed": history})];
    fixture_with(TestBroker::new().await?, &entities, &histories).await
}

/// Finding 2: three pages of a 250-instance series are disjoint, newest first, and hold all 250.
/// Finding 4: an `https:` Entity id reads back as a Thing, a Datastream and an Observation.
/// Finding 15: the first Datastreams page holds the one Datastream behind 120 Entities without.
#[tokio::test]
async fn a_long_series_pages_without_repeats_and_an_http_id_reads_back() -> Result<(), String> {
    let f = long().await?;
    let streams = sta(&f.app, "/Datastreams").await;

    let stream = "https:%2F%2Fexample.org%2Fcar%2F9/speed";
    assert_eq!(ids(&streams.body), [stream], "{}", streams.body);

    let thing = sta(&f.app, "/Things('https://example.org/car/9')").await;
    assert_eq!(thing.status, StatusCode::OK);
    let one = sta(&f.app, &path(&at(&streams.body, "/value/0/@iot.selfLink"))).await;
    assert_eq!(at(&one.body, "/@iot.id"), stream);
    let mut link = format!(
        "{}?$top=100",
        path(&at(&one.body, "/Observations@iot.navigationLink"))
    );
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..4 {
        let page = sta(&f.app, &link).await;
        assert_eq!(page.status, StatusCode::OK, "{link}: {}", page.body);
        seen.extend(ids(&page.body));
        match page.body.get("@iot.nextLink") {
            Some(next) => link = path(next),
            None => break,
        }
    }
    assert_eq!(seen.len(), 250);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 250);
    sorted.reverse();
    assert_eq!(seen, sorted, "newest first");
    let first = seen.first().cloned().unwrap_or_default();
    assert_eq!(first, format!("{stream}/2026-01-01T04:10:00.000Z"));
    let oldest = sta(
        &f.app,
        &format!(
            "/Observations('{}')",
            seen.last().cloned().unwrap_or_default().replace('%', "%25")
        ),
    )
    .await;
    assert_eq!(at(&oldest.body, "/result"), 0, "{}", oldest.body);
    Ok(())
}

/// Finding 13: each HistoricalLocation's Locations is where the Thing was then.
#[tokio::test]
async fn each_historical_location_has_its_own_geometry() -> Result<(), String> {
    let f = fixture().await?;
    let moves = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Vehicle:1')/HistoricalLocations",
    )
    .await;
    let mut geometries = Vec::new();
    for h in moves
        .body
        .get("value")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let places = sta(&f.app, &path(&at(h, "/Locations@iot.navigationLink"))).await;
        assert_eq!(ids(&places.body).len(), 1, "{}", places.body);
        let place = at(&places.body, "/value/0");
        let again = sta(&f.app, &path(&at(&place, "/@iot.selfLink"))).await;
        assert_eq!(again.body, place);
        geometries.push(at(&place, "/location/coordinates"));
    }
    geometries.sort_by_key(Value::to_string);
    geometries.dedup();
    assert_eq!(geometries.len(), 3);
    Ok(())
}

/// Findings 8 and 9: STA names in `$filter`, `result` refused by name, and the type-restricted
/// link the catalogue publishes.
#[tokio::test]
async fn a_filter_reads_sta_names_and_the_catalogue_link_its_type() -> Result<(), String> {
    let f = fixture().await?;
    // T-5918: a property no `q` names is judged over the collection the view answered.
    let all = sta(&f.app, "/Observations?$filter=result gt 0").await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.body);
    let results = at(&all.body, "/value")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_ne!(results.len(), 0);
    assert!(
        results
            .iter()
            .all(|o| o.get("result").and_then(Value::as_f64) > Some(0.0)),
        "{}",
        all.body
    );
    let result = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=result gt 8",
    )
    .await;
    assert_eq!(result.status, StatusCode::OK, "{}", result.body);
    let kept = at(&result.body, "/value")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        kept.iter()
            .all(|o| o.get("result").and_then(Value::as_f64) > Some(8.0)),
        "{}",
        result.body
    );
    let cars = sta(
        &f.app,
        "/Datastreams?$filter=Thing/properties/type eq 'Car' and Thing/name eq 'Žltá'",
    )
    .await;
    assert_eq!(ids(&cars.body).len(), 3, "{}", cars.body);
    let hidden = sta(
        &f.app,
        "/Datastreams?$filter=Thing/properties/type eq 'Radar'",
    )
    .await;
    assert_eq!(
        (hidden.status, ids(&hidden.body).len()),
        (StatusCode::OK, 0)
    );
    let spaced = sta(&f.app, "/Things?$filter=name eq 'Žltá a modrá'").await;
    assert_eq!(spaced.status, StatusCode::OK);
    Ok(())
}

/// Findings 17, 18 and 19: `$select=id`, `$select` on one resource, `$expand` refused by name,
/// `$top=0` the count alone.
#[tokio::test]
async fn select_expand_and_top_answer_as_documented() -> Result<(), String> {
    let f = fixture().await?;
    let list = sta(&f.app, "/Things?$select=id,name&$top=1").await;
    assert_eq!(
        at(&list.body, "/value/0"),
        json!({"@iot.id": "urn:ngsi-ld:Car:1", "name": "Žltá"})
    );
    let one = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')?$select=id").await;
    assert_eq!(one.body, json!({"@iot.id": "urn:ngsi-ld:Car:1"}));
    let deep = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:1')?$expand=Datastreams/Observations",
    )
    .await;
    assert!(
        at(&deep.body, "/Datastreams/0/Observations").is_array(),
        "{}",
        deep.body
    );
    // T-5918: every navigation path is expanded, one through its navigation link.
    let inlined = sta(&f.app, "/Observations?$expand=Datastream/Thing").await;
    assert_eq!(inlined.status, StatusCode::OK, "{}", inlined.body);
    assert!(
        at(&inlined.body, "/value/0/Datastream/Thing/@iot.id").is_string(),
        "{}",
        inlined.body
    );
    let refused = sta(&f.app, "/Observations?$expand=Sensor").await;
    assert_eq!(
        (refused.status, at(&refused.body, "/error/message")),
        (
            StatusCode::BAD_REQUEST,
            json!("$expand: Sensor is not a navigation path of Observations")
        )
    );
    let count = sta(&f.app, "/Things?$top=0&$count=true").await;
    assert_eq!(count.body, json!({"@iot.count": 5, "value": []}));
    let order = sta(&f.app, "/Things?$orderby=name desc").await;
    let names: Vec<Value> = at(&order.body, "/value")
        .as_array()
        .map(|v| {
            v.iter()
                .map(|t| t.get("name").cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let mut sorted = names.clone();
    sorted.sort_by_key(|n| std::cmp::Reverse(n.as_str().map(str::to_owned)));
    assert_eq!((order.status, names.len()), (StatusCode::OK, 5));
    assert_eq!(names, sorted);
    Ok(())
}

/// The edges of the type selector, `$top=0` and the historical Location ids.
#[tokio::test]
async fn the_type_selector_and_the_count_alone_hold_at_their_edges() -> Result<(), String> {
    let f = fixture().await?;
    let every = "/x/views/every/sta/v1.1/Things?$filter=properties/type eq 'Stop'";
    let stops = crate::common::send(&f.app, "GET", every, "").await;
    assert_eq!(ids(&stops.body), ["urn:ngsi-ld:Stop:1"]);
    let hidden = sta(
        &f.app,
        "/HistoricalLocations?$filter=properties/type eq 'Radar'",
    )
    .await;
    assert_eq!(
        (hidden.status, ids(&hidden.body).len()),
        (StatusCode::OK, 0)
    );
    let broken = sta(&f.app, "/HistoricalLocations?$filter=properties/speed eq").await;
    assert_eq!(broken.status, StatusCode::BAD_REQUEST);
    let count = sta(
        &f.app,
        "/Things?$top=0&$count=true&$filter=id ne 'urn:ngsi-ld:Car:1'",
    )
    .await;
    assert_eq!(count.body, json!({"value": []}));
    // Another set's count alone is the count of its first page when that is the whole set.
    let streams = sta(&f.app, "/Datastreams?$top=0&$count=true").await;
    assert_eq!(streams.body, json!({"@iot.count": 4, "value": []}));
    let uncounted = sta(&f.app, "/Datastreams?$top=0").await;
    assert_eq!(uncounted.body, json!({"value": []}));
    // An instant the broker refuses is its 400, never an empty series.
    let odd = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=phenomenonTime ge 2026-01-01T00:00:00%2B0100").await;
    assert_eq!(odd.status, StatusCode::BAD_REQUEST, "{}", odd.body);
    let nowhere = sta(&f.app, "/Locations('x/location/2026-01-01T00:00:00.000Z')").await;
    assert_eq!(nowhere.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// F-ANT5-004: the unknown Sensor heading the first page is no Entity served, so the next link
/// resumes at the second Entity and every Device is listed once over the pages. Car:9 names no
/// Device, so the unknown Sensor has its Datastream.
#[tokio::test]
async fn sensor_pages_list_every_device_once() -> Result<(), String> {
    let mut entities = vec![json!({"id": "urn:ngsi-ld:Car:9", "type": "Car",
        "speed": {"type": "Property", "value": 5}})];
    for i in 1..=3 {
        let device = format!("urn:ngsi-ld:Device:{i}");
        entities.push(json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
            "refDevice": {"type": "Relationship", "object": device}}));
        entities.push(json!({"id": device, "type": "Device",
            "name": {"type": "Property", "value": format!("radar {i}")}}));
    }
    let f = fixture_with(TestBroker::new().await?, &entities, &[]).await?;
    let mut seen = Vec::new();
    let mut path = "/Sensors?$top=1".to_owned();
    for _ in 0..10 {
        let page = sta(&f.app, &path).await;
        seen.extend(ids(&page.body));
        match at(&page.body, "/@iot.nextLink").as_str() {
            Some(link) => {
                path = local(link).replace("/x/views/city/sta/v1.1", "");
            }
            None => break,
        }
    }
    assert_eq!(
        seen,
        [
            "unknown",
            "urn:ngsi-ld:Device:1",
            "urn:ngsi-ld:Device:2",
            "urn:ngsi-ld:Device:3"
        ]
    );
    Ok(())
}

/// T-5918: options combine in the order 15-078r6 9.3.3.1 gives: `$filter`, then `$count`,
/// `$orderby`, `$skip`, `$top` (ets-sta10 Capability3Tests.checkQueriesPriorityOrdering).
#[tokio::test]
async fn query_options_apply_in_their_evaluation_order() -> Result<(), String> {
    let f = fixture().await?;
    let all = sta(&f.app, "/Observations?$filter=result gt 0").await;
    let mut results: Vec<f64> = at(&all.body, "/value")
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|o| o.get("result").and_then(Value::as_f64))
                .collect()
        })
        .unwrap_or_default();
    results.sort_by(f64::total_cmp);
    assert!(results.len() > 1, "{}", all.body);
    let page = sta(
        &f.app,
        "/Observations?$count=true&$top=1&$skip=1&$orderby=result%20asc&$filter=result%20gt%200",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(at(&page.body, "/@iot.count"), json!(results.len()));
    assert_eq!(
        at(&page.body, "/value").as_array().map(Vec::len),
        Some(1),
        "{}",
        page.body
    );
    assert_eq!(
        at(&page.body, "/value/0/result").as_f64(),
        results.get(1).copied()
    );
    let next = at(&page.body, "/@iot.nextLink");
    assert!(
        next.as_str().is_some_and(|l| l.ends_with("$skip=2")),
        "{next}"
    );
    Ok(())
}

/// T-5918: what a filter or an order over the collection refuses, and where it applies.
#[tokio::test]
async fn a_filter_over_the_collection_names_what_it_cannot_judge() -> Result<(), String> {
    let f = fixture().await?;
    let native = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$orderby=phenomenonTime%20desc",
    )
    .await;
    assert_eq!(native.status, StatusCode::OK, "{}", native.body);
    for (path, message) in [
        (
            "/Things('urn:ngsi-ld:Car:1')?$orderby=name",
            "$filter and $orderby apply to a collection",
        ),
        (
            "/Things('urn:ngsi-ld:Car:1')/name/$value?$filter=name%20eq%20'a'",
            "$filter and $orderby apply to a collection",
        ),
        (
            "/Things?$orderby=nosuch",
            "$orderby: nosuch is no property of these resources; quote a text",
        ),
    ] {
        let a = sta(&f.app, path).await;
        assert_eq!(
            (a.status, at(&a.body, "/error/message")),
            (StatusCode::BAD_REQUEST, json!(message)),
            "{path}"
        );
    }
    // an empty collection holds no name to check
    let empty = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Device:1')/Datastreams?$filter=zzz%20eq%201",
    )
    .await;
    assert_eq!((empty.status, ids(&empty.body).len()), (StatusCode::OK, 0));
    let counted = sta(
        &f.app,
        "/Observations?$top=0&$count=true&$filter=result%20gt%200",
    )
    .await;
    assert_eq!(at(&counted.body, "/value"), json!([]));
    assert!(
        at(&counted.body, "/@iot.count").as_u64() > Some(0),
        "{}",
        counted.body
    );
    assert_eq!(at(&counted.body, "/@iot.nextLink"), Value::Null);
    // a Sensor the view hides expands as null, and nothing below it is read
    let hidden = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:2')/Datastreams?$expand=Sensor/Datastreams",
    )
    .await;
    assert_eq!(hidden.status, StatusCode::OK, "{}", hidden.body);
    assert_eq!(
        at(&hidden.body, "/value/0/Sensor"),
        Value::Null,
        "{}",
        hidden.body
    );
    Ok(())
}
