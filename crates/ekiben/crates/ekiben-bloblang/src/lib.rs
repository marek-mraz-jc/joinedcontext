#![forbid(unsafe_code)]
//! `ekiben-bloblang` — The Bloblang mapping language interpreter.
//!
//! Pure synchronous library with no `tokio` or network dependencies so that
//! `ekiben-core`, the Portal mapping preview, and the App SDK can embed it.

pub mod ast;
pub mod cli;
pub mod env;
pub mod error;
pub mod executor;
pub mod functions;
pub mod go_fmt;
pub mod go_json;
pub mod go_sort;
mod html_entities;
pub mod json_schema;
pub mod methods;
pub mod methods_crypto;
pub mod methods_formats;
pub mod methods_numbers;
pub mod methods_strings;
pub mod methods_structured;
pub mod methods_time;
pub mod msgpack;
pub mod parser;
mod required_params;
pub mod time_format;
pub mod value;

pub use ast::{AssignTargetPath, AssignTargetType, TargetPath, TargetType, TargetsContext};
pub use env::{Environment, Importer};
pub use error::{BloblangError, ParseError};
pub use executor::{
    DetailedErrorInfo, EmptyBatch, Executor, FieldExpression, MessageBatch, PartMetaMut,
    SimplePartView,
};
pub use value::{dot_path_to_slice, Value};
