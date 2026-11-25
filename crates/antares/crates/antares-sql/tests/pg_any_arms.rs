// SPDX-License-Identifier: EUPL-1.2
//! The `AnyStore` seam on both arms (T-7594): the reads and writes the broker reaches only on
//! one deployment shape (temporal keyset pages, digests of a document kind, purge of a Tenant,
//! `@context` rows without a kind, the outbox switches, versions), answered the same way the
//! contract answers them, and a database that stops answering turned into the one generic
//! error. The Postgres half skips loudly without ANTARES_TEST_DATABASE_URL (see tests/pg.rs).

#![cfg(feature = "postgres")]

use antares_model::{NgsiError, TenantId};
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_sql::store::filter::{EntityFilter, TemporalFilter};
use antares_sql::store::mem::Store;
use antares_sql::store::pg;
use antares_store::{
    ContextCaps, CurrentStateDriver, Kind, TemporalDriver, TemporalEvent, TemporalOp,
};
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

const SPEED: &str = "https://example.org/speed";

fn temporal(id: &str, at: &str) -> Value {
    json!({
        "id": id, "type": ["https://example.org/V"],
        "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
        SPEED: [{"type": "Property", "value": 1, "observedAt": at, "instanceId": format!("urn:i:{id}")}]
    })
}

async fn tenant(pool: &sqlx::PgPool, name: &str) -> TenantId {
    let t = TenantId::new(name).expect("tenant");
    pg::ensure_tenant(pool, &t).await.expect("tenant row");
    t
}

fn internal(e: NgsiError) -> String {
    match e {
        NgsiError::InternalError(d) => d,
        other => format!("not an InternalError: {other:?}"),
    }
}

/// Temporal state pages by id on Postgres, though it has no keyset reader of its own: a
/// page after an id starts past it, a slice counts every Entity.
#[tokio::test(flavor = "multi_thread")]
async fn temporal_state_pages_by_id_and_slices_with_its_total() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = tenant(&pool, "anytemporalpage").await;
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let _ = store.purge_tenant(&t).await;
    let t = tenant(&pool, "anytemporalpage").await;
    for id in ["urn:v:c", "urn:v:a", "urn:v:b"] {
        store
            .upsert(&t, Kind::Temporal, id, temporal(id, "2026-08-04T09:00:00Z"))
            .await
            .expect("history");
    }
    let page = store
        .list_page(&t, Kind::Temporal, None, 2)
        .await
        .expect("first page");
    let ids = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .map(|r| r["id"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(ids(&page), ["urn:v:a", "urn:v:b"]);
    let rest = store
        .list_page(&t, Kind::Temporal, Some("urn:v:b"), 2)
        .await
        .expect("next page");
    assert_eq!(ids(&rest), ["urn:v:c"]);
    let (slice, total) = store
        .list_slice(&t, Kind::Temporal, 1, 1)
        .await
        .expect("slice");
    assert_eq!((ids(&slice), total), (vec!["urn:v:b".to_owned()], 3));
    let (none, total) = store
        .list_slice(&t, Kind::Temporal, 5, 1)
        .await
        .expect("past the end");
    assert!(none.is_empty());
    assert_eq!(total, 3);

    // an append to an Entity's history lands in what the next read returns; the history
    // half of a mixed deployment appends without looking for the Entity's current state
    let store = AnyStore::Pg(PgBackend::new(pool.clone())).temporal_only();
    store
        .temporal_append(
            &t,
            "urn:v:a",
            &temporal("urn:v:a", "2026-08-04T09:00:00Z"),
            &json!({SPEED: [{"type": "Property", "value": 2, "observedAt": "2026-08-04T10:00:00Z",
                              "instanceId": "urn:i:second"}]}),
        )
        .await
        .expect("append");
    let doc = store
        .get_temporal(&t, "urn:v:a", &TemporalFilter::default())
        .await
        .expect("read")
        .expect("present");
    assert_eq!(doc[SPEED].as_array().map(Vec::len), Some(2), "{doc}");

    // versions: the Postgres arm keeps superseded versions, an Entity without any has none
    let versions = TemporalDriver::versions(&store, &t, "urn:v:a")
        .await
        .expect("versions");
    assert!(versions.values().all(Value::is_array), "{versions:?}");
    assert!(store.purge_tenant(&t).await.expect("purge"));
}

/// Digests of a document kind come from the document table on Postgres; purge removes a known
/// Tenant and reports an unknown one; a `@context` row without its kind is refused before any
/// write; a mutation that changes a row's kind is refused.
#[tokio::test(flavor = "multi_thread")]
async fn documents_digest_purge_and_context_rows_refuse_what_the_contract_refuses() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let t = tenant(&pool, "anydocs").await;
    let sub = json!({"id": "urn:ngsi-ld:Subscription:d1", "type": "Subscription",
                     "entities": [{"type": "https://example.org/V"}],
                     "notification": {"endpoint": {"uri": "http://example.org/n"}}});
    store
        .upsert(&t, Kind::Subscription, "urn:ngsi-ld:Subscription:d1", sub)
        .await
        .expect("subscription");
    let digests = store
        .list_digests(&t, Kind::Subscription)
        .await
        .expect("digests");
    assert_eq!(digests.len(), 1, "{digests:?}");
    assert_eq!(digests[0].0, "urn:ngsi-ld:Subscription:d1");

    // a row without an owner is the default Tenant's: the kind check is the one that refuses
    let default = TenantId::new(TenantId::DEFAULT).expect("default tenant");
    let refused = store
        .context_put(Some(&default), "urn:ctx:nokind", json!({"body": {}}))
        .await
        .expect_err("no kind");
    assert_eq!(internal(refused), "@context row without a kind");
    let refused = store
        .context_put_capped(
            &default,
            "urn:ctx:nokind",
            json!({"body": {}}),
            ContextCaps {
                count: 10,
                bytes: 1 << 20,
            },
        )
        .await
        .expect_err("no kind, capped");
    assert_eq!(internal(refused), "@context row without a kind");

    let unknown = TenantId::new("anydocs-never-made").expect("tenant");
    assert!(!store.purge_tenant(&unknown).await.expect("purge unknown"));
    assert!(store.purge_tenant(&t).await.expect("purge known"));
    assert!(
        store
            .list_digests(&t, Kind::Subscription)
            .await
            .expect("digests after purge")
            .is_empty(),
        "the purge took the Tenant's documents"
    );
    assert!(!store.purge_tenant(&t).await.expect("purge again"), "gone");
}

/// The outbox switches and the batch knobs on Postgres, through the driver trait the broker
/// holds: retaining nothing changes nothing, an unknown seq has no event, and the expired
/// sweep is the database's own job (0 here).
#[tokio::test(flavor = "multi_thread")]
async fn the_outbox_switches_answer_through_the_driver_trait() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let reserve = pg::connect(&url, 2).await.expect("reserve");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()).with_write_reserve(reserve));
    let t = tenant(&pool, "anyoutbox").await;
    CurrentStateDriver::set_outbox(&store, true);
    store.set_async_batch_commit(true);
    assert_eq!(
        CurrentStateDriver::outbox_retain(&store, &t, &[])
            .await
            .expect("retain"),
        0
    );
    assert_eq!(
        CurrentStateDriver::outbox_ack(&store, &[])
            .await
            .expect("ack"),
        0
    );
    assert_eq!(
        CurrentStateDriver::outbox_event(&store, i64::MAX, &t)
            .await
            .expect("event"),
        None
    );
    assert_eq!(store.sweep_expired(), 0);
    assert_eq!(store.seal_plaintext_credentials().await.expect("no key"), 0);

    // a batch write through the reserve pool books its event with the producer on
    let id = "urn:ngsi-ld:T:anyoutbox";
    let _ = store.delete(&t, Kind::Entity, id).await;
    let doc = json!({"id": id, "type": ["https://example.org/T"],
                     "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z"});
    assert!(store.upsert(&t, Kind::Entity, id, doc).await.is_ok());
    let booked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE tenant_id = 'anyoutbox' AND event->>'entity_id' = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("outbox");
    assert!(booked >= 1, "the upsert booked its change");
    CurrentStateDriver::set_outbox(&store, false);
    let _ = store.delete(&t, Kind::Entity, id).await;
}

/// An Entity row whose attribute instance expired is read without it, as the evaluator reads
/// it (4.22), on the raw-text path the query takes when it can.
#[tokio::test(flavor = "multi_thread")]
async fn a_text_query_strips_an_expired_attribute_instance() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    let t = tenant(&pool, "anytextexpiry").await;
    let id = "urn:ngsi-ld:T:textexpiry";
    let _ = store.delete(&t, Kind::Entity, id).await;
    let doc = json!({
        "id": id, "type": ["https://example.org/T"],
        "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
        "https://example.org/old": [{"type": "Property", "value": 1, "expiresAt": "2001-01-01T00:00:00Z"}],
        "https://example.org/kept": [{"type": "Property", "value": 2}]
    });
    assert!(store
        .create(&t, Kind::Entity, id, doc)
        .await
        .expect("create"));
    let out = store
        .query_entities_text(&t, &EntityFilter::default())
        .await
        .expect("query");
    let rows: Vec<Value> = out
        .rows
        .iter()
        .map(|r| serde_json::from_str(r.get()).expect("row text"))
        .collect();
    let row = rows.iter().find(|r| r["id"] == id).expect("the entity");
    assert!(row.get("https://example.org/old").is_none(), "{row}");
    assert_eq!(row["https://example.org/kept"][0]["value"], 2);
    let _ = store.delete(&t, Kind::Entity, id).await;
}

/// A database that stops answering (the pool closed under the drain) turns into the generic
/// 500 detail, never a panic or the driver's own text, on every seam the broker drives.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_pool_answers_the_generic_database_error_everywhere() {
    let url = require_db!();
    let pool = pg::connect(&url, 2).await.expect("connect");
    let t = tenant(&pool, "anyclosed").await;
    let store = AnyStore::Pg(PgBackend::new(pool.clone()));
    store.close().await;
    let doc = temporal("urn:v:closed", "2026-08-04T09:00:00Z");
    let mut details = vec![
        store.list_digests(&t, Kind::Subscription).await.map(|_| ()),
        store.list_page(&t, Kind::Subscription, None, 1).await.map(|_| ()),
        store.list_slice(&t, Kind::Subscription, 0, 1).await.map(|_| ()),
        store
            .query_entities_text(&t, &EntityFilter::default())
            .await
            .map(|_| ()),
        store.entity_id_segments(&t, "urn:", ':', 5).await.map(|_| ()),
        store
            .temporal_append(&t, "urn:v:closed", &doc, &json!({}))
            .await,
        store
            .upsert(&t, Kind::Temporal, "urn:v:closed", doc.clone())
            .await
            .map(|_| ()),
        store.purge_tenant(&t).await.map(|_| ()),
        store
            .context_put_capped(
                &t,
                "urn:ctx:closed",
                json!({"kind": "Hosted", "owner": "anyclosed", "body": {}}),
                ContextCaps { count: 1, bytes: 1 },
            )
            .await
            .map(|_| ()),
        store.outbox_retain(&t, &[1]).await.map(|_| ()),
        TemporalDriver::versions(&store, &t, "urn:v:closed").await.map(|_| ()),
        TemporalDriver::event_list(
            &store,
            &[TemporalEvent {
                op: TemporalOp::AttrCreated,
                tenant: t.clone(),
                entity_id: "urn:v:closed".into(),
                shell: doc.clone(),
                attr: SPEED.into(),
                instance: json!({"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}),
            }],
        )
        .await,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, r)| (i, r.map_err(internal)))
    .collect::<Vec<_>>();
    details.retain(|(_, r)| r.as_ref().err().map(String::as_str) != Some("database error"));
    assert!(
        details.is_empty(),
        "each seam answers the generic detail: {details:?}"
    );
}

/// The memory arm's answers to the same seams: no id segments to offer, no versions, no
/// SQL page to trust, a temporal-only flag that keeps appends to history, and an expired
/// history document read as absent.
#[tokio::test]
async fn the_memory_arm_answers_the_seams_it_has_no_index_for() {
    let store = AnyStore::Mem(Store::default()).temporal_only();
    let t = TenantId::new("anymem").expect("tenant");
    assert_eq!(
        store
            .entity_id_segments(&t, "urn:", ':', 5)
            .await
            .expect("segments"),
        None
    );
    assert!(TemporalDriver::versions(&store, &t, "urn:v:a")
        .await
        .expect("versions")
        .is_empty());
    let q = antares_ql::parse_q("speed>1").expect("q");
    assert!(!TemporalDriver::q_pushdown_exact(&store, &q, None, &|s| s.to_owned()));
    let mut expired = temporal("urn:v:gone", "2026-08-04T09:00:00Z");
    expired["expiresAt"] = json!("2001-01-01T00:00:00Z");
    store
        .upsert(&t, Kind::Temporal, "urn:v:gone", expired)
        .await
        .expect("history");
    assert_eq!(
        store
            .get_temporal(&t, "urn:v:gone", &TemporalFilter::default())
            .await
            .expect("read"),
        None,
        "an expired history document reads as absent"
    );
    // an append to an Entity the memory arm never saw creates its history (temporal-only)
    store
        .temporal_append(
            &t,
            "urn:v:new",
            &temporal("urn:v:new", "2026-08-04T09:00:00Z"),
            &json!({SPEED: [{"type": "Property", "value": 1, "observedAt": "2026-08-04T09:00:00Z"}]}),
        )
        .await
        .expect("append");
    assert!(store
        .get_temporal(&t, "urn:v:new", &TemporalFilter::default())
        .await
        .expect("read")
        .is_some());
}
