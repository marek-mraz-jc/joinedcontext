# daiya Status

This is the state on branch `tests-daiya-1` after the test-coverage pass of 2026-11-08. The
pass started from `jc-one-binary-1`.

Each item gets one of three verdicts:

- **done**: implemented, and covered by a test that exercises the real code.
- **partial**: some of it is implemented or tested; the gap is named.
- **not verified**: written but never run, with the reason given.

Every daiya package passes `cargo test`, `clippy --all-targets -D warnings` and `fmt --check`, and so does `cargo test -p jc`.

The Postgres tests ran against a throwaway PostGIS 17 server through `DAIYA_TEST_DATABASE_URL`, and no test printed a SKIP line.

- They run as a dedicated login role, `daiya_app_login`: a member of `daiya_app`, with NOSUPERUSER and NOBYPASSRLS. They do not run as the superuser.
- Running as the superuser would make forced row-level security (RLS) meaningless, and the boot check refuses that role anyway.

## Contract Items (R1–R12)

- **R1 Store format (Grafana classic JSON at schemaVersion 42, unknown fields kept, migrations 36..=42, below-36 warning)**: done.
  - Tests: `model/tests/migration_registry.rs` and `import/tests/round_trip.rs`.
  - The v38 step deleted a non-string table `displayMode`; it is now kept verbatim.
- **R2 Import / export / CI**: done.
  - **CLI.** `check`, `import --dry-run`, `export`, `plan` and `apply` are tested against the real binary, with JSON and Markdown reports (`server/tests/cli.rs`).
    - Exit codes now follow the kind of error: 1 for invalid JSON, refused input or an unknown uid; 3 for a stale plan; 4 for store, I/O or quota errors. `apply` returns the applied report's own code.
    - Before this pass, invalid JSON exited 4 and a refused apply exited 0.
  - **Configuration.** The binary reads a JSON configuration file (`--config`, `server/src/config.rs`, `deny_unknown_fields` everywhere). It sets the store (memory or Postgres), the API tokens file, the secrets directory, the provisioning directory and the outbound policy.
  - **CLI store.** The CLI subcommands act on the configured store. A Postgres store keeps data across restarts (`server/tests/binary.rs`).
  - **Provisioning loader.** `import/src/loader.rs` loads `datasources/*.yaml` and `dashboards/*.yaml` from disk at boot (`import/tests/loader.rs`).
    - It refuses: `orgId` other than 1 in every list, including `deleteDatasources`; `secureJsonData`; symlink escapes per file; trees deeper than 8 levels; more than 10 000 files.
    - It reads every file and dry-runs every dashboard batch before it writes, so a refused dashboard leaves no dashboards or folders behind.
  - **`compute_plan`.** It now reports unchanged dashboards as unchanged. It compares every datasource field.
  - Gaps:
    - Grafana's `$VAR` expansion in provisioning files is not done. Library code reads no environment variables (R11).
    - `disableDeletion` is not done.
    - YAML block scalars, anchors and merge keys are refused, not parsed.
- **R3 Datasources**: partial.
  - **Driver factory.** `query/src/factory.rs` builds a driver from a stored datasource record: Prometheus, NGSI-LD, OData, Postgres, and the Loki slot. WASM kinds and unknown kinds answer 501.
    - `DaiyaState::driver_for` caches each driver by record revision, and builds it only after the authorization decision (`api/tests/stored_drivers.rs`).
  - **Outbound guard.** Operators can now open non-public networks (`outbound.allowedNetworks`). Cloud metadata addresses are always refused.
  - **Postgres driver.** It runs against the real server under a dedicated reader role (`query/tests/factory.rs`). These are now decoded: int2/int4, numeric, timestamp, date, json and uuid columns, all of which used to come back null. Health runs `SELECT 1`, and a 50 000-row cut is noticed.
  - **PromQL variables.** They are escaped for the literal they stand in. Interpolation is one pass, by whole name.
  - **Prometheus.**
    - Warnings and infos become frame notices.
    - The min step is honoured.
    - Grafana's `api/v1/*` resource paths are accepted.
  - **NGSI-LD.** The projection keeps the geoproperty, and typed literals are read.
  - Gaps:
    - There is no real token exchanger. The default `DenyTokenExchanger` refuses, so NGSI-LD calls that need the caller's token fail until the host supplies an exchanger.
    - The Postgres driver keeps its password in the pool's connect options for the pool's lifetime.
    - The Postgres checker refuses the valid alias form `FROM v m (a, b)`.
    - Postgres `format: time_series` has no long-to-wide conversion.
    - The Prometheus safe step is not rounded the way Grafana rounds it, and heatmap format is treated as time_series.
- **R4 Calculation step (`__expr__` math, reduce, resample, threshold, classic_conditions, `sql` warning; 13 transformations)**: done (`expr/tests/*`). Fixed to match Grafana:
  - math `||` short-circuits on NaN as Go does;
  - reduce of an already-numeric value warns on every value and applies only the output mapping;
  - reducer and calculateField columns use Grafana's display names;
  - calculateField `%` and `^` are real operators;
  - renameByRegex is anchored, keeps its flags and translates `$1`;
  - an outer joinByField sorts its keys.
- **R5 WASM datasource plugins**: partial.
  - `wasmtime` is not a dependency, so no WASM is executed.
  - Tested against the in-process sample: the WIT file, the host adapter, the §3.3 limits (health and resource calls now take concurrency slots, import is size-capped) and digest pinning.
- **R6 / R6b Query editor API, `panel_view`, HTML table**: done (`api/tests/state_and_render.rs`, `api/tests/odata_routes.rs`). Both run on drivers built from stored records.
- **R7 Public dashboards**: done, on memory and on Postgres.
  - Public queries now run as `Subject::public_dashboard(org, uid)` under `PublicDashboardAuthorizer`. That authorizer allows only Read of its own dashboard and of the datasources its saved panels name. They no longer run as an internal "admin" subject (`api/tests/public_subject.rs`).
  - The `public_token_lookup` RLS policy is verified on Postgres: it shows exactly one row and allows no writes (`store-pg/tests/store_contract.rs`).
  - `PATCH` checks the uid in its path.
  - The standalone server rate-limits by the real peer address (`ClientAddr` from `ConnectInfo`).
- **R8 Tenancy**: done, on Postgres.
  - The store contract runs on both stores (`store-pg/tests/store_contract.rs`).
  - Forced RLS hides other organizations from the application role's raw SQL.
  - The boot check refuses a superuser login, a superuser that did `SET ROLE`, a BYPASSRLS role, an unforced table and a table with no policy.
  - The migration runs under an advisory lock and is skipped when its recorded checksum matches. Re-running the DDL next to live transactions used to deadlock.
  - The pool password comes from a file; a URL that carries a password is refused.
  - Folder depth follows Grafana (`depth >= 4` refused), and cycles are refused.
  - The §5.3 isolation suite passes on Postgres (`store-pg/tests/isolation_suite.rs`).
- **R9 Authorization**: done (`model/tests/auth_errors_frames.rs`, `api/tests/*`). Fixed in this pass:
  - Dashboard versions are checked against the dashboard's folder.
  - The OData DashboardVersions, Folders, LibraryPanels and PublicDashboards sets filter per object.
  - A `$batch` GET with an unparsable query answers 400 instead of the unfiltered set.
  - The API can no longer set `readOnly`.
- **R10a / R10 OData, MCP, CLI parity**: done (`model/tests/migration_registry.rs`, `api/tests/odata_routes.rs`, `mcp/tests/tools.rs`, `server/tests/router.rs`).
  - MCP tools refuse a missing uid, path, accessToken or operation, and an out-of-range panelId, with 400. Before, these became an empty string, "read" or panel 1.
  - Gap: `Dashboards('k')/$count` ignores the key.
- **R11 One binary**: partial.
  - The standalone `daiya` binary is done. It covers configuration, stored tokens (SHA-256 digests), provisioning, a Postgres store, graceful SIGINT shutdown that exits 0, and serve flags that override the mount.
  - The `jc` binary (`bins/jc/src/modules/daiya.rs`) still mounts daiya on an in-memory store with no API tokens. It has not been wired to `config.rs` yet.
- **R12 Security**: done for the tested classes.
  - In place: `forbid(unsafe_code)`, RFC 9457 slugs with no internal detail in 500s, constant-time token compare, the §4.3 CVE regression tests, and the PromQL escaping above.
  - Refused: `secureJsonData` at the compat routes and in provisioning, including YAML flow mappings.
  - YAML errors give a line number and never quote the line.
  - File secrets are one name under `<dir>/<org>/`.

## §7.1 Compatibility Matrix

The verdict groups of the previous status hold.

- Provisioning is done (see R2).
- For the compat-route behaviour verified in this pass, see `COMPAT.md`.
- Validation still skips missing-datasource checks for an organization with zero datasources: an empty list means "nothing known". Fixing it needs `import` to pass "no list" and "empty list" separately.

## Coverage (cargo +nightly llvm-cov, all 9 packages together, Postgres set)

Before is `jc-one-binary-1`; after is this branch. Each cell is lines / functions, in %.

| Package | Before | After |
|---|---:|---:|
| daiya-model | 79.1 / 69.1 | 99.5 / 99.0 |
| daiya-expr | 60.9 / 56.5 | 99.2 / 100.0 |
| daiya-query | 64.1 / 58.3 | 98.0 / 96.5 |
| daiya-store-memory | 60.2 / 61.1 | 98.6 / 98.9 |
| daiya-store-pg | 6.2 / 6.4 | 99.2 / 95.0 |
| daiya-import | 83.3 / 67.1 | 98.5 / 97.6 |
| daiya-api | 59.4 / 62.8 | 99.1 / 99.2 |
| daiya-mcp | 75.0 / 60.0 | 98.1 / 100.0 |
| daiya-server | 28.5 / 37.9 | 96.7 / 98.4 |
| all daiya | 60.9 / 54.1 | 98.8 / 98.1 |

Some files are below 95 % functions while their package is above it.

| File | Functions | What is not covered |
|---|---:|---|
| `query/src/guard.rs` | 90.3 % | error closures for a missing host, an unknown port, empty DNS and client build |
| `query/src/postgres.rs` | 92.7 % | a constant regex compile, `SET TRANSACTION` failing, numeric in text format |
| `query/src/sample_plugin.rs` | 90.5 % | serde serialization errors |
| `query/src/wasm.rs` | 92.7 % | serde serialization errors |
| `import/src/loader.rs` | 94.4 % | a directory entry failing mid-listing |
| `server/src/main.rs` | 92.9 % | quota exit, a failing `next_entry`, an `axum::serve` error |

None of these can be triggered without reshaping the code, and no exclusions were added.

## Out of Scope (Per Contract)

- **Alerting engine (§2.4)**: not started. The tables `cr_alert_rule` and `cr_alert_state` exist in the migration.
- **Remaining 31 browser transformations**: imported with a named warning and kept on export.
- **Portal React components**: not in this repository.
