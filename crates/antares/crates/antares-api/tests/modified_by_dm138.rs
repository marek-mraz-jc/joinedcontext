// SPDX-License-Identifier: EUPL-1.2
//! Each attribute's current state keeps who made its last change (platform
//! ADR-N-054 2.8, DM-138): `modifiedBy` {organization, actorKind, actor},
//! stamped only from what the gateway's mesh identity names, answered only
//! with `options=sysAttrs` and only to a caller the gateway grants
//! `readVersions`. A plain read is vanilla NGSI-LD, and vanilla Antares
//! stamps and answers none.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const GATEWAY: &str = "context-gateway.dev.serviceaccount.identity.linkerd.cluster.local";
const ID: &str = "urn:ngsi-ld:Sensor:t5396";

async fn state() -> AppState {
    let mut st = AppState::new("modified-by".into());
    antares_api::wire(&mut st).await;
    st.provenance_from = Some(GATEWAY.into());
    st
}

/// The headers the gateway sets for a person of `organization`.
fn writer(organization: &'static str, actor: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("l5d-client-id", GATEWAY),
        ("jc-provenance-channel", "ui"),
        ("jc-provenance-organization", organization),
        ("jc-provenance-actor-kind", "person"),
        ("jc-provenance-actor", actor),
    ]
}

const GRANTED: [(&str, &str); 2] = [("l5d-client-id", GATEWAY), ("jc-read-versions", "true")];

async fn send(
    st: &AppState,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create(st: &AppState, headers: &[(&str, &str)]) {
    let (status, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/entities",
        headers,
        Some(json!({"id": ID, "type": "Sensor",
                    "t": {"type": "Property", "value": 1},
                    "u": {"type": "Property", "value": 2}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

async fn read(st: &AppState, query: &str, headers: &[(&str, &str)]) -> Value {
    let (status, body) = send(
        st,
        "GET",
        &format!("/ngsi-ld/v1/entities/{ID}{query}"),
        headers,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[cfg(feature = "jc")]
#[tokio::test]
async fn sys_attrs_answers_the_writer_only_to_a_caller_granted_read_versions() {
    let st = state().await;
    create(&st, &writer("city", "u-steward")).await;
    let by = json!({"organization": "city", "actorKind": "person", "actor": "u-steward"});

    let shown = read(&st, "?options=sysAttrs", &GRANTED).await;
    assert_eq!(shown["t"]["modifiedBy"], by, "{shown}");
    assert_eq!(shown["u"]["modifiedBy"], by, "{shown}");
    assert!(shown["t"]["modifiedAt"].is_string(), "{shown}");
    assert_eq!(
        shown.get("modifiedBy"),
        None,
        "an Entity member it is not: {shown}"
    );

    let plain = read(&st, "", &GRANTED).await;
    assert!(!plain.to_string().contains("modifiedBy"), "{plain}");
    let ungranted = read(&st, "?options=sysAttrs", &[("l5d-client-id", GATEWAY)]).await;
    assert!(!ungranted.to_string().contains("modifiedBy"), "{ungranted}");
    // the grant is believed from the gateway alone
    let forged = read(&st, "?options=sysAttrs", &[("jc-read-versions", "true")]).await;
    assert!(!forged.to_string().contains("modifiedBy"), "{forged}");
    let simplified = read(&st, "?options=sysAttrs,keyValues", &GRANTED).await;
    assert!(
        !simplified.to_string().contains("modifiedBy"),
        "{simplified}"
    );
    let concise = read(&st, "?options=sysAttrs,concise", &GRANTED).await;
    assert_eq!(concise["t"]["modifiedBy"], by, "{concise}");
    let (status, page) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Sensor&options=sysAttrs",
        &GRANTED,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        page[0]["t"]["modifiedBy"], by,
        "a query answers it too: {page}"
    );
}

#[cfg(feature = "jc")]
#[tokio::test]
async fn each_change_moves_the_writer_of_the_attributes_it_touched() {
    let st = state().await;
    create(&st, &writer("city", "u-steward")).await;
    // 5.6.4 partial attribute update
    let (status, body) = send(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{ID}/attrs/t"),
        &writer("utility", "u-operator"),
        Some(json!({"value": 3})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let shown = read(&st, "?options=sysAttrs", &GRANTED).await;
    assert_eq!(
        shown["t"]["modifiedBy"]["organization"], "utility",
        "{shown}"
    );
    assert_eq!(shown["t"]["modifiedBy"]["actor"], "u-operator", "{shown}");
    assert_eq!(
        shown["u"]["modifiedBy"]["organization"], "city",
        "untouched: {shown}"
    );

    // 5.6.3 update; a write the gateway did not send names nobody, so the
    // writer it replaced does not stay behind on a value it did not write
    let (status, body) = send(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{ID}/attrs"),
        &[
            ("jc-provenance-organization", "forged"),
            ("jc-provenance-actor-kind", "person"),
            ("jc-provenance-actor", "u-forged"),
        ],
        Some(json!({"u": {"type": "Property", "value": 4}})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let shown = read(&st, "?options=sysAttrs", &GRANTED).await;
    assert_eq!(shown["u"].get("modifiedBy"), None, "{shown}");
    assert_eq!(shown["t"]["modifiedBy"]["actor"], "u-operator", "{shown}");
}

#[cfg(feature = "jc")]
#[tokio::test]
async fn a_client_sub_attribute_named_modified_by_never_passes_for_the_writer() {
    let st = state().await;
    let (status, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        &writer("city", "u-steward"),
        Some(json!({"id": ID, "type": "Sensor",
                    "t": {"type": "Property", "value": 1,
                          "modifiedBy": {"type": "Property", "value": "the mayor"}}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let shown = read(&st, "?options=sysAttrs", &GRANTED).await;
    assert_eq!(shown["t"]["modifiedBy"]["actor"], "u-steward", "{shown}");
    // without the grant it is the client's own sub-Property, as in vanilla
    let plain = read(&st, "", &[]).await;
    assert_eq!(plain["t"]["modifiedBy"]["value"], "the mayor", "{plain}");
}

#[cfg(not(feature = "jc"))]
#[tokio::test]
async fn vanilla_stamps_and_answers_no_writer() {
    let st = state().await;
    create(&st, &writer("city", "u-steward")).await;
    let shown = read(&st, "?options=sysAttrs", &GRANTED).await;
    assert!(shown["t"]["modifiedAt"].is_string(), "{shown}");
    assert!(!shown.to_string().contains("modifiedBy"), "{shown}");
}
