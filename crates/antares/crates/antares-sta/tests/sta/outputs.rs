// SPDX-License-Identifier: EUPL-1.2
//! Named STA outputs (docs API/11 EP-159, T-5225): each output of a view's
//! `representations.sta` is its own service under `/sta/{output}/v1.1`, its Things the Entities
//! of its sources its filter selects, by query and by key, its Datastreams the attributes its
//! `datastreams` role names, read only, never past the view.

use crate::common::{at, fixture, ids, send, Fixture};
use axum::http::StatusCode;
use serde_json::{json, Value};

/// The fixture with the view `out` over `Car` and `Stop`, its named outputs: `slow` (the Cars
/// under 50 km/h, `speed` their one Datastream), `cars` (every Car, every numeric Property),
/// `hidden` (a type the view does not show) and `broken` (no source).
async fn named() -> Result<Fixture, String> {
    let f = fixture().await?;
    let sta = json!([
        {"name": "slow", "filter": {"q": "speed<50"},
         "sources": [{"type": "Car", "mapping": {"datastreams": ["speed"]}}]},
        {"name": "cars", "sources": [{"type": "Car"}]},
        {"name": "hidden", "sources": [{"type": "Radar"}]},
        {"name": "broken", "sources": []}
    ]);
    let view = json!({"id": "out", "tenant": "city", "selector": ["Car", "Stop", "Device"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "NO2", "location", "name", "refDevice"]},
                  {"types": ["Stop", "Device"], "attributes": ["name", "location"]}],
        "operations": ["query_entities", "temporal_retrieve", "temporal_query"],
        "representations": {"sta": sta}});
    let every = json!({"id": "all", "tenant": "city", "selector": ["*"],
        "rules": [{"types": ["*"], "attributes": ["*"]}],
        "operations": ["query_entities", "temporal_retrieve", "temporal_query"],
        "representations": {"sta": [{"name": "cars", "sources": [{"type": "Car"}]}]}});
    for (id, view) in [("out", view), ("all", every)] {
        let a = send(&f.app, "PUT", &format!("/q/views/{id}"), &view.to_string()).await;
        if a.status != StatusCode::CREATED {
            return Err(format!("view {id}: {} {}", a.status, a.body));
        }
    }
    Ok(f)
}

async fn get(f: &Fixture, output: &str, path: &str) -> (StatusCode, Value) {
    let a = send(
        &f.app,
        "GET",
        &format!("/x/views/out/sta/{output}/v1.1{path}"),
        "",
    )
    .await;
    (a.status, a.body)
}

#[tokio::test]
async fn a_filtered_output_never_holds_an_entity_outside_its_filter_by_query_or_by_key(
) -> Result<(), String> {
    let f = named().await?;
    let (status, things) = get(&f, "slow", "/Things").await;
    assert_eq!(
        (status, ids(&things)),
        (StatusCode::OK, vec!["urn:ngsi-ld:Car:1".to_owned()])
    );
    for outside in [
        "/Things('urn:ngsi-ld:Car:2')",
        "/Things('urn:ngsi-ld:Stop:1')",
        "/Datastreams('urn:ngsi-ld:Car:2%2Fspeed')",
        "/Datastreams('urn:ngsi-ld:Car:2%2Fspeed')/Observations",
        "/Things('urn:ngsi-ld:Car:2')/Datastreams",
        "/Things('urn:ngsi-ld:Car:2')/HistoricalLocations",
        "/HistoricalLocations('urn:ngsi-ld:Car:2%2FhistoricalLocation%2F2026-01-01T00:00:00Z')",
        "/HistoricalLocations('urn:ngsi-ld:Car:1%2FhistoricalLocation%2F2020-01-01T00:00:00Z')",
    ] {
        assert_eq!(
            get(&f, "slow", outside).await.0,
            StatusCode::NOT_FOUND,
            "{outside}"
        );
    }
    let (_, by_id) = get(&f, "slow", "/Things?$filter=id%20eq%20'urn:ngsi-ld:Car:2'").await;
    assert_eq!(ids(&by_id), Vec::<String>::new());
    // `properties/type eq` narrows the output's own types: a type the view shows but the
    // output does not list stays outside it
    let (_, car) = get(&f, "slow", "/Things?$filter=properties/type%20eq%20'Car'").await;
    assert_eq!(ids(&car), vec!["urn:ngsi-ld:Car:1".to_owned()]);
    let (status, stop) = get(&f, "slow", "/Things?$filter=properties/type%20eq%20'Stop'").await;
    assert_eq!((status, ids(&stop)), (StatusCode::OK, Vec::<String>::new()));
    let (_, streams) = get(&f, "slow", "/Datastreams").await;
    assert_eq!(ids(&streams), vec!["urn:ngsi-ld:Car:1/speed"]);
    // a numeric Property the role does not name is no Datastream, by key or by its history
    for unnamed in [
        "/Datastreams('urn:ngsi-ld:Car:1%2FNO2')",
        "/Datastreams('urn:ngsi-ld:Car:1%2FNO2')/Observations",
        "/Observations('urn:ngsi-ld:Car:1%2FNO2%2F2020-01-01T00:00:00Z')",
    ] {
        assert_eq!(
            get(&f, "slow", unnamed).await.0,
            StatusCode::NOT_FOUND,
            "{unnamed}"
        );
    }
    let (status, series) = get(
        &f,
        "slow",
        "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations",
    )
    .await;
    assert_eq!((status, ids(&series).len()), (StatusCode::OK, 3));
    // the Device and the Stop are no source of the output, and each of its Datastreams names a
    // Device, so it lists no Sensor, not even the unknown one (T-5205, T-5916)
    for (set, want) in [
        ("/Sensors", Vec::<String>::new()),
        (
            "/Locations?$filter=id%20eq%20'urn:ngsi-ld:Stop:1'",
            Vec::new(),
        ),
    ] {
        let (status, body) = get(&f, "slow", set).await;
        assert_eq!((status, ids(&body)), (StatusCode::OK, want), "{set}");
    }
    Ok(())
}

#[tokio::test]
async fn an_output_without_a_role_serves_every_measurement_and_its_root_names_its_own_sets(
) -> Result<(), String> {
    let f = named().await?;
    let (_, streams) = get(&f, "cars", "/Datastreams").await;
    assert_eq!(
        ids(&streams),
        vec![
            "urn:ngsi-ld:Car:1/NO2",
            "urn:ngsi-ld:Car:1/NO2#urn:ngsi-ld:Dataset:a",
            "urn:ngsi-ld:Car:1/speed",
            "urn:ngsi-ld:Car:2/speed"
        ]
    );
    let (status, root) = get(&f, "cars", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        at(&root, "/value/0/url"),
        "http://localhost:9090/x/views/out/sta/cars/v1.1/Things"
    );
    let (_, thing) = get(&f, "cars", "/Things('urn:ngsi-ld:Car:1')").await;
    assert_eq!(
        at(&thing, "/@iot.selfLink"),
        "http://localhost:9090/x/views/out/sta/cars/v1.1/Things('urn:ngsi-ld:Car:1')"
    );
    // the view's own service is unchanged beside its outputs
    let own = send(&f.app, "GET", "/x/views/out/sta/v1.1/Things", "").await;
    assert_eq!(ids(&own.body).len(), 4);
    // on a view of every type the output names its types as written
    let every = send(&f.app, "GET", "/x/views/all/sta/cars/v1.1/Things", "").await;
    assert_eq!(
        ids(&every.body),
        vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"]
    );
    Ok(())
}

#[tokio::test]
async fn an_output_that_does_not_hold_or_is_not_listed_is_a_404_and_every_output_is_read_only(
) -> Result<(), String> {
    let f = named().await?;
    for output in ["hidden", "broken", "nope", "preview"] {
        assert_eq!(
            get(&f, output, "/Things").await.0,
            StatusCode::NOT_FOUND,
            "{output}"
        );
    }
    let city = send(&f.app, "GET", "/x/views/city/sta/cars/v1.1/Things", "").await;
    assert_eq!(city.status, StatusCode::NOT_FOUND);
    let post = send(&f.app, "POST", "/x/views/out/sta/cars/v1.1/Things", "{}").await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    Ok(())
}
