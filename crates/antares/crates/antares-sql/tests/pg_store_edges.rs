// SPDX-License-Identifier: EUPL-1.2
//! The edges of the Postgres store no operation test reaches (T-7830): a
//! declined confirmation, a stored subject this release cannot read, a
//! refused change of a document or a hosted @context, a Tenant name the
//! seal walk refuses, a migration history it cannot read, a server without
//! PostGIS, a pool that is closed, a lease whose session ended, and the
//! store calls that find nothing to do. Needs ANTARES_TEST_DATABASE_URL (see
//! tests/pg.rs recipe); the tests that change what every test reads run in a
//! database of their own.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_sql::store::pg;
use antares_sql::store::pg::doc::{DocKind, PgDocStore};
use antares_sql::store::pg::maintenance;
use antares_store::confirmations::{Caps, Confirmation, Taken};
use antares_store::filter::EntityFilter;
use antares_store::{CurrentStateDriver as Driver, Kind};
use serde_json::json;
use sqlx::{Executor, PgPool};
use std::time::Duration;

macro_rules! require_db {
    () => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

fn run() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

/// A fresh database on the test server and its URL, nothing migrated.
async fn own_database(url: &str, tag: &str) -> String {
    let admin = PgPool::connect(url).await.expect("connect");
    let name = format!("antares_edges_{tag}_{}", run());
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .expect("own database");
    admin.close().await;
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    format!("{base}/{name}")
}

const CAPS: Caps = Caps {
    tenant: 5,
    subject: 5,
};

fn asked(name: &str) -> Confirmation {
    Confirmation {
        view: "v".into(),
        tool: "delete_entity".into(),
        message: "Allow?".into(),
        subject: vec![("x-jc-subject".to_owned(), name.as_bytes().to_vec())],
        left: Duration::from_secs(600),
        decision: None,
    }
}

/// A declined question is taken once as declined; a row whose stored subject
/// is no subject this release can read answers an error, never a question
/// with an empty subject.
#[tokio::test(flavor = "multi_thread")]
async fn a_decline_is_taken_once_and_an_unreadable_subject_is_an_error() {
    let url = require_db!();
    let pool = pg::connect(&url, 4).await.expect("connect+migrate");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let t = TenantId::new(&format!("edge_confirm_{}", run())).expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let alice = asked("alice");
    assert!(Driver::confirmation_ask(&store, &t, "q", &alice, CAPS)
        .await
        .expect("ask"));
    let decided = Driver::confirmation_decide(&store, &t, "q", false)
        .await
        .expect("decide")
        .expect("the open question");
    assert_eq!(decided.decision, Some(false));
    let take = || Driver::confirmation_take(&store, &t, "q", &alice.subject);
    assert_eq!(take().await.expect("take"), Taken::Declined);
    assert_eq!(take().await.expect("take again"), Taken::Unknown);

    assert!(Driver::confirmation_ask(&store, &t, "bad", &alice, CAPS)
        .await
        .expect("ask"));
    sqlx::query(
        "UPDATE mcp_confirmations SET subject = 'not json' WHERE tenant_id = $1 AND id = 'bad'",
    )
    .bind(t.as_str())
    .execute(&pool)
    .await
    .expect("damage the row");
    assert!(Driver::confirmation_find(&store, &t, "bad").await.is_err());
    assert!(Driver::confirmation_decide(&store, &t, "bad", true)
        .await
        .is_err());
    pool.close().await;
}

/// A refused change of a document writes nothing; an empty delivery batch
/// asks nothing; a hosted @context is changed, refused or kept by its own
/// Tenant without the usage counters a shared Cached copy keeps.
#[tokio::test(flavor = "multi_thread")]
async fn refused_changes_write_nothing_and_a_hosted_context_keeps_no_counters() {
    let url = require_db!();
    let pool = pg::connect(&url, 4).await.expect("connect+migrate");
    let s = PgDocStore::new(pool.clone());
    let t = TenantId::new(&format!("edge_doc_{}", run())).expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Subscription:edge";
    s.upsert(
        &t,
        DocKind::Subscription,
        id,
        &json!({"id": id, "type": "Subscription", "n": 1}),
    )
    .await
    .expect("insert");
    let refused = s
        .mutate(&t, DocKind::Subscription, id, |d| {
            d["n"] = json!(2);
            Err::<(), &str>("no")
        })
        .await
        .expect("mutate");
    assert_eq!(refused, Some(Err("no")));
    assert_eq!(
        s.get(&t, DocKind::Subscription, id)
            .await
            .expect("get")
            .expect("kept")["n"],
        1
    );
    assert!(s
        .record_deliveries(&t, DocKind::Subscription, &[])
        .await
        .expect("nothing to book")
        .is_empty());

    let ctx = format!("urn:edge:ctx:{}", run());
    let hosted = json!({"localId": ctx, "kind": "Hosted", "owner": t.as_str(),
                        "body": {"@context": {"a": "https://a"}}});
    s.context_put(Some(&t), &ctx, &hosted, "Hosted")
        .await
        .expect("put");
    let changed = s
        .context_mutate(Some(&t), &ctx, |d| {
            d["body"] = json!({"@context": {"b": "https://b"}});
            Ok(())
        })
        .await
        .expect("mutate");
    assert_eq!(changed, Some(Ok(())));
    let kept = s
        .context_mutate(Some(&t), &ctx, |d| {
            d["body"] = json!({"@context": {}});
            Err(())
        })
        .await
        .expect("mutate");
    assert_eq!(kept, Some(Err(())));
    let unchanged = s
        .context_mutate(Some(&t), &ctx, |_| Ok(()))
        .await
        .expect("mutate");
    assert_eq!(unchanged, Some(Ok(())));
    let row = s
        .context_get(Some(&t), &ctx)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(row["body"], json!({"@context": {"b": "https://b"}}));
    assert!(row.get("numberOfHits").is_none(), "{row}");
    let other = TenantId::new(&format!("edge_doc_other_{}", run())).expect("tenant");
    assert_eq!(
        s.context_mutate(Some(&other), &ctx, |_| Ok(()))
            .await
            .expect("mutate"),
        None,
        "another Tenant's hosted row is absent"
    );
    s.context_delete(Some(&t), &ctx).await.expect("clean");
    pool.close().await;
}

/// The seal walk visits the default Tenant, which exists without a row, and
/// refuses a Tenant row whose name the broker would never write.
#[tokio::test(flavor = "multi_thread")]
async fn the_seal_walk_adds_the_default_tenant_and_refuses_a_bad_name() {
    let url = require_db!();
    let own = own_database(&url, "seal").await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    let key = || {
        pg::seal::SecretKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .expect("key")
    };
    sqlx::query("DELETE FROM tenants WHERE tenant_id = 'default'")
        .execute(&pool)
        .await
        .expect("no default row");
    let s = PgDocStore::new(pool.clone()).with_secret_key(key());
    assert_eq!(s.seal_plaintext().await.expect("walk"), 0);

    sqlx::query("DELETE FROM maintenance_jobs")
        .execute(&pool)
        .await
        .expect("walk again");
    sqlx::query("INSERT INTO tenants (tenant_id) VALUES ('bad tenant')")
        .execute(&pool)
        .await
        .expect("a name TenantId refuses");
    let err = PgDocStore::new(pool.clone())
        .with_secret_key(key())
        .seal_plaintext()
        .await
        .expect_err("refused");
    assert!(matches!(err, sqlx::Error::Configuration(_)), "{err}");
    pool.close().await;
}

/// A migration history the boot cannot read stops the boot with that error;
/// the server probe of a database without PostGIS names the server only; a
/// closed pool reports no server version and fails the RLS probe closed.
#[tokio::test(flavor = "multi_thread")]
async fn the_boot_probes_answer_a_database_they_cannot_read() {
    let url = require_db!();
    let own = own_database(&url, "probe").await;
    let raw = PgPool::connect(&own).await.expect("connect");
    // the timescale CI image puts its extensions in template1 (T-7594)
    let info = pg::version_info(&raw).await;
    assert!(info.get("server").is_some(), "{info}");
    for ext in ["postgis", "timescaledb"] {
        let held: Option<String> =
            sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = $1")
                .bind(ext)
                .fetch_optional(&raw)
                .await
                .expect("extensions");
        assert_eq!(
            info.get(ext).and_then(|v| v.as_str()),
            held.as_deref(),
            "{info}"
        );
    }
    raw.execute("CREATE TABLE _sqlx_migrations (version bigint)")
        .await
        .expect("a history without its columns");
    let err = pg::connect(&own, 2).await.expect_err("unreadable history");
    assert!(err.to_string().contains("description"), "{err}");
    raw.close().await;

    let info = pg::version_info(&raw).await;
    assert_eq!(info["engine"], "postgres");
    assert!(info.get("server").is_none(), "{info}");
    assert!(
        pg::role_bypasses_rls(&raw).await,
        "a failed probe reads as bypassing"
    );
}

/// A pass with nothing old to drop reports no deletion; a lease whose session was ended is dead
/// and gives back nothing it could not unlock.
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_pass_deletes_nothing_and_a_dead_lease_is_closed() {
    let url = require_db!();
    let own = own_database(&url, "pass").await;
    let pool = pg::connect(&own, 6).await.expect("connect+migrate");
    // a hypertable database where the CI image's template1 holds timescaledb
    let backend = maintenance::detect_temporal_backend(&pool)
        .await
        .expect("backend");
    let first = maintenance::temporal_maintenance(&pool, backend, Some(30), Duration::ZERO)
        .await
        .expect("first pass");
    assert!(!first.contains("superseded versions"), "{first}");

    let mut lease = maintenance::claim_period(&pool, "edge_job", Duration::ZERO)
        .await
        .expect("claim")
        .expect("the lease");
    assert!(lease.alive().await);
    let killer = PgPool::connect(&own).await.expect("connect");
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
          WHERE datname = current_database() AND pid <> pg_backend_pid()",
    )
    .execute(&killer)
    .await
    .expect("end the lease's session");
    assert!(!lease.alive().await, "the ended session holds no lock");
    lease.release().await;
    let again = maintenance::claim_period(&killer, "edge_job", Duration::ZERO)
        .await
        .expect("claim again");
    assert!(again.is_some(), "the lock went with the session");
    drop(again);
    killer.close().await;
    pool.close().await;
}

/// An id prefix holding a NUL matches no stored id, and an empty id list
/// reads nothing; neither asks the database.
#[tokio::test(flavor = "multi_thread")]
async fn id_reads_that_cannot_match_answer_empty() {
    let url = require_db!();
    let pool = pg::connect(&url, 2).await.expect("connect+migrate");
    let p = PgBackend::new(pool.clone());
    let t = TenantId::new("edge_ids").expect("tenant");
    assert_eq!(
        p.entities
            .id_segments(&t, "urn:\0", ':', 10)
            .await
            .expect("segments"),
        None
    );
    assert!(p.entities.get_many(&t, &[]).await.expect("none").is_empty());
    p.temporal.append_many(&[]).await.expect("an empty batch");
    pool.close().await;
}

/// A batch delete names the id it found nothing for; an expired Entity is
/// absent from a text query; the Entity digests are the listing's; a
/// change that turns a hosted @context into a shared copy is refused; a
/// purge of a Tenant that never existed purges nothing.
#[tokio::test(flavor = "multi_thread")]
async fn store_calls_that_find_nothing_say_so() {
    let url = require_db!();
    let pool = pg::connect(&url, 4).await.expect("connect+migrate");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let t = TenantId::new(&format!("edge_any_{}", run())).expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    for (id, expires) in [
        ("urn:e:old", "2000-01-01T00:00:00Z"),
        ("urn:e:live", "2999-01-01T00:00:00Z"),
    ] {
        store
            .upsert(
                &t,
                Kind::Entity,
                id,
                json!({"id": id, "type": ["https://T"], "expiresAt": expires}),
            )
            .await
            .expect("upsert");
    }
    let ids = ["urn:e:old", "urn:e:live"];
    let f = EntityFilter {
        ids: Some(&ids),
        ..Default::default()
    };
    let rows = store.query_entities_text(&t, &f).await.expect("query").rows;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].get().contains("urn:e:live"));
    let digests = store.list_digests(&t, Kind::Entity).await.expect("digests");
    assert!(
        digests.iter().any(|(id, _)| id == "urn:e:live"),
        "{digests:?}"
    );
    let gone = store
        .batch_delete(&t, &["urn:e:live".to_owned(), "urn:e:none".to_owned()])
        .await
        .expect("delete");
    assert_eq!(gone, vec![true, false]);

    let ctx = format!("urn:edge:any:{}", run());
    store
        .context_put(
            Some(&t),
            &ctx,
            json!({"localId": ctx, "kind": "Hosted", "owner": t.as_str(), "body": {}}),
        )
        .await
        .expect("put");
    let turned = store
        .context_mutate(
            Some(&t),
            &ctx,
            Box::new(|d: &mut serde_json::Value| {
                d["kind"] = json!("Cached");
                Ok(())
            }),
        )
        .await
        .expect("mutate");
    assert_eq!(turned, Some(Err(())));
    let never = TenantId::new(&format!("edge_never_{}", run())).expect("tenant");
    assert!(!store.purge_tenant(&never).await.expect("purge"));
    assert!(store.purge_tenant(&t).await.expect("purge"));
    pool.close().await;
}

/// A database the migrations never reached answers a store call with a
/// database error, never a panic; a fresh one sizes no Tenant before its
/// first ANALYZE and purges the default Tenant's rows but keeps its row.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_without_tables_or_statistics_answers_an_error() {
    let url = require_db!();
    let bare = own_database(&url, "bare").await;
    let raw = PgPool::connect(&bare).await.expect("connect");
    let store = AnyStore::Pg(PgBackend::new(raw.clone()));
    let t = TenantId::default();
    assert!(store.get(&t, Kind::Entity, "urn:e:1").await.is_err());
    raw.close().await;

    let fresh = own_database(&url, "fresh").await;
    let pool = pg::connect(&fresh, 4).await.expect("connect+migrate");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    assert!(Driver::tenant_bytes_estimate(&store, &t, 10).await.is_err());
    assert!(store.purge_tenant(&t).await.expect("purge"));
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants WHERE tenant_id = 'default'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(kept, 1);
    pool.close().await;
}
