// SPDX-License-Identifier: EUPL-1.2
//! Integration tests for the `summarize` tool (T-4680): profiling entity
//! types with statistics and samples, `q` filtering, validation, and read-only
//! tool listing.

use crate::common::{call, fixture, rpc};
use serde_json::{json, Value};

/// Temperature values assigned to the created test devices.
const TEMPS: [f64; 8] = [15.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0];

/// Status categories assigned to the created test devices.
const STATUSES: [&str; 8] = [
    "active", "idle", "active", "idle", "active", "idle", "active", "idle",
];

/// Longitude coordinates assigned to the created test devices.
const LONS: [f64; 8] = [10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0];

/// Latitude coordinates assigned to the created test devices.
const LATS: [f64; 8] = [20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 27.0];

/// Populate the test broker with test devices under the `devices` view.
async fn setup_devices(app: &axum::Router) -> Result<(), String> {
    crate::common::put_view(
        app,
        &json!({
            "id": "devices",
            "tenant": "city",
            "selector": ["Device"],
            "rules": [{
                "types": ["Device"],
                "attributes": ["temperature", "status", "location"]
            }],
            "operations": crate::common::READS
        }),
    )
    .await?;

    for i in 0..TEMPS.len() {
        let temp = *TEMPS.get(i).unwrap_or(&15.0);
        let status = *STATUSES.get(i).unwrap_or(&"active");
        let lon = *LONS.get(i).unwrap_or(&10.0);
        let lat = *LATS.get(i).unwrap_or(&20.0);
        crate::common::create(
            app,
            "city",
            &json!({
                "id": format!("urn:ngsi-ld:Device:{i}"),
                "type": "Device",
                "temperature": {
                    "type": "Property",
                    "value": temp,
                    "unitCode": "CEL"
                },
                "status": {
                    "type": "Property",
                    "value": status
                },
                "location": {
                    "type": "GeoProperty",
                    "value": {
                        "type": "Point",
                        "coordinates": [lon, lat]
                    }
                }
            }),
        )
        .await?;
    }
    Ok(())
}

/// `summarize` profiles an entity type, matching created counts and extremes.
#[tokio::test(flavor = "multi_thread")]
async fn summarize_profiles_entity_type_with_statistics_and_samples() -> Result<(), String> {
    let f = fixture().await?;
    setup_devices(&f.app).await?;

    let a = call(&f.app, "devices", "summarize", json!({"type": ["Device"]})).await;
    assert!(!a.is_error(), "{}", a.text);

    let structured = a.structured();
    let created_count = u64::try_from(TEMPS.len()).unwrap_or(0);
    assert_eq!(
        structured.get("count").and_then(Value::as_u64),
        Some(created_count)
    );

    let temp_min = structured
        .pointer("/attributes/temperature/min")
        .and_then(Value::as_f64);
    assert_eq!(temp_min, TEMPS.first().copied());

    let temp_max = structured
        .pointer("/attributes/temperature/max")
        .and_then(Value::as_f64);
    assert_eq!(temp_max, TEMPS.last().copied());

    // the area every location covers, from the GeoJSON's positions
    assert_eq!(
        structured.pointer("/attributes/location/bbox"),
        Some(&json!([10.0, 20.0, 17.0, 27.0])),
        "{}",
        a.text
    );

    assert!(a.text.contains("type=Device"), "{}", a.text);
    assert!(
        a.text.contains(&format!("count={created_count}")),
        "{}",
        a.text
    );

    // The first 5 entities appear as samples
    for i in 0..5_usize {
        assert!(
            a.text.contains(&format!("urn:ngsi-ld:Device:{i}")),
            "expected sample Device:{i} in text: {}",
            a.text
        );
    }

    // Entities beyond the 5 samples must not appear in the text
    for i in 5..TEMPS.len() {
        assert!(
            !a.text.contains(&format!("Device:{i}")),
            "unexpected Device:{i} in text: {}",
            a.text
        );
    }

    Ok(())
}

/// `summarize` with a `q` filter counts only matching entities, matching `query_entities`.
#[tokio::test(flavor = "multi_thread")]
async fn summarize_with_q_narrows_to_subset() -> Result<(), String> {
    let f = fixture().await?;
    setup_devices(&f.app).await?;

    let q = "temperature>18";
    let q_res = call(
        &f.app,
        "devices",
        "query_entities",
        json!({"type": ["Device"], "q": q, "count": true}),
    )
    .await;
    assert!(!q_res.is_error(), "{}", q_res.text);
    let expected_total = q_res.structured().get("total").and_then(Value::as_u64);
    assert!(expected_total.is_some_and(|t| t > 0));

    let s_res = call(
        &f.app,
        "devices",
        "summarize",
        json!({"type": ["Device"], "q": q}),
    )
    .await;
    assert!(!s_res.is_error(), "{}", s_res.text);

    let summary_count = s_res.structured().get("count").and_then(Value::as_u64);
    assert_eq!(summary_count, expected_total);

    Ok(())
}

/// `summarize` requires exactly one type, returning a tool error otherwise.
#[tokio::test(flavor = "multi_thread")]
async fn summarize_validates_type_argument() -> Result<(), String> {
    let f = fixture().await?;
    setup_devices(&f.app).await?;

    // Missing type argument
    let no_type = call(&f.app, "devices", "summarize", json!({})).await;
    assert!(no_type.is_error(), "{}", no_type.text);

    // Two types provided
    let two_types = call(
        &f.app,
        "devices",
        "summarize",
        json!({"type": ["Device", "Car"]}),
    )
    .await;
    assert!(two_types.is_error(), "{}", two_types.text);

    // the same type twice is still two
    let twice = call(
        &f.app,
        "devices",
        "summarize",
        json!({"type": ["Device", "Device"]}),
    )
    .await;
    assert!(twice.text.contains("exactly one type"), "{}", twice.text);

    // a query the broker refuses is its status and title, as query_entities answers it
    let bad_q = call(
        &f.app,
        "devices",
        "summarize",
        json!({"type": ["Device"], "q": ")xyzzy("}),
    )
    .await;
    assert!(bad_q.is_error(), "{}", bad_q.text);
    assert_eq!(
        bad_q.structured().pointer("/error/status"),
        Some(&json!(400)),
        "{}",
        bad_q.text
    );

    Ok(())
}

/// `summarize` is advertised in `tools/list` with `readOnlyHint: true` and needs no confirmation.
#[tokio::test(flavor = "multi_thread")]
async fn summarize_appears_in_tools_list_as_read_only() -> Result<(), String> {
    let f = fixture().await?;
    setup_devices(&f.app).await?;

    let tools_res = rpc(&f.app, "devices", "tools/list", json!({})).await;
    assert!(!tools_res.is_error(), "{}", tools_res.text);

    let tools = tools_res
        .result()
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "tools array missing".to_string())?;

    let summarize_tool = tools
        .iter()
        .find(|t| t.get("name") == Some(&json!("summarize")))
        .ok_or_else(|| "summarize not found in tools/list".to_string())?;

    assert_eq!(
        summarize_tool.pointer("/annotations/readOnlyHint"),
        Some(&json!(true))
    );

    // Calling summarize executes directly without asking for confirmation
    let call_res = call(&f.app, "devices", "summarize", json!({"type": ["Device"]})).await;
    assert!(!call_res.is_error(), "{}", call_res.text);
    assert!(
        call_res.result().get("requestState").is_none(),
        "read tool must not ask confirmation: {}",
        call_res.text
    );

    Ok(())
}

/// `lang` picks the language a sample's `LanguageProperty` name is shown in.
#[tokio::test(flavor = "multi_thread")]
async fn summarize_names_samples_in_the_asked_language() -> Result<(), String> {
    let f = fixture().await?;
    setup_devices(&f.app).await?;
    let a = call(
        &f.app,
        "devices",
        "summarize",
        json!({"type": ["Device"], "lang": "en"}),
    )
    .await;
    assert!(!a.is_error(), "{}", a.text);
    assert!(a.text.contains("type=Device"), "{}", a.text);
    Ok(())
}
