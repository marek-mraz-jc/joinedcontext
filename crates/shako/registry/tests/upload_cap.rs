#![forbid(unsafe_code)]
//! The process-wide cap on open chunked uploads, in a test binary of its own: the cap is shared
//! by every upload of the process, so it would starve the uploads of the other tests.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use shako_api::ShakoState;
use shako_core::{
    Action, InstanceId, LocalAuthorizer, OrgId, OutboundGuard, Principal, RoleBinding,
    ShakoSigningKey, StaticTokenBinding, Subject, TokenVerifier,
};
use shako_registry::oci_v2_routes;
use shako_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

const PUSHER: &str = "pusher-token-0123456789";

#[tokio::test]
async fn an_upload_past_the_open_upload_cap_is_told_to_retry() {
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
    let open = || async {
        let req = Request::builder()
            .method("POST")
            .uri("/v2/city/app/blobs/uploads/")
            .header(header::AUTHORIZATION, format!("Bearer {PUSHER}"))
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(req).await.unwrap()
    };
    let mut accepted = 0;
    let refused = loop {
        let resp = open().await;
        if resp.status() != StatusCode::ACCEPTED {
            break resp;
        }
        accepted += 1;
        assert!(accepted <= 32, "the cap holds at 32 open uploads");
    };
    assert_eq!(accepted, 32);
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = refused.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["errors"][0]["code"], "TOOMANYREQUESTS");
}
