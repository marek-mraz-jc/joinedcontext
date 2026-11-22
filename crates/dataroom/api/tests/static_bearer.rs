#![forbid(unsafe_code)]
//! Dataroom names a caller by a signed, audience-bound token: a static shared secret names nobody.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{portal_publisher, router, DataroomState};
use dataroom_core::{Catalogue, CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn a_static_bearer_is_refused() {
    let (org, cat) = (
        OrgId::from_uuid(Uuid::now_v7()),
        CatalogueId::from_uuid(Uuid::now_v7()),
    );
    let store = Arc::new(MemoryStore::new());
    let catalogue: Catalogue = serde_json::from_value(json!({
        "orgId": org, "id": cat, "name": "city", "host": "data.city.test",
        "title": {"sk": "Katalóg"}, "description": {},
        "defaultLanguage": "sk", "languages": ["sk"],
        "mainProfile": "dcat-ap-3.0.1", "activeProfiles": [],
        "modified": "2026-11-01T10:00:00Z"
    }))
    .unwrap();
    store.upsert_catalogue(catalogue).await.unwrap();
    let secret = Uuid::now_v7().simple().to_string().repeat(2);
    let (binding, grant) = portal_publisher(&secret, org, cat).unwrap();
    let state = DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::new(vec![grant])),
        org,
        cat,
    )
    .with_tokens(vec![binding]);
    for header_name in [header::AUTHORIZATION.as_str(), "x-ckan-api-key"] {
        let req = Request::post("/api/3/action/organization_create")
            .header(header::HOST, "data.city.test")
            .header(header_name, &secret)
            .body(Body::from(r#"{"name":"city"}"#))
            .unwrap();
        let answer = router(state.clone()).oneshot(req).await.unwrap();
        assert_eq!(answer.status(), StatusCode::UNAUTHORIZED, "{header_name}");
    }
}
