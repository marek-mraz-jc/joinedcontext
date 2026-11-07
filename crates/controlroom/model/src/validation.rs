#![forbid(unsafe_code)]
//! CUE-derived dashboard validation (`kinds/dashboard/dashboard_kind.cue`), §7.1 compatibility
//! matrix verdicts (`supports`, `maps`, `refuses`), and safe Markdown/XSS sanitization (R1, R2, R12).

use crate::dashboard::{DashboardDocument, PanelSpec};
use crate::error::{CrError, CrResult};
use crate::limits::InputLimits;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// §7.1 compatibility verdict for a dashboard feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// Kept and drawn natively.
    Supports,
    /// Kept in the document, drawn through a named equivalent.
    Maps,
    /// Kept in the document for export, not drawn; reported with a JSON pointer.
    Refuses,
}

/// One feature finding with its JSON pointer and §7.1 verdict code (R2, §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FeatureFinding {
    pub pointer: String,
    pub code: String,
    pub verdict: Verdict,
    pub message: String,
}

/// Complete validation and compatibility report for one dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub uid: String,
    pub title: String,
    pub valid: bool,
    pub findings: Vec<FeatureFinding>,
    pub referenced_datasources: Vec<String>,
    pub missing_datasources: Vec<String>,
}

impl ValidationReport {
    /// Returns all findings whose verdict is `Refuses`.
    #[must_use]
    pub fn refusals(&self) -> Vec<&FeatureFinding> {
        self.findings
            .iter()
            .filter(|f| f.verdict == Verdict::Refuses)
            .collect()
    }

    /// Returns all findings whose verdict is `Maps`.
    #[must_use]
    pub fn mappings(&self) -> Vec<&FeatureFinding> {
        self.findings
            .iter()
            .filter(|f| f.verdict == Verdict::Maps)
            .collect()
    }
}

/// Supported panel types (§1.3 "used" and "needed", §7.1).
const SUPPORTED_PANELS: &[&str] = &[
    "timeseries",
    "table",
    "stat",
    "gauge",
    "barchart",
    "piechart",
    "text",
    "logs",
    "heatmap",
    "histogram",
    "state-timeline",
    "nodeGraph",
    "row",
];

/// Mapped panel types (§7.1: `bargauge`, `geomap`, `jc-vegalite`, `library-panel-ref`).
const MAPPED_PANELS: &[&str] = &["bargauge", "geomap", "jc-vegalite", "library-panel-ref"];

/// Built-in supported datasource types (§3.2).
const SUPPORTED_DATASOURCE_TYPES: &[&str] = &[
    "prometheus",
    "ngsild",
    "ngsild-grafana-datasource",
    "odata",
    "postgres",
    "grafana-postgresql-datasource",
    "loki",
    "__expr__",
    "datasource",
    "grafana",
];

/// Supported server-side expression types (R4, §3.4).
const SUPPORTED_EXPR_TYPES: &[&str] = &[
    "math",
    "reduce",
    "resample",
    "threshold",
    "classic_conditions",
];

/// Supported transformation IDs (R4: `organize` + the first 12 of §3.4).
pub const SUPPORTED_TRANSFORMATIONS: &[&str] = &[
    "organize",
    "merge",
    "reduce",
    "filterByValue",
    "calculateField",
    "groupBy",
    "sortBy",
    "renameByRegex",
    "joinByField",
    "seriesToColumns",
    "convertFieldType",
    "limit",
    "seriesToRows",
    "labelsToFields",
];

/// Supported variable types (§1.3, §7.1).
const SUPPORTED_VARIABLE_TYPES: &[&str] = &[
    "query",
    "custom",
    "constant",
    "interval",
    "textbox",
    "datasource",
    "system",
    "switch",
];

/// Validates a dashboard JSON document against the CUE schema rules, R12 input caps, and the §7.1
/// compatibility matrix.
pub fn classify_and_validate(
    raw_bytes_len: usize,
    doc: &DashboardDocument,
    known_datasource_uids: &BTreeSet<String>,
    limits: &InputLimits,
) -> CrResult<ValidationReport> {
    if raw_bytes_len > limits.max_dashboard_json_bytes {
        return Err(CrError::BadRequest(format!(
            "dashboard JSON size {} bytes exceeds maximum allowed {} bytes",
            raw_bytes_len, limits.max_dashboard_json_bytes
        )));
    }

    let title = doc.title.as_deref().unwrap_or("").trim();
    if title.is_empty() {
        return Err(CrError::Unprocessable {
            detail: "dashboard title must not be empty".into(),
            errors: vec!["/title: title is required".into()],
        });
    }

    if let Some(uid) = &doc.uid
        && (uid.len() > 40
            || !uid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err(CrError::Unprocessable {
            detail: format!(
                "dashboard uid '{uid}' is invalid (must be 1..=40 ASCII alphanumeric, '-' or '_')"
            ),
            errors: vec![format!("/uid: invalid uid '{uid}'")],
        });
    }

    if let Some(month) = doc.fiscal_year_start_month
        && month >= 12
    {
        return Err(CrError::Unprocessable {
            detail: format!("fiscalYearStartMonth must be < 12, got {month}"),
            errors: vec![format!("/fiscalYearStartMonth: must be < 12, got {month}")],
        });
    }

    if let Some(gt) = doc.graph_tooltip
        && gt > 2
    {
        return Err(CrError::Unprocessable {
            detail: format!("graphTooltip must be 0, 1, or 2, got {gt}"),
            errors: vec![format!("/graphTooltip: invalid value {gt}")],
        });
    }

    // Validate dashboard links for dangerous schemes (javascript:, data:, vbscript:)
    if let Some(links) = &doc.links {
        for (idx, link) in links.iter().enumerate() {
            if let Some(url) = &link.url
                && is_dangerous_url(url)
            {
                return Err(CrError::Unprocessable {
                    detail: format!("dangerous URL scheme in dashboard link at /links/{idx}/url"),
                    errors: vec![format!("/links/{idx}/url: disallowed URL scheme")],
                });
            }
        }
    }

    let mut findings = Vec::new();
    let mut referenced_ds = BTreeSet::new();
    let mut total_panels = 0usize;

    // Check templating variables
    if let Some(templating) = &doc.templating {
        for (v_idx, var) in templating.list.iter().enumerate() {
            let ptr = format!("/templating/list/{v_idx}");
            if SUPPORTED_VARIABLE_TYPES.contains(&var.var_type.as_str()) {
                findings.push(FeatureFinding {
                    pointer: ptr.clone(),
                    code: format!("variable.{}", var.var_type),
                    verdict: Verdict::Supports,
                    message: format!("variable '{}' ({}) supported", var.name, var.var_type),
                });
            } else {
                findings.push(FeatureFinding {
                    pointer: ptr.clone(),
                    code: format!("variable.{}", var.var_type),
                    verdict: Verdict::Refuses,
                    message: format!(
                        "variable type '{}' requires per-datasource adhoc/groupby support",
                        var.var_type
                    ),
                });
            }
            if let Some(ds) = &var.datasource
                && let Some(uid) = &ds.uid
                && !uid.is_empty()
                && uid != "-- Mixed --"
                && uid != "__expr__"
            {
                referenced_ds.insert(uid.clone());
            }
        }
    }

    // Check panels
    if let Some(panels) = &doc.panels {
        validate_panels_slice(
            panels,
            "/panels",
            limits,
            &mut total_panels,
            &mut findings,
            &mut referenced_ds,
        )?;
    }

    if total_panels > limits.max_panels_per_dashboard {
        return Err(CrError::BadRequest(format!(
            "dashboard has {} panels, exceeding limit of {}",
            total_panels, limits.max_panels_per_dashboard
        )));
    }

    let missing_datasources: Vec<String> = referenced_ds
        .iter()
        .filter(|uid| !known_datasource_uids.is_empty() && !known_datasource_uids.contains(*uid))
        .cloned()
        .collect();

    for missing in &missing_datasources {
        findings.push(FeatureFinding {
            pointer: "/panels".to_owned(),
            code: "datasource.missing".to_owned(),
            verdict: Verdict::Refuses,
            message: format!("datasource not found: {missing}"),
        });
    }

    Ok(ValidationReport {
        uid: doc.uid.clone().unwrap_or_default(),
        title: title.to_owned(),
        valid: true,
        findings,
        referenced_datasources: referenced_ds.into_iter().collect(),
        missing_datasources,
    })
}

fn validate_panels_slice(
    panels: &[PanelSpec],
    base_ptr: &str,
    limits: &InputLimits,
    total_panels: &mut usize,
    findings: &mut Vec<FeatureFinding>,
    referenced_ds: &mut BTreeSet<String>,
) -> CrResult<()> {
    for (idx, panel) in panels.iter().enumerate() {
        let ptr = format!("{base_ptr}/{idx}");
        if panel.panel_type.trim().is_empty() {
            return Err(CrError::Unprocessable {
                detail: format!("panel at {ptr} has an empty type"),
                errors: vec![format!("{ptr}/type: must not be empty")],
            });
        }

        if let Some(gp) = &panel.grid_pos
            && (gp.h == 0 || gp.w == 0 || gp.w > 24 || gp.x >= 24)
        {
            return Err(CrError::Unprocessable {
                detail: format!(
                    "panel at {ptr} has invalid gridPos (h={}, w={}, x={}; CUE requires h>0, 0<w<=24, 0<=x<24)",
                    gp.h, gp.w, gp.x
                ),
                errors: vec![format!("{ptr}/gridPos: out of 24-column grid bounds")],
            });
        }

        if let Some(links) = &panel.links {
            for (l_idx, link) in links.iter().enumerate() {
                if let Some(url) = &link.url
                    && is_dangerous_url(url)
                {
                    return Err(CrError::Unprocessable {
                        detail: format!(
                            "dangerous URL scheme in panel link at {ptr}/links/{l_idx}/url"
                        ),
                        errors: vec![format!("{ptr}/links/{l_idx}/url: disallowed URL scheme")],
                    });
                }
            }
        }

        // Data links live in fieldConfig.defaults.links and in override properties
        // (`{"id": "links", "value": [{"url": ...}]}`); every `url` string is checked.
        if let Some(fc) = &panel.field_config
            && let Ok(fc_value) = serde_json::to_value(fc)
            && let Some(bad) = find_dangerous_url(&fc_value, &format!("{ptr}/fieldConfig"))
        {
            return Err(CrError::Unprocessable {
                detail: format!("dangerous URL scheme in data link at {bad}"),
                errors: vec![format!("{bad}: disallowed URL scheme")],
            });
        }

        if panel.panel_type == "row" {
            findings.push(FeatureFinding {
                pointer: format!("{ptr}/type"),
                code: "panel.row".to_owned(),
                verdict: Verdict::Supports,
                message: "row panel supported".to_owned(),
            });
            if let Some(nested) = &panel.panels {
                validate_panels_slice(
                    nested,
                    &format!("{ptr}/panels"),
                    limits,
                    total_panels,
                    findings,
                    referenced_ds,
                )?;
            }
            continue;
        }

        *total_panels += 1;

        if panel.library_panel.is_some() {
            findings.push(FeatureFinding {
                pointer: format!("{ptr}/libraryPanel"),
                code: "panel.libraryPanel".to_owned(),
                verdict: Verdict::Maps,
                message: "libraryPanel resolved inline and link kept".to_owned(),
            });
        }

        // Panel type classification (§7.1)
        if SUPPORTED_PANELS.contains(&panel.panel_type.as_str()) {
            findings.push(FeatureFinding {
                pointer: format!("{ptr}/type"),
                code: format!("panel.{}", panel.panel_type),
                verdict: Verdict::Supports,
                message: format!("panel type '{}' is supported", panel.panel_type),
            });
        } else if MAPPED_PANELS.contains(&panel.panel_type.as_str()) {
            if panel.panel_type == "bargauge" {
                let dm = panel
                    .options
                    .as_ref()
                    .and_then(|o| o.get("displayMode"))
                    .and_then(Value::as_str)
                    .unwrap_or("gradient");
                let msg = if dm == "lcd" || dm == "gradient" {
                    "bargauge.displayMode mapped to SDK bar list (basic)".to_owned()
                } else {
                    "bargauge mapped to SDK bar list".to_owned()
                };
                findings.push(FeatureFinding {
                    pointer: format!("{ptr}/type"),
                    code: "bargauge.displayMode".to_owned(),
                    verdict: Verdict::Maps,
                    message: msg,
                });
            } else if panel.panel_type == "geomap" {
                let layers = panel
                    .options
                    .as_ref()
                    .and_then(|o| o.get("layers"))
                    .and_then(Value::as_array);
                let non_markers = layers.is_some_and(|ls| {
                    ls.iter()
                        .any(|l| l.get("type").and_then(Value::as_str) != Some("markers"))
                });
                if non_markers {
                    findings.push(FeatureFinding {
                        pointer: format!("{ptr}/options/layers"),
                        code: "geomap.layerType".to_owned(),
                        verdict: Verdict::Refuses,
                        message: "geomap layer type other than 'markers' is refused".to_owned(),
                    });
                } else {
                    findings.push(FeatureFinding {
                        pointer: format!("{ptr}/type"),
                        code: "panel.geomap".to_owned(),
                        verdict: Verdict::Maps,
                        message: "geomap markers layer mapped to SDK map".to_owned(),
                    });
                }
            } else {
                findings.push(FeatureFinding {
                    pointer: format!("{ptr}/type"),
                    code: format!("panel.{}", panel.panel_type),
                    verdict: Verdict::Maps,
                    message: format!("panel type '{}' mapped", panel.panel_type),
                });
            }
        } else {
            findings.push(FeatureFinding {
                pointer: format!("{ptr}/type"),
                code: format!("panel.unsupported.{}", panel.panel_type),
                verdict: Verdict::Refuses,
                message: format!(
                    "panel type {} has no controlroom renderer",
                    panel.panel_type
                ),
            });
        }

        // Panel datasource check
        if let Some(ds) = &panel.datasource {
            check_datasource_ref(
                ds.ds_type.as_deref(),
                ds.uid.as_deref(),
                &format!("{ptr}/datasource"),
                findings,
                referenced_ds,
            );
        }

        // Targets check
        if let Some(targets) = &panel.targets {
            if targets.len() > limits.max_targets_per_panel {
                return Err(CrError::BadRequest(format!(
                    "panel at {ptr} has {} targets, exceeding limit of {}",
                    targets.len(),
                    limits.max_targets_per_panel
                )));
            }
            for (t_idx, target) in targets.iter().enumerate() {
                let t_ptr = format!("{ptr}/targets/{t_idx}");
                let ds_obj = target.get("datasource").and_then(Value::as_object);
                let ds_type = ds_obj
                    .and_then(|d| d.get("type"))
                    .and_then(Value::as_str)
                    .or_else(|| panel.datasource.as_ref().and_then(|d| d.ds_type.as_deref()));
                let ds_uid = ds_obj
                    .and_then(|d| d.get("uid"))
                    .and_then(Value::as_str)
                    .or_else(|| panel.datasource.as_ref().and_then(|d| d.uid.as_deref()));

                if ds_uid == Some("__expr__")
                    || ds_uid == Some("-100")
                    || ds_type == Some("__expr__")
                {
                    let expr_type = target.get("type").and_then(Value::as_str).unwrap_or("math");
                    if expr_type == "sql" {
                        findings.push(FeatureFinding {
                            pointer: format!("{t_ptr}/type"),
                            code: "expr.sql".to_owned(),
                            verdict: Verdict::Maps,
                            message: "sql expression type imported with warning (not in this run)"
                                .to_owned(),
                        });
                    } else if SUPPORTED_EXPR_TYPES.contains(&expr_type) {
                        findings.push(FeatureFinding {
                            pointer: format!("{t_ptr}/type"),
                            code: format!("expr.{expr_type}"),
                            verdict: Verdict::Supports,
                            message: format!("expression '{expr_type}' supported"),
                        });
                    } else {
                        findings.push(FeatureFinding {
                            pointer: format!("{t_ptr}/type"),
                            code: format!("expr.unsupported.{expr_type}"),
                            verdict: Verdict::Refuses,
                            message: format!("expression type '{expr_type}' is not supported"),
                        });
                    }
                } else {
                    check_datasource_ref(
                        ds_type,
                        ds_uid,
                        &format!("{t_ptr}/datasource"),
                        findings,
                        referenced_ds,
                    );
                }

                if target.get("exemplar").and_then(Value::as_bool) == Some(true) {
                    findings.push(FeatureFinding {
                        pointer: format!("{t_ptr}/exemplar"),
                        code: "prometheus.exemplar".to_owned(),
                        verdict: Verdict::Maps,
                        message: "exemplar query flag warned and ignored".to_owned(),
                    });
                }
            }
        }

        // Transformations check (R4, §7.1)
        if let Some(txs) = &panel.transformations {
            for (tx_idx, tx) in txs.iter().enumerate() {
                let tx_ptr = format!("{ptr}/transformations/{tx_idx}");
                if SUPPORTED_TRANSFORMATIONS.contains(&tx.id.as_str()) {
                    findings.push(FeatureFinding {
                        pointer: tx_ptr,
                        code: format!("transformation.{}", tx.id),
                        verdict: Verdict::Supports,
                        message: format!("transformation '{}' supported", tx.id),
                    });
                } else {
                    findings.push(FeatureFinding {
                        pointer: tx_ptr,
                        code: format!("transformation.unsupported.{}", tx.id),
                        verdict: Verdict::Refuses,
                        message: format!(
                            "transformation '{}' is not executed and passes through on export",
                            tx.id
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn check_datasource_ref(
    ds_type: Option<&str>,
    ds_uid: Option<&str>,
    pointer: &str,
    findings: &mut Vec<FeatureFinding>,
    referenced_ds: &mut BTreeSet<String>,
) {
    if let Some(uid) = ds_uid
        && !uid.is_empty()
        && uid != "-- Mixed --"
        && uid != "__expr__"
        && uid != "-100"
    {
        referenced_ds.insert(uid.to_owned());
    }
    if let Some(t) = ds_type {
        if t == "datasource"
            || t == "-- Mixed --"
            || t.starts_with("wasm:")
            || SUPPORTED_DATASOURCE_TYPES.contains(&t)
        {
            findings.push(FeatureFinding {
                pointer: pointer.to_owned(),
                code: format!("datasource.{t}"),
                verdict: Verdict::Supports,
                message: format!("datasource type '{t}' supported"),
            });
        } else {
            findings.push(FeatureFinding {
                pointer: pointer.to_owned(),
                code: format!("datasource.unsupported.{t}"),
                verdict: Verdict::Refuses,
                message: format!("datasource type '{t}' has no built-in or WASM plugin"),
            });
        }
    }
}

/// Checks if a URL uses an executable or unsafe scheme (`javascript:`, `vbscript:`, `data:text/html`).
#[must_use]
pub fn is_dangerous_url(url: &str) -> bool {
    let normalized: String = url
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.starts_with("javascript:")
        || normalized.starts_with("vbscript:")
        || (normalized.starts_with("data:")
            && ![
                "data:image/png",
                "data:image/jpeg",
                "data:image/gif",
                "data:image/webp",
            ]
            .iter()
            .any(|raster| {
                normalized
                    .strip_prefix(raster)
                    .is_some_and(|rest| rest.starts_with(';') || rest.starts_with(','))
            }))
}

/// Returns the JSON pointer of the first `url` string under `value` that `is_dangerous_url` refuses.
fn find_dangerous_url(value: &Value, ptr: &str) -> Option<String> {
    match value {
        Value::Object(map) => map.iter().find_map(|(k, v)| {
            let child = format!("{ptr}/{k}");
            match v {
                Value::String(url) if k == "url" && is_dangerous_url(url) => Some(child),
                _ => find_dangerous_url(v, &child),
            }
        }),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, v)| find_dangerous_url(v, &format!("{ptr}/{i}"))),
        _ => None,
    }
}

/// Escapes plain text for safe inclusion in HTML attributes and elements (prevents XSS, §4.3).
#[must_use]
pub fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Renders a safe Markdown subset to HTML on the server, escaping all raw HTML tags, scripts,
/// event handlers, and unsafe URLs (CVE-2023-22462 mitigation, §4.3).
#[must_use]
pub fn sanitize_markdown_to_html(markdown: &str) -> String {
    let mut html = String::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if let Some(h) = trimmed.strip_prefix("### ") {
            html.push_str("<h3>");
            html.push_str(&html_escape(h));
            html.push_str("</h3>\n");
        } else if let Some(h) = trimmed.strip_prefix("## ") {
            html.push_str("<h2>");
            html.push_str(&html_escape(h));
            html.push_str("</h2>\n");
        } else if let Some(h) = trimmed.strip_prefix("# ") {
            html.push_str("<h1>");
            html.push_str(&html_escape(h));
            html.push_str("</h1>\n");
        } else if !trimmed.is_empty() {
            html.push_str("<p>");
            html.push_str(&html_escape(trimmed));
            html.push_str("</p>\n");
        }
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xss_in_markdown_and_links_is_neutralized() {
        let malicious = "# Welcome <script>alert(1)</script>\n<img src=x onerror=alert(1)>";
        let safe = sanitize_markdown_to_html(malicious);
        assert!(!safe.contains("<script>"));
        assert!(!safe.contains("<img"));
        assert!(safe.contains("&lt;script&gt;"));

        assert!(is_dangerous_url("javascript:alert(1)"));
        assert!(is_dangerous_url("  JaVaScRiPt:\nalert(1)"));
        assert!(is_dangerous_url("data:text/html,<script>alert(1)</script>"));
        assert!(!is_dangerous_url("https://joinedcontext.example/d/jc-slo"));
    }

    #[test]
    fn data_links_in_field_config_defaults_and_overrides_are_checked() {
        let fc = serde_json::json!({
            "defaults": {"links": [{"title": "ok", "url": "/d/abc?var-x=${__value.raw}"}]},
            "overrides": [{
                "matcher": {"id": "byName", "options": "temp"},
                "properties": [{"id": "links", "value": [{"title": "x", "url": " javascript:alert(1)"}]}]
            }]
        });
        assert_eq!(
            find_dangerous_url(&fc, "/panels/0/fieldConfig").as_deref(),
            Some("/panels/0/fieldConfig/overrides/0/properties/0/value/0/url")
        );
        let clean = serde_json::json!({"defaults": {"links": [{"url": "https://x.example/"}]}});
        assert_eq!(find_dangerous_url(&clean, ""), None);
    }

    #[test]
    fn dashboard_with_a_javascript_data_link_is_refused() {
        let raw = serde_json::json!({
            "title": "t", "schemaVersion": 42,
            "panels": [{
                "type": "table", "id": 1,
                "gridPos": {"h": 4, "w": 12, "x": 0, "y": 0},
                "fieldConfig": {"defaults": {"links": [{"title": "x", "url": "javascript:alert(document.cookie)"}]}, "overrides": []}
            }]
        });
        let doc: DashboardDocument = serde_json::from_value(raw.clone()).unwrap();
        let err = classify_and_validate(
            raw.to_string().len(),
            &doc,
            &BTreeSet::new(),
            &InputLimits::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("/panels/0/fieldConfig/defaults/links/0/url"),
            "{err}"
        );
    }

    #[test]
    fn svg_data_links_are_dangerous_raster_images_are_not() {
        // SVG documents carry script; only raster image data URLs stay allowed.
        assert!(is_dangerous_url(
            "data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+"
        ));
        assert!(is_dangerous_url("DATA:image/svg+xml,<svg onload=alert(1)>"));
        assert!(is_dangerous_url("data:image/pngx,<svg onload=alert(1)>"));
        assert!(!is_dangerous_url("data:image/png;base64,iVBORw0KGgo="));
        assert!(!is_dangerous_url("data:image/webp,AAAA"));
    }
}
