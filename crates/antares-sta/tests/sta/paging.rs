// SPDX-License-Identifier: EUPL-1.2
//! The read path prod failed on (T-4928): an Observation always has its `phenomenonTime`,
//! `$top` counts resources on a set an Entity holds several of and `@iot.nextLink` resumes
//! without a repeat, `$filter` on the id is answered by id, and a Datastream inlines its
//! Observations under `$expand`.

use crate::common::{at, fixture, fixture_with, ids, local, send, sta};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use serde_json::{json, Value};

/// An id as a query literal: a `#` would end the request target.
fn literal(id: &str) -> String {
    format!("'{}'", id.replace('#', "%23"))
}

#[tokio::test]
async fn an_observation_of_a_value_no_sensor_timed_carries_when_it_was_set() -> Result<(), String> {
    let f = fixture().await?;
    let all = sta(&f.app, "/Observations").await;
    let values = all
        .body
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Car:2's speed and Car:1's NO2 instances carry no observedAt.
    assert_eq!(values.len(), 4, "{:?}", ids(&all.body));
    for o in &values {
        let time = at(o, "/phenomenonTime");
        assert!(time.as_str().is_some_and(|t| !t.is_empty()), "{o}");
        let id = at(o, "/@iot.id");
        assert!(
            id.as_str()
                .is_some_and(|i| i.ends_with(time.as_str().unwrap_or("-"))),
            "{o}"
        );
    }
    let car2 = ids(&all.body)
        .into_iter()
        .find(|i| i.starts_with("urn:ngsi-ld:Car:2/speed/"))
        .unwrap_or_default();
    let one = sta(&f.app, &format!("/Observations({})", literal(&car2))).await;
    assert_eq!(
        (one.status, at(&one.body, "/result")),
        (StatusCode::OK, 60.into())
    );
    Ok(())
}

#[tokio::test]
async fn top_counts_resources_and_the_next_link_resumes_without_a_repeat() -> Result<(), String> {
    let f = fixture().await?;
    for set in ["Observations", "Datastreams"] {
        let whole = ids(&sta(&f.app, &format!("/{set}")).await.body);
        assert!(whole.len() > 2, "{set}: {whole:?}");
        let mut walked = Vec::new();
        let mut next = format!("/x/views/city/sta/v1.1/{set}?$top=1");
        for _ in 0..10 {
            let page = send(&f.app, "GET", &next, "").await;
            assert_eq!(page.status, StatusCode::OK, "{next}");
            let got = ids(&page.body);
            assert!(got.len() <= 1, "{set} $top=1 answered {got:?}");
            walked.extend(got);
            match at(&page.body, "/@iot.nextLink").as_str() {
                Some(link) => next = local(link),
                None => break,
            }
        }
        assert_eq!(walked, whole, "{set}");
        let skipped = ids(&sta(&f.app, &format!("/{set}?$skip=1&$top=2")).await.body);
        assert_eq!(
            skipped,
            whole.get(1..3).unwrap_or_default(),
            "{set} $skip=1"
        );
        let past = sta(&f.app, &format!("/{set}?$skip=50")).await;
        assert_eq!((past.status, ids(&past.body).len()), (StatusCode::OK, 0));
    }
    let bad = sta(&f.app, "/Observations?$skiptoken=1").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn an_id_filter_is_read_by_id_and_its_negation_excludes_it() -> Result<(), String> {
    let f = fixture().await?;
    for set in ["Observations", "Datastreams", "Things"] {
        let whole = ids(&sta(&f.app, &format!("/{set}")).await.body);
        for id in &whole {
            let is = sta(&f.app, &format!("/{set}?$filter=id eq {}", literal(id))).await;
            assert_eq!(
                (is.status, ids(&is.body)),
                (StatusCode::OK, vec![id.clone()]),
                "{set} {id}"
            );
            let not = sta(&f.app, &format!("/{set}?$filter=id ne {}", literal(id))).await;
            let rest: Vec<String> = whole.iter().filter(|i| *i != id).cloned().collect();
            assert_eq!(ids(&not.body), rest, "{set} ne {id}");
        }
    }
    let unknown = sta(
        &f.app,
        "/Observations?$filter=id eq 'urn:ngsi-ld:Car:9/speed/x'",
    )
    .await;
    assert_eq!(
        (unknown.status, ids(&unknown.body).len()),
        (StatusCode::OK, 0)
    );
    let hidden = sta(
        &f.app,
        "/Observations?$filter=@iot.id eq 'urn:ngsi-ld:Car:1/secret'",
    )
    .await;
    assert_eq!(ids(&hidden.body).len(), 0);
    // T-5918: an id order is judged over the collection; a related resource's property
    // beside an `or` is no `q` and no member of an Observation.
    let ordered = sta(&f.app, "/Observations?$filter=id gt 'urn:ngsi-ld:Car:2'").await;
    assert_eq!(ordered.status, StatusCode::OK);
    assert!(ids(&ordered.body)
        .iter()
        .all(|i| i.as_str() > "urn:ngsi-ld:Car:2"));
    for refused in [
        "Datastream/Thing/name gt 1 or id eq 'a'",
        "id eq 'a' and Datastream/Thing/name gt 1",
    ] {
        let r = sta(&f.app, &format!("/Observations?$filter={refused}")).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{refused}");
    }
    Ok(())
}

#[tokio::test]
async fn a_datastream_inlines_its_observations_under_expand() -> Result<(), String> {
    let f = fixture().await?;
    let page = sta(&f.app, "/Datastreams?$expand=Observations($top=1)&$top=1").await;
    assert_eq!(page.status, StatusCode::OK);
    let first = at(&page.body, "/value/0");
    let inlined = first
        .get("Observations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(inlined.len(), 1, "{first}");
    assert_eq!(
        at(
            &at(&first, "/Observations/0"),
            "/Datastream@iot.navigationLink"
        ),
        at(&first, "/@iot.selfLink")
    );
    let plain = sta(&f.app, "/Datastreams?$top=1").await;
    assert!(at(&plain.body, "/value/0").get("Observations").is_none());
    Ok(())
}

#[tokio::test]
async fn a_skip_walks_pages_of_entities_up_to_its_bound() -> Result<(), String> {
    let cars: Vec<Value> = (0..1050)
        .map(|n| json!({"id": format!("urn:ngsi-ld:Car:{n:04}"), "type": "Car", "speed": {"type": "Property", "value": n}}))
        .collect();
    let f = fixture_with(TestBroker::new().await?, &cars, &[]).await?;
    // One Observation per Entity: a resource offset is the Entity offset.
    for skip in [150, 1000] {
        let walked = sta(&f.app, &format!("/Observations?$skip={skip}&$top=2")).await;
        let resumed = sta(&f.app, &format!("/Observations?$skiptoken={skip}.0&$top=2")).await;
        assert_eq!(walked.status, StatusCode::OK, "{skip}: {}", walked.body);
        assert_eq!(ids(&walked.body), ids(&resumed.body), "{skip}");
        assert_eq!(ids(&walked.body).len(), 2, "{skip}");
    }
    let deep = sta(&f.app, "/Observations?$skip=1001").await;
    assert_eq!(deep.status, StatusCode::BAD_REQUEST);
    assert!(at(&deep.body, "/error/message")
        .as_str()
        .is_some_and(|m| m.contains("@iot.nextLink")));
    // T-5918: a filter or an order judged over the collection reads at most 1 000 resources.
    for over in [
        "/Observations?$filter=result gt 5",
        "/Things?$orderby=name",
        "/Datastreams?$filter=name eq 'speed'&$top=1",
    ] {
        let refused = sta(&f.app, over).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{over}");
        assert!(
            at(&refused.body, "/error/message")
                .as_str()
                .is_some_and(|m| m.contains("at most 1000 resources")),
            "{over}: {}",
            refused.body
        );
    }
    // T-5918: a generic `$expand` reads at most 500 related resources per request.
    let wide = sta(&f.app, "/Observations?$top=100&$expand=Datastream/Thing,FeatureOfInterest/Observations,Datastream/Sensor,Datastream/ObservedProperty,Datastream/Observations").await;
    assert_eq!(
        (wide.status, at(&wide.body, "/error/message")),
        (
            StatusCode::BAD_REQUEST,
            json!("$expand reads at most 500 related resources per request: lower $top")
        )
    );
    // A known count is the operation's only while no id is left out after the read.
    let counted = sta(&f.app, "/Things?$count=true&$top=1").await;
    assert_eq!(at(&counted.body, "/@iot.count"), 1050);
    let not = sta(
        &f.app,
        "/Things?$count=true&$top=1&$filter=id ne 'urn:ngsi-ld:Car:0000'",
    )
    .await;
    assert_eq!(at(&not.body, "/@iot.count"), Value::Null);
    // Another set holds no Entity count: a first page that is not the whole answer has none.
    let located = sta(&f.app, "/Locations?$count=true&$top=1").await;
    assert_eq!(at(&located.body, "/@iot.count"), Value::Null);
    Ok(())
}
