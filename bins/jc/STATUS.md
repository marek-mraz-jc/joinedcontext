# jc one binary: status

Branch `jc-one-binary-1` (from `daiya-argon-1`), local only, never pushed. Plan: the feasibility report
(`/home/dev/tmp-jc-feasibility/report.md`). This file says where the work stands after each stage.

## Stage 1: bins/jc with kaisatsu, dataroom, daiya (done)

- `bins/jc`: lib (`app`, `config`, `module`, `modules/*`, `runtime`) and a thin `main.rs`.
- One tokio runtime (built in `main.rs`), one tracing subscriber (`runtime::init_tracing`, `try_init`, idempotent),
  one Prometheus recorder for the `metrics` facade (`runtime::metrics_handle`, installed once, served only on the
  optional internal admin listener `/metrics`, behind its bearer token since the review), one allocator (jemalloc, feature `jemalloc`, `main.rs`), one rustls
  crypto provider (ring, `runtime::install_crypto_provider`, skipped when one is installed), one
  `CancellationToken` and one `TaskTracker` for shutdown (SIGTERM/Ctrl-C in `main.rs`).
- Mounting through `crates/shared/jc-router`: `--mode slug` (any host, `/<slug>`: `/id`, `/opendata`, `/dashboards`,
  later `/streams`, `/broker`, `/portal`) or `--mode host` (`<label>.<base_host>`), plus `[mounts.<module>]`
  overrides (host and/or path prefix) to mix both. Overlap is refused before any module state is built
  (`app::plan_mounts` dry-runs the router table). Unknown slug or host: 404.
- `jc --role all|<module>[,<module>]`; a module that is not compiled in is refused, not skipped.
- Configuration: defaults < TOML file (`--config`/`JCBIN_CONFIG`, unknown keys refused) < `JCBIN_*` < flags. No key
  starts with `JC_` (unit test `no_jc_key_starts_with_jc_underscore`). Default listener `127.0.0.1:8080`.
- Modules mounted: kaisatsu (in-memory store, KEK from file, realm import dir), dataroom (in-memory store, default
  catalogue via the new `dataroom_server::ensure_default_catalogue`, URLs under the mount prefix), daiya
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
  realm (no Keycloak/kaisatsu realm wiring between modules in this prototype); every Postgres path (no Postgres: the
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
| `kaisatsu-server` | 15.02 % (35/233) | root, its own tests (thin binary crate; the logic is in the other kaisatsu crates) |
| `dataroom-server` | 56.32 % (254/451) | root, its own tests |
| `daiya-server` | 28.42 % (81/285) | root, its own tests |

These are the floors. Uncovered in `bins/jc`: the fallback arm for a module not compiled in (`modules/mod.rs:49-52`,
unreachable with default features), `RoleError::NotCompiled` (`module.rs:118`), the runtime-build failure in
`main.rs:42-43`, the `ANTARES_METRICS_PORT` warning (`modules/antares.rs:29-32`), and single error branches.
Note: a stale `llvm-cov-target` from stage 1 first gave 67 % (old `jc` objects merged into the report); after
removing the stale objects the run above is clean.

## Stage 7: Portal wired to its siblings (jc-portal-1)

`jc --role all` runs the Portal against kaisatsu, shako, ekiben, dataroom, daiya and Antares. Nothing external is
needed for sign-in, members, groups, changes and the forge.

### Wiring (`src/siblings.rs` `portal_vars`, `src/modules/portal.rs` `config`)

A row fills only when its URL key is unset in the environment, the sibling is in the role set and every secret the
row needs is held by setup. Inside a filled row, an explicit `JC_*` key still wins. `JCBIN_PORTAL_SIBLINGS=false`
turns it all off.

| Portal setting | sibling | filled keys | status |
|---|---|---|---|
| OIDC, Keycloak admin | kaisatsu | `JC_OIDC_ISSUER` (public `{base}/id/realms/jc`), `JC_OIDC_CLIENT_ID/SECRET`, `JC_COOKIE_KEY`, `JC_KEYCLOAK_ADMIN_CLIENT_ID/SECRET`, `JC_KEYCLOAK_ADMIN_URL` (jc's listener when it differs from the public origin) | works: sign-in, people, groups, approval |
| `JC_GITEA_*` | shako | `URL`, `PUBLIC_URL`, `OWNER=jc`, `REPO=config`, `TOKEN`, `APPS_OWNER=jc-apps`, `APPS_TOKEN`, `WEBHOOK_SECRET` | works: proposal commits, approval merges, sync reads them back |
| pipeline runner | ekiben | `JC_PIPELINE_RUNNER_URL` | wired, refused: no runner credential (gap d) |
| CKAN | dataroom | `JC_CKAN_URL`, `JC_CKAN_API_URL`, only when the public URL is https (EP-67) | wired for https installs; the http e2e leaves it unset (gap e) |
| Grafana | daiya | `JC_GRAFANA_URL`, `_PUBLIC_URL`, `_USER=portal`, `_PASSWORD` | works as server admin; nothing creates the org per project (gap f) |
| broker | antares | `JC_BROKER_URL`, `JC_BROKER_ADMIN_TOKEN_FILE` | wired; the gateway between Portal and broker is no jc module (gap d) |

The Portal's forge client reads its `JC_GITEA_*` through `Config::forge_var` (environment first, then the values jc
filled), so the process environment is never written.

### First-start setup (`src/setup.rs`, `src/provision.rs`)

- Every generated secret is a 0600 file (`create_new`) under one 0700 directory, `JCBIN_STATE_DIR` (default
  `jc-state`); an existing looser directory is tightened. `Held` redacts its value in Debug. Files survive restarts,
  so a second start is idempotent. Only key names are logged.
- kaisatsu: realm `jc` imported through `kaisatsu_import::apply_realm_files` with `$(env:...)` placeholders: client
  `portal` (confidential, code + PKCE S256, groups mapper, `view-clients`), `portal-ui` (public), `portal-admin`
  (service account, `manage-users` + `query-groups`, no realm-admin), group `platform-admins`, user `admin` in it.
- shako: token file; the repository `jc/config` created after serve through `POST /api/v1/orgs/jc/repos`
  (idempotent: GET first, 409 accepted).
- shako also gets `org.yaml` (the installation's Organization, domain = the Portal's host or `jc.invalid`) and
  `users/roles/steward.yaml` (the role a project's opener is bound to), each written only when absent.
- ekiben, Antares admin, daiya (the Portal's Grafana user is daiya's server administrator,
  `DaiyaState::with_server_admin`), dataroom (`dataroom_api::portal_publisher`: Read + Propose on the default
  catalogue only), the webhook secret and the Portal cookie key: one file each.
- The Portal is built after jc serves (a `LateRouter` answers 503 until then), because its discovery calls kaisatsu
  on jc's own listener.
- The Portal is mounted at the root (`[mounts.portal] path_prefix = ""`): its UI and redirects assume `/`.

### Guards and their red proofs

| guard | test | red (guard reverted once) |
|---|---|---|
| a row's credentials go only with jc's URL | `siblings::tests::*_row` | 4 row tests failed |
| secret files 0600 | `setup::tests::secrets_are_private_files_in_a_private_dir_and_kept_across_restarts` | failed at setup.rs:363 |
| state dir 0700, tightened | `setup::tests::an_open_state_dir_is_tightened` | both setup tests failed |
| Debug redaction | same test | failed at setup.rs:376 |
| outbound guard refuses jc's listener (no exception added) | `tests/outbound_guard.rs` | with a plain client in its place the test failed |
| shako webhook to jc's listener | `tests/portal_siblings.rs` | refused by shako's guard |
| ekiben stream to jc's listener | `tests/portal_siblings.rs` | the stream wrote nothing to dataroom |

The blocking sync clients panicked when built on a runtime thread in debug builds; `sync::remote::off_runtime` builds
them on a scoped thread (`the_sync_clients_build_on_a_runtime_thread`).

### End-to-end (`tests/portal_siblings.rs`)

The child binary with `--role all`, memory stores, and kaisatsu on Postgres 17 when `KAISATSU_TEST_DATABASE_URL` is
set (run both ways). `JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR=false` is a test-only setting.

| step | result |
|---|---|
| a. sign-in through kaisatsu (code + PKCE, cookies), session read back from kaisatsu's admin API | pass |
| b. person created through the Portal, read back from kaisatsu | pass |
| c. Group proposed (dry run, propose), approved by a second admin with confirm, commit read back with `git clone`, group read back from kaisatsu | pass |
| d. Pipeline on ekiben writes Entities to Antares | gap: runner refuses the Portal |
| e. public Endpoint in the dataroom catalogue | partial: the Portal's W2 + W5 calls with its token, dataset read back anonymously; the Portal's own sync needs https |
| f. dashboard in daiya | partial: provision.sh's org, folder and dashboard made as the Portal's server admin, read back with G1 and G5; jc does not run those steps per project |

### Compat gaps (refused calls)

- d, ekiben: `GET/PUT/POST/DELETE /streams/streams[/{id}]` without Authorization -> 401. The Portal has no runner
  credential setting (owner: jc-ekiben-portal).
- d, Antares: jc builds Antares without the `views` and `jc-policy` features, so `/broker/q/views` is not served.
  The Portal's Endpoint writes go through context-gateway, which is not a jc module.
- e, Portal: the CKAN URL must be https (EP-67); a CkanInstance token comes only from a `CKAN_*` env var named by
  `apiTokenRef` (no file). dataroom answers `help`/`uri` links with https on an http install.
- f, jc: nothing runs provision.sh's per-project steps (`POST /dashboards/api/orgs`, `POST /api/folders`), so the
  Portal's `GET /dashboards/api/orgs/name/{project}` answers 404 `Organization not found` until they run.
- shako: hooks take Gitea's body but shako refuses loopback targets, so the Portal polls (`JC_PORTAL_SYNC_INTERVAL`);
  the webhook secret store has no API; the mergeRequest link points at `/git/user/oauth2/keycloak?redirect_to=...`,
  which shako does not serve (owner: jc-shako-portal).
- kaisatsu: a `PUT /admin/realms/jc/users/{id}` after `reset-password` made the new password fail at sign-in; PUT
  first, then reset, works (unverified which side is wrong; owner: jc-kaisatsu-portal).
- Audience-bound tokens: the Portal's admin calls use kaisatsu `client_credentials` tokens of `portal-admin`. shako,
  ekiben, daiya, dataroom and the Antares admin still take static credentials from setup files: none of them
  verifies kaisatsu tokens in jc yet.

- Portal, fixed here: `POST /api/v1/projects` wrote the creator RoleBinding with the opener's e-mail, which
  jc-core refuses (PF-126); the reconciler dropped it (`api/projects.rs`, test in `project_open_layout_tests.rs`,
  red with the old line).
- Portal + shako: after `POST /api/v1/projects` (202) and its approval by a second administrator (202), the
  project's repository `jc/{project}` is absent (`GET /git/api/v1/repos/jc/{project}` -> 404) and
  `GET /api/v1/projects` lists no project. The UI shots therefore show the `org` namespace (owner:
  jc-shako-portal / jc-mcp-admin).

### Merged module branches

jc-shako-portal (473e07e), jc-catalog-dash-portal (144a1fb), jc-mcp-admin (e7e7bd1), merged without conflict; jc
uses `dataroom_api::portal_publisher` and `DaiyaState::with_server_admin` from them.

### Not verified

- Host mode with the Portal on its own host.
- Postgres for modules other than kaisatsu.
- Disk: the sandbox had ~14 GB free; builds ran with debuginfo off.

## Secrets (jc-secrets, owner design of 2026-11-08)

### One reader

`src/config.rs` is the only code in `jc` and the module crates that reads the environment (`process_env`, `portal_env`)
or a secret file (`Secrets::load`). Each secret comes from `NAME` (an environment value) or `NAME_FILE` (or the config
file's key naming the file); both set refuses the start. The value is a `jc_secret::Secret`
(`crates/shared/jc-secret`): `Debug`, `Display` and `Serialize` print `<redacted>`, equality is constant time, the bytes
are zeroed on drop. A module builder gets its own entries only.

| secret | `NAME` | `NAME_FILE` / file key | when neither is set |
|---|---|---|---|
| admin listener token | `JCBIN_ADMIN_TOKEN` | `JCBIN_ADMIN_TOKEN_FILE`, `admin_token_file` | start refused with `--admin-listen` |
| kaisatsu KEK | `JCBIN_KAISATSU_KEK` | `JCBIN_KAISATSU_KEK_FILE`, `kaisatsu.kek_file` | generated, 0600 file |
| shako signing key | `JCBIN_SHAKO_SIGNING_KEY` | `JCBIN_SHAKO_SIGNING_KEY_FILE` | generated per boot |
| shako service token | `JCBIN_SHAKO_TOKEN` | `JCBIN_SHAKO_TOKEN_FILE` | generated per boot, memory only |
| ekiben API token | `JCBIN_EKIBEN_TOKEN` | `JCBIN_EKIBEN_TOKEN_FILE` | generated per boot, memory only |
| dataroom Portal token | `JCBIN_DATAROOM_PORTAL_TOKEN` | `JCBIN_DATAROOM_PORTAL_TOKEN_FILE` | generated per boot, memory only |
| daiya Portal password | `JCBIN_DAIYA_PORTAL_PASSWORD` | `JCBIN_DAIYA_PORTAL_PASSWORD_FILE` | generated per boot, memory only |
| Antares admin token | `JCBIN_ANTARES_ADMIN_TOKEN` | `JCBIN_ANTARES_ADMIN_TOKEN_FILE` | memory only; a 0600 file while the Portal runs beside it |
| Portal age identity | refused (file only) | `JCBIN_PORTAL_AGE_KEY_FILE`, `portal.age_key_file` | the Portal's `JC_PORTAL_SOPS_AGE_KEY_FILE` |
| `[remote]` client secret | `JCBIN_REMOTE_CLIENT_SECRET` | `JCBIN_REMOTE_CLIENT_SECRET_FILE`, `remote.client_secret_file` | start refused with `[remote]` |
| ekiben stream variables | `EKIBEN_VAR_<NAME>` | none | none |

The start report logs one line per secret with its name and source (`env`, `file`, `generated`), never the value
(`app::report_secrets`); with `--profile production` a secret from `env` is a warning.

A realm file of `--kaisatsu-import-dir` no longer reads `jc`'s environment through `$(env:NAME)` (it read the whole
process environment before): its secrets are `$(file:...)` under `--kaisatsu-import-secrets-dir`. The standalone
kaisatsu CLI keeps reading the environment (`ApplyOptions::with_process_env`).

### Enforced by clippy

`clippy.toml` in `crates/{dataroom,kaisatsu,daiya,shako,shared}` and `bins/jc` disallows `std::env::var`, `var_os`,
`vars`, `vars_os` and `std::process::Command::new`. Children start through `jc_secret::spawn::command`: a cleared
environment plus `PATH`, `HOME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`, `SSL_CERT_FILE`, `SSL_CERT_DIR`
(jc-manifest's `git` in `assemble.rs` uses it). Tests reading their database URL carry a file-level allow.

### Fewer secrets

In one process the Portal's credentials for shako, ekiben, daiya and dataroom are made per boot in memory; a file of
an older start is removed. The Antares admin token stays a 0600 file while the Portal runs beside Antares, because the
Portal reads it per call from `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE` (`crates/portal/src/config.rs`, not ours to change).

### Project secrets (`secretRef`)

- The Portal resolves a pipeline's `secretRef` from `projects/{project}/secrets` only (`pipeline_secrets::secrets_folder`,
  T-6227). The CKAN `apiTokenRef` resolved against every SOPS file of the staged tree; `reconciler/ckan.rs`
  `project_token` now scopes it to the Endpoint's project too.
- `jc` checks the age identity at start (`app::portal_age_key`): a missing or unusable file refuses the start, naming
  the file only. It fills `JC_PORTAL_SOPS_AGE_KEY_FILE` for the Portal.
- With ekiben beside the Portal, the reconciler hands the resolved runner environment to ekiben's default organization
  (`pipeline_secrets::hand_runner_secrets_to`, `modules::ekiben::HostRunner`) instead of writing a Kubernetes Secret; a
  variable `jc` was given (`EKIBEN_VAR_*`) wins over a pipeline's.

### Role split (`src/door.rs`)

- Module container, `[door]` (`issuer`, `jwks_url`, `clients`): a bearer JWT that kaisatsu signed (ES256, RS256 or
  EdDSA, key from the JWKS, refetched at most every 30 s), with `iss` the issuer, `aud` the module's name, `exp` in
  the future and `azp` in `clients`, is swapped for the module's per-boot credential (Bearer; Basic `portal:` for
  daiya). Every other request reaches the module unchanged. No client named admits nobody.
- Portal container, `[remote]` (`token_url`, `client_id`, client secret, `modules.<name> = <base>`): each remote module
  gets a loopback forwarder; the Portal is pointed at it with a per-boot credential, and the forwarder swaps exactly
  that credential for a client-credentials token with scope and audience the module's name, cached until 30 s before
  it expires. A remote module may not also run locally; only a jc with the Portal takes `[remote]`.
- kaisatsu: one client scope per module with an `oidc-audience-mapper` to the module's name, as optional scopes of
  the caller's service-account client (`tests/split_role.rs` builds that realm).

### Production

Environment values stay visible in `docker inspect` and in the pod spec. Production gives every secret as a file
(compose `secrets:` or a mounted Kubernetes Secret) and runs kaisatsu and the Portal in their own containers (same
image, `--role`), with `[door]` and `[remote]` between them.

### Guards and their red proofs (`/workspace/evidence/jc-secrets/red-*.txt`)

| guard | test | red (guard reverted once) |
|---|---|---|
| child environment cleared | `jc-secret spawn::tests::a_child_sees_only_the_passed_variables` | failed: CARGO_MANIFEST_DIR reached the child |
| `NAME` and `NAME_FILE` both set refused | `jc-secret tests::name_and_name_file_both_set_is_refused` | failed |
| `Debug` redaction | `jc-secret tests::every_formatting_is_redacted` | failed: the value printed |
| in-process tokens off disk, new per boot | `setup::tests::in_process_service_tokens_are_not_on_disk_and_change_per_boot` | failed |
| age identity checked at start | `one_binary::a_missing_or_unusable_age_key_file_refuses_start` | failed: jc started |
| a given variable wins over a pipeline's | `modules::ekiben::tests::a_given_variable_wins_*` | failed |
| module crate reads env or spawns | clippy `-p daiya-server` with a deliberate call | 2 errors |
| door audience | `split_role` | admitted for all 5 modules: shako, ekiben, daiya, dataroom, antares |
| door client allow-list | `split_role` | shako admitted a token of `other-service` |
| forwarder swaps only the local credential | `split_role` | shako admitted another credential |
| CKAN token from its own project | `portal reconciler::ckan::tests::an_api_token_resolves_only_*` | failed: project `oulu` resolved `helsinki`'s token |
| runner environment to the host | `portal tests/host_runner_secrets_tests.rs` | failed: nothing handed |

### Gaps

- No `clippy.toml` in `crates/portal` (15 source files read the environment), `crates/antares` (34), `crates/ekiben`
  (5) or `vendor/joinedcontext-platform` (14): the Portal reads its own `JC_*` keys, Antares its `ANTARES_*`. Moving
  them onto `jc`'s reader needs each module's config to take values, starting with `crates/portal/src/config.rs`.
- `Command::new` remains in jcctl CLI commands (`changelog.rs:103`, `migrate.rs:63`, `git_bundle.rs:56`,
  `apply.rs:188`) and Antares' `build.rs`; the Portal reaches none of them.
- Database passwords: jc-pg's `password_file` is not merged yet; its value goes into `jc_secret::Secret` once it is.
- The Antares admin token remains a file beside the Portal (above).
- The CKAN `apiTokenRef.envVar` path still reads `CKAN_*` from the process environment inside jcctl (one deployment
  value for all projects, as on dev).
- The forwarder buffers bodies whole (64 MiB cap); a git push through it would need streaming.

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
  monorepo crate, e.g. `daiya-api`, `jc`), sources FAILED (`odata-*` from an Antares git rev in
  `crates/daiya/model/Cargo.toml:11`, already in the base). No root policy was added to make it pass.

## Security review of the diff

Read once over `daiya-argon-1..HEAD`, excluding the squashed imports and `vendor/`:

- Fixed while reviewing: the admin listener's `/metrics` was open, but it carries Antares' series, which the broker
  serves only behind its admin token. `--admin-listen` now needs `--admin-token-file` (`JCBIN_ADMIN_TOKEN_FILE`),
  and the bearer is compared in constant time (`app.rs` `admin_router`, `runtime::secret_eq`). Red proof: with the
  check bypassed, `slug_mode_...` got 200 where it expects 401.
- Each module keeps its own state, store, and auth. `jc-router` hands a request only to the router of the mount it
  matched; the modules share no state (only the process-wide recorder, subscriber, and crypto provider).
  - kaisatsu: KEK from a file, `LocalAuthorizer`, as standalone.
  - dataroom: `PolicyAuthorizer::default()`, as its standalone `serve`.
  - daiya: `for_standalone`, as standalone.
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
- A realm between modules (the Portal or Antares verifying kaisatsu tokens), and the Portal UI under `/portal/`.
- Antares under the ETSI suite while hosted (axum writer, no own connection cap or drain); `nest_service` is replaced
  by `fallback_service` (axum 0.8 refuses `nest_service("/")`).
- The Portal's gateway on Antares `b683bfc3`'s ngsild instead of `v0.3.0`, beyond the jc tests.
- The release binary was built but not run beyond `--version`.

## Open problems

- Slug mode puts every module on one origin (`app.rs:75`). The Portal's `__Host-` cookies are `Path=/`
  (`crates/portal/src/auth/session.rs:227`), so they ride along to every module on that host, and an XSS in any
  module could drive the Portal API same-origin. kaisatsu scopes its cookies to its mount. Host mode avoids this;
  production should use host mode at least for the Portal and kaisatsu.
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
