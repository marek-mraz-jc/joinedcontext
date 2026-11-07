// SPDX-License-Identifier: EUPL-1.2
//! The bounds and the refusals: a read-only surface, query options bounded before they are read,
//! a page cut between whole resources, and every refusal of the view passed on by its status.

use crate::common::{at, entities, fixture, fixture_with, histories, ids, local, send, sta};
use antares_ops::test_kit::TestBroker;
use axum::http::StatusCode;
use serde_json::{json, Value};

#[tokio::test]
async fn the_surface_is_read_only() -> Result<(), String> {
    let f = fixture().await?;
    let options = send(&f.app, "OPTIONS", "/x/views/city/sta/v1.1/Things", "").await;
    assert_eq!(
        (options.status, options.allow.as_deref()),
        (StatusCode::NO_CONTENT, Some("GET, HEAD, OPTIONS"))
    );
    let post = send(&f.app, "POST", "/x/views/city/sta/v1.1/Things", "{}").await;
    assert_eq!(
        (post.status, post.allow.as_deref()),
        (StatusCode::METHOD_NOT_ALLOWED, Some("GET, HEAD, OPTIONS"))
    );
    assert_eq!(at(&post.body, "/error/code"), "405");
    let head = send(&f.app, "HEAD", "/x/views/city/sta/v1.1/Things", "").await;
    assert_eq!((head.status, head.bytes), (StatusCode::OK, 0));
    Ok(())
}

#[tokio::test]
async fn an_unknown_view_set_or_path_is_a_404() -> Result<(), String> {
    let f = fixture().await?;
    let unknown = send(&f.app, "GET", "/x/views/nope/sta/v1.1/Things", "").await;
    assert_eq!(
        unknown.body,
        json!({"error": {"code": "404", "message": "no such resource"}})
    );
    for path in [
        "/Cars",
        "/Cars('x')",
        "/Things%zz",
        "/Things('urn:ngsi-ld:Car:1')/Sensors",
        "/Things/Locations",
        "/Locations('urn:ngsi-ld:Car:1/location')/Sensors",
        "/Locations('urn:ngsi-ld:Nope:1/location')/Things",
        "/Datastreams('urn:ngsi-ld:Car:1/speed')/Observations/x",
        "/Datastreams('urn:ngsi-ld:Car:1/speed')/Things",
        "/Things('urn:ngsi-ld:Car:1')/secret",
        "/Things('urn:ngsi-ld:Car:1')/Locations@iot.navigationLink",
        "/Sensors(9223372036854775807)",
        "/FeaturesOfInterest(1)",
        "/Sensors(9223372036854775807)/Datastreams",
        "/Things('urn:ngsi-ld:Car:1')/$value",
        "/Observations('urn:ngsi-ld:Nope:1/speed')",
        "/Observations('urn:ngsi-ld:Car:1')",
        "/Observations('urn:ngsi-ld:Car:1//2025-12-30T00:00:00Z')",
    ] {
        assert_eq!(
            sta(&f.app, path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn query_options_are_bounded_and_a_refusal_names_the_option() -> Result<(), String> {
    let f = fixture().await?;
    let deep = sta(
        &f.app,
        "/Things?$expand=Datastreams/Observations/FeatureOfInterest",
    )
    .await;
    assert_eq!(deep.status, StatusCode::BAD_REQUEST);
    assert!(at(&deep.body, "/error/message")
        .as_str()
        .is_some_and(|m| m.starts_with("$expand")));
    let long = sta(
        &f.app,
        &format!("/Things?$filter=name eq '{}'", "x".repeat(4100)),
    )
    .await;
    assert_eq!(long.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        sta(&f.app, "/Things?$top=many").await.status,
        StatusCode::BAD_REQUEST
    );
    let has = sta(&f.app, "/Things?$filter=name has 'x'").await;
    assert_eq!(
        at(&has.body, "/error/message"),
        "$filter: has is not supported"
    );
    // T-5918: what no time window says is judged over the Datastream's Observations.
    let series = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=result eq -1 or phenomenonTime gt 2026-01-01T00:00:00Z").await;
    assert_eq!(series.status, StatusCode::OK, "{}", series.body);
    let times = at(&series.body, "/value")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(times
        .iter()
        .all(|o| o.get("phenomenonTime").and_then(Value::as_str) > Some("2026-01-01T00:00:00Z")));
    Ok(())
}

#[tokio::test]
async fn a_filter_outside_ascii_answers_without_panicking() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Things?$filter=name eq 'Žilina'").await;
    assert_eq!((a.status, ids(&a.body).len()), (StatusCode::OK, 0));
    let b = sta(&f.app, "/Things?$filter=name eq 'Žltá'").await;
    assert_eq!(ids(&b.body), ["urn:ngsi-ld:Car:1"]);
    let c = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=Žilina eq 'x' and phenomenonTime gt 2026-01-01T00:00:00Z").await;
    assert_eq!(c.status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn a_filter_the_view_cannot_read_is_its_400() -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Things?$filter=properties/speed eq").await;
    assert_eq!(
        (a.status, at(&a.body, "/error/message")),
        (
            StatusCode::BAD_REQUEST,
            json!("the filter is not valid NGSI-LD")
        )
    );
    let b = sta(&f.app, "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations?$filter=phenomenonTime gt yesterday").await;
    assert_eq!(
        (b.status, at(&b.body, "/error/message")),
        (
            StatusCode::BAD_REQUEST,
            json!("$filter: yesterday is no property of these resources; quote a text")
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_page_carries_the_next_link_with_the_other_options_and_counts_what_it_knows(
) -> Result<(), String> {
    let f = fixture().await?;
    let a = sta(&f.app, "/Things?$top=2&$select=@iot.id,name").await;
    assert_eq!(
        at(&a.body, "/value/0"),
        json!({"@iot.id": "urn:ngsi-ld:Car:1", "name": "Žltá"})
    );
    assert_eq!(at(&a.body, "/@iot.count"), json!(null));
    let next = at(&a.body, "/@iot.nextLink");
    let next = local(next.as_str().unwrap_or_default());
    assert!(
        next.ends_with("?%24top=2&%24select=%40iot.id%2Cname&$skip=2"),
        "{next}"
    );
    let b = send(&f.app, "GET", &next, "").await;
    assert_eq!(ids(&b.body), ["urn:ngsi-ld:Device:1", "urn:ngsi-ld:Stop:1"]);
    let counted = sta(&f.app, "/Things?$top=2&$skip=4&$count=true").await;
    assert_eq!(
        (ids(&counted.body).len(), at(&counted.body, "/@iot.count")),
        (1, json!(5))
    );
    assert_eq!(at(&counted.body, "/@iot.nextLink"), json!(null));
    let partial = sta(&f.app, "/Datastreams?$top=1&$count=true").await;
    assert_eq!(at(&partial.body, "/@iot.count"), json!(null));
    assert!(partial.body.get("@iot.nextLink").is_some());
    let whole = sta(&f.app, "/Datastreams?$count=true").await;
    assert_eq!(at(&whole.body, "/@iot.count"), 4);
    // a full page links on; the broker pages its candidates before the time filter, and the
    // second of them is the Entity with a location history
    let moves = sta(&f.app, "/HistoricalLocations?$top=1&$skip=1").await;
    assert_eq!(ids(&moves.body).len(), 3);
    assert!(moves.body.get("@iot.nextLink").is_some());
    let after = sta(&f.app, "/HistoricalLocations?$top=1&$skip=2").await;
    assert_eq!(
        (ids(&after.body).len(), at(&after.body, "/@iot.nextLink")),
        (0, json!(null))
    );
    Ok(())
}

#[tokio::test]
async fn a_page_is_cut_at_the_byte_budget_between_whole_resources() -> Result<(), String> {
    let big = |n: u8| {
        json!({"id": format!("urn:ngsi-ld:Stop:{n}"), "type": "Stop",
                             "name": {"type": "Property", "value": "x".repeat(250_000)}})
    };
    let f = fixture_with(TestBroker::new().await?, &[big(1), big(2), big(3)], &[]).await?;
    let a = sta(&f.app, "/Things").await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(ids(&a.body), ["urn:ngsi-ld:Stop:1", "urn:ngsi-ld:Stop:2"]);
    let next = at(&a.body, "/@iot.nextLink");
    assert!(
        next.as_str().is_some_and(|l| l.ends_with("Things?$skip=2")),
        "{next}"
    );
    let b = send(&f.app, "GET", &local(next.as_str().unwrap_or_default()), "").await;
    assert_eq!(ids(&b.body), ["urn:ngsi-ld:Stop:3"]);
    Ok(())
}

#[tokio::test]
async fn a_refused_read_is_the_views_refusal() -> Result<(), String> {
    let f = fixture().await?;
    for path in [
        "/Things",
        "/HistoricalLocations",
        "/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations",
    ] {
        let a = send(
            &f.app,
            "GET",
            &format!("/x/views/writes/sta/v1.1{path}"),
            "",
        )
        .await;
        assert_eq!(a.status, StatusCode::METHOD_NOT_ALLOWED, "{path}");
        assert_eq!(at(&a.body, "/error/message"), "the view refused the read");
    }
    Ok(())
}

#[tokio::test]
async fn a_tenant_that_holds_nothing_answers_empty_sets() -> Result<(), String> {
    let f = fixture().await?;
    for set in ["Things", "HistoricalLocations"] {
        let a = send(
            &f.app,
            "GET",
            &format!("/x/views/void/sta/v1.1/{set}?$count=true"),
            "",
        )
        .await;
        assert_eq!(
            (a.status, a.body.clone()),
            (StatusCode::OK, json!({"@iot.count": 0, "value": []})),
            "{set}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_answer_past_the_buffer_bound_is_a_413() -> Result<(), String> {
    let broker = TestBroker::with_max_answer_bytes(600).await?;
    let f = fixture_with(broker, &entities(), &histories()).await?;
    for path in ["/Things", "/HistoricalLocations"] {
        let a = sta(&f.app, path).await;
        assert_eq!(
            a.status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "{path}: {}",
            a.body
        );
    }
    let one: Value = sta(&f.app, "/Things('urn:ngsi-ld:Stop:1')").await.body;
    assert_eq!(at(&one, "/@iot.id"), "urn:ngsi-ld:Stop:1");
    Ok(())
}
