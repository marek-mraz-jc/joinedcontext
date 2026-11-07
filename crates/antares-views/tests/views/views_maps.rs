// SPDX-License-Identifier: EUPL-1.2
//! V22 (surfaces-threat-model): an Entity Map holds the ids one query
//! selected, so a map built through one view must not page through them on
//! another view or at the root. Through a view no map is built or read: a
//! request that names one is 400 and creates nothing.

use crate::common::{fixture, get, send, CARS};
use antares_views::NO_MAPS;
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn an_entity_map_is_bound_to_its_view() -> Result<(), String> {
    let f = fixture().await?;
    for uri in [
        format!("{CARS}/entities?type=Car&entityMap=true"),
        format!("{CARS}/entities?type=Car&entity%4Dap=true"),
        format!("{CARS}/entities/urn:ngsi-ld:Car:1?entityMap=true"),
        format!("{CARS}/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z&entityMap=true"),
        format!("{CARS}/entities?type=Car&q=%ZZ"),
    ] {
        let a = get(&f.app, &uri).await;
        assert_eq!(a.status, StatusCode::BAD_REQUEST, "{uri}: {}", a.text);
        assert!(a.header("NGSILD-EntityMap").is_none(), "{uri}");
    }
    let a = get(&f.app, &format!("{CARS}/entities?type=Car&entityMap=true")).await;
    assert_eq!(a.json.get("detail"), Some(&json!(NO_MAPS)));

    // a map built at the root is not read through the view
    let root = send(
        &f.app,
        Method::GET,
        "/ngsi-ld/v1/entities?type=Car&entityMap=true",
        &[("NGSILD-Tenant", "city")],
        None,
    )
    .await;
    assert_eq!(root.status, StatusCode::CREATED, "{}", root.text);
    let map = root
        .header("NGSILD-EntityMap")
        .ok_or("the root built a map")?;
    let through = send(
        &f.app,
        Method::GET,
        &format!("{CARS}/entities?type=Car"),
        &[("NGSILD-EntityMap", &map)],
        None,
    )
    .await;
    assert_eq!(through.status, StatusCode::BAD_REQUEST, "{}", through.text);
    assert!(!through.text.contains("Car:3"), "{}", through.text);
    Ok(())
}
