//! Shared by kaisatsu-store-memory's tests/outbox_contract.rs and kaisatsu-store-pg's
//! odata_pushdown_and_outbox.rs.

use jc_time::TimestampTz;
use jiff::SignedDuration;
use kaisatsu_core::*;
use std::sync::Arc;

pub const LEASE: std::time::Duration = std::time::Duration::from_secs(90);

pub async fn seed_store(store: &Arc<dyn Store>) {
    store
        .upsert_realm(RealmConfig {
            id: "dev".into(),
            realm: "dev".into(),
            display_name: "Dev Realm".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_realm(RealmConfig {
            id: "other".into(),
            realm: "other".into(),
            display_name: "Other Realm".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    for (i, (uname, first, enabled, org)) in [
        ("anna@hel.fi", "Anna", true, Some("helsinki")),
        ("boris@hel.fi", "Boris", false, Some("helsinki")),
        ("clara@hel.fi", "Clara' OR 1=1;--", true, Some("helsinki")),
        ("david@esp.fi", "David", true, Some("espoo")),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .create_user(UserRepresentation {
                id: format!("u-{i}"),
                realm_id: "dev".into(),
                org_id: org.map(str::to_owned),
                username: uname.into(),
                email: Some(uname.into()),
                first_name: Some(first.into()),
                last_name: Some("Tester".into()),
                enabled,
                email_verified: i % 2 == 0,
                created_timestamp: Some(1_700_000_000_000 + (i as i64) * 1000),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    // protocol mappers live inside their application or client scope: the keyset fallback
    // reads them out of both
    let mapper = |id: &str, name: &str| ProtocolMapperRepresentation {
        id: id.into(),
        realm_id: "dev".into(),
        org_id: None,
        name: name.into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-usermodel-attribute-mapper".into(),
        consent_required: false,
        config: Default::default(),
    };
    store
        .upsert_client(ClientRepresentation {
            id: "c-portal".into(),
            client_id: "portal".into(),
            realm_id: "dev".into(),
            protocol_mappers: vec![mapper("pm-b", "department"), mapper("pm-a", "locale")],
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_client_scope(ClientScopeRepresentation {
            id: "cs-profile".into(),
            name: "profile".into(),
            realm_id: "dev".into(),
            protocol_mappers: vec![mapper("pm-c", "nickname")],
            ..Default::default()
        })
        .await
        .unwrap();

    store
        .create_user(UserRepresentation {
            id: "u-other-1".into(),
            realm_id: "other".into(),
            org_id: Some("helsinki".into()),
            username: "foreign@hel.fi".into(),
            email: Some("foreign@hel.fi".into()),
            first_name: Some("Anna".into()),
            enabled: true,
            ..Default::default()
        })
        .await
        .unwrap();
}

pub async fn assert_outbox_and_webauthn_contract(store: Arc<dyn Store>) {
    seed_store(&store).await;

    // 1. Rollback atomicity of update_user_with_outbox:
    // Attempting to update u-0's username to u-1's existing username must fail with Conflict
    // and leave ZERO outbox rows in the store.
    let mut conflict_user = store
        .get_user_by_id("dev", None, "u-0")
        .await
        .unwrap()
        .unwrap();
    conflict_user.username = "boris@hel.fi".into();
    let rolled_back_outbox = EmailOutboxRecord {
        id: "ob-rollback".into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        user_id: "u-0".into(),
        to_address: "anna@hel.fi".into(),
        locale: "en".into(),
        template_kind: "EXECUTE_ACTIONS".into(),
        action_jti: "jti-rollback".into(),
        actions: vec!["UPDATE_PASSWORD".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: "https://idm.localhost/realms/dev".into(),
        status: "pending".into(),
        attempts: 0,
        last_error: None,
        next_attempt_at: TimestampTz::now(),
        created_at: TimestampTz::now(),
        sent_at: None,
    };
    let res = store
        .update_user_with_outbox(conflict_user, rolled_back_outbox)
        .await;
    assert!(matches!(res, Err(KaisatsuError::Conflict(_))));
    assert!(
        store
            .list_outbox_rows("dev", None)
            .await
            .unwrap()
            .is_empty(),
        "failed user update must not leave an outbox row"
    );

    // 2. Valid update_user_with_outbox writes both atomically.
    let mut valid_user = store
        .get_user_by_id("dev", None, "u-0")
        .await
        .unwrap()
        .unwrap();
    valid_user.required_actions = vec!["VERIFY_EMAIL".into()];
    let ob1 = EmailOutboxRecord {
        id: "ob-1".into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        user_id: "u-0".into(),
        to_address: "anna@hel.fi".into(),
        locale: "en".into(),
        template_kind: "VERIFY_EMAIL".into(),
        action_jti: "jti-1".into(),
        actions: vec!["VERIFY_EMAIL".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: "https://idm.localhost/realms/dev".into(),
        status: "pending".into(),
        attempts: 0,
        last_error: None,
        next_attempt_at: TimestampTz::now() - SignedDuration::from_secs(1),
        created_at: TimestampTz::now(),
        sent_at: None,
    };
    store
        .update_user_with_outbox(valid_user, ob1.clone())
        .await
        .unwrap();
    let ob2 = EmailOutboxRecord {
        id: "ob-2".into(),
        action_jti: "jti-2".into(),
        ..ob1
    };
    store.enqueue_outbox(ob2).await.unwrap();

    // 3. Two concurrent dispatchers claiming batches never claim the same row twice.
    let s1 = store.clone();
    let s2 = store.clone();
    let (b1, b2) = tokio::join!(
        s1.claim_outbox_batch("dev", 10, LEASE, 5),
        s2.claim_outbox_batch("dev", 10, LEASE, 5)
    );
    let mut claimed: Vec<EmailOutboxRecord> = b1.unwrap().into_iter().chain(b2.unwrap()).collect();
    claimed.sort_by(|a, b| a.id.cmp(&b.id));
    let claimed_ids: Vec<&str> = claimed.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(claimed_ids, ["ob-1", "ob-2"]);

    // Mark ob-1 sent and ob-2 failed after max_attempts=1, then retry ob-2.
    assert!(
        store
            .complete_outbox_row(&claimed[0], true, None, None, 3)
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox_row(&claimed[1], false, Some("SMTP timeout".into()), None, 1)
            .await
            .unwrap()
    );
    let rows = store.list_outbox_rows("dev", None).await.unwrap();
    let r2 = rows.iter().find(|r| r.id == "ob-2").unwrap();
    assert_eq!(r2.status, "failed");
    // Cross-org retry returns None (404)
    assert!(
        store
            .retry_outbox_row("dev", Some(&["espoo".into()]), "ob-2")
            .await
            .unwrap()
            .is_none()
    );
    let retried = store
        .retry_outbox_row("dev", Some(&["helsinki".into()]), "ob-2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.status, "pending");
    assert_eq!(retried.attempts, 0);

    // 4. WebAuthn credential uniqueness per realm and counter updates (K11).
    let cred = WebAuthnCredentialRecord {
        id: "wa-1".into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        user_id: "u-0".into(),
        user_handle: "uh-0".into(),
        credential_id: "cred-id-abc".into(),
        public_key_cose_json: r#"{"kty":2,"alg":-7,"crv":1,"x":"AA","y":"BB"}"#.into(),
        cose_alg: -7,
        sign_count: 1,
        uv_initialized: true,
        backup_eligible: true,
        backup_state: false,
        transports: vec!["internal".into()],
        attestation_type: "None".into(),
        rp_id: "idm.localhost".into(),
        user_label: Some("Laptop".into()),
        created_at: TimestampTz::now(),
        last_used_at: None,
    };
    store
        .insert_webauthn_credential(cred.clone())
        .await
        .unwrap();
    let dup = store.insert_webauthn_credential(cred.clone()).await;
    assert!(matches!(dup, Err(KaisatsuError::Conflict(_))));
    let fetched = store
        .get_webauthn_by_credential_id("dev", "cred-id-abc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.public_key_cose_json, cred.public_key_cose_json);
    // a credential written without an id gets one
    let mut unnamed = cred.clone();
    unnamed.id = String::new();
    unnamed.credential_id = "cred-id-def".into();
    let stored = store
        .insert_webauthn_credential(unnamed)
        .await
        .expect("insert");
    assert!(!stored.id.is_empty() && stored.id != cred.id);
}
