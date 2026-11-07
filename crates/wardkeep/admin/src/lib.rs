#![forbid(unsafe_code)]
//! `wardkeep-admin` (W1, W4, W5, W6, W11):
//! - Keycloak Admin REST compatibility routes (`/admin/realms/{realm}/...`) covering every row of
//!   `crates/wardkeep/COMPAT.md`.
//! - OData v4.01 entity set endpoints (`/api/v1/realms/{realm}/odata/...`) for all 23 entity sets
//!   (`$metadata`, `$filter`, `$orderby`, `$select`, `$top`, `$skip`, `$count`, `$export`, `$allowed`).
//! - Registry-driven native REST endpoints (`/api/v1/realms/{realm}/...`) including `/allowed`.
//! - SCIM 2.0 server (`/api/v1/realms/{realm}/scim/v2/{Users,Groups}`) scoped to an organization
//!   with filter parser (`eq`, `co`, `sw`, `and`) and bounded recursion depth (Kanidm
//!   GHSA-2pm5-6m23-h692 regression protection).

pub mod compat;
pub mod odata;
pub mod scim;

pub use compat::{admin_router, extract_admin_ctx, AdminState};
pub use odata::odata_router;
pub use scim::{parse_scim_filter, scim_router, ScimExpr};
