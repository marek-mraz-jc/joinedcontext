# shako Status (2026-11-08)

Note: This code was written in one pass without running `cargo build` or `cargo test` yet, so every
item below is marked **not verified (not compiled)** alongside the implemented code and tests.

## Contract Items

- **S1 Module, mount and binaries (`shako-server`, `bins/jc/src/modules/shako.rs`)**: partial / not verified (not compiled).
  - Implemented: `Module::Shako` (`name = "shako"`, `slug = "git"`, `host_label = "git"`, feature `shako`), `shako_server::router(state, mount)` and `shako_server::registry_router(state, mount)` mounted at `/v2` in `bins/jc`, and `shako` CLI subcommands `serve`, `migrate`, `apply -f`, `check -f`, `compact`, `fsck`. (`import-gitea` and `runner` subcommands are out of scope in this run).
  - Tests written: `crates/shako/server/tests/e2e.rs::s1_mount_modes_and_overlap_refusal`, `bins/jc/tests/one_binary.rs`.
- **S2 Store, schema and repository storage (`shako-store-pg`, `shako-store-memory`, `shako-core`)**: not verified (not compiled).
  - Implemented: `crates/shako/store-pg/migrations/0001_init.sql` (schema `shako`, roles `shako_migrate` and `shako_app`, 15 tables with `instance_id` and `org_id`, `ENABLE` + `FORCE ROW LEVEL SECURITY`, leakproof index `idx_shako_repo_instance_owner_name`), `verify_rls_enforced` boot check, `MemoryStore`, UUID-only bare repo paths `{root}/{instance_id}/{repo_id}.git`, and shared isolation suite.
  - Tests written: `crates/shako/store-memory/tests/isolation.rs::isolation_suite_and_boot_rls_refusal_pass_on_memory_store`, `crates/shako/store-pg/tests/isolation.rs::postgres_isolation_boot_rls_and_leakproof_index_plan`.
- **S3 Git smart HTTP: upload-pack (`shako-git/src/upload_pack.rs`, `shako-api/src/smart_http.rs`)**: not verified (not compiled).
  - Implemented: Protocol v0 and v2 advertisement and negotiation (`ls-refs` with `peel`, `symrefs`, `ref-prefix`, `unborn`; `fetch` with `want`, `want-ref`, `have`, `done`, `thin-pack`, `include-tag`, `ofs-delta`, `shallow`, `deepen`, `deepen-relative`, `deepen-since`, `deepen-not`, `filter`, `sideband-all`, `wait-for-done`; `object-info`; `packfile-uris` refused; `odb.prevent_pack_unload()` before `gix-pack` generation; `refs/pull/*` hidden from advertisement but fetchable by name).
  - Tests written: `crates/shako/git/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs::s3_s4_git_client_matrix_v0_v2_and_portal_push_bundle`.
- **S4 Git smart HTTP: receive-pack (`shako-git/src/receive_pack.rs`, `shako-api/src/smart_http.rs`)**: not verified (not compiled).
  - Implemented: Command parsing, `RefPolicy` and quota precheck, streaming into quarantine `objects/incoming-<uuid>/` with `QuarantineDir` RAII cleanup, object size and delta-depth limits, thin-pack completion via `Bundle::write_to_directory`, connectivity check, `gix-ref` atomic/per-ref transactions under `RepoLocks`, `report-status` / `report-status-v2`, and Portal `push_bundle` support.
  - Tests written: `crates/shako/git/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs::s3_s4_git_client_matrix_v0_v2_and_portal_push_bundle`.
- **S5 Ref policy: branch and tag protection (`shako-core/src/policy.rs`)**: not verified (not compiled).
  - Implemented: `BranchProtectionRule` and `TagProtectionRule` evaluated on receive-pack, contents commits, branch create/delete, PR merge, and update-branch; `422` when whitelisting a subject without `write`; `405 ... behind the base branch` when outdated; `block_admin_merge_override` enforced on admins.
  - Tests written: `crates/shako/core/src/policy.rs::tests`, `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S6 Maintenance, quotas, backups (`shako-git/src/compact.rs`, `shako-git/src/bundle.rs`, `shako-core/src/quota.rs`)**: not verified (not compiled).
  - Implemented: Pack compaction under `RepoLocks` with 14-day prune grace and quarantine check, `fsck_repository`, streaming push and storage quotas, full backup bundle writer and `restore_from_bundle`.
  - Tests written: `crates/shako/git/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S7 Gitea REST compatibility subset (`shako-api/src/compat.rs`, `crates/shako/COMPAT.md`)**: not verified (not compiled).
  - Implemented: Rows 1–5 and 12–39 served with Gitea 1.27.3 JSON fields and error quirks (`405 behind the base branch`, `409` on moved head/existing repo/stale blob sha/existing generic package file); Rows 6–11 answer `404` with Gitea error JSON `{"message":"actions are not served by this forge yet","url":...}`; Rows 16, 17, 19 act as transition no-ops recording warning events.
  - Tests written: `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S8 Operation registry and native REST (`shako-core/src/ops.rs`, `shako-api/src/compat.rs`)**: not verified (not compiled).
  - Implemented: `OperationRegistry` (`OperationDef` with `compat_path`), native REST routes under `/api/v1/projects/{project}/repositories/...` with RFC 9457 slugs, and `Allowed` decision parity.
  - Tests written: `crates/shako/core/src/ops.rs::tests::registry_completeness_and_csdl_snapshot`, `crates/shako/server/tests/e2e.rs`.
- **S9 OData 4.01 service (`shako-api/src/odata.rs`, `shako-core/src/ops.rs`)**: not verified (not compiled).
  - Implemented: `{mount}/odata/` service document, `$metadata` (CSDL namespace `Shako`), 16 entity sets on `odata-table` with `Scope` applied before `$filter`, `$expand=Files,Reviews` on `PullRequests`, 501 on unbounded `Commits` scan, 400 on undeclared secret properties, and bound/unbound actions and functions.
  - Tests written: `crates/shako/server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
- **S10 Pull requests, reviews and merge (`shako-git/src/merge.rs`, `shako-api/src/compat.rs`)**: not verified (not compiled).
  - Implemented: Monotonic PR numbers per repo, synchronous in-memory mergeability via `gix-merge`, `merge` / `squash` / `fast-forward-only` (`rebase` refused with 422), `head_commit_id` verification, `update-branch`, reviews (`APPROVED`, `REQUEST_CHANGES`, `COMMENT`), self-approval refusal, and changed-files diff.
  - Tests written: `crates/shako/git/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S11 Contents, trees, commits, archives (`shako-git/src/repo.rs`, `shako-git/src/archive.rs`, `shako-git/src/bundle.rs`)**: not verified (not compiled).
  - Implemented: Tree paging (1,000 per page, `truncated`), single and multi-file commits (`create`, `update`, `delete`, `upload`) written as a single packfile with all-or-nothing blob SHA preconditions, commit history with path filter, `.zip` / `.tar.gz` archives, and `.bundle` v2 exports.
  - Tests written: `crates/shako/git/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs`.
- **S12 Authentication and authorization (`shako-core/src/auth.rs`)**: not verified (not compiled).
  - Implemented: Bearer, `token`, Basic, and `/v2/token` forms; JWKS cache with bounded refresh on unknown `kid`; ES256 registry token minting (<= 15 min); `Authorizer` with 7 verbs and 404 for hidden objects.
  - Tests written: `crates/shako/core/src/auth.rs::tests`, `crates/shako/server/tests/e2e.rs`.
- **S13 Package registry and signing (`shako-registry/src/lib.rs`)**: not verified (not compiled).
  - Implemented: OCI distribution (`/v2/`, `/v2/token`, blob HEAD/GET, monolithic and chunked uploads, same-owner cross-repo mount, manifest PUT/GET/HEAD with 4 MiB cap, tag list, referrers), generic packages (`409` on overwrite), and automatic ECDSA P-256 cosign simple-signing (`sha256-<digest>.sig`).
  - Tests written: `crates/shako/server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
- **S14 Events, outbox, webhooks (`shako-api/src/outbox.rs`)**: not verified (not compiled).
  - Implemented: Idempotent outbox rows, in-process broadcast bus + subscribers, and Gitea-format webhooks with `X-Gitea-Event` and `X-Gitea-Signature` (HMAC-SHA256 hex), secret rotation support, `OutboundGuard`, and 3 retries.
  - Tests written: `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S15 Push mirrors (`shako-git/src/send_pack.rs`, `shako-api/src/compat.rs`)**: not verified (not compiled).
  - Implemented: `push_mirrors` CRUD and `push_mirrors-sync`, `send-pack` over HTTPS through `OutboundGuard` with credentials from `SecretStore` and scrubbed error strings.
  - Tests written: `crates/shako/server/tests/e2e.rs::s5_s7_s10_s11_s14_s15_portal_change_lane_webhooks_mirrors_and_backup`.
- **S16 Actions: workflows, scheduler, runner protocol, artifacts**: out of scope, run 4.
- **S17 jc-runner and the build dispatcher**: out of scope, run 4.
- **S18 UI (`shako-web/src/lib.rs`)**: partial (read-only server-rendered `shako-web` in this run; React Portal pages out of scope, run 3) / not verified (not compiled).
  - Implemented: Server-rendered HTML pages (repo list, repo home with tree and escaped-HTML Markdown README, file view, raw file with `nosniff` + `attachment`, branches, tags, commits, commit diff, PR list and detail) and Gitea page-path redirects.
  - Tests written: `crates/shako/web/src/lib.rs::tests`, `crates/shako/server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
- **S19 MCP tools (`shako-mcp/src/lib.rs`)**: not verified (not compiled).
  - Implemented: `jc_forge_*` tools generated from `OperationRegistry`, DelegationPolicy connector segment filtering (`forge` vs `apps`), secret scan on arguments, and untrusted-data fencing.
  - Tests written: `crates/shako/server/tests/e2e.rs::s8_s9_s13_s18_s19_s21_odata_registry_mcp_web_and_security_threats`.
- **S20 Import from Gitea (`import-gitea`)**: out of scope, run 2.
- **S21 Security (Threats T1–T15)**: not verified (not compiled).
  - Implemented & tested: T1 (`shako-core/src/validate.rs::tests`, `e2e.rs`), T2 (`e2e.rs::t2_no_subprocess_command_in_any_shako_src_file`), T3 (`shako-core/src/outbound.rs::tests`), T4 (`shako-core/src/auth.rs::tests`, `e2e.rs`), T5 (`shako-git/src/pktline.rs::tests`, `shako-git/src/lib.rs::tests`), T6 (`isolation.rs`, `e2e.rs`), T7 & T8 (`shako-core/src/policy.rs::tests`, `e2e.rs`), T9 (`shako-git/src/receive_pack.rs`), T10 (`shako-git/src/lib.rs::tests`, `e2e.rs`), T11 (`e2e.rs`), T13 (`e2e.rs`), T14 (`shako-web/src/lib.rs::tests`, `e2e.rs`), T15 (`shako-git/src/lib.rs::tests`). (T12 is S16 runner protocol, out of scope in this run).
- **S22 Memory, CPU and performance (`shako-core/src/quota.rs::MemoryBudget`)**: partial / not verified (not compiled).
  - Implemented: 192 MiB memory-weighted semaphore returning `503` with `Retry-After: 5` on saturation, 2 MiB per-handle ODB object cache cap, 64 KiB streaming `mpsc::channel(4)`. Live RSS and k6 benchmarks are not run here.
  - Tests written: `crates/shako/core/src/quota.rs::tests::memory_budget_returns_503_when_saturated`.
- **S23 Delivery rules**: not verified (not compiled).
  - Edition 2024, license MIT, `#![forbid(unsafe_code)]` in every crate, `crates/shako/MEMBERS.txt` and workspace `Cargo.toml` updated.
- **S-later LFS**: out of scope, later run.
