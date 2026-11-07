// SPDX-License-Identifier: EUPL-1.2
//! Every history read covers both parents (T-5200, migration 0020): the
//! answers are byte-identical whether a row sits in `attr_instances` in the
//! text shape or in `attr_rows` keyed by dictionary ids and the instance uuid.
//! Nothing writes `attr_rows` before the cut-over, so the test moves the newer
//! half of one Entity's history across by hand, as the cut-over will place
//! it, and compares each read before and after. Skips without
//! ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::compile::temporal::InstanceRange;
use antares_sql::store::filter::{Aggregate, Page, TemporalFilter};
use antares_sql::store::pg;
use antares_sql::store::pg::temporal::PgTemporalStore;
use serde_json::{json, Value};

const SPEED: &str = "https://uri.etsi.org/ngsi-ld/default-context/speed";
const LEVEL: &str = "https://uri.etsi.org/ngsi-ld/default-context/level";
const MOVED_FROM: &str = "2026-01-02T00:00:00Z";

fn expand(term: &str) -> String {
    match term {
        "speed" => SPEED.to_owned(),
        "level" => LEVEL.to_owned(),
        t => t.to_owned(),
    }
}

/// Instances on two days around [`MOVED_FROM`]: the broker's own uuid ids,
/// one client id of another shape, a dataset, a correction (version 2).
fn doc(id: &str) -> Value {
    let at = |h: usize| format!("2026-01-{:02}T{:02}:00:00Z", 1 + h / 24, h % 24);
    let speed: Vec<Value> = (0..48)
        .map(|h| {
            let iid = if h == 30 {
                "urn:client:speed:thirty".to_owned()
            } else {
                format!("urn:ngsi-ld:Instance:00000000-0000-4000-8000-{h:012}")
            };
            json!({"type": "Property", "value": h * 3 % 17, "observedAt": at(h),
                   "instanceId": iid})
        })
        .collect();
    let level: Vec<Value> = (0..48)
        .step_by(4)
        .map(|h| {
            json!({"type": "Property", "value": h, "observedAt": at(h),
                   "datasetId": "urn:ngsi-ld:Dataset:tank",
                   "instanceId": format!("urn:ngsi-ld:Instance:00000000-0000-4000-9000-{h:012}")})
        })
        .collect();
    json!({"id": id, "type": "Tank",
           "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
           SPEED: speed, LEVEL: level})
}

/// Each read shape the store has, as one JSON value per shape.
async fn reads(s: &PgTemporalStore, t: &TenantId, id: &str) -> Vec<(&'static str, Value)> {
    let ids = [id];
    let attrs = [SPEED.to_owned()];
    let methods = ["avg".to_owned(), "max".to_owned(), "totalCount".to_owned()];
    let q = antares_ql::parse_q("speed>10").expect("q");
    let after = |at| {
        Some(InstanceRange {
            timerel: "after",
            time_at: at,
            end_time_at: None,
            timeproperty: "observedAt",
        })
    };
    let between = Some(InstanceRange {
        timerel: "between",
        time_at: "2026-01-01T20:00:00Z",
        end_time_at: Some("2026-01-02T04:00:00Z"),
        timeproperty: "observedAt",
    });
    let modified = Some(InstanceRange {
        timerel: "before",
        time_at: "2999-01-01T00:00:00Z",
        end_time_at: None,
        timeproperty: "modifiedAt",
    });
    let base = || TemporalFilter {
        ids: Some(&ids),
        expand: &expand,
        ..TemporalFilter::default()
    };
    let filters: Vec<(&'static str, TemporalFilter<'_>)> = vec![
        ("all", base()),
        (
            "lastN 5",
            TemporalFilter {
                last_n: Some(5),
                ..base()
            },
        ),
        (
            "lastN 30 after",
            TemporalFilter {
                last_n: Some(30),
                range: after("2026-01-01T12:00:00Z"),
                ..base()
            },
        ),
        (
            "firstN 3",
            TemporalFilter {
                last_n: Some(3),
                oldest_first: true,
                ..base()
            },
        ),
        (
            "between",
            TemporalFilter {
                range: between,
                ..base()
            },
        ),
        (
            "modifiedAt",
            TemporalFilter {
                range: modified,
                timeproperty: "modifiedAt",
                ..base()
            },
        ),
        (
            "attrs + page",
            TemporalFilter {
                attrs: Some(&attrs),
                page: Some(Page {
                    offset: 0,
                    limit: 10,
                    count: true,
                }),
                range: after("2026-01-01T00:00:00Z"),
                ..base()
            },
        ),
        (
            "q",
            TemporalFilter {
                q: Some(&q),
                range: after("2026-01-01T00:00:00Z"),
                ..base()
            },
        ),
        (
            "aggregate",
            TemporalFilter {
                range: after("2026-01-01T00:00:00Z"),
                aggregate: Some(Aggregate {
                    methods: &methods,
                    period_secs: Some(21_600),
                    anchor: Some("2026-01-01T00:00:00Z"),
                }),
                ..base()
            },
        ),
    ];
    let mut out = Vec::new();
    for (name, f) in &filters {
        let o = s.query(t, f).await.expect(name);
        out.push((
            *name,
            json!({"rows": o.rows, "total": o.total, "agg": o.aggregated}),
        ));
        let one = s.get_range(t, id, f).await.expect(name);
        out.push((*name, json!({"one": one})));
    }
    out.push(("get", json!(s.get(t, id).await.expect("get"))));
    out.push((
        "versions",
        json!(s.versions(t, id).await.expect("versions")),
    ));
    out
}

/// Moves the rows of `id` observed from [`MOVED_FROM`] on into `attr_rows`,
/// as the cut-over places them: ids from the Tenant's dictionary, the broker's
/// uuid as the key, a client's other id kept in `data` with a v5-like key.
async fn move_newer_half(pool: &sqlx::PgPool, t: &TenantId, id: &str) -> u64 {
    let mut tx = pool.begin().await.expect("tx");
    pg::set_tenant(&mut tx, t).await.expect("tenant");
    sqlx::query(
        "INSERT INTO iri_ids (tenant_id, iri)
         SELECT DISTINCT $1, unnest(ARRAY[entity_id, attr_id]) FROM attr_instances
          WHERE tenant_id = $1 AND entity_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(t.as_str())
    .bind(id)
    .execute(&mut *tx)
    .await
    .expect("dictionary");
    let moved = sqlx::query(
        "WITH gone AS (DELETE FROM attr_instances
                        WHERE tenant_id = $1 AND entity_id = $2 AND observed_at >= $3::timestamptz
                        RETURNING *)
         INSERT INTO attr_rows (tenant_id, entity_no, attr_no, instance_no, dataset_id,
                                observed_at, created_at, modified_at, deleted_at, data,
                                geo_value, version, provenance)
         SELECT g.tenant_id, iri_no(g.tenant_id, g.entity_id), iri_no(g.tenant_id, g.attr_id),
                CASE WHEN g.instance_id LIKE 'urn:ngsi-ld:Instance:%'
                     THEN substr(g.instance_id, 22)::uuid ELSE md5(g.instance_id)::uuid END,
                g.dataset_id, g.observed_at, g.created_at, g.modified_at, g.deleted_at,
                CASE WHEN g.instance_id LIKE 'urn:ngsi-ld:Instance:%' THEN g.data
                     ELSE g.data || jsonb_build_object('instanceId', g.instance_id) END,
                g.geo_value, g.version, g.provenance
           FROM gone g",
    )
    .bind(t.as_str())
    .bind(id)
    .bind(MOVED_FROM)
    .execute(&mut *tx)
    .await
    .expect("move")
    .rows_affected();
    tx.commit().await.expect("commit");
    moved
}

async fn keyed_rows(pool: &sqlx::PgPool, t: &TenantId) -> i64 {
    let mut tx = pool.begin().await.expect("tx");
    pg::set_tenant(&mut tx, t).await.expect("tenant");
    sqlx::query_scalar("SELECT count(*) FROM attr_rows WHERE tenant_id = $1")
        .bind(t.as_str())
        .fetch_one(&mut *tx)
        .await
        .expect("count")
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_over_both_parents_answer_byte_identically() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pgkeyed").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Tank:keyed1";
    let _ = s.delete(&t, id).await;
    assert!(s.create(&t, id, &doc(id)).await.expect("create"));
    // a correction keeps version 1 in attr_versions and makes the row version 2
    let fix = json!({SPEED: [{"type": "Property", "value": 99,
        "observedAt": "2026-01-02T06:00:00Z",
        "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-8000-000000000030"}]});
    s.append(&t, id, &doc(id), &fix).await.expect("correct");
    assert_eq!(
        keyed_rows(&pool, &t).await,
        0,
        "nothing writes attr_rows before the cut-over"
    );

    let before = reads(&s, &t, id).await;
    let moved = move_newer_half(&pool, &t, id).await;
    assert!(moved >= 30, "moved {moved} rows");
    let after = reads(&s, &t, id).await;
    for ((name, b), (_, a)) in before.iter().zip(&after) {
        assert_eq!(
            serde_json::to_string(b).expect("json"),
            serde_json::to_string(a).expect("json"),
            "{name}: the answer changed once rows sit in attr_rows"
        );
    }

    // deletes reach both parents: an erase of the older half, then the rest
    let erased = s
        .erase(
            &t,
            id,
            Some(&[LEVEL.to_owned()]),
            Some("2026-01-03T00:00:00Z"),
        )
        .await
        .expect("erase");
    assert_eq!(erased, 12, "every level row of both parents");
    assert!(s.delete(&t, id).await.expect("delete"));
    assert_eq!(keyed_rows(&pool, &t).await, 0, "the delete left keyed rows");
}

/// `v` as JSON text without `supersededAt`, the wall clock of each
/// correction and the one member two stores cannot share.
fn without_clock(v: &Value) -> String {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("supersededAt");
                o.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = v.clone();
    strip(&mut v);
    v.to_string()
}

async fn count(pool: &sqlx::PgPool, t: &TenantId, sql: &'static str) -> i64 {
    let mut tx = pool.begin().await.expect("tx");
    pg::set_tenant(&mut tx, t).await.expect("tenant");
    sqlx::query_scalar(sql)
        .bind(t.as_str())
        .fetch_one(&mut *tx)
        .await
        .expect(sql)
}

/// The writes one Entity's history takes: create, a correction, a repeated
/// value (dedup), a new value, an instance update (mutate), a late write.
async fn write_history(s: &PgTemporalStore, t: &TenantId, id: &str) {
    let _ = s.delete(t, id).await;
    assert!(s.create(t, id, &doc(id)).await.expect("create"));
    let at = |h: usize| format!("2026-01-{:02}T{:02}:00:00Z", 1 + h / 24, h % 24);
    let appends = [
        // same observedAt as the client's id at h 30, another value: a correction
        json!({SPEED: [{"type": "Property", "value": 99, "observedAt": at(30),
            "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-8000-0000000000aa"}]}),
        // the newest level again without observedAt: no new observation
        json!({LEVEL: [{"type": "Property", "value": 44, "datasetId": "urn:ngsi-ld:Dataset:tank",
            "modifiedAt": "2026-01-02T23:30:00Z",
            "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-a000-000000000001"}]}),
        // a new value without observedAt
        json!({LEVEL: [{"type": "Property", "value": 45, "datasetId": "urn:ngsi-ld:Dataset:tank",
            "modifiedAt": "2026-01-02T23:40:00Z",
            "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-a000-000000000002"}]}),
        // a late write: observed before the cut-over
        json!({SPEED: [{"type": "Property", "value": 5, "observedAt": "2026-01-01T05:30:00Z",
            "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-a000-000000000003"}]}),
    ];
    for a in &appends {
        s.append(t, id, &doc(id), a).await.expect("append");
    }
    // 5.6.14 on a keyed instance and on the client's one
    let changed = s
        .mutate(t, id, |d| {
            let Some(speed) = d.get_mut(SPEED).and_then(Value::as_array_mut) else {
                return Err("no speed");
            };
            for i in speed.iter_mut() {
                let h40 = i["observedAt"] == json!(at(40));
                if h40 || i["instanceId"] == json!("urn:client:speed:thirty") {
                    i["value"] = json!(1000);
                }
            }
            Ok(())
        })
        .await
        .expect("mutate");
    assert_eq!(changed, Some(Ok(())));
}

/// T-5243: a store with a cut-over writes the rows observed from it on to
/// `attr_rows` and every earlier one, a late write included, to
/// `attr_instances`; each read answers byte-identically to a store that
/// writes the text shape only, and a client's instanceId comes back as sent.
#[tokio::test(flavor = "multi_thread")]
async fn writes_route_by_the_cut_over_and_read_as_the_text_shape() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let cut = chrono::DateTime::parse_from_rfc3339(MOVED_FROM)
        .expect("stamp")
        .to_utc();
    let mut text = PgTemporalStore::with_cutover(pool.clone(), None);
    let mut keyed = PgTemporalStore::with_cutover(pool.clone(), Some(cut));
    // the appends need no current Entity in `entities`
    text.temporal_only = true;
    keyed.temporal_only = true;
    let (tt, tk) = (
        TenantId::new("pgcuttext").expect("tenant"),
        TenantId::new("pgcutkeyed").expect("tenant"),
    );
    for t in [&tt, &tk] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
    }
    let id = "urn:ngsi-ld:Tank:cut1";
    write_history(&text, &tt, id).await;
    write_history(&keyed, &tk, id).await;

    let rows = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1";
    let old = "SELECT count(*) FROM attr_instances WHERE tenant_id = $1";
    let late = "SELECT count(*) FROM attr_instances WHERE tenant_id = $1
                 AND observed_at = '2026-01-01T05:30:00Z'";
    let newer = "SELECT count(*) FROM attr_instances WHERE tenant_id = $1
                  AND observed_at >= '2026-01-02T00:00:00Z'";
    assert_eq!(
        count(&pool, &tt, rows).await,
        0,
        "no cut-over, no keyed row"
    );
    let moved = count(&pool, &tt, newer).await;
    assert!(moved >= 30, "speed h 24..47 and level h 24..44: {moved}");
    assert_eq!(
        count(&pool, &tk, rows).await,
        moved,
        "rows from the cut-over on"
    );
    assert_eq!(
        count(&pool, &tk, newer).await,
        0,
        "no newer row in the text shape"
    );
    assert_eq!(
        count(&pool, &tk, old).await + moved,
        count(&pool, &tt, old).await,
        "one row each, in one parent"
    );
    assert_eq!(
        count(&pool, &tk, late).await,
        1,
        "the late write stays in the text shape"
    );
    let foreign = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND data ? 'instanceId'";
    assert_eq!(
        count(&pool, &tk, foreign).await,
        1,
        "only the client's id rides in data"
    );

    let (a, b) = (reads(&text, &tt, id).await, reads(&keyed, &tk, id).await);
    for ((name, x), (_, y)) in a.iter().zip(&b) {
        assert_eq!(
            without_clock(x),
            without_clock(y),
            "{name}: the keyed store answered otherwise"
        );
    }
    let got = keyed.get(&tk, id).await.expect("get").expect("entity");
    let ids: Vec<&str> = got[SPEED]
        .as_array()
        .expect("speed")
        .iter()
        .filter_map(|i| i["instanceId"].as_str())
        .collect();
    assert!(
        ids.contains(&"urn:client:speed:thirty"),
        "the client's id came back: {ids:?}"
    );

    for (s, t) in [(&text, &tt), (&keyed, &tk)] {
        assert!(s.delete(t, id).await.expect("delete"));
    }
    assert_eq!(
        count(&pool, &tk, rows).await,
        0,
        "the delete left keyed rows"
    );
}

/// T-5243: a batch append for an Entity absent from `entities` writes
/// nothing, its dictionary first sights included: no `iri_ids` row names
/// an IRI only that skipped append carried.
#[tokio::test(flavor = "multi_thread")]
async fn a_skipped_append_leaves_no_dictionary_row() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let cut = chrono::DateTime::parse_from_rfc3339(MOVED_FROM)
        .expect("stamp")
        .to_utc();
    let keyed = PgTemporalStore::with_cutover(pool.clone(), Some(cut));
    let t = TenantId::new("pgorphan").expect("tenant");
    let id = "urn:ngsi-ld:Tank:t5243-absent";
    let probe = "https://example.org/t5243/orphanProbe";
    let shell = json!({"id": id, "type": "Tank",
        "createdAt": "2026-01-03T00:00:00Z", "modifiedAt": "2026-01-03T00:00:00Z"});
    let additions = json!({probe: [{"type": "Property", "value": 1,
        "observedAt": "2026-01-03T00:00:00Z",
        "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-b000-000000000002"}]});
    let mut tx = pool.begin().await.expect("tx");
    pg::set_tenant(&mut tx, &t).await.expect("tenant");
    sqlx::query("DELETE FROM iri_ids WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&mut *tx)
        .await
        .expect("clean");
    tx.commit().await.expect("commit");
    keyed
        .append_many(&[(&t, id, &shell, additions)])
        .await
        .expect("append_many");
    let rows = count(
        &pool,
        &t,
        "SELECT count(*) FROM iri_ids WHERE tenant_id = $1",
    )
    .await;
    assert_eq!(rows, 0);
}

fn short_for(t: &TenantId, short: bool) -> antares_store::ShortHistory {
    let mut s = antares_store::ShortHistory::default();
    s.types
        .insert((t.as_str().to_owned(), "Tank".to_owned()), short);
    s
}

/// T-5249: a type whose retention is PT1H or less writes every history row
/// as `short` (plain mode: the unlogged `attr_rows_short`), before any cut-over, and each read answers
/// as the text shape does. A rule grown past PT1H sends new rows to the logged
/// side (`attr_rows_long`), reads merge both children in order, and the retention
/// sweep deletes the short rows older than its cut.
#[tokio::test(flavor = "multi_thread")]
async fn short_history_lands_unlogged_and_reads_as_the_text_shape() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let mut text = PgTemporalStore::with_cutover(pool.clone(), None);
    // a cut-over before every row: short rows ignore it, long ones key by it
    let cut = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("stamp")
        .to_utc();
    let mut short = PgTemporalStore::with_cutover(pool.clone(), Some(cut));
    text.temporal_only = true;
    short.temporal_only = true;
    let (tt, ts) = (
        TenantId::new("pgshorttext").expect("tenant"),
        TenantId::new("pgshort").expect("tenant"),
    );
    for t in [&tt, &ts] {
        pg::ensure_tenant(&pool, t).await.expect("tenant row");
    }
    short.set_short_history(short_for(&ts, true));
    let id = "urn:ngsi-ld:Tank:short1";
    write_history(&text, &tt, id).await;
    write_history(&short, &ts, id).await;

    // `short` is the partition key of the two children (plain mode); under
    // timescale the same column of the one hypertable
    let unlogged = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND short";
    let logged = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND NOT short";
    let old = "SELECT count(*) FROM attr_instances WHERE tenant_id = $1";
    let all = count(&pool, &tt, old).await;
    assert!(all >= 60, "speed and level rows: {all}");
    assert_eq!(count(&pool, &ts, unlogged).await, all, "every row is short");
    assert_eq!(count(&pool, &ts, old).await, 0, "no row in the text shape");
    assert_eq!(count(&pool, &ts, logged).await, 0, "no row logged");
    let (a, b) = (reads(&text, &tt, id).await, reads(&short, &ts, id).await);
    for ((name, x), (_, y)) in a.iter().zip(&b) {
        assert_eq!(
            without_clock(x),
            without_clock(y),
            "{name}: the short store answered otherwise"
        );
    }

    // P30D now: a new row goes to the logged child, reads merge both
    short.set_short_history(short_for(&ts, false));
    let newest = json!({SPEED: [{"type": "Property", "value": 77,
        "observedAt": "2026-01-03T00:00:00Z",
        "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-a000-000000000077"}]});
    short
        .append(&ts, id, &doc(id), &newest)
        .await
        .expect("append");
    assert_eq!(count(&pool, &ts, logged).await, 1, "the long rule logs");
    let got = short.get(&ts, id).await.expect("get").expect("entity");
    let at: Vec<&str> = got[SPEED]
        .as_array()
        .expect("speed")
        .iter()
        .filter_map(|i| i["observedAt"].as_str())
        .collect();
    let mut sorted = at.clone();
    sorted.sort_unstable();
    assert_eq!(at, sorted, "one order over both children");
    assert_eq!(at.last(), Some(&"2026-01-03T00:00:00Z"), "the logged row");

    let swept = pg::maintenance::retain_history(
        &pool,
        ts.as_str(),
        antares_store::HistoryScope::Type {
            type_iri: "Tank",
            except_ids: &[],
        },
        MOVED_FROM,
        10_000,
        false,
    )
    .await
    .expect("sweep");
    let older = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND short
                  AND observed_at < '2026-01-02T00:00:00Z'";
    assert!(swept >= 20, "the rows before the cut: {swept}");
    assert_eq!(
        count(&pool, &ts, older).await,
        0,
        "a short row outlived its cut"
    );
    assert!(
        count(&pool, &ts, unlogged).await > 0,
        "the newer short rows stay"
    );
    for (s, t) in [(&text, &tt), (&short, &ts)] {
        assert!(s.delete(t, id).await.expect("delete"));
    }
    assert_eq!(
        count(&pool, &ts, unlogged).await,
        0,
        "the delete left short rows"
    );
}

/// T-5249: a burst of 10 reports of one vehicle within a second records 10
/// short rows; the same 10 sent again (new instance ids, same observedAt and
/// value) record none.
#[tokio::test(flavor = "multi_thread")]
async fn a_short_burst_keeps_every_report_and_no_repeat() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let mut s = PgTemporalStore::with_cutover(pool.clone(), None);
    s.temporal_only = true;
    let t = TenantId::new("pgshortburst").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    s.set_short_history(short_for(&t, true));
    let id = "urn:ngsi-ld:Tank:burst1";
    let _ = s.delete(&t, id).await;
    let shell = json!({"id": id, "type": "Tank",
        "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z"});
    assert!(s.create(&t, id, &shell).await.expect("create"));
    let report = |n: usize, round: usize| {
        json!({SPEED: [{"type": "Property", "value": n,
            "observedAt": format!("2026-01-01T00:00:00.{:03}Z", n * 100),
            "instanceId": format!("urn:ngsi-ld:Instance:00000000-0000-4000-b{round:03}-{n:012}")}]})
    };
    for round in 0..2 {
        for n in 0..10 {
            s.append(&t, id, &shell, &report(n, round))
                .await
                .expect("append");
        }
    }
    let rows = "SELECT count(*) FROM attr_rows WHERE tenant_id = $1 AND short";
    assert_eq!(count(&pool, &t, rows).await, 10, "10 reports, no repeat");
    assert!(s.delete(&t, id).await.expect("delete"));
}

/// F-ANT1-013: a store that started before the migration setting the
/// cut-over (a pod during a rolling deploy) routes by it once it is set,
/// instead of keeping the unset value it read first. Runs in its own
/// database, since it moves the one `history_cutover` row.
#[tokio::test(flavor = "multi_thread")]
async fn a_cut_over_set_after_the_first_write_routes_the_next_one() {
    let Ok(shared) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let admin = pg::connect(&shared, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_cutover_{run}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("own database");
    let (base, _) = shared.rsplit_once('/').expect("a database URL");
    let pool = pg::connect(&format!("{base}/{name}"), 3)
        .await
        .expect("connect+migrate the own database");
    let mut s = PgTemporalStore::new(pool.clone());
    s.temporal_only = true;
    let t = TenantId::new("pgcutlate").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Tank:cutlate";
    let first = json!({"id": id, "type": "Tank",
        "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
        SPEED: [{"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z",
                 "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-b000-000000000001"}]});
    assert!(s.create(&t, id, &first).await.expect("create"));
    sqlx::query("UPDATE history_cutover SET at = $1::timestamptz")
        .bind(MOVED_FROM)
        .execute(&pool)
        .await
        .expect("set the cut-over");
    let later = json!({SPEED: [{"type": "Property", "value": 2,
        "observedAt": "2026-01-03T00:00:00Z",
        "instanceId": "urn:ngsi-ld:Instance:00000000-0000-4000-b000-000000000002"}]});
    s.append(&t, id, &first, &later).await.expect("append");
    let rows = count(
        &pool,
        &t,
        "SELECT count(*) FROM attr_rows WHERE tenant_id = $1",
    )
    .await;
    let text = count(
        &pool,
        &t,
        "SELECT count(*) FROM attr_instances WHERE tenant_id = $1",
    )
    .await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("drop own database");
    assert_eq!(
        (rows, text),
        (1, 1),
        "the observation after the cut-over is keyed, the first one is not"
    );
}

/// Two versions kept at one instant (platform ADR-N-054 2.9: two edits in the
/// same millisecond) read in one order, by instance id, whichever was written
/// first.
#[tokio::test]
async fn versions_of_one_instant_read_in_instance_id_order() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pgsameinstant").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Station:same-instant";
    let _ = s.delete(&t, id).await;
    let name = "https://uri.etsi.org/ngsi-ld/default-context/name";
    let version = |value: &str, n: u8| {
        json!({"type": "Property", "value": value, "observedAt": "2026-01-02T06:00:00.123Z",
               "instanceId": format!("urn:ngsi-ld:Instance:00000000-0000-4000-8000-00000000004{n}")})
    };
    let doc =
        json!({"id": id, "type": "Station", name: [version("second", 2), version("first", 1)]});
    assert!(s.create(&t, id, &doc).await.expect("create"));
    let all = s.versions(&t, id).await.expect("versions");
    let values: Vec<Value> = all
        .get(name)
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(|r| r["value"].clone()).collect())
        .unwrap_or_default();
    assert_eq!(values, [json!("first"), json!("second")], "{all:?}");
}
