#![forbid(unsafe_code)]
//! Contract tests for K9 (transactional email outbox, rollback atomicity, concurrent
//! `FOR UPDATE SKIP LOCKED` dispatchers), K11/K12 store methods (`webauthn_credential`,
//! `idp_links`), and K15 (`PgScopedTable` OData SQL pushdown matching `MemoryTable` and
//! resisting SQL injection).

use jc_time::TimestampTz;
use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_memory::MemoryStore;
use kaisatsu_store_pg::PgScopedTable;
use odata_table::{MemoryTable, query};
use std::collections::HashMap;
use std::sync::Arc;

mod common;
#[path = "../../store-memory/tests/outbox_cases/mod.rs"]
mod outbox_cases;
use outbox_cases::*;

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

    // a person without any role reads no entity set: both stores refuse with the same answer
    let nobody_ctx = RequestCtx::new(
        "dev",
        Principal {
            subject: Some("u-1".into()),
            client_id: None,
            realm_roles: Vec::new(),
            client_roles: HashMap::new(),
            organizations: Vec::new(),
            org_roles: HashMap::new(),
            second_factor: false,
        },
    );

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
        "People?$filter=lastName%20eq%20null&$count=true",
        "People?$filter=lastName%20ne%20null&$count=true",
        "People?$filter=createdTimestamp%20gt%201700000000000%20and%20createdTimestamp%20le%201700000002000&$count=true",
        "People?$filter=createdTimestamp%20ge%201700000001000%20and%20createdTimestamp%20lt%201700000003000&$count=true",
        "People?$filter=username%20ne%20'anna@hel.fi'%20or%20enabled%20eq%20false&$count=true",
        "People?$filter=lastName%20gt%20null&$count=true",
        "People?$filter=contains(firstName,'%5C')&$count=true",
        "People?$filter=false&$count=true",
        "ProtocolMappers?$count=true&$orderby=id",
        "ProtocolMappers?$filter=name%20ne%20'locale'&$orderby=id%20desc",
        "ProtocolMappers?$orderby=id&$top=1",
    ];

    for uri_str in cases {
        let parsed = odata_url::parse(uri_str, &edm, &odata_url::Limits::strict()).expect(uri_str);
        let odata_url::ODataUri::Resource { options, .. } = parsed else {
            panic!("not a resource");
        };
        for ctx in [&admin_ctx, &hel_ctx, &nobody_ctx] {
            let set = uri_str.split('?').next().unwrap_or_default();
            let pg_ans = pg
                .query_entity_set(ctx, &authorizer, set, &options, 100)
                .await;
            let mem_ans = mem
                .query_entity_set(ctx, &authorizer, set, &options, 100)
                .await;
            let (pg_ans, mem_ans) = match (pg_ans, mem_ans) {
                (Ok(p), Ok(m)) => (p, m),
                (Err(p), Err(m)) => {
                    assert_eq!(p.to_string(), m.to_string(), "{uri_str}");
                    continue;
                }
                (p, m) => panic!("{uri_str}: pg {p:?} memory {m:?}"),
            };
            if set == "ProtocolMappers" && std::ptr::eq(ctx, &admin_ctx) {
                assert!(!pg_ans.rows.is_empty(), "{uri_str} found no mapper");
            }
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

/// The keyset fallback of the expanded sets skips a stored document it cannot read (an
/// Organization's domains, a client's or a client scope's mappers that are no list) and still
/// answers every row it can.
#[tokio::test]
async fn the_fallback_scan_skips_a_document_it_cannot_read() {
    let Some(db) = common::test_db("the_fallback_scan_skips_a_document_it_cannot_read").await
    else {
        return;
    };
    let pg: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    seed_store(&pg).await;
    pg.upsert_organization(Organization {
        id: "helsinki".into(),
        realm_id: "dev".into(),
        name: "helsinki".into(),
        alias: "helsinki".into(),
        enabled: true,
        description: String::new(),
        domains: Vec::new(),
        attributes: HashMap::new(),
        managed_by: None,
    })
    .await
    .expect("org");
    let admin = db.superuser_pool().await;
    for table in ["organizations", "applications", "client_scopes"] {
        let field = if table == "organizations" {
            "domains"
        } else {
            "protocolMappers"
        };
        let changed = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE kaisatsu.{table} SET doc = doc || '{{\"{field}\": 5}}'::jsonb \
             WHERE realm_id = 'dev'"
        )))
        .execute(&admin)
        .await
        .expect(table);
        assert!(changed.rows_affected() > 0, "{table} holds a row to break");
    }
    let service = kaisatsu_odata_service();
    let csdl = Arc::new(service.model());
    let edm = odata_url::EdmModel(&csdl);
    let ctx = RequestCtx::new("dev", Principal::realm_admin("root"));
    for set in ["OrganizationDomains", "ProtocolMappers"] {
        let uri = format!("{set}?$count=true");
        let odata_url::ODataUri::Resource { options, .. } =
            odata_url::parse(&uri, &edm, &odata_url::Limits::strict()).expect(set)
        else {
            panic!("not a resource");
        };
        let page = pg
            .query_entity_set(&ctx, &LocalAuthorizer, set, &options, 100)
            .await
            .expect(set);
        assert_eq!(page.rows.len(), 0, "{set}: {:?}", page.rows);
    }
    db.drop_db().await;
}

/// K15: the pushdown runs inside the RLS transaction with the caller's organizations, so even a
/// scope without row restrictions shows an org-scoped caller only its organizations' people.
#[tokio::test]
async fn pg_scoped_table_reads_under_the_callers_organization_rls() {
    let Some(db) =
        common::test_db("pg_scoped_table_reads_under_the_callers_organization_rls").await
    else {
        return;
    };
    let pg = db.store(Kek::generate_for_tests()).await;
    let as_store: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    seed_store(&as_store).await;
    let info = pg.table_info("People").unwrap();
    let options = odata_table::QueryOptions::default();
    let names = |ans: odata_table::Answer| -> Vec<String> {
        ans.rows
            .iter()
            .filter_map(|r| {
                r.get("username")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .collect()
    };

    let hel = PgScopedTable::new(
        &pg,
        info.clone(),
        "dev".into(),
        Some(vec!["helsinki".into()]),
    );
    let seen = names(
        query(&hel, &options, &odata_table::Scope::all(), 100)
            .await
            .unwrap(),
    );
    assert!(seen.contains(&"anna@hel.fi".to_owned()), "{seen:?}");
    assert!(
        !seen.contains(&"david@esp.fi".to_owned()),
        "RLS hides espoo: {seen:?}"
    );

    let realm_wide = PgScopedTable::new(&pg, info, "dev".into(), None);
    let seen = names(
        query(&realm_wide, &options, &odata_table::Scope::all(), 100)
            .await
            .unwrap(),
    );
    assert!(seen.contains(&"david@esp.fi".to_owned()), "{seen:?}");
    db.drop_db().await;
}

/// K9: a dispatcher skips outbox rows another transaction holds (`FOR UPDATE SKIP LOCKED`)
/// instead of waiting for it.
#[tokio::test]
async fn pg_outbox_claim_skips_rows_another_dispatcher_holds() {
    let Some(db) = common::test_db("pg_outbox_claim_skips_rows_another_dispatcher_holds").await
    else {
        return;
    };
    let store: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    seed_store(&store).await;
    for id in ["ob-held", "ob-free"] {
        store
            .enqueue_outbox(EmailOutboxRecord {
                id: id.into(),
                realm_id: "dev".into(),
                action_jti: format!("jti-{id}"),
                ..outbox_record()
            })
            .await
            .unwrap();
    }
    let su = db.superuser_pool().await;
    let mut holder = su.begin().await.unwrap();
    sqlx::query("SELECT id FROM kaisatsu.email_outbox WHERE id = 'ob-held' FOR UPDATE")
        .execute(&mut *holder)
        .await
        .unwrap();
    let claimed = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        store.claim_outbox_batch("dev", 10, LEASE, 5),
    )
    .await
    .expect("the claim does not wait for the held row")
    .unwrap();
    let ids: Vec<_> = claimed.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["ob-free"]);
    holder.rollback().await.unwrap();
    db.drop_db().await;
}

/// A record written without an id gets one; a replay guard for no realm or every realm is
/// refused; a realm whose ES256 key went passive gets a new active one on the next listing.
#[tokio::test]
async fn pg_store_mints_missing_ids_refuses_a_wildcard_realm_and_renews_keys() {
    let Some(db) = common::test_db("pg_store_mints_missing_ids").await else {
        return;
    };
    let store: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    seed_store(&store).await;
    let mut queued = outbox_record();
    queued.id = String::new();
    store.enqueue_outbox(queued).await.expect("enqueue");
    let mut with_user = outbox_record();
    with_user.id = String::new();
    with_user.action_jti = "jti-with-user".into();
    let user = store
        .get_user_by_id("dev", None, "u-0")
        .await
        .expect("get")
        .expect("u-0");
    store
        .update_user_with_outbox(user, with_user)
        .await
        .expect("update with outbox");
    let rows = store.list_outbox_rows("dev", None).await.expect("rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| !r.id.is_empty()), "{rows:?}");
    assert_ne!(rows[0].id, rows[1].id);

    // PostgreSQL text holds no NUL: the write is refused as a storage error, nothing stored
    let mut nul = store
        .get_user_by_id("dev", None, "u-1")
        .await
        .expect("get")
        .expect("u-1");
    nul.first_name = Some("Bo\0ris".into());
    let refused = store.update_user(nul).await;
    assert!(
        matches!(&refused, Err(KaisatsuError::Storage(m)) if m.starts_with("postgres: ")),
        "{refused:?}"
    );
    for realm in ["", "*"] {
        let refused = store
            .mark_token_revoked(RevokedTokenRecord {
                jti_hash: "h".into(),
                realm_id: realm.into(),
                expires_at: TimestampTz::now(),
            })
            .await;
        assert!(
            matches!(refused, Err(KaisatsuError::NotFound(_))),
            "{realm:?}: {refused:?}"
        );
    }

    let keys = store.list_signing_keys("dev").await.expect("keys");
    let mut es = keys
        .iter()
        .find(|k| k.algorithm == "ES256" && k.status == "ACTIVE")
        .expect("es256")
        .clone();
    let old_kid = es.kid.clone();
    es.status = "PASSIVE".into();
    store.upsert_signing_key(es).await.expect("passive");
    let renewed = store.list_signing_keys("dev").await.expect("keys");
    let active = renewed
        .iter()
        .find(|k| k.algorithm == "ES256" && k.status == "ACTIVE")
        .expect("a new active ES256 key");
    assert_ne!(active.kid, old_kid);
    db.drop_db().await;
}

fn outbox_record() -> EmailOutboxRecord {
    EmailOutboxRecord {
        id: "ob".into(),
        realm_id: "dev".into(),
        org_id: Some("helsinki".into()),
        user_id: "u-0".into(),
        to_address: "anna@hel.fi".into(),
        locale: "en".into(),
        template_kind: "EXECUTE_ACTIONS".into(),
        action_jti: "jti".into(),
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
    }
}
