//! Port of `internal/impl/sql/output_sql_deprecated.go` (Bento 1.22.0): the deprecated `sql`
//! output, `sql_raw` with the DSN under `data_source_name` and no dynamic query.

use ekiben_core::{lint_fields, ComponentConfig, CoreError, Registry, StreamContext};
use serde_json::Value as JsonValue;

use crate::conn_fields;
use crate::input_sql_raw::{refuse_lint, require};
use crate::output_sql_raw::{lint_max_in_flight, new_raw_output, SqlRawOutput};

const FIELDS: &[&str] = &[
    "driver",
    "data_source_name",
    "query",
    "args_mapping",
    "max_in_flight",
    "batching",
];

pub fn register(registry: &Registry) {
    registry.register_output("sql", |conf, ctx| {
        Ok(std::sync::Arc::new(build(conf, ctx)?))
    });
    registry.register_lint("output", "sql", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    // The deprecated spec has none of the connection fields.
    let mut errs = lint_fields(conf, FIELDS, &[]);
    if let Some(obj) = conf.as_object() {
        errs.extend(conn_fields::lint_conn(obj));
        errs.extend(lint_max_in_flight(obj));
    }
    errs
}

/// `newSQLDeprecatedOutputFromConfig`.
pub fn build(conf: &ComponentConfig, ctx: &StreamContext) -> Result<SqlRawOutput, CoreError> {
    refuse_lint("output", conf, lint)?;
    require(conf, &["driver", "data_source_name", "query"])?;
    let obj = conf
        .config
        .as_object()
        .ok_or_else(|| CoreError::config("sql: expected an object"))?;
    new_raw_output("sql", obj, "data_source_name", false, ctx)
}
