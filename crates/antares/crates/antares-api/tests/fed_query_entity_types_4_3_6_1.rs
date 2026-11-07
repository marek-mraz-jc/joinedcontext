// SPDX-License-Identifier: EUPL-1.2
//! 4.3.6.1: "Ultimately, all constraints specified in the registration shall
//! be respected." A source registered for `Vehicle` is not a source of
//! `OffStreetParking`, even when a multi-type query reaches it and it
//! answers with one: that entity is not this registration's data, and
//! importing it lets it override the source that is registered for it
//! (ETSI IOP_003_02_02, IOP_003_03_02, run 38806308784).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::io::{Read, Write};
use tower::ServiceExt;

/// A Context Source answering every request with `body`.
fn source(body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 65536];
            let _ = s.read(&mut buf);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(reply.as_bytes());
        }
    });
    port
}

async fn call(st: &AppState, req: Request<Body>) -> (StatusCode, Value) {
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// What a `type=OffStreetParking,Vehicle` query returns when its only data
/// comes from one source, registered once per element of `registrations`
/// (each an `information` array), that answers with a Vehicle and an
/// OffStreetParking carrying `speed` and `totalSpotsNumber`.
async fn query_through(registrations: &[Value]) -> Value {
    antares_jsonld::allow_private_egress(true);
    let port = source(
        r#"[{"id":"urn:ngsi-ld:Vehicle:1","type":"Vehicle","speed":{"type":"Property","value":1}},
            {"id":"urn:ngsi-ld:OffStreetParking:1","type":"OffStreetParking","speed":{"type":"Property","value":2},
             "totalSpotsNumber":{"type":"Property","value":3}}]"#,
    );
    let st = AppState::new("antares-fed-types".into());
    for (n, information) in registrations.iter().enumerate() {
        let reg = json!({
            "id": format!("urn:ngsi-ld:ContextSourceRegistration:src{n}"),
            "type": "ContextSourceRegistration",
            "mode": "inclusive",
            "operations": ["federationOps"],
            "information": information,
            "endpoint": format!("http://127.0.0.1:{port}"),
        })
        .to_string();
        let (status, _) = call(
            &st,
            Request::builder()
                .method("POST")
                .uri("/ngsi-ld/v1/csourceRegistrations")
                .header("Content-Type", "application/json")
                .header("Content-Length", reg.len())
                .body(Body::from(reg))
                .expect("request"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) = call(
        &st,
        Request::builder()
            .method("GET")
            .uri("/ngsi-ld/v1/entities?type=OffStreetParking,Vehicle")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn ids_through(information: Value) -> Vec<String> {
    let body = query_through(&[information]).await;
    let mut ids: Vec<String> = body
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entity_outside_the_registered_types_is_not_imported() {
    let ids = ids_through(json!([{"entities": [{"type": "Vehicle"}]}])).await;
    assert_eq!(ids, ["urn:ngsi-ld:Vehicle:1"]);
}

/// An attributes-only RegistrationInfo names no type, so it admits every
/// type, even beside a typed one.
#[tokio::test(flavor = "multi_thread")]
async fn an_info_without_entities_admits_every_type() {
    let ids = ids_through(json!([
        {"entities": [{"type": "Vehicle"}]},
        {"propertyNames": ["speed"]}
    ]))
    .await;
    assert_eq!(
        ids,
        ["urn:ngsi-ld:OffStreetParking:1", "urn:ngsi-ld:Vehicle:1"]
    );
}

/// ETSI IOP_003_02_02: one source registered twice, (OffStreetParking ×
/// totalSpotsNumber) and (Vehicle × all). Its OffStreetParking contributes
/// totalSpotsNumber only; its `speed` belongs to no registration.
#[tokio::test(flavor = "multi_thread")]
async fn two_registrations_of_one_source_do_not_cross_their_scopes() {
    let body = query_through(&[
        json!([{"entities": [{"type": "OffStreetParking"}], "propertyNames": ["totalSpotsNumber"]}]),
        json!([{"entities": [{"type": "Vehicle"}]}]),
    ])
    .await;
    let parking = body
        .as_array()
        .expect("array")
        .iter()
        .find(|e| e["id"] == "urn:ngsi-ld:OffStreetParking:1")
        .unwrap_or_else(|| panic!("{body}"));
    assert!(parking.get("totalSpotsNumber").is_some(), "{body}");
    assert!(parking.get("speed").is_none(), "{body}");
}
