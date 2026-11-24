#![forbid(unsafe_code)]
//! The process-wide cap on the bytes open chunked uploads hold, in a test binary of its own: the
//! cap is shared by every upload of the process. Two uploads holding one package cap each fill
//! it; a chunk that would pass it is told to retry, even when its own blob is far below the cap.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use shako_api::ShakoState;
use shako_api::service::MAX_PACKAGE_BLOB_BYTES;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_registry::oci_v2_routes;
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const PUSHER: &str = "pusher-token-0123456789";

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Bytes,
) -> (StatusCode, String, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {PUSHER}"))
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, location, body)
}

#[tokio::test]
async fn a_chunk_past_the_open_upload_bytes_is_told_to_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let org = OrgId::from_u128(10);
    let state = ShakoState::new(
        InstanceId::from_u128(1),
        tmp.path().to_path_buf(),
        Arc::new(MemoryStore::new()),
        Arc::new(LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("pusher".into()),
            actions: vec![Action::Publish, Action::Read],
            project_id: None,
            repo: None,
        }])),
        TokenVerifier::new(ShakoSigningKey::generate().expect("key")).with_static_tokens(vec![
            StaticTokenBinding::new(
                PUSHER,
                Subject::authenticated("pusher", "pusher", vec![org]),
            ),
        ]),
        OutboundGuard::default(),
    );
    let app = oci_v2_routes().with_state(state);
    let full = Bytes::from(vec![0u8; MAX_PACKAGE_BLOB_BYTES]);
    let open = "/v2/city/app/blobs/uploads/";
    for n in 0..2 {
        let (status, _, body) = send(&app, Method::POST, open, full.clone()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "upload {n}: {body:?}");
    }
    drop(full);
    let (status, location, _) = send(&app, Method::POST, open, Bytes::new()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "an empty upload still fits");

    let (status, _, body) = send(&app, Method::PATCH, &location, Bytes::from_static(b"x")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["errors"][0]["code"], "TOOMANYREQUESTS");

    // The refused chunk was not kept: the empty upload still holds nothing.
    let (status, _, _) = send(&app, Method::PATCH, &location, Bytes::new()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}
