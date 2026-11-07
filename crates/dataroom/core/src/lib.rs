#![forbid(unsafe_code)]
//! Core domain model, authorization contract, store trait, OData service definitions, and
//! operation registry for `dataroom`.

pub mod auth;
pub mod ckan_compat;
pub mod error;
pub mod model;
pub mod odata_defs;
pub mod ops;
pub mod store;
pub mod vocab;

pub use auth::{
    Action, Authorizer, Decision, GrantRule, ObjectTarget, PolicyAuthorizer, RequestCtx, Subject,
};
pub use error::{DataroomError, Result};
pub use model::*;
pub use odata_defs::{dataroom_service, scope_for_context};
pub use ops::{execute_operation, all_operations, OpContext, OperationDef};
pub use store::{DatasetFilter, Store};
