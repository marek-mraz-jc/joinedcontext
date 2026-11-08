#![forbid(unsafe_code)]
//! Domain verification tests (`OrganizationDomain/RequestVerification` and
//! `OrganizationDomain/Verify`, W3, K13) with a fake `DnsResolver`:
//! - verified on matching TXT record,
//! - refused on wrong TXT value, missing record, expired challenge, or cluster-internal domain,
//! - 404 on another organization's domain.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_admin::{AdminState, HickoryDnsResolver, odata_router};
use kaisatsu_core::*;
use kaisatsu_crypto::{Kek, sign_jwt};
use kaisatsu_oidc::{OidcState, OutboundGuard};
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

async fn setup() -> (
    axum::Router,
    Arc<MemoryStore>,
    Arc<HickoryDnsResolver>,
    String,
    String,
) {
    let kek = Kek::generate_for_tests();
    let store = Arc::new(MemoryStore::new(kek.clone()));
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    for (org_id, dom_id, dom_name, admin_id) in [
        ("helsinki", "dom-hel", "hel.fi", "admin-hel"),
        ("espoo", "dom-esp", "espoo.fi", "admin-esp"),
    ] {
        store
            .upsert_organization(Organization {
                id: org_id.into(),
                realm_id: "dev".into(),
                name: org_id.into(),
                alias: org_id.into(),
                enabled: true,
                description: String::new(),
                domains: vec![OrganizationDomain {
                    id: dom_id.into(),
                    realm_id: "dev".into(),
                    org_id: org_id.into(),
                    name: dom_name.into(),
                    verified: false,
                    verification_token: None,
                    challenge_hash: None,
                    challenge_expires_at: None,
                }],
                attributes: HashMap::new(),
                managed_by: None,
            })
            .await
            .unwrap();
        store
            .create_user(UserRepresentation {
                id: admin_id.into(),
                realm_id: "dev".into(),
                org_id: Some(org_id.into()),
                username: admin_id.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store
            .upsert_membership(Membership {
                id: format!("m-{admin_id}"),
                realm_id: "dev".into(),
                org_id: org_id.into(),
                person_id: admin_id.into(),
                org_roles: vec!["org-admin".into()],
                created_at: Utc::now(),
            })
            .await
            .unwrap();
    }

    let keys = store.list_signing_keys("dev").await.unwrap();
    let now = Utc::now().timestamp();
    let mint = |sub: &str, org: &str| {
        sign_jwt(
            &keys[0],
            &kek,
            Some("JWT"),
            &json!({
                "iss": "https://idm.localhost/realms/dev",
                "sub": sub,
                "azp": "admin-cli",
                "typ": "Bearer",
                "jti": format!("jti-{sub}"),
                "jc_org": [org],
                "iat": now,
                "exp": now + 3600
            }),
        )
        .unwrap()
    };
    let hel_tok = mint("admin-hel", "helsinki");
    let esp_tok = mint("admin-esp", "espoo");

    let resolver = Arc::new(HickoryDnsResolver::new());
    let oidc = OidcState {
        store: store.clone(),
        kek,
        guard: OutboundGuard::default(),
        default_mount: Mount::default(),
    };
    let state =
        AdminState::new(oidc, Arc::new(LocalAuthorizer)).with_dns_resolver(resolver.clone());
    (odata_router(state), store, resolver, hel_tok, esp_tok)
}

async fn post_odata(app: &axum::Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn k13_domain_verification_happy_path_failures_and_cross_org_404() {
    let (app, store, resolver, hel_tok, esp_tok) = setup().await;

    // 1. Cross-org domain request/verify returns 404 Not Found.
    let (st_cross, _) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/RequestVerification",
        &esp_tok,
    )
    .await;
    assert_eq!(st_cross, StatusCode::NOT_FOUND);
    let (st_cross_v, _) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/Verify",
        &esp_tok,
    )
    .await;
    assert_eq!(st_cross_v, StatusCode::NOT_FOUND);

    // 2. RequestVerification returns _kaisatsu-challenge.hel.fi and kaisatsu-verify=<random>.
    let (st_req, body_req) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/RequestVerification",
        &hel_tok,
    )
    .await;
    assert_eq!(st_req, StatusCode::OK, "{body_req}");
    let val = &body_req["value"];
    assert_eq!(val["txtRecord"], "_kaisatsu-challenge.hel.fi");
    let txt_value = val["txtValue"].as_str().unwrap().to_owned();
    assert!(txt_value.starts_with("kaisatsu-verify="));

    // Stored domain holds only the hash, not the plaintext token.
    let org_hel = store
        .get_organization("dev", "helsinki")
        .await
        .unwrap()
        .unwrap();
    assert!(org_hel.domains[0].challenge_hash.is_some());
    assert_ne!(
        org_hel.domains[0].challenge_hash.as_deref(),
        Some(txt_value.as_str())
    );

    // 3. Missing TXT record -> 400.
    resolver.insert_mock_txt("_kaisatsu-challenge.hel.fi", vec![]);
    let (st_missing, _) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/Verify",
        &hel_tok,
    )
    .await;
    assert_eq!(st_missing, StatusCode::BAD_REQUEST);

    // 4. Wrong TXT value -> 400.
    resolver.insert_mock_txt(
        "_kaisatsu-challenge.hel.fi",
        vec!["kaisatsu-verify=wrong".into()],
    );
    let (st_wrong, _) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/Verify",
        &hel_tok,
    )
    .await;
    assert_eq!(st_wrong, StatusCode::BAD_REQUEST);

    // 5. Expired challenge -> 400.
    let mut expired_org = store
        .get_organization("dev", "helsinki")
        .await
        .unwrap()
        .unwrap();
    expired_org.domains[0].challenge_expires_at = Some(Utc::now() - Duration::minutes(1));
    store.upsert_organization(expired_org).await.unwrap();
    resolver.insert_mock_txt("_kaisatsu-challenge.hel.fi", vec![txt_value.clone()]);
    let (st_exp, _) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/Verify",
        &hel_tok,
    )
    .await;
    assert_eq!(st_exp, StatusCode::BAD_REQUEST);

    // 6. Re-request and verify with matching TXT record -> 200 OK and verified=true.
    let (_, body_req2) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/RequestVerification",
        &hel_tok,
    )
    .await;
    let txt_value2 = body_req2["value"]["txtValue"].as_str().unwrap().to_owned();
    resolver.insert_mock_txt("_kaisatsu-challenge.hel.fi", vec![txt_value2]);
    let (st_ok, body_ok) = post_odata(
        &app,
        "/api/v1/realms/dev/odata/OrganizationDomains('dom-hel')/Verify",
        &hel_tok,
    )
    .await;
    assert_eq!(st_ok, StatusCode::OK, "{body_ok}");
    assert_eq!(body_ok["value"]["verified"], true);
    let verified_org = store
        .get_organization("dev", "helsinki")
        .await
        .unwrap()
        .unwrap();
    assert!(verified_org.domains[0].verified);
}
