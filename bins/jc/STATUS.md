# jc one binary: status

Branch `jc-one-binary-1` (from `controlroom-argon-1`), local only, never pushed. Plan: the feasibility report
(`/home/dev/tmp-jc-feasibility/report.md`). This file says where the work stands after each stage.

## Stage 1: bins/jc with wardkeep, dataroom, controlroom (done)

- `bins/jc`: lib (`app`, `config`, `module`, `modules/*`, `runtime`) and a thin `main.rs`.
- One tokio runtime (built in `main.rs`), one tracing subscriber (`runtime::init_tracing`, `try_init`, idempotent),
  one Prometheus recorder for the `metrics` facade (`runtime::metrics_handle`, installed once, served only on the
  optional internal admin listener `/metrics`), one allocator (jemalloc, feature `jemalloc`, `main.rs`), one rustls
  crypto provider (ring, `runtime::install_crypto_provider`, skipped when one is installed), one
  `CancellationToken` and one `TaskTracker` for shutdown (SIGTERM/Ctrl-C in `main.rs`).
- Mounting through `crates/shared/jc-router`: `--mode slug` (any host, `/<slug>`: `/id`, `/opendata`, `/dashboards`,
  later `/streams`, `/broker`, `/portal`) or `--mode host` (`<label>.<base_host>`), plus `[mounts.<module>]`
  overrides (host and/or path prefix) to mix both. Overlap is refused before any module state is built
  (`app::plan_mounts` dry-runs the router table). Unknown slug or host: 404.
- `jc --role all|<module>[,<module>]`; a module that is not compiled in is refused, not skipped.
- Configuration: defaults < TOML file (`--config`/`JCBIN_CONFIG`, unknown keys refused) < `JCBIN_*` < flags. No key
  starts with `JC_` (unit test `no_jc_key_starts_with_jc_underscore`). Default listener `127.0.0.1:8080`.
- Modules mounted: wardkeep (in-memory store, KEK from file, realm import dir), dataroom (in-memory store, default
  catalogue via the new `dataroom_server::ensure_default_catalogue`, URLs under the mount prefix), controlroom
  (in-memory store, `for_standalone` state). Each is the module's own `router(state, mount)` over its own state.
- Postgres paths of these modules are not wired into `jc` (memory stores only): not verified, no Postgres here.

Gates: `cargo build -p jc`, `cargo test -p jc -p jc-router -p dataroom-server`, `cargo clippy -p jc -p jc-router
-p dataroom-server --all-targets -- -D warnings`, `cargo fmt --all --check`: green.

## Stage 2: ekiben (done)

- Imported with `git subtree add --squash --prefix crates/ekiben /workspace/ekiben 787dfe3` (commits `2ae9b6b`,
  `140b231`). It keeps its own workspace (`crates/ekiben/Cargo.toml`, own `Cargo.lock`); the root workspace excludes
  it and `jc` depends on `ekiben-server`/`ekiben-core` by path (feature `ekiben`, slug `/streams`).
- `ekiben_server::build_full_registry` moved from `main.rs` into the lib; `main.rs` calls it.
- Security fixes before sharing the process (commit `bd65b59`):
  1. `EnvAccess` (`ekiben-core/src/config.rs`): `None`, `AllowListed` (only `EKIBEN_VAR_*`), `Process`. It governs
     `${VAR}` of posted configs (`api.rs` streams and resources) and Bloblang `env()` (`Environment::with_host_env_prefix`,
     `ekiben-bloblang/src/env.rs`). `StreamManager`/`Scheduler`/`run_stream_once`/`spawn_stream` take
     `impl Into<EnvAccess>` (`true` = `Process`, `false` = `None`, so old callers keep their meaning).
  2. `ServerAuthConfig::require_auth`: every request without a valid token is refused (401), even with no token
     configured, and a middleware guards every endpoint but `/ping` (`/metrics`, `/stats`, `/ready`, `/version`,
     `/endpoints` listed stream ids and organizations).
  3. In `jc` (`bins/jc/src/modules/ekiben.rs`): `EnvAccess::AllowListed`, `require_auth: true`, an admin token from
     `JCBIN_EKIBEN_TOKEN_FILE` (required; start refused without it), and `file_root` always set (configured, or a
     0700 directory under the temp dir) so `file` inputs/outputs cannot read `/proc/self/environ` or another
     module's key file. Private/loopback HTTP targets stay refused by ekiben's default quotas.
- Red proofs (fix line reverted once, test run, fix restored):
  - `EnvAccess::lookup` AllowListed -> `std::env::var`: `allow_listed_access_reads_only_ekiben_var_names` and
    `a_posted_config_cannot_substitute_another_modules_secret` FAILED.
  - prefix check in `Environment::lookup_env` removed: `bloblang_env_of_an_allow_listed_stream_cannot_read_other_variables`
    and `a_running_stream_cannot_read_another_modules_secret_through_env` FAILED.
  - `if self.require_auth { return Err(401) }` removed: `hosted_api_refuses_requests_without_a_token_even_with_none_configured` FAILED.
  - middleware disabled: both `hosted_api_*` tests FAILED.
  - in jc, `EnvAccess::Process`: `another_modules_secret_is_not_readable_through_an_ekiben_stream` FAILED; `require_auth:
    false`: same test FAILED; `file_root` `None`: `an_ekiben_file_input_cannot_read_the_process_environment` FAILED
    (the stream copied the 4.9 KB process environment into the output file).
- Not changed: the standalone `ekiben streams` still uses `Process` env access and serves without auth when no
  token is given (its own process, unchanged behaviour); open item if it should default to `AllowListed`.
- Cooldown: ekiben asks `zip ^9.3`; 9.3.0 was published 5 days ago, so the root resolve could not pick it. The
  requirement is `^9.2` in `crates/ekiben/Cargo.toml` (ekiben's own lockfile still pins 9.3.0); back to `^9.3` from
  2026-11-09.

Gates: ekiben workspace `cargo test -p ekiben-server -p ekiben-core -p ekiben-bloblang`, `cargo clippy` (same crates,
`--all-targets -D warnings`), `cargo fmt --all --check`: green. Root: `cargo build/test/clippy -p jc`, fmt: green.

## Stage 3: Antares (done)

- Imported with `git subtree add --squash --prefix crates/antares /workspace/Antares-NGSI-LD-Context-Broker b683bfc3`
  (origin/dev sha, not the old working tree). Own workspace and lockfile, excluded from the root workspace like
  ekiben. Root `Cargo.toml` copies Antares' `[patch.crates-io] rumqttc` (a patch only works from the root).
- `antares-broker`: `src/main.rs` -> `src/lib.rs`; new `src/main.rs` (allocator + `main_entry()`). `run()` split into
  `config_from_env() -> BrokerConfig`, `build_state(&cfg, metrics_render) -> Broker`, `spawn_background(&mut Broker)`
  (bus wiring, sampler, @context preload, expiry sweep, temporal maintenance) and `router(Broker) -> Served { app,
  metrics_app, drain }`; `run()` calls the same steps, then its own accept loop and drain. Standalone binary
  behaviour unchanged (its `tests/*` that spawn the binary pass: file_mode 18, store_combos 19, nats_e2e 8, ...).
- Telemetry (`telemetry.rs`): `try_init` keeps an installed subscriber; a refused second recorder falls back to the
  installed one (`/q/metrics` 404); `init_hosted()` installs nothing. `tests/hosted_telemetry.rs`; red with the old
  `.init()` (panics "a global default trace dispatcher has already been set") and with the recorder fallback
  disabled (`FailedToSetGlobalRecorder`).
- In `jc` (`bins/jc/src/modules/antares.rs`, feature `antares`, slug `/broker`): config from the broker's own `ANTARES_*`
  environment, `/q` admin token file mandatory (`JCBIN_ANTARES_ADMIN_TOKEN_FILE`; the standalone broker serves `/q`
  without a credential when it is unset), no listener of its own (`ANTARES_HTTP_PORT`/`ANTARES_METRICS_PORT` ignored,
  so no clash with the Portal's 9090), mounted with `Router::fallback_service(served.app)` (axum 0.8 refuses
  `nest_service("/")`; the jc-router already strips the slug). Default features only (`mqtt`), not `full`.
- One Antares per process: its statics (memory budget, WebSocket hub, caches, 153 statics per the feasibility
  report) are process-wide. The jc tests serialize every start with one lock for this reason.
- Differences when hosted, not verified against the ETSI suite: responses go through axum's server (lower-case
  header names) instead of Antares' title-case HTTP/1 writer; no connection cap, header read timeout or drain
  window of its own (jc's graceful shutdown instead); background tasks are plain `tokio::spawn` and stop with the
  runtime, not with jc's cancellation token.
- `jc::start` is not `Send` because `antares_broker::build_state`'s future is not (higher-ranked lifetime inside
  Antares); the binary runs it with `block_on`, tests on the test task.

Gates: Antares workspace `cargo build/test/clippy -p antares-broker --all-targets -D warnings`, `cargo fmt --all
--check`: green (29 test binaries; Postgres/NATS/mosquitto ones skip themselves without their env). Root
`cargo test/clippy -p jc`: green.
