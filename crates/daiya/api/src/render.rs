#![forbid(unsafe_code)]
//! Panel and dashboard query execution, expression pipeline, transformations, server-side HTML
//! first-page table renderer, and the in-process `panel_view` API for the Portal and dataroom (R6, R6b, §9.2).

use crate::state::DaiyaState;
use daiya_expr::{
    ExprNodeSpec, apply_transformations, evaluate_pipeline, is_expression_datasource_uid,
};
use daiya_model::{
    Action, Authorizer, CrError, CrResult, DashboardDocument, DashboardRecord,
    DataTransformerConfig, DatasourceRecord, FieldType, Frame, FrameSet, GridPos, Notice,
    NoticeSeverity, ObjectTarget, OrgId, PanelSpec, RequestCtx, Subject, sanitize_markdown_to_html,
    sha256_hex, validation::html_escape,
};
use daiya_query::{CacheKey, Query, QueryCtx};
use jc_time::TimestampTz;
use jiff::{Span, civil, tz::TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// The revision of a datasource in a cache key: its record, hashed. A datasource the host
/// registers has no record and keeps one revision.
pub(crate) fn datasource_revision(record: Option<&DatasourceRecord>) -> CrResult<String> {
    record.map_or_else(
        || Ok(String::new()),
        |record| {
            serde_json::to_vec(record)
                .map(|bytes| sha256_hex(&bytes))
                .map_err(|e| CrError::Internal(format!("datasource revision: {e}")))
        },
    )
}

/// Time range and variable parameters for [`panel_view`] and `Dashboard/RenderPanel` (R6, R6b).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelTimeRange {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub max_data_points: Option<u32>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// The values picked per variable, as repeated `var-<name>` (`$__all` for All), formatted
    /// from the saved variable as Grafana's Prometheus datasource formats them (see
    /// [`format_picked`]); a name here wins over the same name in `variables`.
    #[serde(default)]
    pub picked: BTreeMap<String, Vec<String>>,
}

impl Default for PanelTimeRange {
    fn default() -> Self {
        Self {
            from: Some("now-6h".into()),
            to: Some("now".into()),
            interval_ms: Some(15_000),
            max_data_points: Some(1000),
            variables: BTreeMap::new(),
            picked: BTreeMap::new(),
        }
    }
}

/// Complete rendered panel answer for `<PanelView>`, `<DashboardView>`, and dataroom dataset pages (R6, R6b, §9.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelView {
    pub dashboard_uid: String,
    pub dashboard_title: String,
    pub panel_id: u32,
    pub panel_title: String,
    pub panel_type: String,
    pub description: Option<String>,
    pub grid_pos: Option<GridPos>,
    pub field_config: Value,
    pub options: Value,
    pub frame_sets: Vec<FrameSet>,
    /// Server-side rendered HTML first-page table (or safe Markdown for text panels) for no-JS and accessibility (R6, R6b, §9.2).
    pub html_table: String,
    /// Named empty state when the caller may read the dashboard but not the panel's datasource (§9.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_denied_message: Option<String>,
}

/// Epoch milliseconds of the first and last instant of the years 0..=9999.
const MIN_TIME_MS: i64 = -62_167_219_200_000;
const MAX_TIME_MS: i64 = 253_402_300_799_999;

/// Which end of a time range a string names: `from` defaults to six hours ago and rounds
/// `/unit` down to the unit's start, `to` defaults to now and rounds up to the unit's last
/// millisecond, as Grafana's `dateMath.parse(text, roundUp)` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeEdge {
    From,
    To,
}

impl TimeEdge {
    fn field(self) -> &'static str {
        match self {
            Self::From => "from",
            Self::To => "to",
        }
    }

    /// Six hours ago for `from`, now for `to`.
    fn default_ms(self) -> i64 {
        let offset_ms = match self {
            Self::From => -6 * 3_600_000,
            Self::To => 0,
        };
        TimestampTz::now().as_millis().saturating_add(offset_ms)
    }
}

/// Parses a Grafana time string into epoch milliseconds, clamped to the years 0..=9999 so no
/// range arithmetic downstream can overflow: `now`, date math such as `now-6h`, `now-1M/M`,
/// `now/d` or `now-7d/d` (units `y M w d h m s`, rounding in UTC, weeks from Monday), epoch
/// milliseconds or RFC 3339. Absent or blank is the edge's default. A string it cannot read is a
/// 400 naming the field: it never turns into another range silently (S9-21).
pub fn parse_time_str_ms(raw: Option<&str>, edge: TimeEdge) -> CrResult<i64> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(edge.default_ms());
    };
    let parsed = match s.strip_prefix("now") {
        Some(math) => date_math(TimestampTz::now().as_millis(), math, edge == TimeEdge::To),
        None => s
            .parse::<i64>()
            .ok()
            .or_else(|| jc_time::parse_rfc3339(s).map(|dt| dt.as_millisecond())),
    };
    parsed
        .map(|ms| ms.clamp(MIN_TIME_MS, MAX_TIME_MS))
        .ok_or_else(|| {
            CrError::BadRequest(format!(
                "{}: cannot read the time '{s}'; write now, now-6h, now-1M/M, now/d, epoch \
                 milliseconds or RFC 3339 (units y M w d h m s)",
                edge.field()
            ))
        })
}

/// Applies Grafana date math after `now` (`-6h`, `+1d`, `-1M/M`, `/d`) to `now_ms`; `None` for
/// a string it cannot read.
fn date_math(now_ms: i64, math: &str, round_up: bool) -> Option<i64> {
    let mut ms = now_ms;
    let mut rest = math;
    while let Some(op) = rest.chars().next() {
        rest = &rest[op.len_utf8()..];
        if op == '/' {
            let unit = rest.chars().next()?;
            rest = &rest[unit.len_utf8()..];
            ms = round_to_unit(ms, unit, round_up)?;
            continue;
        }
        let sign = match op {
            '+' => 1,
            '-' => -1,
            _ => return None,
        };
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let n: i64 = if digits == 0 {
            1
        } else {
            rest[..digits].parse().ok()?
        };
        rest = &rest[digits..];
        let unit = rest.chars().next()?;
        rest = &rest[unit.len_utf8()..];
        let n = n.saturating_mul(sign);
        let moved = match unit {
            'y' => add_months(ms, n.saturating_mul(12)),
            'M' => add_months(ms, n),
            'w' => ms.saturating_add(n.saturating_mul(7 * 86_400_000)),
            'd' => ms.saturating_add(n.saturating_mul(86_400_000)),
            'h' | 'H' => ms.saturating_add(n.saturating_mul(3_600_000)),
            'm' => ms.saturating_add(n.saturating_mul(60_000)),
            's' => ms.saturating_add(n.saturating_mul(1_000)),
            _ => return None,
        };
        ms = moved.clamp(MIN_TIME_MS, MAX_TIME_MS);
    }
    Some(ms)
}

/// `ms` moved by `months` calendar months in UTC (the day clamped to the month's last), or the
/// edge of the years 0..=9999 when that leaves them.
fn add_months(ms: i64, months: i64) -> i64 {
    let past_edge = if months < 0 { MIN_TIME_MS } else { MAX_TIME_MS };
    let Ok(span) = Span::new().try_months(months) else {
        return past_edge;
    };
    utc_datetime(ms)
        .and_then(|dt| dt.checked_add(span).ok())
        .and_then(utc_millis)
        .unwrap_or(past_edge)
}

/// `ms` rounded down to the start of `unit` in UTC (weeks start on Monday), or up to its last
/// millisecond; `None` for a unit date math does not know.
fn round_to_unit(ms: i64, unit: char, round_up: bool) -> Option<i64> {
    if !"yMwdhHms".contains(unit) {
        return None;
    }
    // Only the clamped edges of the years 0..=9999 lie outside jiff's range: they stay put.
    let Some(dt) = utc_datetime(ms) else {
        return Some(ms);
    };
    let day = dt.date();
    let (start, step) = match unit {
        'y' => (
            civil::date(dt.year(), 1, 1).at(0, 0, 0, 0),
            Span::new().years(1),
        ),
        'M' => (
            civil::date(dt.year(), dt.month(), 1).at(0, 0, 0, 0),
            Span::new().months(1),
        ),
        'w' => (
            day.checked_sub(Span::new().days(dt.weekday().to_monday_zero_offset()))
                .ok()?
                .at(0, 0, 0, 0),
            Span::new().weeks(1),
        ),
        'd' => (day.at(0, 0, 0, 0), Span::new().days(1)),
        'h' | 'H' => (day.at(dt.hour(), 0, 0, 0), Span::new().hours(1)),
        'm' => (day.at(dt.hour(), dt.minute(), 0, 0), Span::new().minutes(1)),
        's' => (
            day.at(dt.hour(), dt.minute(), dt.second(), 0),
            Span::new().seconds(1),
        ),
        _ => return None,
    };
    if !round_up {
        return utc_millis(start);
    }
    Some(
        start
            .checked_add(step)
            .ok()
            .and_then(utc_millis)
            .map_or(MAX_TIME_MS, |next| next - 1),
    )
}

fn utc_datetime(ms: i64) -> Option<civil::DateTime> {
    Some(
        jiff::Timestamp::from_millisecond(ms)
            .ok()?
            .to_zoned(TimeZone::UTC)
            .datetime(),
    )
}

fn utc_millis(dt: civil::DateTime) -> Option<i64> {
    Some(TimeZone::UTC.to_timestamp(dt).ok()?.as_millisecond())
}

/// A saved time (the dashboard's or the panel's) the parser refuses: the edge's default, and a
/// notice saying so, where a caller's own value is a 400 (S9-21).
pub(crate) fn saved_time_ms(raw: Option<&str>, edge: TimeEdge, notices: &mut Vec<String>) -> i64 {
    parse_time_str_ms(raw, edge).unwrap_or_else(|e| {
        let instead = match edge {
            TimeEdge::From => "starting six hours ago",
            TimeEdge::To => "ending now",
        };
        notices.push(format!(
            "The saved time range is not readable ({e}); this answer shows the range {instead} instead."
        ));
        edge.default_ms()
    })
}

/// Puts each notice on the first frame of the answer, as Grafana's panel header shows frame
/// notices; an answer with no frame gets one empty frame to carry them.
pub(crate) fn attach_notices(frame_sets: &mut Vec<FrameSet>, notices: Vec<String>) {
    if notices.is_empty() {
        return;
    }
    if frame_sets.is_empty() {
        frame_sets.push(FrameSet::new("", Vec::new()));
    }
    let Some(set) = frame_sets.first_mut() else {
        return;
    };
    if set.frames.is_empty() {
        set.frames.push(Frame::default());
    }
    let Some(first) = set.frames.first_mut() else {
        return;
    };
    first
        .meta
        .notices
        .extend(notices.into_iter().map(|text| Notice {
            severity: NoticeSeverity::Warning,
            text,
        }));
}

/// Runs a panel's transformations once over the frames of all its queries, as Grafana does, so
/// `merge`, `joinByField` and `seriesToRows` combine queries (S9-20). Each frame keeps the refId
/// of the query it came from, and the result is grouped back by refId in first-seen order: a
/// combined frame lands in the set its transformation names (`merge`, `joinByField`, ...).
#[must_use]
pub fn transform_panel_frames(
    frame_sets: Vec<FrameSet>,
    txs: &[DataTransformerConfig],
) -> Vec<FrameSet> {
    if txs.is_empty() {
        return frame_sets;
    }
    // The sets stay in query order. A query that answered no frame keeps its (empty) set; one
    // whose frames a combining step consumed leaves no empty set behind.
    let mut sets: Vec<FrameSet> = Vec::with_capacity(frame_sets.len());
    let mut consumed = BTreeSet::new();
    let mut frames = Vec::new();
    for fs in frame_sets {
        if !fs.frames.is_empty() {
            consumed.insert(fs.ref_id.clone());
        }
        let ref_id = fs.ref_id.clone();
        frames.extend(fs.frames.into_iter().map(|mut f| {
            f.ref_id.get_or_insert_with(|| ref_id.clone());
            f
        }));
        sets.push(FrameSet::new(ref_id, Vec::new()));
    }
    for frame in apply_transformations(frames, txs).frames {
        let ref_id = frame.ref_id.clone().unwrap_or_default();
        match sets.iter_mut().find(|s| s.ref_id == ref_id) {
            Some(set) => set.frames.push(frame),
            None => sets.push(FrameSet::new(ref_id, vec![frame])),
        }
    }
    sets.retain(|s| !s.frames.is_empty() || !consumed.contains(&s.ref_id));
    sets
}

/// Renders a list of [`Frame`]s into a safe, accessible server-side HTML `<table>` for no-JS and
/// screen-reader users (R6, R6b, §9.2). All strings, headers, and cell values are HTML-escaped.
#[must_use]
pub fn render_frames_html_table(panel_title: &str, frames: &[Frame], max_rows: usize) -> String {
    let escaped_title = html_escape(panel_title);
    let Some(first) = frames.first().filter(|f| !f.fields.is_empty()) else {
        return format!(
            "<table class=\"cr-panel-table\" aria-label=\"{escaped_title}\"><caption>{escaped_title}</caption><tbody><tr><td>No data</td></tr></tbody></table>"
        );
    };

    let mut html = format!(
        "<table class=\"cr-panel-table\" aria-label=\"{escaped_title}\"><caption>{escaped_title}</caption><thead><tr>"
    );
    for col in &first.fields {
        html.push_str("<th>");
        html.push_str(&html_escape(col.effective_name()));
        html.push_str("</th>");
    }
    html.push_str("</tr></thead><tbody>");

    let row_cnt = first.row_count().min(max_rows);
    for r in 0..row_cnt {
        html.push_str("<tr>");
        for col in &first.fields {
            html.push_str("<td>");
            let val = col.values.get(r).unwrap_or(&Value::Null);
            let text = match (col.field_type, val) {
                (_, Value::Null) => "–".to_owned(),
                (FieldType::Time, Value::Number(n)) => n
                    .as_i64()
                    .and_then(TimestampTz::from_millis)
                    .map_or_else(|| n.to_string(), TimestampTz::to_rfc3339),
                (_, Value::String(s)) => s.clone(),
                (_, other) => other.to_string(),
            };
            html.push_str(&html_escape(&text));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

/// Finds a panel by numeric `panel_id` in `panels` (including nested row panels).
#[must_use]
pub fn find_panel_in_slice(panels: &[PanelSpec], panel_id: u32) -> Option<PanelSpec> {
    for p in panels {
        if p.id == Some(panel_id) && p.panel_type != "row" {
            return Some(p.clone());
        }
        if let Some(nested) = &p.panels
            && let Some(found) = find_panel_in_slice(nested, panel_id)
        {
            return Some(found);
        }
    }
    None
}

/// Executes a mixed list of datasource targets and `__expr__` expressions (R3, R4, R6),
/// deciding each datasource read with the state's authorizer.
#[allow(clippy::too_many_arguments)]
pub async fn execute_targets_and_expressions(
    state: &DaiyaState,
    req_ctx: &RequestCtx,
    default_ds_uid: Option<&str>,
    raw_targets: &[Value],
    from_ms: i64,
    to_ms: i64,
    interval_ms: u64,
    max_data_points: u32,
    variables: &BTreeMap<String, String>,
    skip_cache: bool,
) -> CrResult<Vec<FrameSet>> {
    execute_targets_with_authorizer(
        state,
        state.authorizer.as_ref(),
        req_ctx,
        default_ds_uid,
        raw_targets,
        from_ms,
        to_ms,
        interval_ms,
        max_data_points,
        variables,
        skip_cache,
    )
    .await
}

/// [`execute_targets_and_expressions`] with an explicit authorizer: the public-dashboard door
/// passes its [`daiya_model::PublicDashboardAuthorizer`] instead of the host's.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_targets_with_authorizer(
    state: &DaiyaState,
    authorizer: &dyn Authorizer,
    req_ctx: &RequestCtx,
    default_ds_uid: Option<&str>,
    raw_targets: &[Value],
    from_ms: i64,
    to_ms: i64,
    interval_ms: u64,
    max_data_points: u32,
    variables: &BTreeMap<String, String>,
    skip_cache: bool,
) -> CrResult<Vec<FrameSet>> {
    crate::state::reads_for_someone(&req_ctx.subject)?;
    if raw_targets.len() > state.limits.max_targets_per_panel {
        return Err(CrError::BadRequest(format!(
            "query request has {} targets, exceeding limit of {}",
            raw_targets.len(),
            state.limits.max_targets_per_panel
        )));
    }

    if to_ms <= from_ms {
        return Err(CrError::BadRequest(
            "invalid time range: 'to' must be after 'from'".into(),
        ));
    }
    // Never more than 11 000 points per series (Grafana's safe resolution), whatever the caller asks.
    let span_ms = u64::try_from(to_ms.saturating_sub(from_ms)).unwrap_or(u64::MAX);
    let interval_ms = interval_ms.max(span_ms / 11_000).max(1);

    let mut ds_targets: Vec<(String, Query, Value)> = Vec::new();
    let mut hidden_ds_refs: BTreeSet<String> = BTreeSet::new();
    let mut expr_nodes: Vec<ExprNodeSpec> = Vec::new();
    let mut seen_refs: BTreeSet<String> = BTreeSet::new();

    for t in raw_targets {
        let ref_id = t
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        if !seen_refs.insert(ref_id.clone()) {
            return Err(CrError::BadRequest(format!(
                "multiple queries using the same RefId '{ref_id}' is not allowed"
            )));
        }

        let ds_uid = t
            .get("datasource")
            .and_then(|d| d.get("uid").or(Some(d)))
            .and_then(Value::as_str)
            .or(default_ds_uid)
            .unwrap_or("");
        let ds_type = t
            .get("datasource")
            .and_then(|d| d.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if is_expression_datasource_uid(ds_uid) || is_expression_datasource_uid(ds_type) {
            let node = ExprNodeSpec::from_json(t, state.limits.max_expression_depth)?;
            expr_nodes.push(node);
        } else {
            if ds_uid.is_empty() || ds_uid == "-- Mixed --" {
                return Err(CrError::BadRequest(format!(
                    "Query {ref_id} is missing datasource information"
                )));
            }
            let hide = t.get("hide").and_then(Value::as_bool).unwrap_or(false);
            if hide {
                hidden_ds_refs.insert(ref_id.clone());
            }
            ds_targets.push((
                ds_uid.to_owned(),
                Query {
                    ref_id,
                    hide: false, // Run hidden datasource queries if expressions depend on them; pipeline filters them out at the end
                    model: t.clone(),
                },
                t.clone(),
            ));
        }
    }

    let has_expressions = !expr_nodes.is_empty();
    let q_ctx = QueryCtx {
        org_id: req_ctx.org_id,
        instance_id: req_ctx.instance_id,
        subject: req_ctx.subject.clone(),
        from_ms,
        to_ms,
        max_data_points,
        interval_ms,
        timeout: Duration::from_secs(10),
        variables: variables.clone(),
    };

    let mut ds_frame_sets: Vec<FrameSet> = Vec::with_capacity(ds_targets.len());
    // Whom a server administrator reads for (a token digest, or the public) is a principal of
    // the cache key too.
    let person = req_ctx
        .subject
        .on_behalf_of
        .as_ref()
        .map(|sub| format!("person:{sub}"));
    // The credential a driver forwards shapes the rows an Endpoint answers: two tokens of one
    // subject (a person and a narrower Connection, a token minted before a revocation) never
    // share an entry.
    let credential = req_ctx
        .subject
        .bearer_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .map(|token| format!("credential:{}", daiya_model::sha256_hex(token.as_bytes())));
    let mut principals = req_ctx.subject.principals();
    principals.extend(person.as_deref());
    principals.extend(credential.as_deref());

    for (ds_uid, query, raw_target) in ds_targets {
        if !has_expressions && hidden_ds_refs.contains(&query.ref_id) {
            continue;
        }

        // Check authorization on the datasource in the caller's organization (R9, §5.3 test 3)
        let ds_meta = state.store.get_datasource(req_ctx.org_id, &ds_uid).await?;
        let registered = state.get_datasource_driver(req_ctx.org_id, &ds_uid);

        if ds_meta.is_none() && registered.is_none() {
            return Err(CrError::NotFound(format!(
                "Data source '{ds_uid}' not found"
            )));
        }

        let decision = authorizer.check(
            req_ctx,
            Action::Read,
            &ObjectTarget::Datasource {
                org_id: req_ctx.org_id,
                datasource_uid: ds_uid.clone(),
            },
        );
        decision.into_result(format!("Data source '{ds_uid}' not found"))?;
        // WRK-04: nobody in particular reads an Endpoint by its public Policy, never a source that
        // holds a credential or a driver of the host's. Judged on the record the driver is built
        // from, before the cache.
        if req_ctx.subject.is_public_dashboard()
            && (registered.is_some()
                || !ds_meta
                    .as_ref()
                    .is_some_and(|record| state.reads_endpoint(record)))
        {
            return Err(CrError::Forbidden(crate::public::SOURCE_REFUSED.into()));
        }

        // The answer depends on the target, the variable values interpolated into it and the
        // point budget: all three are in the key, so one caller's variables never serve another's.
        let cache_key = CacheKey::new(
            req_ctx.org_id,
            req_ctx.instance_id,
            &principals,
            &ds_uid,
            &datasource_revision(ds_meta.as_ref())?,
            &json!({
                "target": raw_target,
                "variables": variables,
                "maxDataPoints": max_data_points,
            }),
            from_ms,
            to_ms,
            interval_ms,
        );

        if !skip_cache && let Some(cached) = state.query_cache.get(&cache_key) {
            ds_frame_sets.extend(cached);
            continue;
        }

        // Built only after the read decision: building resolves the datasource's host.
        let drv = match (registered, &ds_meta) {
            (Some(driver), _) => driver,
            (None, Some(record)) => state.driver_for_record(record).await?,
            (None, None) => {
                return Err(CrError::NotFound(format!(
                    "Data source '{ds_uid}' not found"
                )));
            }
        };

        let sets = drv.query(&q_ctx, &[query]).await?;
        state
            .query_cache
            .insert(cache_key, sets.clone(), Duration::from_secs(30));
        ds_frame_sets.extend(sets);
    }

    if has_expressions {
        let out = evaluate_pipeline(
            &ds_frame_sets,
            &hidden_ds_refs,
            &expr_nodes,
            from_ms,
            to_ms,
            state.limits.max_expression_depth,
        )?;
        Ok(out.frame_sets)
    } else {
        Ok(ds_frame_sets)
    }
}

/// In-process Rust function that the Portal and dataroom crates call directly when mounted in one
/// binary (R6, R6b, §9.2): evaluates a panel's targets, expressions, and transformations and returns
/// its frames, `fieldConfig`, `options`, and server-rendered HTML table.
pub async fn panel_view(
    state: &DaiyaState,
    subject: &Subject,
    org_id: OrgId,
    dashboard_uid: &str,
    panel_id: u32,
    range: PanelTimeRange,
) -> CrResult<PanelView> {
    let req_ctx = RequestCtx::new(
        state.installation_id,
        org_id,
        state.default_instance_id,
        subject.clone(),
    );
    let dash_record = load_dashboard_for_read(state, &req_ctx, dashboard_uid).await?;
    let doc = dashboard_document(&dash_record)?;
    render_panel(state, &req_ctx, &dash_record, &doc, panel_id, range).await
}

/// The stored dashboard `dashboard_uid`, once the caller in `req_ctx` may read it; a page of
/// panels loads and authorises it once and renders each panel with [`render_panel`].
pub async fn load_dashboard_for_read(
    state: &DaiyaState,
    req_ctx: &RequestCtx,
    dashboard_uid: &str,
) -> CrResult<DashboardRecord> {
    let org_id = req_ctx.org_id;
    let dash_record = state
        .store
        .get_dashboard(org_id, dashboard_uid)
        .await?
        .ok_or_else(|| CrError::NotFound(format!("Dashboard '{dashboard_uid}' not found")))?;

    let dash_decision = state.authorizer.check(
        req_ctx,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id,
            dashboard_uid: dash_record.uid.clone(),
            folder_uid: dash_record.folder_uid.clone(),
        },
    );
    dash_decision.into_result(format!("Dashboard '{dashboard_uid}' not found"))?;
    Ok(dash_record)
}

/// The document a stored dashboard holds.
pub fn dashboard_document(record: &DashboardRecord) -> CrResult<DashboardDocument> {
    serde_json::from_value(record.data.clone())
        .map_err(|e| CrError::Internal(format!("stored dashboard failed to deserialize: {e}")))
}

/// Panel `panel_id` of a dashboard [`load_dashboard_for_read`] loaded for `req_ctx`: each of its
/// sources is still checked, the dashboard is not loaded or decided again.
pub async fn render_panel(
    state: &DaiyaState,
    req_ctx: &RequestCtx,
    dash_record: &DashboardRecord,
    doc: &DashboardDocument,
    panel_id: u32,
    range: PanelTimeRange,
) -> CrResult<PanelView> {
    let org_id = req_ctx.org_id;
    let dashboard_uid = dash_record.uid.as_str();
    let panels = doc.panels.as_deref().unwrap_or(&[]);
    let panel = find_panel_in_slice(panels, panel_id).ok_or_else(|| {
        CrError::NotFound(format!(
            "Panel {panel_id} not found on dashboard '{dashboard_uid}'"
        ))
    })?;

    let panel_title = panel
        .title
        .clone()
        .unwrap_or_else(|| format!("Panel {panel_id}"));
    let field_config = panel
        .field_config
        .as_ref()
        .and_then(|fc| serde_json::to_value(fc).ok())
        .unwrap_or_else(|| json!({ "defaults": {}, "overrides": [] }));
    let options = panel
        .options
        .as_ref()
        .and_then(|o| serde_json::to_value(o).ok())
        .unwrap_or_else(|| json!({}));

    // Text panel: render safe Markdown subset on the server (CVE-2023-22462 mitigation, §4.3)
    if panel.panel_type == "text" {
        let md_content = panel
            .options
            .as_ref()
            .and_then(|o| o.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let safe_html = sanitize_markdown_to_html(md_content);
        return Ok(PanelView {
            dashboard_uid: dash_record.uid.clone(),
            dashboard_title: dash_record.title.clone(),
            panel_id,
            panel_title,
            panel_type: panel.panel_type,
            description: panel.description,
            grid_pos: panel.grid_pos,
            field_config,
            options,
            frame_sets: Vec::new(),
            html_table: safe_html,
            access_denied_message: None,
        });
    }

    let default_ds_uid = panel.datasource.as_ref().and_then(|d| d.uid.as_deref());

    // Check if the caller lacks read access to any datasource of this panel (§9.2 named empty state)
    let raw_targets: Vec<Value> = panel
        .targets
        .as_ref()
        .map(|ts| {
            ts.iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect()
        })
        .unwrap_or_default();

    for t in &raw_targets {
        let ds_uid = t
            .get("datasource")
            .and_then(|d| d.get("uid"))
            .and_then(Value::as_str)
            .or(default_ds_uid)
            .unwrap_or("");
        if !ds_uid.is_empty() && ds_uid != "-- Mixed --" && !is_expression_datasource_uid(ds_uid) {
            let dec = state.authorizer.check(
                req_ctx,
                Action::Read,
                &ObjectTarget::Datasource {
                    org_id,
                    datasource_uid: ds_uid.to_owned(),
                },
            );
            if !dec.allowed {
                // Only the uid the dashboard already carries: the stored name, or whether the
                // source exists at all, is not told to a caller who may not read it.
                let msg = format!("You cannot read the source of this panel: {ds_uid}");
                return Ok(PanelView {
                    dashboard_uid: dash_record.uid.clone(),
                    dashboard_title: dash_record.title.clone(),
                    panel_id,
                    panel_title: panel_title.clone(),
                    panel_type: panel.panel_type,
                    description: panel.description,
                    grid_pos: panel.grid_pos,
                    field_config,
                    options,
                    frame_sets: Vec::new(),
                    html_table: render_frames_html_table(&panel_title, &[], 50),
                    access_denied_message: Some(msg),
                });
            }
        }
    }

    let dash_from = doc.time.as_ref().map(|t| t.from.as_str());
    let dash_to = doc.time.as_ref().map(|t| t.to.as_str());
    // A caller's own range that does not parse is a 400 naming the field; a saved one falls back
    // to the default with a notice on the answer. A panel's `timeFrom` ("2h") is Grafana's
    // relative override: the last two hours.
    let mut notices = Vec::new();
    let panel_from = panel
        .time_from
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            if t.starts_with("now") {
                t.to_owned()
            } else {
                format!("now-{t}")
            }
        });
    let from_ms = match range.from.as_deref() {
        Some(own) => parse_time_str_ms(Some(own), TimeEdge::From)?,
        None => saved_time_ms(
            panel_from.as_deref().or(dash_from),
            TimeEdge::From,
            &mut notices,
        ),
    };
    let to_ms = match range.to.as_deref() {
        Some(own) => parse_time_str_ms(Some(own), TimeEdge::To)?,
        None => saved_time_ms(dash_to, TimeEdge::To, &mut notices),
    };

    // Merge dashboard default variable values with caller-supplied variable values
    let mut vars = BTreeMap::new();
    if let Some(templating) = &doc.templating {
        for v in &templating.list {
            if let Some(curr) = &v.current
                && let Some(val_str) = curr.value.as_ref().and_then(Value::as_str)
            {
                if val_str == "$__all" {
                    if let Some(all_v) = &v.all_value {
                        vars.insert(v.name.clone(), all_v.clone());
                    }
                } else {
                    vars.insert(v.name.clone(), val_str.to_owned());
                }
            }
            if !vars.contains_key(&v.name)
                && let Some(all_v) = &v.all_value
            {
                vars.insert(v.name.clone(), all_v.clone());
            }
        }
    }
    for (k, v) in range.variables {
        vars.insert(k, v);
    }
    let saved = doc
        .templating
        .as_ref()
        .map_or(&[][..], |t| t.list.as_slice());
    for (name, values) in range.picked {
        let variable = saved.iter().find(|v| v.name == name);
        vars.insert(name, format_picked(variable, &values));
    }

    let mut frame_sets = if raw_targets.is_empty() {
        Vec::new()
    } else {
        execute_targets_and_expressions(
            state,
            req_ctx,
            default_ds_uid,
            &raw_targets,
            from_ms,
            to_ms,
            range.interval_ms.unwrap_or(15_000),
            range.max_data_points.unwrap_or(1000),
            &vars,
            false,
        )
        .await?
    };

    // Apply panel transformations (R4)
    let txs: &[DataTransformerConfig] = panel.transformations.as_deref().unwrap_or(&[]);
    frame_sets = transform_panel_frames(frame_sets, txs);
    attach_notices(&mut frame_sets, notices);

    // Apply table panel options.sortBy if configured (`bbsk.json:145`, `helsinki.json:274`)
    if let Some(sort_by) = panel
        .options
        .as_ref()
        .and_then(|o| o.get("sortBy"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        let display_name = sort_by
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or("");
        let desc = sort_by
            .get("desc")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !display_name.is_empty() {
            for fs in &mut frame_sets {
                for frame in &mut fs.frames {
                    if let Some(idx) = frame
                        .fields
                        .iter()
                        .position(|f| f.name == display_name || f.effective_name() == display_name)
                    {
                        daiya_expr::tx::sort_frame_by_col(frame, idx, desc);
                    }
                }
            }
        }
    }

    let all_frames: Vec<Frame> = frame_sets
        .iter()
        .flat_map(|fs| fs.frames.iter().cloned())
        .collect();
    let html_table = render_frames_html_table(&panel_title, &all_frames, 50);

    Ok(PanelView {
        dashboard_uid: dash_record.uid.clone(),
        dashboard_title: dash_record.title.clone(),
        panel_id,
        panel_title,
        panel_type: panel.panel_type,
        description: panel.description,
        grid_pos: panel.grid_pos,
        field_config,
        options,
        frame_sets,
        html_table,
        access_denied_message: None,
    })
}

/// A regex sign Grafana's Prometheus datasource escapes in a picked value
/// (`prometheusSpecialRegexEscape`).
fn regex_escaped(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if "\\$^*{}[]'+?.()|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The text the picked `values` of `variable` stand for in a query, as Grafana 12's Prometheus
/// datasource formats them (`interpolateQueryExpr`): a single-value variable as picked; a
/// multi-value or All variable regex-escaped, several values as `(a|b)`; All as the variable's
/// `allValue` unchanged, else every value it offers. The quoting of the literal the text lands
/// in is left to the query's own interpolation (`interpolate_promql_variables`), so a `\` here
/// becomes the `\\` Grafana writes into the PromQL string.
#[must_use]
pub fn format_picked(variable: Option<&daiya_model::VariableModel>, values: &[String]) -> String {
    let multi = variable.is_some_and(|v| v.multi == Some(true) || v.include_all == Some(true));
    let all = values.iter().any(|v| v == "$__all");
    let values: Vec<String> = if all {
        if let Some(all_value) = variable
            .and_then(|v| v.all_value.as_deref())
            .filter(|v| !v.is_empty())
        {
            return all_value.to_owned();
        }
        variable
            .and_then(|v| v.options.as_ref())
            .into_iter()
            .flatten()
            .filter_map(|o| o.value.as_ref().and_then(Value::as_str))
            .filter(|v| *v != "$__all")
            .map(str::to_owned)
            .collect()
    } else {
        values.to_vec()
    };
    match values.as_slice() {
        [] => String::new(),
        [one] if !multi => one.clone(),
        [one] => regex_escaped(one),
        many => format!(
            "({})",
            many.iter()
                .map(|v| regex_escaped(v))
                .collect::<Vec<_>>()
                .join("|")
        ),
    }
}
