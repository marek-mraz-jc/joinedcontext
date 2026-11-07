# `jc-auth` — Token Verification, DPoP & mTLS Identity Library

## Scope
`jc-auth` provides stateless cryptographic verification and request authentication primitives used by `edge`, `ekiben`, `broker`, `portal`, `forge`, and standalone module binaries:
- OIDC / OAuth 2.1 JWT signature and claim verification (`iss`, `aud`, `exp`, `nbf`, `org_id`, `azp`).
- RFC 9449 DPoP proof verification (`htm`, `htu`, `ath`, JWK thumbprint binding).
- Constant-time bearer token and webhook HMAC comparison (`subtle::ConstantTimeEq`).
- Internal mTLS peer certificate SPIFFE ID extraction (`spiffe://jc.internal/ns/{cell}/sa/{module}`).
- Minting and verifying short-lived signed `RequestCtx` headers between `edge` and downstream modules.

## Public API
- `TokenVerifier`, `DpopVerifier`, `ConstantTimeBearer`, `MtlsIdentityExtractor`, and `axum` middleware layers.

## Standalone Binary
None (library crate only).

## Configuration
- `issuer_url` / JWKS endpoint or static public key set
- `trusted_ca_pem` for internal mTLS
- `clock_skew_leeway_secs` (default `5`)

## Standards Implemented
- RFC 7519 (JSON Web Token), RFC 7517 (JSON Web Key)
- RFC 9449 (OAuth 2.0 Demonstrating Proof of Possession - DPoP)
- SPIFFE X.509-SVID profile

## Origin Today
Extracted from `context-gateway` auth middleware and `joinedcontext-portal` token verification.
