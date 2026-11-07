// SPDX-License-Identifier: EUPL-1.2
//! lastN read through each Attribute's newest rows (T-4246): the store keeps
//! exactly what ranking the whole window keeps. The oracle here ranks the
//! window's instances by the same canonical text key the SQL rank orders on;
//! the data put offset stamps where the parsed column and that key disagree,
//! and a sparse dataset below the Attribute's N-th newest. Skips loudly
//! without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use std::collections::BTreeSet;

use antares_model::TenantId;
use antares_sql::compile::temporal::InstanceRange;
use antares_sql::store::filter::TemporalFilter;
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
const LEVEL: &str = "https://uri.etsi.org/ngsi-ld/default-context/level";

/// `dt_key_sql` for whole-second stamps: a `Z` stamp keys as its first 19
/// characters and a zero fraction, any other as it stands.
fn key(stamp: &str) -> String {
    if stamp.ends_with('Z') && stamp.len() >= 20 {
        format!("{}.000000", &stamp[..19])
    } else {
        stamp.to_owned()
    }
}

fn inst(n: usize, at: &str, dataset: Option<&str>) -> Value {
    let mut i = json!({"type": "Property", "value": n, "observedAt": at,
                       "instanceId": format!("urn:ngsi-ld:Instance:{n}")});
    if let Some(d) = dataset {
        i["datasetId"] = json!(d);
    }
    i
}

/// The instances ranking the window would keep: per datasetId, every
/// instance fewer than `n` others outrank (SQL `rank() <= n`).
fn oracle(instances: &[Value], after: Option<&str>, n: i64) -> BTreeSet<String> {
    let shown: Vec<&Value> = instances
        .iter()
        .filter(|i| after.is_none_or(|a| key(i["observedAt"].as_str().unwrap_or("")) >= key(a)))
        .collect();
    shown
        .iter()
        .filter(|i| {
            let k = key(i["observedAt"].as_str().unwrap_or(""));
            let above = shown
                .iter()
                .filter(|o| o["datasetId"] == i["datasetId"])
                .filter(|o| key(o["observedAt"].as_str().unwrap_or("")) > k)
                .count();
            (above as i64) < n
        })
        .map(|i| i["instanceId"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn ids(rows: &Value) -> BTreeSet<String> {
    rows.as_array()
        .map(|a| {
            a.iter()
                .map(|i| i["instanceId"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn last_n_keeps_what_ranking_the_whole_window_keeps() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pglastn").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Vehicle:lastn1";
    let _ = s.delete(&t, id).await;

    // speed: 200 hourly Z instances up to 2026-01-09T07:00Z, plus offset
    // stamps the column and the key order differently: +14:00 keys newest
    // and is 14 h older; -12:00 keys older than it is.
    let mut speed: Vec<Value> = (0..200)
        .map(|h| {
            let at = chrono_free(h);
            inst(h, &at, None)
        })
        .collect();
    speed.push(inst(1000, "2026-01-09T20:00:00+14:00", None));
    speed.push(inst(1001, "2026-01-08T20:00:00-12:00", None));
    speed.push(inst(1002, "2026-01-09T07:00:00+00:00", None));
    // level: a busy default and a dataset whose 3 instances are days older
    // than the default's newest, so the Attribute's N-th newest is above them.
    let mut level: Vec<Value> = (0..100)
        .map(|h| inst(2000 + h, &chrono_free(h + 100), None))
        .collect();
    for (j, at) in [
        "2026-01-02T00:00:00Z",
        "2026-01-02T01:00:00Z",
        "2026-01-02T02:00:00Z",
    ]
    .iter()
    .enumerate()
    {
        level.push(inst(3000 + j, at, Some("urn:ngsi-ld:Dataset:sparse")));
    }
    let doc = json!({
        "id": id, "type": "Vehicle",
        "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
        SPEED: speed, LEVEL: level,
    });
    assert!(s.create(&t, id, &doc).await.expect("create"));
    // The write records the Attribute with dataset instances (migration 0009),
    // which is what keeps the sparse dataset below from being cut.
    let listed: Vec<String> = sqlx::query_scalar(
        "SELECT unnest(dataset_attrs) FROM temporal_entities WHERE tenant_id = $1 AND id = $2",
    )
    .bind(t.as_str())
    .bind(id)
    .fetch_all(&pool)
    .await
    .expect("dataset_attrs");
    assert_eq!(listed, vec![LEVEL.to_owned()]);

    let expand = |t: &str| t.to_owned();
    for (after, n) in [
        (None, 5),
        (None, 1),
        (Some("2026-01-05T00:00:00Z"), 5),
        (Some("2026-01-09T06:00:00Z"), 3),
        (None, 400),
    ] {
        let range = after.map(|a| InstanceRange {
            timerel: "after",
            time_at: a,
            end_time_at: None,
            timeproperty: "observedAt",
        });
        let f = TemporalFilter {
            ids: Some(&[id]),
            range,
            last_n: Some(n),
            expand: &expand,
            ..TemporalFilter::default()
        };
        let out = s.query(&t, &f).await.expect("query");
        assert_eq!(out.rows.len(), 1, "{after:?} {n}");
        let row = &out.rows[0];
        assert_eq!(
            ids(&row[SPEED]),
            oracle(&doc[SPEED].as_array().expect("speed")[..], after, n),
            "speed {after:?} lastN={n}"
        );
        assert_eq!(
            ids(&row[LEVEL]),
            oracle(&doc[LEVEL].as_array().expect("level")[..], after, n),
            "level {after:?} lastN={n}"
        );
    }
    // the dataset's instances are kept by its own rank, not cut by the default's
    let f = TemporalFilter {
        ids: Some(&[id]),
        last_n: Some(2),
        expand: &expand,
        ..TemporalFilter::default()
    };
    let out = s.query(&t, &f).await.expect("query");
    assert!(
        ids(&out.rows[0][LEVEL]).contains("urn:ngsi-ld:Instance:3002"),
        "{}",
        out.rows[0][LEVEL]
    );

    assert!(s.delete(&t, id).await.expect("delete"));
}

/// T-5616: the write keeps `offset_until` (migration 0028), the newest
/// instance whose observedAt the rank may order unlike `observed_at`, and
/// lastN stays exact both where the read takes the N newest alone (every row
/// within 28 h of the N-th newest a UTC stamp) and where an offset stamp
/// inside those 28 h keeps the wider read.
#[tokio::test(flavor = "multi_thread")]
async fn last_n_reads_the_newest_rows_alone_after_the_last_offset_stamp() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pglastn").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Vehicle:lastn-utc";
    let _ = s.delete(&t, id).await;
    let offset_until = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT to_char(offset_until AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM temporal_entities WHERE tenant_id = $1 AND id = $2",
        )
        .bind(t.as_str())
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("offset_until")
    };

    // 200 hourly UTC stamps, one with a fraction, and an old offset stamp
    // more than 28 h before any N-th newest asked below
    let mut speed: Vec<Value> = (0..200).map(|h| inst(h, &chrono_free(h), None)).collect();
    speed.push(inst(500, "2026-01-09T06:30:00.5Z", None));
    speed.push(inst(501, "2026-01-01T12:00:00+14:00", None));
    let doc = json!({
        "id": id, "type": "Vehicle",
        "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
        SPEED: speed,
    });
    assert!(s.create(&t, id, &doc).await.expect("create"));
    assert_eq!(offset_until().await, "2025-12-31 22:00:00");

    let expand = |t: &str| t.to_owned();
    let check = |doc: Value, cases: Vec<(Option<&'static str>, i64)>| {
        let s = &s;
        let t = &t;
        let expand = &expand;
        async move {
            for (after, n) in cases {
                let f = TemporalFilter {
                    ids: Some(&[id]),
                    range: after.map(|a| InstanceRange {
                        timerel: "after",
                        time_at: a,
                        end_time_at: None,
                        timeproperty: "observedAt",
                    }),
                    last_n: Some(n),
                    expand,
                    ..TemporalFilter::default()
                };
                let out = s.query(t, &f).await.expect("query");
                assert_eq!(
                    ids(&out.rows[0][SPEED]),
                    oracle(&doc[SPEED].as_array().expect("speed")[..], after, n),
                    "{after:?} lastN={n}"
                );
            }
        }
    };
    let cases = vec![
        (None, 1),
        (None, 3),
        (Some("2026-01-09T00:00:00Z"), 10),
        (None, 199),
    ];
    check(doc.clone(), cases.clone()).await;

    // an offset stamp keys newest yet is the oldest instant of the last day:
    // the read keeps 28 h and the rank still ranks it first
    let mut doc = doc;
    doc[SPEED]
        .as_array_mut()
        .expect("speed")
        .push(inst(502, "2026-01-09T20:00:00+14:00", None));
    assert!(s.delete(&t, id).await.expect("delete"));
    assert!(s.create(&t, id, &doc).await.expect("create"));
    assert_eq!(offset_until().await, "2026-01-09 06:00:00");
    check(doc, cases).await;

    assert!(s.delete(&t, id).await.expect("delete"));
}

/// T-5616: under a policy the page qualifies an Entity only through the
/// Attributes its rules show: one whose window holds only a hidden
/// Attribute is neither on the page nor in the count.
#[tokio::test(flavor = "multi_thread")]
async fn the_page_qualifies_through_the_shown_attributes_only() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pglastn").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let (shown, hidden) = (
        "urn:ngsi-ld:Station:page-shown",
        "urn:ngsi-ld:Station:page-hidden",
    );
    for (id, speed_at) in [
        (shown, "2026-01-09T00:00:00Z"),
        (hidden, "2026-01-01T00:00:00Z"),
    ] {
        let _ = s.delete(&t, id).await;
        let doc = json!({
            "id": id, "type": "Station",
            "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
            SPEED: [inst(1, speed_at, None)],
            LEVEL: [inst(2, "2026-01-09T00:00:00Z", None)],
        });
        assert!(s.create(&t, id, &doc).await.expect("create"));
    }
    let expand = |t: &str| t.to_owned();
    let types = ["Station".to_owned()];
    let only_speed = [SPEED.to_owned()];
    for (page_attrs, want) in [
        (None, vec![hidden, shown]),
        (Some(&only_speed[..]), vec![shown]),
    ] {
        let f = TemporalFilter {
            types: Some(&types),
            range: Some(InstanceRange {
                timerel: "after",
                time_at: "2026-01-05T00:00:00Z",
                end_time_at: None,
                timeproperty: "observedAt",
            }),
            page: Some(antares_store::filter::Page {
                offset: 0,
                limit: 10,
                count: true,
            }),
            page_attrs,
            expand: &expand,
            ..TemporalFilter::default()
        };
        let out = s.query(&t, &f).await.expect("query");
        let got: Vec<&str> = out.rows.iter().filter_map(|r| r["id"].as_str()).collect();
        assert_eq!(got, want, "{page_attrs:?}");
        assert!(out.paged, "{page_attrs:?}");
        assert_eq!(out.total, i64::try_from(want.len()).ok(), "{page_attrs:?}");
    }
    for id in [shown, hidden] {
        assert!(s.delete(&t, id).await.expect("delete"));
    }
}

/// `2026-01-01T00:00:00Z` plus `h` hours, without a date library.
fn chrono_free(h: usize) -> String {
    format!("2026-01-{:02}T{:02}:00:00Z", 1 + h / 24, h % 24)
}

/// T-4310: no migration builds an index over the history it would have to read
/// in full. A partial index on `attr_instances` (the first 0009) reads every
/// partition under a lock that blocks INSERT; on dev that was ~145 GB.
#[tokio::test(flavor = "multi_thread")]
async fn no_migration_builds_a_partial_index_over_history() {
    let url = require_db!();
    let pool = pg::connect(&url, 2).await.expect("connect");
    let partial: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         WHERE i.indrelid = 'attr_instances'::regclass AND i.indpred IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("indexes");
    assert!(
        partial.is_empty(),
        "partial indexes on attr_instances: {partial:?}"
    );
}

/// T-5835: an Entity with no history row is marked `bare` by the sweep and
/// the page qualification skips it without a probe (19 782 history-less
/// KeyPerformanceIndicators passed the 30 s statement timeout, each probe
/// reading every weekly partition); an instance written later clears the mark
/// and the Entity is on the page again.
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_entity_is_skipped_by_the_page_until_it_gains_history() {
    use antares_sql::store::pg::maintenance::mark_bare;
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let mut s = PgTemporalStore::new(pool.clone());
    s.temporal_only = true;
    let t = TenantId::new("pgbare").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let kept = "urn:ngsi-ld:Kpi:bare-kept";
    let bare: Vec<String> = (0..3)
        .map(|n| format!("urn:ngsi-ld:Kpi:bare-{n}"))
        .collect();
    let _ = s.delete(&t, kept).await;
    for id in &bare {
        let _ = s.delete(&t, id).await;
    }
    let shell = |id: &str| {
        json!({"id": id, "type": "Kpi",
               "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z"})
    };
    let mut doc = shell(kept);
    doc[SPEED] = json!([inst(1, "2026-01-09T00:00:00Z", None)]);
    assert!(s.create(&t, kept, &doc).await.expect("create"));
    for id in &bare {
        assert!(s.create(&t, id, &shell(id)).await.expect("create"));
    }
    // a pass from wherever another call left the cursor, then a whole one
    for _ in 0..2 {
        while !mark_bare(&pool, std::time::Duration::from_secs(60))
            .await
            .expect("sweep")
            .wrapped
        {}
    }
    let marks = || async {
        let mut tx = pool.begin().await.expect("tx");
        sqlx::query(antares_sql::SET_TENANT_SQL)
            .bind(t.as_str())
            .execute(&mut *tx)
            .await
            .expect("tenant");
        let rows: Vec<(String, bool)> = sqlx::query_as(
            "SELECT id, bare FROM temporal_entities WHERE tenant_id = $1 ORDER BY id",
        )
        .bind(t.as_str())
        .fetch_all(&mut *tx)
        .await
        .expect("marks");
        rows
    };
    let mut want: Vec<(String, bool)> = bare.iter().map(|id| (id.clone(), true)).collect();
    want.push((kept.to_owned(), false));
    want.sort();
    assert_eq!(marks().await, want);

    // the qualification probes the one Entity that is not bare
    let qual = pg::temporal::has_history("", "DESC", None);
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query(antares_sql::SET_TENANT_SQL)
        .bind(t.as_str())
        .execute(&mut *tx)
        .await
        .expect("tenant");
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, FORMAT TEXT) \
         SELECT count(*) FROM temporal_entities m WHERE m.tenant_id = 'pgbare' AND {qual}"
    )))
    .fetch_all(&mut *tx)
    .await
    .expect("explain");
    tx.rollback().await.expect("rollback");
    let plan = plan.join("\n");
    let probes: Vec<&str> = plan
        .lines()
        .filter(|l| l.contains("Limit (actual rows="))
        .collect();
    assert!(!probes.is_empty(), "{plan}");
    assert!(
        probes.iter().all(|l| l.contains("loops=1)")),
        "each probe runs for the one Entity with history only — plan:\n{plan}"
    );

    let types = ["Kpi".to_owned()];
    let expand = |t: &str| t.to_owned();
    let page = |types| TemporalFilter {
        types: Some(types),
        range: Some(InstanceRange {
            timerel: "after",
            time_at: "2026-01-05T00:00:00Z",
            end_time_at: None,
            timeproperty: "observedAt",
        }),
        page: Some(antares_store::filter::Page {
            offset: 0,
            limit: 10,
            count: true,
        }),
        expand: &expand,
        ..TemporalFilter::default()
    };
    let ids = |out: &antares_store::filter::TemporalOutcome| -> Vec<String> {
        out.rows
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect()
    };
    let out = s.query(&t, &page(&types)).await.expect("query");
    assert_eq!(ids(&out), vec![kept.to_owned()]);

    // a bare Entity that gains history is on the page again
    let first = &bare[0];
    s.append(
        &t,
        first,
        &shell(first),
        &json!({SPEED: [inst(2, "2026-01-09T00:00:00Z", None)]}),
    )
    .await
    .expect("append");
    let out = s.query(&t, &page(&types)).await.expect("query");
    assert_eq!(ids(&out), vec![first.clone(), kept.to_owned()]);
    assert_eq!(out.total, Some(2));
    assert!(marks().await.contains(&(first.clone(), false)));

    for id in bare.iter().map(String::as_str).chain([kept]) {
        assert!(s.delete(&t, id).await.expect("delete"));
    }
}

/// `2026-01-01T00:00:00Z` plus `i` half hours, without a date library.
fn half_hour(i: usize) -> String {
    let (h, m) = (i / 2, (i % 2) * 30);
    let day = h / 24;
    let (month, d) = if day < 31 {
        (1, day + 1)
    } else {
        (2, day - 30)
    };
    format!("2026-{month:02}-{d:02}T{:02}:{m:02}:00Z", h % 24)
}

/// Heap rows the plan read from `attr_instances` (every scan node on a
/// partition of it that visits the heap, actual rows times loops): on dev
/// each costs about a page read, while the N-th newest probes are index only.
fn history_rows_read(plan: &str) -> usize {
    plan.lines()
        .filter(|l| l.contains("Scan") && l.contains(" on attr_instances"))
        .filter(|l| !l.contains("Index Only Scan"))
        .filter_map(|l| {
            let n = |key: &str| -> Option<usize> {
                let rest = &l[l.find(key)? + key.len()..];
                rest.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            };
            Some(n("actual rows=")?.saturating_mul(n("loops=")?))
        })
        .sum()
}

/// T-6346: one Entity's one-Attribute week read with lastN reads only that
/// Attribute's rows inside the window's column bound. Before, `between` read
/// every Attribute's rows of the window (dev: 3.4 s, a heap page a row), and
/// `after` with fewer than N rows in the window read the Attribute's whole
/// history, every weekly partition (dev: 20 546 rows from disk, past the
/// 30 s statement timeout). Four Attributes of 2 500 instances each, half
/// hourly over eight weeks; the window is the last week.
#[tokio::test(flavor = "multi_thread")]
async fn a_one_attribute_week_reads_only_its_rows_in_the_window() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("pgweek").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:BikeHireDockingStation:week";
    let _ = s.delete(&t, id).await;
    const D: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
    let names: Vec<String> = ["availableBikeNumber", "freeSlotNumber", "status", "total"]
        .iter()
        .map(|n| format!("{D}{n}"))
        .collect();
    let mut doc = json!({
        "id": id, "type": "BikeHireDockingStation",
        "createdAt": "2026-01-01T00:00:00Z", "modifiedAt": "2026-01-01T00:00:00Z",
    });
    for (a, name) in names.iter().enumerate() {
        let insts: Vec<Value> = (0..2500)
            .map(|i| inst(a * 10_000 + i, &half_hour(i), None))
            .collect();
        doc[name] = Value::Array(insts);
    }
    assert!(s.create(&t, id, &doc).await.expect("create"));

    let asked = [names[0].clone()];
    let (from, to) = ("2026-02-15T00:00:00Z", "2026-02-22T00:00:00Z");
    let all = doc[&names[0]].as_array().expect("instances").clone();
    let in_window = |end: Option<&str>| -> BTreeSet<String> {
        all.iter()
            .filter(|i| {
                let at = i["observedAt"].as_str().unwrap_or("");
                at >= from && end.is_none_or(|e| at < e)
            })
            .map(|i| i["instanceId"].as_str().unwrap_or("").to_owned())
            .collect()
    };
    // the rows inside the widened (48 h) column bound of the asked Attribute
    let widened = all
        .iter()
        .filter(|i| i["observedAt"].as_str().unwrap_or("") >= "2026-02-13T00:00:00Z")
        .count();
    for (timerel, end) in [("between", Some(to)), ("after", None)] {
        let filter = |attrs| TemporalFilter {
            attrs,
            range: Some(InstanceRange {
                timerel,
                time_at: from,
                end_time_at: end,
                timeproperty: "observedAt",
            }),
            last_n: Some(1000),
            timeproperty: "observedAt",
            ..TemporalFilter::default()
        };
        let f = filter(Some(&asked[..]));
        let got = s.get_range(&t, id, &f).await.expect("read").expect("held");
        assert_eq!(ids(&got[&names[0]]), in_window(end), "{timerel}");
        for other in &names[1..] {
            assert!(got.get(other).is_none(), "{timerel}: {other} was read");
        }
        // without `attrs` the same instances of the asked Attribute come back
        let every = filter(None);
        let whole = s
            .get_range(&t, id, &every)
            .await
            .expect("read")
            .expect("held");
        assert_eq!(
            ids(&whole[&names[0]]),
            in_window(end),
            "{timerel} unfiltered"
        );
        assert!(whole.get(&names[3]).is_some(), "{timerel} unfiltered");

        let st = pg::temporal::range_statement(&f).expect("statement");
        let mut tx = pool.begin().await.expect("tx");
        sqlx::query(antares_sql::SET_TENANT_SQL)
            .bind(t.as_str())
            .execute(&mut *tx)
            .await
            .expect("tenant");
        let mut q = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, FORMAT TEXT) {}",
            st.sql
        )))
        .bind(t.as_str())
        .bind(id)
        .bind(st.only.expect("attrs bound"));
        for b in st.binds {
            q = q.bind(b);
        }
        let plan = q.fetch_all(&mut *tx).await.expect("explain").join("\n");
        tx.rollback().await.expect("rollback");
        let read = history_rows_read(&plan);
        assert!(
            read > 0,
            "{timerel}: no history scan found in the plan:\n{plan}"
        );
        assert!(
            read <= 3 * widened,
            "{timerel} read {read} history rows for {widened} in the window's bound:\n{plan}"
        );
    }
    assert!(s.delete(&t, id).await.expect("delete"));
}
