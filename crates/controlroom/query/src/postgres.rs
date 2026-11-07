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
            } else if j < n && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
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
            // A comment separates tokens like whitespace (`FROM/**/t` is `FROM t`).
            out.push(' ');
            i += 2;
            while i + 1 < n {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else if i + 1 < n && b == b'-' && bytes[i + 1] == b'-' {
            out.push(' ');
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
            if name == "__timeGroup"
                && let Some(idx) = sql.find(full_match)
            {
                let after = idx + full_match.len();
                if sql.as_bytes().get(after) == Some(&b',') {
                    name = "__timeGroupAlias".to_owned();
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
    fn check_col<'c>(name: &str, col: &'c str) -> Result<&'c str, DsError> {
        if !is_safe_sql_identifier(col) {
            return Err(DsError::InvalidQuery {
                ref_id: "sql".into(),
                reason: format!("unsafe column identifier '{col}' in macro {name}"),
            });
        }
        Ok(col)
    }

    match name {
        "__time" => {
            let col = check_col(
                name,
                args.first().ok_or_else(|| DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "missing time column argument for $__time".into(),
                })?,
            )?;
            Ok(format!("{col} AS \"time\""))
        }
        "__timeEpoch" => {
            let col = check_col(
                name,
                args.first().ok_or_else(|| DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "missing time column argument for $__timeEpoch".into(),
                })?,
            )?;
            Ok(format!("extract(epoch from {col}) as \"time\""))
        }
        "__timeFilter" => {
            let col = check_col(
                name,
                args.first().ok_or_else(|| DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "missing time column argument for $__timeFilter".into(),
                })?,
            )?;
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
            let col = check_col(name, args[0])?;
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
            let col = check_col(
                name,
                args.first().ok_or_else(|| DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: "missing time column argument for $__unixEpochFilter".into(),
                })?,
            )?;
            Ok(format!("{col} >= {from_sec} AND {col} <= {to_sec}"))
        }
        "__unixEpochGroup" | "__unixEpochGroupAlias" => {
            if args.len() < 2 {
                return Err(DsError::InvalidQuery {
                    ref_id: "sql".into(),
                    reason: format!("macro {name} needs time column and interval"),
                });
            }
            let col = check_col(name, args[0])?;
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

/// Functions a dashboard query may call (aggregates, window, math, time, text, conditional).
/// Everything else (`query_to_xml`, `lo_get`, `current_setting`, `pg_*`, `dblink_*`, set-returning
/// generators, any schema-qualified function) is refused: a function can read what the view
/// allow-list does not cover.
const ALLOWED_SQL_FUNCTIONS: &[&str] = &[
    "abs",
    "age",
    "array_agg",
    "avg",
    "bool_and",
    "bool_or",
    "btrim",
    "cast",
    "ceil",
    "ceiling",
    "char_length",
    "coalesce",
    "concat",
    "concat_ws",
    "count",
    "cume_dist",
    "date_bin",
    "date_part",
    "date_trunc",
    "dense_rank",
    "div",
    "every",
    "exp",
    "extract",
    "first_value",
    "floor",
    "greatest",
    "initcap",
    "lag",
    "last_value",
    "lead",
    "least",
    "left",
    "length",
    "ln",
    "log",
    "log10",
    "lower",
    "lpad",
    "ltrim",
    "make_interval",
    "max",
    "min",
    "mod",
    "mode",
    "now",
    "nth_value",
    "ntile",
    "nullif",
    "percent_rank",
    "percentile_cont",
    "percentile_disc",
    "position",
    "power",
    "rank",
    "replace",
    "right",
    "round",
    "row_number",
    "rpad",
    "rtrim",
    "sign",
    "split_part",
    "sqrt",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "string_agg",
    "strpos",
    "substr",
    "substring",
    "sum",
    "time_bucket",
    "timezone",
    "to_char",
    "to_date",
    "to_number",
    "to_timestamp",
    "trim",
    "trunc",
    "upper",
    "var_pop",
    "var_samp",
    "variance",
    "width_bucket",
];

/// Keywords that legitimately stand before `(` without being a function call.
const KEYWORDS_BEFORE_PAREN: &[&str] = &[
    "all",
    "and",
    "any",
    "as",
    "between",
    "by",
    "case",
    "distinct",
    "else",
    "exists",
    "filter",
    "from",
    "having",
    "in",
    "is",
    "join",
    "materialized",
    "not",
    "on",
    "or",
    "over",
    "partition",
    "select",
    "some",
    "then",
    "using",
    "values",
    "when",
    "where",
    "within",
    "with",
    "array",
    "row",
    "like",
    "ilike",
    "except",
    "intersect",
    "union",
    "lateral",
];

/// Type names that take a modifier in parentheses (`::numeric(10,2)`, `AS varchar(20)`).
const TYPE_NAMES_WITH_MODIFIER: &[&str] = &[
    "numeric",
    "decimal",
    "varchar",
    "char",
    "character",
    "timestamp",
    "timestamptz",
    "time",
    "timetz",
    "interval",
    "bit",
    "varbit",
    "float",
];

/// Functions whose argument list uses `FROM` as a separator, not as a relation clause.
const FROM_SEPARATOR_FUNCTIONS: &[&str] = &["extract", "substring", "trim", "overlay", "position"];

/// Keywords that end one FROM item.
const FROM_ITEM_END: &[&str] = &[
    "where",
    "group",
    "having",
    "window",
    "order",
    "limit",
    "offset",
    "fetch",
    "union",
    "intersect",
    "except",
    "on",
    "using",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
    "outer",
    "for",
    "returning",
    "tablesample",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum SqlTok {
    /// Unquoted identifier or keyword, lowercased.
    Word(String),
    /// Double-quoted identifier, exact.
    Quoted(String),
    Str,
    Num,
    Open,
    Close,
    Comma,
    Dot,
    Cast,
    Op,
}

fn sql_refuse(ref_id: &str, reason: impl Into<String>) -> DsError {
    DsError::InvalidQuery {
        ref_id: ref_id.to_owned(),
        reason: reason.into(),
    }
}

/// Tokenizes comment-free SQL. Refuses what Postgres would read differently from this scanner:
/// escape strings (`E'..'`), Unicode escapes (`U&`), dollar quoting, backslashes and `;`.
fn tokenize_sql(ref_id: &str, sql: &str) -> Result<Vec<SqlTok>, DsError> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '\'' {
            if let Some(SqlTok::Word(w)) = out.last()
                && (w == "e" || w == "u")
                && i > 0
                && !chars[i - 1].is_whitespace()
            {
                return Err(sql_refuse(
                    ref_id,
                    "escape string literals (E'...') are refused",
                ));
            }
            i += 1;
            let mut closed = false;
            while i < n {
                if chars[i] == '\'' {
                    if i + 1 < n && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                if chars[i] == '\\' {
                    return Err(sql_refuse(
                        ref_id,
                        "backslashes in string literals are refused",
                    ));
                }
                i += 1;
            }
            if !closed {
                return Err(sql_refuse(
                    ref_id,
                    "unterminated string literal in SQL query",
                ));
            }
            out.push(SqlTok::Str);
        } else if c == '"' {
            i += 1;
            let mut ident = String::new();
            let mut closed = false;
            while i < n {
                if chars[i] == '"' {
                    if i + 1 < n && chars[i + 1] == '"' {
                        ident.push('"');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                ident.push(chars[i]);
                i += 1;
            }
            if !closed || ident.is_empty() {
                return Err(sql_refuse(
                    ref_id,
                    "unterminated or empty quoted identifier",
                ));
            }
            out.push(SqlTok::Quoted(ident));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            if i < n && chars[i] == '$' {
                return Err(sql_refuse(ref_id, "'$' in identifiers is refused"));
            }
            if i < n && chars[i] == '&' {
                return Err(sql_refuse(
                    ref_id,
                    "Unicode escape literals (U&) are refused",
                ));
            }
            out.push(SqlTok::Word(
                chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            ));
        } else if c.is_ascii_digit() {
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                i += 1;
            }
            out.push(SqlTok::Num);
        } else {
            match c {
                '(' => out.push(SqlTok::Open),
                ')' => out.push(SqlTok::Close),
                ',' => out.push(SqlTok::Comma),
                '.' => out.push(SqlTok::Dot),
                ':' if i + 1 < n && chars[i + 1] == ':' => {
                    out.push(SqlTok::Cast);
                    i += 1;
                }
                ';' => {
                    return Err(sql_refuse(
                        ref_id,
                        "multiple SQL statements (';') are forbidden",
                    ));
                }
                '$' => {
                    return Err(sql_refuse(
                        ref_id,
                        "dollar quoting and parameters are refused",
                    ));
                }
                '+' | '-' | '*' | '/' | '%' | '<' | '>' | '=' | '!' | '|' | '&' | '^' | '~'
                | '#' | '@' | '?' | '[' | ']' | ':' => out.push(SqlTok::Op),
                other => {
                    return Err(sql_refuse(
                        ref_id,
                        format!("unexpected character '{other}'"),
                    ));
                }
            }
            i += 1;
        }
    }
    Ok(out)
}

fn word_at(toks: &[SqlTok], i: usize) -> Option<&str> {
    match toks.get(i) {
        Some(SqlTok::Word(w)) => Some(w.as_str()),
        _ => None,
    }
}

/// Index after the parenthesis group that opens at `open` (which must be `SqlTok::Open`).
fn skip_group(toks: &[SqlTok], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < toks.len() {
        match toks[i] {
            SqlTok::Open => depth += 1,
            SqlTok::Close => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Reads a possibly qualified relation name at `i`; returns (name, next index).
fn relation_name_at(toks: &[SqlTok], mut i: usize) -> Option<(String, usize)> {
    let mut parts = Vec::new();
    loop {
        match toks.get(i) {
            Some(SqlTok::Word(w)) => parts.push(w.clone()),
            Some(SqlTok::Quoted(q)) => parts.push(q.clone()),
            _ => return None,
        }
        i += 1;
        if toks.get(i) == Some(&SqlTok::Dot) {
            i += 1;
            continue;
        }
        return Some((parts.join("."), i));
    }
}

/// Parses and validates a SQL query against the organization's `allowed_views` before it runs (R3,
/// R12): one `SELECT` (or top-level `WITH ... SELECT`), no statement separator, no write or lock
/// clause, only allow-listed functions, and every relation in every `FROM` list (comma joins
/// included) and `JOIN` either a granted view or a top-level CTE. Returns the comment-free SQL
/// that is executed, so the text checked is the text run.
pub fn validate_readonly_select(
    ref_id: &str,
    sql: &str,
    allowed_views: &BTreeSet<String>,
) -> Result<String, DsError> {
    let stripped = strip_sql_comments(sql);
    let trimmed = stripped.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(sql_refuse(ref_id, "SQL query is empty"));
    }
    let toks = tokenize_sql(ref_id, trimmed)?;
    match word_at(&toks, 0) {
        Some("select" | "with") => {}
        Some(other) => {
            return Err(sql_refuse(
                ref_id,
                format!("only SELECT queries are permitted, found '{other}'"),
            ));
        }
        None => return Err(sql_refuse(ref_id, "only SELECT queries are permitted")),
    }

    for (idx, t) in toks.iter().enumerate() {
        let SqlTok::Word(w) = t else { continue };
        if FORBIDDEN_SQL_KEYWORDS.contains(&w.as_str()) || w == "into" {
            return Err(sql_refuse(
                ref_id,
                format!("forbidden SQL keyword or function '{w}'"),
            ));
        }
        if w == "for"
            && matches!(
                word_at(&toks, idx + 1),
                Some("update" | "share" | "no" | "key")
            )
        {
            return Err(sql_refuse(
                ref_id,
                "locking clauses (FOR UPDATE / FOR SHARE) are forbidden",
            ));
        }
    }

    // Top-level CTE names; a WITH anywhere else is refused (its scope would need a real parser).
    let allowed_lower: BTreeSet<String> = allowed_views
        .iter()
        .map(|v| v.to_ascii_lowercase())
        .collect();
    let mut cte_names: BTreeSet<String> = BTreeSet::new();
    if word_at(&toks, 0) == Some("with") {
        let mut i = 1usize;
        if word_at(&toks, i) == Some("recursive") {
            i += 1;
        }
        loop {
            let name = match toks.get(i) {
                Some(SqlTok::Word(w)) => w.clone(),
                Some(SqlTok::Quoted(q)) => q.clone(),
                _ => return Err(sql_refuse(ref_id, "malformed WITH clause")),
            };
            i += 1;
            if toks.get(i) == Some(&SqlTok::Open) {
                i = skip_group(&toks, i);
            }
            if word_at(&toks, i) != Some("as") {
                return Err(sql_refuse(ref_id, "malformed WITH clause"));
            }
            i += 1;
            if word_at(&toks, i) == Some("not") {
                i += 1;
            }
            if word_at(&toks, i) == Some("materialized") {
                i += 1;
            }
            if toks.get(i) != Some(&SqlTok::Open) {
                return Err(sql_refuse(ref_id, "malformed WITH clause"));
            }
            cte_names.insert(name);
            i = skip_group(&toks, i);
            if toks.get(i) == Some(&SqlTok::Comma) {
                i += 1;
                continue;
            }
            break;
        }
        if word_at(&toks, i) != Some("select") {
            return Err(sql_refuse(ref_id, "WITH must be followed by SELECT"));
        }
    }
    // Only the leading WITH handled above; `WITHIN GROUP` is a different word.
    let with_count = toks
        .iter()
        .filter(|t| matches!(t, SqlTok::Word(w) if w == "with"))
        .count();
    if with_count > usize::from(word_at(&toks, 0) == Some("with")) {
        return Err(sql_refuse(ref_id, "nested WITH clauses are refused"));
    }

    let relation_ok = |tok: &SqlTok, name: &str| -> bool {
        let quoted = matches!(tok, SqlTok::Quoted(_));
        if !name.contains('.') && cte_names.contains(name) {
            return true;
        }
        if quoted {
            // A quoted identifier is case-sensitive in Postgres: `"Metrics"` is not `metrics`.
            allowed_views.contains(name)
                || (name == name.to_ascii_lowercase() && allowed_lower.contains(name))
        } else {
            allowed_lower.contains(name)
        }
    };

    // Paren stack: true when the group is the argument list of a FROM-separator function.
    let mut stack: Vec<bool> = Vec::new();
    let mut idx = 0usize;
    while idx < toks.len() {
        match &toks[idx] {
            SqlTok::Open => {
                let prev_fn = match idx.checked_sub(1).and_then(|p| toks.get(p)) {
                    Some(SqlTok::Word(w)) => FROM_SEPARATOR_FUNCTIONS.contains(&w.as_str()),
                    _ => false,
                };
                stack.push(prev_fn);
            }
            SqlTok::Close => {
                stack.pop();
            }
            SqlTok::Word(w) => {
                let next_is_open = toks.get(idx + 1) == Some(&SqlTok::Open);
                if next_is_open && !KEYWORDS_BEFORE_PAREN.contains(&w.as_str()) {
                    let prev = idx.checked_sub(1).and_then(|p| toks.get(p));
                    let is_type = matches!(prev, Some(SqlTok::Cast))
                        || matches!(prev, Some(SqlTok::Word(p)) if p == "as")
                            && TYPE_NAMES_WITH_MODIFIER.contains(&w.as_str());
                    let qualified = matches!(prev, Some(SqlTok::Dot));
                    let after_cte_decl_start = matches!(prev, Some(SqlTok::Comma))
                        || matches!(prev, Some(SqlTok::Word(p)) if p == "with" || p == "recursive");
                    let cte_cols = word_at(&toks, 0) == Some("with")
                        && cte_names.contains(w.as_str())
                        && after_cte_decl_start
                        && matches!(
                            toks.get(skip_group(&toks, idx + 1)),
                            Some(SqlTok::Word(a)) if a == "as"
                        );
                    if qualified {
                        return Err(sql_refuse(
                            ref_id,
                            format!("schema-qualified function call '{w}' is refused"),
                        ));
                    }
                    if !is_type && !cte_cols && !ALLOWED_SQL_FUNCTIONS.contains(&w.as_str()) {
                        return Err(sql_refuse(
                            ref_id,
                            format!("function '{w}' is not allowed in dashboard queries"),
                        ));
                    }
                }
                let in_separator_fn = stack.last().copied().unwrap_or(false);
                let distinct_from = idx
                    .checked_sub(1)
                    .and_then(|p| word_at(&toks, p))
                    .is_some_and(|p| p == "distinct");
                let starts_from_list = w == "from" && !in_separator_fn && !distinct_from;
                if starts_from_list || w == "join" {
                    let mut j = idx + 1;
                    loop {
                        if word_at(&toks, j) == Some("lateral") || word_at(&toks, j) == Some("only")
                        {
                            j += 1;
                        }
                        match toks.get(j) {
                            Some(SqlTok::Open) => {
                                // Derived table: its own FROM clauses are checked when the scan reaches them.
                                j = skip_group(&toks, j);
                            }
                            Some(tok @ (SqlTok::Word(_) | SqlTok::Quoted(_))) => {
                                let Some((name, after)) = relation_name_at(&toks, j) else {
                                    return Err(sql_refuse(ref_id, "malformed FROM item"));
                                };
                                if toks.get(after) == Some(&SqlTok::Open) {
                                    return Err(sql_refuse(
                                        ref_id,
                                        format!("table function '{name}' in FROM is refused"),
                                    ));
                                }
                                if name.starts_with("pg_") || name.starts_with("information_schema")
                                {
                                    return Err(sql_refuse(
                                        ref_id,
                                        format!("system catalog '{name}' is not accessible"),
                                    ));
                                }
                                if !relation_ok(tok, &name) {
                                    return Err(sql_refuse(
                                        ref_id,
                                        format!(
                                            "relation '{name}' is not in the organization's granted views allow-list"
                                        ),
                                    ));
                                }
                                j = after;
                            }
                            _ => return Err(sql_refuse(ref_id, "malformed FROM item")),
                        }
                        // Optional alias: [AS] name [(columns)].
                        if word_at(&toks, j) == Some("as") {
                            j += 1;
                        }
                        let alias_word = match toks.get(j) {
                            Some(SqlTok::Word(a)) => !FROM_ITEM_END.contains(&a.as_str()),
                            Some(SqlTok::Quoted(_)) => true,
                            _ => false,
                        };
                        if alias_word {
                            j += 1;
                            if toks.get(j) == Some(&SqlTok::Open) {
                                j = skip_group(&toks, j);
                            }
                        }
                        if starts_from_list && toks.get(j) == Some(&SqlTok::Comma) {
                            j += 1;
                            continue;
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
        idx += 1;
    }

    Ok(trimmed.to_owned())
}

/// Most rows one Postgres target returns; the rest of the result is not read.
pub const MAX_SQL_ROWS: usize = 50_000;

fn pg_err(e: &sqlx::Error) -> DsError {
    DsError::Upstream {
        status: 400,
        message: format!("postgres query failed: {e}"),
    }
}

/// Runs the checked SELECT inside a READ ONLY transaction with a statement timeout and
/// standard-conforming strings (the scanner's view of literals), reads at most
/// [`MAX_SQL_ROWS`] rows and rolls back. The database refuses a write even if the text check
/// ever missed one.
async fn run_read_only(
    pool: &PgPool,
    verified_sql: &str,
    timeout: std::time::Duration,
) -> Result<Vec<sqlx::postgres::PgRow>, DsError> {
    use futures_util::TryStreamExt;
    let mut tx = pool.begin().await.map_err(|e| pg_err(&e))?;
    sqlx::query("SET TRANSACTION READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|e| pg_err(&e))?;
    let timeout_ms = timeout.as_millis().clamp(1, 60_000).to_string();
    sqlx::query("SELECT set_config('statement_timeout', $1, true), set_config('standard_conforming_strings', 'on', true)")
        .bind(timeout_ms)
        .execute(&mut *tx)
        .await
        .map_err(|e| pg_err(&e))?;
    let mut rows = Vec::new();
    {
        let mut stream = sqlx::query(sqlx::AssertSqlSafe(verified_sql.to_owned())).fetch(&mut *tx);
        while let Some(row) = stream.try_next().await.map_err(|e| pg_err(&e))? {
            rows.push(row);
            if rows.len() >= MAX_SQL_ROWS {
                break;
            }
        }
    }
    tx.rollback().await.map_err(|e| pg_err(&e))?;
    Ok(rows)
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

                let rows = run_read_only(pool, &verified_sql, ctx.timeout).await?;

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
        let obj = grafana_target
            .as_object()
            .ok_or_else(|| DsError::InvalidQuery {
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
        let expanded = interpolate_postgres_macros(
            sql_with_macros,
            1_700_000_000_000,
            1_700_003_600_000,
            60_000,
            false,
        )
        .unwrap();
        assert!(expanded.contains("AS \"time\""));
        assert!(expanded.contains("BETWEEN '2023-11-14T22:13:20"));
        assert!(validate_readonly_select("A", &expanded, &allowed).is_ok());

        // Unauthorized table/view refused
        assert!(validate_readonly_select("A", "SELECT * FROM other_org_table", &allowed).is_err());
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

    fn views() -> BTreeSet<String> {
        ["org_metrics_view".to_owned(), "reports.daily".to_owned()]
            .into_iter()
            .collect()
    }

    #[test]
    fn select_guard_refuses_relations_hidden_from_a_token_scan() {
        let v = views();
        for sql in [
            // Comma join: the second relation was never checked.
            "SELECT * FROM org_metrics_view, secret_table",
            "SELECT * FROM org_metrics_view a, secret_table b",
            "SELECT * FROM (SELECT 1) s, secret_table",
            // `x AS ...` once made `secret_table` look like a CTE name.
            "SELECT secret_table AS x FROM secret_table",
            // SQL in a string literal run by a function.
            "SELECT query_to_xml('select * from secret_table', true, true, '')",
            "SELECT * FROM org_metrics_view WHERE current_setting('cr.org') <> ''",
            "SELECT lo_get(1234)",
            "SELECT pg_catalog.lo_get(1234)",
            "SELECT version()",
            // Table functions and nested CTE scopes.
            "SELECT * FROM generate_series(1, 1000000000)",
            "SELECT * FROM org_metrics_view CROSS JOIN LATERAL dblink_get_result('c') x",
            "SELECT * FROM (WITH secret_table AS (SELECT 1) SELECT 1) x, secret_table",
            // Other schemas and catalogs.
            "SELECT * FROM public.org_metrics_view",
            "SELECT * FROM pg_catalog.pg_authid",
            "SELECT * FROM \"Org_Metrics_View\"",
            // Literal tricks the scanner would read differently from Postgres.
            "SELECT E'\\'' , 1 FROM org_metrics_view",
            "SELECT U&'\\0041' FROM org_metrics_view",
            "SELECT $$x$$ FROM org_metrics_view",
            "SELECT 'a\\' FROM org_metrics_view",
            // Writes, locks, several statements, comment tricks.
            "SELECT * INTO new_table FROM org_metrics_view",
            "SELECT * FROM org_metrics_view FOR UPDATE",
            "SELECT * FROM org_metrics_view FOR NO KEY UPDATE",
            "SELECT 1; DELETE FROM org_metrics_view",
            "SELECT 1 /* ; */ ; DROP TABLE x",
            "SELECT 1 FROM org_metrics_view -- \n; DROP TABLE x",
            "SELECT 1 FROM/**/secret_table",
            "WITH d AS (DELETE FROM org_metrics_view RETURNING *) SELECT * FROM d",
            "TABLE secret_table",
            "VALUES (1)",
        ] {
            assert!(
                validate_readonly_select("A", sql, &v).is_err(),
                "guard let through: {sql}"
            );
        }
    }

    #[test]
    fn select_guard_keeps_ordinary_dashboard_sql() {
        let v = views();
        for sql in [
            "SELECT time_bucket('300.000s',created_at) AS \"time\", avg(value) FROM org_metrics_view GROUP BY 1 ORDER BY 1",
            "SELECT floor(extract(epoch from created_at)/300)*300 AS \"time\", count(*) FROM org_metrics_view WHERE created_at BETWEEN '2023-01-01' AND '2023-01-02' GROUP BY 1",
            "SELECT a.x, b.y FROM org_metrics_view a JOIN reports.daily b ON a.id = b.id",
            "SELECT * FROM org_metrics_view AS m, reports.daily d WHERE m.id = d.id",
            "WITH last AS (SELECT max(created_at) AS t FROM org_metrics_view) SELECT * FROM org_metrics_view, last WHERE created_at = last.t",
            "WITH c(a, b) AS (SELECT 1, 2) SELECT a FROM c",
            "SELECT substring(name from 1 for 3), trim(both from name) FROM org_metrics_view",
            "SELECT value::numeric(10,2), CAST(value AS varchar(20)) FROM org_metrics_view",
            "SELECT avg(value) OVER (PARTITION BY host ORDER BY created_at) FROM org_metrics_view",
            "SELECT count(*) FILTER (WHERE value > 1) FROM org_metrics_view WHERE a IS DISTINCT FROM b",
            "SELECT * FROM org_metrics_view WHERE id IN (SELECT id FROM reports.daily)",
            "SELECT 'it''s' AS label FROM \"org_metrics_view\"",
            "SELECT * FROM org_metrics_view;",
        ] {
            assert!(
                validate_readonly_select("A", sql, &v).is_ok(),
                "guard refused: {sql}: {:?}",
                validate_readonly_select("A", sql, &v)
            );
        }
    }
}
