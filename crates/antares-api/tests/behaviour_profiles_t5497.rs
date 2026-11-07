// SPDX-License-Identifier: EUPL-1.2
//! Behaviour profile keys (T-5497, docs/conformance/profiles.md): where the
//! spec texts or the conformance suites accept two answers, one test per
//! value of each key. The default is the answer the pinned ETSI suite asserts.

use antares_api::{AppState, Profile};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::io::{Read, Write};
use tower::ServiceExt;

async fn send(
    st: &AppState,
    method: &str,
    path: &str,
    tenant: Option<&str>,
    body: Option<String>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1{path}"));
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let body = match body {
        Some(b) => {
            req = req
                .header("Content-Type", "application/json")
                .header("Content-Length", b.len());
            Body::from(b)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn with(profile: Profile) -> AppState {
    let mut st = AppState::new("antares-profile".into());
    st.profile = profile;
    st
}

fn error_type(body: &Value) -> &str {
    body["type"].as_str().unwrap_or("")
}

/// ANTARES_NON_OBJECT_BODY: `[]` is valid JSON (8.2.3 first bullet passes)
/// but no Entity; the default answers InvalidRequest as ETSI 001_02_02
/// asserts, `bad-request-data` answers the 8.2.3 second bullet.
#[tokio::test]
async fn non_object_body_error_type_per_profile_value() {
    for (path, body) in [("/entities", "[]"), ("/csourceRegistrations", "[1,2,3]")] {
        let (status, got) = send(
            &with(Profile::default()),
            "POST",
            path,
            None,
            Some(body.into()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {got}");
        assert_eq!(
            error_type(&got),
            "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest",
            "{path}: {got}"
        );

        let profile = Profile {
            non_object_bad_request_data: true,
            ..Profile::default()
        };
        let (status, got) = send(&with(profile), "POST", path, None, Some(body.into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {got}");
        assert_eq!(
            error_type(&got),
            "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
            "{path}: {got}"
        );
    }
    // not JSON at all stays InvalidRequest under either value (8.2.3 first bullet)
    let profile = Profile {
        non_object_bad_request_data: true,
        ..Profile::default()
    };
    let (_, got) = send(&with(profile), "POST", "/entities", None, Some("{".into())).await;
    assert_eq!(
        error_type(&got),
        "https://uri.etsi.org/ngsi-ld/errors/InvalidRequest",
        "{got}"
    );
}

fn upsert_body() -> String {
    json!([{"id": "urn:ngsi-ld:Vehicle:p1", "type": "Vehicle", "speed": {"type": "Property", "value": 1}}]).to_string()
}

/// ANTARES_IMPLICIT_TENANTS: by default Batch Upsert creates an unknown
/// Tenant like Batch Create; `listed` keeps implicit creation to the 7.8.2
/// list, so the upsert answers NonexistentTenant and creates nothing.
#[tokio::test]
async fn batch_upsert_on_an_unknown_tenant_per_profile_value() {
    let st = with(Profile::default());
    let (status, got) = send(
        &st,
        "POST",
        "/entityOperations/upsert",
        Some("nx1"),
        Some(upsert_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{got}");
    let (status, _) = send(
        &st,
        "GET",
        "/entities/urn:ngsi-ld:Vehicle:p1",
        Some("nx1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let st = with(Profile {
        implicit_tenants_listed_only: true,
        ..Profile::default()
    });
    let (status, got) = send(
        &st,
        "POST",
        "/entityOperations/upsert",
        Some("nx1"),
        Some(upsert_body()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{got}");
    assert_eq!(
        error_type(&got),
        "https://uri.etsi.org/ngsi-ld/errors/NonexistentTenant",
        "{got}"
    );
    let (status, _) = send(
        &st,
        "GET",
        "/entities/urn:ngsi-ld:Vehicle:p1",
        Some("nx1"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the refused upsert created the tenant"
    );
    // Batch Create is on the 7.8.2 list and still creates the Tenant
    let (status, got) = send(
        &st,
        "POST",
        "/entityOperations/create",
        Some("nx1"),
        Some(upsert_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{got}");
    let (status, got) = send(
        &st,
        "POST",
        "/entityOperations/upsert",
        Some("nx1"),
        Some(upsert_body()),
    )
    .await;
    assert!(
        status.is_success(),
        "an existing tenant takes the upsert: {status} {got}"
    );
}

/// A Context Source answering every request 404 with a ProblemDetails.
fn source_404() -> u16 {
    let body =
        r#"{"type":"https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound","title":"Not Found"}"#;
    let reply = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 65536];
            let _ = s.read(&mut buf);
            let _ = s.write_all(reply.as_bytes());
        }
    });
    port
}

async fn delete_beside_a_source_404(profile: Profile) -> StatusCode {
    antares_jsonld::allow_private_egress(true);
    let st = with(profile);
    let id = "urn:ngsi-ld:Vehicle:d1";
    let entity =
        json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 1}}).to_string();
    let (status, got) = send(&st, "POST", "/entities", None, Some(entity)).await;
    assert_eq!(status, StatusCode::CREATED, "{got}");
    let csr = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:p404",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "operations": ["deleteEntity"],
        "information": [{"entities": [{"id": id, "type": "Vehicle"}]}],
        "endpoint": format!("http://127.0.0.1:{}", source_404()),
    })
    .to_string();
    let (status, got) = send(&st, "POST", "/csourceRegistrations", None, Some(csr)).await;
    assert_eq!(status, StatusCode::CREATED, "{got}");
    let (status, _) = send(&st, "DELETE", &format!("/entities/{id}"), None, None).await;
    status
}

/// ANTARES_FED_WRITE_SOURCE_404: a local delete beside a source's 404 is a
/// 207 by default (ETSI D002_02_01_inc), 204 under `ignore` (forge
/// all-fixes, Coraine distop_delete_inclusive_source_404 step 03).
#[tokio::test(flavor = "multi_thread")]
async fn distributed_delete_beside_a_source_404_per_profile_value() {
    assert_eq!(
        delete_beside_a_source_404(Profile::default()).await,
        StatusCode::MULTI_STATUS
    );
    let ignore = Profile {
        fed_write_ignore_404: true,
        ..Profile::default()
    };
    assert_eq!(
        delete_beside_a_source_404(ignore).await,
        StatusCode::NO_CONTENT
    );
}

/// The keys parse their two documented values; anything else is a startup
/// error naming the key.
#[test]
fn profile_keys_parse_two_values_and_refuse_others() {
    let s = |v: &str| Some(v.to_owned());
    assert_eq!(Profile::from_vars(None, None, None), Ok(Profile::default()));
    assert_eq!(
        Profile::from_vars(s("invalid-request"), s("report"), s("with-upsert")),
        Ok(Profile::default())
    );
    assert_eq!(
        Profile::from_vars(s("bad-request-data"), s("ignore"), s("listed")),
        Ok(Profile {
            non_object_bad_request_data: true,
            fed_write_ignore_404: true,
            implicit_tenants_listed_only: true,
        })
    );
    let err = Profile::from_vars(None, s("drop"), None).expect_err("unknown value");
    assert!(err.contains("ANTARES_FED_WRITE_SOURCE_404"), "{err}");
}
