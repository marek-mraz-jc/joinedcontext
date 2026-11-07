//! Usage per Endpoint, surface and day for the publishers' Usage tab (T-5118, API/01 §54).
//!
//! Every gateway replica posts what each Endpoint's surfaces answered since its last post, every
//! five minutes (`POST /internal/usage`); the Portal adds the counts into one row per project,
//! Endpoint, surface and UTC day and keeps them 400 days, long enough for a year of monthly
//! exports. A row is a name, a kind of caller (person, workload, agent, anonymous; T-6186) and a
//! number: the gateway sends no address, token, subject, client id or query, and nothing here
//! could store one.

use std::collections::BTreeMap;
use std::sync::RwLock;

use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The surfaces the gateway counts, in the order the Usage tab shows them.
pub const SURFACES: &[&str] = &[
    "ngsi-ld",
    "ogc",
    "odata",
    "sta",
    "ical",
    "feed",
    "mcp",
    "websocket",
    "download",
];
/// The kinds of caller a row names, in the order the grid shows them; `unknown` is what was
/// counted before the gateway told them apart, or by a gateway that does not.
pub const CALLERS: &[&str] = &["person", "workload", "agent", "anonymous", "unknown"];
/// How long a day's row is kept.
pub const KEEP_DAYS: i64 = 400;
/// The longest window the Usage tab asks for.
pub const MAX_DAYS: i64 = 90;
/// Lines in one post; a replica sends one per Endpoint and surface it served.
pub const MAX_LINES: usize = 5000;
/// One line's count: 33,000 answers a second for five minutes on one replica.
pub const MAX_COUNT: u64 = 10_000_000;

/// One project, Endpoint name, surface, day and kind of caller.
type Key = (String, String, String, NaiveDate, String);

/// One line of a gateway's post.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Line {
    /// The Endpoint's slug, as in `/api/endpoint/{slug}`.
    pub endpoint: String,
    pub surface: String,
    /// One of [`CALLERS`]; absent from an older gateway, which is `unknown`.
    #[serde(default)]
    pub caller: Option<String>,
    pub count: u64,
}

/// A gateway's post.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Post {
    pub counts: Vec<Line>,
}

/// A post as the gateway sent it, read and checked; the error names the first fault.
pub fn parse(body: &[u8]) -> Result<Post, String> {
    let post: Post =
        serde_json::from_slice(body).map_err(|e| format!("the usage post is not readable: {e}"))?;
    check(&post)?;
    Ok(post)
}

/// The first fault of a post, as the 422 names it; a post is taken whole or not at all.
pub fn check(post: &Post) -> Result<(), String> {
    if post.counts.len() > MAX_LINES {
        return Err(format!("counts: at most {MAX_LINES} lines in one post"));
    }
    for (i, line) in post.counts.iter().enumerate() {
        if !crate::resource::is_dns1123(&line.endpoint) {
            return Err(format!("counts[{i}].endpoint: not an Endpoint slug"));
        }
        if !SURFACES.contains(&line.surface.as_str()) {
            return Err(format!(
                "counts[{i}].surface: one of {}",
                SURFACES.join(", ")
            ));
        }
        if line
            .caller
            .as_deref()
            .is_some_and(|caller| !CALLERS.contains(&caller))
        {
            return Err(format!("counts[{i}].caller: one of {}", CALLERS.join(", ")));
        }
        if line.count > MAX_COUNT {
            return Err(format!("counts[{i}].count: at most {MAX_COUNT}"));
        }
    }
    Ok(())
}

/// The post's lines summed per row of `day`, each slug resolved to its project and Endpoint name
/// by `resolve`; a slug no Endpoint has is dropped, so a row always names a configured Endpoint.
pub fn rows(
    post: &Post,
    day: NaiveDate,
    resolve: impl Fn(&str) -> Option<(String, String)>,
) -> BTreeMap<Key, u64> {
    let mut rows = BTreeMap::new();
    for line in post.counts.iter().filter(|line| line.count > 0) {
        let Some((project, name)) = resolve(&line.endpoint) else {
            continue;
        };
        let held = rows
            .entry((
                project,
                name,
                line.surface.clone(),
                day,
                line.caller.clone().unwrap_or_else(|| "unknown".to_owned()),
            ))
            .or_insert(0_u64);
        *held = held.saturating_add(line.count);
    }
    rows
}

/// One Endpoint's calls on one day, by surface.
#[derive(Debug, Clone, Default, PartialEq, Serialize, ToSchema)]
pub struct Day {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub calls: BTreeMap<String, u64>,
    pub total: u64,
}

/// One Endpoint's usage over a window, a row for every day, empty days included.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndpointUsage {
    /// The Endpoint's manifest name.
    pub name: String,
    pub from: String,
    pub to: String,
    pub days: Vec<Day>,
    /// The window's calls by surface.
    pub totals: BTreeMap<String, u64>,
    /// The window's calls by kind of caller, one of person, workload, agent, anonymous or
    /// unknown (counted before the gateway told them apart; T-6186).
    pub callers: BTreeMap<String, u64>,
    pub total: u64,
}

/// One stored row as a read answers it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub name: String,
    pub surface: String,
    pub day: NaiveDate,
    /// One of [`CALLERS`].
    pub caller: String,
    pub calls: u64,
}

/// The `days` days ending with `to`, oldest first.
pub fn window(to: NaiveDate, days: i64) -> (NaiveDate, NaiveDate) {
    (to - Duration::days(days.clamp(1, MAX_DAYS) - 1), to)
}

/// The first and last day of `month` (`YYYY-MM`); `None` for anything else.
pub fn month(text: &str) -> Option<(NaiveDate, NaiveDate)> {
    let (year, month) = text.split_once('-')?;
    if year.len() != 4 || month.len() != 2 {
        return None;
    }
    let first = NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)?;
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)?
    };
    Some((first, next - Duration::days(1)))
}

/// `name`'s rows from `rows` laid out over the window, every day present.
pub fn usage(name: &str, from: NaiveDate, to: NaiveDate, rows: &[Stored]) -> EndpointUsage {
    let mut days: Vec<Day> = from
        .iter_days()
        .take_while(|day| *day <= to)
        .map(|day| Day {
            day: day.to_string(),
            ..Day::default()
        })
        .collect();
    let mut totals = BTreeMap::new();
    let mut callers = BTreeMap::new();
    let mut total = 0_u64;
    for row in rows.iter().filter(|row| row.name == name) {
        let Some(day) = usize::try_from((row.day - from).num_days())
            .ok()
            .and_then(|i| days.get_mut(i))
        else {
            continue;
        };
        *day.calls.entry(row.surface.clone()).or_insert(0) += row.calls;
        day.total = day.total.saturating_add(row.calls);
        *totals.entry(row.surface.clone()).or_insert(0_u64) += row.calls;
        *callers.entry(row.caller.clone()).or_insert(0_u64) += row.calls;
        total = total.saturating_add(row.calls);
    }
    EndpointUsage {
        name: name.to_owned(),
        from: from.to_string(),
        to: to.to_string(),
        days,
        totals,
        callers,
        total,
    }
}

/// The monthly export: one line per day, Endpoint and surface that had calls, every kind of
/// caller summed, so the published columns stay as they were.
pub fn csv(rows: &[Stored]) -> String {
    let mut lines: BTreeMap<(NaiveDate, &str, &str), u64> = BTreeMap::new();
    for row in rows.iter().filter(|row| row.calls > 0) {
        let held = lines.entry((row.day, &row.name, &row.surface)).or_insert(0);
        *held = held.saturating_add(row.calls);
    }
    let mut out = String::from("day,endpoint,surface,calls\n");
    for ((day, name, surface), calls) in lines {
        // Names are DNS-1123 and surfaces come from SURFACES: nothing to quote.
        out.push_str(&format!("{day},{name},{surface},{calls}\n"));
    }
    out
}

enum Inner {
    Postgres(sqlx::PgPool),
    Memory(RwLock<BTreeMap<Key, u64>>),
}

/// The rows: Postgres when the Portal has a database, else this process's memory.
pub struct UsageStore {
    inner: Inner,
}

impl UsageStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        Self {
            inner: match db {
                Some(pool) => Inner::Postgres(pool),
                None => Inner::Memory(RwLock::default()),
            },
        }
    }

    /// Adds `rows` and drops what is older than [`KEEP_DAYS`] before `today`.
    pub async fn add(&self, rows: BTreeMap<Key, u64>, today: NaiveDate) -> Result<(), sqlx::Error> {
        let oldest = today - Duration::days(KEEP_DAYS);
        let pool = match &self.inner {
            Inner::Memory(memory) => {
                let mut memory = memory.write().unwrap_or_else(|e| e.into_inner());
                for (key, calls) in rows {
                    let held = memory.entry(key).or_insert(0);
                    *held = held.saturating_add(calls);
                }
                memory.retain(|key, _| key.3 >= oldest);
                return Ok(());
            }
            Inner::Postgres(pool) => pool,
        };
        let column = |f: fn(&Key) -> String| rows.keys().map(f).collect::<Vec<_>>();
        let calls: Vec<i64> = rows
            .values()
            .map(|n| i64::try_from(*n).unwrap_or(i64::MAX))
            .collect();
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO endpoint_usage_daily AS u (project, endpoint, surface, day, caller, calls) \
             SELECT p, e, s, d::date, k, c FROM UNNEST($1::text[], $2::text[], $3::text[], \
             $4::text[], $5::text[], $6::bigint[]) AS t(p, e, s, d, k, c) \
             ON CONFLICT (project, endpoint, day, surface, caller) \
             DO UPDATE SET calls = u.calls + EXCLUDED.calls",
        )
        .bind(column(|k| k.0.clone()))
        .bind(column(|k| k.1.clone()))
        .bind(column(|k| k.2.clone()))
        .bind(column(|k| k.3.to_string()))
        .bind(column(|k| k.4.clone()))
        .bind(calls)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM endpoint_usage_daily WHERE day < $1::date")
            .bind(oldest.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    /// `project`'s rows from `from` to `to`, both inclusive.
    pub async fn read(
        &self,
        project: &str,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<Stored>, sqlx::Error> {
        let pool = match &self.inner {
            Inner::Memory(memory) => {
                let memory = memory.read().unwrap_or_else(|e| e.into_inner());
                return Ok(memory
                    .iter()
                    .filter(|(k, _)| k.0 == project && k.3 >= from && k.3 <= to)
                    .map(|(k, calls)| Stored {
                        name: k.1.clone(),
                        surface: k.2.clone(),
                        day: k.3,
                        caller: k.4.clone(),
                        calls: *calls,
                    })
                    .collect());
            }
            Inner::Postgres(pool) => pool,
        };
        let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
            "SELECT endpoint, surface, to_char(day, 'YYYY-MM-DD'), caller, calls \
             FROM endpoint_usage_daily \
             WHERE project = $1 AND day BETWEEN $2::date AND $3::date",
        )
        .bind(project)
        .bind(from.to_string())
        .bind(to.to_string())
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(name, surface, day, caller, calls)| {
                Some(Stored {
                    name,
                    surface,
                    day: day.parse().ok()?,
                    caller,
                    calls: u64::try_from(calls).unwrap_or(0),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> NaiveDate {
        text.parse().unwrap_or_default()
    }

    fn post(lines: &[(&str, &str, u64)]) -> Post {
        Post {
            counts: lines
                .iter()
                .map(|(endpoint, surface, count)| Line {
                    endpoint: (*endpoint).to_owned(),
                    surface: (*surface).to_owned(),
                    caller: None,
                    count: *count,
                })
                .collect(),
        }
    }

    #[test]
    fn a_post_names_its_first_fault() {
        assert_eq!(check(&post(&[("air", "ngsi-ld", 3)])), Ok(()));
        assert_eq!(
            check(&post(&[("air", "ngsi-ld", 3), ("Air!", "ogc", 1)])),
            Err("counts[1].endpoint: not an Endpoint slug".to_owned())
        );
        assert!(check(&post(&[("air", "graphql", 1)]))
            .is_err_and(|e| e.starts_with("counts[0].surface")));
        assert!(check(&post(&[("air", "ogc", MAX_COUNT + 1)]))
            .is_err_and(|e| e.starts_with("counts[0].count")));
        let many: Vec<(&str, &str, u64)> = vec![("air", "ogc", 1); MAX_LINES + 1];
        assert!(check(&post(&many)).is_err_and(|e| e.starts_with("counts:")));
    }

    /// Two replicas' lines for the same Endpoint add up; an unknown slug stores nothing.
    #[test]
    fn lines_sum_per_row_and_an_unknown_slug_is_dropped() {
        let day = date("2026-10-22");
        let rows = rows(
            &post(&[
                ("air", "ngsi-ld", 2),
                ("air", "ngsi-ld", 5),
                ("ghost", "ogc", 9),
                ("air", "ogc", 0),
            ]),
            day,
            |slug| (slug == "air").then(|| ("city".to_owned(), "air-quality".to_owned())),
        );
        let key = (
            "city".to_owned(),
            "air-quality".to_owned(),
            "ngsi-ld".to_owned(),
            day,
            "unknown".to_owned(),
        );
        assert_eq!(rows, BTreeMap::from([(key, 7)]));
    }

    /// A line names its kind of caller, and the kinds are rows of their own (T-6186).
    #[test]
    fn a_caller_kind_is_checked_and_kept_apart() {
        let read = parse(
            br#"{"counts":[{"endpoint":"air","surface":"feed","caller":"agent","count":2},
                {"endpoint":"air","surface":"feed","caller":"person","count":3}]}"#,
        );
        let day = date("2026-10-22");
        let rows = rows(&read.unwrap_or(Post { counts: vec![] }), day, |_| {
            Some(("city".to_owned(), "air".to_owned()))
        });
        let calls: Vec<(&str, u64)> = rows.iter().map(|(k, n)| (k.4.as_str(), *n)).collect();
        assert_eq!(calls, vec![("agent", 2), ("person", 3)]);
        assert_eq!(
            parse(br#"{"counts":[{"endpoint":"air","surface":"ogc","caller":"ana@city.sk","count":1}]}"#)
                .err(),
            Some(format!("counts[0].caller: one of {}", CALLERS.join(", ")))
        );
    }

    #[test]
    fn a_window_has_every_day_and_its_totals() {
        let (from, to) = window(date("2026-10-22"), 3);
        assert_eq!(from, date("2026-10-20"));
        let stored = |surface: &str, day: &str, calls| Stored {
            name: "air".to_owned(),
            surface: surface.to_owned(),
            day: date(day),
            caller: if surface == "ogc" { "agent" } else { "person" }.to_owned(),
            calls,
        };
        let rows = [
            stored("ngsi-ld", "2026-10-20", 4),
            stored("ogc", "2026-10-20", 1),
            stored("download", "2026-10-22", 2),
            stored("ogc", "2026-10-19", 50),
            Stored {
                name: "other".to_owned(),
                ..stored("ogc", "2026-10-21", 8)
            },
        ];
        let usage = usage("air", from, to, &rows);
        assert_eq!(usage.days.len(), 3);
        assert_eq!(usage.days[0].total, 5);
        assert_eq!(
            usage.days[1],
            Day {
                day: "2026-10-21".to_owned(),
                ..Day::default()
            }
        );
        assert_eq!(usage.days[2].calls.get("download"), Some(&2));
        assert_eq!(usage.total, 7);
        assert_eq!(usage.totals.get("ogc"), Some(&1));
        assert_eq!(
            usage.callers,
            BTreeMap::from([("agent".to_owned(), 1), ("person".to_owned(), 6)])
        );
        assert_eq!(window(date("2026-10-22"), 500).0, date("2026-07-25"));
    }

    #[test]
    fn a_month_is_its_first_and_last_day() {
        assert_eq!(
            month("2026-02"),
            Some((date("2026-02-01"), date("2026-02-28")))
        );
        assert_eq!(
            month("2026-12"),
            Some((date("2026-12-01"), date("2026-12-31")))
        );
        assert_eq!(month("2026-13"), None);
        assert_eq!(month("26-10"), None);
        assert_eq!(month("2026-1"), None);
        assert_eq!(month("2026-10-01"), None);
    }

    #[test]
    fn the_export_has_a_header_and_a_sorted_line_per_row() {
        let rows = [
            Stored {
                name: "b".to_owned(),
                surface: "ogc".to_owned(),
                day: date("2026-10-02"),
                caller: "person".to_owned(),
                calls: 3,
            },
            Stored {
                name: "a".to_owned(),
                surface: "mcp".to_owned(),
                day: date("2026-10-02"),
                caller: "agent".to_owned(),
                calls: 1,
            },
            Stored {
                name: "a".to_owned(),
                surface: "mcp".to_owned(),
                day: date("2026-10-02"),
                caller: "anonymous".to_owned(),
                calls: 4,
            },
            Stored {
                name: "a".to_owned(),
                surface: "ogc".to_owned(),
                day: date("2026-10-01"),
                caller: "person".to_owned(),
                calls: 0,
            },
        ];
        assert_eq!(
            csv(&rows),
            "day,endpoint,surface,calls\n2026-10-02,a,mcp,5\n2026-10-02,b,ogc,3\n"
        );
    }

    #[test]
    fn a_post_is_read_and_checked_whole() {
        let read = parse(br#"{"counts":[{"endpoint":"air","surface":"ogc","count":2}]}"#);
        assert_eq!(read.map(|p| p.counts.len()), Ok(1));
        assert_eq!(
            parse(br#"{"counts":[{"endpoint":"Air!","surface":"ogc","count":2}]}"#).err(),
            Some("counts[0].endpoint: not an Endpoint slug".to_owned())
        );
        assert!(parse(b"{\"counts\":[],\"extra\":1}")
            .is_err_and(|e| e.starts_with("the usage post is not readable")));
    }

    /// The store holds names, a day and a number; it has no column a person could be in.
    #[tokio::test]
    async fn the_memory_store_adds_reads_and_forgets_after_its_keep() {
        let store = UsageStore::new(None);
        let today = date("2026-10-22");
        let key = |day| {
            (
                "city".to_owned(),
                "air".to_owned(),
                "ogc".to_owned(),
                day,
                "person".to_owned(),
            )
        };
        let old = today - Duration::days(KEEP_DAYS + 1);
        store
            .add(BTreeMap::from([(key(today), 2), (key(old), 9)]), today)
            .await
            .ok();
        store
            .add(BTreeMap::from([(key(today), 3)]), today)
            .await
            .ok();
        let read = store.read("city", old, today).await.unwrap_or_default();
        assert_eq!(
            read,
            vec![Stored {
                name: "air".to_owned(),
                surface: "ogc".to_owned(),
                day: today,
                caller: "person".to_owned(),
                calls: 5
            }]
        );
        assert!(store
            .read("elsewhere", old, today)
            .await
            .unwrap_or_default()
            .is_empty());
    }
}
