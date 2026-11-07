// SPDX-License-Identifier: EUPL-1.2
//! A history row keeps its instanceId in the `instance_id` column only
//! (T-5131) and every read puts it back. Skips loudly without
//! ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::temporal::PgTemporalStore;
use antares_store::filter::{InstanceRange, TemporalFilter};
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
const NAME: &str = "https://uri.etsi.org/ngsi-ld/default-context/name";
const PLACE: &str = "https://uri.etsi.org/ngsi-ld/location";
const OWNER: &str = "https://uri.etsi.org/ngsi-ld/default-context/owner";

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

/// (instance_id column, instanceId as stored in `data`) per row.
async fn stored(pool: &sqlx::PgPool, tenant: &str) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT instance_id, data ->> 'instanceId' FROM attr_instances
         WHERE tenant_id = $1 ORDER BY instance_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .expect("rows")
}

fn inst(n: u32, created: &str, modified: &str, extra: Value) -> Value {
    let mut i = json!({"type": "Property", "value": n, "observedAt": "2026-10-20T08:00:00+02:00",
                       "instanceId": format!("urn:ngsi-ld:Instance:s{n}"),
                       "createdAt": created, "modifiedAt": modified});
    if let (Some(o), Some(e)) = (i.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    i
}

/// Every attribute kind reads back identical with no instanceId in `data`,
/// whatever the spelling of its stamps, and a pre-T-5131 row (instanceId
/// still in `data`) reads back as stored.
#[tokio::test(flavor = "multi_thread")]
async fn instance_ids_live_in_their_column_only() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgstamps").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:stamps1";
    let ms = "2026-10-20T06:00:00.120Z";
    let ms2 = "2026-10-20T06:05:00.000Z";
    let doc = json!({
        "id": id, "type": "Vehicle",
        "createdAt": ms, "modifiedAt": ms2,
        // in read order: createdAt, then observedAt, then instanceId
        SPEED: [
            // seconds form, an offset, a micro fraction
            inst(2, "2026-10-20T06:00:00Z", ms2, json!({})),
            inst(1, ms, ms2, json!({"unitCode": "KMH", "datasetId": "urn:ngsi-ld:Dataset:gps"})),
            inst(3, ms, "2026-10-20T08:05:00.000+02:00", json!({})),
            inst(5, ms, ms2, json!({SPEED: [{"type": "Property", "value": "sub",
                                             "createdAt": ms, "modifiedAt": ms2}]})),
            inst(4, "2026-10-20T06:00:00.123456Z", ms2, json!({})),
        ],
        NAME: [{"type": "LanguageProperty", "languageMap": {"sk": "auto", "en": "car"},
                "instanceId": "urn:ngsi-ld:Instance:s6", "createdAt": ms, "modifiedAt": ms2}],
        PLACE: [{"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.1, 48.7]},
                 "instanceId": "urn:ngsi-ld:Instance:s7", "createdAt": ms, "modifiedAt": ms2}],
        OWNER: [{"type": "Relationship", "object": "urn:ngsi-ld:Person:p1",
                 "instanceId": "urn:ngsi-ld:Instance:s8", "createdAt": ms, "modifiedAt": ms2}],
    });
    assert!(s.create(&t, id, &doc).await.expect("create"));
    assert_eq!(s.get(&t, id).await.expect("get").expect("present"), doc);

    let rows = stored(&pool, t.as_str()).await;
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|r| r.1.is_none()), "{rows:?}");

    // A row written before T-5131 holds its instanceId in data: read as stored.
    sqlx::query(
        "UPDATE attr_instances SET data = data || jsonb_build_object('instanceId', instance_id)
         WHERE tenant_id = $1 AND instance_id = 'urn:ngsi-ld:Instance:s1'",
    )
    .bind(t.as_str())
    .execute(&pool)
    .await
    .expect("legacy row");
    assert_eq!(s.get(&t, id).await.expect("get").expect("present"), doc);

    // Without the member a row is smaller.
    let (full, bare): (i32, i32) = sqlx::query_as(
        "SELECT pg_column_size(data || jsonb_build_object('instanceId', instance_id)),
                pg_column_size(data)
         FROM attr_instances WHERE tenant_id = $1 AND instance_id = 'urn:ngsi-ld:Instance:s3'",
    )
    .bind(t.as_str())
    .fetch_one(&pool)
    .await
    .expect("sizes");
    assert!(bare < full, "stripped {bare} B, full {full} B");
    clean(&pool, t.as_str()).await;
}

/// Windows and lastN on every timeproperty return rows whose instanceId
/// lives in the column, whole.
#[tokio::test(flavor = "multi_thread")]
async fn windows_on_created_and_modified_see_stripped_rows() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgstampsw").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:stamps2";
    let at = |m: u32| format!("2026-10-20T06:{m:02}:00.000Z");
    let speed: Vec<Value> = (1..=4)
        .map(|n| {
            json!({"type": "Property", "value": n, "observedAt": at(n),
                   "instanceId": format!("urn:ngsi-ld:Instance:w{n}"),
                   "createdAt": at(n), "modifiedAt": at(n + 10)})
        })
        .collect();
    let doc = json!({"id": id, "type": "Vehicle", SPEED: speed});
    assert!(s.create(&t, id, &doc).await.expect("create"));
    let expand = |t: &str| t.to_owned();
    let values = |v: Option<Value>| -> Vec<i64> {
        let v = v.expect("present");
        let mut out: Vec<i64> = v[SPEED]
            .as_array()
            .map(|a| a.iter().filter_map(|i| i["value"].as_i64()).collect())
            .unwrap_or_default();
        out.sort_unstable();
        out
    };

    for (tp, from, want) in [
        ("createdAt", at(3), vec![3, 4]),
        ("modifiedAt", at(12), vec![2, 3, 4]),
        ("observedAt", at(4), vec![4]),
    ] {
        let f = TemporalFilter {
            range: Some(InstanceRange {
                timerel: "after",
                time_at: &from,
                end_time_at: None,
                timeproperty: tp,
            }),
            timeproperty: tp,
            expand: &expand,
            ..TemporalFilter::default()
        };
        assert_eq!(
            values(s.get_range(&t, id, &f).await.expect("range")),
            want,
            "{tp}"
        );
    }
    let f = TemporalFilter {
        last_n: Some(2),
        timeproperty: "modifiedAt",
        expand: &expand,
        ..TemporalFilter::default()
    };
    assert_eq!(
        values(s.get_range(&t, id, &f).await.expect("lastN")),
        [3, 4]
    );
    // each instance read back whole, stamps included
    let got = s.get(&t, id).await.expect("get").expect("present");
    assert_eq!(got[SPEED], doc[SPEED]);
    clean(&pool, t.as_str()).await;
}

/// A correction keeps the replaced version whole in attr_versions, and the
/// corrected row reads back with the stamps the write stated.
#[tokio::test(flavor = "multi_thread")]
async fn corrections_keep_every_stamp() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgstampsc").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:stamps3";
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         VALUES ($1, $2, jsonb_build_object('id', $2::text), ARRAY['T'], now(), now())",
    )
    .bind(t.as_str())
    .bind(id)
    .execute(&pool)
    .await
    .expect("entity row");
    let shell = json!({"id": id, "type": "Vehicle"});
    let obs = "2026-10-20T07:00:00Z";
    let reading = |v: i64, i: &str, created: &str| {
        json!({SPEED: [{"type": "Property", "value": v, "observedAt": obs,
                        "instanceId": i, "createdAt": created, "modifiedAt": created}]})
    };
    let c1 = "2026-10-20T07:00:01.000Z";
    let c2 = "2026-10-20T07:00:02.000Z";
    s.append(&t, id, &shell, &reading(1, "urn:ngsi-ld:Instance:c1", c1))
        .await
        .expect("first");
    // Same instant, another value and instanceId: corrected in place, the
    // stored instanceId kept, the stamps the write stated.
    s.append(&t, id, &shell, &reading(2, "urn:ngsi-ld:Instance:c2", c2))
        .await
        .expect("correction");
    let got = s.get(&t, id).await.expect("get").expect("present");
    assert_eq!(
        got[SPEED],
        json!([{"type": "Property", "value": 2, "observedAt": obs,
                "instanceId": "urn:ngsi-ld:Instance:c1", "createdAt": c2, "modifiedAt": c2}])
    );
    // Same instanceId again (the upsert's conflict), its createdAt moved.
    let c3 = "2026-10-20T07:00:03.000Z";
    s.append(&t, id, &shell, &reading(3, "urn:ngsi-ld:Instance:c1", c3))
        .await
        .expect("conflict");
    let got = s.get(&t, id).await.expect("get").expect("present");
    assert_eq!(
        got[SPEED],
        json!([{"type": "Property", "value": 3, "observedAt": obs,
                "instanceId": "urn:ngsi-ld:Instance:c1", "createdAt": c3, "modifiedAt": c3}])
    );
    // Both replaced versions are whole.
    let versions = s.versions(&t, id).await.expect("versions");
    let chain = versions[SPEED].as_array().expect("chain");
    assert_eq!(chain.len(), 3, "{chain:?}");
    assert_eq!(chain[0]["createdAt"], c1);
    assert_eq!(chain[0]["instanceId"], "urn:ngsi-ld:Instance:c1");
    assert_eq!(chain[1]["createdAt"], c2);
    assert_eq!(chain[2]["createdAt"], c3);
    assert_eq!(chain[2]["modifiedAt"], c3);
    clean(&pool, t.as_str()).await;
}

/// Bytes a row takes, numeric and text separately: avg pg_column_size of
/// the whole row, of `data` as stored, and of `data` with instanceId put
/// back (the shape before T-5131).
async fn row_bytes(pool: &sqlx::PgPool, tenant: &str, numeric: bool) -> (f64, f64, f64) {
    sqlx::query_as(
        "SELECT avg(pg_column_size(a.*))::float8, avg(pg_column_size(data))::float8,
                avg(pg_column_size(data || jsonb_build_object('instanceId', instance_id)))::float8
         FROM attr_instances a
         WHERE tenant_id = $1 AND (jsonb_typeof(data -> 'value') = 'number') = $2",
    )
    .bind(tenant)
    .bind(numeric)
    .fetch_one(pool)
    .await
    .expect("row bytes")
}

/// Every kind of history value (owner 2026-10-22T08:50Z) reads back
/// byte-identical in both row shapes: instanceId only in its column
/// (T-5131) and instanceId also in `data` (rows written before). Text up to
/// 10 KB (TOAST) changed three times keeps every version.
#[tokio::test(flavor = "multi_thread")]
async fn every_kind_reads_back_byte_identical_in_both_shapes() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgstampsk").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    clean(&pool, t.as_str()).await;
    let s = PgTemporalStore::new(pool.clone());
    let id = "urn:ngsi-ld:Vehicle:stamps4";
    let ms = "2026-10-20T06:00:00.120Z";
    let at = |m: u32| format!("2026-10-20T06:{m:02}:00Z");
    let term = |t: &str| format!("https://uri.etsi.org/ngsi-ld/default-context/{t}");
    let one = |n: u32, body: Value| -> Value {
        let mut i = json!({"observedAt": at(n), "instanceId": format!("urn:ngsi-ld:Instance:k{n}"),
                           "createdAt": ms, "modifiedAt": ms});
        if let (Some(o), Some(b)) = (i.as_object_mut(), body.as_object()) {
            o.extend(b.clone());
        }
        json!([i])
    };
    let long = |c: char, n: usize| -> String { std::iter::repeat_n(c, n).collect() };
    let doc = json!({
        "id": id, "type": "Vehicle",
        SPEED: one(1, json!({"type": "Property", "value": 42.5, "unitCode": "KMH"})),
        term("note"): one(2, json!({"type": "Property", "value": "short text, ľščťžýáíé"})),
        term("manual"): one(3, json!({"type": "Property", "value": long('m', 2500)})),
        NAME: one(4, json!({"type": "LanguageProperty", "languageMap": {"sk": "auto", "en": "car"}})),
        PLACE: one(5, json!({"type": "GeoProperty",
                             "value": {"type": "Polygon", "coordinates": [[[19.1, 48.7], [19.2, 48.7], [19.2, 48.8], [19.1, 48.7]]]}})),
        OWNER: one(6, json!({"type": "Relationship", "object": "urn:ngsi-ld:Person:p1", "objectType": "Person"})),
        term("config"): one(7, json!({"type": "JsonProperty", "json": {"a": [1, {"b": null}], "c": "d"}})),
        term("category"): one(8, json!({"type": "VocabProperty", "vocab": "https://example.org/vocab#bus"})),
        term("readings"): one(9, json!({"type": "ListProperty", "valueList": [1, "two", {"three": 3}]})),
        term("status"): one(10, json!({"type": "Property", "value": "ok",
            term("reason"): {"type": "Property", "value": "checked", "createdAt": ms, "modifiedAt": ms},
            term("checkedBy"): {"type": "Relationship", "object": "urn:ngsi-ld:Person:p2",
                                "createdAt": ms, "modifiedAt": ms}})),
    });
    assert!(s.create(&t, id, &doc).await.expect("create"));

    // A 10 KB text changed three times: four versions of the attribute.
    // An append keeps history only of a live Entity.
    sqlx::query(
        "INSERT INTO entities (tenant_id, id, entity, types, created_at, modified_at)
         VALUES ($1, $2, jsonb_build_object('id', $2::text), ARRAY['T'], now(), now())",
    )
    .bind(t.as_str())
    .bind(id)
    .execute(&pool)
    .await
    .expect("entity row");
    let shell = json!({"id": id, "type": "Vehicle"});
    let text = term("description");
    let mut versions = Vec::new();
    for (n, c) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        let n = u32::try_from(n).expect("small").saturating_add(20);
        let v = json!({"type": "Property", "value": long(c, 10 * 1024), "observedAt": at(n),
                       "instanceId": format!("urn:ngsi-ld:Instance:k{n}"),
                       "createdAt": ms, "modifiedAt": ms});
        s.append(&t, id, &shell, &json!({text.clone(): [v.clone()]}))
            .await
            .expect("text change");
        versions.push(v);
    }
    let mut want = doc.clone();
    if let Some(o) = want.as_object_mut() {
        o.insert(text.clone(), Value::Array(versions));
    }
    let byte_identical = |got: &Value| {
        let mut got = got.clone();
        if let Some(arr) = got[&text].as_array_mut() {
            arr.sort_by_key(|i| i["observedAt"].as_str().map(str::to_owned));
        }
        assert_eq!(
            serde_json::to_string(&got).expect("json"),
            serde_json::to_string(&want).expect("json")
        );
    };

    // T-5131 shape: no instanceId in data.
    let rows = stored(&pool, t.as_str()).await;
    assert_eq!(rows.len(), 14);
    assert!(rows.iter().all(|r| r.1.is_none()), "{rows:?}");
    byte_identical(&s.get(&t, id).await.expect("get").expect("present"));
    let (num_row, num_data, num_old) = row_bytes(&pool, t.as_str(), true).await;
    // The text rows' third figure is detoasted (the || decompresses), so
    // only the numeric one compares the two shapes.
    let (txt_row, txt_data, _) = row_bytes(&pool, t.as_str(), false).await;
    eprintln!(
        "bytes a row: numeric row {num_row:.0}, data {num_data:.0} (with instanceId {num_old:.0}); \
         text and other kinds row {txt_row:.0}, data {txt_data:.0}"
    );
    assert!(
        num_data < num_old,
        "{num_data} B, before T-5131 {num_old} B"
    );

    // Shape before T-5131: instanceId also in data. Same answer.
    sqlx::query(
        "UPDATE attr_instances SET data = data || jsonb_build_object('instanceId', instance_id)
         WHERE tenant_id = $1",
    )
    .bind(t.as_str())
    .execute(&pool)
    .await
    .expect("old shape");
    assert!(stored(&pool, t.as_str())
        .await
        .iter()
        .all(|r| r.1.as_deref() == Some(r.0.as_str())));
    byte_identical(&s.get(&t, id).await.expect("get").expect("present"));
    clean(&pool, t.as_str()).await;
}
