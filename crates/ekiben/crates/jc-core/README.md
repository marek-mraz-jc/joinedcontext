# `jc-core` — Foundational Platform Types & Contracts

## Scope
`jc-core` is the foundational pure-Rust library crate shared across all JoinedContext (`jc`) modules. It holds:
- Tenancy primitives: `OrgId` (UUIDv7 newtype), `OrgSlug`, `ProjectId`, and `RequestCtx` (Ed25519/HMAC signed request context).
- Compile-time tenant-isolated caching: `BoundedCache<K, V>` requiring `(OrgId, GrantDigest)` key prefixes.
- RFC 7807 / RFC 9457 `ProblemDetails` wire error types.
- Resource manifest schemas (`DataSource`, `Endpoint`, `Pipeline`, `DataModel`, `Policy`, `App`, `Dashboard`, `Connector`) and KPI join AST/compiler.
- Protobuf and Apache Arrow Flight batch schemas for zero-copy in-process and gRPC split-process calls.

## Public API
- Rust library traits and structs (`jc_core::tenancy::*`, `jc_core::kinds::*`, `jc_core::problem::*`, `jc_core::cache::*`).

## Standalone Binary
None (library crate only).

## Configuration
Configured via caller structs (`CacheConfig`, `VerifyKeyConfig`); reads no environment variables or database tables directly.

## Standards Implemented
- RFC 9457 / RFC 7807 Problem Details for HTTP APIs
- RFC 9562 UUIDv7
- Apache Arrow in-memory columnar format

## Origin Today
Moves and consolidates `jc-core` from `joinedcontext-portal/crates/jc-core`.
