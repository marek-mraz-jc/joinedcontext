# kaisatsu Architecture & Security Decisions (K1–K17)

## 1. THINK-CHECKLIST Decisions

| Section | Item | Decision & Rationale | Verification Test |
|---|---|---|---|
| **1. Domains, tenants, isolation** | Tenant chain & verified token scope | Every read/write takes `realm_id` from the mount/path and organization scope from the verified token's `jc_org` / memberships (`extract_admin_ctx`), never from a request body field. | `store-pg/tests/isolation_suite.rs`, `admin/tests/authz_regressions.rs` |
| **1. Domains, tenants, isolation** | Outside-scope objects answer 404 | `check_decision` maps `Decision { visible: false, .. }` to `404 NotFound`; `visible_person`, `find_visible_domain`, SCIM, OData, and passkey/outbox operations filter by `ctx.allowed_orgs()` first so hidden and absent IDs return identical 404 responses. | `admin/tests/domain_verification.rs::k13_domain_verification_happy_path_failures_and_cross_org_404`, `server/tests/scim_service.rs` |
| **1. Domains, tenants, isolation** | Cookies (`SameSite`, `Secure`, `HttpOnly`, `__Host-`) | `Mount::cookie_name` uses `__Host-wk_{auth,sso}` at root and `__Secure-wk_{auth,sso}` with realm path when prefixed; K1 verifies the auth cookie in constant time on every login-actions step. | `server/tests/login_pages.rs::k1_session_code_replayed_from_second_browser_is_refused` |
| **1. Domains, tenants, isolation** | CSRF on cookie-authenticated writes | Login POSTs verify `csrf_token` bound to `AuthSession`; logout without `id_token_hint` verifies a single-use session-bound `logout_csrf_token` (K5); broker link confirmation POST verifies `csrf_token` + auth cookie (K12). | `server/tests/oidc_flows.rs::rp_initiated_logout_and_backchannel_dispatch`, `server/tests/broker_flow.rs` |
| **1. Domains, tenants, isolation** | Postgres forced RLS & boot check | Migration `0002_k_items.sql` enables and forces RLS on `webauthn_credential`, `idp_links`, and `email_outbox`; `verify_boot_rls` refuses startup if any table in `kaisatsu` lacks `FORCE ROW LEVEL SECURITY`. | `store-pg/tests/isolation_suite.rs::migration_ddl_enforces_rls_and_leakproof_indexes_on_every_table` |
| **1. Domains, tenants, isolation** | Shared process (`jc`) isolation | `KaisatsuState` keeps `kek` and `PgPool` private; `jc` reads `database_url_file` and `db_password_file` from the secrets mount (K16). | `bins/jc/src/modules/kaisatsu.rs::tests::k16_config_parses_database_url_file_refuses_unknown_fields_and_refuses_inline_password` |
| **2. Identity & authorization** | Identity only from verified tokens & DPoP | `verify_jwt` pins the algorithm from the stored key and enforces `iss`, `aud`, `exp`, `nbf`, and `jti` revocation; `verify_resource_dpop` enforces RFC 9449 `cnf.jkt` and server nonces across UserInfo, Admin, OData, MCP, and SCIM (K6). | `server/tests/dpop_binding.rs::k6_dpop_enforced_across_admin_odata_mcp_and_scim_and_server_nonce_round_trip` |
| **2. Identity & authorization** | Deny by default & least privilege | `view-clients` is restricted to read-only on `Role` and `ClientScope` while preserving the narrow T-2725 `managed-by: joinedcontext` application exception (K3). | `core/tests/authz_matrix.rs`, `admin/tests/authz_regressions.rs::k3_view_clients_cannot_write_roles_or_client_scopes_on_compat_routes` |
| **3. Input, injection, output** | `deny_unknown_fields` & bounds | Added `#[serde(deny_unknown_fields)]` on SCIM User/Group/PatchOp payloads, WebAuthn client responses, broker queries/forms, and action-token queries; CBOR capped at 64 KiB and depth 8; SCIM capped at 1 MiB and 100 operations. | `crypto/tests/webauthn.rs::cbor_decoder_enforces_size_depth_and_definite_length_bounds`, `server/tests/scim_service.rs::k14_scim_put_patch_groups_etags_errors_and_deactivation_session_revocation` |
| **3. Input, injection, output** | Parameterized SQL (`PgScopedTable`) | `PgScopedTable` binds every filter literal, JSON path array, `LIMIT`, and `OFFSET` via `QueryBuilder::push_bind`; only validated schema properties and static subqueries enter SQL text (K15). | `store-pg/tests/odata_pushdown_and_outbox.rs::pg_scoped_table_matches_memory_table_and_binds_filter_values` |
| **3. Input, injection, output** | Path canonicalization & symlink refusal | `load_realm_files_from_path` and `compute_import_dir_digest` canonicalize every entry in `--import-dir` and refuse symlinks pointing outside the directory (K17). | `server/tests/cli.rs::k17_import_dir_watch_applies_changes_keeps_last_good_state_on_error_and_refuses_symlinks` |
| **4. Outbound calls (SSRF)** | `OutboundGuard` & DNS suffix policy | Broker token exchange and JWKS fetches use `OutboundGuard`; `HickoryDnsResolver` refuses `.cluster.local`, `.svc`, `.local`, `.internal`, and `localhost` before resolving TXT records (K12, K13). | `server/tests/broker_flow.rs`, `admin/tests/domain_verification.rs` |
| **5. Secrets & crypto** | Platform crypto & no secrets in outbox/logs | WebAuthn uses `aws-lc-rs` (`ECDSA_P256_SHA256_ASN1`, `ED25519`, `RSA_PKCS1_2048_8192_SHA256`); `EmailOutboxRecord` stores only `action_jti` and mints the signed link at dispatch time; domain challenges are stored as SHA-256 hashes; pairwise salts are redacted on export. | `crypto/tests/webauthn.rs`, `store-pg/tests/odata_pushdown_and_outbox.rs` |
| **6. Supply chain** | Existing `Cargo.lock` dependencies only | Zero new crates added; CBOR/COSE decoded in `kaisatsu_crypto::webauthn`, DNS TXT via `hickory-resolver` 0.26.3, file watching via polling in `kaisatsu-server`. | Workspace `Cargo.toml` |
| **7. Resources, memory, DoS** | Bounded caches, batches, and scans | Outbox claim batch <= 50; OData keyset fallback scan <= 5000 rows; CBOR <= 64 KiB / 1024 items; SCIM <= 1 MiB / 100 ops. | `crypto/tests/webauthn.rs`, `server/tests/scim_service.rs` |
| **8. Scalability & performance** | OData SQL pushdown & stateless DPoP nonces | OData queries push filters/order/limit/count down to PostgreSQL inside the RLS transaction; DPoP nonces are stateless HMACs over 60s time slots. | `store-pg/tests/odata_pushdown_and_outbox.rs`, `server/tests/dpop_binding.rs` |
| **9. Failure, consistency, data safety** | Transactional outbox & optimistic concurrency | `update_user_with_outbox` writes user and outbox row in one transaction; `claim_outbox_batch` uses `FOR UPDATE SKIP LOCKED`; SCIM supports `If-Match` weak ETags; migration `0002_k_items.sql` is forward-only. | `store-pg/tests/odata_pushdown_and_outbox.rs` |
| **10. Compatibility & contracts** | Drop-in Keycloak compat & OData 4.01 | Every new operation (`RequestVerification`, `Verify`, `RegisterPasskey`, `DeletePasskey`, `Retry`) is registered in `OperationRegistry` and exposed on OData/REST/MCP; new entity sets (`WebAuthnCredentials`, `EmailOutbox`, `IdentityProviderLinks`) are in `kaisatsu_odata_service`. | `server/tests/odata_service.rs`, `admin/tests/domain_verification.rs` |
| **11. Personal data (GDPR)** | Pairwise `sub` & write-only credentials | `oidc-sha256-pairwise-sub-mapper` prevents cross-sector subject correlation (K4); secret/credential fields are omitted from OData entity sets. | `core/tests/claims_mappers.rs::k4_pairwise_subject_mapper_differs_across_sectors_and_salts_and_hides_user_id` |
| **12. MCP & AI surfaces** | Shared executor & authorization | MCP `tools/list` and `tools/call` run through `OperationRegistry` and `kaisatsu_admin::execute` behind the same DPoP and `Authorizer` checks. | `server/tests/dpop_binding.rs` |
| **13. Observability & operations** | Audit events without secrets | Every admin, SCIM, and domain verification write records an `AdminEvent` with secret fields stripped. | `admin/tests/domain_verification.rs`, `server/tests/scim_service.rs` |
| **14. Tests & proof** | Red-proven regression tests | Every K1–K17 item has unit/integration tests; security items K1–K6 document what failed before the fix. | All test files in `crates/kaisatsu/*/tests/` |
| **15. UI** | Localized Askama templates with CSP nonces | Logout confirmation, verify-email, recovery-code, passkey, and broker link confirmation pages use per-response CSP nonces, `frame-ancestors 'none'`, and i18n bundles across `en`/`sk`/`cs`/`de`/`hu`. | `server/tests/login_pages.rs`, `server/tests/oidc_flows.rs` |

## 2. SECURITY-THINK Decisions (Sections A, B, C, D, F, G, I)

- **Section A (Trust boundaries & request path)**:
  - `jc-router` strips and resolves `X-Forwarded-*` only from configured `trusted_proxies`; `with_dpop_req_info` records the external HTTP method and path so `verify_resource_dpop` checks `htm` and `htu` against the external mount URL across Admin, OData, MCP, and SCIM (`admin/src/compat.rs`, `oidc/src/dpop_guard.rs`).
- **Section B (Identity & tokens)**:
  - JWT `iss`, `aud`, `exp`, `nbf`, `typ`, and `jti` revocation are verified on every token; DPoP proofs (`dpop+jwt`) require asymmetric algorithms (`ES256`, `RS256`, `EdDSA`), `ath` matching the access token hash, `jkt` matching `cnf.jkt`, single-use `jti` in `revoked_token`, and stateless HMAC server nonces when `dpop.require.nonce` is enabled (`oidc/src/dpop.rs`, `oidc/src/dpop_guard.rs`).
  - Auth session cookies (`__Host-wk_auth` / `__Secure-wk_auth`) are verified in constant time against `auth_session.handle_hash` on every login-actions GET/POST and broker callback (K1).
  - Logout without `id_token_hint` requires a POST carrying a single-use, session-bound CSRF token (K5).
- **Section C (Authorization & tenancy)**:
  - `organization:<alias>` scope is checked against the person's memberships (`user_is_member_of_org`) before pinning `active_org` onto an `AuthSession`, `SsoSession`, or token claim (K2).
  - `view-clients` is read-only on `Role` and `ClientScope` and cannot create/delete client roles or client scopes (K3).
  - Cross-organization lookups on People, Groups, Applications, Domains, WebAuthnCredentials, and EmailOutbox return `404 NotFound` (`visible: false`).
- **Section D (Injection & parsing of untrusted input)**:
  - `PgScopedTable` uses `sqlx::QueryBuilder::push_bind` for all filter literals and JSON paths, preventing SQL injection in OData `$filter` (K15).
  - `kaisatsu_crypto::webauthn::decode_cbor` enforces definite lengths, depth <= 8, items <= 1024, and size <= 64 KiB using checked arithmetic (`checked_add`, `.get(..)`).
  - SCIM filter and `valuePath` parsers enforce `MAX_SCIM_FILTER_DEPTH` (16), `MAX_SCIM_BODY_BYTES` (1 MiB), and `MAX_SCIM_PATCH_OPS` (100).
- **Section F (Secrets & crypto)**:
  - `EmailOutboxRecord` stores `action_jti` rather than a signed JWT token; `OutboxDispatcher` mints the signed link at send time (K9).
  - Organization domain verification challenges are stored as `sha256_hex` digests (`challenge_hash`) and compared in constant time (K13).
  - Pairwise subject mapper salts are redacted to `$(PAIRWISE_SALT_<CLIENT>)` on realm export (K4).
- **Section G (Denial of service & resource exhaustion)**:
  - All parsers, batches, and fallback scans have explicit byte and item caps (`MAX_CBOR_BYTES`, `MAX_SCIM_BODY_BYTES`, `MAX_OUTBOX_BATCH`, `MAX_FALLBACK_SCAN_ROWS`).
  - Failed combined password+OTP submissions record brute-force failures and never advance session state (K8).
- **Section I (Personal data & AI surfaces)**:
  - Pairwise subject identifiers (`oidc-sha256-pairwise-sub-mapper`) hash `sector || '\0' || sub || '\0' || salt` so relying parties in different sectors cannot correlate users (K4).
  - WebAuthn `user.id` (userHandle) is a random 256-bit opaque handle stored in `webauthn.user.handle`, never the user's UUID or username (K11).

## 3. Explicit Run Decisions ("Decided (model, owner may override)")

1. **K6 Realm DPoP Nonce Default (`dpop.require.nonce`)**: Controlled by `realm.attributes["dpop.require.nonce"] == "true"`. Defaults to off (`"false"`) for imported Keycloak realms so existing drop-in OIDC clients that do not expect an initial `400 use_dpop_nonce` challenge keep working unchanged, while any realm setting `dpop.require.nonce: "true"` enforces stateless HMAC nonces at both the AS token endpoint (400 + `DPoP-Nonce`) and all RS APIs (401 + `WWW-Authenticate: DPoP error="use_dpop_nonce"` + `DPoP-Nonce`).
2. **K7 VerifyEmail Step Completion**: Supports both completion paths: (a) clicking the signed single-use action link sent via the K9 outbox (`/realms/{realm}/login-actions/action-token`), and (b) submitting the verification code on the in-flight `VerifyEmail` login page.
3. **K9 Outbox Secret Avoidance**: `kaisatsu.email_outbox` stores `action_jti` and the action metadata (`actions`, `client_id`, `redirect_uri`, `lifespan_secs`), never a signed JWT or plaintext secret; `OutboxDispatcher` mints the signed JWT action link at send time using the realm's active signing key.

## 4. Decisions of the finishing run (kaisatsu-finish, 2026-11-08, owner may override)

1. **Action link lifespan cap stays 12 h.** 00-task.md asks for short links; `execute-actions-email` callers
   (Keycloak admin API, the Portal) pass `lifespan` up to 43 200 s and expect it honoured. The cap is 12 h;
   every link is single use (`action_jti` revoked on first POST, red-proven).
2. **Passkey registration challenge is server-issued and MAC'd.** `RegisterPasskey` step 1 returns
   `challenge = base64url(16 random bytes || 8-byte expiry || HMAC-SHA256)` under
   `Kek::mac(REG_CHALLENGE_PURPOSE, realm || 0 || person || 0 || body)`, valid 300 s; step 2 verifies the
   MAC and expiry before the single-use mark. A client-chosen challenge is refused (400).
3. **K3 client roles keep T-2725.** `view-clients` alone cannot write roles; a role write on a client is also
   allowed when the `Application` Update decision allows it (portal-api writing `managed-by: joinedcontext`
   App client roles through fine-grained permissions). Roles of any other client answer 404.
4. **Outbox sender in `serve`.** `kaisatsu serve --smtp-password-file F` runs `OutboxDispatcher` every
   `--outbox-interval-secs` (default 10) with `RealmSmtpSender` (each realm's `smtpServer`, the password
   from the mount). Without the file, `serve` logs one warning and queued mails stay pending. `jc` does
   not run the dispatcher yet (STATUS limitations).
5. **SCIM never carries proof of an address.** A SCIM create, PUT or PATCH that sets or changes `emails`
   leaves the person `emailVerified=false`, because account linking trusts a verified address.
6. **Userinfo checks the pairwise subject.** A token's `sub` must equal the subject computed for its
   client (constant time), or userinfo answers 401; back-channel logout skips a client whose pairwise
   subject cannot be computed instead of sending the local user id.
