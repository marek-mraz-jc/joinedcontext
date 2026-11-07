//! Portal authentication: Keycloak OIDC authorization code flow with PKCE (CC-40)
//! and encrypted cookie sessions (docs/Architecture/09-portal.md §1).
//!
//! Tokens live in an encrypted, `HttpOnly` cookie — never in `localStorage`, never in a log line.
//! Services call the API with `Authorization: Bearer`; the Portal verifies that token itself
//! (ES256 or RS256 against the realm JWKS). Behind the APISIX edge the `openid-connect` plugin
//! does the login and hands the user's token over as `X-Access-Token`, which the Portal verifies
//! the same way when `JC_TRUST_EDGE_TOKEN` is set; the cookie flow stays as the fallback for an
//! installation without an edge (ADR-N-019, AP-27, AP-28).

pub mod bearer;
pub mod client_auth;
pub mod csrf;
pub mod internal;
pub mod oidc;
pub mod refresh;
pub mod session;

pub use session::{CurrentUser, Front, Identity, Session};
