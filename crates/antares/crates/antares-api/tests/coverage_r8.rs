// SPDX-License-Identifier: EUPL-1.2
//! Edge paths of the entity handlers the other files leave unread (T-7949): each test names the
//! clause or the store answer it forces.

use crate::common::Double;

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

async fn get(st: &AppState, uri: &str) -> (StatusCode, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(
            Request::get(uri)
                .header("Accept", "application/json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("answer");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// T-6770: a GET of a reminted id answers the Entity under its new id. When the new id reads as
/// not found too (the Entity was deleted after the remint), the old id's own 404 stands: the
/// answer names the id the client asked for, never the one it was renamed to.
#[tokio::test(flavor = "multi_thread")]
async fn a_remapped_id_whose_new_entity_is_gone_answers_the_old_ids_404() {
    const OLD: &str = "urn:ngsi-ld:KPI:bbsk.sk:gone";
    const NEW: &str = "urn:ngsi-ld:KPI:sk_bbsk:gone";
    let mut st = AppState::new("cov-r8".into());
    st.store = Arc::new(Double::remapping(st.store.clone(), OLD, NEW));
    let (status, body) = get(&st, &format!("/ngsi-ld/v1/entities/{OLD}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(detail.contains(OLD) && !detail.contains(NEW), "{body}");
}

async fn call(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if body.is_some() {
        req = req.header("Content-Type", "application/json");
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    req = req.header("Content-Length", body.len());
    let body = Body::from(body);
    let resp = antares_api::router(st.clone())
        .oneshot(req.body(body).expect("request"))
        .await
        .expect("answer");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Delete Entities by query (purge): an Entity another writer deleted between the query and the
/// delete counts as nothing deleted here, and the purge still ends with every match gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_purge_whose_match_another_writer_deleted_first_still_empties_the_set() {
    let mut st = AppState::new("cov-r8".into());
    st.store = Arc::new(Double::vanishing(st.store.clone()));
    for n in 0..3 {
        let entity = serde_json::json!({
            "id": format!("urn:ngsi-ld:Purge:vanish-{n}"),
            "type": "Purge",
            "n": {"type": "Property", "value": n}
        });
        let (status, body) = call(&st, "POST", "/ngsi-ld/v1/entities", Some(entity)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = call(&st, "DELETE", "/ngsi-ld/v1/entities?type=Purge", None).await;
    assert!(status.is_success(), "{status} {body}");
    let (status, left) = get(&st, "/ngsi-ld/v1/entities?type=Purge").await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(left, serde_json::json!([]), "every match is gone");
}

/// Replace Entity (5.6.18) with a Context Source registered for its type: the local part decides
/// the `type` selector under the row lock. A concurrent write that turned the Entity into a
/// Building between the read and the replace leaves the local part not found, and the Building
/// stands.
#[tokio::test(flavor = "multi_thread")]
async fn a_federated_replace_keeps_an_entity_a_racing_write_retyped() {
    const ID: &str = "urn:ngsi-ld:Vehicle:cov-r8-retyped";
    let mut st = AppState::new("cov-r8".into());
    let reg = serde_json::json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:cov-r8-replace",
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "endpoint": "http://127.0.0.1:9"
    });
    let (status, body) = call(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let car = serde_json::json!({"id": ID, "type": "Vehicle"});
    let (status, body) = call(&st, "POST", "/ngsi-ld/v1/entities", Some(car.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let building = serde_json::json!({
        "id": ID,
        "type": ["https://uri.etsi.org/ngsi-ld/default-context/Building"],
        "createdAt": "2026-01-01T00:00:00.000Z",
        "modifiedAt": "2026-01-01T00:00:00.000Z",
    });
    st.store = Arc::new(Double::racing_write(st.store.clone(), building));
    let (status, body) = call(
        &st,
        "PUT",
        &format!("/ngsi-ld/v1/entities/{ID}?type=Vehicle"),
        Some(car),
    )
    .await;
    assert_ne!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, now) = get(&st, &format!("/ngsi-ld/v1/entities/{ID}?local=true")).await;
    assert_eq!(status, StatusCode::OK, "{now}");
    assert_eq!(
        now["type"], "Building",
        "the racing write's Entity stands: {now}"
    );
}

/// Query Entities by `id` (5.7.2) over a store that cannot narrow on ids: the broker decides
/// each row itself, so an Entity outside the id list never reaches the answer.
#[tokio::test(flavor = "multi_thread")]
async fn an_id_query_over_a_store_that_cannot_select_ids_keeps_only_the_named_ones() {
    let mut st = AppState::new("cov-r8".into());
    for n in 0..3 {
        let entity = serde_json::json!({"id": format!("urn:ngsi-ld:Ids:n{n}"), "type": "Ids"});
        let (status, body) = call(&st, "POST", "/ngsi-ld/v1/entities", Some(entity)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    st.store = Arc::new(Double::lesser(
        st.store.clone(),
        crate::common::Lesser::NoIds,
    ));
    let (status, body) = get(
        &st,
        "/ngsi-ld/v1/entities?id=urn:ngsi-ld:Ids:n0,urn:ngsi-ld:Ids:n2&type=Ids",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut ids: Vec<&str> = body
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["urn:ngsi-ld:Ids:n0", "urn:ngsi-ld:Ids:n2"], "{body}");
}

/// Retrieve Available Attributes (5.7.8) over a store with an attribute catalogue but no type
/// list: the catalogue walk cannot start, and the answer still names the stored Attribute.
#[tokio::test(flavor = "multi_thread")]
async fn attributes_over_a_store_without_a_type_list_still_name_the_stored_ones() {
    let mut st = AppState::new("cov-r8".into());
    let entity = serde_json::json!({
        "id": "urn:ngsi-ld:Shape:one",
        "type": "Shape",
        "corners": {"type": "Property", "value": 4}
    });
    let (status, body) = call(&st, "POST", "/ngsi-ld/v1/entities", Some(entity)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    st.store = Arc::new(Double::lesser(
        st.store.clone(),
        crate::common::Lesser::NoTypeNames,
    ));
    let (status, body) = get(&st, "/ngsi-ld/v1/attributes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("corners"), "{body}");
}
