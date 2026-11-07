// SPDX-License-Identifier: EUPL-1.2
//! One test per entity set of the Sensing profile, read through the `city` view: the ids the
//! gateway made (rule 2 and the three formerly empty sets changed), and nothing the view hides.

use crate::common::{at, fixture, ids, local, send, sta, T1_ID};
use axum::http::StatusCode;
use serde_json::json;

const BASE: &str = "http://localhost:9090/x/views/city/sta/v1.1";

#[tokio::test]
async fn the_service_document_lists_the_eight_sets_under_the_requests_own_base(
) -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "").await;
    assert_eq!(a.status, StatusCode::OK);
    let names: Vec<_> = a
        .body
        .get("value")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .map(|s| at(s, "/name"))
        .collect();
    assert_eq!(names, antares_sta::SETS.map(|s| json!(s)));
    assert_eq!(at(&a.body, "/value/0/url"), format!("{BASE}/Things"));
    assert_eq!(sta(&f.app, "/").await.body, a.body);
    let info = antares_ops::Surface::version_info(&antares_sta::Sta);
    assert_eq!(at(&info, "/endpoint"), "/x/views/{view}/sta/v1.1");
    assert_eq!(
        at(&info, "/conformsTo"),
        at(&a.body, "/serverSettings/conformance")
    );
    Ok(())
}

#[tokio::test]
async fn things_are_the_entities_of_the_shown_types_and_a_hidden_one_is_unknown(
) -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Things?$count=true").await;
    assert_eq!(
        ids(&a.body),
        [
            "urn:ngsi-ld:Car:1",
            "urn:ngsi-ld:Car:2",
            "urn:ngsi-ld:Device:1",
            "urn:ngsi-ld:Stop:1",
            "urn:ngsi-ld:Vehicle:1"
        ]
    );
    assert_eq!(at(&a.body, "/@iot.count"), 5);
    let car = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:1')?$expand=Locations,Datastreams",
    )
    .await;
    assert_eq!(at(&car.body, "/name"), "Žltá");
    assert_eq!(at(&car.body, "/properties/secret"), json!(null));
    assert_eq!(
        at(&car.body, "/Locations/0/@iot.id"),
        "urn:ngsi-ld:Car:1/location"
    );
    assert_eq!(at(&car.body, "/Datastreams@iot.count"), 3);
    let hidden = sta(&f.app, "/Things('urn:ngsi-ld:Depot:1')").await;
    let unknown = sta(&f.app, "/Things('urn:ngsi-ld:Car:9')").await;
    assert_eq!(
        (hidden.status, &hidden.body),
        (StatusCode::NOT_FOUND, &unknown.body)
    );
    assert_eq!(
        sta(&f.app, "/Things('car-1')").await.status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn a_view_of_every_type_reads_the_whole_tenant() -> Result<(), String> {
    let f = fixture().await?;
    let a = send(&f.app, "GET", "/x/views/every/sta/v1.1/Things", "").await;
    assert_eq!(ids(&a.body).len(), 7);
    assert!(ids(&a.body).contains(&"urn:ngsi-ld:Depot:1".to_owned()));
    Ok(())
}

#[tokio::test]
async fn locations_are_the_entities_location_by_set_by_id_and_by_thing() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Locations").await;
    assert_eq!(
        ids(&a.body),
        [
            "urn:ngsi-ld:Car:1/location",
            "urn:ngsi-ld:Stop:1/location",
            "urn:ngsi-ld:Vehicle:1/location"
        ]
    );
    let one = sta(&f.app, "/Locations('urn:ngsi-ld:Stop:1%2Flocation')").await;
    assert_eq!(at(&one.body, "/location/coordinates"), json!([17.2, 48.2]));
    let of = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/Locations?$count=true").await;
    assert_eq!(
        (ids(&of.body).len(), at(&of.body, "/@iot.count")),
        (1, json!(1))
    );
    assert_eq!(
        sta(&f.app, "/Locations('urn:ngsi-ld:Car:2/location')")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn historical_locations_come_from_the_location_history() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/HistoricalLocations?$count=true").await;
    let moved = format!("urn:ngsi-ld:Vehicle:1/historicalLocation/{T1_ID}");
    assert_eq!(ids(&a.body).len(), 3);
    assert!(ids(&a.body).contains(&moved));
    assert_eq!(at(&a.body, "/@iot.count"), 3);
    let one = sta(&f.app, &format!("/HistoricalLocations('{moved}')")).await;
    assert_eq!(at(&one.body, "/time"), T1_ID);
    assert_eq!(
        at(&one.body, "/Thing@iot.navigationLink"),
        format!("{BASE}/Things('urn:ngsi-ld:Vehicle:1')")
    );
    let of = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Vehicle:1')/HistoricalLocations?$top=2&$count=true",
    )
    .await;
    assert_eq!(ids(&of.body).len(), 2);
    assert_eq!(at(&of.body, "/@iot.count"), json!(null));
    assert!(at(&of.body, "/@iot.nextLink")
        .as_str()
        .is_some_and(|l| l.contains("$skip=2")));
    let whole = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Vehicle:1')/HistoricalLocations?$count=true",
    )
    .await;
    assert_eq!(at(&whole.body, "/@iot.count"), 3);
    for missing in [
        "/HistoricalLocations('urn:ngsi-ld:Vehicle:1/historicalLocation/2020-01-01T00:00:00Z')",
        "/HistoricalLocations('x')",
        "/Things('urn:ngsi-ld:Car:9')/HistoricalLocations",
    ] {
        assert_eq!(
            sta(&f.app, missing).await.status,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn datastreams_are_one_per_measured_instance_and_a_hidden_attribute_has_none(
) -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Datastreams").await;
    assert_eq!(
        ids(&a.body),
        [
            "urn:ngsi-ld:Car:1/NO2",
            "urn:ngsi-ld:Car:1/NO2#urn:ngsi-ld:Dataset:a",
            "urn:ngsi-ld:Car:1/speed",
            "urn:ngsi-ld:Car:2/speed"
        ]
    );
    let speed = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')").await;
    assert_eq!(
        at(&speed.body, "/unitOfMeasurement/definition"),
        "https://vocabulary.uncefact.org/UnitMeasureCode#KMH"
    );
    let dataset = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:1%2FNO2%23urn:ngsi-ld:Dataset:a')",
    )
    .await;
    assert_eq!(at(&dataset.body, "/name"), "NO2");
    let secret = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fsecret')").await;
    let unknown = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:9%2Fspeed')").await;
    assert_eq!(
        (secret.status, &secret.body),
        (StatusCode::NOT_FOUND, &unknown.body)
    );
    let of = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/Datastreams").await;
    assert_eq!(ids(&of.body).len(), 3);
    assert_eq!(
        sta(&f.app, "/Things('urn:ngsi-ld:Car:9')/Datastreams")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn sensors_are_the_shown_devices_the_entities_point_at() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Sensors").await;
    // T-5205 finding 7: the Sensor of the Datastreams that name no Device comes first, and only
    // while one does (T-5916): every Entity of `city` with a Datastream names a Device.
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Device:1"]);
    assert_eq!(at(&a.body, "/value/0/metadata"), json!(["speed"]));
    let one = sta(&f.app, "/Sensors('urn:ngsi-ld:Device:1')").await;
    assert_eq!(at(&one.body, "/name"), "radar 1");
    assert_eq!(
        sta(&f.app, "/Sensors('urn:ngsi-ld:Radar:1')").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sta(&f.app, "/Sensors('urn:ngsi-ld:Car:1')").await.status,
        StatusCode::NOT_FOUND
    );
    // T-5205 finding 8: a Sensor's name is no Entity Attribute this surface filters on.
    // T-5918: it is judged over the Sensors the view answered.
    let none = sta(&f.app, "/Sensors?$filter=name eq 'Hlavná'").await;
    assert_eq!((none.status, ids(&none.body).len()), (StatusCode::OK, 0));
    let radar = sta(&f.app, "/Sensors?$filter=name eq 'radar 1'").await;
    assert_eq!(ids(&radar.body), ["urn:ngsi-ld:Device:1"]);
    Ok(())
}

#[tokio::test]
async fn observations_are_the_current_instants_and_a_datastreams_its_history() -> Result<(), String>
{
    let f = fixture().await?;
    let a = sta(&f.app, "/Observations").await;
    let now = format!("urn:ngsi-ld:Car:1/speed/{T1_ID}");
    assert!(ids(&a.body).contains(&now));
    assert!(!ids(&a.body).iter().any(|id| id.contains("secret")));
    assert_eq!(
        at(
            &sta(&f.app, &format!("/Observations('{now}')")).await.body,
            "/result"
        ),
        10
    );
    let series = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$orderby=phenomenonTime desc&$count=true").await;
    assert_eq!(ids(&series.body).first(), Some(&now));
    assert_eq!(at(&series.body, "/@iot.count"), 3);
    let window = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=phenomenonTime ge '2025-12-31T00:00:00Z' and phenomenonTime lt '2026-01-01T00:00:00Z'").await;
    assert_eq!(
        ids(&window.body),
        ["urn:ngsi-ld:Car:1/speed/2025-12-31T00:00:00.000Z"]
    );
    let page = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$top=1&$skip=1",
    )
    .await;
    let next = at(&page.body, "/@iot.nextLink");
    assert_eq!(
        next,
        format!("{BASE}/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?%24top=1&$skip=2")
    );
    let last = send(&f.app, "GET", &local(next.as_str().unwrap_or_default()), "").await;
    assert_eq!(
        (ids(&last.body).len(), at(&last.body, "/@iot.nextLink")),
        (1, json!(null))
    );
    let secret = sta(
        &f.app,
        "/Datastreams('urn:ngsi-ld:Car:1%2Fsecret')/Observations",
    )
    .await;
    assert_eq!(secret.status, StatusCode::NOT_FOUND);
    let every = "/x/views/every/sta/v1.1/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations";
    assert_eq!(ids(&send(&f.app, "GET", every, "").await.body).len(), 3);
    for missing in [
        "/Datastreams('urn:ngsi-ld:Car:9%2Fspeed')/Observations",
        "/Datastreams('speed')/Observations",
    ] {
        assert_eq!(
            sta(&f.app, missing).await.status,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn one_attribute_is_one_observed_property_in_every_view() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/ObservedProperties").await;
    let speed = antares_ops::core_iri("speed");
    assert_eq!(ids(&a.body), [antares_ops::core_iri("NO2"), speed.clone()]);
    let every = send(
        &f.app,
        "GET",
        "/x/views/every/sta/v1.1/ObservedProperties",
        "",
    )
    .await;
    assert!(ids(&every.body).contains(&speed));
    let one = sta(&f.app, &format!("/ObservedProperties('{speed}')")).await;
    assert_eq!(at(&one.body, "/definition"), speed.as_str());
    let secret = antares_ops::core_iri("secret");
    assert_eq!(
        sta(&f.app, &format!("/ObservedProperties('{secret}')"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let name = antares_ops::core_iri("name");
    assert_eq!(
        sta(&f.app, &format!("/ObservedProperties('{name}')"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn features_of_interest_are_the_point_of_interest_or_the_entitys_own_location(
) -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/FeaturesOfInterest").await;
    assert_eq!(
        ids(&a.body),
        [
            "urn:ngsi-ld:Car:1/location",
            "urn:ngsi-ld:Stop:1",
            "urn:ngsi-ld:Stop:1/location",
            "urn:ngsi-ld:Vehicle:1/location"
        ]
    );
    let poi = sta(&f.app, "/FeaturesOfInterest('urn:ngsi-ld:Stop:1')").await;
    assert_eq!(at(&poi.body, "/name"), "Hlavná");
    let own = sta(
        &f.app,
        "/FeaturesOfInterest('urn:ngsi-ld:Car:1%2Flocation')",
    )
    .await;
    assert_eq!(at(&own.body, "/feature/type"), "Point");
    for missing in [
        "/FeaturesOfInterest('urn:ngsi-ld:Car:2/location')",
        "/FeaturesOfInterest('urn:ngsi-ld:Depot:1')",
        "/FeaturesOfInterest('urn:ngsi-ld:Car:2')",
    ] {
        assert_eq!(
            sta(&f.app, missing).await.status,
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
    Ok(())
}

/// A client follows the links an answer gives (T-5013): the `#` of a datasetId Datastream id
/// is encoded in its selfLink and navigationLinks, so the link reads that Datastream, its
/// Observations and its Thing, never the default instance a raw `#` would cut it down to.
#[tokio::test]
async fn links_of_a_dataset_datastream_lead_back_to_it() -> Result<(), String> {
    let f = fixture().await?;
    let id = "urn:ngsi-ld:Car:1/NO2#urn:ngsi-ld:Dataset:a";
    let of = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:1')/Datastreams?$expand=Observations",
    )
    .await;
    let stream = of
        .body
        .get("value")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|s| at(s, "/@iot.id") == id)
        .cloned()
        .ok_or_else(|| format!("no {id}: {}", of.body))?;
    let follow = |link: serde_json::Value| {
        let link = link.as_str().unwrap_or_default().to_owned();
        assert!(!link.contains('#'), "{link}");
        link.strip_prefix(BASE).map(str::to_owned).unwrap_or(link)
    };
    let own = sta(&f.app, &follow(at(&stream, "/@iot.selfLink"))).await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.body);
    assert_eq!(at(&own.body, "/@iot.id"), id);
    // NO2 carries no observedAt, so its history is empty: the link reads that Datastream's
    // series (200), where a raw `#` would leave `Datastreams('…/NO2` (404)
    let series = sta(
        &f.app,
        &follow(at(&stream, "/Observations@iot.navigationLink")),
    )
    .await;
    assert_eq!(series.status, StatusCode::OK, "{}", series.body);
    let observation = &at(&stream, "/Observations/0");
    assert_eq!(at(observation, "/result"), json!(16.0), "{stream}");
    let back = sta(
        &f.app,
        &follow(at(observation, "/Datastream@iot.navigationLink")),
    )
    .await;
    assert_eq!(at(&back.body, "/@iot.id"), id);
    let own_link = sta(&f.app, &follow(at(observation, "/@iot.selfLink"))).await;
    assert_eq!(own_link.status, StatusCode::OK, "{}", own_link.body);
    assert_eq!(at(&own_link.body, "/result"), json!(16.0));
    let thing = sta(&f.app, &follow(at(&stream, "/Thing@iot.navigationLink"))).await;
    assert_eq!(at(&thing.body, "/@iot.id"), "urn:ngsi-ld:Car:1");
    Ok(())
}
