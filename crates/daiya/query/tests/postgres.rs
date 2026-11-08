#![forbid(unsafe_code)]
//! The Postgres driver: Grafana's SQL macros (`macros.go`), the read-only SELECT checker, and
//! (with `DAIYA_TEST_DATABASE_URL`) real results read back with their Postgres types, the row
//! limit and the health ping.

mod common;

use common::{closed_port, ctx, q};
use daiya_model::{FieldColumn, FieldType, NoticeSeverity, Subject};
use daiya_query::postgres::{
    MAX_SQL_ROWS, interpolate_postgres_macros, sql_time_series, strip_sql_comments,
    validate_readonly_select,
};
use daiya_query::{Datasource, DsError, HealthState, PostgresDatasource, Query, ResourceReq};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const FROM: i64 = 1_700_000_000_000;
const TO: i64 = 1_700_003_600_500;

fn expand(sql: &str) -> Result<String, DsError> {
    interpolate_postgres_macros(sql, FROM, TO, 60_000, false)
}

fn views(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|v| (*v).to_owned()).collect()
}

#[test]
fn comments_go_and_literals_stay() {
    assert_eq!(
        strip_sql_comments("SELECT 1 -- x\nFROM/* y */t"),
        "SELECT 1  \nFROM t"
    );
    // Comment markers inside literals, quoted identifiers and dollar quotes are text.
    for kept in [
        "SELECT '--not' , 'it''s /* no */'",
        "SELECT \"a--b\", \"q\"\"/*\"",
        "SELECT $$ -- $$, $tag$ /* $tag$",
        "SELECT $1, $ x, $a1",
        "SELECT 'héllo -- ü'",
    ] {
        assert_eq!(strip_sql_comments(kept), kept, "{kept}");
    }
    // An unterminated block comment or dollar quote runs to the end.
    assert_eq!(strip_sql_comments("SELECT 1 /* open"), "SELECT 1  ");
    assert_eq!(strip_sql_comments("SELECT $q$ open"), "SELECT $q$ open");
}

#[test]
fn macros_expand_like_grafana() {
    assert_eq!(
        expand("SELECT $__time(ts), $__timeEpoch(ts) FROM v").unwrap(),
        "SELECT ts AS \"time\", extract(epoch from ts) as \"time\" FROM v"
    );
    assert_eq!(
        expand("WHERE $__timeFilter(ts) AND x > $__timeFrom() AND x < $__timeTo()").unwrap(),
        "WHERE ts BETWEEN '2023-11-14T22:13:20.000000000Z' AND '2023-11-14T23:13:20.500000000Z' \
         AND x > '2023-11-14T22:13:20.000000000Z' AND x < '2023-11-14T23:13:20.500000000Z'"
    );
    assert_eq!(
        expand("WHERE $__unixEpochFilter(t)").unwrap(),
        "WHERE t >= 1700000000 AND t <= 1700003600"
    );
    // A timeGroup followed by a comma gets the time alias, as in Grafana.
    assert_eq!(
        expand("SELECT $__timeGroup(ts, '5m'), avg(v) GROUP BY $__timeGroup(ts, 5m)").unwrap(),
        "SELECT floor(extract(epoch from ts)/300)*300 AS \"time\", avg(v) GROUP BY floor(extract(epoch from ts)/300)*300"
    );
    assert_eq!(
        expand("SELECT $__timeGroupAlias(ts, 1h) FROM v").unwrap(),
        "SELECT floor(extract(epoch from ts)/3600)*3600 AS \"time\" FROM v"
    );
    assert_eq!(
        interpolate_postgres_macros("$__timeGroup(ts, 90s)", FROM, TO, 60_000, true).unwrap(),
        "time_bucket('90.000s',ts)"
    );
    assert_eq!(
        expand("$__unixEpochGroup(t, 1m) $__unixEpochGroupAlias(t, \"30s\")").unwrap(),
        "floor((t)/60)*60 floor((t)/30)*30 AS \"time\""
    );
    assert_eq!(
        expand("$__interval $__interval_ms ${__interval} ${__interval_ms}").unwrap(),
        "'1m' 60000 '1m' 60000"
    );
    // A macro inside a comment is gone with the comment.
    assert_eq!(expand("SELECT 1 -- $__bogus(x)").unwrap(), "SELECT 1  ");

    for (sql, needle) in [
        ("$__time()", "missing time column"),
        ("$__timeEpoch()", "missing time column"),
        ("$__timeFilter()", "missing time column"),
        ("$__unixEpochFilter()", "missing time column"),
        ("$__timeFilter(ts; DROP)", "unsafe column identifier"),
        ("$__time(a b)", "unsafe column identifier"),
        ("$__timeGroup(ts)", "needs time column and interval"),
        ("$__unixEpochGroup(ts)", "needs time column and interval"),
        ("$__timeGroup(ts, soon)", "error parsing interval"),
        ("$__unixEpochGroup(ts, soon)", "error parsing interval"),
        ("$__timeGroup(ts, 0s)", "interval must be positive"),
        ("$__timeGroup(\"x y\", 1m)", "unsafe column identifier"),
        ("$__nope(ts) $__time()", "unknown macro '__nope'"),
    ] {
        let err = expand(sql).unwrap_err();
        assert!(err.to_string().contains(needle), "{sql}: {err}");
    }
}

#[test]
fn the_checker_refuses_malformed_and_foreign_sql_with_a_reason() {
    let v = views(&["metrics", "Mixed"]);
    for (sql, needle) in [
        ("", "empty"),
        (" ; ", "empty"),
        ("1", "only SELECT"),
        ("EXPLAIN SELECT 1", "found 'explain'"),
        ("SELECT 'open", "unterminated string"),
        ("SELECT \"\" FROM metrics", "quoted identifier"),
        ("SELECT \"open FROM metrics", "quoted identifier"),
        ("SELECT a$b FROM metrics", "'$' in identifiers"),
        (
            "SELECT 1 FROM metrics WHERE a = 'x' {",
            "unexpected character",
        ),
        ("SELECT pg_sleep(1)", "forbidden SQL keyword"),
        ("SELECT * FROM metrics FOR SHARE", "locking clauses"),
        ("WITH", "malformed WITH"),
        ("WITH x SELECT 1", "malformed WITH"),
        ("WITH x AS SELECT 1", "malformed WITH"),
        ("WITH (SELECT 1) SELECT 1", "malformed WITH"),
        ("WITH x AS (SELECT 1) TABLE x", "followed by SELECT"),
        (
            "WITH RECURSIVE x AS NOT MATERIALIZED (SELECT 1) SELECT 1 FROM x, (WITH y AS (SELECT 1) SELECT 1) z",
            "nested WITH",
        ),
        ("SELECT * FROM metrics JOIN", "malformed FROM item"),
        ("SELECT * FROM metrics, 1", "malformed FROM item"),
        ("SELECT * FROM \"METRICS\"", "not in the organization"),
        ("SELECT * FROM information_schema.tables", "system catalog"),
        ("SELECT * FROM pg_stat_activity", "system catalog"),
        (
            "SELECT now()::numeric(2) FROM metrics WHERE f(1)",
            "function 'f'",
        ),
    ] {
        let err = validate_readonly_select("A", sql, &v).unwrap_err();
        assert!(
            matches!(&err, DsError::InvalidQuery { ref_id, .. } if ref_id == "A"),
            "{err}"
        );
        assert!(err.to_string().contains(needle), "{sql}: {err}");
    }
    // A quoted identifier matches the granted view exactly; an unquoted one in any case.
    for ok in [
        "SELECT * FROM \"Mixed\"",
        "SELECT * FROM MIXED",
        "SELECT * FROM ONLY metrics m",
        "WITH RECURSIVE x AS MATERIALIZED (SELECT 1) SELECT * FROM x",
    ] {
        assert!(validate_readonly_select("A", ok, &v).is_ok(), "{ok}");
    }
    // The text checked is the text returned: comments stripped, trailing `;` dropped.
    assert_eq!(
        validate_readonly_select("A", "SELECT 1 FROM metrics -- c\n;", &v).unwrap(),
        "SELECT 1 FROM metrics"
    );
}

#[tokio::test]
async fn without_a_pool_the_checked_sql_is_answered_as_an_empty_frame() {
    let d = PostgresDatasource::new("pg", views(&["metrics"]), false, None);
    assert_eq!(d.kind(), "postgres");
    let c = ctx(Subject::anonymous());
    let queries = vec![
        Query {
            ref_id: "H".into(),
            hide: true,
            model: json!({ "rawSql": "DROP TABLE x" }),
        },
        Query {
            ref_id: "A".into(),
            hide: false,
            model: json!({ "rawSql": "SELECT $__time(ts) FROM metrics WHERE $__timeFilter(ts)" }),
        },
    ];
    let sets = d.query(&c, &queries).await.unwrap();
    assert_eq!(sets.len(), 1);
    let frame = &sets[0].frames[0];
    assert!(frame.fields.is_empty());
    assert!(
        frame
            .meta
            .executed_query_string
            .as_deref()
            .unwrap()
            .starts_with("SELECT ts AS \"time\" FROM metrics WHERE ts BETWEEN")
    );
    let err = d
        .query(&c, &q(json!({ "sql": "SELECT 1" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("missing 'rawSql'"), "{err}");
    let err = d
        .query(&c, &q(json!({ "rawSql": "SELECT * FROM secret" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allow-list"), "{err}");
    let err = d
        .query(&c, &q(json!({ "rawSql": "SELECT $__time() FROM metrics" })))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("missing time column"), "{err}");

    let h = d.health(&c).await;
    assert_eq!(h.status, HealthState::Ok);
    assert!(h.message.contains("1 granted view"));
    let r = d
        .resource(
            &c,
            ResourceReq {
                path: "views".into(),
                params: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(r.data, json!({ "allowedViews": ["metrics"] }));

    let target = json!({ "refId": "B", "rawSql": "SELECT 1", "format": "table", "hide": true });
    let imported = d.import(&target).unwrap();
    assert!(imported.warnings.is_empty());
    assert!(imported.query.hide);
    assert_eq!(d.export(&imported.query), target);
    let plain = d.import(&json!({ "rawSql": "SELECT 1" })).unwrap();
    assert_eq!(plain.query.ref_id, "A");
    assert_eq!(
        d.export(&plain.query),
        json!({ "rawSql": "SELECT 1", "refId": "A" })
    );
    assert_eq!(
        d.export(&Query {
            ref_id: "Z".into(),
            hide: false,
            model: json!(1)
        }),
        json!({ "refId": "Z" })
    );
    assert!(d.import(&json!("SELECT 1")).is_err());
    assert_eq!(d.editor().json_schema["required"], json!(["rawSql"]));
}

#[tokio::test]
async fn health_fails_when_the_database_cannot_be_reached() {
    let port = closed_port().await;
    let opts = sqlx::postgres::PgConnectOptions::new()
        .host("127.0.0.1")
        .port(port)
        .username("daiya_nobody")
        .password("never-shown-9f2")
        .database("daiya_none");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy_with(opts);
    let d = PostgresDatasource::new("pg", views(&["metrics"]), false, Some(pool));
    let c = ctx(Subject::anonymous());
    let h = d.health(&c).await;
    assert_eq!(h.status, HealthState::Error, "{}", h.message);
    assert!(h.message.contains("postgres query failed"), "{}", h.message);
    assert!(!h.message.contains("never-shown-9f2"));
    let err = d
        .query(&c, &q(json!({ "rawSql": "SELECT 1 FROM metrics" })))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DsError::Upstream { status: 400, .. }),
        "{err}"
    );
    assert!(!err.to_string().contains("never-shown-9f2"));
}

/// One password per test process, so a test's role setup never invalidates another's pool.
static READER_PASSWORD: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| uuid::Uuid::new_v4().simple().to_string());
static SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A reader role on the test server that may read the given views (created by the admin).
async fn reader(setup: &str) -> Option<sqlx::PgPool> {
    let Ok(admin_url) = std::env::var("DAIYA_TEST_DATABASE_URL") else {
        println!("SKIP: DAIYA_TEST_DATABASE_URL is unset");
        return None;
    };
    let opts: sqlx::postgres::PgConnectOptions = admin_url.parse().unwrap();
    let admin = sqlx::PgPool::connect_with(opts.clone()).await.unwrap();
    let password = READER_PASSWORD.as_str();
    let _serial = SETUP.lock().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'daiya_ds_types') THEN
                CREATE ROLE daiya_ds_types LOGIN NOSUPERUSER NOBYPASSRLS;
            END IF;
         END $$;
         ALTER ROLE daiya_ds_types PASSWORD '{password}';
         {setup}"
    )))
    .execute(&admin)
    .await
    .unwrap();
    let reader = opts.username("daiya_ds_types").password(password);
    Some(sqlx::PgPool::connect_with(reader).await.unwrap())
}

#[tokio::test]
async fn real_results_keep_their_postgres_types() {
    let Some(pool) = reader(
        "CREATE OR REPLACE VIEW daiya_ds_types AS SELECT
            1::int2 AS i2, 2::int4 AS i4, 3::int8 AS i8, 1.5::float4 AS f4, 2.25::float8 AS f8,
            12345.678::numeric AS n, (-0.0001)::numeric AS nn, 0::numeric AS nz,
            123456789012345678.5::numeric AS nbig, 'NaN'::numeric AS nnan,
            true AS b, 'txt'::text AS t, 'v'::varchar AS v, 'c'::char(3) AS c,
            '{\"a\": 1}'::jsonb AS j, '[2]'::json AS jj,
            '6f1e8a5c-0000-4000-8000-000000000001'::uuid AS u,
            TIMESTAMP '2024-01-02 03:04:05.5' AS ts, DATE '2024-01-02' AS d,
            TIMESTAMPTZ '2024-01-02 03:04:05+01' AS tz, interval '1 day' AS iv,
            NULL::int4 AS nul, NULL::numeric AS nnul;
         GRANT SELECT ON daiya_ds_types TO daiya_ds_types;",
    )
    .await
    else {
        return;
    };
    let d = PostgresDatasource::new("pg", views(&["daiya_ds_types"]), false, Some(pool));
    let c = ctx(Subject::anonymous());
    let sets = d
        .query(&c, &q(json!({ "rawSql": "SELECT * FROM daiya_ds_types" })))
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    let col = |name: &str| {
        frame
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no column {name}"))
    };
    for (name, ty, value) in [
        ("i2", FieldType::Number, json!(1.0)),
        ("i4", FieldType::Number, json!(2.0)),
        ("i8", FieldType::Number, json!(3.0)),
        ("f4", FieldType::Number, json!(1.5)),
        ("f8", FieldType::Number, json!(2.25)),
        ("n", FieldType::Number, json!(12345.678)),
        ("nn", FieldType::Number, json!(-0.0001)),
        ("nz", FieldType::Number, json!(0.0)),
        ("nbig", FieldType::Number, json!(123_456_789_012_345_678.5)),
        ("nnan", FieldType::Number, Value::Null),
        ("b", FieldType::Bool, json!(true)),
        ("t", FieldType::String, json!("txt")),
        ("v", FieldType::String, json!("v")),
        ("c", FieldType::String, json!("c  ")),
        ("j", FieldType::Json, json!({ "a": 1 })),
        ("jj", FieldType::Json, json!([2])),
        (
            "u",
            FieldType::String,
            json!("6f1e8a5c-0000-4000-8000-000000000001"),
        ),
        ("ts", FieldType::Time, json!(1_704_164_645_500_i64)),
        ("d", FieldType::Time, json!(1_704_153_600_000_i64)),
        ("tz", FieldType::Time, json!(1_704_161_045_000_i64)),
        ("nul", FieldType::Number, Value::Null),
        ("nnul", FieldType::Number, Value::Null),
    ] {
        let f = col(name);
        assert_eq!(f.field_type, ty, "{name}");
        let got = &f.values[0];
        match (got.as_f64(), value.as_f64()) {
            (Some(a), Some(b)) => assert!((a - b).abs() <= b.abs() * 1e-12, "{name}: {got}"),
            _ => assert_eq!(got, &value, "{name}"),
        }
    }
    // A type the driver does not read is named in a notice, never silently empty.
    assert_eq!(col("iv").values, vec![Value::Null]);
    assert_eq!(frame.meta.notices.len(), 1, "{:?}", frame.meta.notices);
    assert!(
        frame.meta.notices[0]
            .text
            .contains("'iv' has type INTERVAL")
    );

    // avg() over integers answers NUMERIC, the common dashboard case.
    let sets = d
        .query(
            &c,
            &q(json!({ "rawSql": "SELECT avg(i4) AS mean, sum(i8) AS total FROM daiya_ds_types" })),
        )
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    assert_eq!(frame.fields[0].f64_at(0), Some(2.0));
    assert_eq!(frame.fields[1].f64_at(0), Some(3.0));
    assert!(frame.meta.notices.is_empty());

    // No rows: no columns; an error of the database is an upstream error.
    let sets = d
        .query(
            &c,
            &q(json!({ "rawSql": "SELECT i4 FROM daiya_ds_types WHERE i4 > 10" })),
        )
        .await
        .unwrap();
    assert!(sets[0].frames[0].fields.is_empty());
    let err = d
        .query(
            &c,
            &q(json!({ "rawSql": "SELECT i4 / 0 FROM daiya_ds_types" })),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DsError::Upstream { status: 400, message } if message.contains("division by zero")),
        "{err}"
    );
    let h = d.health(&c).await;
    assert_eq!(h.status, HealthState::Ok, "{}", h.message);
}

#[tokio::test]
async fn a_result_past_the_row_limit_is_cut_with_a_notice() {
    let Some(pool) = reader(&format!(
        "CREATE OR REPLACE VIEW daiya_ds_big AS SELECT n FROM generate_series(1, {}) AS n;
         CREATE OR REPLACE VIEW daiya_ds_exact AS SELECT n FROM generate_series(1, {MAX_SQL_ROWS}) AS n;
         GRANT SELECT ON daiya_ds_big, daiya_ds_exact TO daiya_ds_types;",
        MAX_SQL_ROWS + 1
    ))
    .await
    else {
        return;
    };
    let d = PostgresDatasource::new(
        "pg",
        views(&["daiya_ds_big", "daiya_ds_exact"]),
        false,
        Some(pool),
    );
    let c = ctx(Subject::anonymous());
    let sets = d
        .query(&c, &q(json!({ "rawSql": "SELECT n FROM daiya_ds_big" })))
        .await
        .unwrap();
    let frame = &sets[0].frames[0];
    assert_eq!(frame.fields[0].values.len(), MAX_SQL_ROWS);
    assert_eq!(frame.meta.notices.len(), 1);
    assert_eq!(frame.meta.notices[0].severity, NoticeSeverity::Warning);
    assert!(frame.meta.notices[0].text.contains("limited to 50000 rows"));
    // Exactly the limit: nothing was cut, no notice.
    let sets = d
        .query(&c, &q(json!({ "rawSql": "SELECT n FROM daiya_ds_exact" })))
        .await
        .unwrap();
    assert_eq!(sets[0].frames[0].fields[0].values.len(), MAX_SQL_ROWS);
    assert!(sets[0].frames[0].meta.notices.is_empty());
}

fn column(name: &str, field_type: FieldType, values: Vec<Value>) -> FieldColumn {
    FieldColumn {
        name: name.into(),
        field_type,
        unit: None,
        display_name: None,
        labels: BTreeMap::new(),
        values,
    }
}

/// `format: time_series` turns a long result wide as Grafana's `sqleng` does (`LongToWide`).
#[test]
fn a_long_time_series_turns_wide_with_a_field_per_label_set() {
    let long = vec![
        column(
            "time",
            FieldType::Time,
            vec![json!(1000), json!(1000), json!(2000)],
        ),
        column(
            "host",
            FieldType::String,
            vec![json!("a"), json!("b"), json!("a")],
        ),
        column(
            "cpu",
            FieldType::Number,
            vec![json!(1.0), json!(2.0), json!(3.0)],
        ),
        column(
            "mem",
            FieldType::Number,
            vec![json!(10), json!(20), json!(30)],
        ),
    ];
    let wide = sql_time_series(long).unwrap();
    let names: Vec<(&str, Option<&String>)> = wide
        .iter()
        .map(|f| (f.name.as_str(), f.labels.get("host")))
        .collect();
    let (a, b) = ("a".to_owned(), "b".to_owned());
    assert_eq!(
        names,
        [
            ("Time", None),
            ("cpu", Some(&a)),
            ("mem", Some(&a)),
            ("cpu", Some(&b)),
            ("mem", Some(&b)),
        ]
    );
    assert_eq!(wide[0].values, [json!(1000), json!(2000)]);
    assert_eq!(wide[1].values, [json!(1.0), json!(3.0)]);
    // b has no point at 2000: the gap is null.
    assert_eq!(wide[3].values, [json!(2.0), Value::Null]);
    assert_eq!(wide[4].values, [json!(20), Value::Null]);
}

#[test]
fn a_metric_column_names_its_series_and_epochs_become_milliseconds() {
    let long = vec![
        column(
            "time",
            FieldType::Number,
            vec![json!(1_700_000_000), json!(1_700_000_060)],
        ),
        column(
            "metric",
            FieldType::String,
            vec![json!("up"), json!("down")],
        ),
        column("value", FieldType::Number, vec![json!(1), json!(0)]),
    ];
    let wide = sql_time_series(long).unwrap();
    assert_eq!(wide[0].field_type, FieldType::Time);
    assert_eq!(
        wide[0].values,
        [json!(1_700_000_000_000_i64), json!(1_700_000_060_000_i64)]
    );
    assert_eq!(wide[1].name, "up");
    assert!(wide[1].labels.is_empty());
    assert_eq!(wide[2].name, "down");
    assert_eq!(wide[2].values, [Value::Null, json!(0)]);
}

#[test]
fn a_wide_result_keeps_its_columns_and_refusals_name_grafanas_reason() {
    let wide = vec![
        column("time", FieldType::Time, vec![json!(1000)]),
        column("value", FieldType::Number, vec![json!(1)]),
    ];
    let out = sql_time_series(wide).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].name, "Time");
    assert_eq!(out[1].name, "value");

    let no_time = vec![column("value", FieldType::Number, vec![json!(1)])];
    assert_eq!(
        sql_time_series(no_time).unwrap_err(),
        "no time column found"
    );
    let unsorted = vec![
        column("time", FieldType::Time, vec![json!(2000), json!(1000)]),
        column("host", FieldType::String, vec![json!("a"), json!("a")]),
        column("v", FieldType::Number, vec![json!(1), json!(2)]),
    ];
    assert!(
        sql_time_series(unsorted)
            .unwrap_err()
            .contains("sorted ascending")
    );
}
