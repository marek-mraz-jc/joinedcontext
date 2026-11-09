#![forbid(unsafe_code)]
//! Refresh tokens are opaque, rotated handles with reuse detection (W1 grants, W2 sessions):
//! `{family id}.{256-bit random}`, only the SHA-256 stored, one winner per rotation, a
//! rotated-away handle revokes the family (refuse/reuse cases on both stores: refresh_reuse.rs),
//! a forged handle revokes nothing.

use axum::http::StatusCode;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::sha256_hex;
use kaisatsu_server::router;
use serde_json::Value;

mod common;
use common::*;

const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const INTROSPECT: &str = "/realms/dev/protocol/openid-connect/token/introspect";
const REVOKE: &str = "/realms/dev/protocol/openid-connect/revoke";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("json body ({e}): {body}"))
}

async fn password_tokens(app: &axum::Router, scope: &str) -> Value {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
            ("scope", scope),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    json(&body)
}

async fn refresh(app: &axum::Router, token: &str) -> (StatusCode, Value) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("refresh_token", token),
        ],
        None,
    )
    .await;
    (st, json(&body))
}

async fn introspect(app: &axum::Router, token: &str) -> Value {
    let (st, _, body) = post_form(
        app,
        INTROSPECT,
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", token),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    json(&body)
}

fn family_id(token: &str) -> &str {
    token.split_once('.').expect("handle has a family id").0
}

#[tokio::test]
async fn refresh_token_is_an_opaque_handle_stored_only_as_its_hash() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let tokens = password_tokens(&app, "openid").await;
    let rt = tokens["refresh_token"].as_str().expect("refresh token");
    assert_eq!(rt.matches('.').count(), 1, "not a JWT: {rt}");
    let (fid, secret) = rt.split_once('.').expect("two parts");
    assert_eq!(secret.len(), 43, "256 random bits, base64url");
    let fam = store
        .get_refresh_family("dev", fid)
        .await
        .unwrap()
        .expect("family stored");
    assert_eq!(fam.current_jti_hash, sha256_hex(rt));
    let stored = serde_json::to_string(&fam).unwrap();
    assert!(
        !stored.contains(secret),
        "the handle itself is never stored"
    );
    assert_eq!(fam.client_id, "apisix-gateway");
    assert!(tokens["refresh_expires_in"].as_i64().unwrap() > 0);

    let info = introspect(&app, rt).await;
    assert_eq!(info["active"], true, "{info}");
    assert_eq!(info["typ"], "Refresh");
    assert_eq!(info["client_id"], "apisix-gateway");
    assert_eq!(info["sid"], Value::String(fam.sso_session_id.clone()));
}

#[tokio::test]
async fn rotation_and_reuse_detection_revoke_the_family_and_keep_the_session() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let first = password_tokens(&app, "openid").await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (st, body) = refresh(&app, &first).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let second = body["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(first, second);
    assert_eq!(
        family_id(&first),
        family_id(&second),
        "a rotation keeps the family"
    );
    assert_eq!(introspect(&app, &first).await["active"], false);
    assert_eq!(introspect(&app, &second).await["active"], true);

    let fam = store
        .get_refresh_family("dev", family_id(&first))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fam.rotation_counter, 2);
    assert_eq!(fam.previous_hashes, [sha256_hex(&first)]);

    // The rotated-away handle again: reuse, the family ends and the SSO session stays (Keycloak
    // keeps the IdP session, the client signs the person in again).
    let (st, body) = refresh(&app, &first).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
    let fam = store
        .get_refresh_family("dev", family_id(&first))
        .await
        .unwrap()
        .unwrap();
    assert!(fam.revoked);
    assert!(
        store
            .get_sso_session("dev", &fam.sso_session_id)
            .await
            .unwrap()
            .is_some()
    );
    let (st, _) = refresh(&app, &second).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "the newest handle dies with the family"
    );
}

#[tokio::test]
async fn a_forged_or_foreign_handle_revokes_nothing() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let rt = password_tokens(&app, "openid").await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let fid = family_id(&rt).to_owned();
    // Someone who learned the family id (it is the offline session id, visible to admins)
    // but not the secret must not be able to end the session.
    let forged = format!("{fid}.{}", kaisatsu_crypto::random_handle_256());
    for bad in [forged.as_str(), "x.y", "nodot", ".", "a.", ".b"] {
        let (st, body) = refresh(&app, bad).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(body["error"], "invalid_grant", "{bad}");
        assert_eq!(introspect(&app, bad).await["active"], false);
    }
    assert!(
        !store
            .get_refresh_family("dev", &fid)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );

    // Another confidential client presenting the handle is refused and changes nothing.
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("refresh_token", &rt),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        !store
            .get_refresh_family("dev", &fid)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    let (st, _) = refresh(&app, &rt).await;
    assert_eq!(st, StatusCode::OK, "the real owner still refreshes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_refresh_with_one_handle_has_one_winner() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let rt = password_tokens(&app, "openid").await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let racers = 16;
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(racers));
    let mut handles = Vec::new();
    for _ in 0..racers {
        let app = app.clone();
        let rt = rt.clone();
        let start = start.clone();
        handles.push(tokio::spawn(async move {
            start.wait().await;
            refresh(&app, &rt).await.0
        }));
    }
    let mut ok = 0;
    for h in handles {
        if h.await.unwrap() == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 1, "exactly one request rotates a handle");
    let fam = store
        .get_refresh_family("dev", family_id(&rt))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fam.rotation_counter, 2);
}

#[tokio::test]
async fn revocation_ends_the_handle_only_for_its_own_client() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let rt = password_tokens(&app, "openid").await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    // RFC 7009: another client's revocation request is answered 200 and does nothing.
    let (st, _, _) = post_form(
        &app,
        REVOKE,
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", &rt),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(introspect(&app, &rt).await["active"], true);

    let (st, _, _) = post_form(
        &app,
        REVOKE,
        &[
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("token", &rt),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        store
            .get_refresh_family("dev", family_id(&rt))
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    assert_eq!(introspect(&app, &rt).await["active"], false);
    let (st, _) = refresh(&app, &rt).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn offline_handles_report_offline_and_revocation_removes_the_offline_session() {
    let (state, store, _) = setup_dev_realm_state().await;
    // ri_dev's gateway client does not offer offline_access; offer it for this test.
    let mut gateway = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gateway.optional_client_scopes.push("offline_access".into());
    store.upsert_client(gateway).await.unwrap();
    let app = router(state, mount());
    let tokens = password_tokens(&app, "openid offline_access").await;
    let rt = tokens["refresh_token"].as_str().unwrap().to_owned();
    assert_eq!(
        tokens["refresh_expires_in"], 0,
        "offline tokens do not idle out per session"
    );
    let info = introspect(&app, &rt).await;
    assert_eq!(info["active"], true, "{info}");
    assert_eq!(info["typ"], "Offline");
    assert!(info.get("sid").is_none());
    let fid = family_id(&rt).to_owned();

    let ctx = RequestCtx::new(
        "dev",
        Principal {
            subject: Some("root".into()),
            client_id: None,
            realm_roles: vec!["realm-admin".into()],
            client_roles: Default::default(),
            organizations: Vec::new(),
            org_roles: Default::default(),
            second_factor: true,
        },
    );
    let offline_count = |store: std::sync::Arc<kaisatsu_store_memory::MemoryStore>| {
        let ctx = ctx.clone();
        async move {
            store
                .query_entity_set(
                    &ctx,
                    &LocalAuthorizer,
                    "OfflineSessions",
                    &odata_table::QueryOptions {
                        count: Some(true),
                        ..Default::default()
                    },
                    100,
                )
                .await
                .unwrap()
                .count
        }
    };
    assert_eq!(offline_count(store.clone()).await, Some(1));
    let (st, _, _) = post_form(
        &app,
        REVOKE,
        &[
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
            ("token", &rt),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(offline_count(store.clone()).await, Some(0));
    assert!(
        store
            .get_refresh_family("dev", &fid)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    let (st, _) = refresh(&app, &rt).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}
