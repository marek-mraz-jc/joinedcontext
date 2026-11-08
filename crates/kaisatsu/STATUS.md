# kaisatsu Implementation & Test Status

Reviewed 2026-11-07 after the Argon build was made to compile. **done** means a passing test
covers the claim; **partial** names what is missing. The Argon version of this file marked every
row done; the rows below say where that was false.

Checks on branch `kaisatsu-argon-1`: every kaisatsu package and `jc-router` compiles, all tests
pass, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean. Line coverage
(`cargo +nightly llvm-cov`): 71.95 % over the kaisatsu packages and `jc-router`, short of the 95 % target (see the end of this file).

| Item | Description | Status | Verification / what is missing |
|---|---|---|---|
| **W1** | Drop-in wire surface: discovery, authorize, PAR, token grants, PKCE S256, DPoP, refresh rotation, token exchange V2 + `act`, jwt-bearer, federated-jwt, introspection, revocation, userinfo, RP/back-channel logout, CIMD, parameterized scopes, RI mappers, password hash migration | **partial** | Done and tested: `server/tests/e2e_protocol_and_security_tests.rs`, `server/tests/security_regressions.rs`, `server/tests/dpop_binding.rs`, `core/src/claims.rs`, `crypto/src/password.rs`, `oidc/src/dpop.rs`. Missing: refresh tokens are signed JWTs, not opaque handles; the CIMD cache never refreshes; DPoP-bound tokens are enforced at userinfo but not on the admin, OData, MCP and SCIM APIs (a stolen bound token still works there as a Bearer token); no server DPoP nonce is ever issued. |
| **W1b** | Declarative config apply (JSON/YAML realm files, `check`, `plan`/`apply` with stale-plan refusal, `diff`, `export` round-trip, `--prune` preserving Portal objects, secret substitution, RI + `keycloak-clients.yaml` fixture test) | **partial** | Apply, prune, stale-plan refusal and secret substitution: `import/tests/import_tests.rs`. Missing: `.yaml`/`.yml` files are parsed as JSON (no YAML parser is among the allowed crates), so a real YAML realm file fails; the CLI `plan`, `diff` and `export` run against an empty in-memory store, not a server or database; `serve --import-dir` applies once at start, no watch. |
| **W2** | PostgreSQL store with forced RLS, leakproof indexes, `verify_boot_rls`, `FOR UPDATE SKIP LOCKED` sweeper, `LISTEN/NOTIFY`, `MemoryStore`, shared isolation suite | **partial** | `MemoryStore` passes the shared isolation suite (`store-pg/tests/isolation_suite.rs`). Missing: `PgStore` keeps every record in an embedded `MemoryStore` (85 delegations, 6 SQL statements), so nothing persists; the migrations are not idempotent, carry no version table and grant nothing; `serve` never uses Postgres. The live Postgres test skips without `KAISATSU_TEST_DATABASE_URL` and was not run (no Docker in the review sandbox). |
| **W3** | Tenancy: realms, organizations, verified domains, memberships, group subtrees, composite roles with cycle detection, service-account users, transient users, cross-org isolation | **partial** | Realms, organizations, memberships, groups, service-account users, cross-org isolation: `admin/tests/compat_tests.rs`, `admin/tests/authz_regressions.rs`, `store-pg/tests/isolation_suite.rs` (memory store). Missing: domain verification is not implemented (`OrganizationDomain/Verify` answers 501 `not-supported`); composite-role cycle detection has no test with a cycle. |
| **W4** | OData 4.01 for the 23 entity sets via `odata-table` (`$filter`, `$orderby`, `$select`, `$top`, `$skip`, `$count`, `$metadata`, `$export`, `$allowed`, secret fields unselectable and unfilterable) | **done** | `server/tests/odata_service.rs::entity_sets_answer_queries_count_export_and_allowed_over_http`, `::metadata_lists_every_registry_operation`, `store-pg/tests/isolation_suite.rs`. Limits: `$export` returns at most 1000 rows as one JSON array (no streaming); `$allowed` looks the key up among the first 1000 visible rows. |
| **W5** | One operation registry serving REST, OData, MCP, CLI and `/allowed` without revealing hidden records | **partial** | Since 2026-11-07 (owner order "all APIs that should be called should be OData") the native API is one OData 4.01 service: entity sets plus every non-CRUD registry operation as a bound or unbound action or function in the same `$metadata` (`Person/ResetCredentials`, `Person/RevokeSessions`, `Person/SendRequiredActions`, `Realm/Apply` with `dryRun`, `Realm/Export`, `SigningKey/Rotate`, `OrganizationDomain/Verify`, `Consent/Revoke`, `Allowed(operation, object)`, …), with `Core.OperationId` naming the registry operation. Errors are OData error JSON whose `code` is the RFC 9457 slug. OData, MCP `tools/call`, SCIM user writes and the native REST routes run one executor (`kaisatsu_admin::execute`). Tests: `server/tests/odata_service.rs` (`$metadata` lists every registry operation; `Allowed` and `ResetCredentials` give the same answer through OData, MCP and REST; MCP lists only permitted tools). Missing: the CLI has no per-operation subcommands, only `check`/`plan`/`apply`/`diff`/`export`/`migrate`/`serve`. The Argon MCP answered a fake `ok` for every write tool; that is replaced. |
| **W6** | `Authorizer` trait with `LocalAuthorizer` (`realm-admin`, `org-admin`, `people-manager`, `application-manager`, `auditor`, `self`), 404 on cross-org, `AdminEvent` audit rows | **done** | `admin/tests/authz_regressions.rs` (7 tests), `admin/tests/compat_tests.rs`, `server/tests/scim_service.rs`. Several routes skipped the Authorizer in the Argon build (see Security below). |
| **W7** | Login state machines (unit test per edge, property test that no path reaches `Code` without every factor), brute force, Askama pages, `en`/`sk`/`cs`/`de`/`hu`, `lettre` e-mail with 15-min single-use signed links | **partial** | Machine, property test, brute force, i18n, TOTP enrolment: `login/src/machine.rs`, `oidc/src/brute_force.rs`, `login/src/i18n.rs`, `server/tests/security_regressions.rs`. Missing: no route consumes action links (`mint_action_token`/`consume_action_token` are only called by their own test); `execute-actions-email` and `Person/SendRequiredActions` send nothing (501 without SMTP, otherwise no mail is built); the reset-credentials page ignores its form; passkey registration answers 501. |
| **W8** | Single sign-on across applications, per-application consent, per-application `acr` step-up (`mfa`), back-channel logout | **done** | `server/tests/e2e_protocol_and_security_tests.rs` (Portal login with OTP step-up and back-channel logout; CIMD connector with consent). |
| **W9** | Library crates + `kaisatsu_server::router(state, mount)` + `crates/shared/jc-router` (slugs on one domain, dedicated host, overlap refusal, cookie prefixes, trusted-proxy client address) | **done** | `crates/shared/jc-router/src/lib.rs` (incl. `client_addr_trusts_forwarded_for_only_from_trusted_proxies`), `oidc/src/client_ip.rs`, `server/tests/e2e_protocol_and_security_tests.rs`. |
| **W10** | Keys & crypto: ES256 default, RS256 for `edge`, EdDSA, AES-256-GCM at rest under a KEK file, rotation, `jsonwebtoken` 11 + `aws-lc-rs`, `deny.toml` bans | **done** | `crypto/src/jose.rs`, `crypto/src/totp.rs`, `server/tests/odata_service.rs` (`SigningKey/Rotate`), `deny.toml`. `cargo deny` was not run in this review. |
| **W11** | Federation: upstream OIDC broker + first-broker-login by verified e-mail, Kubernetes federated-jwt, SCIM 2.0 with bounded filter depth | **partial** | federated-jwt: `server/tests/security_regressions.rs`; SCIM: `server/tests/scim_service.rs`; broker library: `oidc/src/broker.rs` tests. Missing: the broker is wired to no route (no upstream code exchange, no callback, no link confirmation page), so upstream login cannot be used; SCIM has no PUT/PATCH and no group membership. |
| **W12** | `#![forbid(unsafe_code)]`, RFC 9457 slugs, strict redirect URIs, single-use 60 s codes with replay revocation, `OutboundGuard` SSRF protection, advisory regression tests | **done** | `oidc/src/guard.rs`, `oidc/src/redirect.rs`, `server/tests/e2e_protocol_and_security_tests.rs`, `server/tests/security_regressions.rs`. Code replay ends the SSO session (introspection and userinfo refuse its tokens); access tokens already issued stay valid as JWTs at resource servers that do not introspect. |
| **COMPAT.md rows 1–4** | Realms and `partialImport` | **done** | `admin/tests/compat_tests.rs::row_01_02_03_realms_and_partial_import` |
| **COMPAT.md rows 5–17** | Users CRUD, `execute-actions-email`, `reset-password`, `logout`, `credentials`, `sessions`, `consents` | **partial** | `admin/tests/compat_tests.rs::row_05_to_17_users_lifecycle_and_subresources`; `execute-actions-email` answers but sends no mail (see W7). |
| **COMPAT.md rows 18–34** | Groups, memberships, client roles, role mappings | **done** | `admin/tests/compat_tests.rs::row_18_to_34_groups_roles_and_mappings`, `admin/tests/authz_regressions.rs` |
| **COMPAT.md rows 35–46** | Clients, `client-secret`, `service-account-user`, mappers, client scopes, `attack-detection/brute-force` | **done** | `admin/tests/compat_tests.rs::row_35_to_46_clients_secrets_mappers_and_scopes`, `admin/tests/authz_regressions.rs::application_and_group_subresources_are_checked_by_the_authorizer` |

## Security fixes in this review

Each has a test that fails on the code before the fix.

- jwt-bearer grant minted a token for any `sub` the client named (impersonation), with no `jti` replay check.
- federated-jwt and `private_key_jwt` assertions were accepted for any audience; a disabled federated client still authenticated.
- HTTP Basic secrets were cut at a raw `=`; an empty stored secret matched an empty presented one.
- `client_credentials` took any person named `service-account-{client}` as the principal; deleting a client deleted that person.
- The password grant had no brute-force protection; a wrong OTP could be retried without limit inside one login.
- Brute force counted every request under the hard-coded address `127.0.0.1` (one person could lock out everybody); the client address now comes from the peer or trusted proxies.
- Browser TOTP enrolment stored the constant secret `enrol-seed-secret-256` for everybody.
- A public client could run token exchange; userinfo answered for tokens of an ended session or a revoked token.
- A person could give themselves `realm-admin` through `PUT /users/{own id}` or their own role mappings; roles were merged across realms and from role names; `platform-admin`/`admin` counted as realm admin; a revoked admin token was accepted.
- Internal error details (storage, crypto) reached callers; SSRF guard missed IPv6-embedded IPv4, CGNAT and other reserved ranges.
- A DPoP-bound token was accepted at userinfo as a plain Bearer token.
- SCIM `DELETE`/`GET /Users/{id}` and `GET`/`POST /Groups` ran no authorization: any member could delete or read every person of their organization; a group name with `/` landed in another path; SCIM marked addresses verified.
- Ten compat routes on client scope links, mappers, role holders and group members ran no authorization: a member could attach any client scope to an application of their organization.
- The (unwired) broker did not check the login nonce and linked an existing local account by e-mail alone.

## Open risks

- Per-address brute-force locking hits every person behind one NAT address.
- DPoP binding is not enforced on the admin, OData, MCP and SCIM APIs.
- A person may delete their own sessions through self-service (intended by the RI's self rules, not reviewed against the Portal).
- The pending TOTP secret sits unencrypted in the auth session record until enrolment completes.
- Group membership adds by an org admin were not reviewed for groups that carry realm roles.

## Coverage

| Package | Lines | Missed | Line coverage |
|---|---|---|---|
| kaisatsu-store-pg | 713 | 713 | 0.0 % |
| kaisatsu-server | 233 | 198 | 15.0 % |
| kaisatsu-store-memory | 2047 | 628 | 69.3 % |
| kaisatsu-admin | 4400 | 1187 | 73.0 % |
| kaisatsu-oidc | 3916 | 997 | 74.5 % |
| kaisatsu-crypto | 992 | 236 | 76.2 % |
| kaisatsu-login | 1726 | 394 | 77.2 % |
| kaisatsu-import | 946 | 198 | 79.1 % |
| kaisatsu-core | 2690 | 512 | 81.0 % |
| jc-router | 381 | 28 | 92.7 % |
| kaisatsu-mcp | 108 | 1 | 99.1 % |
| **total** | 18152 | 5092 | **71.95 %** |

Not covered: `store-pg` (needs a live Postgres; its
code is mostly the stub described in W2), `server/src/main.rs` (the CLI binary, no test drives
it), and the larger unhappy paths of `compat.rs`, `oidc/src/handlers.rs` and
`login/src/handlers.rs`. Reaching 95 % needs about 4,200 more covered lines of real-behaviour
tests; it was 66.6 % before the tests added in this review.
