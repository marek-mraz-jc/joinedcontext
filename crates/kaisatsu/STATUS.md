# kaisatsu Implementation & Test Status

Updated 2026-11-08 for items K1–K17 on branch `tests-kaisatsu-1`. Every K1–K17 edit and test in
this run is **not verified (not compiled)** in this environment.

## Contract Rows (W1–W12 & COMPAT.md)

| Item | Description | Status | Verification / what is missing |
|---|---|---|---|
| **W1** | Drop-in wire surface: discovery, authorize, PAR, token grants, PKCE, DPoP, refresh rotation, token exchange V2 + `act`, jwt-bearer, federated-jwt, introspection, revocation, userinfo, RP/back-channel logout, CIMD, parameterized scopes, RI mappers, password hash migration | **partial (not verified, not compiled)** | `server/tests/oidc_flows.rs`, `server/tests/token_endpoint.rs`, `server/tests/refresh_tokens.rs`, `server/tests/dpop_binding.rs::k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip` (DPoP on Admin, OData, MCP, SCIM + server nonces), `core/tests/claims_mappers.rs::k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id` (pairwise `sub`). Out of scope, later run: CIMD cache refresh. |
| **W1b** | Declarative config apply (JSON/YAML realm files, `check`, `plan`/`apply` with stale-plan refusal, `diff`, `export`, `--prune`, secret substitution, `--import-dir` watch) | **partial (not verified, not compiled)** | `import/tests/import_tests.rs`, `import/tests/check_rules.rs`, `server/tests/cli.rs::k17_import_dir_watch_applies_changes_keeps_last_good_state_on_error_and_refuses_symlinks`. Out of scope, later run: re-applying a realm whose `RealmConfig` defaults reference objects of the same file. |
| **W2** | PostgreSQL store with forced RLS, `verify_boot_rls`, `FOR UPDATE SKIP LOCKED` sweeper & outbox, `LISTEN/NOTIFY`, `PgScopedTable` OData pushdown, `jc` on PostgreSQL, `MemoryStore`, shared isolation suite | **partial (not verified, not compiled)** | `store-pg/tests/pg_store.rs`, `store-pg/tests/store_contract.rs`, `store-pg/tests/isolation_suite.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs`, `bins/jc/src/modules/kaisatsu.rs::tests::k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password`. Out of scope, later run: People RLS group path-root fallback. |
| **W3** | Tenancy: realms, organizations, verified domains, memberships, group subtrees, composite roles, service-account users, cross-org isolation | **done (not verified, not compiled)** | `admin/tests/domain_verification.rs::k13_domain_verification_happy_path_failures_and_cross_org_404`, `server/tests/security_regressions.rs::k2_organization_scope_checks_membership_and_never_emits_foreign_org_claim`, `store-pg/tests/isolation_suite.rs`. |
| **W4** | OData 4.01 for the entity sets via `odata-table` & `PgScopedTable` | **partial (not verified, not compiled)** | `server/tests/odata_service.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs::pg_scoped_table_matches_memory_table_and_binds_filter_values` (streamed pages on `$export`). Out of scope, later run: `$allowed` beyond the first 1000 rows. |
| **W5** | One operation registry serving REST, OData, MCP, CLI and `/allowed` | **partial (not verified, not compiled)** | `server/tests/odata_service.rs`, `admin/tests/compat_routes.rs`. Out of scope, later run: per-operation CLI subcommands. |
| **W6** | `Authorizer` with `LocalAuthorizer`, 404 on cross-org, `AdminEvent` audit rows, least-privilege `view-clients` | **done (not verified, not compiled)** | `core/tests/authz_matrix.rs`, `admin/tests/authz_regressions.rs::k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes`. |
| **W7** | Login state machines, brute force, pages in `en`/`sk`/`cs`/`de`/`hu`, transactional e-mail outbox & action links, WebAuthn passkeys, recovery codes, verify-email | **partial (not verified, not compiled)** | `server/tests/login_pages.rs::k1_session_code_replayed_from_second_browser_is_refused`, `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys`, `crypto/tests/webauthn.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs`. Out of scope, later run: per-address brute-force locking behind NAT; the pending TOTP secret unencrypted in the auth session; a branding writer. |
| **W8** | Single sign-on, per-application consent, `acr` step-up, RP logout confirmation, back-channel logout | **done (not verified, not compiled)** | `server/tests/oidc_flows.rs`, `server/tests/e2e_protocol_and_security_tests.rs`. |
| **W9** | Library crates + `kaisatsu_server::router(state, mount)` + `jc-router` | **done (not verified, not compiled)** | `server/tests/*`, `crates/shared/jc-router/src/lib.rs`. |
| **W10** | Keys & crypto: ES256, RS256, EdDSA, AES-256-GCM under a KEK, rotation, `jsonwebtoken` + `aws-lc-rs`, WebAuthn CBOR/COSE verification | **done (not verified, not compiled)** | `crypto/tests/crypto_paths.rs`, `crypto/tests/webauthn.rs`. |
| **W11** | Federation: upstream OIDC broker routes, Kubernetes federated-jwt, SCIM 2.0 Users & Groups with PUT/PATCH | **done (not verified, not compiled)** | `server/tests/broker_flow.rs::k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation`, `server/tests/scim_service.rs::k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation`, `server/tests/token_endpoint.rs`. |
| **W12** | `#![forbid(unsafe_code)]`, RFC 9457 slugs, strict redirect URIs, single-use codes, `OutboundGuard` SSRF protection | **done (not verified, not compiled)** | `oidc/tests/guard_paths.rs`, `core/tests/errors_branding_defs.rs`, `server/tests/security_regressions.rs`. |
| **COMPAT.md rows 1–46** | Keycloak-compatible admin routes (including `execute-actions-email` queuing outbox mail) | **done (not verified, not compiled)** | `admin/tests/compat_routes.rs`, `admin/tests/compat_tests.rs`, `server/tests/login_pages.rs`. |

## K1–K17 Item Status

| Item | Summary | Status | Test Names |
|---|---|---|---|
| **K1** | Auth session cookie checked in constant time against `session_code` | **done (not verified, not compiled)** | `server/tests/login_pages.rs::k1_session_code_replayed_from_second_browser_is_refused` |
| **K2** | `organization:<alias>` scope verified against person's memberships | **done (not verified, not compiled)** | `server/tests/security_regressions.rs::k2_organization_scope_checks_membership_and_never_emits_foreign_org_claim` |
| **K3** | `view-clients` restricted to read-only on roles and client scopes | **done (not verified, not compiled)** | `core/tests/authz_matrix.rs::realm_management_roles`, `admin/tests/authz_regressions.rs::k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes` |
| **K4** | Pairwise subject mapper (`oidc-sha256-pairwise-sub-mapper`) | **done (not verified, not compiled)** | `core/tests/claims_mappers.rs::k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id` |
| **K5** | Logout confirmation page + single-use session-bound CSRF token when `id_token_hint` is absent | **done (not verified, not compiled)** | `server/tests/oidc_flows.rs::rp_initiated_logout_and_backchannel_dispatch` |
| **K6** | DPoP enforced on Admin, OData, MCP, SCIM and stateless HMAC server nonces | **done (not verified, not compiled)** | `server/tests/dpop_binding.rs::k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip` |
| **K7** | `RecoveryCode` and `VerifyEmail` POST handlers | **done (not verified, not compiled)** | `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys` |
| **K8** | Failed combined password+OTP POST keeps the password step | **done (not verified, not compiled)** | `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys` |
| **K9** | Transactional `email_outbox`, `FOR UPDATE SKIP LOCKED` dispatcher, `MailSender` & `RecordingSender` | **done (not verified, not compiled)** | `store-pg/tests/odata_pushdown_and_outbox.rs::memory_store_outbox_and_webauthn_contract`, `store-pg/tests/odata_pushdown_and_outbox.rs::pg_store_outbox_and_webauthn_contract` |
| **K10** | `SendRequiredActions`, `execute-actions-email`, `reset-credentials` POST, and `/login-actions/action-token` route | **done (not verified, not compiled)** | `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys` |
| **K11** | WebAuthn passkeys (W3C WebAuthn L3 §7.1 & §7.2, bounded CBOR/COSE decoder, `aws-lc-rs` verification) | **done (not verified, not compiled)** | `crypto/tests/webauthn.rs::cbor_decoder_enforces_size_depth_and_definite_length_bounds`, `crypto/tests/webauthn.rs::registration_and_authentication_ceremonies_verify_and_reject_every_violation`, `crypto/tests/webauthn.rs::ed25519_and_rsa256_cose_keys_verify_signatures`, `server/tests/login_pages.rs::k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys` |
| **K12** | Upstream OIDC broker routes (`/realms/{r}/broker/{alias}/{login,endpoint}`) with PKCE, nonce, and link confirmation | **done (not verified, not compiled)** | `server/tests/broker_flow.rs::k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation` |
| **K13** | Organization domain DNS TXT verification (`RequestVerification` & `Verify` via `DnsResolver`) | **done (not verified, not compiled)** | `admin/tests/domain_verification.rs::k13_domain_verification_happy_path_failures_and_cross_org_404` |
| **K14** | SCIM 2.0 PUT/PATCH, `/Groups` lifecycle & membership, `If-Match` ETags, `scimType` errors, deactivation session revocation | **done (not verified, not compiled)** | `server/tests/scim_service.rs::k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation` |
| **K15** | OData pushed down to PostgreSQL via `PgScopedTable` inside the RLS transaction | **done (not verified, not compiled)** | `store-pg/tests/odata_pushdown_and_outbox.rs::pg_scoped_table_matches_memory_table_and_binds_filter_values` |
| **K16** | `jc` binary wired to `PgStore` via `database_url_file` with `verify_boot_rls` | **done (not verified, not compiled)** | `bins/jc/src/modules/kaisatsu.rs::tests::k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password` |
| **K17** | `serve --import-dir DIR --import-watch SECS` polling watcher with symlink refusal | **done (not verified, not compiled)** | `server/tests/cli.rs::k17_import_dir_watch_applies_changes_keeps_last_good_state_on_error_and_refuses_symlinks` |

## Out of Scope (Later Run)

- CIMD cache refresh: out of scope, later run.
- Re-applying a realm whose `RealmConfig` defaults reference objects of the same file: out of scope, later run.
- People RLS group path-root fallback: out of scope, later run.
- Per-address brute-force locking behind NAT: out of scope, later run.
- The pending TOTP secret unencrypted in the auth session: out of scope, later run.
- A branding writer: out of scope, later run.
- Per-operation CLI subcommands: out of scope, later run.
- `$allowed` beyond the first 1000 rows: out of scope, later run.
