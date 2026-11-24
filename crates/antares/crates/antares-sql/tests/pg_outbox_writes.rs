// SPDX-License-Identifier: EUPL-1.2
//! The change events every entity write puts in the outbox, in its own transaction, once the
//! producer is on (bus=nats, T-7594): delete, conditional delete, patch and the four batch
//! writes each book one event per entity they changed, with the before- and after-images and
//! the changed attributes, and none for an entity they did not touch. Skips loudly without
//! ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::entity::PgEntityStore;
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

const N: &str = "https://example.org/n";

fn doc(id: &str, n: i64) -> Value {
    json!({
        "id": id, "type": ["https://example.org/T"],
        "createdAt": "2026-08-04T09:00:00Z", "modifiedAt": "2026-08-04T09:00:00Z",
        N: {"type": "Property", "value": n}
    })
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| format!("urn:ngsi-ld:T:{s}")).collect()
}

/// The events booked for `tenant` since `after`, oldest first, and the newest seq.
async fn events(pool: &sqlx::PgPool, tenant: &str, after: i64) -> (Vec<Value>, i64) {
    let rows: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT seq, event FROM outbox WHERE tenant_id = $1 AND seq > $2 ORDER BY seq",
    )
    .bind(tenant)
    .bind(after)
    .fetch_all(pool)
    .await
    .expect("outbox");
    let last = rows.last().map_or(after, |(s, _)| *s);
    (rows.into_iter().map(|(_, e)| e).collect(), last)
}

/// `(op, entity id)` of each event.
fn ops(evs: &[Value]) -> Vec<(String, String)> {
    evs.iter()
        .map(|e| {
            (
                e["op"].as_str().unwrap_or_default().to_owned(),
                e["entity_id"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_entity_write_books_its_change_in_the_same_transaction() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = "pgoutboxwrites";
    let tenant = TenantId::new(t).expect("tenant");
    pg::ensure_tenant(&pool, &tenant).await.expect("tenant row");
    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(t)
        .execute(&pool)
        .await
        .expect("clean entities");
    sqlx::query("DELETE FROM outbox WHERE tenant_id = $1")
        .bind(t)
        .execute(&pool)
        .await
        .expect("clean outbox");
    let s = PgEntityStore::new(pool.clone());
    let [a, b, c, d, e, f, g] =
        ["a", "b", "c", "d", "e", "f", "missing"].map(|n| format!("urn:ngsi-ld:T:{n}"));
    for id in [&a, &b, &c] {
        assert!(s.create(&tenant, id, &doc(id, 1)).await.expect("create"));
    }
    // the producer off: these creates booked nothing
    let (before, mut seq) = events(&pool, t, 0).await;
    assert!(before.is_empty(), "{before:?}");
    s.set_outbox(true);

    // delete: one event with the before-image and every attribute it held
    let gone = s
        .delete(&tenant, &a)
        .await
        .expect("delete")
        .expect("present");
    assert_eq!(gone[N]["value"], 1);
    assert_eq!(s.delete(&tenant, &g).await.expect("delete absent"), None);
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    assert_eq!(ops(&evs), [("delete".to_owned(), a.clone())]);
    assert_eq!(evs[0]["prev_payload"][N]["value"], 1);
    assert_eq!(evs[0]["payload"], Value::Null);
    assert_eq!(evs[0]["changed_attrs"], json!([N]));

    // a conditional delete the condition refuses books nothing; one it allows books one
    assert_eq!(
        s.delete_if(&tenant, &b, &|_| false).await.expect("refused"),
        None
    );
    assert!(s.get(&tenant, &b).await.expect("get").is_some(), "kept");
    assert!(s
        .delete_if(&tenant, &b, &|v| v[N]["value"] == 1)
        .await
        .expect("allowed")
        .is_some());
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    assert_eq!(ops(&evs), [("delete".to_owned(), b.clone())]);

    // a patch: an update event naming only the attribute that changed
    let patched = s
        .mutate(&tenant, &c, |doc| {
            doc[N]["value"] = json!(2);
            Ok::<(), ()>(())
        })
        .await
        .expect("mutate");
    assert_eq!(patched, Some(Ok(())));
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    assert_eq!(ops(&evs), [("update".to_owned(), c.clone())]);
    assert_eq!(evs[0]["prev_payload"][N]["value"], 1);
    assert_eq!(evs[0]["payload"][N]["value"], 2);
    assert_eq!(evs[0]["changed_attrs"], json!([N]));
    assert!(
        evs[0]["version"].as_i64().is_some_and(|v| v >= 2),
        "{}",
        evs[0]
    );

    // batch create: one create per new id, none for the existing or the repeated one
    let made = s
        .batch_create(
            &tenant,
            &[
                (d.clone(), doc(&d, 1)),
                (e.clone(), doc(&e, 1)),
                (d.clone(), doc(&d, 9)),
                (c.clone(), doc(&c, 9)),
            ],
        )
        .await
        .expect("batch create");
    assert_eq!(made, [true, true, false, false]);
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    assert_eq!(
        ops(&evs),
        [
            ("create".to_owned(), d.clone()),
            ("create".to_owned(), e.clone())
        ]
    );
    assert_eq!(evs[0]["version"], 1);

    // batch upsert (replace): an update for the existing id, a create for the new one
    let upserted = s
        .batch_upsert_replace(&tenant, &[(d.clone(), doc(&d, 3)), (f.clone(), doc(&f, 1))])
        .await
        .expect("batch upsert");
    assert_eq!(upserted.len(), 2);
    assert!(
        !upserted[0].0 && upserted[0].1.is_some(),
        "d replaced: {upserted:?}"
    );
    assert!(
        upserted[1].0 && upserted[1].1.is_none(),
        "f created: {upserted:?}"
    );
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    let mut got = ops(&evs);
    got.sort();
    assert_eq!(
        got,
        [
            ("create".to_owned(), f.clone()),
            ("update".to_owned(), d.clone())
        ]
    );
    let update = evs.iter().find(|e| e["op"] == "update").expect("update");
    assert_eq!(update["prev_payload"][N]["value"], 1);
    assert_eq!(update["payload"][N]["value"], 3);

    // batch patch: an update per entity the closure changed; the absent id books nothing and
    // the one the closure refuses keeps its row and books nothing either
    let results = s
        .batch_mutate(&tenant, &[d.clone(), e.clone(), g.clone()], |id, doc| {
            if id.ends_with(":e") {
                return Err("refused");
            }
            doc[N]["value"] = json!(4);
            Ok(())
        })
        .await
        .expect("batch mutate");
    assert_eq!(results, [Some(Ok(())), Some(Err("refused")), None]);
    let (evs, last) = events(&pool, t, seq).await;
    seq = last;
    assert_eq!(ops(&evs), [("update".to_owned(), d.clone())]);
    assert_eq!(evs[0]["payload"][N]["value"], 4);

    // batch delete: one delete per row it removed, in one INSERT
    let deleted = s
        .batch_delete(&tenant, &ids(&["d", "e", "missing"]))
        .await
        .expect("batch delete");
    let mut removed: Vec<String> = deleted.iter().map(|(id, _)| id.clone()).collect();
    removed.sort();
    assert_eq!(removed, [d.clone(), e.clone()]);
    let (evs, _) = events(&pool, t, seq).await;
    let mut got = ops(&evs);
    got.sort();
    assert_eq!(
        got,
        [
            ("delete".to_owned(), d.clone()),
            ("delete".to_owned(), e.clone())
        ]
    );
    assert!(evs
        .iter()
        .all(|e| e["payload"].is_null() && e["prev_payload"][N].is_object()));
}
