# shako Decisions (`THINK-CHECKLIST.md`, `SECURITY-THINK.md`, Better-Over-Faithful, and gix API Notes)

## 1. `THINK-CHECKLIST.md` Decisions

1. **Domains, tenants, isolation (`THINK-CHECKLIST 1`)**:
   - **Decision**: Scope every read and write strictly by `(instance_id, org_id)` resolved from the verified token and the mount, never from forged `x-*` headers or URL segments (`shako-api/src/state.rs::authenticate_request`). Objects outside the caller's scope or without `Action::Read` return `404 Not Found` (never `403`) with identical error bodies and store+authz execution paths (`shako-api/src/state.rs::resolve_and_authorize_repo`). Postgres uses schema `shako`, request role `shako_app` (no `BYPASSRLS`, no `SUPERUSER`, owns 0 tables), `FORCE ROW LEVEL SECURITY` on all 15 tables with `SET LOCAL` (`set_config`), and leakproof `uuid_eq`/`texteq` btree index `idx_shako_repo_instance_owner_name`.
   - **Tests**: `shako-store-memory/tests/isolation.rs`, `shako-store-pg/tests/isolation.rs`, `shako-server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
2. **Identity and authorization (`THINK-CHECKLIST 2`)**:
   - **Decision**: Verify kaisatsu JWTs against cached JWKS (bounded 64 keys, 10s refresh cooldown on unknown `kid`, `aud` claim explicitly required and checked for `"shako"` or `"container_registry"`, alg pinned from JWK) or SHA-256 hashed static service tokens in constant time (`shako-core/src/auth.rs::TokenVerifier`). One `Authorizer` check per operation shared across REST, OData, MCP, and CLI (`shako-api/src/odata.rs::exec_odata_operation`).
   - **Tests**: `shako-core/src/auth.rs::tests::token_verifier_checks_audience_expiry_and_unknown_kid_refetch`, `shako-server/tests/e2e.rs`.
3. **Input, injection, output (`THINK-CHECKLIST 3`)**:
   - **Decision**: `#[serde(deny_unknown_fields)]` on every request struct; parameterised SQL only (`sqlx::query` with `.bind()`); OData `$filter` parsed into an AST via `odata-url` + `odata-table`; repository disk paths derived solely from `(InstanceId, RepoId)` UUIDs (`shako-core/src/validate.rs::repo_disk_path`); Markdown rendered through `pulldown-cmark` with raw HTML events converted to escaped text and `javascript:`/`data:` URLs neutralized; per-response random CSP nonce without `unsafe-inline` script (`shako-web/src/lib.rs`, `shako-server/src/lib.rs`).
   - **Tests**: `shako-core/src/validate.rs::tests`, `shako-web/src/lib.rs::tests`, `shako-server/tests/e2e.rs`.
4. **Outbound calls / SSRF (`THINK-CHECKLIST 4`)**:
   - **Decision**: All outbound calls (push mirrors and webhooks) pass through `shako-core::outbound::OutboundGuard`: `https` only (except configured in-cluster hosts), allow-list, no userinfo in URLs, DNS resolved once and checked against private/loopback/link-local/ULA/CGNAT/NAT64/6to4/IPv4-mapped/cloud-metadata (`169.254.169.254`), pinned on `reqwest::ClientBuilder::resolve`, redirects refused, 5s timeout, 1 MiB response cap.
   - **Tests**: `shako-core/src/outbound.rs::tests::outbound_guard_blocks_metadata_private_ranges_redirects_and_large_bodies`.
5. **Secrets and crypto (`THINK-CHECKLIST 5`)**:
   - **Decision**: Secrets loaded from files in the secrets mount or resolved by `secretRef` via `SecretStore`; constant-time comparisons via `subtle::ConstantTimeEq` and `hmac::Mac::verify_slice`; ES256 (P-256) signing for registry tokens and cosign image signatures; secret values never returned in REST, OData, or error strings.
   - **Tests**: `shako-core/src/auth.rs::tests`, `shako-server/tests/e2e.rs`.
6. **Supply chain and vulnerabilities (`THINK-CHECKLIST 6`)**:
   - **Decision**: All non-gix dependencies come from the monorepo `Cargo.lock`; the `gix` 0.90 / `gix-pack` 0.77 family was published 2026-10-22 (17 days old, past both the 2-day and 7-day cooldowns). No `git2`, no `openssl`, no `rsa` crate in `shako`.
   - **Tests**: Workspace Cargo manifest resolution.
7. **Resources, memory, denial of service (`THINK-CHECKLIST 7`)**:
   - **Decision**: One 192 MiB memory-weighted semaphore (`shako-core/src/quota.rs::MemoryBudget`) governs concurrent pack, archive, and merge work, answering `503 Service Unavailable` with `Retry-After: 5` when exhausted; pkt-line capped at 65520 bytes; push capped at 512 MiB; single object capped at 100 MiB; delta depth capped at 50; refs per push capped at 1,000; ODB decoded object cache capped at 2 MiB per handle; response bodies streamed in 64 KiB chunks over a bounded `mpsc::channel(4)`.
   - **Tests**: `shako-core/src/quota.rs::tests`, `shako-git/src/pktline.rs::tests`, `shako-git/src/lib.rs::tests`.
8. **Scalability and performance (`THINK-CHECKLIST 8`)**:
   - **Decision**: In-process `gix` reads and tree merges avoid per-request subprocess creation and temporary worktrees; Postgres queries use `(instance_id, owner, name)` leakproof btree index; OData `Commits` walks are bounded at 10,000 commits and require `RepoId`.
   - **Tests**: `shako-store-pg/tests/isolation.rs`, `shako-server/tests/e2e.rs`.
9. **Failure, consistency, data safety (`THINK-CHECKLIST 9`)**:
   - **Decision**: Incoming packs stream into `{repo}.git/objects/incoming-<uuid>/` quarantine with an RAII drop guard (`QuarantineDir`) that deletes the quarantine on any error or policy refusal; per-repo write locks (`RepoLocks`) serialize pushes, API commits, merges, and compactions; outbox events carry idempotency keys; nightly backup bundles restore cleanly.
   - **Tests**: `shako-git/src/lib.rs::tests`, `shako-server/tests/e2e.rs`.
10. **Compatibility and contracts (`THINK-CHECKLIST 10`)**:
    - **Decision**: All 39 Gitea rows are documented in `COMPAT.md` (rows 1–5, 12–39 served with Gitea's exact field names and status quirks; rows 6–11 answering 404 with Gitea error JSON until S16); every operation is also exposed via OData 4.01 (`/odata/`), MCP (`jc_forge_*`), and CLI.
    - **Tests**: `shako-server/tests/e2e.rs`.
11. **Personal data and governance (`THINK-CHECKLIST 11`)**:
    - **Decision**: `shako` stores no user accounts, passwords, or sessions table; commit author/committer names and emails are Git commit metadata only, scoped by forced RLS per organization.
    - **Tests**: `shako-store-pg/migrations/0001_init.sql`.
12. **MCP and AI surfaces (`THINK-CHECKLIST 12`)**:
    - **Decision**: `shako-mcp` generates `jc_forge_*` tools from `OperationRegistry`, filters `tools/list` and `tools/call` by the caller's DelegationPolicy `connector_segments` (`forge` vs `apps`), scans arguments for secrets before permission checks, and fences tool results with `"untrustedData": true`.
    - **Tests**: `shako-server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
13. **Observability and operations (`THINK-CHECKLIST 13`)**:
    - **Decision**: Request IDs (`RequestCtx::request_id`), structured tracing without secrets, outbox audit events for every state change, and `/api/v1/version` / `/api/healthz` health probes.
    - **Tests**: `shako-server/tests/e2e.rs`.
14. **Tests and proof (`THINK-CHECKLIST 14`)**:
    - **Decision**: Real unit and end-to-end tests across `shako-core`, `shako-git`, `shako-store-memory`, `shako-store-pg`, `shako-web`, and `shako-server`, including real `git` client invocations in `shako-server/tests/e2e.rs`.
    - **Tests**: All test modules in `crates/shako/**`.
15. **UI (`THINK-CHECKLIST 15`)**:
    - **Decision**: S18 React pages are out of scope in this run; `shako-web` provides server-rendered read-only HTML pages with semantic markup, viewport meta for 400 px mobile widths, and Gitea page-path redirects.
    - **Tests**: `shako-web/src/lib.rs::tests`, `shako-server/tests/e2e.rs`.

## 2. `SECURITY-THINK.md` Forge Decisions (A–H & Threats T1–T15)

- **A. Trust boundaries & headers**: `authenticate_request` reads only `Authorization` (`Bearer`, `token`, `Basic`) and ignores any client-supplied `x-*` identity headers.
- **B. Identity & tokens**: `TokenVerifier` pins algorithm from JWK, requires `aud` presence (`"shako"` or `"container_registry"`), validates `exp`/`nbf`, and rate-limits JWKS refreshes to 1 per 10s (T4).
- **C. Authorization & tenancy**: `Authorizer` returns `visible: false` -> `404` on all surfaces when the caller lacks `Read` or targets another organization; OData `Scope` is applied before `$filter`, `$count`, and `$expand` (T6).
- **D. Injection & untrusted input**: `validate_repo_or_owner_name`, `validate_full_ref_name`, `validate_tree_path`, and `repo_disk_path` block path traversal and `.git` components (T1); `open_isolated` ignores repo hooks/configs and zero `std::process::Command` exists in `src/` (T2); `pulldown-cmark` escapes raw HTML and raw file downloads set `nosniff` + `attachment` on SVG/HTML (T14).
- **E. Outbound calls**: `OutboundGuard` blocks metadata `169.254.169.254`, loopback, private ranges, IPv4-mapped IPv6, NAT64, 6to4, redirects, and bodies > 1 MiB (T3).
- **F. Secrets & crypto**: Pre-receive secret scanner (`scan_blob_for_secrets`) blocks private keys and tokens in pushes and MCP arguments (T9); webhook HMAC-SHA256 verified in constant time (T11); OCI cross-repo blob mount restricted to the same `owner` and digests verified while streaming (T13).
- **G. DoS & resource exhaustion**: `MemoryBudget` 192 MiB semaphore, 65520 pkt-line cap, 512 MiB push cap, 100 MiB object cap, 50 delta depth cap, 1,000 refs/push cap (T5).
- **H. Data safety & integrity**: `RefPolicy` enforced on every ref-moving path (T7); whitelisting a user without `write` returns `422` naming the user instead of silently dropping them (T8); `head_commit_id` verified before merge (T10); compaction runs under `RepoLocks` with 14-day prune grace and skips when quarantine exists (T15). (T12 runner impersonation is not applicable in this run as S16/S17 Actions are out of scope).

## 3. Better-Over-Faithful Choices

1. **422 on whitelisting a subject without `write` (`shako-core::policy::RefPolicy::validate_rule_for_repo`)**: Replaces Gitea's silent drop of non-writers in `push_whitelist_usernames` / `merge_whitelist_usernames` (`KNOWLEDGE.md:190`, T-6438) with an explicit `422 Unprocessable Entity` naming the subject.
2. **200 OK no-op on `POST pulls/{n}/update?style=merge` when already up to date (`shako-git::merge::update_pr_branch`)**: Replaces Gitea's HTTP 500 when a PR branch is not behind base with a clean `200 OK` no-op (Contract S10).
3. **Pack-only API commits and merges (`shako-git::repo::write_objects_as_pack`)**: Replaces loose-object creation on API file writes and PR merges with a single indexed packfile per commit, eliminating loose-object inode bloat and `git gc` races (R§2.5).

## 4. `gix` API Calls Marked "gix API not verified" (Not Compiled in Offline Prompt Generation)

Every `gix` call in `shako-git` was written against the PART 5 docs.rs signatures (`gix 0.90.0`, `gix-pack 0.77.0`, `gix-packetline 0.24.1`, `gix-ref 0.69.1`, `gix-object 0.66.1`, `gix-merge 0.23.0`); because the code has not been compiled yet, the following `gix` calls are marked **gix API not verified**:
- `gix_pack::data::output::count::objects` and `gix_pack::data::output::entry::iter_from_counts` trait bounds on `gix::Repository::objects` (`OdbHandle`) in `shako-git/src/upload_pack.rs`.
- `gix_pack::Bundle::verify_integrity` options struct path (`gix_pack::bundle::verify::integrity::Options`) in `shako-git/src/compact.rs`.
- `gix::Repository::merge_trees` `Labels` import path (`gix_merge::blob::builtin_driver::text::Labels`) in `shako-git/src/merge.rs`.
