# kaisatsu Implementation & Test Status

Updated 2026-11-08 by the finishing run (kaisatsu-finish) on branch `kaisatsu-argon-1`. Every crate
under `crates/kaisatsu/*` and `bins/jc` compiles; `cargo fmt --check`, `cargo clippy --all-targets -- -D
warnings` and `cargo test` pass (298 tests, 0 failed, 0 skipped), the store-pg and jc PostgreSQL tests
included: they ran against a real PostgreSQL 17 (`KAISATSU_TEST_DATABASE_URL`, docker in the sandbox).

## 1. K1–K17 Item Status

"Red proof" names the guard removed in a scratch worktree and the test that then failed
(`evidence/kaisatsu-finish/redproof*.log`, 27 cases, all red).

| Item | Status | Tests | Red proof (guard removed -> test red) |
|---|---|---|---|
| **K1** auth session cookie, constant time | done | `server/tests/login_pages.rs::k1_session_code_replayed_from_second_browser_is_refused` | `auth_cookie_matches` check |
| **K2** `organization:` scope vs memberships | done | `server/tests/security_regressions.rs::k2_organization_scope_checks_membership_and_never_emits_foreign_org_claim` | `user_is_member_of_org` |
| **K3** `view-clients` read-only on roles, client scopes | done | `admin/tests/authz_regressions.rs::k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes`, `core/tests/authz_matrix.rs` | `can_write` role list; managed-App client-role rule (`client_role_write`) |
| **K4** pairwise subject | done | `core/tests/claims_mappers.rs::k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id`, `server/tests/oidc_flows.rs::k4_pairwise_sub_agrees_across_tokens_userinfo_and_introspection`, `::k4_userinfo_refuses_a_sub_that_is_not_the_clients_subject` | mapper hash; userinfo subject compare |
| **K5** logout confirmation, single-use CSRF token | done | `server/tests/oidc_flows.rs::k5_logout_without_hint_needs_a_confirmed_single_use_token`, `::rp_initiated_logout_and_backchannel_dispatch` | confirmation branch; single-use mark |
| **K6** DPoP on Admin, OData, MCP, SCIM + server nonce | done | `server/tests/dpop_binding.rs::k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip`, `server/tests/dpop_errors.rs` | DPoP scheme for bound tokens; `jkt` match; RS nonce; `jti` replay |
| **K7** recovery-code and verify-email POSTs | done | `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys`, `::k7_recovery_code_single_use_holds_under_a_concurrent_submit` | atomic recovery-code use mark |
| **K8** failed combined password+OTP keeps the step | done | `server/tests/login_pages.rs::k7_k8_k10_k11_...` | `is_combined_pw_otp` render state |
| **K9** e-mail outbox, `SKIP LOCKED` dispatcher, sender | done | `store-pg/tests/odata_pushdown_and_outbox.rs::pg_store_outbox_and_webauthn_contract`, `::pg_outbox_claim_skips_rows_another_dispatcher_holds`, `login/tests/outbox_dispatch.rs` | `status = 'pending'` claim filter; `FOR UPDATE SKIP LOCKED` |
| **K10** required-action mails and action links | done | `server/tests/login_pages.rs::k7_k8_k10_k11_...`, `server/tests/login_errors.rs` | action link single use |
| **K11** passkeys (registration + login) | done | `crypto/tests/webauthn.rs` (`registration_challenge_is_bound_to_realm_person_and_time` and the ceremony tests), `crypto/tests/webauthn_errors.rs`, `server/tests/odata_service.rs` | sign-count regression; registration challenge MAC |
| **K12** upstream OIDC broker on routes | done | `server/tests/broker_flow.rs::k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation`, `server/tests/broker_errors.rs` | `state` compare; `nonce` compare |
| **K13** domain verification (DNS TXT) | done | `admin/tests/domain_verification.rs::k13_domain_verification_happy_path_failures_and_cross_org_404`, `admin/tests/domain_errors.rs` | challenge expiry |
| **K14** SCIM PUT/PATCH, groups, ETags | done | `server/tests/scim_service.rs::k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation`, `server/tests/scim_errors.rs` | `If-Match` compare |
| **K15** OData pushed down to PostgreSQL under RLS | done | `store-pg/tests/odata_pushdown_and_outbox.rs::pg_scoped_table_matches_memory_table_and_binds_filter_values`, `::pg_scoped_table_reads_under_the_callers_organization_rls` | caller's organizations in the pushdown's RLS scope |
| **K16** `jc` on PostgreSQL | done | `bins/jc/src/modules/kaisatsu.rs::tests::k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password`, `::k16_postgres_store_runs_the_boot_rls_check_and_persists` | `verify_boot_rls` call in `PgStore::connect` |
| **K17** `--import-dir` watch, symlink refusal | done | `server/tests/cli.rs::k17_import_dir_watch_applies_changes_keeps_last_good_state_on_error_and_refuses_symlinks`, `server/tests/server_errors.rs` | loader symlink refusal |

Defects the finishing run found in the Argon write and fixed (each with a test red before the fix):
`DeletePasskey` without an ownership or admin check; `RegisterPasskey` accepting a client-chosen challenge;
`clientDataJson` instead of `clientDataJSON`; `deny_unknown_fields` on clientDataJSON (browsers add members);
K3 breaking portal-api's managed App client roles (T-2725); userinfo not comparing the pairwise subject;
back-channel logout sending the local user id when the pairwise subject failed; a recovery-code double use
under concurrent submits; `serve` never running the outbox dispatcher; an action link storing the new password
before checking the TOTP code; SCIM PUT/PATCH keeping `emailVerified` on a changed address; a group PATCH with
an unknown op acting as a rename; the broker link confirmation page writing the username unescaped.

## 2. Coverage

`cargo +nightly llvm-cov` over the 10 kaisatsu packages, PostgreSQL tests included, no exclusions
(`evidence/kaisatsu-finish/cov-final.json`).

| Crate | Lines | Functions |
|---|---|---|
| admin | 95.2 % | 96.3 % (519/539) |
| core | 99.6 % | 99.5 % (183/184) |
| crypto | 95.8 % | 84.7 % (166/196) |
| import | 92.8 % | 97.4 % (76/78) |
| login | 92.0 % | 87.6 % (197/225) |
| mcp | 100 % | 100 % (7/7) |
| oidc | 96.7 % | 96.6 % (458/474) |
| server | 91.1 % | 89.8 % (53/59) |
| store-memory | 98.9 % | 99.5 % (432/434) |
| store-pg | 94.1 % | 94.5 % (328/347) |
| **all** | **95.6 %** | **95.1 %** |

The functions left are `map_err`/`ok_or_else` closures behind earlier checks (a state transition already
validated, serialising our own types, a DNS or HTTP client that cannot be built, a database failing
mid-transaction) and `main.rs` signal handling. crypto, login, server and store-pg stay under 95 % alone.

## 3. Limitations (known, not fixed in this run)

- Token exchange with a pairwise `subject_token` fails closed (the subject does not resolve to a person).
- `jc` runs neither the e-mail outbox dispatcher nor the expired-row sweeper; `kaisatsu serve` runs both.
- The People RLS policy shows a person with any membership or group row to every organization-scoped
  transaction; the authorizer's row scope still filters them (part of "People RLS group path-root fallback").
- The K5 logout CSRF token has no expiry of its own; it dies with the SSO session.

## 4. Contract Rows (W1–W12 & COMPAT.md)

| Item | Description | Status | Verification / what is missing |
|---|---|---|---|
| **W1** | Drop-in wire surface: discovery, authorize, PAR, token grants, PKCE, DPoP, refresh rotation, token exchange V2 + `act`, jwt-bearer, federated-jwt, introspection, revocation, userinfo, RP/back-channel logout, CIMD, parameterized scopes, RI mappers, password hash migration | **partial** | `server/tests/oidc_flows.rs`, `server/tests/token_endpoint.rs`, `server/tests/refresh_tokens.rs`, `server/tests/dpop_binding.rs::k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip` (DPoP on Admin, OData, MCP, SCIM + server nonces), `core/tests/claims_mappers.rs::k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id` (pairwise `sub`). Out of scope, later run: CIMD cache refresh. |
| **W1b** | Declarative config apply (JSON/YAML realm files, `check`, `plan`/`apply` with stale-plan refusal, `diff`, `export`, `--prune`, secret substitution, `--import-dir` watch) | **partial** | `import/tests/import_tests.rs`, `import/tests/check_rules.rs`, `server/tests/cli.rs::k17_import_dir_watch_applies_changes_keeps_last_good_state_on_error_and_refuses_symlinks`. Out of scope, later run: re-applying a realm whose `RealmConfig` defaults reference objects of the same file. |
| **W2** | PostgreSQL store with forced RLS, `verify_boot_rls`, `FOR UPDATE SKIP LOCKED` sweeper & outbox, `LISTEN/NOTIFY`, `PgScopedTable` OData pushdown, `jc` on PostgreSQL, `MemoryStore`, shared isolation suite | **partial** | `store-pg/tests/pg_store.rs`, `store-pg/tests/store_contract.rs`, `store-pg/tests/isolation_suite.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs`, `bins/jc/src/modules/kaisatsu.rs::tests::k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password`. Out of scope, later run: People RLS group path-root fallback. |
| **W3** | Tenancy: realms, organizations, verified domains, memberships, group subtrees, composite roles, service-account users, cross-org isolation | **done** | `admin/tests/domain_verification.rs::k13_domain_verification_happy_path_failures_and_cross_org_404`, `server/tests/security_regressions.rs::k2_organization_scope_checks_membership_and_never_emits_foreign_org_claim`, `store-pg/tests/isolation_suite.rs`. |
| **W4** | OData 4.01 for the entity sets via `odata-table` & `PgScopedTable` | **partial** | `server/tests/odata_service.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs::pg_scoped_table_matches_memory_table_and_binds_filter_values` (streamed pages on `$export`). Out of scope, later run: `$allowed` beyond the first 1000 rows. |
| **W5** | One operation registry serving REST, OData, MCP, CLI and `/allowed` | **partial** | `server/tests/odata_service.rs`, `admin/tests/compat_routes.rs`. Out of scope, later run: per-operation CLI subcommands. |
| **W6** | `Authorizer` with `LocalAuthorizer`, 404 on cross-org, `AdminEvent` audit rows, least-privilege `view-clients` | **done** | `core/tests/authz_matrix.rs`, `admin/tests/authz_regressions.rs::k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes`. |
| **W7** | Login state machines, brute force, pages in `en`/`sk`/`cs`/`de`/`hu`, transactional e-mail outbox & action links, WebAuthn passkeys, recovery codes, verify-email | **partial** | `server/tests/login_pages.rs::k1_session_code_replayed_from_second_browser_is_refused`, `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys`, `crypto/tests/webauthn.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs`. Out of scope, later run: per-address brute-force locking behind NAT; the pending TOTP secret unencrypted in the auth session; a branding writer. |
| **W8** | Single sign-on, per-application consent, `acr` step-up, RP logout confirmation, back-channel logout | **done** | `server/tests/oidc_flows.rs`, `server/tests/e2e_protocol_and_security_tests.rs`. |
| **W9** | Library crates + `kaisatsu_server::router(state, mount)` + `jc-router` | **done** | `server/tests/*`, `crates/shared/jc-router/src/lib.rs`. |
| **W10** | Keys & crypto: ES256, RS256, EdDSA, AES-256-GCM under a KEK, rotation, `jsonwebtoken` + `aws-lc-rs`, WebAuthn CBOR/COSE verification | **done** | `crypto/tests/crypto_paths.rs`, `crypto/tests/webauthn.rs`. |
| **W11** | Federation: upstream OIDC broker routes, Kubernetes federated-jwt, SCIM 2.0 Users & Groups with PUT/PATCH | **done** | `server/tests/broker_flow.rs::k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation`, `server/tests/scim_service.rs::k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation`, `server/tests/token_endpoint.rs`. |
| **W12** | `#![forbid(unsafe_code)]`, RFC 9457 slugs, strict redirect URIs, single-use codes, `OutboundGuard` SSRF protection | **done** | `oidc/tests/guard_paths.rs`, `core/tests/errors_branding_defs.rs`, `server/tests/security_regressions.rs`. |
| **COMPAT.md rows 1–46** | Keycloak-compatible admin routes (including `execute-actions-email` queuing outbox mail) | **done** | `admin/tests/compat_routes.rs`, `admin/tests/compat_tests.rs`, `server/tests/login_pages.rs`. |

## 5. Out of Scope (Later Run)

- CIMD cache refresh: out of scope, later run.
- Re-applying a realm whose `RealmConfig` defaults reference objects of the same file: out of scope, later run.
- People RLS group path-root fallback: out of scope, later run.
- Per-address brute-force locking behind NAT: out of scope, later run.
- The pending TOTP secret unencrypted in the auth session: out of scope, later run.
- A branding writer: out of scope, later run.
- Per-operation CLI subcommands: out of scope, later run.
- `$allowed` beyond the first 1000 rows: out of scope, later run.
