#![forbid(unsafe_code)]
//! Built-in Postgres read-only datasource (R3, §3.2, `GF pkg/tsdb/grafana-postgresql-datasource/macros.go`):
//! - Strips SQL comments (`--` and `/* */`) while preserving single-quoted, double-quoted, and
//!   dollar-quoted (`$$...$$`, `$tag$...$tag$`) regions verbatim (`macros.go:63-119`).
//! - Expands Grafana SQL macros: `$__timeFilter(col)`, `$__timeGroup(col, interval)`,
//!   `$__timeGroupAlias(col, interval)`, `$__time(col)`, `$__timeEpoch(col)`, `$__timeFrom()`,
//!   `$__timeTo()`, `$__unixEpochFilter(col)`, `$__unixEpochGroup(col, interval)`,
//!   `$__interval`, `$__interval_ms`.
//! - Parses and checks the query BEFORE execution:
//!   1. Must be a single `SELECT` statement (no `;` statement separators, no `INTO`, no `FOR UPDATE`).
//!   2. Rejects any DDL/DML/administrative or file/network function token (`INSERT`, `UPDATE`,
//!      `DELETE`, `DROP`, `ALTER`, `CREATE`, `TRUNCATE`, `COPY`, `GRANT`, `REVOKE`, `SET`,
//!      `RESET`, `SHOW`, `CALL`, `DO`, `EXECUTE`, `PREPARE`, `PG_READ_FILE`, `PG_LS_DIR`,
//!      `PG_STAT_FILE`, `LO_IMPORT`, `LO_EXPORT`, `DBLINK`, `PG_SLEEP`, `PG_TERMINATE_BACKEND`).
//!   3. Extracts every relation referenced after `FROM` or `JOIN` (excluding CTE names declared in
//!      `WITH`) and verifies each is in the organization's `allowed_views` set!

use crate::prometheus::format_prom_interval;
use crate::{
    Datasource, DsError, DsFuture, EditorFieldHint, EditorSchema, Health, ImportOutcome, Query,
    QueryCtx, ResourceReq, ResourceResp,
};
use chrono::{DateTime, SecondsFormat, Utc};
use controlroom_expr::parse_duration_ms;
use controlroom_model::{FieldColumn, FieldType, Frame, FrameMeta, FrameSet};
use regex::Regex;
use serde_json::{Value, json};
use sqlx::{Column, PgPool, Row, TypeInfo};
use std::collections::{BTreeMap, BTreeSet};

const FORBIDDEN_SQL_KEYWORDS: &[&str] = &[
    "insert",
    "update",
    "delete",
    "drop",
    "alter",
    "create",
    "truncate",
    "copy",
    "grant",
    "revoke",
    "set",
    "reset",
    "show",
    "call",
    "do",
    "execute",
    "prepare",
    "deallocate",
    "vacuum",
    "analyze",
    "listen",
    "notify",
    "unlisten",
    "lock",
    "comment",
    "security",
    "reassign",
    "discard",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_ls_dir",
    "pg_stat_file",
    "lo_import",
    "lo_export",
    "dblink",
    "pg_sleep",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "set_config",
];

/// Quote-aware SQL comment stripper ported from `GF pkg/tsdb/grafana-postgresql-datasource/macros.go:63-119`.
#[must_use]
pub fn strip_sql_comments(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n);
    let mut i = 0usize;

    while i < n {
        let b = bytes[i];
        if b == b'$' {
            // Check for $$ or $tag$
            let j = i + 1;
            if j < n && bytes[j] == b'$' {
                i = consume_dollar_quoted(sql, i, "$$", &mut out);
                continue;
            } else if j < n
                && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_')
            {
                let mut k = j + 1;
                while k < n && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
                    k += 1;
                }
                if k < n && bytes[k] == b'$' {
                    let tag = &sql[i..=k];
                    i = consume_dollar_quoted(sql, i, tag, &mut out);
                    continue;
                }
            }
            out.push('$');
            i += 1;
        } else if b == b'\'' || b == b'"' {
            let q = b as char;
            out.push(q);
            i += 1;
            while i < n {
                let ch = sql[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
                if ch == q {
                    if i < n && sql[i..].starts_with(q) {
                        out.push(q);
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
        } else if i + 1 < n && b == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else if i + 1 < n && b == b'-' && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            let ch = sql[i..].chars().next().unwrap_or('\0');
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    out
}

fn consume_dollar_quoted(sql: &str, mut i: usize, closing: &str, out: &mut String) -> usize {
    out.push_str(closing);
    i += closing.len();
    let n = sql.len();
    while i < n {
        if sql[i..].starts_with(closing) {
            out.push_str(closing);
            i += closing.len();
            break;
        }
        let ch = sql[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    i
}

fn is_safe_sql_identifier(ident: &str) -> bool {
    let trimmed = ident.trim().trim_matches('"');
    !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Interpolates Grafana PostgreSQL macros (`$__timeFilter`, `$__timeGroup`, `$__interval`, etc., `macros.go:130-264`).
pub fn interpolate_postgres_macros(
    raw_sql: &str,
    from_ms: i64,
    to_ms: i64,
    interval_ms: u64,
    timescaledb: bool,
) -> Result<String, DsError> {
    let stripped = strip_sql_comments(raw_sql);
    let interval_str = format_prom_interval(interval_ms);
    let mut sql = stripped
        .replace("$__interval_ms", &interval_ms.to_string())
        .replace("${__interval_ms}", &interval_ms.to_string())
        .replace("$__interval", &format!("'{interval_str}'"))
        .replace("${__interval}", &format!("'{interval_str}'"));

    let re = Regex::new(r"\$(__[_a-zA-Z0-9]+)\(([^)]*)\)").map_err(|e| DsError::InvalidQuery {
        ref_id: "sql".into(),
        reason: e.to_string(),
    })?;

    let from_iso = DateTime::<Utc>::from_timestamp_millis(from_ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Nanos, true);
    let to_iso = DateTime::<Utc>::from_timestamp_millis(to_ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Nanos, true);
    let from_sec = from_ms / 1000;
    let to_sec = to_ms / 1000;

    let mut macro_err: Option<DsError> = None;
    sql = re
        .replace_all(&sql, |caps: &regex::Captures<'_>| {
            let full_match = &caps[0];
            let mut name = caps[1].to_owned();
            if name == "__timeGroup" {
                if let Some(idx) = sql.find(full_match) {
                    let after = idx + full_match.len();
                    if sql.as_bytes().get(after) == Some(&b',') {
                        name = "__timeGroupAlias".to_owned();
                    }
                }
            }
            let args: Vec<&str> = caps[2]
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();

            match eval_pg_macro(
                &name,
                &args,
                &from_iso,
                &to_iso,
                from_sec,
                to_sec,
                timescaledb,
            ) {
                Ok(rep) => rep,
                Err(e) => {
                    if macro_err.is_none() {
                        macro_err = Some(e);
                    }
                    "macro_error()".to_owned()
                }
            }
        })
        .into_owned();

    if let Some(err) = macro_err {
        return Err(err);
    }
    Ok(sql)
}

fn eval_pg_macro(
    name: &str,
    args: &[&str],
    from_iso: &str,
    to_iso: &str,
    from_sec: i64,
    to_sec: i64,
    timescaledb: bool,
) -> Result<String, DsError> {
    let check_col = |col: &str| -> Result<&str, DsError> {
        if !is_safe_sql_identifier(col) {
            return Err(DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: format!("unsafe column identifier '{col}' in macro {name}"),
            });
        }
        Ok(col)
    };

    match name {
        "__time" => {
            let col = check_col(*args.first().ok_or_else(|| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: "missing time column argument for $__time".into(),
            })?)?;
            Ok(format!("{col} AS \"time\""))
        }
        "__timeEpoch" => {
            let col = check_col(*args.first().ok_or_else(|| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: "missing time column argument for $__timeEpoch".into(),
            })?)?;
            Ok(format!("extract(epoch from {col}) as \"time\""))
        }
        "__timeFilter" => {
            let col = check_col(*args.first().ok_or_else(|| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: "missing time column argument for $__timeFilter".into(),
            })?)?;
            Ok(format!("{col} BETWEEN '{from_iso}' AND '{to_iso}'"))
        }
        "__timeFrom" => Ok(format!("'{from_iso}'")),
        "__timeTo" => Ok(format!("'{to_iso}'")),
        "__timeGroup" | "__timeGroupAlias" => {
            if args.len() < 2 {
                return Err(DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: format!("macro {name} needs time column and interval"),
                });
            }
            let col = check_col(args[0])?;
            let iv_raw = args[1].trim_matches(['\'', '"']);
            let iv_ms = parse_duration_ms(iv_raw).map_err(|_| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: format!("error parsing interval '{}'", args[1]),
            })?;
            if iv_ms <= 0 {
                return Err(DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "interval must be positive".into(),
                });
            }
            let sec = (iv_ms as f64) / 1000.0;
            let base = if timescaledb {
                format!("time_bucket('{sec:.3}s',{col})")
            } else {
                format!("floor(extract(epoch from {col})/{sec})*{sec}")
            };
            if name == "__timeGroupAlias" {
                Ok(format!("{base} AS \"time\""))
            } else {
                Ok(base)
            }
        }
        "__unixEpochFilter" => {
            let col = check_col(*args.first().ok_or_else(|| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: "missing time column argument for $__unixEpochFilter".into(),
            })?)?;
            Ok(format!("{col} >= {from_sec} AND {col} <= {to_sec}"))
        }
        "__unixEpochGroup" | "__unixEpochGroupAlias" => {
            if args.len() < 2 {
                return Err(DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: format!("macro {name} needs time column and interval"),
                });
            }
            let col = check_col(args[0])?;
            let iv_raw = args[1].trim_matches(['\'', '"']);
            let iv_ms = parse_duration_ms(iv_raw).map_err(|_| DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: format!("error parsing interval '{}'", args[1]),
            })?;
            let sec = (iv_ms as f64) / 1000.0;
            let base = format!("floor(({col})/{sec})*{sec}");
            if name == "__unixEpochGroupAlias" {
                Ok(format!("{base} AS \"time\""))
            } else {
                Ok(base)
            }
        }
        other => Err(DsError::InvalidQuery {
            ref_id: "sql".into(),
            reason: format!("unknown macro '{other}'"),
        }),
    }
}

/// Parses and validates a SQL query against the organization's `allowed_views` before it runs (R3, R12).
pub fn validate_readonly_select(
    ref_id: &str,
    sql: &str,
    allowed_views: &BTreeSet<String>,
) -> Result<String, DsError> {
    let stripped = strip_sql_comments(sql);
    let trimmed = stripped.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(DsError::InvalidQuery {
            ref_id: ref_id.to_owned(),
            reason: "SQL query is empty".into(),
        });
    }

    // Mask single-quoted string literals before scanning tokens so ';' or words inside strings don't false-trigger,
    // while keeping outside tokens intact.
    let masked = mask_single_quoted_strings(trimmed)?;
    if masked.contains(';') {
        return Err(DsError::InvalidQuery {
            ref_id: ref_id.to_owned(),
            reason: "multiple SQL statements (';') are forbidden".into(),
        });
    }

    let tokens: Vec<String> = Regex::new(r#"[a-zA-Z_][a-zA-Z0-9_$.]*|"[^"]+""#)
        .map_err(|e| DsError::InvalidQuery {
            ref_id: ref_id.to_owned(),
            reason: e.to_string(),
        })?
        .find_iter(&masked)
        .map(|m| m.as_str().trim_matches('"').to_ascii_lowercase())
        .collect();

    let Some(first) = tokens.first() else {
        return Err(DsError::InvalidQuery {
            ref_id: ref_id.to_owned(),
            reason: "no SQL tokens found".into(),
        });
    };

    if first != "select" && first != "with" {
        return Err(DsError::InvalidQuery {
            ref_id: ref_id.to_owned(),
            reason: format!("only SELECT queries are permitted, found '{first}'"),
        });
    }

    for tok in &tokens {
        let base_tok = tok.rsplit_once('.').map_or(tok.as_str(), |(_, b)| b);
        if FORBIDDEN_SQL_KEYWORDS.contains(&base_tok) {
            return Err(DsError::InvalidQuery {
                ref_id: ref_id.to_owned(),
                reason: format!("forbidden SQL keyword or function '{tok}'"),
            });
        }
    }

    // Disallow SELECT ... INTO and FOR UPDATE / FOR SHARE
    for window in tokens.windows(2) {
        if window == ["for", "update"] || window == ["for", "share"] {
            return Err(DsError::InvalidQuery {
                ref_id: ref_id.to_owned(),
                reason: "locking clauses (FOR UPDATE / FOR SHARE) are forbidden".into(),
            });
        }
    }

    // Collect CTE names declared after WITH so they aren't mistaken for unauthorized relations
    let mut cte_names = BTreeSet::new();
    for window in tokens.windows(2) {
        if window[1] == "as" && window[0] != "select" {
            cte_names.insert(window[0].clone());
        }
    }

    // Verify every table/view after FROM or JOIN is in allowed_views
    let allowed_lower: BTreeSet<String> = allowed_views
        .iter()
        .map(|v| v.to_ascii_lowercase())
        .collect();

    let mut referenced_views = Vec::new();
    let mut idx = 0usize;
    while idx + 1 < tokens.len() {
        let kw = &tokens[idx];
        let next = &tokens[idx + 1];
        if kw == "from" || kw == "join" {
            // Note: `extract(epoch from col)` has `from` inside `extract(...)`.
            let is_extract_from = idx >= 2 && tokens[idx - 2] == "extract";
            if !is_extract_from && next != "select" && !cte_names.contains(next) {
                referenced_views.push(next.clone());
            }
        }
        idx += 1;
    }

    for view in &referenced_views {
        if view.starts_with("pg_") || view.starts_with("information_schema") {
            return Err(DsError::InvalidQuery {
                ref_id: ref_id.to_owned(),
                reason: format!("system catalog '{view}' is not accessible"),
            });
        }
        if !allowed_lower.contains(view) {
            return Err(DsError::InvalidQuery {
                ref_id: ref_id.to_owned(),
                reason: format!(
                    "relation '{view}' is not in the organization's granted views allow-list"
                ),
            });
        }
    }

    Ok(trimmed.to_owned())
}

fn mask_single_quoted_strings(sql: &str) -> Result<String, DsError> {
    let mut out = String::with_capacity(sql.len());
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut i = 0usize;
    while i < n {
        if chars[i] == '\'' {
            out.push_str("''");
            i += 1;
            let mut closed = false;
            while i < n {
                if chars[i] == '\'' {
                    i += 1;
                    if i < n && chars[i] == '\'' {
                        i += 1;
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if !closed {
                return Err(DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "unterminated string literal in SQL query".into(),
                });
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Built-in Postgres datasource executing verified read-only `SELECT` queries over granted views (R3, §3.2).
pub struct PostgresDatasource {
    pub uid: String,
    pub allowed_views: BTreeSet<String>,
    pub timescaledb: bool,
    pub pool: Option<PgPool>,
}

impl PostgresDatasource {
    #[must_use]
    pub fn new(
        uid: impl Into<String>,
        allowed_views: BTreeSet<String>,
        timescaledb: bool,
        pool: Option<PgPool>,
    ) -> Self {
        Self {
            uid: uid.into(),
            allowed_views,
            timescaledb,
            pool,
        }
    }
}

impl Datasource for PostgresDatasource {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    fn query<'a>(
        &'a self,
        ctx: &'a QueryCtx,
        queries: &'a [Query],
    ) -> DsFuture<'a, Result<Vec<FrameSet>, DsError>> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(queries.len());
            for q in queries {
                if q.hide {
                    continue;
                }
                let raw_sql = q
                    .model
                    .get("rawSql")
                    .and_then(Value::as_str)
                    .ok_or_else(|| DsError::InvalidQuery {
                        ref_id: q.ref_id.clone(),
                        reason: "missing 'rawSql' field".into(),
                    })?;

                let interpolated = interpolate_postgres_macros(
                    raw_sql,
                    ctx.from_ms,
                    ctx.to_ms,
                    ctx.interval_ms,
                    self.timescaledb,
                )?;
                let verified_sql =
                    validate_readonly_select(&q.ref_id, &interpolated, &self.allowed_views)?;

                let Some(pool) = &self.pool else {
                    // Return validated empty frame with executed_query_string when no live pool is attached
                    let frame = Frame {
                        name: q.ref_id.clone(),
                        ref_id: Some(q.ref_id.clone()),
                        fields: Vec::new(),
                        meta: FrameMeta {
                            executed_query_string: Some(verified_sql),
                            ..FrameMeta::default()
                        },
                    };
                    out.push(FrameSet::new(q.ref_id.clone(), vec![frame]));
                    continue;
                };

                let rows = sqlx::query(&verified_sql)
                    .fetch_all(pool)
                    .await
                    .map_err(|e| DsError::Upstream {
                        status: 400,
                        message: format!("postgres query failed: {e}"),
                    })?;

                let mut fields: Vec<FieldColumn> = Vec::new();
                if let Some(first_row) = rows.first() {
                    for col in first_row.columns() {
                        let type_name = col.type_info().name().to_ascii_uppercase();
                        let ft = match type_name.as_str() {
                            "TIMESTAMPTZ" | "TIMESTAMP" => FieldType::Time,
                            "INT2" | "INT4" | "INT8" | "FLOAT4" | "FLOAT8" | "NUMERIC" => {
                                FieldType::Number
                            }
                            "BOOL" => FieldType::Bool,
                            _ => FieldType::String,
                        };
                        fields.push(FieldColumn {
                            name: col.name().to_owned(),
                            field_type: ft,
                            unit: None,
                            display_name: None,
                            labels: BTreeMap::new(),
                            values: Vec::with_capacity(rows.len()),
                        });
                    }
                }

                for row in &rows {
                    for (c_idx, f) in fields.iter_mut().enumerate() {
                        let val = match f.field_type {
                            FieldType::Time => row
                                .try_get::<DateTime<Utc>, _>(c_idx)
                                .ok()
                                .map_or(Value::Null, |dt| Value::from(dt.timestamp_millis())),
                            FieldType::Number => row
                                .try_get::<f64, _>(c_idx)
                                .or_else(|_| row.try_get::<i64, _>(c_idx).map(|i| i as f64))
                                .ok()
                                .and_then(serde_json::Number::from_f64)
                                .map_or(Value::Null, Value::Number),
                            FieldType::Bool => row
                                .try_get::<bool, _>(c_idx)
                                .ok()
                                .map_or(Value::Null, Value::Bool),
                            _ => row
                                .try_get::<String, _>(c_idx)
                                .ok()
                                .map_or(Value::Null, Value::String),
                        };
                        f.values.push(val);
                    }
                }

                out.push(FrameSet::new(
                    q.ref_id.clone(),
                    vec![Frame {
                        name: q.ref_id.clone(),
                        ref_id: Some(q.ref_id.clone()),
                        fields,
                        meta: FrameMeta {
                            executed_query_string: Some(verified_sql),
                            ..FrameMeta::default()
                        },
                    }],
                ));
            }
            Ok(out)
        })
    }

    fn health<'a>(&'a self, _ctx: &'a QueryCtx) -> DsFuture<'a, Health> {
        Box::pin(async move {
            Health::ok(format!(
                "Postgres datasource ready ({} granted view(s))",
                self.allowed_views.len()
            ))
        })
    }

    fn resource<'a>(
        &'a self,
        _ctx: &'a QueryCtx,
        _r: ResourceReq,
    ) -> DsFuture<'a, Result<ResourceResp, DsError>> {
        Box::pin(async move {
            let views: Vec<&String> = self.allowed_views.iter().collect();
            Ok(ResourceResp {
                data: json!({ "allowedViews": views }),
            })
        })
    }

    fn editor(&self) -> EditorSchema {
        EditorSchema {
            kind: "postgres".into(),
            json_schema: json!({
                "$schema": "https://json-schema.org/draft-04/schema",
                "type": "object",
                "required": ["rawSql"],
                "properties": {
                    "rawSql": { "type": "string" },
                    "format": { "type": "string", "enum": ["time_series", "table"] },
                    "editorMode": { "type": "string", "enum": ["code", "builder"] }
                }
            }),
            field_hints: vec![EditorFieldHint {
                name: "rawSql".into(),
                label: "SQL Query (read-only views)".into(),
                widget: "sql-code".into(),
                resource_path: Some("views".into()),
                required: true,
            }],
        }
    }

    fn import(&self, grafana_target: &Value) -> Result<ImportOutcome, DsError> {
        let obj = grafana_target.as_object().ok_or_else(|| DsError::InvalidQuery {
            ref_id: "A".into(),
            reason: "Postgres target must be a JSON object".into(),
        })?;
        let ref_id = obj
            .get("refId")
            .and_then(Value::as_str)
            .unwrap_or("A")
            .to_owned();
        let hide = obj.get("hide").and_then(Value::as_bool).unwrap_or(false);
        Ok(ImportOutcome {
            query: Query {
                ref_id,
                hide,
                model: Value::Object(obj.clone()),
            },
            warnings: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_macros_and_comment_stripper_work_and_guard_rejects_unauthorized_sql() {
        let allowed: BTreeSet<String> = ["org_metrics_view".to_owned()].into_iter().collect();

        let sql_with_macros = r#"
            -- comment with $__timeFilter(ignored)
            SELECT
                $__timeGroup(created_at, '5m'),
                avg(value)
            FROM org_metrics_view
            WHERE $__timeFilter(created_at)
            GROUP BY 1
        "#;
        let expanded =
            interpolate_postgres_macros(sql_with_macros, 1_700_000_000_000, 1_700_003_600_000, 60_000, false)
                .unwrap();
        assert!(expanded.contains("AS \"time\""));
        assert!(expanded.contains("BETWEEN '2023-11-14T22:13:20"));
        assert!(validate_readonly_select("A", &expanded, &allowed).is_ok());

        // Unauthorized table/view refused
        assert!(
            validate_readonly_select("A", "SELECT * FROM other_org_table", &allowed).is_err()
        );
        // Multiple statements refused
        assert!(
            validate_readonly_select(
                "A",
                "SELECT * FROM org_metrics_view; DROP TABLE users",
                &allowed
            )
            .is_err()
        );
        // File read / sleep functions refused
        assert!(
            validate_readonly_select("A", "SELECT pg_read_file('/etc/passwd')", &allowed).is_err()
        );
    }
}
