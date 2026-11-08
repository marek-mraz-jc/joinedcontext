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
  `nest_service("/")`; the jc-router already strips the slug). Since stage 8 built as dev builds it (`full`,
  `jc-policy`, `linkml`, `ical`, `feed`; not `parquet`, `tabular`).
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
| broker | antares | `JC_PORTAL_BROKER_URL`, `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE` | works: views, rule documents and space views written and loaded (stage 8) |
| gateway | gateway | `JC_PORTAL_GATEWAY_URL`, `JC_PORTAL_GATEWAY_CLIENT_ID=context-gateway` | works: the internal listener admits the gateway's revocation poll (stage 8) |

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
- d, Antares: closed in stage 8 (views and `jc-policy` built, context-gateway a jc module).
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

## Stage 8: context-gateway inside jc (jc-gateway)

Branch `jc-gateway` from `jc-portal-1`. Endpoints answer inside `jc` as on dev: `/api/endpoint/{slug}/...`,
`/api/endpoints`, `/api/mcp[/...]`, `/cs/...`, `/catalog*`, `/oauth/*` and the gateway's two `oauth-*` discovery
documents. The Portal keeps `/`, `/api/v1/*` and `/.well-known/oauth-protected-resource[/api/v1/mcp]`.

- Library: `context-gateway` gained `serve::{broker_of, build}` (main's wiring, errors returned instead of
  `exit(1)`), `Config::from_vars(lookup)` (`from_env` is `from_vars(std::env::var)`), `Broker::with_hop_header` /
  `stamp` (HTTP and WebSocket), and a metrics recorder that yields to an installed one. The standalone binary's
  behaviour is unchanged.
- Module `src/modules/gateway.rs` (feature `gateway`, default on). In slug mode the gateway is mounted at each path of
  `Module::root_paths` (jc-router matches whole segments, so `/catalog` is not `/catalogue`) and `whole_path` puts the
  stripped prefix back; in host mode it owns `api.{base_host}`. Settings: environment first (`JC_GATEWAY_*`, the
  shared `JC_OIDC_*` read as `JC_GATEWAY_OIDC_*`), then `vars()` from the siblings, a group filled only when its first
  key is unset and its secret is held; `portal_siblings = false` leaves the realm and the forge unfilled.

| gateway setting | from |
|---|---|
| `JC_GATEWAY_BROKER_URL` + hop header | Antares' loopback hop (below) |
| `JC_OIDC_ISSUER` (public), `JC_OIDC_JWKS_URL` (listener), `JC_OIDC_CLIENT_ID=context-gateway`, secret | kaisatsu; realm file `30-jc-gateway.json`: client_credentials only, `fullScopeAllowed` false, audience `portal-internal` |
| `JC_GATEWAY_REVOCATIONS_URL`, `_CONNECTORS_URL`, `_USAGE_URL` | the Portal's internal listener, only when `JC_INTERNAL_BIND` names a fixed `host:port` (reached on loopback); the realm group needs it too |
| `JC_GATEWAY_REPO_DIR`, `_PROJECTS_DIR`, `_ASSEMBLY_DIR` | shako `jc/config`, checked out under `{state}/gateway` with jc-manifest `GitCheckouts` every 10 s (`repo/current` swapped by one rename; layout 2 projects through `jcctl checkouts`) |
| `JC_GATEWAY_ORG_DOMAIN`, `_PUBLIC_URL`, `_PREVIEWS_DIR` | the mounts and the state dir |

- The gateway's own token comes from the issuer's public token endpoint, as the Portal's sign-in does: kaisatsu
  names the issuer by the Host a request reached it on, and the Portal's internal listener checks it (a token from
  the listener address was refused 401).
- Antares: `BrokerConfig::defaults` holds keys the host supplies when the environment leaves them unset (unknown or
  non-`ANTARES_` keys refused): `ANTARES_API_SURFACES` (dev's list without parquet/tabular), `ANTARES_POLICY=
  joinedcontext`, `ANTARES_POLICY_SUBJECT_HEADERS` (a `OnceLock` default behind the env `LazyLock`, no `set_var`),
  `ANTARES_PROVENANCE_FROM` and `ANTARES_POLICY_SUBJECT_FROM` = `context-gateway.jc`.
- Trust: on dev Linkerd sets `l5d-client-id`. Here the public `/broker` mount drops every `l5d-*` header and the hop
  header; the gateway reaches the broker through a second listener on `127.0.0.1:0` (`modules::Hop`) that answers 403
  without the per-installation secret `gateway-hop-token` (constant-time compare) and then sets `l5d-client-id` itself.
  The secret is a 0600 file, `Sensitive` in the gateway's headers, redacted in `Hop`'s Debug.
- Under the `joinedcontext` engine the broker answers data routes only to a believed subject: the admin token reads
  no Entity (`GET /broker/ngsi-ld/v1/entities/{id}` with it: 403 "a request without a subject reaches the Context
  Source operations only"), anonymous `/broker/ngsi-ld/v1/info/sourceIdentity` is 401, as on dev; `/q/health` stays
  open (the one_binary probe moved there).

### End-to-end (`tests/gateway_endpoints.rs`, child binary, `--role all`)

A realm file in kaisatsu's import directory adds a person in `e2e-readers` and `e2e-writers`, a client whose tokens
name the closed Endpoint and one naming another slug (password grant, secrets through `$(env:...)`). The test writes
project `e2e` (layout 1: project, space `things`, a `public` and a `project-list` Endpoint, Policies `public-read`
(role public), `readers`, `writers`) to shako's `jc/config` through the contents API, then:

| step | result |
|---|---|
| public Endpoint `GET .../ngsi-ld/v1/types` and `entities?type=Thing` anonymous | 200 |
| closed Endpoint anonymous | 401 |
| closed Endpoint with the person's token | 200, the Entity |
| `POST .../entities` of a granted type by a writer | 201, tenant `things` in `/broker/q/tenants` |
| `POST` of a type the Policy does not grant | 403 |
| the person's token for another Endpoint | 401 |
| `/broker/ngsi-ld/v1/entities` with `l5d-client-id: context-gateway.jc`, `x-jc-client`, `x-jc-subject`, `x-jc-groups` | no Entity |
| `POST /api/mcp` `tools/list`, `tools/call query_entities` anonymous | 200, the Entity |
| the gateway's revocation poll | admitted (no 401 in 25 s) |

### Guards and their red proofs (stage 8)

| guard | test | red (reverted once) |
|---|---|---|
| public mount drops `l5d-*` and the hop header | `modules::antares::tests` (2) | both failed |
| hop answers 403 without its secret | `only_the_hop_with_its_secret_names_the_gateway_to_the_broker` | failed at antares.rs:269 |
| public mount layer in place | `gateway_endpoints` | "the broker believed a claimed identity: 200 OK [Thing:one]" |
| anonymous only on a public Endpoint | `gateway_endpoints` | closed Endpoint answered 200 anonymous |
| a token for another Endpoint | `gateway_endpoints` | answered 200 |
| hop header replaces a client's | `proxy::tests::the_hop_header_replaces_a_clients_and_never_prints` | failed at proxy.rs:464 |
| outbound guard refuses jc's listener | `tests/outbound_guard.rs` | unchanged, green |

### Gates (stage 8)

- `jc` (after merging jc-portal-1 `c0a410a7`): fmt, clippy `--all-targets -D warnings --locked`, tests: lib 48, binary 4, one_binary 12, outbound_guard 1,
  gateway_endpoints 1, portal_siblings 1 (kaisatsu on Postgres 17 and on memory): green.
- context-gateway: the vendored copy cannot build its own tests (no `tools/model-tools`, dev-dependencies outside
  the root lock), so its gates ran in a clone of joinedcontext-platform at `e59ceb9f` with the same patch (files
  compared byte for byte): fmt, clippy `--all-targets -D warnings --locked`, `cargo test --locked`: 1839 passed,
  0 failed, 4 ignored.
- Antares workspace (`antares-broker`, `antares-api`): fmt, clippy `--all-targets --all-features -D warnings`
  (the preflight lints), `cargo doc --document-private-items -D warnings`, tests: 1555 passed in 53 binaries, 0
  failed (Postgres tests skip without `ANTARES_TEST_DATABASE_URL`; no store code changed).

### Gaps left (stage 8)

- Portal UI reads of a closed Endpoint: the UI calls `GET /api/endpoint/{slug}/ngsi-ld/v1/...` with its session
  cookie only; the gateway reads `Authorization` alone (`auth/subject.rs` `admit_on`), so in jc that call is
  anonymous: 401 on a closed Endpoint. On dev APISIX's openid-connect plugin turns the session into a bearer.
- No Portal-made token names an Endpoint: the Portal logs `hub scopes wait for the realm scope=endpoint:{slug}
  ... when the realm's admin-permissions CronJob links it to the hub`; no jc step links them, and kaisatsu has no
  per-Endpoint audience mapper. A person's Portal token on a closed Endpoint is 401; the e2e uses an imported client
  with an audience mapper.
- The gateway's own token comes from the public issuer URL: a jc whose public origin the process cannot reach
  (no hairpin through the TLS proxy) gets no token, and the Portal's internal listener refuses its polls (401).
- `JC_INTERNAL_BIND` unset (jc-portal-1 `c0a410a7`: the Portal binds a free loopback port and `portal::build`
  drops the address `server::spawn_internal_listener` returns): the gateway gets no realm, because a gateway with
  a realm must poll the revocations (`JC_GATEWAY_REVOCATIONS_URL`), so every token on an Endpoint is 401 and a
  warning names the setting; public Endpoints answer. Fix for the owner of `modules/portal.rs`: hand that address
  to `jc` (built before the gateway), or keep a fixed default.
- Not filled from siblings: `JC_GATEWAY_PREVIEWS_URL` (workspace previews), `_ACTIVITY_URL`, `_ASSISTANT_URL`,
  `_DOMAIN_VERIFICATIONS_URL`, `_INTERNAL_BIND` (no `/internal/mqtt/auth`; jc runs no MQTT broker).
- OGC `f=parquet|csv|zip`: features `parquet` and `tabular` are not built.
- Layout 2 organizations: `jcctl checkouts` runs after the org checkout but is unexercised (shako's project
  repository creation, jc-portal-integrate report).
- Host mode e2e: only the one_binary probe (`api.city.test/api/endpoints`).

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
