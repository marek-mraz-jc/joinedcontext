#![forbid(unsafe_code)]
//! Built-in Prometheus datasource (R3, §3.2, `GF pkg/promlib/models/query.go`):
//! - HTTP API `/api/v1/query`, `/api/v1/query_range`, `/api/v1/labels`, `/api/v1/label/{name}/values`, `/api/v1/series`.
//! - Interpolates Grafana built-in variables (`$__interval`, `$__interval_ms`, `$__range`,
//!   `$__range_s`, `$__range_ms`, `$__rate_interval`, `$__rate_interval_ms`) and template
//!   variables with safe PromQL escaping.
//! - Formats series names with `legendFormat` (`{{label}}` templates).
//! - Supports `format`: `time_series`, `table`, `heatmap`; warns and drops `exemplar` on import.

use crate::guard::{OutboundGuard, OutboundRequest};
use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp, SecretResolver,
};
use controlroom_model::{FieldColumn, Frame, FrameMeta, FrameSet};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Built-in Prometheus datasource implementation (§3.2, R3).
pub struct PrometheusDatasource {
    pub uid: String,
    pub base_url: String,
    pub scrape_interval_ms: u64,
    pub auth_secret_ref: Option<String>,
    pub guard: OutboundGuard,
    pub secrets: Arc<dyn SecretResolver>,
}

impl PrometheusDatasource {
    #[must_use]
    pub fn new(
        uid: impl Into<String>,
        base_url: impl Into<String>,
        guard: OutboundGuard,
        secrets: Arc<dyn SecretResolver>,
    ) -> Self {
        Self {
            uid: uid.into(),
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            scrape_interval_ms: 15_000,
            auth_secret_ref: None,
            guard,
            secrets,
        }
    }

    #[must_use]
    pub fn with_secret_ref(mut self, secret_ref: impl Into<String>) -> Self {
        self.auth_secret_ref = Some(secret_ref.into());
        self
    }

    fn auth_headers(&self, ctx: &QueryCtx) -> Result<Vec<(String, String)>, DsError> {
        let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
        if let Some(secret_ref) = &self.auth_secret_ref {
            let token = self.secrets.resolve_secret(ctx.org_id, secret_ref)?;
            headers.push((
                "Authorization".to_owned(),
                format!("Bearer {}", token.as_str()),
            ));
        }
        Ok(headers)
    }
}

/// Formats a millisecond duration as a Prometheus duration string (`15s`, `1m`, `500ms`).
#[must_use]
pub fn format_prom_interval(ms: u64) -> String {
    let ms = ms.max(1);
    if ms % 86_400_000 == 0 {
        format!("{}d", ms / 86_400_000)
    } else if ms % 3_600_000 == 0 {
        format!("{}h", ms / 3_600_000)
    } else if ms % 60_000 == 0 {
        format!("{}m", ms / 60_000)
    } else if ms % 1_000 == 0 {
        format!("{}s", ms / 1_000)
    } else {
        format!("{ms}ms")
    }
}

/// Escapes a template variable value for safe inclusion inside a PromQL string/regex literal
/// so user-supplied variable values cannot break out of quotes and inject arbitrary PromQL (R12).
#[must_use]
pub fn escape_promql_var(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Interpolates Grafana built-in interval/range variables and dashboard template variables (`query.go:349-396`).
#[must_use]
pub fn interpolate_promql_variables(
    expr: &str,
    step_ms: u64,
    scrape_ms: u64,
    range_ms: u64,
    vars: &BTreeMap<String, String>,
) -> String {
    let step_ms = step_ms.max(1_000);
    let scrape_ms = scrape_ms.max(1_000);
    let rate_interval_ms = (step_ms + scrape_ms).max(4 * scrape_ms);
    let range_s = ((range_ms as f64) / 1000.0).round() as u64;

    let step_str = format_prom_interval(step_ms);
    let rate_str = format_prom_interval(rate_interval_ms);
    let range_str = format!("{range_s}s");

    let mut out = expr.to_owned();
    for (pat, rep) in [
        ("$__interval_ms", step_ms.to_string()),
        ("${__interval_ms}", step_ms.to_string()),
        ("$__interval", step_str.clone()),
        ("${__interval}", step_str),
        ("$__range_ms", range_ms.to_string()),
        ("${__range_ms}", range_ms.to_string()),
        ("$__range_s", range_s.to_string()),
        ("${__range_s}", range_s.to_string()),
        ("$__range", range_str.clone()),
        ("${__range}", range_str),
        ("$__rate_interval_ms", rate_interval_ms.to_string()),
        ("${__rate_interval_ms}", rate_interval_ms.to_string()),
        ("$__rate_interval", rate_str.clone()),
        ("${__rate_interval}", rate_str),
    ] {
        out = out.replace(pat, &rep);
    }

    for (k, v) in vars {
        let escaped = escape_promql_var(v);
        out = out.replace(&format!("${{{k}}}"), &escaped);
        out = out.replace(&format!("${k}"), &escaped);
    }

    out
}

/// Renders a Prometheus `legendFormat` template (`"{{status}} {{method}}"`) against metric labels.
#[must_use]
pub fn format_legend(
    legend_format: Option<&str>,
    metric: &BTreeMap<String, String>,
    fallback_expr: &str,
) -> String {
    let Some(tpl) = legend_format.filter(|s| !s.is_empty()) else {
        if metric.is_empty() {
            return fallback_expr.to_owned();
        }
        let name = metric.get("__name__").map_or("", String::as_str);
        let pairs: Vec<String> = metric
            .iter()
            .filter(|(k, _)| *k != "__name__")
            .map(|(k, v)| format!("{k}=\"{v}\""))
            .collect();
        if pairs.is_empty() {
            return name.to_owned();
        }
        return format!("{name}{{{}}}", pairs.join(", "));
    };

    let mut result = tpl.to_owned();
    let Ok(re) = Regex::new(r"\{\{\s*([a-zA-Z0-9_]+)\s*\}\}") else {
        return result;
    };
    result = re
        .replace_all(&result, |caps: &regex::Captures<'_>| {
            let key = &caps[1];
            metric.get(key).cloned().unwrap_or_default()
        })
        .into_owned();
    result
}

/// Aligns epoch milliseconds down to a multiple of `step_ms` (`query.go:AlignTimeRange`).
#[must_use]
pub const fn align_time_ms(t_ms: i64, step_ms: i64) -> i64 {
    if step_ms <= 0 {
        return t_ms;
    }
    t_ms.div_euclid(step_ms) * step_ms
}

fn parse_prom_response_to_frames(
    ref_id: &str,
    expr: &str,
    legend_format: Option<&str>,
    format: &str,
    payload: &Value,
) -> Result<Vec<Frame>, DsError> {
    let status = payload.get("status").and_then(Value::as_str).unwrap_or("");
    if status != "success" {
        let err_msg = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("prometheus query failed");
        return Err(DsError::Upstream {
            status: 400,
            message: err_msg.to_owned(),
        });
    }

    let data = payload.get("data").unwrap_or(&Value::Null);
    let result_type = data
        .get("resultType")
        .and_then(Value::as_str)
        .unwrap_or("matrix");
    let results = data
        .get("result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if format == "table" {
        return Ok(vec![build_prom_table_frame(ref_id, expr, &results)]);
    }

    let mut frames = Vec::with_capacity(results.len());
    for item in results {
        let metric_labels: BTreeMap<String, String> = item
            .get("metric")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();

        let series_name = format_legend(legend_format, &metric_labels, expr);
        let mut times = Vec::new();
        let mut vals = Vec::new();

        if result_type == "vector" {
            if let Some(pair) = item.get("value").and_then(Value::as_array) {
                if pair.len() == 2 {
                    let ts_sec = pair[0].as_f64().unwrap_or(0.0);
                    let val_str = pair[1].as_str().unwrap_or("NaN");
                    times.push((ts_sec * 1000.0) as i64);
                    vals.push(val_str.parse::<f64>().ok());
                }
            }
        } else if let Some(values_arr) = item.get("values").and_then(Value::as_array) {
            for pair_val in values_arr {
                if let Some(pair) = pair_val.as_array() {
                    if pair.len() == 2 {
                        let ts_sec = pair[0].as_f64().unwrap_or(0.0);
                        let val_str = pair[1].as_str().unwrap_or("NaN");
                        times.push((ts_sec * 1000.0) as i64);
                        vals.push(val_str.parse::<f64>().ok());
                    }
                }
            }
        }

        let mut val_col = FieldColumn::numbers(&series_name, metric_labels, vals);
        val_col.display_name = Some(series_name.clone());

        let mut custom = BTreeMap::new();
        custom.insert(
            "resultType".to_owned(),
            Value::String(result_type.to_owned()),
        );

        frames.push(Frame {
            name: series_name,
            ref_id: Some(ref_id.to_owned()),
            fields: vec![FieldColumn::time("Time", times), val_col],
            meta: FrameMeta {
                executed_query_string: Some(expr.to_owned()),
                custom,
                ..FrameMeta::default()
            },
        });
    }

    Ok(frames)
}

fn build_prom_table_frame(ref_id: &str, expr: &str, results: &[Value]) -> Frame {
    let mut label_keys = std::collections::BTreeSet::new();
    for item in results {
        if let Some(m) = item.get("metric").and_then(Value::as_object) {
            for k in m.keys() {
                label_keys.insert(k.clone());
            }
        }
    }

    let mut times = Vec::with_capacity(results.len());
    let mut values = Vec::with_capacity(results.len());
    let mut label_cols: BTreeMap<String, Vec<Option<String>>> = label_keys
        .iter()
        .map(|k| (k.clone(), Vec::with_capacity(results.len())))
        .collect();

    for item in results {
        let m = item.get("metric").and_then(Value::as_object);
        for k in &label_keys {
            let v = m
                .and_then(|map| map.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if let Some(col) = label_cols.get_mut(k) {
                col.push(v);
            }
        }
        let pair = item
            .get("value")
            .and_then(Value::as_array)
            .or_else(|| {
                item.get("values")
                    .and_then(Value::as_array)
                    .and_then(|a| a.last())
                    .and_then(Value::as_array)
            });
        if let Some(p) = pair {
            let ts = (p.first().and_then(Value::as_f64).unwrap_or(0.0) * 1000.0) as i64;
            let v = p
                .get(1)
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<f64>().ok());
            times.push(ts);
            values.push(v);
        } else {
            times.push(0);
            values.push(None);
        }
    }

    let mut fields = vec![FieldColumn::time("Time", times)];
    for (k, col) in label_cols {
        fields.push(FieldColumn::strings(k, col));
    }
    fields.push(FieldColumn::numbers(
        format!("Value #{ref_id}"),
        BTreeMap::new(),
        values,
    ));

    Frame {
        name: ref_id.to_owned(),
        ref_id: Some(ref_id.to_owned()),
        fields,
        meta: FrameMeta {
            executed_query_string: Some(expr.to_owned()),
            preferred_visualisation_type: Some("table".to_owned()),
            ..FrameMeta::default()
        },
    }
}

impl Datasource for PrometheusDatasource {
    fn kind(&self) -> &'static str {
        "prometheus"
    }

    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        queries: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            let headers = self.auth_headers(ctx)?;
            let range_ms = (ctx.to_ms - ctx.from_ms).max(1_000) as u64;
            let safe_step = (range_ms / 11_000).max(1_000);
            let step_ms = ctx.interval_ms.max(self.scrape_interval_ms).max(safe_step);

            let mut out = Vec::with_capacity(queries.len());
            for q in queries {
                if q.hide {
                    continue;
                }
                let raw_expr = q
                    .model
                    .get("expr")
                    .and_then(Value::as_str)
                    .ok_or_else(|| DsError::InvalidQuery {
                        ref_id: q.ref_id.clone(),
                        reason: "missing 'expr' field".into(),
                    })?;

                let interval_factor = q
                    .model
                    .get("intervalFactor")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1);
                let effective_step_ms = step_ms * interval_factor;

                let expr = interpolate_promql_variables(
                    raw_expr,
                    effective_step_ms,
                    self.scrape_interval_ms,
                    range_ms,
                    &ctx.variables,
                );
                let legend_format = q.model.get("legendFormat").and_then(Value::as_str);
                let format = q
                    .model
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("time_series");
                let instant = q
                    .model
                    .get("instant")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                let encoded_expr = utf8_percent_encode(&expr, NON_ALPHANUMERIC).to_string();
                let url = if instant {
                    let time_sec = (ctx.to_ms as f64) / 1000.0;
                    format!(
                        "{}/api/v1/query?query={encoded_expr}&time={time_sec:.3}",
                        self.base_url
                    )
                } else {
                    let aligned_start =
                        align_time_ms(ctx.from_ms, effective_step_ms as i64) as f64 / 1000.0;
                    let aligned_end =
                        align_time_ms(ctx.to_ms, effective_step_ms as i64) as f64 / 1000.0;
                    let step_sec = (effective_step_ms as f64) / 1000.0;
                    format!(
                        "{}/api/v1/query_range?query={encoded_expr}&start={aligned_start:.3}&end={aligned_end:.3}&step={step_sec:.3}",
                        self.base_url
                    )
                };

                let resp = self
                    .guard
                    .send(OutboundRequest {
                        method: Method::GET,
                        url,
                        headers: headers.clone(),
                        body: None,
                        timeout: Some(ctx.timeout),
                    })
                    .await?;

                if resp.status >= 400 {
                    let text = String::from_utf8_lossy(&resp.body);
                    return Err(DsError::Upstream {
                        status: resp.status,
                        message: text.chars().take(200).collect(),
                    });
                }

                let payload: Value = serde_json::from_slice(&resp.body).map_err(|e| {
                    DsError::Upstream {
                        status: 502,
                        message: format!("invalid JSON from Prometheus: {e}"),
                    }
                })?;

                let frames = parse_prom_response_to_frames(
                    &q.ref_id,
                    &expr,
                    legend_format,
                    format,
                    &payload,
                )?;
                out.push(FrameSet::new(q.ref_id.clone(), frames));
            }

            Ok(out)
        })
    }

    fn health<'a>(&'a self, ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            let Ok(headers) = self.auth_headers(ctx) else {
                return Health::error("failed to resolve Prometheus credentials");
            };
            let url = format!("{}/api/v1/query?query=1%2B1", self.base_url);
            match self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url,
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await
            {
                Ok(resp) if resp.status == 200 => {
                    Health::ok("Data source is working")
                }
                Ok(resp) => Health::error(format!(
                    "Prometheus returned HTTP {}",
                    resp.status
                )),
                Err(e) => Health::error(format!("unreachable: {e}")),
            }
        })
    }

    fn resource<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            let headers = self.auth_headers(ctx)?;
            let path = r.path.trim_matches('/');
            let url = if path == "labels" {
                format!("{}/api/v1/labels", self.base_url)
            } else if let Some(rest) = path.strip_prefix("label/") {
                let label = rest.strip_suffix("/values").unwrap_or(rest);
                let enc = utf8_percent_encode(label, NON_ALPHANUMERIC);
                if let Some(metric) = r.params.get("match[]").or_else(|| r.params.get("metric")) {
                    let enc_m = utf8_percent_encode(metric, NON_ALPHANUMERIC);
                    format!(
                        "{}/api/v1/label/{enc}/values?match%5B%5D={enc_m}",
                        self.base_url
                    )
                } else {
                    format!("{}/api/v1/label/{enc}/values", self.base_url)
                }
            } else if path == "series" {
                let m = r.params.get("match[]").map_or("up", String::as_str);
                let enc_m = utf8_percent_encode(m, NON_ALPHANUMERIC);
                format!("{}/api/v1/series?match%5B%5D={enc_m}", self.base_url)
            } else if let Some(var_q) = r.params.get("query") {
                // Support `label_values(metric, label)` or `label_values(label)` (`k6-load.json:28`)
                if let Some((metric_opt, label)) = parse_label_values_call(var_q) {
                    let enc_l = utf8_percent_encode(&label, NON_ALPHANUMERIC);
                    if let Some(m) = metric_opt {
                        let enc_m = utf8_percent_encode(&m, NON_ALPHANUMERIC);
                        format!(
                            "{}/api/v1/label/{enc_l}/values?match%5B%5D={enc_m}",
                            self.base_url
                        )
                    } else {
                        format!("{}/api/v1/label/{enc_l}/values", self.base_url)
                    }
                } else {
                    return Err(DsError::InvalidQuery {
                        ref_id: "resource".into(),
                        reason: format!("unsupported Prometheus resource query '{var_q}'"),
                    });
                }
            } else {
                return Err(DsError::InvalidQuery {
                    ref_id: "resource".into(),
                    reason: format!("unsupported resource path '{path}'"),
                });
            };

            let resp = self
                .guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url,
                    headers,
                    body: None,
                    timeout: Some(ctx.timeout),
                })
                .await?;
            let data: Value = serde_json::from_slice(&resp.body).map_err(|e| DsError::Upstream {
                status: 502,
                message: format!("invalid JSON in resource response: {e}"),
            })?;
            Ok(ResourceResp { data })
        })
    }

    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "prometheus".into(),
            json_schema: json!({
                "$schema": "https://json-schema.org/draft-04/schema",
                "type": "object",
                "required": ["expr"],
                "properties": {
                    "expr": { "type": "string", "description": "PromQL expression" },
                    "legendFormat": { "type": "string", "description": "Series name template, e.g. {{label}}" },
                    "format": { "type": "string", "enum": ["time_series", "table", "heatmap"] },
                    "instant": { "type": "boolean" },
                    "range": { "type": "boolean" },
                    "interval": { "type": "string" },
                    "intervalFactor": { "type": "integer" }
                }
            }),
            field_hints: vec![
                EditorFieldHint {
                    name: "expr".into(),
                    label: "PromQL Expression".into(),
                    widget: "promql-code".into(),
                    resource_path: Some("labels".into()),
                    required: true,
                },
                EditorFieldHint {
                    name: "legendFormat".into(),
                    label: "Legend".into(),
                    widget: "text".into(),
                    resource_path: None,
                    required: false,
                },
                EditorFieldHint {
                    name: "format".into(),
                    label: "Format".into(),
                    widget: "select".into(),
                    resource_path: None,
                    required: false,
                },
            ],
        }
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let obj = grafana_target.as_object().ok_or_else(|| DsError::InvalidQuery {
            ref_id: "A".into(),
            reason: "target must be a JSON object".into(),
        })?;
        let ref_id = obj
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hide = obj.get("hide").and_then(Value::as_bool).unwrap_or(false);

        let mut warnings = Vec::new();
        let mut model = obj.clone();
        if model.remove("exemplar").as_ref().and_then(Value::as_bool) == Some(true) {
            warnings.push(format!(
                "target '{ref_id}': exemplar flag is not supported and was dropped"
            ));
        }

        Ok(ImportOutcome {
            query: Query {
                ref_id,
                hide,
                model: Value::Object(model),
            },
            warnings,
        })
    }

    fn export(&self, q: &Query) -> Value {
        let mut map = q.model.as_object().cloned().unwrap_or_default();
        map.insert("refId".to_owned(), Value::String(q.ref_id.clone()));
        if q.hide {
            map.insert("hide".to_owned(), Value::Bool(true));
        }
        Value::Object(map)
    }
}

/// Parses `label_values(metric, label)` or `label_values(label)`.
#[must_use]
pub fn parse_label_values_call(q: &str) -> Option<(Option<String>, String)> {
    let trimmed = q.trim();
    let inner = trimmed
        .strip_prefix("label_values(")?
        .strip_suffix(')')?
        .trim();
    if let Some((first, second)) = inner.rsplit_once(',') {
        Some((
            Some(first.trim().to_owned()),
            second.trim().to_owned(),
        ))
    } else if !inner.is_empty() {
        Some((None, inner.to_owned()))
    } else {
        None
    }
}
