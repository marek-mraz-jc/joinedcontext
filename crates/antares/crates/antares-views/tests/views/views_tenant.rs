// SPDX-License-Identifier: EUPL-1.2
//! ADR-0023 rule 10 with snapshots (6.3.22; surfaces-test-plan section 6,
//! V17, V21): a snapshot read through a view is the view's narrowing over
//! the frozen copy, and a snapshot of another Tenant is not reachable.

use crate::common::{fixture, send, Answer, CARS};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

/// A snapshot of every Car of `tenant`, once it is ready.
async fn snapshot(app: &axum::Router, tenant: &str) -> Result<String, String> {
    let body = json!({"type": "Snapshot", "snapshotQueries": [{"type": "Query", "entities": [{"type": "Car"}]}]});
    let made = send(
        app,
        Method::POST,
        "/ngsi-ld/v1/snapshots",
        &[("NGSILD-Tenant", tenant)],
        Some(&body),
    )
    .await;
    let loc = made
        .header("location")
        .ok_or_else(|| format!("no Location: {} {}", made.status, made.text))?;
    for _ in 0..200 {
        let status: Answer = send(app, Method::GET, &loc, &[("NGSILD-Tenant", tenant)], None).await;
        match status.json.get("snapshotStatus").and_then(Value::as_str) {
            Some("preparing") => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
            Some("success") => {
                return status
                    .json
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| status.text.clone());
            }
            _ => return Err(status.text),
        }
    }
    Err("the snapshot never got ready".into())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_read_through_a_view_is_narrowed() -> Result<(), String> {
    let f = fixture().await?;
    let city = snapshot(&f.app, "city").await?;
    let uri = format!("{CARS}/entities?type=Car");
    let frozen = send(
        &f.app,
        Method::GET,
        &uri,
        &[("NGSILD-Snapshot", &city)],
        None,
    )
    .await;
    assert_eq!(frozen.status, StatusCode::OK, "{}", frozen.text);
    let mut ids = frozen.ids();
    ids.sort();
    assert_eq!(
        ids,
        ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"],
        "{}",
        frozen.text
    );
    assert!(!frozen.text.contains("secret"), "{}", frozen.text);
    let direct = send(
        &f.app,
        Method::GET,
        "/ngsi-ld/v1/entities?type=Car",
        &[("NGSILD-Tenant", "city"), ("NGSILD-Snapshot", &city)],
        None,
    )
    .await;
    assert_eq!(
        direct.ids().len(),
        3,
        "the snapshot holds Car:3 and the secrets: {}",
        direct.text
    );
    assert!(direct.text.contains("secret"));

    let other = snapshot(&f.app, "other").await?;
    let foreign = send(
        &f.app,
        Method::GET,
        &uri,
        &[("NGSILD-Snapshot", &other)],
        None,
    )
    .await;
    assert_ne!(foreign.status, StatusCode::OK, "{}", foreign.text);
    assert!(!foreign.text.contains("Car:9"), "{}", foreign.text);
    Ok(())
}

/// F4/V18: the view's Tenant is named only to a caller who named it.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_does_not_echo_a_tenant_the_client_did_not_send() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        format!("{CARS}/entities?type=Car"),
        format!("{CARS}/entities/urn:ngsi-ld:Car:1"),
        format!("{CARS}/entities/urn:ngsi-ld:Car:3"),
        format!("{CARS}/types"),
        format!("{CARS}/temporal/entities/urn:ngsi-ld:Car:1"),
    ] {
        let unnamed = send(&f.app, Method::GET, &uri, &[], None).await;
        assert!(
            unnamed.headers.get("NGSILD-Tenant").is_none(),
            "{uri} names the tenant: {:?}",
            unnamed.headers
        );
        assert!(!unnamed.text.contains("city"), "{uri}: {}", unnamed.text);
        let named = send(
            &f.app,
            Method::GET,
            &uri,
            &[("NGSILD-Tenant", "city")],
            None,
        )
        .await;
        assert_eq!(named.status, unnamed.status, "{uri}");
        assert_eq!(
            named.header("NGSILD-Tenant").as_deref(),
            Some("city"),
            "{uri}"
        );
    }
    Ok(())
}
