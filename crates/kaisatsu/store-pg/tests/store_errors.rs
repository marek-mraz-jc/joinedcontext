#![forbid(unsafe_code)]
//! Error paths and less travelled methods of both stores: WebAuthn credential update, list and
//! organization-scoped delete (K11), identity-provider links (K12), outbox backoff without a
//! given retry time (K9), sessions removed with their person, the K entity sets over OData, and
//! the PostgreSQL boot and migration refusals. Runs against `MemoryStore` always and `PgStore`
//! with `KAISATSU_TEST_DATABASE_URL`.

use chrono::{Duration, Utc};
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::{PgStore, verify_boot_rls};
use std::collections::HashMap;
use std::sync::Arc;

mod common;

const R: &str = "dev";

fn person(id: &str, org: &str) -> UserRepresentation {
    UserRepresentation {
        id: id.into(),
        realm_id: R.into(),
        org_id: Some(org.into()),
        username: format!("{id}@example.test"),
        email: Some(format!("{id}@example.test")),
        enabled: true,
        ..Default::default()
    }
}

fn passkey(id: &str, user: &str, org: &str) -> WebAuthnCredentialRecord {
    WebAuthnCredentialRecord {
        id: id.into(),
        realm_id: R.into(),
        org_id: Some(org.into()),
        user_id: user.into(),
        user_handle: format!("handle-{id}"),
        credential_id: format!("cred-{id}"),
        public_key_cose_json: r#"{"kty":2,"alg":-7,"crv":1,"x":"AA","y":"BB"}"#.into(),
        cose_alg: -7,
        sign_count: 1,
        uv_initialized: true,
        backup_eligible: false,
        backup_state: false,
        transports: vec!["usb".into()],
        attestation_type: "None".into(),
        rp_id: "idm.example.test".into(),
        user_label: Some("Key".into()),
        created_at: Utc::now(),
        last_used_at: None,
    }
}

fn outbox_row(id: &str, user: &str) -> EmailOutboxRecord {
    EmailOutboxRecord {
        id: id.into(),
        realm_id: R.into(),
        org_id: Some("helsinki".into()),
        user_id: user.into(),
        to_address: format!("{user}@example.test"),
        locale: "en".into(),
        template_kind: "EXECUTE_ACTIONS".into(),
        action_jti: format!("jti-{id}"),
        actions: vec!["UPDATE_PASSWORD".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: "https://idm.example.test/realms/dev".into(),
        status: "pending".into(),
        attempts: 0,
        last_error: None,
        next_attempt_at: Utc::now() - Duration::seconds(1),
        created_at: Utc::now(),
        sent_at: None,
    }
}

fn sso(id: &str, user: &str, expires_in: Duration) -> SsoSession {
    let now = Utc::now();
    SsoSession {
        id: id.into(),
        realm_id: R.into(),
        org_id: Some("helsinki".into()),
        handle_hash: format!("hash-{id}"),
        user_id: user.into(),
        username: user.into(),
        ip_address: "192.0.2.1".into(),
        start: now.timestamp(),
        last_access: now.timestamp(),
        auth_time: now.timestamp(),
        acr: "1".into(),
        amr: vec!["pwd".into()],
        clients: HashMap::new(),
        idle_expires_at: now + expires_in,
        max_expires_at: now + expires_in,
    }
}

fn offline(id: &str, user: &str, client: &str, expires_in: Duration) -> OfflineSession {
    let now = Utc::now();
    OfflineSession {
        id: id.into(),
        realm_id: R.into(),
        org_id: Some("helsinki".into()),
        user_id: user.into(),
        client_id: client.into(),
        scopes: vec!["offline_access".into()],
        created_at: now,
        last_access: now,
        idle_expires_at: now + expires_in,
        max_expires_at: now + expires_in,
    }
}

async fn seed(store: &Arc<dyn Store>) {
    store
        .upsert_realm(RealmConfig {
            id: R.into(),
            realm: R.into(),
            display_name: "Dev".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    for (id, org) in [("ann", "helsinki"), ("bob", "espoo")] {
        store.create_user(person(id, org)).await.unwrap();
    }
}

async fn passkeys_and_links(store: &Arc<dyn Store>) {
    // Updating a credential nobody stored is a 404, never an insert.
    let missing = store
        .update_webauthn_credential(passkey("pk-none", "ann", "helsinki"))
        .await;
    assert!(
        matches!(missing, Err(KaisatsuError::NotFound(_))),
        "{missing:?}"
    );

    store
        .insert_webauthn_credential(passkey("pk-ann", "ann", "helsinki"))
        .await
        .unwrap();
    store
        .insert_webauthn_credential(passkey("pk-bob", "bob", "espoo"))
        .await
        .unwrap();
    let mut used = passkey("pk-ann", "ann", "helsinki");
    used.sign_count = 7;
    used.last_used_at = Some(Utc::now());
    store.update_webauthn_credential(used).await.unwrap();
    let listed = store.list_webauthn_credentials(R, "ann").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].sign_count, 7);
    assert!(
        listed[0].public_key_cose_json.contains("\"kty\":2"),
        "the public key survives an update"
    );
    assert!(
        store
            .list_webauthn_credentials(R, "nobody")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .get_webauthn_by_credential_id(R, "cred-unknown")
            .await
            .unwrap()
            .is_none()
    );

    // Another organization's passkey is not deleted; the owner's organization deletes it once.
    let espoo = ["espoo".to_owned()];
    let helsinki = ["helsinki".to_owned()];
    assert!(
        !store
            .delete_webauthn_credential(R, Some(&espoo), "pk-ann")
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .list_webauthn_credentials(R, "ann")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .delete_webauthn_credential(R, Some(&helsinki), "pk-ann")
            .await
            .unwrap()
    );
    assert!(
        !store
            .delete_webauthn_credential(R, None, "pk-ann")
            .await
            .unwrap()
    );

    // Identity-provider links: one per (alias, external id), re-linking replaces it.
    assert!(
        store
            .get_idp_link_by_external(R, "upstream", "ext-1")
            .await
            .unwrap()
            .is_none()
    );
    let link = IdentityProviderLinkRecord {
        id: String::new(),
        realm_id: R.into(),
        org_id: Some("helsinki".into()),
        user_id: "ann".into(),
        idp_alias: "upstream".into(),
        external_user_id: "ext-1".into(),
        external_username: Some("ann.upstream".into()),
        created_at: Utc::now(),
    };
    store.upsert_idp_link(link.clone()).await.unwrap();
    let found = store
        .get_idp_link_by_external(R, "upstream", "ext-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.user_id, "ann");
    assert!(!found.id.is_empty(), "an id is minted");
    store
        .upsert_idp_link(IdentityProviderLinkRecord {
            id: "link-2".into(),
            user_id: "bob".into(),
            org_id: Some("espoo".into()),
            ..link
        })
        .await
        .unwrap();
    assert!(
        store
            .list_idp_links_for_user(R, "ann")
            .await
            .unwrap()
            .is_empty()
    );
    let bobs = store.list_idp_links_for_user(R, "bob").await.unwrap();
    assert_eq!(bobs.len(), 1);
    assert_eq!(bobs[0].id, "link-2");
}

async fn outbox_backoff(store: &Arc<dyn Store>) {
    // Updating an unknown person never queues the mail.
    let res = store
        .update_user_with_outbox(person("ghost", "helsinki"), outbox_row("ob-ghost", "ghost"))
        .await;
    assert!(matches!(res, Err(KaisatsuError::NotFound(_))), "{res:?}");
    assert!(store.list_outbox_rows(R, None).await.unwrap().is_empty());

    store
        .enqueue_outbox(outbox_row("ob-a", "ann"))
        .await
        .unwrap();
    let claimed = store.claim_outbox_batch(R, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    // A failed send under the attempt cap goes back to pending with a default backoff.
    let before = Utc::now();
    store
        .complete_outbox_row(R, "ob-a", false, Some("421 try later".into()), None, 5)
        .await
        .unwrap();
    let row = store
        .list_outbox_rows(R, None)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == "ob-a")
        .unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.attempts, 1);
    assert_eq!(row.last_error.as_deref(), Some("421 try later"));
    assert!(row.next_attempt_at > before, "backed off into the future");
    // Not due yet: nothing to claim.
    assert!(store.claim_outbox_batch(R, 10).await.unwrap().is_empty());
    // Retrying a row nobody knows is a 404 (None).
    assert!(
        store
            .retry_outbox_row(R, None, "ob-none")
            .await
            .unwrap()
            .is_none()
    );
}

async fn sessions_go_with_their_person(store: &Arc<dyn Store>) {
    store
        .upsert_sso_session(sso("s-live", "ann", Duration::hours(1)))
        .await
        .unwrap();
    store
        .upsert_sso_session(sso("s-old", "bob", -Duration::hours(1)))
        .await
        .unwrap();
    store
        .upsert_offline_session(offline("o-1", "ann", "portal", Duration::hours(1)))
        .await
        .unwrap();
    // The same offline session written twice stays one row.
    store
        .upsert_offline_session(offline("o-1", "ann", "portal", Duration::hours(2)))
        .await
        .unwrap();
    store
        .upsert_offline_session(offline("o-old", "bob", "portal", -Duration::hours(1)))
        .await
        .unwrap();
    store
        .upsert_consent(ConsentRecord {
            id: "c-1".into(),
            realm_id: R.into(),
            org_id: Some("helsinki".into()),
            user_id: "ann".into(),
            client_id: "portal".into(),
            granted_client_scopes: vec!["openid".into()],
            created_date: 1,
            last_updated_date: 1,
        })
        .await
        .unwrap();
    // Revoking consent also drops the client's offline sessions.
    assert!(store.delete_consent(R, "ann", "portal").await.unwrap());
    assert!(store.list_consents(R, "ann").await.unwrap().is_empty());

    // The sweeper removes the expired SSO and offline sessions.
    let removed = store.sweep_expired().await.unwrap();
    assert!(removed >= 2, "swept {removed}");
    assert!(store.get_sso_session(R, "s-old").await.unwrap().is_none());
    assert!(store.get_sso_session(R, "s-live").await.unwrap().is_some());

    // Deleting a person removes their sessions, passkeys and links.
    store
        .insert_webauthn_credential(passkey("pk-ann2", "ann", "helsinki"))
        .await
        .unwrap();
    assert!(store.delete_user(R, None, "ann").await.unwrap());
    assert!(store.get_sso_session(R, "s-live").await.unwrap().is_none());
    assert!(
        store
            .list_webauthn_credentials(R, "ann")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_realms()
            .await
            .unwrap()
            .iter()
            .any(|r| r.realm == R)
    );
}

/// The K entity sets read their rows through OData on both stores, scoped to the realm.
async fn k_entity_sets(store: &Arc<dyn Store>) {
    store
        .insert_webauthn_credential(passkey("pk-odata", "bob", "espoo"))
        .await
        .unwrap();
    store
        .upsert_idp_link(IdentityProviderLinkRecord {
            id: "link-odata".into(),
            realm_id: R.into(),
            org_id: Some("espoo".into()),
            user_id: "bob".into(),
            idp_alias: "upstream".into(),
            external_user_id: "ext-odata".into(),
            external_username: None,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    store
        .enqueue_outbox(outbox_row("ob-odata", "bob"))
        .await
        .unwrap();
    let service = kaisatsu_odata_service();
    let csdl = Arc::new(service.model());
    let edm = odata_url::EdmModel(&csdl);
    let admin = RequestCtx::new(R, Principal::realm_admin("root"));
    for (set, id) in [
        ("WebAuthnCredentials", "pk-odata"),
        ("IdentityProviderLinks", "link-odata"),
        ("EmailOutbox", "ob-odata"),
    ] {
        let uri = format!("{set}?$count=true");
        let parsed = odata_url::parse(&uri, &edm, &odata_url::Limits::strict()).expect(&uri);
        let odata_url::ODataUri::Resource { options, .. } = parsed else {
            panic!("{uri} is no resource");
        };
        let ans = store
            .query_entity_set(&admin, &LocalAuthorizer, set, &options, 100)
            .await
            .expect(&uri);
        assert!(
            ans.rows.iter().any(|r| r["id"] == id),
            "{set}: {:?}",
            ans.rows
        );
        let raw = serde_json::to_string(&ans.rows).unwrap();
        assert!(!raw.contains("\"kty\""), "no key material in {set}: {raw}");
    }
    // A numeric comparison binds the number.
    let uri = "People?$filter=createdTimestamp%20gt%201&$count=true";
    let parsed = odata_url::parse(uri, &edm, &odata_url::Limits::strict()).unwrap();
    let odata_url::ODataUri::Resource { options, .. } = parsed else {
        panic!("no resource");
    };
    store
        .query_entity_set(&admin, &LocalAuthorizer, "People", &options, 100)
        .await
        .expect(uri);
    // An unknown set is a 404.
    let parsed = odata_url::parse("People", &edm, &odata_url::Limits::strict()).unwrap();
    let odata_url::ODataUri::Resource { options, .. } = parsed else {
        panic!("no resource");
    };
    let unknown = store
        .query_entity_set(&admin, &LocalAuthorizer, "NoSuchSet", &options, 100)
        .await;
    assert!(
        matches!(unknown, Err(KaisatsuError::NotFound(_))),
        "{unknown:?}"
    );
}

async fn contract(store: Arc<dyn Store>) {
    seed(&store).await;
    passkeys_and_links(&store).await;
    outbox_backoff(&store).await;
    k_entity_sets(&store).await;
    sessions_go_with_their_person(&store).await;
}

#[tokio::test]
async fn memory_store_error_paths() {
    contract(Arc::new(MemoryStore::default())).await;
}

#[tokio::test]
async fn pg_store_error_paths() {
    let Some(db) = common::test_db("pg_store_error_paths").await else {
        return;
    };
    let store = db.store(Kek::generate_for_tests()).await;
    // Signing keys written directly are listed back (rotation path).
    let kek = Kek::generate_for_tests();
    store
        .upsert_realm(RealmConfig {
            id: "keys".into(),
            realm: "keys".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let key = kaisatsu_crypto::KeyManager::generate_es256("keys", &kek, 100, "ACTIVE").unwrap();
    store.upsert_signing_key(key.clone()).await.unwrap();
    assert!(
        store
            .list_signing_keys("keys")
            .await
            .unwrap()
            .iter()
            .any(|k| k.kid == key.kid)
    );
    contract(Arc::new(store)).await;
    db.drop_db().await;
}

#[tokio::test]
async fn pg_boot_check_and_migrations_refuse_a_dead_or_unprivileged_pool() {
    let Some(db) = common::test_db("pg_boot_check_dead_pool").await else {
        return;
    };
    // A closed pool fails the first boot query with a storage error, not a panic.
    let app = db.app_pool().await;
    app.close().await;
    let err = verify_boot_rls(&app)
        .await
        .expect_err("closed pool refused");
    assert!(matches!(err, KaisatsuError::Storage(_)), "{err}");
    // The request role may not migrate (no CREATE on the database).
    let app = db.app_pool().await;
    let err = PgStore::run_migrations(&app)
        .await
        .expect_err("kaisatsu_app cannot migrate");
    assert!(err.to_string().contains("migration failed"), "{err}");
    app.close().await;
    db.drop_db().await;
}
