#![forbid(unsafe_code)]
#![allow(clippy::disallowed_methods)] // The test reads its database URL; jc runs none of this.
//! Round trip of [`TimestampTz`], [`PgTimestamp`] and [`PgDate`] through a real Postgres, in
//! the binary (prepared) and text (simple query) formats, with the session in Europe/Bratislava
//! and Europe/Helsinki across their DST changes. `JC_TIME_TEST_DATABASE_URL` names the server;
//! unset prints `SKIP`. The SQL built with `format!` holds only the constants below.
#![cfg(feature = "sqlx")]

use jc_time::{PgDate, PgTimestamp, SecondsFormat, TimestampTz, parse_rfc3339};
use jiff::civil;
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row};

async fn connect() -> Option<PgConnection> {
    let Ok(url) = std::env::var("JC_TIME_TEST_DATABASE_URL") else {
        println!("SKIP: JC_TIME_TEST_DATABASE_URL is unset");
        return None;
    };
    Some(PgConnection::connect(&url).await.expect("connect"))
}

fn ts(s: &str) -> TimestampTz {
    TimestampTz(parse_rfc3339(s).expect(s))
}

#[tokio::test]
async fn timestamptz_round_trips_in_both_formats_and_zones() {
    let Some(mut db) = connect().await else {
        return;
    };
    let values = [
        "2000-01-01T00:00:00Z",
        "1999-12-31T23:59:59.999999Z",
        "1970-01-01T00:00:00Z",
        "2026-11-18T16:20:00.123456Z",
        "2026-03-29T00:59:59Z",
        "2026-03-29T01:00:00Z",
        "2026-10-25T00:30:00Z",
        "2026-10-25T01:30:00Z",
    ];
    for zone in ["UTC", "Europe/Bratislava", "Europe/Helsinki"] {
        sqlx::raw_sql(AssertSqlSafe(format!("SET TIME ZONE '{zone}'")))
            .execute(&mut db)
            .await
            .expect("set zone");
        for v in values {
            let sent = ts(v);
            let row = sqlx::query("SELECT $1::timestamptz AS t, to_char($1 AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US') AS s")
                .bind(sent)
                .fetch_one(&mut db)
                .await
                .expect("binary");
            assert_eq!(row.get::<TimestampTz, _>("t"), sent, "{zone} binary {v}");
            let printed: String = row.get("s");
            assert_eq!(
                format!("{printed}Z"),
                sent.to_rfc3339_opts(SecondsFormat::Micros, true),
                "{zone} server sees {v}"
            );
            let text = sqlx::raw_sql(AssertSqlSafe(format!("SELECT '{v}'::timestamptz AS t")))
                .fetch_one(&mut db)
                .await
                .expect("text");
            assert_eq!(text.get::<TimestampTz, _>("t"), sent, "{zone} text {v}");
        }
    }
    // A local wall time in the spring gap and the autumn fold names one instant per zone.
    sqlx::raw_sql("SET TIME ZONE 'Europe/Helsinki'")
        .execute(&mut db)
        .await
        .expect("set zone");
    let row = sqlx::query("SELECT '2026-10-25 03:30:00 Europe/Helsinki'::timestamptz AS fold")
        .fetch_one(&mut db)
        .await
        .expect("fold");
    // Postgres reads a time in the fold as standard time (+02), the later instant.
    assert_eq!(
        row.get::<TimestampTz, _>("fold"),
        ts("2026-10-25T01:30:00Z")
    );
    let row = sqlx::query("SELECT '2026-03-29 02:30:00 Europe/Bratislava'::timestamptz AS gap")
        .fetch_one(&mut db)
        .await
        .expect("gap");
    // A time in the spring gap is read with the offset before the change (+01).
    assert_eq!(row.get::<TimestampTz, _>("gap"), ts("2026-03-29T01:30:00Z"));
}

#[tokio::test]
async fn timestamp_and_date_round_trip() {
    let Some(mut db) = connect().await else {
        return;
    };
    for (dt, d) in [
        (
            civil::date(2000, 1, 1).at(0, 0, 0, 0),
            civil::date(2000, 1, 1),
        ),
        (
            civil::date(1999, 12, 31).at(23, 59, 59, 999_999_000),
            civil::date(1969, 7, 20),
        ),
        (
            civil::date(2026, 3, 29).at(2, 30, 0, 0),
            civil::date(2026, 10, 25),
        ),
    ] {
        let row =
            sqlx::query("SELECT $1::timestamp AS t, $2::date AS d, $1::text AS ts, $2::text AS ds")
                .bind(PgTimestamp(dt))
                .bind(PgDate(d))
                .fetch_one(&mut db)
                .await
                .expect("binary");
        assert_eq!(row.get::<PgTimestamp, _>("t").0, dt);
        assert_eq!(row.get::<PgDate, _>("d").0, d);
        let (ts_text, d_text): (String, String) = (row.get("ts"), row.get("ds"));
        let text = sqlx::raw_sql(AssertSqlSafe(format!(
            "SELECT '{ts_text}'::timestamp AS t, '{d_text}'::date AS d"
        )))
        .fetch_one(&mut db)
        .await
        .expect("text");
        assert_eq!(text.get::<PgTimestamp, _>("t").0, dt);
        assert_eq!(text.get::<PgDate, _>("d").0, d);
    }
}

#[tokio::test]
async fn infinity_is_an_error_not_a_panic() {
    let Some(mut db) = connect().await else {
        return;
    };
    let row = sqlx::query("SELECT 'infinity'::timestamptz AS t, '-infinity'::timestamp AS u")
        .fetch_one(&mut db)
        .await
        .expect("query");
    assert!(row.try_get::<TimestampTz, _>("t").is_err());
    assert!(row.try_get::<PgTimestamp, _>("u").is_err());
}
