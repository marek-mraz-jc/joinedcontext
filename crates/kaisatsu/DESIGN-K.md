# kaisatsu K1–K17 Design & Architecture

## 1. Scope & Decisions (Section 0)

This document records the architecture and state-machine designs for items **K1–K17** closing the open rows of `STATUS.md`.

### Part 1: Security & Sessions (K1–K6)
- **K1 Auth session cookie binding**:
  - `authorize_get` already issues `__Host-wk_auth=<handle>` (or `__Secure-wk_auth=<handle>` when mounted under a path prefix) and stores `handle_hash = sha256_hex(handle)` on `AuthSession`.
  - Every `login-actions` GET and POST (`authenticate`, `first-broker-login`) extracts the `auth` cookie via `Mount::cookie_name("auth")`, hashes its value with SHA-256, and compares it against `auth_session.handle_hash` in constant time (`kaisatsu_crypto::constant_time_str_eq`).
  - Missing or mismatched cookies are refused with the exact expired-session page on GET (`page_id = "expired"`, no CSRF token or details) and `400 Bad Request` (`"login session expired"`) on POST.
- **K2 Organization pinned by `organization:<alias>` scope**:
  - `person_org_memberships` resolves the person's organizations from `user.org_id`, `Membership` rows, and `GroupRepresentation` (`org_id`, `jc_org` attribute, and `/<org>/...` path roots), normalizing both organization IDs and aliases.
  - `organization:<alias>` is honoured only when the person is a member of `<alias>` in the realm. When honoured, the token carries `"organization": "<alias>"` and `jc_org` includes `<alias>`.
  - When a non-member requests `organization:<alias>`: if interactive login has multiple candidate organizations, `LoginState::OrganizationChoice` is shown; otherwise `organization:<alias>` is stripped from the granted scope and no foreign `organization` or `jc_org` claim is emitted.
- **K3 Least-privilege `view-clients`**:
  - In `LocalAuthorizer::check` (`core/src/authz.rs`), `view-clients` is moved from `can_write` to `can_read` on `ObjectTarget::Role` and `ObjectTarget::ClientScope`.
  - In `admin/src/compat.rs`, client role creation and deletion (`compat_create_client_role`, `compat_delete_client_role`) check `ObjectTarget::Role` rather than `ObjectTarget::Application`.
  - The T-2725 exception on `ObjectTarget::Application` (`view-clients` may write only applications with `managed_by == None | Some("joinedcontext")`) remains unchanged.
- **K4 Pairwise subject (`oidc-sha256-pairwise-sub-mapper`)**:
  - Computes `URL_SAFE_NO_PAD.encode(Sha256::digest(sector_identifier || local_sub || salt))` (OIDC Core §8.1).
  - Sector identifier is the host of `mapper.config["sectorIdentifierUri"]`, or the single host of `client.redirect_uris` (falling back to `root_url` / `base_url` / `web_origins` / `client_id` when no redirect URI is configured; refusing when multiple distinct redirect URI hosts exist without `sectorIdentifierUri`).
  - Salt is resolved from `pairwiseSubHashSalt` / `salt` / `secretRef` (supporting `$(env:NAME)` / `$(file:...)` secret references, never logged).
  - Applies across ID token, access token, UserInfo, token introspection, and back-channel logout tokens.
- **K5 RP-Initiated Logout confirmation**:
  - `GET` or `POST /realms/{realm}/protocol/openid-connect/logout` with a valid `id_token_hint` ends the session immediately as before.
  - Without `id_token_hint`, GET (or POST without a confirmation token) renders a localized confirmation page (`en`, `sk`, `cs`, `de`, `hu`) with per-response CSP nonce and a single-use, session-bound CSRF token (`typ = "LogoutConfirm"`, bound to `sso.id` and `jti`).
  - Only a POST with a valid, unconsumed `csrf_token` matching the active SSO session ends the session and dispatches back-channel logout; wrong or reused tokens are refused with `400 Bad Request` and leave the session alive.
- **K6 DPoP on every API & stateless server nonces**:
  - `oidc/src/dpop_guard.rs` provides a unified verifier and middleware used by UserInfo, Admin Compat, Native Admin, OData, MCP, and SCIM.
  - Enforces RFC 9449 §4.3 (`typ == dpop+jwt`, asymmetric `alg`, `htm`, `htu` reconstructed from external `Mount`, `ath`, `cnf.jkt` match in constant time, `iat` window, single-use `jti` recorded in `revoked_token`).
  - Refuses `Bearer` with a `cnf.jkt`-bound token and refuses `DPoP` scheme with an unbound token (`401 Unauthorized` + `WWW-Authenticate: DPoP ...`).
  - Stateless server nonces: `"{slot_hex}.{hmac_b64url}"` where `slot = unix_secs / 60` and `hmac = HMAC-SHA256(kek.derive_subkey("kaisatsu:dpop-nonce:v1"), "{realm_id}:{slot}")`. Current and previous 60s slots are accepted in constant time.
  - Decided: enabled per realm via `attributes["dpop.require.nonce"] == "true"` (default `"true"` for newly created realms via `POST /admin/realms`, `"false"` when importing an existing Keycloak realm unless explicitly set in the file).

### Part 2: Login Completeness (K7–K12)
- **K7 Recovery codes & VerifyEmail**:
  - `LoginState::RecoveryCode` POST verifies the submitted code in constant time against all remaining SHA-256 hashes in the user's `recovery-authn-codes` credential, consumes the matched hash (single use), counts failures in `BruteForceProtector`, and transitions via `LoginInput::VerifyRecoveryCodeOk`.
  - `LoginState::VerifyEmail` queues a verification email in the outbox (carrying both a signed action link and a 6-digit code hashed in `AuthSession`). Completing either the action link (K10) or POSTing the matching code sets `user.email_verified = true`, removes `VERIFY_EMAIL` from `required_actions`, and advances via `LoginInput::CompleteVerifyEmailOk`.
- **K8 Combined password+OTP failure**:
  - When a combined `username` + `password` + `otp` POST from `IdentityFirst` / `Password` fails OTP verification (or replays a used code), `authenticate_page_post` keeps the session on `IdentityFirst` / `Password` (`page_id = "login"`), records the brute-force failure once, never echoes the password, and allows the next valid submission to sign in.
- **K9 Transactional e-mail outbox & dispatcher**:
  - `EmailOutboxRecord` is stored in `kaisatsu.email_outbox` (migration `0002` adds `org_id`, `next_attempt_at`, `last_error`, `payload_enc`) in the same transaction as the state change (`update_user_with_outbox`).
  - Action links are never stored in clear in `doc`: `doc` stores only `token_jti` and the action parameters, and `OutboxDispatcher` mints the signed link at send time (or decrypts `payload_enc` under the KEK).
  - `claim_pending_outbox` uses `FOR UPDATE SKIP LOCKED` batches (<= 50), retries with exponential backoff, and marks rows `failed` after `max_attempts`.
- **K10 Mails & action-token route**:
  - `Person/SendRequiredActions` and `PUT .../execute-actions-email` validate `client_id` and `redirect_uri`, cap `lifespan` (default 900s), and atomically update the user and enqueue the outbox row.
  - `POST /realms/{realm}/login-actions/reset-credentials` runs dummy Argon2id work for unknown users so known and unknown users share the same response page and timing band.
  - `GET/POST /realms/{realm}/login-actions/action-token` consumes the single-use `SignedActionToken` and executes `VERIFY_EMAIL`, `UPDATE_PASSWORD`, `CONFIGURE_TOTP`, `TERMS_AND_CONDITIONS`, or `webauthn-register`.
- **K11 WebAuthn Passkeys**:
  - `crypto/src/webauthn.rs` implements a bounded, panic-free CBOR decoder (`MAX_CBOR_BYTES = 65536`, `MAX_CBOR_DEPTH = 8`, `MAX_CBOR_ITEMS = 1024`, definite lengths only), `COSE_Key` parser (`ES256` P-256, `EdDSA` Ed25519, `RS256` 2048–8192 bit RSA), signature verification via `aws-lc-rs`, and WebAuthn L3 §7.1 / §7.2 verifiers.
  - Stored in `kaisatsu.webauthn_credential` (migration `0002`) with random per-user `user_handle`, realm-unique `credential_id`, `sign_count` regression refusal, and `backup_eligible` / `backup_state` tracking.
- **K12 Upstream OIDC broker routes**:
  - `GET /realms/{realm}/broker/{alias}/login` binds state, nonce, and PKCE S256 verifier to `AuthSession`.
  - `GET /realms/{realm}/broker/{alias}/endpoint` verifies state and auth cookie, exchanges the code with `code_verifier` via `OutboundGuard`, verifies the upstream ID token against upstream JWKS, signs in linked users (`kaisatsu.identity_provider_link`), or requires POST + CSRF + local password re-authentication at `POST /realms/{realm}/broker/{alias}/first-broker-login` before linking an existing verified-email account.

### Part 3: Features & Wiring (K13–K17)
- **K13 Domain verification**:
  - `OrganizationDomain/RequestVerification` generates `kaisatsu-verify=<random>` for `_kaisatsu-challenge.<domain>`, storing only its SHA-256 hash and expiry.
  - `OrganizationDomain/Verify` resolves TXT records via `DnsResolver` (`HickoryDnsResolver` in production, refusing cluster-internal suffixes), compares hashes in constant time, and marks `verified = true`.
- **K14 SCIM 2.0 PUT/PATCH & Groups**:
  - Full `PUT` and `PATCH` (`add`, `remove`, `replace` with `valuePath` filters) on `/Users/{id}` and `/Groups/{id}`, `/Groups` CRUD with `members`, weak ETags + `If-Match`, `scimType` error responses, `deny_unknown_fields`, 1 MiB / 100-operation caps, and session termination on `active = false`.
- **K15 OData pushed down to PostgreSQL (`PgScopedTable`)**:
  - `store-pg/src/odata_pg.rs` implements `odata_table::Table` inside `PgStore`'s RLS transaction, translating `Plan`/`Cond`/`SortKey` into parameterized `QueryBuilder` SQL with bounded keyset paging fallback.
- **K16 `jc` on PostgreSQL**:
  - `[kaisatsu]` supports `database_url_file` and `db_password_file` (refusing inline passwords in config URLs), connecting `PgStore` with `verify_boot_rls` when set and warning once when falling back to `MemoryStore`.
- **K17 `--import-dir` watch**:
  - `kaisatsu serve --import-dir DIR --import-watch SECS` polls SHA-256 file snapshots, refuses symlinks escaping `DIR`, applies via `create_plan`/`apply_plan` (`NoDelete`), and preserves the last good state on error.
