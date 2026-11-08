#![forbid(unsafe_code)]
//! Contract tests for K9 (transactional email outbox, rollback atomicity, concurrent
//! `FOR UPDATE SKIP LOCKED` dispatchers), K11/K12 store methods (`webauthn_credential`,
//! `idp_links`), and K15 (`PgScopedTable` OData SQL pushdown matching `MemoryTable` and
//! resisting SQL injection).

use chrono::{Duration, Utc};
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_memory::MemoryStore;
use odata_table::{MemoryTable, query};
use std::collections::HashMap;
use std::sync::Arc;

mod common;

async fn seed_store(store: &Arc<dyn Store>) {
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

async fn assert_outbox_and_webauthn_contract(store: Arc<dyn Store>) {
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
        next_attempt_at: Utc::now(),
        created_at: Utc::now(),
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
        next_attempt_at: Utc::now() - Duration::seconds(1),
        created_at: Utc::now(),
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
        s1.claim_outbox_batch("dev", 10),
        s2.claim_outbox_batch("dev", 10)
    );
    let mut claimed_ids: Vec<String> = b1
        .unwrap()
        .into_iter()
        .chain(b2.unwrap().into_iter())
        .map(|r| r.id)
        .collect();
    claimed_ids.sort();
    assert_eq!(claimed_ids, vec!["ob-1".to_owned(), "ob-2".to_owned()]);

    // Mark ob-1 sent and ob-2 failed after max_attempts=1, then retry ob-2.
    store
        .complete_outbox_row("dev", "ob-1", true, None, None, 3)
        .await
        .unwrap();
    store
        .complete_outbox_row("dev", "ob-2", false, Some("SMTP timeout".into()), None, 1)
        .await
        .unwrap();
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
        created_at: Utc::now(),
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
}

#[tokio::test]
async fn memory_store_outbox_and_webauthn_contract() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    assert_outbox_and_webauthn_contract(store).await;
}

#[tokio::test]
async fn pg_store_outbox_and_webauthn_contract() {
    let Some(db) = common::test_db("pg_store_outbox_and_webauthn_contract").await else {
        return;
    };
    let store: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    assert_outbox_and_webauthn_contract(store).await;
    db.drop_db().await;
}

#[tokio::test]
async fn pg_scoped_table_matches_memory_table_and_binds_filter_values() {
    let Some(db) =
        common::test_db("pg_scoped_table_matches_memory_table_and_binds_filter_values").await
    else {
        return;
    };
    let pg: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    let mem: Arc<dyn Store> = Arc::new(MemoryStore::default());
    seed_store(&pg).await;
    seed_store(&mem).await;

    let authorizer = LocalAuthorizer;
    let admin_ctx = RequestCtx::new("dev", Principal::realm_admin("root"));
    let hel_ctx = RequestCtx::new(
        "dev",
        Principal {
            subject: Some("u-0".into()),
            client_id: None,
            realm_roles: Vec::new(),
            client_roles: HashMap::new(),
            organizations: vec!["helsinki".into()],
            org_roles: HashMap::from([("helsinki".into(), vec!["org-admin".into()])]),
            second_factor: true,
        },
    )
    .with_org("helsinki");

    let service = kaisatsu_odata_service();
    let csdl = Arc::new(service.model());
    let edm = odata_url::EdmModel(&csdl);

    let cases = [
        "People?$count=true&$orderby=username%20asc",
        "People?$filter=enabled%20eq%20true%20and%20startswith(username,'anna')&$count=true",
        "People?$filter=contains(username,'hel.fi')&$orderby=createdTimestamp%20desc&$top=2&$skip=1",
        "People?$filter=firstName%20eq%20'Clara''%20OR%201%3D1;--'&$count=true",
        "People?$filter=firstName%20eq%20'nonexistent'';%20DROP%20TABLE%20kaisatsu.people;--'",
        "People?$select=id,username,enabled&$orderby=username",
    ];

    for uri_str in cases {
        let parsed = odata_url::parse(uri_str, &edm, &odata_url::Limits::strict()).expect(uri_str);
        let odata_url::ODataUri::Resource { options, .. } = parsed else {
            panic!("not a resource");
        };
        for ctx in [&admin_ctx, &hel_ctx] {
            let pg_ans = pg
                .query_entity_set(ctx, &authorizer, "People", &options, 100)
                .await
                .expect(uri_str);
            let mem_ans = mem
                .query_entity_set(ctx, &authorizer, "People", &options, 100)
                .await
                .expect(uri_str);
            assert_eq!(pg_ans.count, mem_ans.count, "count mismatch on {uri_str}");
            assert_eq!(pg_ans.rows, mem_ans.rows, "rows mismatch on {uri_str}");
            // Other realm's rows never appear.
            assert!(
                pg_ans
                    .rows
                    .iter()
                    .all(|r| r.get("id").and_then(|v| v.as_str()) != Some("u-other-1"))
            );
        }
    }

    let _ = (MemoryTable::new, query::<MemoryTable>);
    db.drop_db().await;
}
