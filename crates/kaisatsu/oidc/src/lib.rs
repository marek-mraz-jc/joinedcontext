#![forbid(unsafe_code)]
//! `kaisatsu-oidc` (W1, W8, W11, W12):
//! - Discovery (`/.well-known/openid-configuration` and RFC 8414 `/.well-known/oauth-authorization-server`
//!   without `registration_endpoint` per T-5611).
//! - Authorization endpoint (`/protocol/openid-connect/auth`) with PKCE S256 (`plain` refused),
//!   PAR (RFC 9126), CIMD client resolution, parameterized scopes (`connector:{id}`), `prompt`,
//!   `max_age`, `acr_values`, `login_hint`, `ui_locales`, and `claims` essential `acr`.
//! - Token endpoint (`/protocol/openid-connect/token`) supporting:
//!   - `authorization_code` (single-use, 60 s, replay revokes tokens issued from it, bound to
//!     `client_id`, `redirect_uri`, `realm_id`, and PKCE verifier),
//!   - `refresh_token` (rotation, reuse detection revoking the whole family and its access
//!     tokens while the SSO session stays, DPoP binding),
//!   - `client_credentials` (`client-secret`, `client-jwt` / `private_key_jwt`, and `federated-jwt`
//!     for Kubernetes ServiceAccount tokens),
//!   - `urn:ietf:params:oauth:grant-type:token-exchange` (Standard Token Exchange V2 + `act` claim),
//!   - `urn:ietf:params:oauth:grant-type:jwt-bearer` (RFC 7523),
//!   - `password` (only when `directAccessGrantsEnabled: true`, refused for CIMD/`app-*`/`mcp-*`,
//!     requires `totp` when the person holds `mfa-required` or an enrolled OTP).
//! - Introspection (RFC 7662), Revocation (RFC 7009), UserInfo, RP-Initiated Logout, Back-Channel
//!   Logout, JWKS (`/certs`), and Outbound Guard (W12).

pub mod broker;
pub mod broker_routes;
pub mod brute_force;
pub mod cimd;
pub mod client_ip;
pub mod device;
pub mod dpop;
pub mod dpop_guard;
pub mod guard;
pub mod handlers;
pub mod redirect;
pub mod second_factor;

pub use broker::UpstreamOidcBroker;
pub use broker_routes::broker_router;
pub use brute_force::BruteForceProtector;
pub use cimd::{CimdMetadata, validate_and_build_cimd_client};
pub use client_ip::ClientIp;
pub use dpop::{
    DpopProof, issue_dpop_nonce, realm_requires_dpop_nonce, verify_dpop_nonce, verify_dpop_proof,
};
pub use dpop_guard::{
    external_htu, into_dpop_aware_response, parse_authz_header, verify_resource_dpop,
};
pub use guard::{ClusterCredentials, OutboundGuard, OutboundPolicy};
pub use handlers::{OidcState, dispatch_backchannel_logout, oidc_router, redirect_with_params};
pub use redirect::{validate_post_logout_redirect_uri, validate_redirect_uri};
pub use second_factor::{ORG_SECOND_FACTOR_ATTRIBUTE, SecondFactor, second_factor_required};
