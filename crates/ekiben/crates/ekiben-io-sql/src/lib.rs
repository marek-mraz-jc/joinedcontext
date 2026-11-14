#![forbid(unsafe_code)]
//! `ekiben-io-sql`: the sql inputs and outputs of Bento 1.22.0 (`internal/impl/sql/*.go`), one
//! module per Go file, over sqlx for the postgres, mysql and sqlite drivers.

pub mod conn_fields;
pub mod input_sql_raw;
pub mod input_sql_select;
pub mod output_sql_deprecated;
pub mod output_sql_insert;
pub mod output_sql_raw;
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
}
