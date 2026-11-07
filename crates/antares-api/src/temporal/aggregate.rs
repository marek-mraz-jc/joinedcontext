// SPDX-License-Identifier: EUPL-1.2
//! Aggregated temporal representation (4.5.19).

use super::*;

/// Aggregated representation (4.5.19): attr → `{type, <method>: [[v,start,end]]}`.
/// Aggregation datatype class per 4.5.19.1 (Tables -1, -2, -3). Booleans
/// count as numbers (1/0, table NOTE); a JSON String, a DateTime and a Date
/// share the ordered min/max column; a Time additionally supports avg.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum AggrClass {
    Number,
    Text,
    /// 4.6.3 DateTime or Date: ordered, and Table 4.5.19.1-2 gives it no
    /// arithmetic.
    Instant,
    TimeOfDay,
    List,
    Opaque,
    Relationship,
}

/// The 4.6.3 datatype a Property instance's value carries, in either
/// representation C.6 gives for one: a JSON-LD typed value
/// (`{"@type": "DateTime", "@value": …}`), or a string whose `valueType`
/// names the datatype and is coerced to its URI on the way in (4.5.2.2).
/// Table 4.5.19.1-2 applies to the datatype, not to the spelling.
pub(super) fn value_datatype(inst: &Value) -> Option<&str> {
    fn term(s: &str) -> &str {
        s.strip_prefix(antares_jsonld::NGSI_LD_BASE).unwrap_or(s)
    }
    if let Some(vt) = inst.get("valueType").and_then(Value::as_str) {
        if inst.get("value").is_some_and(Value::is_string) {
            return Some(term(vt));
        }
    }
    inst.get("value")?
        .get("@type")
        .and_then(Value::as_str)
        .map(term)
}

/// The lexical form of a value: the string itself, or the `@value` of a
/// JSON-LD typed value.
pub(super) fn lexical_of(v: &Value) -> Option<&str> {
    v.as_str()
        .or_else(|| v.get("@value").and_then(Value::as_str))
}

/// The key the ordered classes compare by. A JSON String and a Date are
/// compared as written — 4.6.3 fixes the width of every component of a Date,
/// so lexicographical order is chronological — a DateTime by its canonical
/// instant, since an optional seconds fraction is written before the `Z` it
/// follows and sorts ahead of it, and a Time by its second of the day, at a
/// fixed width so one string comparison serves all four.
pub(super) fn order_key(class: AggrClass, v: &Value) -> Option<String> {
    let s = lexical_of(v)?;
    Some(match class {
        AggrClass::Instant => antares_model::dt_key(s),
        AggrClass::TimeOfDay => format!("{:013.6}", seconds_of_day(s)?),
        _ => s.to_owned(),
    })
}

pub(super) fn classify_instance(inst: &Value) -> AggrClass {
    if inst.get("object").is_some() {
        return AggrClass::Relationship;
    }
    if inst.get("valueList").is_some() || inst.get("objectList").is_some() {
        return AggrClass::List;
    }
    if inst.get("vocab").is_some()
        || inst.get("languageMap").is_some()
        || inst.get("json").is_some()
    {
        // URI / JSON-object valued kinds: only counting methods apply
        return AggrClass::Opaque;
    }
    match value_datatype(inst) {
        Some("DateTime" | "Date") => return AggrClass::Instant,
        Some("Time") => return AggrClass::TimeOfDay,
        _ => {}
    }
    match inst.get("value") {
        Some(Value::Number(_)) | Some(Value::Bool(_)) => AggrClass::Number,
        Some(Value::String(_)) => AggrClass::Text,
        Some(Value::Array(_)) => AggrClass::List,
        _ => AggrClass::Opaque,
    }
}

/// Table 4.5.19.1 eligibility: which methods apply to which datatype class.
pub(super) fn aggr_eligible(class: AggrClass, method: &str) -> bool {
    match method {
        "totalCount" | "distinctCount" => true,
        "min" | "max" => !matches!(class, AggrClass::Opaque | AggrClass::Relationship),
        "avg" => matches!(
            class,
            AggrClass::Number | AggrClass::List | AggrClass::TimeOfDay
        ),
        "sum" => matches!(class, AggrClass::Number | AggrClass::List),
        "stddev" | "sumsq" => matches!(class, AggrClass::Number),
        _ => false,
    }
}

/// `HH:MM:SS[.f]` → seconds of day (4.6.3 Time is UTC with optional `Z`).
pub(super) fn seconds_of_day(s: &str) -> Option<f64> {
    let t = s.strip_suffix('Z').unwrap_or(s);
    let b = t.as_bytes();
    if b.len() < 8 || b.get(2) != Some(&b':') || b.get(5) != Some(&b':') {
        return None;
    }
    let h: f64 = t.get(0..2)?.parse().ok()?;
    let m: f64 = t.get(3..5)?.parse().ok()?;
    let sec: f64 = t.get(6..)?.parse().ok()?;
    (h < 24.0 && m < 60.0 && sec < 62.0).then_some(h * 3600.0 + m * 60.0 + sec)
}

/// The raw member an instance carries its data under.
pub(super) fn raw_of(inst: &Value) -> Option<&Value> {
    for k in [
        "value",
        "object",
        "valueList",
        "objectList",
        "vocab",
        "languageMap",
        "json",
    ] {
        if let Some(v) = inst.get(k) {
            return Some(v);
        }
    }
    None
}

/// Numeric view of one raw value for the class (None ⇒ excluded from
/// numeric methods; List aggregates SIZES per Table 4.5.19.1-1).
pub(super) fn numeric_of(class: AggrClass, v: &Value) -> Option<f64> {
    match class {
        AggrClass::Number => match v {
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => v.as_f64(),
        },
        AggrClass::List => v.as_array().map(|a| a.len() as f64),
        AggrClass::TimeOfDay => lexical_of(v).and_then(seconds_of_day),
        _ => None,
    }
}

pub(super) fn render_aggregated(
    w: &Windowed,
    tq: Option<&TemporalQ>,
    r: &TRepr,
    ctx: &Context,
    timeprop: &str,
) -> Result<Map<String, Value>, NgsiError> {
    let mut out = Map::new();
    for (k, instances) in &w.attrs {
        let held = w.held.contains(k);
        if let Some(attr_out) = aggregate_attr(k, instances, held, tq, r, ctx, timeprop)? {
            out.insert(ctx.compact_iri(k), Value::Object(attr_out));
        }
    }
    Ok(out)
}

type Moment = chrono::DateTime<chrono::FixedOffset>;
/// One period: its start and its (exclusive) end.
type Bucket = (Moment, Moment);
/// The instances of one period, by their time.
type Rows<'a> = Vec<(Moment, &'a Value)>;

/// One Attribute's 4.5.19 member, `None` when no instance carries a time.
/// `held`: the Attribute records `onChange` (ADR-N-054 2.5).
fn aggregate_attr(
    k: &str,
    instances: &[Value],
    held: bool,
    tq: Option<&TemporalQ>,
    r: &TRepr,
    ctx: &Context,
    timeprop: &str,
) -> Result<Option<Map<String, Value>>, NgsiError> {
    let (mut times, class) = timed_rows(instances, timeprop);
    // Set alongside the first instance, and `times` is non-empty then.
    let Some(class) = class.filter(|_| !times.is_empty()) else {
        return Ok(None);
    };
    check_methods(k, class, held, r, ctx)?;
    times.sort_by_key(|(t, _)| *t);
    let Some(whole) = whole_range(&times, tq) else {
        return Ok(None);
    };
    let Some(&(earliest, _)) = times.first() else {
        return Ok(None);
    };
    let anchor = tq
        .and_then(|tq| chrono::DateTime::parse_from_rfc3339(&tq.time_at).ok())
        .unwrap_or(earliest);
    // the row carried in from before the window holds from its start
    if held {
        for (t, _) in &mut times {
            *t = (*t).max(whole.0);
        }
    }
    let bucket_of = |t: Moment| -> Bucket {
        match r.aggr_period {
            AggrPeriod::Whole => whole,
            AggrPeriod::Seconds(sc) => seconds_bucket(anchor, sc, t),
            AggrPeriod::Months(m, sc) => months_bucket(anchor, m, sc, t),
        }
    };
    let Some(buckets) = bucketed(&times, held, whole, &bucket_of) else {
        return Err(NgsiError::BadRequestData(format!(
            "aggrPeriodDuration splits the window of {} into more than {MAX_HELD_BUCKETS} \
             periods; ask a longer period or a shorter window",
            ctx.compact_iri(k)
        )));
    };
    Ok(Some(render_buckets(&buckets, class, held, whole.1, r)))
}

/// The instances that carry `timeprop`, with their raw values, and the
/// aggregation class of the first of them.
fn timed_rows<'a>(instances: &'a [Value], timeprop: &str) -> (Rows<'a>, Option<AggrClass>) {
    let mut times: Rows<'a> = Vec::new();
    let mut class: Option<AggrClass> = None;
    for inst in instances {
        let Some(t) = inst
            .get(timeprop)
            .and_then(Value::as_str)
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        else {
            continue;
        };
        if class.is_none() {
            class = Some(classify_instance(inst));
        }
        let Some(raw) = raw_of(inst) else { continue };
        times.push((t, raw));
    }
    (times, class)
}

/// 5.7.4.4 p.211: "If an aggregated temporal representation is
/// requested and any of the requested Attributes is not eligible for
/// at least one of the aggregation methods specified in the request
/// parameters, then an error of type InvalidRequest shall be raised."
/// ADR-N-054 2.5 (DM-118): an `onChange` Attribute leaves out the rows
/// that repeat a value, and a left-out row is not a zero.
fn check_methods(
    k: &str,
    class: AggrClass,
    held: bool,
    r: &TRepr,
    ctx: &Context,
) -> Result<(), NgsiError> {
    for method in &r.aggr_methods {
        if !aggr_eligible(class, method) {
            return Err(NgsiError::InvalidRequest(format!(
                "attribute {} ({class:?}-valued) is not eligible for \
                 aggregation method {method} (4.5.19.1, 5.7.4.4)",
                ctx.compact_iri(k)
            )));
        }
    }
    if let Some(method) = r
        .aggr_methods
        .iter()
        .find(|m| held && matches!(m.as_str(), "sum" | "totalCount"))
    {
        return Err(NgsiError::InvalidRequest(format!(
            "aggregation-not-meaningful: {method} over {}, which records a row only \
             when its value changes (ADR-N-054 2.5)",
            ctx.compact_iri(k)
        )));
    }
    Ok(())
}

/// 4.5.19.1: "A duration of 0 second (e.g. expressed as "PT0S" or
/// "P0D") is valid and is interpreted as a duration spanning the whole
/// time range specified by the temporal query." The query names one
/// edge of that range; 4.11 leaves the other open for `before` and
/// `after`, so the data closes the open one. `times` is sorted.
fn whole_range(times: &Rows<'_>, tq: Option<&TemporalQ>) -> Option<Bucket> {
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    let (&(first, _), &(last_at, _)) = (times.first()?, times.last()?);
    let last = last_at
        .checked_add_signed(chrono::Duration::seconds(1))
        .unwrap_or(last_at);
    Some(match tq {
        Some(q) if q.timerel == "before" => (first, at(&q.time_at).unwrap_or(last)),
        Some(q) if q.timerel == "between" => (
            at(&q.time_at).unwrap_or(first),
            q.end_time_at.as_deref().and_then(at).unwrap_or(last),
        ),
        Some(q) if q.timerel == "after" => (at(&q.time_at).unwrap_or(first), last),
        _ => (first, last),
    })
}

/// The `sc`-second period from `anchor` holding `t`. Checked throughout:
/// an offset no representable date can hold puts the instant in one final
/// open-ended bucket instead of panicking.
fn seconds_bucket(anchor: Moment, sc: i64, t: Moment) -> Bucket {
    let idx = t.signed_duration_since(anchor).num_seconds().div_euclid(sc);
    let bucket = idx
        .checked_mul(sc)
        .and_then(chrono::Duration::try_seconds)
        .and_then(|off| anchor.checked_add_signed(off))
        .and_then(|start| {
            chrono::Duration::try_seconds(sc)
                .and_then(|w| start.checked_add_signed(w))
                .map(|end| (start, end))
        });
    match bucket {
        Some(b) => b,
        None => (anchor, chrono::DateTime::<chrono::Utc>::MAX_UTC.into()),
    }
}

/// The period of `m` months and `sc` seconds from `anchor` holding `t`.
fn months_bucket(anchor: Moment, m: u32, sc: i64, t: Moment) -> Bucket {
    use chrono::Datelike;
    // start of the k-th period, O(1) in k. Negative k are
    // the periods BEFORE the anchor, which is what a
    // `before` query is made of.
    let step = |k: i64| -> Option<Moment> {
        let n = k.unsigned_abs().checked_mul(u64::from(m))?;
        let n = chrono::Months::new(u32::try_from(n).ok()?);
        let base = if k < 0 {
            anchor.checked_sub_months(n)?
        } else {
            anchor.checked_add_months(n)?
        };
        let off = chrono::Duration::try_seconds(k.checked_mul(sc)?)?;
        base.checked_add_signed(off)
    };
    // The whole-month distance ignores the day and the time
    // of day, and one period is at least one month, so it
    // brackets the exact index within one step: binary-search
    // between it and the anchor instead of walking there,
    // which is O(log) however far the instant is.
    let approx = i64::from(t.year())
        .saturating_sub(i64::from(anchor.year()))
        .saturating_mul(12)
        .saturating_add(i64::from(t.month()))
        .saturating_sub(i64::from(anchor.month()))
        .div_euclid(i64::from(m));
    let (mut lo, mut hi) = (
        approx.min(0).saturating_sub(1),
        approx.max(0).saturating_add(1),
    );
    while hi.saturating_sub(lo) > 1 {
        let mid = lo.saturating_add(hi.saturating_sub(lo).div_euclid(2));
        if step(mid).is_some_and(|s| s <= t) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    // saturate instead of panic: a huge month period or a
    // far-future timeAt overflows chrono's range — treat the
    // remainder as one open-ended bucket
    match (step(lo), step(hi)) {
        (Some(start), Some(end)) if start <= t => (start, end),
        (Some(start), None) if start <= t => {
            (start, chrono::DateTime::<chrono::Utc>::MAX_UTC.into())
        }
        _ => (anchor, chrono::DateTime::<chrono::Utc>::MAX_UTC.into()),
    }
}

/// The sorted rows in their periods. An onChange attribute held its value
/// through a period no row fell in: that period answers the value, as
/// `every` would have, up to the window's end, never past now, and at most
/// MAX_HELD_BUCKETS of them: a far edge and PT1S is refused (`None`), not
/// allocated (T-4669).
fn bucketed<'a>(
    times: &Rows<'a>,
    held: bool,
    whole: Bucket,
    bucket_of: &dyn Fn(Moment) -> Bucket,
) -> Option<Vec<(Bucket, Rows<'a>)>> {
    let mut buckets: Vec<(Bucket, Rows)> = Vec::new();
    let edge = whole.1.min(chrono::Utc::now().fixed_offset());
    let mut refused = false;
    let mut fill = |buckets: &mut Vec<(Bucket, Rows)>, until: Moment| {
        while let Some(end) = buckets.last().map(|((_, end), _)| *end) {
            let gap = bucket_of(end);
            if end >= until || end >= edge || gap.0 < end {
                break;
            }
            if buckets.len() >= MAX_HELD_BUCKETS {
                refused = true;
                break;
            }
            buckets.push((gap, Vec::new()));
        }
    };
    for &(t, v) in times {
        let b = bucket_of(t);
        if held {
            fill(&mut buckets, b.0);
        }
        match buckets.last_mut() {
            Some((bb, rows)) if bb.0 == b.0 => rows.push((t, v)),
            _ => buckets.push((b, vec![(t, v)])),
        }
    }
    if held {
        fill(&mut buckets, whole.1);
    }
    (!refused).then_some(buckets)
}

/// The member of one Attribute: its type and, per method, one
/// `[value, start, end]` per period. `window_end` caps a held value's weight.
fn render_buckets(
    buckets: &[(Bucket, Rows<'_>)],
    class: AggrClass,
    held: bool,
    window_end: Moment,
    r: &TRepr,
) -> Map<String, Value> {
    let fmt = |d: Moment| d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // the value the last bucket with a row ended on holds into the next one
    let carried: Vec<Option<&Value>> = buckets
        .iter()
        .scan(None, |last, (_, rows)| {
            let before = *last;
            if let Some(&(_, v)) = rows.last() {
                *last = Some(v);
            }
            Some(before)
        })
        .collect();
    let mut attr_out = Map::new();
    // 4.5.19.0: the member is labelled "Property" for Properties and
    // "Relationship" for Relationships.
    let label = if class == AggrClass::Relationship {
        "Relationship"
    } else {
        "Property"
    };
    attr_out.insert("type".into(), Value::String(label.into()));
    for method in &r.aggr_methods {
        let rows: Vec<Value> = buckets
            .iter()
            .zip(&carried)
            .map(|(((bs, be), rows), &before)| {
                let weighted = (held && method == "avg")
                    .then(|| time_weighted_avg(class, rows, before, *bs, (*be).min(window_end)))
                    .flatten();
                let val = weighted.unwrap_or_else(|| {
                    let vals: Vec<&Value> = match (rows.is_empty(), before) {
                        (true, Some(v)) => vec![v],
                        _ => rows.iter().map(|&(_, v)| v).collect(),
                    };
                    aggregate_bucket(method, class, &vals)
                });
                Value::Array(vec![val, Value::String(fmt(*bs)), Value::String(fmt(*be))])
            })
            .collect();
        attr_out.insert(method.clone(), Value::Array(rows));
    }
    attr_out
}

/// The mean of a bucket from `start` to `end` whose `rows` (time, value) each
/// hold until the next, `before` holding until the first (ADR-N-054 2.5);
/// `None` for a class with no number or a bucket no value held for a moment.
pub(super) fn time_weighted_avg(
    class: AggrClass,
    rows: &[(chrono::DateTime<chrono::FixedOffset>, &Value)],
    before: Option<&Value>,
    start: chrono::DateTime<chrono::FixedOffset>,
    end: chrono::DateTime<chrono::FixedOffset>,
) -> Option<Value> {
    if class == AggrClass::TimeOfDay {
        return None;
    }
    let held = before
        .map(|v| (start, v))
        .into_iter()
        .chain(rows.iter().copied());
    let until = rows
        .iter()
        .skip(usize::from(before.is_none()))
        .map(|&(t, _)| t)
        .chain([end]);
    let (mut weighted, mut span) = (0.0, 0.0);
    for ((from, v), to) in held.zip(until) {
        let secs = to.signed_duration_since(from).num_milliseconds() as f64 / 1000.0;
        if let (Some(x), true) = (numeric_of(class, v), secs > 0.0) {
            weighted += x * secs;
            span += secs;
        }
    }
    (span > 0.0).then(|| serde_json::json!(weighted / span))
}

/// One bucket, one method — per-class semantics from Tables 4.5.19.1-1/2/3.
/// Never emits an out-of-range float (the old fold seeded with
/// f64::INFINITY, which serde_json serializes as null).
pub(super) fn aggregate_bucket(method: &str, class: AggrClass, vals: &[&Value]) -> Value {
    let nums: Vec<f64> = vals.iter().filter_map(|v| numeric_of(class, v)).collect();
    let finite = |x: f64| {
        if x.is_finite() {
            serde_json::json!(x)
        } else {
            Value::Null
        }
    };
    match method {
        "totalCount" => serde_json::json!(vals.len()),
        "distinctCount" => {
            // Relationship: "count of distinct relationship TARGETS" — an
            // object may be a URI or an array of URIs, so flatten first.
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for v in vals {
                let items: Vec<&Value> = match (class, v) {
                    (AggrClass::Relationship, Value::Array(a)) => a.iter().collect(),
                    _ => vec![*v],
                };
                for it in items {
                    seen.insert(it.to_string());
                }
            }
            serde_json::json!(seen.len())
        }
        "min" | "max" => match class {
            // ordered classes: the first or last value in the order the
            // tables give the datatype, returned as it was written
            AggrClass::Text | AggrClass::Instant | AggrClass::TimeOfDay => {
                let mut keyed: Vec<(String, &Value)> = vals
                    .iter()
                    .filter_map(|v| order_key(class, v).map(|k| (k, *v)))
                    .collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                let pick = if method == "min" {
                    keyed.first()
                } else {
                    keyed.last()
                };
                pick.map_or(Value::Null, |(_, v)| (*v).clone())
            }
            _ => {
                let it = nums.iter().copied();
                let picked = if method == "min" {
                    it.fold(None, |a: Option<f64>, v| Some(a.map_or(v, |x| x.min(v))))
                } else {
                    it.fold(None, |a: Option<f64>, v| Some(a.map_or(v, |x| x.max(v))))
                };
                picked.map_or(Value::Null, &finite)
            }
        },
        "sum" => finite(nums.iter().sum::<f64>()),
        "avg" => {
            if nums.is_empty() {
                Value::Null
            } else if class == AggrClass::TimeOfDay {
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let (h, m, sec) = (
                    (mean / 3600.0) as u32,
                    ((mean % 3600.0) / 60.0) as u32,
                    (mean % 60.0) as u32,
                );
                // 4.6.3: a Time is `hh:mm:ssZ`, and its JSON-LD type is what
                // tells a reader it is one — written bare it reads back as a
                // JSON String, which Table 4.5.19.1-1 gives no average.
                serde_json::json!({
                    "@type": "Time",
                    "@value": format!("{h:02}:{m:02}:{sec:02}Z"),
                })
            } else {
                finite(nums.iter().sum::<f64>() / nums.len() as f64)
            }
        }
        "stddev" => {
            if nums.is_empty() {
                Value::Null
            } else {
                let n = nums.len() as f64;
                let mean = nums.iter().sum::<f64>() / n;
                finite((nums.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt())
            }
        }
        "sumsq" => finite(nums.iter().map(|v| v * v).sum::<f64>()),
        _ => Value::Null,
    }
}
