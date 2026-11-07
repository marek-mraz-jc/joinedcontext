// SPDX-License-Identifier: EUPL-1.2
//! The window of a temporal retrieval (5.7.3): `datasetId` and `omit` pick
//! instances, an `onChange` Attribute carries its level into the window
//! (ADR-N-054 2.5), and so does a temporal Scope (4.18, annex C.5.16).

use super::*;
use crate::state::{OnChange, RecordMode, Recording};

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

async fn get(app: &Router, uri: &str) -> serde_json::Value {
    let resp = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    body_json(resp).await
}

fn values(doc: &serde_json::Value, attr: &str) -> Vec<serde_json::Value> {
    let mut v: Vec<serde_json::Value> = doc[attr]
        .as_array()
        .map(|a| a.iter().map(|i| i["value"].clone()).collect())
        .unwrap_or_default();
    v.sort_by_key(ToString::to_string);
    v
}

#[tokio::test]
async fn a_window_takes_its_datasets_and_carries_in_what_held_at_its_start() {
    let st = AppState::new("antares-test".into());
    let mut level = Recording::default();
    level.attributes.insert(
        format!("{DEFAULT}level"),
        Some(RecordMode::OnChange(OnChange {
            heartbeat: std::time::Duration::from_secs(3600),
            deadband: None,
            min_distance: None,
        })),
    );
    st.history_off
        .write()
        .expect("rules")
        .recording
        .insert(("default".into(), format!("{DEFAULT}Tank")), level);
    let app = router(st);
    let body = serde_json::json!({
        "id": "urn:ngsi-ld:Tank:w", "type": "Tank",
        "speed": [
            {"type": "Property", "value": 1, "datasetId": "urn:d:1", "observedAt": "2026-01-01T02:00:00Z"},
            {"type": "Property", "value": 2, "datasetId": "urn:d:2", "observedAt": "2026-01-01T02:00:00Z"},
            {"type": "Property", "value": 3, "observedAt": "2026-01-01T02:00:00Z"}
        ],
        "level": [
            {"type": "Property", "value": 10, "observedAt": "2026-01-01T00:00:00Z"},
            {"type": "Property", "value": 11, "observedAt": "2026-01-01T00:30:00Z"},
            {"type": "Property", "value": 12, "observedAt": "2026-01-01T02:00:00Z"}
        ],
        "scope": [
            {"type": "Property", "value": "/old", "observedAt": "2026-01-01T00:00:00Z"},
            {"type": "Property", "value": "/now", "observedAt": "2026-01-01T00:40:00Z"},
            {"type": "Property", "value": "/later", "observedAt": "2026-01-01T03:00:00Z"}
        ]
    })
    .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let base = "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Tank:w?timerel=after&timeAt=2026-01-01T01:00:00Z";
    let doc = get(&app, &format!("{base}&datasetId=urn:d:1,@none")).await;
    assert_eq!(
        values(&doc, "speed"),
        [serde_json::json!(1), serde_json::json!(3)],
        "{doc}"
    );
    assert_eq!(
        values(&doc, "level"),
        [serde_json::json!(11), serde_json::json!(12)],
        "the onChange level that held at the start comes along: {doc}"
    );
    let scopes: Vec<&str> = doc["scope"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["value"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        scopes.contains(&"/now") && scopes.contains(&"/later") && !scopes.contains(&"/old"),
        "the scope valid at the start is carried in: {doc}"
    );

    let doc = get(&app, &format!("{base}&datasetId=urn:d:2&omit=level")).await;
    assert_eq!(values(&doc, "speed"), [serde_json::json!(2)], "{doc}");
    assert!(doc.get("level").is_none(), "omitted: {doc}");

    // `before` opens no window start: nothing is carried in
    let doc = get(
        &app,
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Tank:w?timerel=before&timeAt=2026-01-01T00:10:00Z",
    )
    .await;
    assert_eq!(values(&doc, "level"), [serde_json::json!(10)], "{doc}");

    // the hourly mean of an onChange level weighs each value by the time
    // it held: 10 for half an hour, then 11
    let doc = get(
        &app,
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Tank:w?timerel=between\
         &timeAt=2026-01-01T00:00:00Z&endTimeAt=2026-01-01T02:00:00Z&attrs=level\
         &aggrMethods=avg&aggrPeriodDuration=PT1H&format=aggregatedValues",
    )
    .await;
    let buckets = doc["level"]["avg"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        buckets.first().map(|b| b[0].clone()),
        Some(serde_json::json!(10.5)),
        "{doc}"
    );
}

/// TS 104 176 6.4.7.3: a lastN page links to the next one back in time
/// with offsetN moved by the page, and from there back to the first; the
/// Links keep the rest of the request.
#[tokio::test]
async fn a_last_n_page_links_its_neighbours_by_offset() {
    let app = app();
    let body = serde_json::json!({
        "id": "urn:ngsi-ld:Tank:p", "type": "Tank",
        "speed": [
            {"type": "Property", "value": 1, "observedAt": "2026-01-01T01:00:00Z"},
            {"type": "Property", "value": 2, "observedAt": "2026-01-01T02:00:00Z"},
            {"type": "Property", "value": 3, "observedAt": "2026-01-01T03:00:00Z"}
        ]
    })
    .to_string();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("req"),
        )
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let links = |uri: &'static str| {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(Request::get(uri).body(Body::empty()).expect("req"))
                .await
                .expect("resp");
            assert!(resp.status().is_success(), "{uri}: {}", resp.status());
            resp.headers()
                .get_all(axum::http::header::LINK)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(", <"))
                .map(str::to_owned)
                .collect::<Vec<String>>()
        }
    };
    let first = links(
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Tank:p?lastN=1&timerel=before&timeAt=2030-01-01T00:00:00Z",
    )
    .await;
    assert!(
        first.iter().any(|l| l.contains("offsetN=1")
            && l.contains("lastN=1")
            && l.contains("rel=\"intervalbefore\"")),
        "the next page back in time: {first:?}"
    );
    assert!(
        !first.iter().any(|l| l.contains("intervalafter")),
        "{first:?}"
    );
    let second = links(
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Tank:p?lastN=1&offsetN=1&timerel=before&timeAt=2030-01-01T00:00:00Z",
    )
    .await;
    assert!(
        second
            .iter()
            .any(|l| l.contains("offsetN=0") && l.contains("rel=\"intervalafter\"")),
        "back to the first page: {second:?}"
    );
}
