#![forbid(unsafe_code)]
//! Core domain model, authorization contract, store trait, OData service definitions, and
//! operation registry for `dataroom`.

pub mod auth;
pub mod branding;
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
pub use branding::{
    is_safe_logo_filename, is_valid_hex_colour, localized_text, parse_rgb,
    publisher_badge_svg_data_uri, resolve_publisher_branding, validate_and_sanitise_logo,
    wcag_contrast_ratio, yiq_foreground, BrandingBlock, BrandingColours, BrandingFonts,
    BrandingLanguages, PublisherBranding, SanitisedAsset,
};
pub use error::{DataroomError, Result};
pub use model::*;
pub use odata_defs::{dataroom_service, scope_for_context};
pub use ops::{all_operations, execute_operation, OpContext, OperationDef};
pub use store::{DatasetFilter, Store};
