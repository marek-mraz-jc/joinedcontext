#![forbid(unsafe_code)]
//! Edge paths of the shared OIDC helpers (W1, W3, K2, K4, P-4-01): an organization scope named by
//! id and by alias is granted once, a parameterized scope without a pattern takes any parameter,
//! an authorization response to a redirect URI that is no URL is a 400, a password hash that
//! panics frees its slot, and back-channel logout skips a client whose pairwise subject cannot be
//! computed while the session's other clients still get their logout token.

use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_oidc::handlers::{redirect_with_params, resolve_scopes_for_client};
use kaisatsu_oidc::{
    OidcState, OutboundGuard, SecondFactor, dispatch_backchannel_logout, password_work,
};
use kaisatsu_store_memory::MemoryStore;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

const REALM: &str = "r1";

fn realm() -> RealmConfig {
    RealmConfig {
        id: REALM.into(),
        realm: REALM.into(),
        ..RealmConfig::default()
    }
}

fn store() -> Arc<dyn Store> {
    Arc::new(MemoryStore::new(Kek::generate_for_tests()))
}

fn scope(name: &str, attributes: &[(&str, &str)]) -> ClientScopeRepresentation {
    ClientScopeRepresentation {
        id: format!("sc-{name}"),
        realm_id: REALM.into(),
        name: name.into(),
        protocol: "openid-connect".into(),
        attributes: attributes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn an_organization_named_by_id_and_by_alias_is_granted_once() {
    let store = store();
    store
        .upsert_organization(Organization {
            id: "org-acme".into(),
            realm_id: REALM.into(),
            name: "Acme".into(),
            alias: "acme".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: HashMap::new(),
            managed_by: None,
        })
        .await
        .unwrap();
    let client = ClientRepresentation::default();
    let (_, granted) = resolve_scopes_for_client(
        &store,
        &realm(),
        &client,
        Some("openid organization:acme organization:org-acme"),
    )
    .await
    .unwrap();
    assert_eq!(granted.matches("organization:").count(), 1, "{granted}");
    assert!(granted.ends_with(" organization:acme"), "{granted}");
    // An organization the realm lacks is refused, whatever else was asked for.
    let refused = resolve_scopes_for_client(
        &store,
        &realm(),
        &client,
        Some("openid organization:acme organization:ghost"),
    )
    .await;
    assert!(
        matches!(&refused, Err(KaisatsuError::OAuth { error, .. }) if error == "invalid_scope"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_parameterized_scope_without_a_pattern_takes_any_parameter_once() {
    let store = store();
    let conn = scope(
        "conn",
        &[
            ("is.parameterized.scope", "true"),
            ("include.in.token.scope", "true"),
        ],
    );
    store.upsert_client_scope(conn).await.unwrap();
    let client = ClientRepresentation {
        optional_client_scopes: vec!["conn".into()],
        ..Default::default()
    };
    let (active, _) = resolve_scopes_for_client(
        &store,
        &realm(),
        &client,
        Some("openid conn:Any-Value_1 conn:other"),
    )
    .await
    .unwrap();
    assert_eq!(
        active.iter().filter(|s| s.name == "conn").count(),
        1,
        "{active:?}"
    );
    // The family is still bound to the client: one that does not list it is refused.
    let other = ClientRepresentation::default();
    let refused = resolve_scopes_for_client(&store, &realm(), &other, Some("conn:x")).await;
    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn an_authorization_response_to_a_redirect_uri_that_is_no_url_is_a_400() {
    for mode in ["query", "fragment"] {
        let resp = redirect_with_params("not a url", mode, &[("code", "c")]);
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST, "{mode}");
        assert!(
            resp.headers().get(http::header::LOCATION).is_none(),
            "{mode}"
        );
    }
    let resp = redirect_with_params("", "query", &[]);
    assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_password_hash_that_panics_is_a_crypto_error_and_frees_its_slot() {
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
    // More panics than slots: each one gives its slot back, or the last call would wait forever.
    for _ in 0..=cores {
        let failed = password_work(|| -> u8 { panic!("hash panicked") }).await;
        assert!(
            matches!(&failed, Err(KaisatsuError::Crypto(m)) if m == "password work failed"),
            "{failed:?}"
        );
    }
    assert_eq!(password_work(|| 7_u8).await.unwrap(), 7);
}

fn backchannel_client(client_id: &str, url: &str, pairwise: bool) -> ClientRepresentation {
    ClientRepresentation {
        id: format!("uuid-{client_id}"),
        realm_id: REALM.into(),
        client_id: client_id.into(),
        enabled: true,
        attributes: BTreeMap::from([("backchannel.logout.url".to_owned(), url.to_owned())]),
        protocol_mappers: if pairwise {
            vec![ProtocolMapperRepresentation {
                name: "pairwise".into(),
                protocol: "openid-connect".into(),
                protocol_mapper: "oidc-sha256-pairwise-sub-mapper".into(),
                ..Default::default()
            }]
        } else {
            Vec::new()
        },
        ..Default::default()
    }
}

#[tokio::test]
async fn back_channel_logout_skips_a_client_whose_pairwise_subject_cannot_be_computed() {
    let kek = Kek::generate_for_tests();
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    store.upsert_realm(realm()).await.unwrap();
    store.ensure_signing_key(REALM, "ES256").await.unwrap();
    let broken = "https://broken.example.test/logout";
    let fine = "https://fine.example.test/logout";
    // No single redirect host and no sector URI: the pairwise subject cannot be computed.
    store
        .upsert_client(backchannel_client("pw", broken, true))
        .await
        .unwrap();
    store
        .upsert_client(backchannel_client("plain", fine, false))
        .await
        .unwrap();
    let guard = OutboundGuard::default();
    guard.insert_mock(broken, 200, "");
    guard.insert_mock(fine, 200, "");
    let state = OidcState {
        store,
        kek,
        guard: guard.clone(),
        default_mount: Mount::new("kaisatsu", "https", "idm.example.test", "").unwrap(),
        second_factor: SecondFactor::default(),
    };
    let now = jc_time::TimestampTz::now();
    let session = SsoSession {
        id: "sso-1".into(),
        realm_id: REALM.into(),
        org_id: None,
        handle_hash: "h".into(),
        user_id: "user-1".into(),
        username: "person".into(),
        ip_address: "127.0.0.1".into(),
        user_agent: String::new(),
        start: now.as_millis(),
        last_access: now.as_millis(),
        auth_time: now.as_second(),
        acr: "pwd".into(),
        amr: vec!["pwd".into()],
        clients: HashMap::from([
            ("pw".to_owned(), String::new()),
            ("plain".to_owned(), String::new()),
            ("gone".to_owned(), String::new()),
        ]),
        idle_expires_at: now + jiff::SignedDuration::from_hours(1),
        max_expires_at: now + jiff::SignedDuration::from_hours(8),
    };
    let mount = state.default_mount.clone();
    dispatch_backchannel_logout(&state, &mount, &realm(), &session)
        .await
        .unwrap();
    assert!(
        guard.posted_forms(broken).is_empty(),
        "no logout token with a local subject the pairwise mapper hides"
    );
    let posted = guard.posted_forms(fine);
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(posted[0][0].0, "logout_token");
}

#[tokio::test]
async fn a_route_that_names_no_realm_passes_the_realm_check_and_an_unknown_realm_is_404() {
    use tower::ServiceExt;
    let kek = Kek::generate_for_tests();
    let state = OidcState {
        store: Arc::new(MemoryStore::new(kek.clone())),
        kek,
        guard: OutboundGuard::default(),
        default_mount: Mount::new("kaisatsu", "https", "idm.example.test", "").unwrap(),
        second_factor: SecondFactor::default(),
    };
    let app = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/realms/{realm}/x", axum::routing::get(|| async { "ok" }))
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            kaisatsu_oidc::handlers::active_realm,
        ));
    let get = |uri: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(
                http::Request::get(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };
    assert_eq!(get("/health").await, http::StatusCode::OK);
    assert_eq!(get("/realms/ghost/x").await, http::StatusCode::NOT_FOUND);
}
