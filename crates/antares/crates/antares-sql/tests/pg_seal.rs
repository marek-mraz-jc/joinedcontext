// SPDX-License-Identifier: EUPL-1.2
//! T-5838: notification credentials sealed at rest in the doc store. Needs
//! ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe); skips loudly without.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::doc::{DocKind, PgDocStore};
use antares_sql::store::pg::seal::SecretKey;
use serde_json::{json, Value};

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

fn sub(id: &str) -> Value {
    json!({"id": id, "type": "Subscription", "notification": {"endpoint": {
        "uri": "http://h/n",
        "receiverInfo": [{"key": "Authorization", "value": TOKEN}]
    }}})
}

async fn tenant(pool: &sqlx::PgPool, name: &str) -> TenantId {
    let t = TenantId::new(name).expect("tenant");
    pg::ensure_tenant(pool, &t).await.expect("tenant row");
    forget(pool, name).await;
    t
}

/// Delete what a test sealed: the database is shared with every other pg
/// test, and a broker started there without this key fails each read of
/// such a row (antares-broker upsert_transactions never went idle).
async fn forget(pool: &sqlx::PgPool, name: &str) {
    for table in ["subscriptions", "dead_letters"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE tenant_id = '{name}'"
        )))
        .execute(pool)
        .await
        .expect("clean");
    }
}

/// The stored text of one row, read past the store.
async fn raw(pool: &sqlx::PgPool, t: &TenantId, table: &str, col: &str, id: &str) -> String {
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(t.as_str())
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    let text: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT {col}::text FROM {table} WHERE tenant_id = $1 AND id = $2"
    )))
    .bind(t.as_str())
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .expect("row");
    tx.commit().await.expect("commit");
    text
}

fn value(doc: &Value) -> &Value {
    &doc["notification"]["endpoint"]["receiverInfo"][0]["value"]
}

#[tokio::test(flavor = "multi_thread")]
async fn every_write_seals_and_every_read_opens_the_credential() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("pool");
    let s = PgDocStore::new(pool.clone())
        .with_secret_key(SecretKey::from_base64(TEST_KEY).expect("key"));
    let t = tenant(&pool, "pgseal").await;
    let id = "urn:ngsi-ld:Subscription:sealed";
    assert!(s
        .create(&t, DocKind::Subscription, id, &sub(id))
        .await
        .expect("create"));
    let text = raw(&pool, &t, "subscriptions", "subscription", id).await;
    assert!(
        !text.contains("the-webhook-token") && text.contains("enc:v1:"),
        "{text}"
    );

    let got = s
        .get(&t, DocKind::Subscription, id)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(value(&got), TOKEN);
    assert_eq!(
        value(&s.list(&t, DocKind::Subscription).await.expect("list")[0]),
        TOKEN
    );
    let page = s
        .list_page(&t, DocKind::Subscription, None, 5)
        .await
        .expect("page");
    assert_eq!(value(&page[0]), TOKEN);
    let (slice, _) = s
        .list_slice(&t, DocKind::Subscription, 0, 5)
        .await
        .expect("slice");
    assert_eq!(value(&slice[0]), TOKEN);
    let (after, _) = s
        .record_delivery(&t, DocKind::Subscription, id, "2026-10-27T00:00:00Z")
        .await
        .expect("delivery")
        .expect("row");
    assert_eq!(value(&after), TOKEN);

    // a read-modify-write sees plaintext and stores ciphertext again
    let seen = s
        .mutate(&t, DocKind::Subscription, id, |d| {
            d["description"] = json!("changed");
            Ok::<Value, std::convert::Infallible>(value(d).clone())
        })
        .await
        .expect("mutate");
    assert!(matches!(seen, Some(Ok(v)) if v == TOKEN));
    let text = raw(&pool, &t, "subscriptions", "subscription", id).await;
    assert!(
        !text.contains("the-webhook-token") && text.contains("changed"),
        "{text}"
    );

    s.upsert(&t, DocKind::Subscription, id, &sub(id))
        .await
        .expect("upsert");
    let text = raw(&pool, &t, "subscriptions", "subscription", id).await;
    assert!(!text.contains("the-webhook-token"), "{text}");

    let letter = "urn:ngsi-ld:DeadLetter:sealed";
    let doc = json!({"id": letter, "receiverInfo": [["Authorization", TOKEN]]});
    s.upsert(&t, DocKind::DeadLetter, letter, &doc)
        .await
        .expect("letter");
    let text = raw(&pool, &t, "dead_letters", "doc", letter).await;
    assert!(!text.contains("the-webhook-token"), "{text}");
    let back = s
        .get(&t, DocKind::DeadLetter, letter)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(back, doc);

    // a store without the key, or with another, refuses the sealed row
    assert!(PgDocStore::new(pool.clone())
        .get(&t, DocKind::Subscription, id)
        .await
        .is_err());
    let other =
        SecretKey::from_base64("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=").expect("key");
    let wrong = PgDocStore::new(pool.clone()).with_secret_key(other);
    assert!(wrong.get(&t, DocKind::Subscription, id).await.is_err());
    forget(&pool, "pgseal").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_written_before_the_key_are_read_and_sealed_at_boot() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("pool");
    let t = tenant(&pool, "pgsealboot").await;
    let id = "urn:ngsi-ld:Subscription:legacy";
    let letter = "urn:ngsi-ld:DeadLetter:legacy";
    let before = PgDocStore::new(pool.clone());
    assert_eq!(before.seal_plaintext().await.expect("no key"), 0);
    before
        .upsert(&t, DocKind::Subscription, id, &sub(id))
        .await
        .expect("legacy");
    let doc = json!({"id": letter, "headers": [["Authorization", TOKEN]]});
    before
        .upsert(&t, DocKind::DeadLetter, letter, &doc)
        .await
        .expect("letter");

    let s = PgDocStore::new(pool.clone())
        .with_secret_key(SecretKey::from_base64(TEST_KEY).expect("key"));
    let got = s
        .get(&t, DocKind::Subscription, id)
        .await
        .expect("plaintext reads");
    assert_eq!(value(&got.expect("row")), TOKEN);

    assert!(s.seal_plaintext().await.expect("seal") >= 2);
    for (table, col, row) in [
        ("subscriptions", "subscription", id),
        ("dead_letters", "doc", letter),
    ] {
        let text = raw(&pool, &t, table, col, row).await;
        assert!(
            !text.contains("the-webhook-token") && text.contains("enc:v1:"),
            "{text}"
        );
    }
    assert_eq!(
        s.get(&t, DocKind::DeadLetter, letter).await.expect("get"),
        Some(doc)
    );
    let again = s.seal_plaintext().await.expect("again");
    let still = raw(&pool, &t, "subscriptions", "subscription", id).await;
    assert!(
        still.contains("enc:v1:"),
        "{still} after a second pass sealing {again}"
    );
    forget(&pool, "pgsealboot").await;
}

/// The bounce of T-5838: a dump holder who can create subscriptions pastes
/// another row's `enc:v1:` value. Sent as a value, it is sealed as text and
/// delivered as that text; copied into the row by SQL, it does not open.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_copied_from_another_row_is_never_decrypted() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("pool");
    let s = PgDocStore::new(pool.clone())
        .with_secret_key(SecretKey::from_base64(TEST_KEY).expect("key"));
    let t = tenant(&pool, "pgsealcopy").await;
    let victim = "urn:ngsi-ld:Subscription:victim";
    s.create(&t, DocKind::Subscription, victim, &sub(victim))
        .await
        .expect("victim");
    let stored: Value =
        serde_json::from_str(&raw(&pool, &t, "subscriptions", "subscription", victim).await)
            .expect("json");
    let pasted = value(&stored).as_str().expect("sealed text").to_owned();
    assert!(pasted.starts_with("enc:v1:"), "{pasted}");

    let thief = "urn:ngsi-ld:Subscription:thief";
    let mut doc = sub(thief);
    doc["notification"]["endpoint"]["receiverInfo"][0]["value"] = json!(pasted);
    s.create(&t, DocKind::Subscription, thief, &doc)
        .await
        .expect("thief");
    let got = s
        .get(&t, DocKind::Subscription, thief)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(
        value(&got),
        &json!(pasted),
        "the literal text, not the token"
    );

    // the row copied by SQL, as a database writer would
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(t.as_str())
        .execute(&mut *tx)
        .await
        .expect("tenant setting");
    sqlx::query(
        "UPDATE subscriptions SET subscription = jsonb_set(subscription,
           '{notification,endpoint,receiverInfo,0,value}', to_jsonb($3::text))
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(t.as_str())
    .bind(thief)
    .bind(&pasted)
    .execute(&mut *tx)
    .await
    .expect("copy");
    tx.commit().await.expect("commit");
    assert!(s.get(&t, DocKind::Subscription, thief).await.is_err());
    forget(&pool, "pgsealcopy").await;
}
