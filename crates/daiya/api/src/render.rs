#![forbid(unsafe_code)]
//! Panel and dashboard query execution, expression pipeline, transformations, server-side HTML
//! first-page table renderer, and the in-process `panel_view` API for the Portal and dataroom (R6, R6b, §9.2).

use crate::state::DaiyaState;
use chrono::Utc;
use daiya_expr::{
    ExprNodeSpec, apply_transformations, evaluate_pipeline, is_expression_datasource_uid,
    parse_duration_ms,
};
use daiya_model::{
    Action, Authorizer, CrError, CrResult, DashboardDocument, DataTransformerConfig, FieldType,
    Frame, FrameSet, GridPos, ObjectTarget, OrgId, PanelSpec, RequestCtx, Subject,
    sanitize_markdown_to_html, validation::html_escape,
};
use daiya_query::{CacheKey, Query, QueryCtx};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

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
}

impl Default for PanelTimeRange {
    fn default() -> Self {
        Self {
            from: Some("now-6h".into()),
            to: Some("now".into()),
            interval_ms: Some(15_000),
            max_data_points: Some(1000),
            variables: BTreeMap::new(),
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

/// Parses a Grafana time string (`"now"`, `"now-6h"`, `"now-30d"`, ISO 8601, or epoch ms string) into epoch milliseconds.
#[must_use]
pub fn parse_time_str_ms(raw: Option<&str>, default_offset_ms: i64) -> i64 {
    let now_ms = Utc::now().timestamp_millis();
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return now_ms + default_offset_ms;
    };
    if s == "now" {
        return now_ms;
    }
    if let Some(rest) = s.strip_prefix("now-") {
        let clean = rest.split('/').next().unwrap_or(rest);
        if let Ok(dur_ms) = parse_duration_ms(clean) {
            return now_ms.saturating_sub(dur_ms);
        }
    }
    if let Ok(epoch_ms) = s.parse::<i64>() {
        return epoch_ms;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.timestamp_millis();
    }
    now_ms + default_offset_ms
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
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map_or_else(|| n.to_string(), |dt| dt.to_rfc3339()),
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
    let principals = req_ctx.subject.principals();

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

        // The answer depends on the target, the variable values interpolated into it and the
        // point budget: all three are in the key, so one caller's variables never serve another's.
        let cache_key = CacheKey::new(
            req_ctx.org_id,
            req_ctx.instance_id,
            &principals,
            &ds_uid,
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

    let dash_record = state
        .store
        .get_dashboard(org_id, dashboard_uid)
        .await?
        .ok_or_else(|| CrError::NotFound(format!("Dashboard '{dashboard_uid}' not found")))?;

    let dash_decision = state.authorizer.check(
        &req_ctx,
        Action::Read,
        &ObjectTarget::Dashboard {
            org_id,
            dashboard_uid: dash_record.uid.clone(),
            folder_uid: dash_record.folder_uid.clone(),
        },
    );
    dash_decision.into_result(format!("Dashboard '{dashboard_uid}' not found"))?;

    let doc: DashboardDocument = serde_json::from_value(dash_record.data.clone())
        .map_err(|e| CrError::Internal(format!("stored dashboard failed to deserialize: {e}")))?;

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
            dashboard_uid: dash_record.uid,
            dashboard_title: dash_record.title,
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
                &req_ctx,
                Action::Read,
                &ObjectTarget::Datasource {
                    org_id,
                    datasource_uid: ds_uid.to_owned(),
                },
            );
            if !dec.allowed {
                let ds_title = state
                    .store
                    .get_datasource(org_id, ds_uid)
                    .await?
                    .map_or_else(|| ds_uid.to_owned(), |d| d.name);
                let msg = format!("You cannot read the source of this panel: {ds_title}");
                return Ok(PanelView {
                    dashboard_uid: dash_record.uid,
                    dashboard_title: dash_record.title,
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
    let from_ms = parse_time_str_ms(
        range
            .from
            .as_deref()
            .or(panel.time_from.as_deref())
            .or(dash_from),
        -6 * 3_600_000,
    );
    let to_ms = parse_time_str_ms(range.to.as_deref().or(dash_to), 0);

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

    let mut frame_sets = if raw_targets.is_empty() {
        Vec::new()
    } else {
        execute_targets_and_expressions(
            state,
            &req_ctx,
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
    if !txs.is_empty() {
        for fs in &mut frame_sets {
            let outcome = apply_transformations(std::mem::take(&mut fs.frames), txs);
            fs.frames = outcome.frames;
        }
    }

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
        dashboard_uid: dash_record.uid,
        dashboard_title: dash_record.title,
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
