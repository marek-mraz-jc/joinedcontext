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

## Stage 4: Portal (done)

- Imported with `git subtree add --squash --prefix crates/portal /workspace/joinedcontext-portal 8d2241f9` (commits
  `141e04e`, `1aa86f8`). Own workspace and lockfile, excluded from the root workspace.
- Its git dependencies cannot be fetched here (GitHub refuses the token), so they are vendored (`vendor/SOURCES.md`):
  `vendor/joinedcontext-platform` (origin/main `e59ceb9f`, the 8 crates the Portal links; the Portal pinned
  `9b0b8594`, 4 commits behind) and `vendor/ngsild` (`v0.3.0`, `b06b4145`, from the local cargo git cache). Both the
  root manifest and `crates/portal/Cargo.toml` patch the two git URLs.
- One ngsild in `jc`: the root patches the ngsild URL to Antares' own `antares-{ql,jsonld,matcher,model}` (the ngsild
  repository is a generated copy of them at Antares `15c08b85`; Antares `b683bfc3` is newer). Two path packages of one
  name and version cannot share a lockfile. The standalone Portal workspace still builds against `vendor/ngsild`.
  Not verified: that the Portal's gateway behaves the same on Antares `b683bfc3`'s ngsild as on `v0.3.0` beyond the
  jc tests (the Portal's own suite runs against `vendor/ngsild`).
- `crates/portal/src/server.rs`: `serve()` split into `spawn_background(&state)` (reapers, sync, SyncSource and
  foreign-model loops, build pods, agent reaper, erasures, kept answers, Grafana restore) and
  `spawn_internal_listener(&state, shutdown) -> Option<SocketAddr>`; `serve()` calls both, then `app(state)` as before.
  `app`, `AppState::from_config` and `Config::from_vars` were public already.
- In `jc` (`bins/jc/src/modules/portal.rs`, feature `portal`, slug `/portal`): the Portal reads its own `JC_*`
  environment exactly as standalone (its own DB URL, realm, client secret, auth); the only new default is
  `JC_PORTAL_PUBLIC_URL` = the mount's public base when unset (standalone: `http://localhost:8080`).
- The 9090 clash: Antares opens no listener inside `jc` (stage 3), so the Portal's internal listener
  (`JC_INTERNAL_BIND`, default `0.0.0.0:9090`, opened only when a realm is configured, every route needs a workload
  token) is the only one on 9090; a clash with `jc`'s own `--listen`/`--admin-listen` shows as a bind error at start.
- Metrics: `jc` installs its recorder before any module is built (`app::start`); the Portal's `telemetry::install`
  then falls back to a detached recorder (its warning says so). Portal series go to `jc`'s admin `/metrics`, without the
  Portal's custom histogram buckets; the Portal's own `/metrics` is empty inside `jc`.
- Background tasks of the Portal are plain `tokio::spawn` and stop with the runtime; the internal listener stops on
  `jc`'s cancellation token.
- Not verified: the Portal UI (embedded assets, absolute asset paths) under `/portal/` in slug mode; the Portal with a
  realm (no Keycloak/wardkeep realm wiring between modules in this prototype); every Postgres path (no Postgres: the
  12 tests that need `JC_APPS_DB_TEST_URL` fail by design without it, all other 4965 pass).

Gates: Portal workspace `cargo test -p joinedcontext-portal --no-fail-fast` (2 jobs, `debug=0`): 4965 passed, 12
failed, all 12 at `tests/common/apps_db.rs:16` (`JC_APPS_DB_TEST_URL` unset); `cargo clippy -p joinedcontext-portal
--all-targets -D warnings`, `cargo fmt --all --check`: green (2 deprecation warnings in vendored `jcctl`, upstream,
unchanged). Root `cargo test -p jc` (lib 20, binary 4, one_binary 11), `cargo clippy -p jc --all-targets -D
warnings`, `cargo fmt --all --check`: green.

## Stage 5: end-to-end tests (done)

`bins/jc/tests/one_binary.rs` (in-process, ephemeral ports, one `jc` at a time because of Antares' statics) and
`bins/jc/tests/binary.rs` (the built binary as a child process):

- `--role all` starts every module; a health or discovery route of each answers in slug mode
  (`slug_mode_reaches_every_module_and_404s_unknown_slugs`) and in host mode through a trusted proxy's
  `X-Forwarded-Host` (`host_mode_reaches_every_module_and_404s_unknown_hosts`), plus mixed explicit mounts.
- Unknown slug, unknown host, `..` and `%2e%2e` paths: 404. A forwarded host from an untrusted peer is ignored.
- Overlap is refused at start; a bad prefix, a bad module setting, a bad `ANTARES_*` value, a taken port, and an admin
  listener without a readable token are refused.
- Another module's secret is not readable through ekiben: not by `${VAR}` or `env()` (in-process), not by a `file`
  input of `/proc/self/environ` (child process, the secret in the initial environment).
- Antares `/q` needs its token inside `jc`; the admin listener's `/metrics` needs the admin token and `/metrics` is
  404 on the public listener; SIGTERM stops the binary cleanly; a refused config exits 2, a start failure 1.

Result: `cargo test -p jc`: lib 22, binary 4, one_binary 12, all green.

## Stage 6: coverage (done)

`cargo +nightly llvm-cov`, line coverage, real tests only (no exclusions, nothing ignored):

| Crate | Lines | Measured with |
|---|---|---|
| `bins/jc` | **96.77 %** (898/928) | `-p jc -p jc-router` (unit, in-process e2e, child-process binary tests) |
| `crates/shared/jc-router` | **97.87 %** (460/470) | same run |
| `ekiben-server` | 72.21 % (1894/2623) | ekiben workspace, its own tests |
| `ekiben-core` | 54.37 % (1399/2573) | same |
| `ekiben-bloblang` | 72.03 % (7151/9928) | same |
| `antares-broker` | 66.61 % (1578/2369) | Antares workspace, its own tests (Postgres/NATS/MQTT ones skip without their env) |
| `joinedcontext-portal` | 90.10 % (162536/180395) | Portal workspace, its own tests; the 12 Postgres tests fail without `JC_APPS_DB_TEST_URL` |
| `wardkeep-server` | 15.02 % (35/233) | root, its own tests (thin binary crate; the logic is in the other wardkeep crates) |
| `dataroom-server` | 56.32 % (254/451) | root, its own tests |
| `controlroom-server` | 28.42 % (81/285) | root, its own tests |

These are the floors. Uncovered in `bins/jc`: the fallback arm for a module not compiled in (`modules/mod.rs:49-52`,
unreachable with default features), `RoleError::NotCompiled` (`module.rs:118`), the runtime-build failure in
`main.rs:42-43`, the `ANTARES_METRICS_PORT` warning (`modules/antares.rs:29-32`), and single error branches.
Note: a stale `llvm-cov-target` from stage 1 first gave 67 % (old `jc` objects merged into the report); after
removing the stale objects the run above is clean.

## Release binary

`cargo build --release -p jc` (default release profile, no LTO, all default features incl. the Portal's embedded UI):
196,455,176 bytes, 151,883,704 bytes stripped. Built in 9 min.

## Dependency checks (once, root lockfile = everything `jc` links)

- `cargo audit`: 4 vulnerabilities, none introduced by `jc` code, none ignored:
  - `quick-xml 0.37.5`: RUSTSEC-2026-0194 and -0195. It comes through `oxrdfxml 0.1.7` from `dataroom-rdf`, which was
    already in the base. The fix needs quick-xml 0.41, i.e. a newer oxrdfxml.
  - `crypto-bigint 0.5.5` (RUSTSEC-2026-0387, through `age` -> `jcctl`/Portal): the fix is 0.7.6, a different
    major version used by elliptic-curve 0.13.
  - `rsa 0.9.10` (RUSTSEC-2023-0071, through `openidconnect` -> Portal): no fixed version exists.
  - Warnings for unmaintained crates: `atomic-polyfill`, `basic-toml`, `paste`.
- `cargo deny check`: the root has no `deny.toml`; with `crates/portal/deny.toml` the result is licenses ok,
  advisories FAILED (the vulnerabilities above plus `paste`), bans FAILED (wildcard path dependencies in every
  monorepo crate, e.g. `controlroom-api`, `jc`), sources FAILED (`odata-*` from an Antares git rev in
  `crates/controlroom/model/Cargo.toml:11`, already in the base). No root policy was added to make it pass.

## Security review of the diff

Read once over `controlroom-argon-1..HEAD`, excluding the squashed imports and `vendor/`:

- Fixed while reviewing: the admin listener's `/metrics` was open, but it carries Antares' series, which the broker
  serves only behind its admin token. `--admin-listen` now needs `--admin-token-file` (`JCBIN_ADMIN_TOKEN_FILE`),
  and the bearer is compared in constant time (`app.rs` `admin_router`, `runtime::secret_eq`). Red proof: with the
  check bypassed, `slug_mode_...` got 200 where it expects 401.
- Each module keeps its own state, store, and auth. `jc-router` hands a request only to the router of the mount it
  matched; the modules share no state (only the process-wide recorder, subscriber, and crypto provider).
  - wardkeep: KEK from a file, `LocalAuthorizer`, as standalone.
  - dataroom: `PolicyAuthorizer::default()`, as its standalone `serve`.
  - controlroom: `for_standalone`, as standalone.
  - ekiben: `require_auth` plus allow-listed env plus a file root (stage 2).
  - Antares: admin token mandatory.
  - Portal: its own `JC_*` config, unchanged.
- Standalone behaviour is unchanged:
  - ekiben's `main.rs` passes `true`, i.e. `EnvAccess::Process`, and `require_auth: false`, as before.
  - Antares' `run()` takes the same steps.
  - Portal `serve()` spawns the same tasks.
  - dataroom's `run_cli` uses the extracted function.
- No secret is logged. Token files are read trimmed and errors name only the path. The TOML config holds paths, not
  secrets, and parse errors carry only the message.
- New defaults:
  - listener `127.0.0.1:8080` (loopback)
  - scheme `https`
  - ekiben file root: a 0700 temp directory
  - Portal public URL: the mount base (standalone: `http://localhost:8080`)
  - dataroom org and catalogue ids: the same as its standalone defaults
- Every new requirement narrows access: tokens for ekiben, the Antares `/q` surface, and admin `/metrics`.

## Not verified

- Postgres paths of every module: no Postgres here (dind is broken). `jc` wires memory stores only; the Portal
  connects to whatever `JC_*` names, untested.
- A realm between modules (the Portal or Antares verifying wardkeep tokens), and the Portal UI under `/portal/`.
- Antares under the ETSI suite while hosted (axum writer, no own connection cap or drain); `nest_service` is replaced
  by `fallback_service` (axum 0.8 refuses `nest_service("/")`).
- The Portal's gateway on Antares `b683bfc3`'s ngsild instead of `v0.3.0`, beyond the jc tests.
- The release binary was built but not run beyond `--version`.

## Open problems

- Slug mode puts every module on one origin (`app.rs:75`). The Portal's `__Host-` cookies are `Path=/`
  (`crates/portal/src/auth/session.rs:227`), so they ride along to every module on that host, and an XSS in any
  module could drive the Portal API same-origin. wardkeep scopes its cookies to its mount. Host mode avoids this;
  production should use host mode at least for the Portal and wardkeep.
- Module background work is not on `jc`'s `TaskTracker`: the tracker is passed but unused (`modules/mod.rs:34`).
  Antares (`crates/antares/crates/antares-broker/src/lib.rs:1323`) and the Portal (`crates/portal/src/server.rs:292`)
  `tokio::spawn` on their own and stop with the runtime, not with the token.
- One Antares per process (statics). `jc::start` is not `Send` because of Antares' `build_state` (`app.rs:235`).
- Portal metrics lose their custom histogram buckets inside `jc`, and the Portal's own `/metrics` is empty
  (`crates/portal/src/telemetry.rs:148`).
- Standalone ekiben still serves with `Process` env access and without auth when no token is given
  (`crates/ekiben/crates/ekiben-server/src/main.rs:204`).
- `zip` requirement relaxed to `^9.2` for the cooldown (`crates/ekiben/Cargo.toml:64`); back to `^9.3` from
  2026-11-09.
- The vulnerable dependencies and `cargo deny` findings above.
- gitleaks over the branch reports 73 findings, all in upstream test fixtures and docs of the imported trees and
  `vendor/` (JWT/PEM test vectors, sample keys), none in `bins/jc`. These repositories have their own allow-lists
  upstream.
- 2 deprecation warnings in vendored `jcctl` (`vendor/joinedcontext-platform/crates/jcctl/src/secrets/sops.rs:635`),
  upstream code.
