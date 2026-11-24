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
    let obj = require(conf, &["driver", "data_source_name", "query"])?;
    new_raw_output("sql", obj, "data_source_name", false, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{conf, stream_ctx};
    use ekiben_core::{Output, Part};
    use serde_json::json;

    #[tokio::test]
    async fn sql_reads_its_dsn_from_data_source_name_and_has_no_conn_fields() {
        let dir = tempfile::tempdir().unwrap();
        let dsn = dir.path().join("d.db").display().to_string();
        rusqlite::Connection::open(&dsn)
            .unwrap()
            .execute_batch("CREATE TABLE t (a int)")
            .unwrap();
        let ctx = stream_ctx(None);
        let out = build(
            &conf(
                "sql",
                json!({"driver": "sqlite", "data_source_name": dsn,
                "query": "INSERT INTO t VALUES (?)", "args_mapping": "root = [ 7 ]"}),
            ),
            &ctx,
        )
        .unwrap();
        out.connect(&ctx).await.unwrap();
        out.write_batch(&ctx, vec![Part::from_str("{}")])
            .await
            .unwrap();
        let a: i64 = rusqlite::Connection::open(&dsn)
            .unwrap()
            .query_row("SELECT a FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(a, 7);
        // init_statement and the other connection fields came after this spec.
        let errs = lint(
            &json!({"driver": "sqlite", "data_source_name": "x", "query": "q",
            "init_statement": "x"}),
        );
        assert_eq!(errs, ["field init_statement not recognised"]);
        let e = build(
            &conf("sql", json!({"driver": "sqlite", "dsn": "x", "query": "q"})),
            &ctx,
        )
        .err()
        .expect("refused");
        assert!(e.to_string().contains("dsn not recognised"), "{e}");
        let e = build(
            &conf("sql", json!({"driver": "sqlite", "query": "q"})),
            &ctx,
        )
        .err()
        .expect("refused");
        assert!(
            e.to_string().contains("field data_source_name is required"),
            "{e}"
        );
    }
}
