// SPDX-License-Identifier: EUPL-1.2
//! Versions of a measurement and provenance per write (T-4665, platform
//! ADR-N-054 2.1, 2.3, 2.4) on the Postgres temporal store. Skips loudly
//! without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::temporal::PgTemporalStore;
use serde_json::{json, Value};

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

const SPEED: &str = "https://uri.etsi.org/ngsi-ld/default-context/speed";

async fn ensure_entity(pool: &sqlx::PgPool, tenant: &TenantId, id: &str) {
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         VALUES ($1, $2, jsonb_build_object('id', $2::text), ARRAY['T'], now(), now())
         ON CONFLICT (tenant_id, id) DO NOTHING",
    )
    .bind(tenant.as_str())
    .bind(id)
    .execute(pool)
    .await
    .expect("entity row");
}

async fn clean(pool: &sqlx::PgPool, tenant: &str) {
    for sql in [
        "DELETE FROM attr_instances WHERE tenant_id = $1",
        "DELETE FROM attr_versions WHERE tenant_id = $1",
        "DELETE FROM temporal_entities WHERE tenant_id = $1",
        "DELETE FROM entities WHERE tenant_id = $1",
    ] {
        sqlx::query(sql)
            .bind(tenant)
            .execute(pool)
            .await
            .expect("clean");
    }
}

fn reading(value: f64, at: &str, by: &str) -> Value {
    json!({ SPEED: [{
        "type": "Property", "value": value, "observedAt": at,
        "instanceId": format!("urn:ngsi-ld:Instance:{at}"),
        "@provenance": {"channel": "pipeline", "actor": by, "operation": "update", "requestId": "r1"}
    }]})
}

/// (version, value, provenance) of the stored rows of one instant, current
/// row last.
async fn chain(pool: &sqlx::PgPool, tenant: &str, at: &str) -> Vec<(i32, Value, Option<Value>)> {
    let mut out: Vec<(i32, Value, Option<Value>)> = sqlx::query_as(
        "SELECT version, data -> 'value', provenance FROM attr_versions
         WHERE tenant_id = $1 AND observed_at = $2::timestamptz ORDER BY version",
    )
    .bind(tenant)
    .bind(at)
    .fetch_all(pool)
    .await
    .expect("versions");
    out.extend(
        sqlx::query_as::<_, (i32, Value, Option<Value>)>(
            "SELECT version, data -> 'value', provenance FROM attr_instances
             WHERE tenant_id = $1 AND observed_at = $2::timestamptz",
        )
        .bind(tenant)
        .bind(at)
        .fetch_all(pool)
        .await
        .expect("current"),
    );
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_correction_keeps_the_version_it_replaces_and_a_re_send_keeps_nothing() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgversions").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:v1";
    ensure_entity(&pool, &t, id).await;
    let shell = json!({"id": id, "type": "Vehicle"});
    let at = "2026-10-18T07:00:00Z";

    s.append(&t, id, &shell, &reading(80.0, at, "pipeline-a"))
        .await
        .expect("first");
    // The same value again is a re-send: no version, the row untouched.
    s.append(&t, id, &shell, &reading(80.0, at, "pipeline-a"))
        .await
        .expect("re-send");
    let once = chain(&pool, t.as_str(), at).await;
    assert_eq!(once.len(), 1, "{once:?}");
    assert_eq!(once[0].0, 1);
    assert_eq!(
        once[0].2.as_ref().expect("provenance")["actor"],
        "pipeline-a"
    );

    // Another value for the same instant is a correction: v1 kept, v2 current.
    s.append(&t, id, &shell, &reading(60.0, at, "pipeline-b"))
        .await
        .expect("correction");
    let corrected = chain(&pool, t.as_str(), at).await;
    assert_eq!(corrected.len(), 2, "{corrected:?}");
    assert_eq!((corrected[0].0, &corrected[0].1), (1, &json!(80.0)));
    assert_eq!((corrected[1].0, &corrected[1].1), (2, &json!(60.0)));
    let now = corrected[1].2.as_ref().expect("provenance");
    assert_eq!(
        (now["operation"].as_str(), now["actor"].as_str()),
        (Some("correct"), Some("pipeline-b"))
    );
    assert_eq!(
        corrected[0].2.as_ref().expect("kept")["actor"],
        "pipeline-a"
    );

    // A revert is one more version, never a rewrite of v1.
    s.append(&t, id, &shell, &reading(80.0, at, "pipeline-a"))
        .await
        .expect("revert");
    let reverted: Vec<i32> = chain(&pool, t.as_str(), at)
        .await
        .iter()
        .map(|r| r.0)
        .collect();
    assert_eq!(reverted, [1, 2, 3]);

    // A read answers the current version, and never the provenance as data.
    let doc = s.get(&t, id).await.expect("get").expect("present");
    let instances = doc[SPEED].as_array().expect("instances");
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0]["value"], 80.0);
    assert!(instances[0].get("@provenance").is_none(), "{doc}");

    // Deleting the Entity's history takes its versions too.
    assert!(s.delete(&t, id).await.expect("delete"));
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM attr_versions WHERE tenant_id = $1")
        .bind(t.as_str())
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0);
    clean(&pool, t.as_str()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_instance_leaves_its_last_version() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgversionsdel").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:v2";
    ensure_entity(&pool, &t, id).await;
    let at = "2026-10-18T08:00:00Z";
    s.append(
        &t,
        id,
        &json!({"id": id, "type": "Vehicle"}),
        &reading(5.0, at, "ui-user"),
    )
    .await
    .expect("append");
    // 5.6.15: the instance goes from history; its version stays on record.
    let r = s
        .mutate(&t, id, |d| {
            d[SPEED].as_array_mut().expect("instances").clear();
            Ok::<(), ()>(())
        })
        .await;
    assert!(matches!(r, Ok(Some(Ok(())))));
    let kept = chain(&pool, t.as_str(), at).await;
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!((kept[0].0, &kept[0].1), (1, &json!(5.0)));
    clean(&pool, t.as_str()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn history_retention_takes_the_superseded_versions_along() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgversionsret").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:v3";
    ensure_entity(&pool, &t, id).await;
    let shell = json!({"id": id, "type": "Vehicle"});
    let at = "2026-10-18T09:00:00Z";
    for (v, by) in [(1.0, "a"), (2.0, "b")] {
        s.append(&t, id, &shell, &reading(v, at, by))
            .await
            .expect("append");
    }
    assert_eq!(chain(&pool, t.as_str(), at).await.len(), 2);
    let gone = pg::maintenance::retain_history(
        &pool,
        t.as_str(),
        antares_store::HistoryScope::Entity(id),
        "2026-10-18T10:00:00Z",
        100,
        false,
    )
    .await
    .expect("retain");
    assert_eq!(gone, 2, "the current row and its superseded version");
    assert!(chain(&pool, t.as_str(), at).await.is_empty());
    clean(&pool, t.as_str()).await;
}
