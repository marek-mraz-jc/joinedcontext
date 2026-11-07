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
