# daiya Status

State on branch `daiya-argon-1` after the compile-and-verify pass (2026-11-07). The first
version of this file, written with the generated code before it had ever compiled, marked
R1–R12 as done. Several of those claims did not hold; they are corrected below. Verdicts:
**done** (implemented and covered by a test that exercises the real code), **partial** (part
implemented or tested, the gap is named), **not verified** (written but never run, for the
reason given).

Every package compiles, `cargo test` (59 tests), `clippy --all-targets -D warnings` and
`fmt --check` are clean. No test runs against Postgres: the host has no Postgres server and
Docker cannot start containers (overlay mount fails), so everything in `daiya-store-pg`
that touches a database is **not verified**.

## Contract Items (R1–R12)

- **R1 Store format (Grafana classic JSON at schemaVersion 42, unknown fields kept, migrations 36..=42 + below-36 warning)**: done (`model/src/migration.rs::tests`, `import/tests/round_trip.rs::all_9_dashboards_match_inventory_and_import_export_import_is_fixed_point`).
- **R2 Import / export / CI**: partial.
  - Done in-process: import with `--dry-run`, export, `check`, `plan`/`apply` with stale-plan refusal and the exit codes, JSON and Markdown reports, and the 9-dashboard round trip as a fixed point (`import/tests/round_trip.rs`).
  - Provisioning YAML is parsed and its paths are confined to the provisioning root (`import/src/provisioning.rs::tests`), but no code loads a provider's dashboards from disk.
  - The `daiya` binary runs on an empty in-memory store for each command. `export`, `plan` and `apply` therefore act on nothing, and `serve` has no persistent store and no configuration.
- **R3 Datasources**: partial.
  - Drivers are written for Prometheus, NGSI-LD, OData 4.01 and Postgres, with the `-- Mixed --` datasource and the Loki slot.
  - The outbound guard covers the allow-list, DNS pinning, no redirects, a size cap, no env proxy, and IPv4 embedded in IPv6 (`query/src/guard.rs::tests`).
  - The Postgres guard is a token-level single-SELECT check run in a READ ONLY transaction with a row cap (`query/src/postgres.rs::tests`).
  - The OData driver is tested against a local HTTP server (`query/src/odata.rs::tests`). The entity set must be a simple identifier; before this pass it was pasted into the URL path. Columns come out in response order (serde_json preserve_order, as in the jc binary).
  - Gaps:
    - Nothing builds a driver from a stored datasource record. `DaiyaState::datasources` is filled only by tests, so a running server cannot query any datasource that was created through the API.
    - There is no real token exchanger: the default `DenyTokenExchanger` refuses, and NGSI-LD calls that need the caller's token fail until the host supplies an exchanger.
    - The guard refuses private addresses with no operator allowance, so an in-cluster Prometheus or broker cannot be reached.
    - The Postgres driver has never run against a server.
- **R4 Calculation step (`__expr__` math, reduce, resample, threshold, classic_conditions, `sql` warning; 13 transformations)**: done (`expr/src/*::tests`, `server/tests/e2e.rs::e2e_mixed_panel_with_math_and_reduce_expressions`).
- **R5 WASM datasource plugins**: partial. `wasmtime` is not in the root `Cargo.lock`. The WIT file, the host adapter trait, the §3.3 limits and digest pinning are tested against the in-process sample (`query/src/sample_plugin.rs::tests`); no WASM is executed.
- **R6 Query editor and API for the Portal (`EditorSchema`, `Query`, `Resource`, `RenderPanel`, server-side HTML table)**: partial. The routes are tested end to end with drivers registered by hand (`server/tests/e2e.rs`). In a running server they have no drivers (see R3).
- **R6b `panel_view` in-process function + HTML table**: partial. It exists in `api/src/render.rs` and is reached through the e2e test, with the same driver gap.
- **R7 Public dashboards**: done in memory (`server/tests/e2e.rs::e2e_public_dashboard_rules_of_r7`, `security_compat_routes_authorize_every_object`).
  - Covered: opt-in off by default, a hashed 256-bit token, saved queries only, refusal of forged payloads, range and interval caps, rotation answering 404, and a per-IP rate limit with a bounded address table.
  - The cache key now includes the interval and maxDataPoints.
  - The token lookup under forced RLS on Postgres has a new `public_token_lookup` policy that is **not verified**.
  - Public queries execute as an internal subject named `admin`, which works but is fragile.
- **R8 Tenancy**: partial.
  - Done on the memory store: all 8 isolation tests of §5.3 (`store-pg/tests/isolation_suite.rs::isolation_suite_passes_on_memory_store_and_verifies_boot_rls_refusal`). Tests 3, 5, 7 and 8 used to be vacuous; 5 and 8 now drive the real query cache and export.
  - Not verified on Postgres: the forced RLS migrations, the `daiya_app` role, the boot check and the secret-file pool config. `isolation_suite_passes_on_postgres_store` passes only because it skips when no database URL is set.
  - The memory store's boot RLS refusal is simulated, not a catalog check.
- **R9 Authorization (`Authorizer`, Viewer/Editor/Admin per folder, cross-org 404)**: done (`server/tests/e2e.rs::e2e_cross_organization_404s_and_cve_regression_suite`, `security_*` tests). Before this pass:
  - The caller's identity came from `x-daiya-subject` and `x-daiya-orgs` request headers, and an anonymous request was admin of the default org.
  - Export, the org and folder compat routes, search and the public-dashboard config had no or partial checks.
  - Identity now comes only from a host `SubjectResolver` or a hashed API token.
- **R10a / R10 OData and registry (`/odata/v4`, CSDL `$metadata`, `$batch`, 8 entity sets, 12 operations, OData/MCP/CLI parity)**: done (`model/src/registry.rs::tests`, `server/tests/e2e.rs::e2e_odata_mcp_and_cli_parity_and_jc_router_mount`). `$batch` is now capped at 100 requests. MCP tools refuse a subject with no role, refuse writes by a viewer and by a read-only connector (`server/tests/e2e.rs::security_mcp_tools_authorize_like_odata`).
- **R11 One binary (`router(state, mount)`, standalone binary, no globals in library crates)**: partial. The router and mount are tested. The binary has no store or configuration wiring and no way to configure API tokens, so `serve` answers every authenticated route with 404.
- **R12 Security**: partial.
  - Done: `forbid(unsafe_code)`, no unwrap or panic outside tests, RFC 9457 slugs, constant-time token compare, the §4.3 CVE regression tests, and the security tests added in this pass.
  - Not true before this pass: "parameterized SQL / query-language isolation". The Postgres view guard was bypassable, and the template-variable cache key ignored the variables.
  - PromQL variable escaping only covers `"` and `\`. This is not exploitable today, because authenticated callers may send raw PromQL and public variables are restricted to saved values.

## §7.1 Compatibility Matrix Verdict Groups

- **Dashboard top-level fields, schemaVersion 39 → 42**: done (`import/tests/round_trip.rs`).
- **`gridPos` validation, `timeseries`/`table`/`stat`**: done (`import/tests/round_trip.rs`).
- **`bargauge`, `geomap` markers layer**: done (`import/tests/round_trip.rs`).
- **Datasource `{type, uid}` resolution within the org, missing uid refused**: done (`store-pg/tests/isolation_suite.rs`, memory store).
- **PromQL and NGSI-LD target fields**: done with hand-registered drivers (`server/tests/e2e.rs`). An explicit NGSI-LD `attributes` list is now a projection; before this pass, every attribute was returned.
- **Query variables and `fieldConfig`**: done (`import/tests/round_trip.rs`). Data links in `fieldConfig` defaults and overrides are now checked for dangerous schemes, and `data:image/svg+xml` is refused (`model/src/validation.rs::tests`).
- **The 13 transformations**: done (`expr/src/tx.rs::tests::all_13_transformations_execute_cleanly`).
- **`-- Mixed --` and `__expr__`**: done (`server/tests/e2e.rs::e2e_mixed_panel_with_math_and_reduce_expressions`).
- **Unsupported panel and datasource types refused with a pointer**: done (`model/src/validation.rs`).
- **`v2beta1` → v1 at 42**: done (`model/src/migration.rs::tests::v2beta1_converts_to_v1_at_42`).
- **Provisioning YAML (`secureJsonData` refused, `secretRef` required)**: partial. Parsing and the guards are done (`import/tests/round_trip.rs::plan_and_apply_refuse_stale_plan_and_provisioning_guards_work`); loading providers is not.

## Coverage (cargo +nightly llvm-cov, lines)

Target 95 % lines; not reached. Only tests of real behaviour were added. `store-pg` needs a
Postgres server to run its code, and `server` is mostly the CLI `main`, which no test starts.

| Package | Lines | Missed | Line coverage |
|---|---:|---:|---:|
| daiya-model | 2959 | 620 | 79.0 % |
| daiya-expr | 3341 | 1305 | 60.9 % |
| daiya-query | 3963 | 1423 | 64.1 % |
| daiya-store-memory | 560 | 223 | 60.2 % |
| daiya-store-pg | 1372 | 1344 | 2.0 % |
| daiya-import | 935 | 155 | 83.4 % |
| daiya-api | 2907 | 1177 | 59.5 % |
| daiya-mcp | 152 | 38 | 75.0 % |
| daiya-server | 285 | 204 | 28.4 % |
| all daiya | 16474 | 6489 | 60.6 % |

Files with no coverage at all: `query/src/loki.rs` (a trait slot), `server/src/main.rs` and
`model/src/entities.rs`. `expr/src/pipeline.rs`, `expr/src/tx.rs`, `api/src/compat.rs` and
`api/src/odata.rs` sit between 33 % and 45 %.

## Out of Scope for This Run (Per Contract)

- **Alerting engine (§2.4)**: not started. The tables `cr_alert_rule` and `cr_alert_state` exist in the migration (not verified on Postgres).
- **Remaining 31 browser transformations**: imported with a named warning and kept on export.
- **Portal React components**: not in this repository.
