// SPDX-License-Identifier: EUPL-1.2
//! SensorThings 1.1 req/resource-path through the `city` view (T-4878): every navigation link
//! a resource carries answers, a member path answers `{name: value}`, and an earlier
//! Observation reads by its own id. The OGC ETS (ets-sta10) checks the links exist; these
//! tests check where they lead.

use crate::common::{at, fixture, ids, send, sta};
use axum::http::StatusCode;
use serde_json::{json, Value};

const BASE: &str = "http://localhost:9090/x/views/city/sta/v1.1";

/// A link of this surface as the path below the view's base, its `#` sent as `%23`.
fn path(link: &Value) -> String {
    let link = link.as_str().unwrap_or_default();
    link.strip_prefix(BASE).unwrap_or(link).replace('#', "%23")
}

#[tokio::test]
async fn every_navigation_link_of_every_set_answers() -> Result<(), String> {
    let f = fixture().await?;
    let mut followed = 0;
    for set in antares_sta::SETS {
        let list = sta(&f.app, &format!("/{set}")).await;
        assert_eq!(list.status, StatusCode::OK, "{set}");
        for item in list
            .body
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let links = item.as_object().into_iter().flatten();
            for (name, link) in links.filter(|(n, _)| n.ends_with("@iot.navigationLink")) {
                let a = sta(&f.app, &path(link)).await;
                // Car:2 names the hidden Radar:1 as its Device: no Sensor the view shows.
                let hidden = link
                    .as_str()
                    .is_some_and(|l| l.contains("Car:2%2Fspeed')/Sensor"));
                let want = if hidden {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::OK
                };
                assert_eq!(a.status, want, "{set} {name} {link}: {}", a.body);
                followed += 1;
            }
        }
    }
    assert!(followed > 40, "{followed} links followed");
    Ok(())
}

/// T-5918: every navigation path one and two deep expands on every set, inlined as an array
/// for a collection and an object for a single resource (15-078r6 9.3.2.1; ets-sta10
/// Capability3Tests.readEntitiesWithExpandQO); a path that is none is refused by name.
#[tokio::test]
async fn every_navigation_path_expands_on_every_set() -> Result<(), String> {
    let f = fixture().await?;
    for (set, path, kind) in [
        ("Things", "HistoricalLocations/Locations", "array"),
        ("Things", "Datastreams/Sensor", "array"),
        ("Locations", "Things/Datastreams", "array"),
        ("HistoricalLocations", "Thing/Locations", "object"),
        ("Datastreams", "ObservedProperty/Datastreams", "object"),
        ("Datastreams", "Sensor", "object"),
        ("Sensors", "Datastreams/Observations", "array"),
        ("ObservedProperties", "Datastreams/Thing", "array"),
        ("Observations", "FeatureOfInterest/Observations", "object"),
        ("FeaturesOfInterest", "Observations/Datastream", "array"),
    ] {
        let head = path.split('/').next().unwrap_or_default();
        let a = sta(&f.app, &format!("/{set}?$expand={path}&$select=id")).await;
        assert_eq!(a.status, StatusCode::OK, "{set} {path}: {}", a.body);
        let first = at(&a.body, "/value/0");
        let inlined = first.get(head).cloned().unwrap_or_default();
        let shape = if inlined.is_array() {
            "array"
        } else if inlined.is_object() {
            "object"
        } else {
            "none"
        };
        assert_eq!(shape, kind, "{set} {path}: {first}");
        // `$select=id` keeps the id and the expanded member, and no navigation link.
        assert!(
            first.get("@iot.selfLink").is_none(),
            "{set} {path}: {first}"
        );
    }
    let one = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:1')?$expand=HistoricalLocations",
    )
    .await;
    assert!(
        at(&one.body, "/HistoricalLocations").is_array(),
        "{}",
        one.body
    );
    // a head an earlier path inlined is read once and walked again
    let twice = sta(
        &f.app,
        "/Things?$expand=HistoricalLocations,HistoricalLocations/Thing",
    )
    .await;
    assert_eq!(twice.status, StatusCode::OK, "{}", twice.body);
    // a related collection expands too, each of its resources
    let related = sta(
        &f.app,
        "/Sensors('urn:ngsi-ld:Device:1')/Datastreams?$expand=Observations",
    )
    .await;
    let streams = at(&related.body, "/value")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!streams.is_empty(), "{}", related.body);
    assert!(
        streams
            .iter()
            .all(|d| d.get("Observations").is_some_and(Value::is_array)),
        "{}",
        related.body
    );
    let bad = sta(&f.app, "/Sensors?$expand=Thing").await;
    assert_eq!(
        (bad.status, at(&bad.body, "/error/message")),
        (
            StatusCode::BAD_REQUEST,
            json!("$expand: Thing is not a navigation path of Sensors")
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_single_navigation_answers_the_entity_and_a_collection_its_page() -> Result<(), String> {
    let f = fixture().await?;
    let stream = "/Datastreams('urn:ngsi-ld:Car:1/speed')";
    let sensor = sta(&f.app, &format!("{stream}/Sensor")).await;
    assert_eq!(at(&sensor.body, "/@iot.id"), "urn:ngsi-ld:Device:1");
    let thing = sta(&f.app, &format!("{stream}/Thing")).await;
    assert_eq!(at(&thing.body, "/@iot.id"), "urn:ngsi-ld:Car:1");
    let property = sta(&f.app, &format!("{stream}/ObservedProperty")).await;
    assert_eq!(at(&property.body, "/name"), "speed");

    let streams = sta(&f.app, "/Sensors('urn:ngsi-ld:Device:1')/Datastreams").await;
    let mut got = ids(&streams.body);
    got.sort();
    assert_eq!(
        got,
        [
            "urn:ngsi-ld:Car:1/NO2",
            "urn:ngsi-ld:Car:1/NO2#urn:ngsi-ld:Dataset:a",
            "urn:ngsi-ld:Car:1/speed"
        ]
    );
    let measuring = sta(
        &f.app,
        &path(&at(&property.body, "/Datastreams@iot.navigationLink")),
    )
    .await;
    let mut got = ids(&measuring.body);
    got.sort();
    assert_eq!(got, ["urn:ngsi-ld:Car:1/speed", "urn:ngsi-ld:Car:2/speed"]);

    let moves = sta(
        &f.app,
        "/Locations('urn:ngsi-ld:Vehicle:1/location')/HistoricalLocations",
    )
    .await;
    assert_eq!(ids(&moves.body).len(), 3);
    let things = sta(
        &f.app,
        "/Locations('urn:ngsi-ld:Vehicle:1/location')/Things",
    )
    .await;
    assert_eq!(ids(&things.body), ["urn:ngsi-ld:Vehicle:1"]);

    // Car:2 points at Stop:1: its Observations are that feature's; Car:1's own location is its.
    let at_stop = sta(
        &f.app,
        "/FeaturesOfInterest('urn:ngsi-ld:Stop:1')/Observations",
    )
    .await;
    // No observedAt: the Observation's time is when the value was set (`modifiedAt`).
    let Ok([oid]) = <[String; 1]>::try_from(ids(&at_stop.body)) else {
        return Err(format!("one Observation at Stop:1: {}", at_stop.body));
    };
    assert!(oid.starts_with("urn:ngsi-ld:Car:2/speed/"), "{oid}");
    let feature = sta(&f.app, &format!("/Observations('{oid}')/FeatureOfInterest")).await;
    assert_eq!(at(&feature.body, "/@iot.id"), "urn:ngsi-ld:Stop:1");
    Ok(())
}

#[tokio::test]
async fn a_member_path_and_an_earlier_observation_read_by_id() -> Result<(), String> {
    let f = fixture().await?;
    let name = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/name").await;
    assert_eq!(
        (name.status, name.body),
        (StatusCode::OK, json!({"name": "Žltá"}))
    );

    let earlier = "/Observations('urn:ngsi-ld:Car:1/speed/2025-12-30T00:00:00.000Z')";
    let a = sta(&f.app, earlier).await;
    assert_eq!(
        (a.status, at(&a.body, "/result")),
        (StatusCode::OK, json!(8))
    );
    let stream = sta(&f.app, &format!("{earlier}/Datastream")).await;
    assert_eq!(at(&stream.body, "/@iot.id"), "urn:ngsi-ld:Car:1/speed");
    // Depot:1 measures `capacity` and names no Device: its Datastream's Sensor is the unknown one
    // (T-5205 finding 7), whose Datastreams are those of every Entity naming no Device.
    let no_device = "/x/views/every/sta/v1.1/Datastreams('urn:ngsi-ld:Depot:1/capacity')";
    assert_eq!(
        send(&f.app, "GET", no_device, "").await.status,
        StatusCode::OK
    );
    let sensor = send(&f.app, "GET", &format!("{no_device}/Sensor"), "").await;
    assert_eq!(at(&sensor.body, "/@iot.id"), "unknown");
    let streams = send(
        &f.app,
        "GET",
        "/x/views/every/sta/v1.1/Sensors('unknown')/Datastreams",
        "",
    )
    .await;
    assert!(ids(&streams.body).contains(&"urn:ngsi-ld:Depot:1/capacity".to_owned()));
    assert!(!ids(&streams.body)
        .iter()
        .any(|i| i.starts_with("urn:ngsi-ld:Car:1/")));
    let unknown = sta(
        &f.app,
        "/Observations('urn:ngsi-ld:Car:1/speed/2001-01-01T00:00:00Z')",
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// `{member}/$value` (15-078r6 9.2.6) per kind of property: a string and a number as their
/// text, an object as its JSON; a navigation property, a member the view hides and a resource
/// outside the view are the same 404.
#[tokio::test]
async fn a_property_value_is_its_raw_text() -> Result<(), String> {
    let f = fixture().await?;
    let text = Some("text/plain; charset=utf-8".to_owned());
    let name = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/name/$value").await;
    assert_eq!(
        (name.status, name.text.as_str(), name.content_type),
        (StatusCode::OK, "Žltá", text.clone())
    );
    let earlier = "/Observations('urn:ngsi-ld:Car:1/speed/2025-12-30T00:00:00.000Z')";
    let result = sta(&f.app, &format!("{earlier}/result/$value")).await;
    assert_eq!(
        (result.status, result.text.as_str(), result.content_type),
        (StatusCode::OK, "8", text)
    );
    // The percent-encoded `$` is the same path.
    let encoded = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/name/%24value").await;
    assert_eq!(encoded.text, "Žltá");
    // A null member is `204` (OData 4.0 Part 2 11.2.6.1): the `resultTime` of Car:2's speed,
    // which no sensor timed.
    let at_stop = sta(
        &f.app,
        "/FeaturesOfInterest('urn:ngsi-ld:Stop:1')/Observations",
    )
    .await;
    let untimed = format!("/Observations('{}')", ids(&at_stop.body).concat());
    let unknown = sta(&f.app, &format!("{untimed}/resultTime")).await;
    assert_eq!(unknown.body, json!({"resultTime": null}));
    let none = sta(&f.app, &format!("{untimed}/resultTime/$value")).await;
    assert_eq!(
        (none.status, none.text.as_str()),
        (StatusCode::NO_CONTENT, "")
    );

    let stream = "/Datastreams('urn:ngsi-ld:Car:1/speed')";
    let unit = sta(&f.app, &format!("{stream}/unitOfMeasurement/$value")).await;
    let member = sta(&f.app, &format!("{stream}/unitOfMeasurement")).await;
    assert_eq!(
        (unit.status, unit.content_type.as_deref(), unit.body),
        (
            StatusCode::OK,
            Some("application/json"),
            at(&member.body, "/unitOfMeasurement")
        )
    );
    assert!(unit.text.starts_with('{'), "{}", unit.text);

    // `$expand` names navigation properties, which a raw value has none of.
    let expanded = sta(
        &f.app,
        "/Things('urn:ngsi-ld:Car:1')/name/$value?$expand=Locations",
    )
    .await;
    assert_eq!(expanded.status, StatusCode::BAD_REQUEST);

    let locations = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/Locations").await;
    let Ok([location]) = <[String; 1]>::try_from(ids(&locations.body)) else {
        return Err(format!("one Location of Car:1: {}", locations.body));
    };
    let place = sta(&f.app, &format!("/Locations('{location}')/location/$value")).await;
    assert_eq!(
        (place.status, at(&place.body, "/type")),
        (StatusCode::OK, json!("Point"))
    );

    for path in [
        "/Things('urn:ngsi-ld:Car:1')/Datastreams/$value",
        "/Things('urn:ngsi-ld:Car:1')/secret/$value",
        "/Things('urn:ngsi-ld:Car:1')/Datastreams@iot.navigationLink/$value",
        "/Things('urn:ngsi-ld:Car:1')/@iot.selfLink/$value",
        "/Things('urn:ngsi-ld:Radar:1')/name/$value",
        "/Things('urn:ngsi-ld:Car:1')/name/$value/$value",
    ] {
        let a = sta(&f.app, path).await;
        assert_eq!(
            (a.status, at(&a.body, "/error/message")),
            (StatusCode::NOT_FOUND, json!("no such resource")),
            "{path}"
        );
    }
    Ok(())
}

/// `{path}/$ref` (15-078r6 9.2.7): a collection, a related collection and a related resource as
/// their self-links alone, the next link to the next page of links; a property and a hidden
/// resource have none.
#[tokio::test]
async fn an_association_link_is_the_self_links_alone() -> Result<(), String> {
    let f = fixture().await?;
    let things = sta(&f.app, "/Things").await;
    let refs = sta(&f.app, "/Things/$ref?$select=name&$expand=Locations").await;
    let links = |doc: &Value| -> Vec<Value> {
        doc.get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|t| json!({ "@iot.selfLink": at(t, "/@iot.selfLink") }))
            .collect()
    };
    assert_eq!(
        (refs.status, at(&refs.body, "/value")),
        (StatusCode::OK, Value::Array(links(&things.body)))
    );
    let counted = sta(&f.app, "/Things/$ref?$count=true").await;
    assert_eq!(
        at(&counted.body, "/@iot.count"),
        json!(links(&things.body).len())
    );
    let page = sta(&f.app, "/Things/$ref?$top=1").await;
    let next = at(&page.body, "/@iot.nextLink");
    assert!(
        next.as_str().is_some_and(|n| n.contains("/Things/$ref?")),
        "{next}"
    );
    assert_eq!(
        at(&page.body, "/value/0"),
        json!({ "@iot.selfLink": at(&things.body, "/value/0/@iot.selfLink") })
    );

    let stream = "/Datastreams('urn:ngsi-ld:Car:1/speed')";
    let thing = sta(&f.app, &format!("{stream}/Thing/$ref")).await;
    assert_eq!(
        (thing.status, thing.body),
        (
            StatusCode::OK,
            json!({ "@iot.selfLink": "http://localhost:9090/x/views/city/sta/v1.1/Things('urn:ngsi-ld:Car:1')" })
        )
    );
    let located = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/Locations/$ref").await;
    assert_eq!(
        at(&located.body, "/value/0")
            .as_object()
            .map(serde_json::Map::len),
        Some(1)
    );
    for path in [
        "/Things('urn:ngsi-ld:Car:1')/name/$ref",
        "/Things('urn:ngsi-ld:Radar:1')/Datastreams/$ref",
    ] {
        let a = sta(&f.app, path).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{path}");
    }
    Ok(())
}

/// A Sensor's `metadata` is mandatory (15-078r6 Table 8): the Sensor `unknown` answers `[]`.
/// It is a Sensor only while a Datastream names it: in `every`, Depot:1's `capacity`; in
/// `city`, where every Entity with a Datastream names a Device, it is neither listed nor read.
#[tokio::test]
async fn the_unknown_sensor_is_one_while_it_measures() -> Result<(), String> {
    let f = fixture().await?;
    let every = "/x/views/every/sta/v1.1/Sensors";
    let a = send(
        &f.app,
        "GET",
        &format!("{every}('unknown')/metadata/$value"),
        "",
    )
    .await;
    assert_eq!((a.status, a.text.as_str()), (StatusCode::OK, "[]"));
    let listed = send(&f.app, "GET", every, "").await;
    assert_eq!(
        ids(&listed.body).first().map(String::as_str),
        Some("unknown")
    );

    let city = sta(&f.app, "/Sensors").await;
    assert!(
        !ids(&city.body).contains(&"unknown".to_owned()),
        "{}",
        city.body
    );
    let one = sta(&f.app, "/Sensors('unknown')").await;
    assert_eq!(one.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// A resource path through a keyed related resource (15-078r6 9.2.4) reads that resource once
/// it is related; through an unrelated or hidden one it is the 404 of an unknown resource.
#[tokio::test]
async fn a_path_through_a_related_resource_reads_it() -> Result<(), String> {
    let f = fixture().await?;
    let through = "/Things('urn:ngsi-ld:Car:1')/Datastreams('urn:ngsi-ld:Car:1/speed')";
    let direct = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1/speed')/Thing").await;
    let a = sta(&f.app, &format!("{through}/Thing?$select=name")).await;
    assert_eq!(
        (a.status, a.body),
        (StatusCode::OK, json!({"name": at(&direct.body, "/name")}))
    );
    let name = sta(&f.app, &format!("{through}/Thing/name/$value")).await;
    assert_eq!(name.text, "Žltá");
    let earlier = "Observations('urn:ngsi-ld:Car:1/speed/2025-12-30T00:00:00.000Z')";
    let deep = sta(&f.app, &format!("{through}/{earlier}/result")).await;
    assert_eq!(
        (deep.status, deep.body),
        (StatusCode::OK, json!({"result": 8}))
    );
    // Each single navigation property followed by more, and each set reached back by one.
    let iri = at(
        &sta(
            &f.app,
            "/Datastreams('urn:ngsi-ld:Car:1/speed')/ObservedProperty",
        )
        .await
        .body,
        "/@iot.id",
    );
    let iri = iri.as_str().unwrap_or_default();
    let speed = "Datastreams('urn:ngsi-ld:Car:1/speed')";
    let at_stop = sta(
        &f.app,
        "/FeaturesOfInterest('urn:ngsi-ld:Stop:1')/Observations",
    )
    .await;
    let stop = format!("Observations('{}')", ids(&at_stop.body).concat());
    for path in [
        format!("/{earlier}/Datastream/Thing"),
        format!("/{speed}/Sensor/Datastreams"),
        format!("/{speed}/ObservedProperty/Datastreams"),
        format!("/{stop}/FeatureOfInterest/Observations"),
        format!("/Sensors('urn:ngsi-ld:Device:1')/{speed}/Thing"),
        format!("/ObservedProperties('{iri}')/{speed}/Thing"),
        format!("/FeaturesOfInterest('urn:ngsi-ld:Stop:1')/{stop}/Datastream"),
    ] {
        let a = sta(&f.app, &path).await;
        assert_eq!(a.status, StatusCode::OK, "{path}: {}", a.body);
    }
    let locations = sta(&f.app, "/Things('urn:ngsi-ld:Car:1')/Locations").await;
    let Ok([location]) = <[String; 1]>::try_from(ids(&locations.body)) else {
        return Err(format!("one Location of Car:1: {}", locations.body));
    };
    let things = sta(
        &f.app,
        &format!("/Things('urn:ngsi-ld:Car:1')/Locations('{location}')/Things/$ref"),
    )
    .await;
    assert_eq!(things.status, StatusCode::OK, "{}", things.body);

    for path in [
        "/Things('urn:ngsi-ld:Car:2')/Datastreams('urn:ngsi-ld:Car:1/speed')",
        "/Things('urn:ngsi-ld:Radar:1')/Datastreams('urn:ngsi-ld:Car:1/speed')/Thing",
        "/Things('urn:ngsi-ld:Car:1')/Datastreams('urn:ngsi-ld:Radar:1/speed')/Thing",
        "/Datastreams('urn:ngsi-ld:Radar:1/speed')/Thing/name",
        "/Things('urn:ngsi-ld:Car:2')/Locations('{location}')",
        "/Things('urn:ngsi-ld:Car:1')/Thing('urn:ngsi-ld:Car:1')",
    ] {
        let path = path.replace("{location}", &location);
        let a = sta(&f.app, &path).await;
        assert_eq!(a.status, StatusCode::NOT_FOUND, "{path}: {}", a.body);
    }
    Ok(())
}
