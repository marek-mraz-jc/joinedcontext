#![forbid(unsafe_code)]
//! Dataroom names a caller by a signed, audience-bound token: a static shared secret names nobody.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use dataroom_api::{portal_publisher, router, DataroomState, AUDIENCE};
use dataroom_core::{Catalogue, CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use jc_door::{testing::Issuer, Verifier};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT: &str = "portal-publisher";

/// A catalogue whose publisher is the Portal, without a verifier.
async fn unverified() -> DataroomState {
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
    let (subject, grant) = portal_publisher(org, cat);
    let authorizer = Arc::new(PolicyAuthorizer::new(vec![grant]));
    DataroomState::new(store, authorizer, org, cat).with_publishers([((org, cat), subject)].into())
}

fn verifying(state: DataroomState, issuer: &Issuer) -> DataroomState {
    let verifier = Verifier::new(&issuer.issuer, &issuer.jwks_url, &[CLIENT.to_owned()]).unwrap();
    state.with_verifier(Arc::new(verifier))
}

async fn create(state: &DataroomState, name: &str, header_name: &str, value: &str) -> StatusCode {
    let req = Request::post("/api/3/action/organization_create")
        .header(header::HOST, "data.city.test")
        .header(header_name, value)
        .body(Body::from(json!({ "name": name }).to_string()))
        .unwrap();
    router(state.clone()).oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn a_static_bearer_is_refused() {
    let issuer = Issuer::start("https://id.city.test", "k1").await;
    let state = verifying(unverified().await, &issuer);
    let secret = Uuid::now_v7().simple().to_string().repeat(2);
    for header_name in ["authorization", "x-ckan-api-key"] {
        for value in [secret.clone(), format!("Bearer {secret}")] {
            let status = create(&state, "city", header_name, &value).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{header_name}");
        }
    }
}

#[tokio::test]
async fn only_a_signed_token_of_the_admitted_client_for_dataroom_writes() {
    let issuer = Issuer::start("https://id.city.test", "k1").await;
    let state = verifying(unverified().await, &issuer);
    // Signed with another key under the same key id.
    let forger = Issuer::start("https://id.city.test", "k1").await;
    for (case, token) in [
        ("another audience", issuer.token(CLIENT, "gitea", 300)),
        ("another client", issuer.token("portal-api", AUDIENCE, 300)),
        ("expired", issuer.token(CLIENT, AUDIENCE, -120)),
        ("another key", forger.token(CLIENT, AUDIENCE, 300)),
    ] {
        let status = create(&state, "city", "authorization", &token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{case}");
    }
    let token = issuer.token(CLIENT, AUDIENCE, 300);
    for (name, header_name, value) in [
        ("bare", "authorization", token.clone()),
        ("lower-case", "authorization", format!("bearer {token}")),
        ("key-header", "x-ckan-api-key", token.clone()),
    ] {
        let status = create(&state, name, header_name, &value).await;
        assert_eq!(status, StatusCode::OK, "{name}");
    }
}

#[tokio::test]
async fn a_valid_token_is_nobody_without_a_verifier_or_a_publisher() {
    let issuer = Issuer::start("https://id.city.test", "k1").await;
    let token = issuer.token(CLIENT, AUDIENCE, 300);
    let no_verifier = unverified().await;
    let no_publisher = verifying(unverified().await, &issuer).with_publishers(Default::default());
    for state in [no_verifier, no_publisher] {
        let status = create(&state, "city", "authorization", &token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
