# kaisatsu Implementation & Test Status

Reviewed 2026-11-08 on branch `tests-kaisatsu-1`. **done** means a passing test covers the claim;
**partial** names what is missing.

Checks: every kaisatsu package compiles, all tests pass with `KAISATSU_TEST_DATABASE_URL` set
against a live PostgreSQL 17 (no skipped Postgres test), `cargo test -p jc` passes,
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean. Coverage at the end
of this file: 95.9 % lines and 91.4 % functions over the kaisatsu packages. The target was 95 % for
both, per package; six packages fall short on functions and four on lines (see Coverage).

| Item | Description | Status | Verification / what is missing |
|---|---|---|---|
| **W1** | Drop-in wire surface: discovery, authorize, PAR, token grants, PKCE, DPoP, refresh rotation, token exchange V2 + `act`, jwt-bearer, federated-jwt, introspection, revocation, userinfo, RP/back-channel logout, CIMD, parameterized scopes, RI mappers, password hash migration | **partial** | `server/tests/oidc_flows.rs` (discovery, JWKS, authorization errors never redirected, SSO, `prompt`, `max_age`, step-up, consent, every `response_mode` including interactive login, PAR, userinfo, RP logout with back-channel), `server/tests/token_endpoint.rs` (client authentication refusals, `private_key_jwt`, federated-jwt, authorization_code, client_credentials, introspection, revocation, password grant with TOTP, token exchange, jwt-bearer), `server/tests/refresh_tokens.rs` (opaque `{family}.{secret}` handles, compare-and-swap rotation, reuse revokes the family and ends the session), `server/tests/dpop_binding.rs`, `oidc/tests/cimd_rules.rs`, `server/tests/e2e_protocol_and_security_tests.rs`. Missing: the CIMD cache never refreshes; DPoP-bound tokens are enforced at userinfo but not on the admin, OData, MCP and SCIM APIs; no server DPoP nonce is issued; `oidc-sha256-pairwise-sub-mapper` returns the plain user id. |
| **W1b** | Declarative config apply (JSON/YAML realm files, `check`, `plan`/`apply` with stale-plan refusal, `diff`, `export`, `--prune`, secret substitution) | **partial** | `import/tests/import_tests.rs`, `import/tests/check_rules.rs`, `server/tests/cli.rs` (every subcommand against PostgreSQL, password inside the URL refused, `serve` stops cleanly on SIGTERM). `.yaml`/`.yml` files are parsed as YAML. An unset secret variable refuses the apply instead of storing a placeholder; an OTP seed obeys the literal-secret rule. Missing: `serve --import-dir` applies once at start, no watch; re-applying a realm fails when `RealmConfig` default references point at objects created in the same file. |
| **W2** | PostgreSQL store with forced RLS, `verify_boot_rls`, `FOR UPDATE SKIP LOCKED` sweeper, `LISTEN/NOTIFY`, `MemoryStore`, shared isolation suite | **done** | `PgStore` is a real store: one transaction per call setting `kaisatsu.realm`/`kaisatsu.orgs`, forced RLS, versioned migrations granting data rights only, AES-256-GCM secrets under the KEK. `store-pg/tests/pg_store.rs` (persistence across instances, RLS through `kaisatsu_app`, boot refusals, encryption at rest, code redemption race, NOTIFY), `store-pg/tests/store_contract.rs` and `store-pg/tests/isolation_suite.rs` against both stores. Limits: OData queries on PostgreSQL load the realm's rows into a `MemoryTable` before filtering; the People RLS policy has no group path-root fallback; `jc` (the one binary) still mounts kaisatsu on the memory store. |
| **W3** | Tenancy: realms, organizations, verified domains, memberships, group subtrees, composite roles, service-account users, cross-org isolation | **partial** | `admin/tests/compat_routes.rs`, `admin/tests/compat_tests.rs`, `admin/tests/authz_regressions.rs`, `core/tests/authz_matrix.rs`, `store-pg/tests/isolation_suite.rs`. Missing: domain verification (`OrganizationDomain/Verify` answers 501 `not-supported` after the visibility check); the organization pinned by the `organization:` scope is not checked against the person's memberships. |
| **W4** | OData 4.01 for the entity sets via `odata-table` | **done** | `server/tests/odata_service.rs`, `server/tests/oidc_flows.rs::every_entity_set_reads_the_rows_a_login_leaves`. Limits: `$export` returns at most 1000 rows as one JSON array; `$allowed` looks the key up among the first 1000 visible rows. |
| **W5** | One operation registry serving REST, OData, MCP, CLI and `/allowed` | **partial** | `server/tests/odata_service.rs`, `admin/tests/compat_routes.rs` (Allowed judges the object's own organization). Missing: the CLI has no per-operation subcommands. |
| **W6** | `Authorizer` with `LocalAuthorizer`, 404 on cross-org, `AdminEvent` audit rows | **done** | `core/tests/authz_matrix.rs` (decision table), `admin/tests/authz_regressions.rs`, `admin/tests/compat_routes.rs`, `server/tests/scim_service.rs`. Open: `view-clients` grants write on roles and client scopes. |
| **W7** | Login state machines, brute force, pages in `en`/`sk`/`cs`/`de`/`hu`, `lettre` e-mail with signed action links | **partial** | `server/tests/login_pages.rs` (every login page, required actions `TERMS_AND_CONDITIONS` and `UPDATE_PASSWORD` complete with policy and history checks, TOTP, organization choice), `login/tests/email_paths.rs` (SMTP refusals without leaking the password, action-link type, issuer and key checks), `login/src/machine.rs`, `oidc/src/brute_force.rs`. Missing: no e-mail is ever sent (no outbox, no route sends; `Person/SendRequiredActions` stores the actions and sends nothing, the reset-credentials page ignores its form); no route consumes action links; passkey registration answers 501; `RecoveryCode` and `VerifyEmail` states have no POST handler; a failed combined password+OTP POST does not keep the password step. |
| **W8** | Single sign-on, per-application consent, `acr` step-up, back-channel logout | **done** | `server/tests/oidc_flows.rs`, `server/tests/e2e_protocol_and_security_tests.rs`. |
| **W9** | Library crates + `kaisatsu_server::router(state, mount)` + `jc-router` | **done** | `server/tests/*`, `crates/shared/jc-router/src/lib.rs`. |
| **W10** | Keys & crypto: ES256, RS256, EdDSA, AES-256-GCM under a KEK, rotation, `jsonwebtoken` + `aws-lc-rs` | **done** | `crypto/tests/crypto_paths.rs`, `crypto/src/*` unit tests, `admin/tests/compat_routes.rs` (rotation refuses unknown algorithms). `cargo deny` was not run. |
| **W11** | Federation: upstream OIDC broker, Kubernetes federated-jwt, SCIM 2.0 | **partial** | federated-jwt: `server/tests/token_endpoint.rs`; SCIM: `server/tests/scim_service.rs`; broker library: `oidc/src/broker.rs` tests. Missing: the broker is wired to no route; SCIM has no PUT/PATCH and no group membership. |
| **W12** | `#![forbid(unsafe_code)]`, RFC 9457 slugs, strict redirect URIs, single-use codes, `OutboundGuard` SSRF protection | **done** | `oidc/tests/guard_paths.rs` (real local HTTP server: schemes, userinfo, allow-list, literal and resolved forbidden addresses, redirects not followed, 1 MiB cap declared and streamed), `core/tests/errors_branding_defs.rs`, `server/tests/security_regressions.rs`. |
| **COMPAT.md rows 1–46** | Keycloak-compatible admin routes | **partial** | `admin/tests/compat_routes.rs` (each route: 401, 404 on unknown and foreign ids, lifecycles), `admin/tests/compat_tests.rs`. `execute-actions-email` sends no mail (see W7). |

## Security fixes

Each has a test that was run red against the code before the fix (fix line reverted once).

Branch `tests-kaisatsu-1` (2026-11-08):

- `PgStore` kept every record in an embedded `MemoryStore`: nothing persisted and RLS decided nothing (real store with forced RLS, boot check refuses superuser, owner, BYPASSRLS and unforced tables).
- Refresh tokens were reusable signed JWTs; now opaque, rotated by compare-and-swap, reuse revokes the family.
- A password inside `--database-url` was accepted; an unset secret variable stored the placeholder `resolved:NAME` as the client secret; the SMTP password variable was stored as written.
- Compat routes: an organization admin could move a group or application to another organization, edit or delete a realm-wide client scope, unlock any user id; a mapper update skipped the kind check; `Allowed` judged the caller's organization; removing the last mapper kept it; key rotation fell back to ES256 for HS256.
- A token without `cnf.jkt` was accepted under the `DPoP` authorization scheme.
- `verify_jwt_with_jwk` accepted a token that left out `iss` or `aud` when they were expected; token exchange with no audience in the subject token is refused.
- Realm appliers could not reach realm-level objects (`check_org_scope`).
- SVG logos: `<style>`, `@import`, event handlers after `/` or quotes, and `href`/`url()` to outside resources passed `is_safe_svg`.
- A temporary password or `UPDATE_PASSWORD` led to a page whose POST answered 400 (sign-in could never finish); the new password now passes the policy and history.
- A literal OTP seed in a realm file bypassed the literal-secret rule.
- `serve` did not shut down gracefully on SIGTERM.

Earlier review (2026-11-07): jwt-bearer impersonation and replay; any-audience assertions;
Basic secret parsing and empty-secret match; `service-account-{client}` lookup by name; password
grant brute force; brute force keyed on `127.0.0.1`; constant TOTP enrolment secret; public
clients running token exchange; userinfo after session end; self-granted `realm-admin`; internal
error details; SSRF guard gaps; DPoP-bound token as Bearer at userinfo; SCIM and ten compat routes
without authorization; broker nonce and e-mail-only linking.

## Open risks

- The auth session cookie is never checked against the session code it carries.
- Logout without `id_token_hint` ends the session without confirmation.
- Per-address brute-force locking hits every person behind one NAT address.
- DPoP binding is not enforced on the admin, OData, MCP and SCIM APIs.
- `view-clients` grants write on roles and client scopes.
- The `organization:` scope pins an organization without checking membership.
- The pending TOTP secret sits unencrypted in the auth session record until enrolment completes.
- No branding writer exists; branding is read only.

## Coverage

`cargo +nightly llvm-cov --no-cfg-coverage` over the ten kaisatsu packages, with
`KAISATSU_TEST_DATABASE_URL` set (all tests green, none skipped).

| Package | Lines before | Lines after | Functions before | Functions after |
|---|---|---|---|---|
| kaisatsu-admin | 73.02 % | 94.29 % | 52.48 % | 89.23 % |
| kaisatsu-core | 80.97 % | 99.48 % | 68.24 % | 98.26 % |
| kaisatsu-crypto | 76.21 % | 94.99 % | 56.78 % | 82.20 % |
| kaisatsu-import | 79.07 % | 91.55 % | 65.00 % | 85.90 % |
| kaisatsu-login | 77.17 % | 92.26 % | 64.75 % | 79.23 % |
| kaisatsu-mcp | 99.07 % | 99.07 % | 85.71 % | 85.71 % |
| kaisatsu-oidc | 74.54 % | 95.74 % | 59.51 % | 90.27 % |
| kaisatsu-server | 15.02 % | 95.92 % | 19.05 % | 90.48 % |
| kaisatsu-store-memory | 69.32 % | 98.59 % | 66.14 % | 97.92 % |
| kaisatsu-store-pg | 2.38 % | 97.15 % | 3.67 % | 94.64 % |
| **total** | 71.60 % | **95.89 %** | 56.66 % | **91.43 %** |

Short of 95 %: the uncovered functions in `crypto` are mostly error closures of calls that cannot
fail with valid constants (CSPRNG, `Params::new`, key generation, `Hmac::new_from_slice`,
`UnboundKey::new`); in `login` the unhappy paths of `handlers.rs` (brute-force lock during the
password step, recovery code, missing session records) and the unused e-mail paths; in `admin` and
`import` the storage-failure branches; `mcp` misses one function. `serve` paths count only because
the process now exits gracefully; a killed process writes no profile.
