//! What an answer shows of what it read (T-7504, API/05 §1.3): the entities that answer a
//! superlative and no page of their type, a count with its split over one attribute, a history as
//! at most [`POINTS`] aggregated points, and how many of all a map shows. Code decides each from
//! the read's own arguments and answer; the router only names the attribute.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::agent::{Ask, Connected, Table, Words};
use super::routed;

/// The most entities a question for the most or the least of something is answered with, ties
/// with the last one included.
pub const TOP_MAX: usize = 20;
/// The most points one chart is sent.
pub const POINTS: usize = 500;
/// The most series one history chart draws.
const SERIES: usize = 8;
/// The categories a split names; the rest are one row.
const GROUPS: usize = 8;
/// The categories the model is given to say; the rest are one number. The chart's rest is past
/// [`GROUPS`], a smaller number, so its bar is named for that and never "others" too.
const SAID_GROUPS: usize = 3;
/// Past this many categories, names that share their head before `, ` or ` - ` count as one
/// (an organisation's regional branches).
const FOLD_PAST: usize = 8;
/// The fewest words of a head names fold to: "Košice - okolie" is no branch of "Košice".
const HEAD_WORDS: usize = 3;
/// A year column of more distinct years than this is counted per decade.
const YEARS: usize = 10;
/// The years before this are one bar, so a register's 1000 and 1700 draw no axis of centuries.
const OLD_YEAR: i64 = 1800;
/// A date column of more months than this is counted per year.
const MONTHS: usize = 24;
/// A history over this many days is drawn per day, never per hour.
const DAILY_FROM: usize = 3;

/// The pages of one split, and how long they may take beside the rows.
// ponytail: a scan of one attribute, about 5,000-10,000 rows in the deadline; the broker's own
// group-by on MCP when a register is larger (until then the count says "N of TOTAL").
const SCAN_PAGES: usize = 12;
const SCAN_DEADLINE: Duration = Duration::from_secs(4);
const SCAN_PAGE: usize = 1_000;

/// The categories code names itself, with their words.
const OTHER: &str = "_other";
const NONE: &str = "_none";
const BEFORE: &str = "_before";

/// The view the router asked for one read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    List,
    /// The first this many by the read's `order`.
    Top(usize),
    /// How many there are, split by the attribute when one is named.
    Count(Option<String>),
}

/// What a table's numbers are when code computed them, as the `data` event carries it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Aggregation {
    /// `count`, or the NGSI-LD `aggrMethods` name of a history.
    pub method: String,
    /// `value`, `year`, `decade` or `month` of a count; `year`, `month`, `day`, `hour` or
    /// `period` of a history.
    pub per: &'static str,
    /// The attribute it is over.
    pub of: String,
    /// The entities counted, or the series drawn, and how many there are.
    pub shown: u64,
    pub total: u64,
    /// Only part of the period asked is in it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The days with a value and the days of the window asked, of one daily series.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days: Option<(u64, u64)>,
    /// The first and the last year in a history per month or per year (T-7526).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub years: Option<(i32, i32)>,
}

/// Whether the question itself asks for the most or the least of something, for a router reply
/// that ranks and names no `top`.
// ponytail: word lists of Slovak, Czech and English; another language relies on the router's `top`.
fn superlative(question: &str) -> bool {
    const EN: &[&str] = &[
        "longest", "shortest", "oldest", "newest", "youngest", "largest", "biggest", "smallest",
        "highest", "lowest", "nearest", "closest", "busiest", "earliest", "latest", "tallest",
        "widest", "most", "least",
    ];
    question
        .to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .any(|w| {
            EN.contains(&w)
                || (w.chars().count() > 6 && (w.starts_with("naj") || w.starts_with("nej")))
        })
}

/// The view of one `data` item of the router's reply; `order` is the order kept for it, and a
/// split only names an attribute the Endpoint is shown with.
pub fn wanted(read: &Value, order: Option<&str>, attributes: &[String], question: &str) -> View {
    if order.is_some() {
        let top = read.get("top").and_then(Value::as_u64).filter(|n| *n > 0);
        return match top {
            Some(n) => View::Top(usize::try_from(n).unwrap_or(TOP_MAX).min(TOP_MAX)),
            None if superlative(question) => View::Top(1),
            None => View::List,
        };
    }
    let group = read
        .get("group")
        .and_then(Value::as_str)
        .filter(|g| attributes.iter().any(|a| a == g));
    if group.is_some() || read.get("count") == Some(&json!(true)) {
        return View::Count(group.map(str::to_owned));
    }
    View::List
}

/// The attribute of an `orderBy` and whether it is descending; `None` for more than one.
fn order_of(order: &str) -> Option<(&str, bool)> {
    let (name, direction) = order.split_once(';').unwrap_or((order, "asc"));
    (routed::is_type_name(name) && matches!(direction, "asc" | "desc"))
        .then_some((name, direction == "desc"))
}

fn both(q: Option<&str>, more: &str) -> String {
    match q.filter(|q| !q.is_empty()) {
        Some(q) => format!("({q});{more}"),
        None => more.to_owned(),
    }
}

fn offers(schema: &Value, argument: &str) -> bool {
    schema.pointer(&format!("/properties/{argument}")).is_some()
}

/// The arguments a ranked read is sent with: descending, the broker lists the entities without
/// the attribute first, so the read keeps those that have it.
pub fn ranked_arguments(schema: &Value, arguments: &Value) -> Value {
    let mut sent = arguments.clone();
    let order = arguments.get("orderBy").and_then(Value::as_str);
    if let (Some((name, true)), true) = (order.and_then(order_of), offers(schema, "q")) {
        let q = arguments.get("q").and_then(Value::as_str);
        sent["q"] = json!(both(q, name));
    }
    sent
}

/// How many entities a model's `query_entities` asks to rank: an `orderBy` with a small `limit`
/// on the first page.
pub fn top_of(arguments: &Value) -> Option<usize> {
    order_of(arguments.get("orderBy")?.as_str()?)?;
    if arguments.get("cursor").is_some() || arguments.get("offset").is_some() {
        return None;
    }
    let limit = usize::try_from(arguments.get("limit")?.as_u64()?).ok()?;
    (1..=TOP_MAX).contains(&limit).then_some(limit)
}

/// Whether a model's read, as it wrote it, only counts a whole type: no filter and no page of
/// rows asked (the loop's rules count without a `limit`).
pub fn counts_all(arguments: &Value) -> bool {
    arguments.get("count") == Some(&json!(true))
        && arguments.get("orderBy").is_none()
        && ["q", "id", "idPattern", "georel", "scopeQ"]
            .iter()
            .all(|filter| arguments.get(*filter).is_none())
        && arguments
            .get("limit")
            .is_none_or(|limit| limit.as_u64().is_some_and(|limit| limit <= 1))
}

/// Cuts a ranked table to the `top` rows that answer and those tied with the last of them. The
/// placeholder rows are set aside for the answer to name, never shown as the answer (T-4803).
pub fn keep_top(table: &mut Table, order: &str, top: usize) {
    let placeholders: Vec<bool> = table
        .rows
        .iter()
        .map(|row| routed::placeholder(&table.columns, &table.units, row))
        .collect();
    let real = placeholders.iter().filter(|p| !**p).count();
    if real == 0 {
        return;
    }
    // Real rows first, in the order read (`placeholders_last`).
    let mut keep = top.clamp(1, TOP_MAX).min(real);
    let column = order_of(order).and_then(|(name, _)| table.columns.iter().position(|c| c == name));
    if let Some(column) = column {
        let edge = table.rows[keep - 1].get(column).cloned();
        if edge.as_ref().is_some_and(|value| !value.is_null()) {
            while keep < real.min(TOP_MAX) && table.rows[keep].get(column) == edge.as_ref() {
                keep += 1;
            }
        }
    }
    table.set_aside = table.rows.split_off(real);
    table.rows.truncate(keep);
    table.geometries.truncate(keep);
    table.top = Some(keep);
}

/// One more count of a read with `more` beside its `q`: the same Endpoint, type and filters.
async fn count_of(
    ask: &Ask<'_>,
    connector: &Connected,
    schema: &Value,
    arguments: &Value,
    more: Option<&str>,
) -> Option<u64> {
    if !offers(schema, "count") || (more.is_some() && !offers(schema, "q")) {
        return None;
    }
    let mut asked = arguments.clone();
    let fields = asked.as_object_mut()?;
    for page in [
        "orderBy", "cursor", "offset", "pick", "attrs", "omit", "format",
    ] {
        fields.remove(page);
    }
    fields.insert("count".into(), json!(true));
    let least = schema
        .pointer("/properties/limit/minimum")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    fields.insert("limit".into(), json!(least));
    if let Some(more) = more {
        let q = both(arguments.get("q").and_then(Value::as_str), more);
        fields.insert("q".into(), json!(q));
    }
    super::standard::fits(schema, &asked).ok()?;
    let began = Instant::now();
    let answered = tokio::time::timeout(
        SCAN_DEADLINE,
        connector
            .surface
            .call(ask.http, "query_entities", asked.clone()),
    )
    .await
    .ok()?;
    let answered = routed::without_personal(ask, connector, answered).await;
    ask.trace.tool(
        "query_entities",
        Some(&connector.endpoint),
        &asked,
        began,
        &answered,
    );
    routed::total_of(&answered.ok()?)
}

/// What a read's table shows, once its rows are in: the top rows with the count of all they were
/// ranked among, and for a map how many of all have a location. `arguments` are the read as
/// asked, `sent` as [`ranked_arguments`] sent it.
pub async fn finish(
    ask: &Ask<'_>,
    connector: &Connected,
    schema: &Value,
    table: &mut Table,
    (arguments, sent): (&Value, &Value),
    top: Option<usize>,
) {
    if let (Some(top), Some(order)) = (top, arguments.get("orderBy").and_then(Value::as_str)) {
        keep_top(table, order, top);
    }
    let domain = ask.viz.and_then(super::viz::domain);
    let placed = table.geometries.iter().any(Option::is_some);
    // With every entity on the table its rows say who has no place; else the broker counts them.
    let part = table.top.is_some()
        || table
            .total
            .is_none_or(|total| total > table.rows.len() as u64);
    let (all, located) = tokio::join!(
        async {
            // The map says "of all", so a read without a count is counted.
            if arguments == sent && (table.total.is_some() || !placed) {
                return None;
            }
            count_of(ask, connector, schema, arguments, None).await
        },
        async {
            if !(part && placed && domain.is_some()) {
                return None;
            }
            count_of(ask, connector, schema, arguments, Some("location")).await
        }
    );
    table.total = all.or(table.total);
    table.located = located;
    table.map = domain
        .filter(|_| !table.counted)
        .and_then(|domain| super::viz::default_map(table, domain));
}

/// A value as a category reads it: a scalar, a LanguageProperty's text, an address's town.
fn category(value: &Value) -> Option<Value> {
    let value = match value.get("value") {
        Some(inner) => inner,
        None => value,
    };
    let text = match value {
        Value::Number(_) => return Some(value.clone()),
        Value::Bool(b) => return Some(json!(b.to_string())),
        Value::String(text) => Some(text.as_str()),
        Value::Object(map) => {
            let words = map.get("languageMap").and_then(Value::as_object);
            words
                .and_then(|w| {
                    w.get("sk")
                        .or_else(|| w.get("en"))
                        .or_else(|| w.values().next())
                })
                .or_else(|| map.get("addressLocality"))
                .and_then(Value::as_str)
        }
        _ => None,
    };
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| json!(text))
}

/// The head a long name folds to, when it is a name of [`HEAD_WORDS`] words or more.
fn head(name: &str) -> &str {
    let cut = [", ", " - ", " – "]
        .iter()
        .filter_map(|mark| name.find(mark))
        .min()
        .and_then(|at| name.get(..at));
    cut.filter(|head| head.split_whitespace().count() >= HEAD_WORDS)
        .unwrap_or(name)
}

/// Every period from the first to the last, the empty ones as 0, so an axis of time has no gap
/// drawn as no distance; as counted when that would be more than a chart draws.
fn run(mut counts: BTreeMap<i64, u64>, step: i64) -> BTreeMap<i64, u64> {
    if let (Some(&first), Some(&last)) = (counts.keys().next(), counts.keys().next_back()) {
        if (last - first) / step < POINTS as i64 {
            let mut at = first;
            while at < last {
                counts.entry(at).or_insert(0);
                at += step;
            }
        }
    }
    counts
}

fn tally<T: Ord>(items: impl Iterator<Item = T>) -> BTreeMap<T, u64> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item).or_default() += 1;
    }
    counts
}

/// A split: what each bar is, and each bar with its count, the rows without a value last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    pub per: &'static str,
    pub rows: Vec<(String, u64)>,
}

/// `values` of `attribute` counted over one attribute, so the parts sum to the entities counted;
/// `None` when it splits nothing a person can read (one value, or a measured quantity).
pub fn split(attribute: &str, values: &[Option<Value>]) -> Option<Split> {
    let present: Vec<&Value> = values.iter().flatten().collect();
    let numbers: Vec<i64> = present.iter().filter_map(|v| v.as_i64()).collect();
    let texts: Vec<&str> = present.iter().filter_map(|v| v.as_str()).collect();
    let year = attribute.to_lowercase().contains("year")
        && numbers.len() == present.len()
        && numbers.iter().all(|y| (1..=2999).contains(y));
    let dated = texts.len() == present.len()
        && texts
            .iter()
            .all(|t| t.len() >= 10 && t.is_char_boundary(10) && is_day(&t[..10]));

    let (per, mut rows): (&'static str, Vec<(String, u64)>) = if year {
        let distinct: BTreeSet<i64> = numbers.iter().copied().collect();
        let span = distinct
            .last()
            .zip(distinct.first())
            .map_or(0, |(b, a)| b - a);
        let decades = distinct.len() > YEARS || span > 3 * YEARS as i64;
        let (old, new): (Vec<i64>, Vec<i64>) =
            numbers.iter().partition(|&&y| decades && y < OLD_YEAR);
        let step = if decades { 10 } else { 1 };
        let counts = run(tally(new.iter().map(|y| y - y % step)), step);
        let mut named: Vec<(String, u64)> = counts
            .into_iter()
            .map(|(from, n)| match decades {
                true => (format!("{from}–{}", from + 9), n),
                false => (from.to_string(), n),
            })
            .collect();
        if !old.is_empty() {
            named.insert(0, (BEFORE.to_owned(), old.len() as u64));
        }
        (if decades { "decade" } else { "year" }, named)
    } else if dated {
        // `is_day` left digits alone where the year and the month are.
        let number = |text: &str| text.parse::<i64>().unwrap_or_default();
        let at: Vec<(i64, i64)> = texts
            .iter()
            .map(|t| (number(&t[..4]), number(&t[5..7]).clamp(1, 12) - 1))
            .collect();
        let months = run(tally(at.iter().map(|(y, m)| y * 12 + m)), 1);
        if months.len() > MONTHS {
            let years = run(tally(at.iter().map(|(y, _)| *y)), 1);
            let named = years.into_iter().map(|(y, n)| (format!("{y:04}"), n));
            ("year", named.collect())
        } else {
            let named = months
                .into_iter()
                .map(|(m, n)| (format!("{:04}-{:02}", m / 12, m % 12 + 1), n));
            ("month", named.collect())
        }
    } else {
        let mut names: Vec<String> = present
            .iter()
            .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
            .collect();
        let distinct = names.iter().collect::<BTreeSet<_>>().len();
        // A measured quantity has no categories.
        if numbers.len() != present.len() && texts.is_empty() {
            return None;
        }
        if texts.is_empty() && distinct > GROUPS {
            return None;
        }
        if distinct > FOLD_PAST {
            for name in &mut names {
                *name = head(name).to_owned();
            }
        }
        let mut counts: Vec<(String, u64)> = tally(names.into_iter()).into_iter().collect();
        // A name per row (a code, a title) splits nothing.
        if counts.len() > GROUPS && counts.len() * 2 > present.len() {
            return None;
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if counts.len() > GROUPS + 1 {
            let rest = counts.split_off(GROUPS).iter().map(|(_, n)| n).sum();
            counts.push((OTHER.to_owned(), rest));
        }
        ("value", counts)
    };
    if rows.len() < 2 {
        return None;
    }
    let without = (values.len() - present.len()) as u64;
    if without > 0 {
        rows.push((NONE.to_owned(), without));
    }
    Some(Split { per, rows })
}

fn is_day(text: &str) -> bool {
    text.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 => b == b'-',
        _ => b.is_ascii_digit(),
    })
}

/// Reads one attribute of every entity the read matches, page after page, and counts it. Read
/// with the same tool, Endpoint and `q` as the rows; a person's read still withholds what their
/// model marks personal, for which each entity keeps its id and type.
pub async fn counted(
    ask: &Ask<'_>,
    connector: &Connected,
    schema: &Value,
    arguments: &Value,
    attribute: &str,
) -> Option<(Split, u64)> {
    let mut page = json!({"type": arguments.get("type")?, "limit": SCAN_PAGE});
    if let Some(q) = arguments.get("q") {
        page["q"] = q.clone();
    }
    if offers(schema, "format") {
        page["format"] = json!("keyValues");
    }
    let pick = if connector.surface.token.is_some() {
        json!(["id", "type", attribute])
    } else {
        json!([attribute])
    };
    if offers(schema, "pick") {
        page["pick"] = pick;
    } else if offers(schema, "attrs") {
        page["attrs"] = json!([attribute]);
    } else {
        return None;
    }
    let began = Instant::now();
    let mut values: Vec<Option<Value>> = Vec::new();
    for _ in 0..SCAN_PAGES {
        if super::standard::fits(schema, &page).is_err() {
            break;
        }
        let Ok(answered) = tokio::time::timeout(
            SCAN_DEADLINE.saturating_sub(began.elapsed()),
            connector
                .surface
                .call(ask.http, "query_entities", page.clone()),
        )
        .await
        else {
            break;
        };
        let Ok(text) = routed::without_personal(ask, connector, answered).await else {
            break;
        };
        let Ok(answer) = serde_json::from_str::<Value>(&text) else {
            break;
        };
        let Some(entities) = answer.get("entities").and_then(Value::as_array) else {
            break;
        };
        values.extend(entities.iter().map(|e| e.get(attribute).and_then(category)));
        match answer.get("nextCursor") {
            Some(cursor) if !entities.is_empty() && offers(schema, "cursor") => {
                page["cursor"] = cursor.clone();
            }
            _ => break,
        }
    }
    let counted = format!("{} rows counted by {attribute}", values.len());
    ask.trace.note("tool", "split", began, None, Some(&counted));
    Some((split(attribute, &values)?, values.len() as u64))
}

fn words(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(lang, text)| ((*lang).to_owned(), (*text).to_owned()))
        .collect()
}

/// Turns a count's table into its split: a row per category with its count, no map and no row
/// of the type.
pub fn show_split(table: &mut Table, attribute: &str, split: &Split, counted: u64) {
    let at = table.columns.iter().position(|c| c == attribute);
    let label = at.and_then(|i| table.labels.get(i).cloned().flatten());
    let mut named: Words = at
        .and_then(|i| table.enums.get(i).cloned().flatten())
        .unwrap_or_default();
    for (key, _) in &split.rows {
        let said: &[(&str, &str)] = match key.as_str() {
            OTHER => &[
                ("sk", "Mimo 8 najväčších"),
                ("cs", "Mimo 8 největších"),
                ("en", "Outside the top 8"),
                ("de", "Außerhalb der 8 größten"),
                ("fi", "8 suurimman ulkopuolella"),
            ],
            NONE => &[
                ("sk", "Neuvedené"),
                ("cs", "Neuvedeno"),
                ("en", "Not given"),
                ("de", "Ohne Angabe"),
                ("fi", "Ei ilmoitettu"),
            ],
            BEFORE => &[
                ("sk", "<1800, neoverené"),
                ("cs", "<1800, neověřené"),
                ("en", "<1800, unverified"),
                ("de", "<1800, ungeprüft"),
                ("fi", "<1800, vahvistamaton"),
            ],
            _ => continue,
        };
        named.insert(key.clone(), words(said));
    }
    table.columns = vec![attribute.to_owned(), "count".to_owned()];
    table.rows = split
        .rows
        .iter()
        .map(|(key, n)| vec![json!(key), json!(n)])
        .collect();
    table.units = vec![None, None];
    table.labels = vec![
        label,
        Some(words(&[
            ("sk", "Počet"),
            ("cs", "Počet"),
            ("en", "Count"),
            ("de", "Anzahl"),
            ("fi", "Määrä"),
        ])),
    ];
    table.enums = vec![(!named.is_empty()).then_some(named), None];
    table.summary = Vec::new();
    table.geometries = Vec::new();
    table.geojson = None;
    table.map = None;
    table.profile = None;
    table.counted = false;
    table.aggregation = Some(Aggregation {
        method: "count".into(),
        per: split.per,
        of: attribute.to_owned(),
        shown: counted,
        total: table.total.unwrap_or(counted).max(counted),
        partial: false,
        days: None,
        years: None,
    });
}

/// The split as the answer call says it: the largest parts and the rest as one number, all over
/// one attribute, so they sum to what was counted. `None` for a table that is no count.
pub fn said(table: &Table) -> Option<Value> {
    let made = table.aggregation.as_ref().filter(|a| a.method == "count")?;
    let name = |key: &Value| {
        let words = key
            .as_str()
            .and_then(|key| table.enums.first()?.as_ref()?.get(key));
        match words {
            Some(words) => json!(words),
            None => key.clone(),
        }
    };
    let mut parts: Vec<(&Value, u64)> = table
        .rows
        .iter()
        .filter_map(|row| Some((row.first()?, row.get(1)?.as_u64()?)))
        .filter(|(key, _)| !matches!(key.as_str(), Some(OTHER | NONE)))
        .collect();
    parts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    parts.truncate(SAID_GROUPS);
    let named: u64 = parts.iter().map(|(_, n)| n).sum();
    let mut said = json!({
        "by": made.of,
        "per": made.per,
        "parts": parts.iter().map(|(key, n)| json!([name(key), n])).collect::<Vec<_>>(),
        "other": made.shown.saturating_sub(named),
        "counted": made.shown,
    });
    if made.shown < made.total {
        said["of"] = json!(made.total);
    }
    Some(said)
}

/// The arguments a history is sent with: a window of [`DAILY_FROM`] days or more that averages
/// per less than a day is asked per day, and a longer one no finer than its range is read by
/// (`jc_time::granularity`, T-7526), so the broker aggregates it before it is sent on.
pub fn history_arguments(schema: &Value, arguments: &Value, now: OffsetDateTime) -> Value {
    let mut sent = arguments.clone();
    let text = |key: &str| arguments.get(key).and_then(Value::as_str);
    let instant = |key: &str| text(key).and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok());
    let span = match text("timerel") {
        Some("between") => instant("endTimeAt")
            .zip(instant("timeAt"))
            .map(|(b, a)| b - a),
        Some("after") => instant("timeAt").map(|a| now - a),
        _ => None,
    };
    let long = span.is_some_and(|span| span >= time::Duration::days(DAILY_FROM as i64));
    let finer = text("aggrPeriodDuration")
        .is_some_and(|period| period.starts_with("PT") && period != "PT0S" && period != "PT24H");
    if arguments.get("aggrMethods").is_none() || !offers(schema, "aggrPeriodDuration") {
        return sent;
    }
    let Some(asked) = text("aggrPeriodDuration") else {
        return sent;
    };
    let mut period = if long && finer { "P1D" } else { asked };
    if let Some(span) = span {
        let span = jc_time::granularity::SignedDuration::from_secs(span.whole_seconds());
        period = jc_time::granularity::bounded(span, period);
    }
    sent["aggrPeriodDuration"] = json!(period);
    sent
}

/// The UTC days a `between` window touches; the moment it ends at is outside it.
fn window_days(arguments: &Value) -> Option<u64> {
    if arguments.get("timerel").and_then(Value::as_str) != Some("between") {
        return None;
    }
    let at = |key: &str| {
        let text = arguments.get(key).and_then(Value::as_str)?;
        let at = OffsetDateTime::parse(text, &Rfc3339).ok()?;
        Some(at.to_offset(UtcOffset::UTC))
    };
    let last = at("endTimeAt")?.checked_sub(time::Duration::SECOND)?;
    let days = last.date().to_julian_day() - at("timeAt")?.date().to_julian_day() + 1;
    u64::try_from(days).ok()
}

/// The calendar a history of days or longer is said and drawn by, finest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Calendar {
    Day,
    Month,
    Year,
}

impl Calendar {
    /// The calendar of an ISO 8601 period, a month or a year whatever its day count.
    fn of(period: &str) -> Self {
        const DAY: i64 = 86_400;
        match jc_time::granularity::seconds(period) {
            Some(secs) if secs >= 365 * DAY => Self::Year,
            Some(secs) if secs >= 28 * DAY => Self::Month,
            _ => Self::Day,
        }
    }

    fn per(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The first moment of the day, the month or the year holding `at`.
    fn start(self, at: OffsetDateTime) -> Option<OffsetDateTime> {
        let date = match self {
            Self::Day => at.date(),
            Self::Month => at.date().replace_day(1).ok()?,
            Self::Year => {
                time::Date::from_calendar_date(at.year(), time::Month::January, 1).ok()?
            }
        };
        Some(at.replace_date(date).replace_time(time::Time::MIDNIGHT))
    }
}

/// The calendar a history at `instants` is read by: no finer than the table's for its span, the
/// period the broker `aggregated` by, or the one its moments already keep.
fn calendar(instants: &BTreeSet<OffsetDateTime>, aggregated: bool, arguments: &Value) -> Calendar {
    let (Some(first), Some(last)) = (instants.first(), instants.last()) else {
        return Calendar::Day;
    };
    let span = jc_time::granularity::SignedDuration::from_secs((*last - *first).whole_seconds());
    let mut read = Calendar::of(jc_time::granularity::finest(span));
    if aggregated {
        if let Some(period) = arguments.get("aggrPeriodDuration").and_then(Value::as_str) {
            read = read.max(Calendar::of(period));
        }
    }
    if instants.len() > 1 {
        let kept = [Calendar::Year, Calendar::Month]
            .into_iter()
            .find(|calendar| instants.iter().all(|at| calendar.start(*at) == Some(*at)));
        read = read.max(kept.unwrap_or(Calendar::Day));
    }
    read
}

/// One attribute's history of one entity: its numbers by instant.
struct Series {
    id: String,
    points: Vec<(OffsetDateTime, f64)>,
}

/// A history read as a table of at most [`POINTS`] points, with the method that made them.
#[derive(Debug, Clone, PartialEq)]
pub struct History {
    pub ids: Vec<String>,
    /// The entity's place in `ids`, the moment (a UTC day or an instant) and the value.
    pub rows: Vec<(usize, String, f64)>,
    pub made: Aggregation,
}

/// The NGSI-LD methods a history is drawn of, the first one present wins.
const METHODS: &[&str] = &["avg", "sum", "max", "min", "totalCount"];

/// The method the broker aggregated by, and each instant's number.
type Points = (Option<&'static str>, Vec<(OffsetDateTime, f64)>);

/// The numbers of one attribute's history in any of the three temporal forms, with the method
/// the broker aggregated them by.
fn points(attribute: &Value) -> Option<Points> {
    let pair = |value: &Value, at: &Value| {
        let at = OffsetDateTime::parse(at.as_str()?, &Rfc3339).ok()?;
        Some((at.to_offset(UtcOffset::UTC), value.as_f64()?))
    };
    let tuples = |list: &Value| -> Option<Vec<(OffsetDateTime, f64)>> {
        let found: Vec<_> = list
            .as_array()?
            .iter()
            .filter_map(|t| pair(t.get(0)?, t.get(1)?))
            .collect();
        (!found.is_empty()).then_some(found)
    };
    if let Some(node) = attribute.as_object() {
        for &method in METHODS {
            if let Some(found) = node.get(method).and_then(tuples) {
                return Some((Some(method), found));
            }
        }
        if let Some(found) = node.get("values").and_then(tuples) {
            return Some((None, found));
        }
    }
    let instances = match attribute {
        Value::Array(list) => list.iter().collect(),
        one => vec![one],
    };
    let found: Vec<_> = instances
        .into_iter()
        .filter_map(|i| pair(i.get("value")?, i.get("observedAt")?))
        .collect();
    (!found.is_empty()).then_some((None, found))
}

/// A temporal answer as the points a chart draws: one attribute, per day from [`DAILY_FROM`]
/// days on, per month or year where [`calendar`] reads so or the points would pass [`POINTS`],
/// at most [`SERIES`] series; only past a year per point are the newest alone kept.
pub fn history(text: &str, arguments: &Value) -> Option<History> {
    let answer: Value = serde_json::from_str(text).ok()?;
    let entities: Vec<&Value> = match answer.get("entities").unwrap_or(&answer) {
        Value::Array(list) => list.iter().collect(),
        one => vec![one],
    };
    let asked: Vec<&str> = ["attrs", "pick"]
        .iter()
        .filter_map(|key| arguments.get(*key)?.as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // The attribute asked first, else the one with the most numbers.
    let mut found: BTreeMap<&str, (Option<&'static str>, Vec<Series>)> = BTreeMap::new();
    for entity in &entities {
        let (Some(id), Some(fields)) =
            (entity.get("id").and_then(Value::as_str), entity.as_object())
        else {
            continue;
        };
        for (name, attribute) in fields {
            if matches!(name.as_str(), "id" | "type" | "@context" | "scope") {
                continue;
            }
            if let Some((method, points)) = points(attribute) {
                let (kept, series) = found.entry(name).or_insert((method, Vec::new()));
                if *kept == method {
                    series.push(Series {
                        id: id.to_owned(),
                        points,
                    });
                }
            }
        }
    }
    let (attribute, (method, mut series)) =
        found.into_iter().min_by_key(|(name, (_, series))| {
            let place = asked.iter().position(|a| a == name).unwrap_or(asked.len());
            let size: usize = series.iter().map(|s| s.points.len()).sum();
            (place, std::cmp::Reverse(size))
        })?;

    let days: BTreeSet<time::Date> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|(at, _)| at.date()))
        .collect();
    let instants: BTreeSet<OffsetDateTime> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|(at, _)| *at))
        .collect();
    let asked_per = calendar(&instants, method.is_some(), arguments);
    let dated = days.len() >= DAILY_FROM || asked_per > Calendar::Day;
    let hourly = instants
        .iter()
        .all(|at| at.minute() == 0 && at.second() == 0);
    // Values the broker did not aggregate are averaged here, and said so.
    let method = method.unwrap_or("avg");
    let all = series.len();
    let total = answer
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .max(all as u64);
    series.truncate(SERIES);
    let most = POINTS / series.len().max(1);
    let fits = |read: &Calendar| {
        series.iter().all(|one| {
            let starts: BTreeSet<_> = one.points.iter().map(|(at, _)| read.start(*at)).collect();
            starts.len() <= most
        })
    };
    let read = dated.then(|| {
        [Calendar::Day, Calendar::Month, Calendar::Year]
            .into_iter()
            .filter(|read| *read >= asked_per)
            .find(fits)
            .unwrap_or(Calendar::Year)
    });
    let per = match read {
        Some(read) => read.per(),
        None if hourly => "hour",
        None => "period",
    };
    let mut partial = answer.get("more").is_some();
    let mut rows = Vec::new();
    for (at, one) in series.iter().enumerate() {
        let mut buckets: BTreeMap<OffsetDateTime, Vec<f64>> = BTreeMap::new();
        for (instant, value) in &one.points {
            let key = match read {
                Some(read) => read.start(*instant)?,
                None => *instant,
            };
            buckets.entry(key).or_default().push(*value);
        }
        let older = buckets.len().saturating_sub(most);
        partial |= older > 0;
        for (instant, values) in buckets.into_iter().skip(older) {
            let value = match method {
                "sum" | "totalCount" => values.iter().sum(),
                "max" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                "min" => values.iter().copied().fold(f64::INFINITY, f64::min),
                _ => values.iter().sum::<f64>() / values.len() as f64,
            };
            let moment = if read.is_some() {
                instant.date().to_string()
            } else {
                instant.format(&Rfc3339).ok()?
            };
            rows.push((at, moment, (value * 1e4).round() / 1e4));
        }
    }
    let years = read
        .filter(|read| *read > Calendar::Day)
        .and(instants.first().zip(instants.last()))
        .map(|(first, last)| (first.year(), last.year()));
    let days = (read == Some(Calendar::Day) && series.len() == 1)
        .then(|| window_days(arguments))
        .flatten()
        .map(|asked| (rows.len() as u64, asked.max(rows.len() as u64)));
    Some(History {
        ids: series.into_iter().map(|s| s.id).collect(),
        rows,
        made: Aggregation {
            method: method.to_owned(),
            per,
            of: attribute.to_owned(),
            shown: all.min(SERIES) as u64,
            total,
            partial,
            days,
            years,
        },
    })
}

/// A history's table: the place, the moment and the value, named and with its unit from the
/// entities' current state `current` when that could be read.
pub fn history_table(endpoint: &str, kind: &str, history: &History, current: &str) -> Table {
    let entities: Vec<Value> = serde_json::from_str::<Value>(current)
        .ok()
        .and_then(|v| v.get("entities").cloned())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let of = |id: &str| {
        entities
            .iter()
            .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
    };
    let names: Vec<Value> = history
        .ids
        .iter()
        .map(|id| {
            of(id)
                .and_then(|e| e.get("name"))
                .and_then(category)
                .unwrap_or_else(|| json!(id.rsplit(':').next().unwrap_or(id)))
        })
        .collect();
    let unit = history
        .ids
        .iter()
        .find_map(|id| routed::unit_of(of(id)?.get(&history.made.of)?));
    let moment = if matches!(history.made.per, "day" | "month" | "year") {
        "date"
    } else {
        "observedAt"
    };
    Table {
        endpoint: endpoint.to_owned(),
        entity_type: kind.to_owned(),
        columns: vec![
            "name".to_owned(),
            moment.to_owned(),
            history.made.of.clone(),
        ],
        rows: history
            .rows
            .iter()
            .map(|(at, when, value)| vec![names[*at].clone(), json!(when), json!(value)])
            .collect(),
        units: vec![None, None, unit],
        aggregation: Some(history.made.clone()),
        ..Table::default()
    }
}

/// A temporal read's table, its places named by one read of the same entities.
pub async fn history_of(
    ask: &Ask<'_>,
    connector: &Connected,
    (kind, text, arguments): (&str, &str, &Value),
    left: Duration,
) -> Option<Table> {
    let history = history(text, arguments)?;
    let mut current = String::new();
    if let Some(tool) = connector.tools.iter().find(|t| t.name == "query_entities") {
        let schema = &tool.input_schema;
        let mut read = json!({"type": [kind], "id": history.ids, "limit": history.ids.len()});
        if offers(schema, "pick") {
            read["pick"] = json!(["id", "type", "name", history.made.of]);
        }
        if super::standard::fits(schema, &read).is_ok() {
            let began = Instant::now();
            if let Ok(answered) = tokio::time::timeout(
                left.min(SCAN_DEADLINE),
                connector
                    .surface
                    .call(ask.http, "query_entities", read.clone()),
            )
            .await
            {
                let answered = routed::without_personal(ask, connector, answered).await;
                ask.trace.tool(
                    "query_entities",
                    Some(&connector.endpoint),
                    &read,
                    began,
                    &answered,
                );
                current = answered.unwrap_or_default();
            }
        }
    }
    Some(history_table(&connector.endpoint, kind, &history, &current))
}

/// Whether a table is an answer by itself rather than a page of its type.
fn focused(table: &Table) -> bool {
    table.top.is_some() || table.aggregation.is_some() || table.filter.is_some()
}

/// The tables an answer shows of those it read, `cited` the numbers it cites: never the page
/// behind a count, only the answering rows beside a superlative, and no page of a type whose
/// answering rows are cited. With none cited every table, as beside an error (T-4741).
pub fn selected<'a>(tables: &'a [(usize, Table)], cited: &BTreeSet<usize>) -> Vec<&'a Table> {
    let answering: Vec<&Table> = tables
        .iter()
        .filter(|(n, table)| cited.contains(n) && focused(table))
        .map(|(_, table)| table)
        .collect();
    let superlative = answering.iter().any(|table| table.top.is_some());
    tables
        .iter()
        .filter(|(n, table)| {
            if table.counted {
                return false;
            }
            if superlative {
                return cited.contains(n) && focused(table);
            }
            focused(table)
                || !answering
                    .iter()
                    .any(|a| a.endpoint == table.endpoint && a.entity_type == table.entity_type)
        })
        .map(|(_, table)| table)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The register as measured on 2026-11-22: 1,651 bridges, their managers' own names.
    fn managers() -> Vec<Option<Value>> {
        let groups: [(&str, usize); 8] = [
            (
                "Banskobystrická regionálna správa ciest, a.s., Oblasť prevádzky JUH",
                393,
            ),
            (
                "Banskobystrická regionálna správa ciest, a.s., Oblasť prevádzky STRED",
                329,
            ),
            (
                "Banskobystrická regionálna správa ciest, a.s., Oblasť prevádzky SEVER",
                315,
            ),
            ("Slovenská správa ciest - IVSC Banská Bystrica", 421),
            ("Slovenská správa ciest - IVSC Žilina", 1),
            ("Národná diaľničná spoločnosť, a.s. - Stredisko Zvolen", 123),
            ("Národná diaľničná spoločnosť, a.s. - Stredisko Lučenec", 27),
            ("Via Pribina Operation, a.s.", 22),
        ];
        let mut all: Vec<Option<Value>> = groups
            .iter()
            .flat_map(|(name, n)| std::iter::repeat_n(Some(json!(name)), *n))
            .collect();
        for town in 0..20 {
            all.push(Some(json!(format!("Obec {}", town % 10))));
        }
        all
    }

    fn bridges(rows: &[(&str, i64, f64)]) -> Table {
        Table {
            endpoint: "registre".into(),
            entity_type: "Bridge".into(),
            columns: vec!["name".into(), "yearBuilt".into(), "bridgedLength".into()],
            units: vec![None, None, Some("m".into())],
            rows: rows
                .iter()
                .map(|(name, year, metres)| vec![json!(name), json!(year), json!(metres)])
                .collect(),
            geometries: rows
                .iter()
                .map(|_| Some(json!({"type": "Point", "coordinates": [19.1, 48.7]})))
                .collect(),
            total: Some(1651),
            ..Table::default()
        }
    }

    #[test]
    fn a_count_of_1651_bridges_says_its_manager_groups_and_they_sum() {
        let values = managers();
        assert_eq!(values.len(), 1651);
        let split = split("managerName", &values).expect("a split");
        let mut table = bridges(&[("M21", 1953, 9.46)]);
        table.map = Some(json!({"a": "map"}));
        table.counted = true;
        show_split(&mut table, "managerName", &split, 1651);
        let said = said(&table).expect("said");
        assert_eq!(
            said["parts"],
            json!([
                ["Banskobystrická regionálna správa ciest", 1037],
                ["Slovenská správa ciest", 422],
                ["Národná diaľničná spoločnosť", 150]
            ])
        );
        assert_eq!(said["other"], 42);
        assert!(said.get("of").is_none(), "all of them were counted");
        // The bar past the eight holds 10 of those 42, so it is not called "others" as well.
        let named = table.enums[0].as_ref().and_then(|words| words.get(OTHER));
        assert_eq!(
            named.and_then(|words| words.get("en")).map(String::as_str),
            Some("Outside the top 8")
        );
        let sum: u64 = table.rows.iter().filter_map(|r| r[1].as_u64()).sum();
        assert_eq!(sum, 1651, "one attribute: the parts sum to the total");
        assert!(table.map.is_none() && table.geometries.is_empty() && !table.counted);
        assert!(
            table.rows.iter().all(|row| row[0] != "M21"),
            "no row of the type"
        );
    }

    #[test]
    fn a_split_of_part_of_the_type_names_its_enum_words_the_old_and_the_blank_bars() {
        let mut years: Vec<Option<Value>> = (1950..1990).map(|y| Some(json!(y))).collect();
        years.extend([Some(json!(1700)), None]);
        let split = split("yearBuilt", &years).expect("a split");
        let mut table = bridges(&[("M21", 1953, 9.46)]);
        let words: Words = [(
            "1950–1959".to_owned(),
            [("en".to_owned(), "Fifties".to_owned())].into(),
        )]
        .into();
        table.enums = vec![None, Some(words), None];
        show_split(&mut table, "yearBuilt", &split, 42);
        let named = table.enums[0].as_ref().expect("words");
        for key in [BEFORE, NONE, "1950–1959"] {
            assert!(named.contains_key(key), "{key}");
        }
        let said = said(&table).expect("said");
        assert_eq!(said["of"], 1651, "42 of 1651 counted");
        assert_eq!(said["parts"][0][0]["en"], "Fifties");
        // no count, no sentence
        assert_eq!(super::said(&bridges(&[])), None);
    }

    #[test]
    fn a_short_or_small_name_is_never_folded_and_a_code_is_no_split() {
        let districts: Vec<Option<Value>> = (0..60)
            .map(|i| {
                Some(json!(match i % 10 {
                    0 => "Košice - okolie".to_owned(),
                    1 => "Košice".to_owned(),
                    n => format!("Okres {n}"),
                }))
            })
            .collect();
        let split_of = split("districtName", &districts).expect("a split");
        assert!(split_of
            .rows
            .iter()
            .any(|(name, _)| name == "Košice - okolie"));
        let codes: Vec<Option<Value>> = (0..40).map(|i| Some(json!(format!("M{i}")))).collect();
        assert_eq!(split("bridgeCode", &codes), None);
        let lengths: Vec<Option<Value>> =
            (0..40).map(|i| Some(json!(f64::from(i) + 0.5))).collect();
        assert_eq!(split("bridgedLength", &lengths), None);
    }

    #[test]
    fn years_are_counted_per_decade_with_the_old_ones_as_one_bar_and_blanks_last() {
        let mut years: Vec<Option<Value>> = (1950..1990).map(|y| Some(json!(y))).collect();
        years.extend([Some(json!(1000)), Some(json!(1700)), None]);
        let split = split("yearBuilt", &years).expect("a split");
        assert_eq!(split.per, "decade");
        assert_eq!(
            split.rows,
            [
                (BEFORE.to_owned(), 2),
                ("1950–1959".to_owned(), 10),
                ("1960–1969".to_owned(), 10),
                ("1970–1979".to_owned(), 10),
                ("1980–1989".to_owned(), 10),
                (NONE.to_owned(), 1),
            ]
        );
        let days: Vec<Option<Value>> = ["2026-11-02", "2026-11-30", "2027-01-03"]
            .map(|d| Some(json!(d)))
            .to_vec();
        let months = super::split("startDate", &days).expect("months");
        assert_eq!(months.per, "month");
        let named: Vec<(&str, u64)> = months.rows.iter().map(|(k, n)| (k.as_str(), *n)).collect();
        assert_eq!(
            named,
            [("2026-11", 2), ("2026-12", 0), ("2027-01", 1)],
            "no gap in time"
        );
    }

    #[test]
    fn a_split_reads_years_months_years_of_days_and_names_and_refuses_what_is_no_category() {
        // a few years close together are a bar each, no decade and no old bar
        let few: Vec<Option<Value>> = [2001, 2003, 2003].map(|y| Some(json!(y))).to_vec();
        let years = split("yearBuilt", &few).expect("years");
        assert_eq!(years.per, "year");
        assert_eq!(
            years.rows,
            [
                ("2001".to_owned(), 1),
                ("2002".to_owned(), 0),
                ("2003".to_owned(), 2)
            ]
        );
        // decades with no year before 1800 have no old bar
        let modern: Vec<Option<Value>> = (1900..1960).map(|y| Some(json!(y))).collect();
        let decades = split("yearBuilt", &modern).expect("decades");
        assert_eq!(decades.per, "decade");
        assert!(decades.rows.iter().all(|(k, _)| k != BEFORE));
        // days over more than two years are counted per year
        let days: Vec<Option<Value>> = ["2020-01-05", "2021-06-01", "2023-12-31"]
            .map(|d| Some(json!(d)))
            .to_vec();
        let per_year = split("startDate", &days).expect("per year");
        assert_eq!(per_year.per, "year");
        assert_eq!(per_year.rows.len(), 4);
        // a span wider than a chart draws keeps only the years counted
        let wide: Vec<Option<Value>> = ["0100-01-01", "2026-01-01"]
            .map(|d| Some(json!(d)))
            .to_vec();
        assert_eq!(split("startDate", &wide).map(|s| s.rows.len()), Some(2));
        // more than eight kinds: the eight largest and the rest as one bar
        let kinds: Vec<Option<Value>> = (0..10)
            .flat_map(|k| std::iter::repeat_n(Some(json!(format!("kind {k}"))), 10 - k))
            .collect();
        let kinds = split("category", &kinds).expect("kinds");
        assert_eq!(kinds.rows.len(), GROUPS + 1);
        assert_eq!(kinds.rows.last(), Some(&(OTHER.to_owned(), 3)));
        // long names fold to their heads once there are many of them
        let long: Vec<Option<Value>> = (0..30)
            .map(|i| Some(json!(format!("Správa ciest kraja, stredisko {}", i % 10))))
            .collect();
        assert_eq!(split("managerName", &long), None, "one head is one value");
        // one value, or more than eight numbers, splits nothing
        assert_eq!(
            split("status", &[Some(json!("open")), Some(json!("open"))]),
            None
        );
        let numbers: Vec<Option<Value>> = (0..20).map(|n| Some(json!(n))).collect();
        assert_eq!(split("lanes", &numbers), None);
        let mixed = [Some(json!(1)), Some(json!({"a": 1})), Some(json!(2))];
        assert_eq!(split("lanes", &mixed), None);
    }

    #[test]
    fn a_superlative_is_one_entity_a_list_a_page_with_its_total() {
        let attributes = ["bridgedLength".to_owned(), "managerName".to_owned()];
        let of = |read: Value, order: Option<&str>, question: &str| {
            wanted(&read, order, &attributes, question)
        };
        let order = Some("bridgedLength;desc");
        assert_eq!(
            of(
                json!({}),
                order,
                "Which bridge has the longest bridged length?"
            ),
            View::Top(1)
        );
        assert_eq!(
            of(json!({}), order, "Ktorý most je najdlhší?"),
            View::Top(1)
        );
        assert_eq!(
            of(json!({"top": 3}), order, "three longest bridges"),
            View::Top(3)
        );
        assert_eq!(of(json!({"top": 500}), order, "x"), View::Top(TOP_MAX));
        assert_eq!(
            of(json!({}), None, "bridges in Zvolen district"),
            View::List
        );
        assert_eq!(
            of(json!({}), order, "events this week, soonest first"),
            View::List
        );
        assert_eq!(
            of(
                json!({"group": "managerName"}),
                None,
                "How many bridges are there?"
            ),
            View::Count(Some("managerName".into()))
        );
        assert_eq!(
            of(json!({"count": true, "group": "secret"}), None, "How many?"),
            View::Count(None),
            "only an attribute the Endpoint is shown with"
        );

        let mut table = bridges(&[
            ("50_178", 1996, 710.0),
            ("R2", 2022, 556.0),
            ("R2 b", 2022, 513.4),
        ]);
        keep_top(&mut table, "bridgedLength;desc", 1);
        assert_eq!(
            table.rows,
            [vec![json!("50_178"), json!(1996), json!(710.0)]]
        );
        assert_eq!(
            (table.geometries.len(), table.top, table.total),
            (1, Some(1), Some(1651))
        );
    }

    #[test]
    fn the_oldest_is_every_bridge_of_that_year_and_no_placeholder() {
        // Ordered as `placeholders_last` leaves them.
        let mut table = bridges(&[
            ("2588-2", 1800, 6.56),
            ("2530-004", 1800, 3.7),
            ("2530-005", 1810, 3.9),
            ("Pracovný názov: Nadjazd", 1700, 0.1),
        ]);
        keep_top(&mut table, "yearBuilt;asc", 1);
        assert_eq!(table.rows.len(), 2, "the two of 1800");
        assert_eq!(table.set_aside.len(), 1, "named by the answer, never shown");
    }

    #[test]
    fn a_rank_by_a_column_not_shown_or_a_blank_edge_keeps_just_the_top_and_only_placeholders_keep_all(
    ) {
        let three = [("A", 1990, 5.0), ("B", 1991, 4.0), ("C", 1992, 4.0)];
        let mut unshown = bridges(&three);
        keep_top(&mut unshown, "height;desc", 1);
        assert_eq!(unshown.rows.len(), 1);
        let mut blank = bridges(&three);
        for row in &mut blank.rows {
            row[2] = Value::Null;
        }
        keep_top(&mut blank, "bridgedLength;desc", 2);
        assert_eq!(blank.rows.len(), 2, "a blank edge ties with nothing");
        let mut placeholders = bridges(&[
            ("Pracovný názov: A", 1700, 0.1),
            ("Pracovný názov: B", 1710, 0.2),
        ]);
        keep_top(&mut placeholders, "yearBuilt;asc", 1);
        assert_eq!(
            placeholders.rows.len(),
            2,
            "nothing real to rank: the table stays"
        );
        // a page past the first ranks nothing
        assert_eq!(
            top_of(&json!({"orderBy": "yearBuilt", "limit": 3, "cursor": "c"})),
            None
        );
        assert_eq!(
            top_of(&json!({"orderBy": "yearBuilt", "limit": 3, "offset": 3})),
            None
        );
        assert_eq!(
            top_of(&json!({"orderBy": "yearBuilt", "limit": 3})),
            Some(3)
        );
    }

    #[test]
    fn a_descending_rank_asks_only_entities_with_the_attribute() {
        let schema = json!({"properties": {"q": {}, "orderBy": {}}});
        let sent = |read: Value| ranked_arguments(&schema, &read);
        assert_eq!(
            sent(json!({"orderBy": "bridgedLength;desc"}))["q"],
            "bridgedLength"
        );
        assert_eq!(
            sent(json!({"orderBy": "bridgedLength;desc", "q": "a==1|b==2"}))["q"],
            "(a==1|b==2);bridgedLength"
        );
        assert!(sent(json!({"orderBy": "yearBuilt;asc"})).get("q").is_none());
        assert!(sent(json!({"orderBy": "a|b;desc"})).get("q").is_none());
        assert_eq!(top_of(&json!({"orderBy": "a;desc", "limit": 1})), Some(1));
        assert_eq!(top_of(&json!({"orderBy": "a;desc", "limit": 50})), None);
        assert_eq!(top_of(&json!({"limit": 1})), None);
        assert!(counts_all(
            &json!({"type": ["Bridge"], "count": true, "limit": 1})
        ));
        assert!(counts_all(&json!({"type": ["Bridge"], "count": true})));
        assert!(!counts_all(
            &json!({"count": true, "limit": 1, "q": "a==1"})
        ));
        assert!(!counts_all(&json!({"count": true, "limit": 20})));
    }

    #[test]
    fn a_category_reads_a_scalar_a_language_s_text_or_an_address_s_town() {
        assert_eq!(category(&json!(3)), Some(json!(3)));
        assert_eq!(category(&json!({"value": true})), Some(json!("true")));
        assert_eq!(category(&json!({"value": "  Park  "})), Some(json!("Park")));
        assert_eq!(category(&json!("   ")), None);
        assert_eq!(
            category(&json!({"languageMap": {"en": "Bridge", "sk": "Most"}})),
            Some(json!("Most"))
        );
        assert_eq!(
            category(&json!({"languageMap": {"en": "Bridge", "de": "Brücke"}})),
            Some(json!("Bridge"))
        );
        assert_eq!(
            category(&json!({"languageMap": {"hu": "Híd"}})),
            Some(json!("Híd"))
        );
        assert_eq!(
            category(&json!({"value": {"addressLocality": "Zvolen"}})),
            Some(json!("Zvolen"))
        );
        assert_eq!(category(&json!({"value": {"street": "Hlavná"}})), None);
        assert_eq!(category(&json!([1, 2])), None);
        assert_eq!(category(&Value::Null), None);
    }

    #[test]
    fn a_history_reads_instances_and_any_aggregation_method() {
        let instances = json!({"id": "urn:ngsi-ld:AirQualityObserved:a", "type": "AirQualityObserved",
            "pm10": [{"type": "Property", "value": 12.0, "observedAt": "2026-10-01T08:00:00Z"},
                     {"type": "Property", "value": 14.0, "observedAt": "2026-10-02T08:00:00Z"},
                     {"type": "Property", "value": "n/a"}]});
        let read = history(&instances.to_string(), &json!({"attrs": ["pm10"]})).expect("a history");
        assert_eq!(read.ids, ["urn:ngsi-ld:AirQualityObserved:a"]);
        assert_eq!(read.rows.len(), 2);
        let one = json!({"id": "urn:ngsi-ld:AirQualityObserved:b", "type": "AirQualityObserved",
            "pm10": {"type": "Property", "value": 9.0, "observedAt": "2026-10-01T08:00:00Z"}});
        assert_eq!(
            history(&one.to_string(), &json!({}))
                .expect("a history")
                .rows
                .len(),
            1
        );
        let maxed = json!({"entities": [{"id": "urn:ngsi-ld:AirQualityObserved:c", "type": "AirQualityObserved",
            "pm10": {"type": "Property", "max": [[30, "2026-10-01T00:00:00Z", "2026-10-02T00:00:00Z"]]}}]});
        let read = history(&maxed.to_string(), &json!({"attrs": ["pm10"]})).expect("a history");
        assert_eq!(read.made.method, "max");
        assert!(history("not json", &json!({})).is_none());
        assert!(history(&json!({"entities": []}).to_string(), &json!({})).is_none());
    }

    #[test]
    fn a_history_across_new_year_is_a_dated_line_and_two_places_get_a_legend() {
        let start = time::macros::datetime!(2025-12-15 00:00 UTC);
        let values = |offset: f64| -> Vec<Value> {
            (0..40 * 24)
                .map(|h| {
                    let at = start + time::Duration::hours(h);
                    json!([
                        offset + (h % 24) as f64,
                        at.format(&Rfc3339).expect("a time")
                    ])
                })
                .collect()
        };
        let one = json!({"entities": [{"id": "urn:ngsi-ld:AirQualityObserved:a", "type": "AirQualityObserved",
            "pm10": {"type": "Property", "values": values(0.0)}}]});
        let read = history(&one.to_string(), &json!({"attrs": ["pm10"]})).expect("a history");
        assert_eq!(read.made.per, "day");
        let table = history_table("air", "AirQualityObserved", &read, "");
        let chart = super::super::viz::aggregated(&table).expect("a chart");
        assert_eq!(chart["spec"]["encoding"]["x"]["scale"]["type"], "utc");
        assert_eq!(
            chart["spec"]["encoding"]["x"]["axis"]["format"],
            "%-d %b %Y"
        );
        assert_eq!(chart["spec"]["encoding"]["color"]["legend"], Value::Null);
        assert_eq!(chart["series"], "a");
        assert_eq!(
            chart["spec"]["mark"]["point"], true,
            "40 points are drawn as points"
        );

        let two = json!({"entities": [
            {"id": "urn:ngsi-ld:AirQualityObserved:a", "type": "AirQualityObserved", "pm10": {"type": "Property", "values": values(0.0)}},
            {"id": "urn:ngsi-ld:AirQualityObserved:b", "type": "AirQualityObserved", "pm10": {"type": "Property", "values": values(5.0)}}]});
        let read = history(&two.to_string(), &json!({"attrs": ["pm10"]})).expect("a history");
        let table = history_table("air", "AirQualityObserved", &read, "");
        let chart = super::super::viz::aggregated(&table).expect("a chart");
        assert_eq!(
            chart["spec"]["encoding"]["color"]["legend"]["orient"],
            "bottom"
        );
        assert!(chart.get("series").is_none());
    }

    #[test]
    fn thirty_days_of_hours_become_daily_points_with_the_method_named() {
        let start = time::macros::datetime!(2026-10-01 00:00 UTC);
        let values: Vec<Value> = (0..720)
            .map(|h| {
                let at = start + time::Duration::hours(h);
                json!([h % 24, at.format(&Rfc3339).expect("a time")])
            })
            .collect();
        let text = json!({"entities": [{"id": "urn:ngsi-ld:AirQualityObserved:eea-SK0263A", "type": "AirQualityObserved", "pm10": {"type": "Property", "values": values}}]});
        let history = history(&text.to_string(), &json!({"attrs": ["pm10"]})).expect("a history");
        assert_eq!(history.rows.len(), 30);
        assert_eq!(
            (history.made.method.as_str(), history.made.per),
            ("avg", "day")
        );
        assert_eq!(history.rows[0], (0, "2026-10-01".to_owned(), 11.5));
        assert_eq!(
            history.made.days, None,
            "no window, no total of days to state"
        );
        let october = json!({"attrs": ["pm10"], "timerel": "between",
            "timeAt": "2026-10-01T00:00:00Z", "endTimeAt": "2026-11-01T00:00:00Z"});
        let asked = super::history(&text.to_string(), &october).expect("a history");
        assert_eq!(
            asked.made.days,
            Some((30, 31)),
            "a day without a reading shows"
        );

        let current = json!({"entities": [{"id": history.ids[0], "name": {"type": "Property", "value": "Štadlerovo nábrežie"}, "pm10": {"value": 9, "unitCode": "GQ"}}]});
        let table = history_table(
            "ovzdusie",
            "AirQualityObserved",
            &history,
            &current.to_string(),
        );
        assert_eq!(table.columns, ["name", "date", "pm10"]);
        assert_eq!(table.rows[0][0], "Štadlerovo nábrežie");
        assert_eq!(table.units[2].as_deref(), Some("µg/m³"));
    }

    #[test]
    fn a_history_is_at_most_500_points_and_says_how_many_series_of_all() {
        let start = time::macros::datetime!(2026-01-01 00:00 UTC);
        let day = |d: i64| {
            (start + time::Duration::days(d))
                .format(&Rfc3339)
                .expect("a time")
        };
        let avg: Vec<Value> = (0..200).map(|d| json!([d, day(d), day(d + 1)])).collect();
        let entities: Vec<Value> = (0..12)
            .map(|i| json!({"id": format!("urn:ngsi-ld:A:{i}"), "type": "A", "pm10": {"type": "Property", "avg": avg}}))
            .collect();
        let history =
            history(&json!({"entities": entities}).to_string(), &json!({})).expect("a history");
        assert!(history.rows.len() <= POINTS);
        assert_eq!(
            (history.made.shown, history.made.total, history.made.partial),
            (8, 12, false)
        );
        // 200 days of 8 series are their 7 months, none dropped (T-7526)
        assert_eq!((history.rows.len(), history.made.per), (56, "month"));
        assert_eq!(history.rows[0].1, "2026-01-01");
    }

    /// Yearly or monthly buckets of `from` up to 2026, as the broker aggregates them.
    fn buckets(from: i32, months: &[u8]) -> Vec<Value> {
        (from..2026)
            .flat_map(|year| months.iter().map(move |month| (year, *month)))
            .map(|(year, month)| {
                json!([
                    year,
                    format!("{year}-{month:02}-01T00:00:00Z"),
                    "2026-01-01T00:00:00Z"
                ])
            })
            .collect()
    }

    #[test]
    fn ten_and_a_hundred_years_show_their_whole_span_with_the_period_stated() {
        let year: Vec<u8> = (1..=12).collect();
        for (from, months, period, per, rows) in [
            (2016, &year[..], "P1M", "month", 120),
            (1926, &year[..1], "P1Y", "year", 100),
            // two years of two buckets are years too
            (2024, &year[..1], "P1Y", "year", 2),
            // forty-five years of months are more than a chart holds
            (1981, &year[..], "P1M", "year", 45),
        ] {
            let text = json!({"entities": [{"id": "urn:ngsi-ld:K:1", "type": "K", "population": {"type": "Property", "avg": buckets(from, months)}}]});
            let asked = json!({"attrs": ["population"], "aggrPeriodDuration": period});
            let history = history(&text.to_string(), &asked).expect("a history");
            assert_eq!(
                (history.made.per, history.rows.len()),
                (per, rows),
                "{from}"
            );
            assert_eq!(history.made.years, Some((from, 2025)), "{from}");
            assert!(!history.made.partial, "{from}");
            assert_eq!(history.rows[0].1, format!("{from}-01-01"));
            assert_eq!(history.made.days, None);
            let table = history_table("kpi", "K", &history, "");
            assert_eq!(table.columns, ["name", "date", "population"]);
        }
    }

    #[test]
    fn only_years_past_the_points_lose_the_oldest_and_say_so() {
        let entities: Vec<Value> = (0..8)
            .map(|i| json!({"id": format!("urn:ngsi-ld:K:{i}"), "type": "K", "population": {"type": "Property", "avg": buckets(1900, &[1])}}))
            .collect();
        let history =
            history(&json!({"entities": entities}).to_string(), &json!({})).expect("a history");
        assert_eq!((history.made.per, history.made.partial), ("year", true));
        assert!(history.rows.len() <= POINTS);
    }

    #[test]
    fn a_long_range_is_asked_no_finer_than_the_shared_table() {
        let schema = json!({"properties": {"aggrPeriodDuration": {}}});
        let now = time::macros::datetime!(2026-11-22 12:00 UTC);
        for (from, asked, sent) in [
            ("2025-11-22", "P1D", "P1D"),
            ("2016-11-22", "P1D", "P1M"),
            ("2016-11-22", "PT1H", "P1M"),
            ("2016-11-22", "P1Y", "P1Y"),
            ("1926-11-22", "P1M", "P1Y"),
            ("1800-01-01", "P1Y", "P1Y"),
            ("1926-11-22", "PT0S", "PT0S"),
        ] {
            let read = json!({"timerel": "after", "timeAt": format!("{from}T00:00:00Z"), "aggrMethods": ["avg"], "aggrPeriodDuration": asked});
            assert_eq!(
                history_arguments(&schema, &read, now)["aggrPeriodDuration"],
                sent,
                "{from} {asked}"
            );
        }
    }

    #[test]
    fn a_month_of_hourly_averages_is_asked_per_day() {
        let schema = json!({"properties": {"aggrPeriodDuration": {}}});
        let now = time::macros::datetime!(2026-11-22 12:00 UTC);
        let month = json!({"timerel": "after", "timeAt": "2026-10-23T00:00:00Z", "aggrMethods": ["avg"], "aggrPeriodDuration": "PT1H"});
        assert_eq!(
            history_arguments(&schema, &month, now)["aggrPeriodDuration"],
            "P1D"
        );
        let mut day = month.clone();
        day["timeAt"] = json!("2026-11-22T00:00:00Z");
        assert_eq!(history_arguments(&schema, &day, now), day);
    }

    #[test]
    fn a_window_between_two_instants_is_asked_per_day_and_a_broken_one_as_written() {
        let schema = json!({"properties": {"aggrPeriodDuration": {}}});
        let now = time::macros::datetime!(2026-11-22 12:00 UTC);
        let between = json!({"timerel": "between", "timeAt": "2026-10-01T00:00:00Z", "endTimeAt": "2026-11-01T00:00:00Z", "aggrMethods": ["avg"], "aggrPeriodDuration": "PT1H"});
        assert_eq!(
            history_arguments(&schema, &between, now)["aggrPeriodDuration"],
            "P1D"
        );
        let mut broken = between.clone();
        broken["endTimeAt"] = json!("next week");
        assert_eq!(history_arguments(&schema, &broken, now), broken);
    }

    #[test]
    fn no_answer_attaches_a_page_of_unrelated_rows() {
        let page = bridges(&[("M21", 1953, 9.0), ("M22", 1960, 4.0)]);
        let mut top = bridges(&[("50_178", 1996, 710.0)]);
        top.top = Some(1);
        let read = vec![(1, page.clone()), (2, top.clone())];
        assert_eq!(selected(&read, &BTreeSet::from([1, 2])), [&top]);
        assert_eq!(selected(&read, &BTreeSet::from([1])), [&page, &top]);

        let mut counted = page.clone();
        counted.counted = true;
        assert!(selected(&[(1, counted)], &BTreeSet::from([1])).is_empty());

        let mut part = page.clone();
        part.filter = Some("yearBuilt<1900".into());
        let read = vec![(1, page.clone()), (2, part.clone())];
        assert_eq!(selected(&read, &BTreeSet::from([1, 2])), [&part]);
        assert_eq!(
            selected(&[(1, page.clone())], &BTreeSet::from([1])),
            [&page]
        );
    }
}
