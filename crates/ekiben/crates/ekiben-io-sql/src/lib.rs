#![forbid(unsafe_code)]
//! `ekiben-io-sql`: the sql inputs, outputs and processors of Bento 1.22.0
//! (`internal/impl/sql/*.go`), one module per Go file, over sqlx for the postgres, mysql and
//! sqlite drivers; and the `sql` cache (an ekiben extension).

pub mod cache_sql;
pub mod conn_fields;
pub mod input_sql_raw;
pub mod input_sql_select;
pub mod output_sql_deprecated;
pub mod output_sql_insert;
pub mod output_sql_raw;
pub mod processor_sql;
pub mod sqlite;
pub mod util;

use ekiben_core::Registry;

/// The sqlx paths this crate uses, from the driver crates (see the workspace Cargo.toml).
mod sqlx {
    pub use sqlx_core::encode::Encode;
    pub use sqlx_core::{
        arguments::Arguments, column::Column, decode::Decode, encode, error, executor::Executor,
        query::query_with, raw_sql::raw_sql, row::Row, sql_str::AssertSqlSafe, sql_str::SqlSafeStr,
        statement::Statement, type_info::TypeInfo, types::Type, value::ValueRef, Either, Error,
    };
    pub mod postgres {
        pub use sqlx_postgres::*;
    }
    pub mod mysql {
        pub use sqlx_mysql::*;
    }
}

pub fn register_all(registry: &Registry) {
    input_sql_raw::register(registry);
    input_sql_select::register(registry);
    output_sql_insert::register(registry);
    output_sql_raw::register(registry);
    output_sql_deprecated::register(registry);
    processor_sql::register(registry);
    cache_sql::register(registry);
}

/// What the fuzz targets in `fuzz/` drive (cargo fuzz).
#[cfg(feature = "fuzzing")]
pub mod fuzzing {
    pub use crate::util::fuzz_values as values;

    /// Lints `conf` as every sql component does.
    pub fn lint_all(conf: &serde_json::Value) {
        for lint in [
            crate::cache_sql::lint,
            crate::input_sql_raw::lint,
            crate::input_sql_select::lint,
            crate::output_sql_insert::lint,
            crate::output_sql_raw::lint,
            crate::output_sql_deprecated::lint,
            crate::processor_sql::lint_raw,
            crate::processor_sql::lint_select,
            crate::processor_sql::lint_insert,
        ] {
            let _ = lint(conf);
        }
    }
}

/// What the tests of every module share: a stream context and a component config.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;
    use std::sync::Arc;

    use ekiben_core::{
        ComponentConfig, OrgId, OrgQuotas, Registry, ResourceManager, StreamContext, StreamMetrics,
    };

    /// A stream whose organization may reach private addresses (the CI databases), with every
    /// sql component registered; `quotas` adjusts the rest of the organization's limits.
    pub fn stream_ctx_with(root: Option<&std::path::Path>, quotas: OrgQuotas) -> StreamContext {
        let resources = ResourceManager::new();
        let org = resources.set_org_quotas(
            &OrgId::new("o"),
            OrgQuotas {
                allow_private_ips: true,
                ..quotas
            },
        );
        let registry = Registry::new();
        super::register_all(&registry);
        StreamContext {
            stream_id: "s".into(),
            org,
            resources,
            registry: Arc::new(registry),
            blobl_env: ekiben_bloblang::Environment::new(),
            metrics: StreamMetrics::new("s", "o"),
            label: String::new(),
            path: "root".into(),
            file_root: root.map(Into::into),
            env_vars: Arc::new(HashMap::new()),
            shutdown_rx: tokio::sync::watch::channel(false).1,
        }
    }

    pub fn stream_ctx(root: Option<&std::path::Path>) -> StreamContext {
        stream_ctx_with(root, OrgQuotas::default())
    }

    /// The CI database `key` names (PG_DSN, MY_DSN); a test of one runs only where it is given.
    pub fn env_dsn(key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|s| !s.is_empty())
    }

    pub fn conf(kind: &str, config: serde_json::Value) -> ComponentConfig {
        ComponentConfig {
            label: String::new(),
            component_type: kind.into(),
            config,
            processors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{conf, stream_ctx};
    use serde_json::json;

    /// A config that is no object gets lint_fields' one answer from every SQL component, and
    /// the connection lint, which reads an object, adds nothing.
    #[test]
    fn a_config_that_is_no_object_is_named_once_by_every_lint() {
        let lints: [(&str, fn(&serde_json::Value) -> Vec<String>); 8] = [
            ("sql_raw input", crate::input_sql_raw::lint),
            ("sql_select input", crate::input_sql_select::lint),
            ("sql output", crate::output_sql_deprecated::lint),
            ("sql_insert output", crate::output_sql_insert::lint),
            ("sql_raw output", crate::output_sql_raw::lint),
            ("sql_raw processor", crate::processor_sql::lint_raw),
            ("sql_select processor", crate::processor_sql::lint_select),
            ("sql_insert processor", crate::processor_sql::lint_insert),
        ];
        for (name, lint) in lints {
            for conf in [json!(null), json!("driver: postgres"), json!([1]), json!(7)] {
                assert_eq!(lint(&conf), vec!["expected object value"], "{name}: {conf}");
            }
        }
    }

    #[tokio::test]
    async fn every_component_builds_and_lints_by_its_registered_name() {
        let ctx = stream_ctx(None);
        let r = &ctx.registry;
        let lite = |extra: serde_json::Value| {
            let mut v = json!({"driver": "sqlite", "dsn": ":memory:"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let query = lite(json!({"query": "SELECT 1"}));
        let select = lite(json!({"table": "t", "columns": ["a"]}));
        let insert = lite(json!({"table": "t", "columns": ["a"], "args_mapping": "root = [1]"}));
        let input = |k: &str, v: &serde_json::Value| r.build_input(&conf(k, v.clone()), &ctx);
        assert_eq!(input("sql_raw", &query).unwrap().input_type(), "sql_raw");
        assert_eq!(
            input("sql_select", &select).unwrap().input_type(),
            "sql_select"
        );
        let output = |k: &str, v: &serde_json::Value| r.build_output(&conf(k, v.clone()), &ctx);
        assert_eq!(output("sql_raw", &query).unwrap().output_type(), "sql_raw");
        assert_eq!(
            output("sql_insert", &insert).unwrap().output_type(),
            "sql_insert"
        );
        let deprecated =
            json!({"driver": "sqlite", "data_source_name": ":memory:", "query": "SELECT 1"});
        assert_eq!(output("sql", &deprecated).unwrap().output_type(), "sql");
        let pctx = ctx.to_proc_context("sql", "", "pipeline.processors");
        for (k, v) in [
            ("sql_raw", &query),
            ("sql_select", &select),
            ("sql_insert", &insert),
        ] {
            let p = r.build_processor(&conf(k, v.clone()), &pctx, "0").unwrap();
            assert_eq!(p.processor_type(), k);
        }
        let kv = lite(json!({"table": "kv", "key_column": "k", "value_column": "v"}));
        assert!(r.build_cache(&conf("sql", kv), &ctx).is_ok());
        for (kind, name) in [
            ("input", "sql_raw"),
            ("input", "sql_select"),
            ("output", "sql_raw"),
            ("output", "sql_insert"),
            ("output", "sql"),
            ("processor", "sql_raw"),
            ("processor", "sql_select"),
            ("processor", "sql_insert"),
            ("cache", "sql"),
        ] {
            let errs = r.lint_component(kind, &conf(name, json!({"bogus": 1})));
            assert!(
                errs.iter()
                    .any(|e| e.contains("field bogus not recognised")),
                "{kind} {name}: {errs:?}"
            );
        }
    }
}
