//! Port of `internal/impl/sql/input_sql_select.go` (Bento 1.22.0): selects `columns` from `table`
//! (squirrel's `SELECT ... FROM ... WHERE ...`) and emits a message per row.

use ekiben_core::{lint_fields, ComponentConfig, CoreError, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::conn_fields::CONN_FIELDS;
use crate::input_sql_raw::{refuse_lint, require, Conn, RowsInput};
use crate::util::Placeholder;

const FIELDS: &[&str] = &[
    "driver",
    "dsn",
    "table",
    "columns",
    "where",
    "args_mapping",
    "prefix",
    "suffix",
    "auto_replay_nacks",
];

pub fn register(registry: &Registry) {
    registry.register_input("sql_select", |conf, ctx| Ok(Box::new(build(conf, ctx)?)));
    registry.register_lint("input", "sql_select", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    let known: Vec<&str> = FIELDS.iter().chain(CONN_FIELDS).copied().collect();
    let mut errs = lint_fields(conf, &known, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(crate::conn_fields::lint_conn(obj));
    }
    errs
}

pub(crate) fn string_list(
    obj: &serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<Vec<String>, CoreError> {
    obj.get(key)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CoreError::config(format!("field {key}: expected a list of strings")))?
        .iter()
        .map(|v| {
            v.as_str().map(String::from).ok_or_else(|| {
                CoreError::config(format!("field {key}: expected a list of strings"))
            })
        })
        .collect()
}

/// squirrel `Select(columns...).From(table).Where(where).Prefix(prefix).Suffix(suffix)`.
pub fn select_sql(
    columns: &[String],
    table: &str,
    where_: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
    ph: Placeholder,
) -> String {
    let mut sql = String::new();
    if let Some(p) = prefix {
        sql.push_str(p);
        sql.push(' ');
    }
    sql.push_str("SELECT ");
    sql.push_str(&columns.join(", "));
    sql.push_str(" FROM ");
    sql.push_str(table);
    if !where_.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(where_);
    }
    if let Some(s) = suffix {
        sql.push(' ');
        sql.push_str(s);
    }
    ph.replace(&sql)
}

/// `newSQLSelectInputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<RowsInput, CoreError> {
    refuse_lint("input", conf, lint)?;
    require(conf, &["driver", "dsn", "table", "columns"])?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql_select: expected an object"))?;
    let conn = Conn::from_config(obj, "dsn", ctx)?;
    let s = |k: &str| obj.get(k).and_then(JsonValue::as_str);
    let query = select_sql(
        &string_list(obj, "columns")?,
        s("table").unwrap_or_default(),
        s("where").unwrap_or_default(),
        s("prefix"),
        s("suffix"),
        Placeholder::for_driver(conn.driver),
    );
    RowsInput::new("sql_select", conn, query, obj, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squirrel_select() {
        let cols = vec!["foo".to_string(), "bar".to_string()];
        assert_eq!(
            select_sql(
                &cols,
                "t",
                "a = ? and b = ?",
                Some("WITH x AS (SELECT 1)"),
                Some("ORDER BY bar"),
                Placeholder::Dollar
            ),
            "WITH x AS (SELECT 1) SELECT foo, bar FROM t WHERE a = $1 and b = $2 ORDER BY bar"
        );
        assert_eq!(
            select_sql(&["*".into()], "t", "", None, None, Placeholder::Question),
            "SELECT * FROM t"
        );
    }
}
