# `identity` (`jc-identity`) — OIDC Provider, OAuth 2.1 Server, Digital Credentials & Zanzibar Authorization

## Scope
`identity` provides multi-organization authentication, federation, digital credential issuance/verification, and fine-grained authorization:
- Relational OpenID Connect 1.0 Provider (OP) and OAuth 2.1 Authorization Server (AS) with PKCE S256, DPoP (RFC 9449), Token Exchange (RFC 8693), Token Revocation (RFC 7009), Resource Indicators (RFC 8707), and MCP Client ID Metadata Documents (CIMD).
- MFA via WebAuthn Level 3 passkeys (`webauthn-rs`) and TOTP.
- Digital Wallets: OpenID4VP 1.0 verifier and OpenID4VCI 1.0 issuer (HAIP 1.0, SD-JWT VC, ISO mdoc).
- Zanzibar-style relational authorization service (`Check`, `ListObjects`, `Expand`, `Grant`) over `authz_*` tables.
- Internal Certificate Authority issuing short-lived mTLS X.509 certificates for split-mode processes.

## Public API
- HTTP: `/.well-known/openid-configuration`, `/oauth2/authorize`, `/oauth2/token`, `/oauth2/revoke`, `/oauth2/introspect`, `/oauth2/jwks.json`, `/oid4vp/*`, `/oid4vci/*`, `/api/v1/identity/*`
- gRPC / In-Process: `jc_identity_api::AuthzService` (`Check`, `ListObjects`, `Expand`, `Grant`) and `IdentityService`

## Standalone Binary
`jc-identity` (usable as a standalone OIDC/OAuth 2.1 + Zanzibar server for any software, needing only PostgreSQL).

## Configuration
- `DATABASE_URL` (role `jc_identity`, schemas `identity` and `authz`)
- `IDENTITY_ISSUER_URL`
- `IDENTITY_KEY_ENCRYPTION_KEY` (KMS/OpenBao URI or master key for signing key material)
- `IDENTITY_LISTEN_ADDR` (default `127.0.0.1:8082`)

## Standards Implemented
- OpenID Connect Core 1.0, Discovery 1.0, RP-Initiated Logout 1.0
- OAuth 2.1, RFC 7636 (PKCE), RFC 8693 (Token Exchange), RFC 9449 (DPoP), RFC 7009, RFC 8707
- W3C WebAuthn Level 3, RFC 6238 (TOTP)
- OpenID4VP 1.0, OpenID4VCI 1.0, IETF SD-JWT VC

## Origin Today
Replaces **Keycloak** (Java), the Portal's permission resolver, the Context Gateway PDP grant lookup, and Linkerd's identity CA.
