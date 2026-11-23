// SPDX-License-Identifier: EUPL-1.2
//! T-7688 (review S5-26, S5-27, S5-28): start-up and the background ticks do
//! not walk every Tenant or every view each time. Each test migrates a
//! database of its own, so no other test's boot pass or write moves the
//! marks it reads. Needs ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe);
//! skips loudly without.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_sql::store::pg;
use antares_sql::store::pg::doc::{DocKind, PgDocStore};
use antares_sql::store::pg::seal::SecretKey;
use antares_store::Kind;
use serde_json::{json, Value};
use sqlx::{Executor, PgPool};
use std::time::Instant;

/// 32 zero bytes in base64: a test key that is plainly not a secret.
const TEST_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const TOKEN: &str = "Bearer the-webhook-token";

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

/// A migrated database of this test's own.
async fn own_database(url: &str, tag: &str) -> PgPool {
    let admin = pg::connect(url, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_scale_{tag}_{run}");
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .expect("own database");
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    pg::connect(&format!("{base}/{name}"), 4)
        .await
        .expect("connect+migrate the own database")
}

async fn tenants(pool: &PgPool, from: usize, to: usize) {
    for n in from..to {
        let t = TenantId::new(&format!("scale{n:04}")).expect("tenant");
        pg::ensure_tenant(pool, &t).await.expect("tenant row");
    }
}

fn sub(id: &str) -> Value {
    json!({"id": id, "type": "Subscription", "notification": {"endpoint": {
        "uri": "http://h/n",
        "receiverInfo": [{"key": "Authorization", "value": TOKEN}]
    }}})
}

/// A subscription row holding a plaintext credential, written past the
/// store. `tracked` = with the triggers on, as any release or statement
/// writes; untracked, with them off (a superuser's replica role), so no mark
/// is forgotten and only a walk that runs can seal it.
async fn raw_plaintext(pool: &PgPool, t: &TenantId, id: &str, tracked: bool) {
    let mut tx = pool.begin().await.expect("tx");
    if !tracked {
        sqlx::query("SET LOCAL session_replication_role = replica")
            .execute(&mut *tx)
            .await
            .expect("triggers off");
    }
    sqlx::query(
        "INSERT INTO subscriptions (tenant_id, id, subscription, context)
         VALUES ($1, $2, $3, '{}'::jsonb)",
    )
    .bind(t.as_str())
    .bind(id)
    .bind(sub(id))
    .execute(&mut *tx)
    .await
    .expect("raw row");
    tx.commit().await.expect("commit");
}

async fn sealed(pool: &PgPool, t: &TenantId, id: &str) -> bool {
    let text: String = sqlx::query_scalar(
        "SELECT subscription::text FROM subscriptions WHERE tenant_id = $1 AND id = $2",
    )
    .bind(t.as_str())
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("row");
    text.contains("enc:v1:") && !text.contains("the-webhook-token")
}

async fn marks(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM maintenance_jobs WHERE name LIKE 'plaintext_seal%' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .expect("marks")
}

async fn forget_marks(pool: &PgPool) {
    sqlx::query("DELETE FROM maintenance_jobs WHERE name LIKE 'plaintext_seal%'")
        .execute(pool)
        .await
        .expect("forget");
}

/// S5-27: the plaintext seal walk runs once, not at every boot. A second
/// keyed boot walks no Tenant and its time does not grow from 10 to 1,000
/// Tenants; a plaintext credential written by any statement forgets the mark
/// and the next keyed boot seals it; a walk that died leaves no mark; an
/// UPDATE of a row already plain forgets nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_seal_walk_runs_once_until_a_plaintext_write() {
    let url = require_db!();
    let pool = own_database(&url, "seal").await;
    let keyed = PgDocStore::new(pool.clone())
        .with_secret_key(SecretKey::from_base64(TEST_KEY).expect("key"));
    let keyless = PgDocStore::new(pool.clone());
    let a = TenantId::new("scale0000").expect("tenant");
    let mut timings = Vec::new();
    for (from, to) in [(0, 10), (10, 1_000)] {
        tenants(&pool, from, to).await;
        forget_marks(&pool).await;
        let started = Instant::now();
        keyed.seal_plaintext().await.expect("walk");
        let walk = started.elapsed();
        assert_eq!(marks(&pool).await, ["plaintext_sealed"]);
        let started = Instant::now();
        assert_eq!(keyed.seal_plaintext().await.expect("marked"), 0);
        timings.push((to, walk, started.elapsed()));
    }
    for (n, walk, marked) in &timings {
        eprintln!("seal at boot, {n} tenants: walk {walk:?}, marked {marked:?}");
    }

    raw_plaintext(&pool, &a, "urn:ngsi-ld:Subscription:untracked", false).await;
    keyed.seal_plaintext().await.expect("marked");
    assert!(
        !sealed(&pool, &a, "urn:ngsi-ld:Subscription:untracked").await,
        "a marked boot walks no Tenant"
    );

    // any statement leaving a plaintext credential forgets the mark
    raw_plaintext(&pool, &a, "urn:ngsi-ld:Subscription:tracked", true).await;
    assert!(marks(&pool).await.is_empty());
    assert!(keyed.seal_plaintext().await.expect("walk again") >= 2);
    for id in ["untracked", "tracked"] {
        assert!(sealed(&pool, &a, &format!("urn:ngsi-ld:Subscription:{id}")).await);
    }
    let id = "urn:ngsi-ld:Subscription:keyless";
    keyless
        .upsert(&a, DocKind::Subscription, id, &sub(id))
        .await
        .expect("keyless write");
    assert!(marks(&pool).await.is_empty(), "a keyless write forgets it");
    assert!(keyed.seal_plaintext().await.expect("walk again") >= 1);
    assert!(sealed(&pool, &a, id).await);

    // a walk that started and died: its token alone, no finished mark
    forget_marks(&pool).await;
    sqlx::query(
        "INSERT INTO maintenance_jobs (name, last_run) VALUES ('plaintext_seal_walk', now())",
    )
    .execute(&pool)
    .await
    .expect("a dead walk");
    raw_plaintext(&pool, &a, "urn:ngsi-ld:Subscription:afterdeath", false).await;
    keyed.seal_plaintext().await.expect("walk after a dead one");
    assert!(sealed(&pool, &a, "urn:ngsi-ld:Subscription:afterdeath").await);
    assert_eq!(marks(&pool).await, ["plaintext_sealed"]);

    // a bookkeeping write of a row already plain forgets nothing
    let plain = "urn:ngsi-ld:Subscription:plain";
    raw_plaintext(&pool, &a, plain, false).await;
    keyless
        .mutate(&a, DocKind::Subscription, plain, |doc| {
            doc["notification"]["timesSent"] = json!(1);
            Ok::<(), std::convert::Infallible>(())
        })
        .await
        .expect("bookkeeping");
    assert_eq!(marks(&pool).await, ["plaintext_sealed"]);
}

/// S5-27, S5-28: the Tenants holding a subscription are counted in the
/// writing transaction, so the mirror seed and the interval sweep list
/// those and not every Tenant: of 1,000 Tenants, the one with a
/// subscription and a context-source subscription, then one holding only a
/// registration (the registration mirror's domain); none once all go.
#[tokio::test(flavor = "multi_thread")]
async fn subscription_tenants_are_the_tenants_holding_one() {
    let url = require_db!();
    let pool = own_database(&url, "subt").await;
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let docs = PgDocStore::new(pool.clone());
    let a = TenantId::new("scale0007").expect("tenant");
    let mut timings = Vec::new();
    for (from, to) in [(0, 10), (10, 1_000)] {
        tenants(&pool, from, to).await;
        let started = Instant::now();
        let listed = store.subscription_tenants().await.expect("list");
        timings.push((to, started.elapsed(), listed.len()));
    }
    for (n, took, listed) in &timings {
        eprintln!("subscription_tenants, {n} tenants: {took:?}, {listed} listed");
    }
    assert!(timings.iter().all(|(_, _, listed)| *listed == 0));

    let s = "urn:ngsi-ld:Subscription:one";
    let c = "urn:ngsi-ld:Subscription:csub";
    docs.create(&a, DocKind::Subscription, s, &json!({"id": s}))
        .await
        .expect("sub");
    docs.create(&a, DocKind::CSourceSubscription, c, &json!({"id": c}))
        .await
        .expect("csub");
    assert_eq!(
        store.subscription_tenants().await.expect("list"),
        [a.as_str()]
    );
    assert!(docs
        .delete(&a, DocKind::Subscription, s)
        .await
        .expect("del"));
    assert_eq!(
        store.subscription_tenants().await.expect("list"),
        [a.as_str()]
    );
    assert!(docs
        .delete(&a, DocKind::CSourceSubscription, c)
        .await
        .expect("del"));
    assert!(store.subscription_tenants().await.expect("list").is_empty());
    let b = TenantId::new("scale0008").expect("tenant");
    let r = "urn:ngsi-ld:ContextSourceRegistration:only";
    docs.create(
        &b,
        DocKind::Registration,
        r,
        &json!({"id": r, "endpoint": "http://cs.invalid"}),
    )
    .await
    .expect("registration");
    assert_eq!(
        store.subscription_tenants().await.expect("list"),
        [b.as_str()]
    );
}

/// S5-26: the view reload lists 20,000 view documents whole, in id pages
/// with no row ceiling (the listing refused past MAX_UNDECIDED_ROWS, 10,000,
/// and the broker could not start), and the revision it polls first moves
/// with a write or a delete, never with a read or an unchanged write.
#[tokio::test(flavor = "multi_thread")]
async fn twenty_thousand_views_list_and_the_revision_moves_only_on_change() {
    let url = require_db!();
    let pool = own_database(&url, "views").await;
    let docs = PgDocStore::new(pool.clone());
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let t = TenantId::new("scaleviews").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    assert_eq!(docs.views_revision(&t).await.expect("revision"), 0);
    sqlx::query(
        "INSERT INTO views (tenant_id, id, doc)
         SELECT $1, format('v%s', lpad(n::text, 6, '0')), jsonb_build_object('n', n)
           FROM generate_series(1, 20000) n",
    )
    .bind(t.as_str())
    .execute(&pool)
    .await
    .expect("views");
    let revision = docs.views_revision(&t).await.expect("revision");
    assert_eq!(revision, 20_000);

    let started = Instant::now();
    let listed = docs.list_digests(&t, DocKind::View).await.expect("digests");
    eprintln!("list_digests, 20000 views: {:?}", started.elapsed());
    assert_eq!(listed.len(), 20_000);
    assert!(listed.windows(2).all(|w| w[0].0 < w[1].0), "id order");
    let started = Instant::now();
    assert_eq!(
        store.doc_revision(&t, Kind::View).await.expect("revision"),
        Some(revision),
        "a read moves nothing"
    );
    eprintln!("views revision read: {:?}", started.elapsed());

    docs.upsert(&t, DocKind::View, "v000001", &json!({"n": 1}))
        .await
        .expect("same body");
    assert_eq!(docs.views_revision(&t).await.expect("same"), revision);
    docs.upsert(&t, DocKind::View, "v000001", &json!({"n": -1}))
        .await
        .expect("new body");
    assert_eq!(docs.views_revision(&t).await.expect("moved"), revision + 1);
    assert!(docs
        .delete(&t, DocKind::View, "v000002")
        .await
        .expect("del"));
    assert_eq!(docs.views_revision(&t).await.expect("moved"), revision + 2);
    assert_eq!(
        store
            .doc_revision(&t, Kind::Subscription)
            .await
            .expect("none"),
        None
    );
}
