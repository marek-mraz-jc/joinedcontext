#![forbid(unsafe_code)]
//! `kaisatsu-core`: domain types, RFC 9457 Problem errors, `Store` and `Authorizer` traits,
//! operation registry, OData entity definitions, branding block, and token claim mapper evaluation.

pub mod authz;
pub mod branding;
pub mod claims;
pub mod error;
pub mod model;
pub mod odata_defs;
pub mod ops;
pub mod store;

pub use authz::{
    Action, Authorizer, Decision, LocalAuthorizer, ObjectTarget, Principal, RequestCtx,
};
pub use branding::{BrandingBlock, Colours, Fonts, Languages};
pub use claims::{MapperContext, TokenKind, evaluate_mappers, known_mapper_kinds};
pub use error::{KaisatsuError, OAuthErrorResponse, ProblemDetails, Result};
pub use model::*;
pub use odata_defs::{kaisatsu_odata_service, scope_for_context};
pub use ops::{ODataKind, ODataOperation, ODataParam, OperationDef, OperationRegistry};
pub use store::Store;
// ci timing probe (jc-ci-fast), reverted in the next commit
