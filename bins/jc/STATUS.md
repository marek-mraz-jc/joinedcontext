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
| `JC_SHAKO_*` | shako | `URL`, `PUBLIC_URL`, `OWNER`, `REPO` (`setup::forge_repo`: the environment's, else `jc`/`config`; the gateway checks out the same), `TOKEN`, `TOKEN`, `APPS_OWNER=jc-apps`, `APPS_TOKEN`, `WEBHOOK_SECRET` | works: proposal commits, approval merges, sync reads them back |
| pipeline runner | ekiben | `JC_PIPELINE_RUNNER_URL` | wired, refused: no runner credential (gap d) |
| CKAN | dataroom | `JC_CKAN_URL`, `JC_CKAN_API_URL`, only when the public URL is https (EP-67) | wired for https installs; the http e2e leaves it unset (gap e) |
| Grafana | daiya | `JC_GRAFANA_URL`, `_PUBLIC_URL`, `_USER=portal`, `_PASSWORD` | works as server admin; the Portal makes each opened project's org and folder (`grafana::spawn_provisioning`, jc-charts) |
| broker | antares | `JC_PORTAL_BROKER_URL`, `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE` | works: views, rule documents and space views written and loaded (stage 8) |
| gateway | gateway | `JC_PORTAL_GATEWAY_URL`, `JC_PORTAL_GATEWAY_CLIENT_ID=context-gateway` | works: the internal listener admits the gateway's revocation poll (stage 8) |
| Model Tools | model-tools | `JC_PORTAL_MODEL_TOOLS_URL` (its own loopback listener; no public mount) | works for the views: `POST /generate` gives `brokerSchema` and `context`, the reconciler writes `/q/schemas/{tenant}` and Endpoints serve OGC and OData; the save path's `jsonSchema`, `docs` and `example` are not rendered (evidence/jc-model-tools) |

The Portal's forge client reads its `JC_SHAKO_*` through `Config::forge_var` (environment first, then the values jc
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
| f. dashboard in daiya | pass: an opened project's org and folder are made by the Portal (step g, jc-charts), dashboards read back through the Portal |

### Compat gaps (refused calls)

- d, ekiben: `GET/PUT/POST/DELETE /streams/streams[/{id}]` without Authorization -> 401. The Portal has no runner
  credential setting (owner: jc-ekiben-portal).
- d, Antares: closed in stage 8 (views and `jc-policy` built, context-gateway a jc module).
- e, Portal: the CKAN URL must be https (EP-67); a CkanInstance token comes only from a `CKAN_*` env var named by
  `apiTokenRef` (no file). dataroom's `help`/`uri` links follow the configured public URL (http on an http
  install) since jc-rest item 4.
- f, closed: the Portal's `grafana::spawn_provisioning` (jc-charts) runs provision.sh's per-project steps.
- shako: hooks take Gitea's body but shako refuses loopback targets, so the Portal polls (`JC_PORTAL_SYNC_INTERVAL`);
  the webhook secret store has no API (owner: jc-rest item 3, waits for jc-shako-odata). The mergeRequest link
  `/git/user/oauth2/keycloak?redirect_to=...` is served by shako-web (sign-in, then the redirect).
- kaisatsu: `PUT /admin/realms/jc/users/{id}` after `reset-password` does not reproduce on jc-rest: kaisatsu keeps
  the credential as Keycloak does (jc-rest item 2, kaisatsu-server `portal_contract::a_profile_write_after_a_password_reset_keeps_the_new_password`).
- Audience-bound tokens: the Portal's admin calls use kaisatsu `client_credentials` tokens of `portal-admin`. shako,
  ekiben, daiya, dataroom and the Antares admin still take static credentials from setup files: none of them
  verifies kaisatsu tokens in jc yet.

- Portal, fixed here: `POST /api/v1/projects` wrote the creator RoleBinding with the opener's e-mail, which
  jc-core refuses (PF-126); the reconciler dropped it (`api/projects.rs`, test in `project_open_layout_tests.rs`,
  red with the old line).
- Portal + shako, closed (99a59bf): an opened project lives in `config/projects/{project}/project.yaml` of the
  organization repository, and the Portal's `Mirror::namespaces()` now counts its `Project` manifest, so
  `GET /api/v1/projects` lists it (e2e step c2). The organization's agent pages still withhold a project the
  caller does not read (`authz_matrix_tests::the_other_member_sees_no_agent_and_no_run_of_a_project_it_does_not_read`).

### Merged module branches

jc-shako-portal (473e07e), jc-catalog-dash-portal (144a1fb), jc-mcp-admin (e7e7bd1), merged without conflict; jc
uses `dataroom_api::portal_publisher` and `DaiyaState::with_server_admin` from them.

### Not verified (stage 7)

- Host mode with the Portal on its own host.
- Postgres for modules other than kaisatsu: done in Stage 8.
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
| `JC_GATEWAY_REVOCATIONS_URL`, `_CONNECTORS_URL`, `_USAGE_URL` | the Portal's internal listener at the address it took (a free loopback port unless `JC_INTERNAL_BIND` names one): jc builds the Portal first and hands the address on; the realm group needs it too |
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
| the person disabled in the Portal, the same token on the closed Endpoint | 401 within one poll (4.9 s) |

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

## Stage 8b: every module on Postgres (jc-pg)

Each module takes an optional `[<module>.database]` section (`src/pg.rs`). Without it the module keeps its memory
store (the default); with it the module never falls back to memory: a failed connection, a failed migration or a
failed boot RLS check stops `jc` with exit 1.

```toml
[shako.database]                 # also kaisatsu, daiya, dataroom, ekiben, antares
url = "postgres://shako_app@db:5432/jc"            # request role; a password in the URL is refused
password_file = "/run/secrets/shako-app-password"
max_connections = 5              # optional, default 5
migrate_url = "postgres://shako_migrate@db:5432/jc"  # optional: migrate at start as this owning role
migrate_password_file = "/run/secrets/shako-migrate-password"
```

- Migrations run as `migrate_url`'s role under the module's advisory lock (sqlx `Migrator` for shako, ekiben,
  kaisatsu; daiya's and dataroom's own `pg_advisory_lock`), then grant the request role. Without `migrate_url` the schema must be
  migrated already; the boot check refuses a missing table. Antares migrates itself as its request role (its
  design) and refuses a `migrate_url`.
- Boot checks run inside jc as standalone: the request role must be neither SUPERUSER nor BYPASSRLS, own no table
  of the module's schema, and every table there has forced RLS and a policy. Antares runs with `require_rls`
  forced whenever it has a database.
- shako keeps every record (repos, PRs, issues, counters...) through an op log `shako.store_log` (forced RLS)
  replayed into its mirror at start; its repositories live under `<state_dir>/shako`, no longer a temp dir.
- ekiben saves continuous stream definitions as posted (before `${VAR}` interpolation, so no filled secret is
  stored) in schema `ekiben` through `ekiben_server::StreamPersistence`, and starts them again at boot. A refused
  save refuses the create; a refused delete keeps the stream.
- Antares on `[antares.database]` switches `ANTARES_STORE=memory` to postgres and refuses `ANTARES_BUS=local`
  unless `ANTARES_ALLOW_SHARED_LOCAL=1` (one jc process).
- kaisatsu keeps its legacy flat keys (`database_url_file`); a section together with them is refused.

### Postgres connection budget (pool max per module)

| Module | Connections jc opens | Notes |
|---|---|---|
| kaisatsu | `max_connections` (default 5, clamped 1..=50) + 1 | the +1 is the LISTEN connection of its change listener |
| shako | `max_connections` (default 5, clamped 1..=20) | outbox listener not spawned in jc |
| daiya | `max_connections` (default 5) | |
| dataroom | `max_connections` (default 5) | |
| ekiben | `max_connections` (default 5) | |
| antares | `max_connections` if set, else `ANTARES_PG_POOL` (default 20) | `ANTARES_PG_WRITE_RESERVE` is carved out of the pool, not added |
| at start | +1 per module with `migrate_url` | closed after the migration |

With every default: 5+1 + 5 + 5 + 5 + 5 + 20 = 46 connections per jc process, plus up to 5 transient migration
connections at start. The e2e measures it per role from `pg_stat_activity` at `max_connections = 3`.

### Operator preparation (not done by jc)

- One login role per module for requests (`NOSUPERUSER NOBYPASSRLS`) and, for migrations, an owning role with
  `CREATE` on the database. daiya's and Antares' migrating roles create tables in `public`: on Postgres 15+ grant
  them `CREATE ON SCHEMA public`.
- Antares needs the extensions `postgis`, `btree_gin` and `unaccent`, created by a superuser.

### End-to-end (`tests/restart_pg.rs`, `JC_PG_TEST_DATABASE_URL` = a superuser URL; prints SKIP without it)

On postgis/postgis:17-3.5: `jc --role all` writes a shako repo + commit, a daiya dashboard, a dataroom dataset, an
ekiben stream, a kaisatsu user and an Antares entity through the public mounts, stops, starts again and reads all
six back. Red proof: each module switched to memory in turn fails the read-back. Also: a wrong password stops jc
(exit 1), and a SUPERUSER request role is refused by every module (red proofs: Antares without `require_rls`,
ekiben without its boot check).

### Gaps

- shako webhook secrets stay in a memory secret store and are lost on restart (re-enter them); the signing key is
  generated per start unless configured.
- ekiben run-once streams, org quotas and resources are not persisted.
- shako's op log grows without compaction; replay sets `updated_at` to the replay time.

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

Database passwords (jc-pg's `[<module>.database] password_file` and `migrate_password_file`) are
read through the same reader (`jc_secret::read_file`) at connect, held as `jc_secret::Secret`, and
listed in the start report as `<module>-db-password` / `<module>-db-migrate-password`, source `file`.
Antares gets its URL with the password as a `Secret` (`DatabaseSection::app_url_with_password`).
The Portal's age identity is the operator's `--portal-age-key-file` (checked at start) when given,
else the one setup generates (`portal-age-key`, an age identity in a 0600 file).

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

### Endpoint scopes (`src/modules/kaisatsu.rs`)

The Portal's token exchange for a data read (`jc_data_query`, audience `mcp-hub`, scope
`gateway-hub endpoint:{slug}`) needs `endpoint:{slug}` as an optional scope of `portal-api`. On
dev the deployment's keycloak-admin-permissions-sync CronJob links it as the realm admin; jc's
kaisatsu does the same in process every 5 s: every Portal-made `endpoint:` scope (marked
`managed-by: joinedcontext`, no protocol mapper) becomes an optional scope of `mcp-hub`, `jcctl`
and `portal-api` (whichever exist); an unmarked, removed or default-linked `endpoint:` scope is
unlinked, and `endpoint:*` stays only as the hub's default. No realm-wide optional scope (T-6301).
`tests/gateway_endpoints.rs` reads the closed Endpoint through `/api/v1/mcp` as a member, and a
person in no group gets the exchanged token and is still refused by the gateway's Policy.

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
- The Antares admin token remains a file beside the Portal (above).
- The CKAN `apiTokenRef.envVar` path still reads `CKAN_*` from the process environment inside jcctl (one deployment
  value for all projects, as on dev).
- The forwarder buffers bodies whole (64 MiB cap); a git push through it would need streaming.

## Stage 9: merge round on jc-portal-1 (jc-portal-integrate, 2026-11-08)

Merged into `jc-portal-1`, each followed by the jc gates (fmt, clippy, lib/bins, one_binary, outbound_guard,
gateway_endpoints, portal_siblings on memory and Postgres 17, restart_pg on PostGIS 17) and the Portal gates
(fmt, clippy, `--lib`, authz_matrix, route_coverage, openapi, `pnpm build`, `pnpm lint`):

| branch | merge | conflicts and fixes |
|---|---|---|
| jc-gateway 25bfe59 | 2d9a2259 | the Portal's internal listener address goes to the gateway module (`portal_internal`); e2e: a disabled person's token is refused by the closed Endpoint within 25 s (4.9 s measured) |
| jc-pg 5ffda4b5 | 0659273c | 11 files: jc-pg's pg wiring kept, BrokerConfig keeps the gateway defaults; restart_pg writes with the Antares admin token under `ANTARES_POLICY=allow-all` (storage only) |
| jc-rest 3170a0df | e9a50006 | the Portal and the gateway built in one background task after jc serves; host-aware test cookie jar |
| jc-charts dab4a277 | d2837531 | `server.rs`: jc-charts' `spawn_provisioning` replaces this branch's duplicate `spawn_organizations` |
| jc-shako-odata 603229b4 | 5ed3bf22 | none; jc fills `JC_SHAKO_*` (was `JC_GITEA_*`), hooks name `/api/v1/webhooks/shako` |
| jc-shako-ui edcb53ba | 8243e19e | `ops/mod.rs` both registries, four locales both blocks, MCP byte ratchet 73,062; `ops/repositories.rs` moved from the deleted `GiteaClient` to `ShakoClient` |
| jc-secrets 04ccea80 | (this merge) | `gateway.rs` takes the in-memory shako token and this branch's `forge_repo`; `provision.rs` reads through `config::portal_env`; the ekiben bearer goes to the Portal by value (below) |

Fixes in this round, each with a red proof under `/workspace/evidence/jc-portal-integrate/red/`:

- The gateway checked out `jc/config` whatever `JC_SHAKO_OWNER`/`JC_SHAKO_REPO` named (jc-smoke-mcp: 0 Endpoints
  on `joinedcontext/configuration`). `setup::forge_repo` is the one reading for the gateway and provisioning;
  `gateway_endpoints` runs on `e2e-city/configuration` (`gateway-forge-repo.txt`).
- daiya in jc refused every datasource on jc's gateway (default outbound policy). `[daiya] provisioning_dir` loads
  Grafana provisioning files at start; only the datasources those files wrote reach jc's listener, by its exact
  IP literal and port, while the stored record matches the files. API-made or changed datasources, another port
  and a host name stay refused (`crates/daiya/server/tests/own_listener.rs`, `daiya-own-listener.txt`).
  Provisioning writes organization 1 only, so per-project organizations cannot be file-provisioned yet.
- The Portal called jc's ekiben without its bearer, so stream deletes got 401 (jc-migrate's rehearsal, G15). The
  Portal now sends `JC_PORTAL_PIPELINE_RUNNER_TOKEN` (or the file `JC_PORTAL_PIPELINE_RUNNER_TOKEN_FILE` names,
  read at start) on every runner call, through its own client only; jc hands it the in-memory ekiben token
  (`modules::ekiben::tests::a_portal_issued_delete_reaches_ekiben`, `ekiben-runner-bearer.txt`).
- Antares `q_admin_sweep` and `dev/new-surface.py` named `antares-broker/src/main.rs`, whose mounts 6387a8da moved
  to `lib.rs` (`antares-broker-lib-paths.txt`).

Open: the dataroom catalogue is empty in jc (jc-smoke-endpoints gap 2: `package_show` 404 for every published
dataset), step e above. Not measured this round. Two causes are known from the code: on an http install jc fills
no `JC_PORTAL_CKAN_URL` unless `JC_PORTAL_CKAN_ALLOW_HTTP=true`, so the Portal's catalogue wave publishes nothing
(`the CKAN wave finished reports=0` in the e2e log); and a migrated `CkanInstance` names dev's CKAN URL and an
`apiTokenRef` that must resolve to dataroom's Portal token (`portal_publisher`) through jc's SOPS key or a
`CKAN_*` variable. Next step: one smoke run with `JC_PORTAL_CKAN_ALLOW_HTTP=true` and the CkanInstance pointed at
jc's dataroom, reading the wave's report.

## Stage 10: the knowledge assistant inside jc (jc-assistant-module, G2, G7, G18)

- Module `src/modules/assistant.rs` (feature `assistant`, default on; it needs `gateway`). The library half moved
  out of the `jc-assistant` binary into `assistant::host` (settings through one lookup, pool options, probe, the
  minute's worker); the binary keeps reading its environment.
- Public paths: `/d/...` and `/api/v1/d/...` only, as dev's edge routes (slug mode at the root, host mode on
  `assistant.{base_host}`). Everything else (the Portal's `/internal/v1/projects/...` administration paths, the
  gateway's `/internal/v1/search`) answers only on the module's own loopback listener, the sibling's internal
  address. `public_only` refuses the rest with 404.
- Settings: `JC_ASSISTANT_*`, `JC_MODEL_BASE`, `JC_MODEL_PROVIDER`, `ORT_DYLIB_PATH` from the environment first
  (`config::assistant_env`), then `modules::assistant::vars` from the siblings (gateway, kaisatsu client
  `jc-assistant`, the Portal, the gateway's shako checkout for `JC_ASSISTANT_REPO_DIR`). Database:
  `[assistant.database]` (request role; it migrates as that role so forced RLS holds; `migrate_url` is refused).
- The model proxy (`jc-agent-proxy`) runs on a loopback port with the key from `JCBIN_MODEL_KEY_FILE` /
  `[assistant] model_key_file`, read by `jc_secret::read_file` and set on the proxy's config field: no lookup, no
  environment, no log line carries it. `JCBIN_MODEL_KEY` and `JC_MODEL_KEY` as values refuse the start
  (`config::tests::the_model_key_is_a_file_only_and_its_value_never_shows`). An explicit `JC_ASSISTANT_PROXY_URL`
  uses that proxy instead. Client secrets `assistant-client-secret` and `proxy-client-secret` come from setup;
  the realm gains both clients and portal-api's `jc-assistant` audience (`setup::add_assistant_audience`).
- Off: no model dir, no ONNX Runtime, no key file and no proxy URL, no `[assistant.database]`, a missing
  sibling or secret, or any build failure (DB, migrations, model load) logs one `assistant: off` warning with the
  reason; the mount answers 404 and the module leaves the sibling table, so the Portal gets no
  `JC_PORTAL_KNOWLEDGE_URL` and dataroom no launcher. jc keeps running.
- Portal: `siblings::portal_vars` fills `JC_PORTAL_KNOWLEDGE_URL` with the module's loopback listener (G18).
- dataroom: `BrandingBlock::assistant_widget` (serde-skipped, set only by the host) names the widget page
  `{assistant}/d/{catalogue_deployment}/widget` or `""` (off); the CSP's frame-src gains that origin when it is
  cross-origin. The launcher corner is on every page with a footer (about and resource pages added).
- `[assistant] catalogue_deployment` (default `data`): this jc's https dataroom origin joins that deployment's
  `allowedOrigins` at run time (`Minute::extra_origins`).
- kaisatsu introspection answers `username` as Keycloak does: agent-proxy and the assistant admit service
  accounts by it (`token_endpoint::client_credentials_refusals_and_introspection_and_revocation`).
- Image: `bins/jc/Dockerfile` fetches PDFium, e5-small and ONNX Runtime with the platform's pinned scripts and sets
  `JC_ASSISTANT_MODEL_DIR` and `ORT_DYLIB_PATH`.

## Release binary

`cargo build --release -p jc` (default release profile, no LTO, all default features incl. the Portal's embedded UI):
196,455,176 bytes, 151,883,704 bytes stripped. Built in 9 min.

## Dependency checks (root lockfile = everything `jc` links; 2026-11-08, jc-ship)

Root `deny.toml` and `cargo audit` carry no ignore. Fixed on branch `jc-ship`:

- Wildcard path dependencies: every root workspace crate has `publish = false`.
- `quick-xml 0.37.5` (RUSTSEC-2026-0194, -0195, CVSS 7.5): `vendor/oxrdfxml` is oxrdfxml 0.1.7 ported to
  quick-xml 0.42 (`vendor/SOURCES.md`).
- `basic-toml` through askama: askama without its `config` feature.
- `paste` (RUSTSEC-2024-0436): `vendor/tikv-jemalloc-ctl`, upstream commit 66b124bdf4 without paste.
- `crypto-bigint 0.5.5` through shako-core: p256 0.14 (crypto-bigint 0.7.10).

Still red, with no fixed release to take:

- RUSTSEC-2026-0444 `astral-tl 0.8.0` (unmaintained since 2026-10-05, no patched release) <-
  `html-to-markdown-rs 3.17.1` (its HTML parser, not optional) <- `kreuzberg 4.10.4` feature `html` <-
  `assistant` (vendor/joinedcontext-platform/crates/assistant), which reads every crawled page through it
  (`extract::extract`). The platform workspace carries the same finding. Clears with a kreuzberg whose
  html-to-markdown-rs moves to `astral-html`, or an owner decision (no ignore is added here).
- RUSTSEC-2026-0387 `crypto-bigint 0.5.5` <- `elliptic-curve 0.13.8` <- `p256 0.13.2` (and `hpke 0.12.0`,
  `ecdsa`, `primeorder`) <- `age-core 0.12.0` / `age 0.12.1` <- `jcctl` (vendor/joinedcontext-platform) and
  `joinedcontext-portal` <- `jc`. age 0.12.1 (2026-07-14) is the newest release and requires hpke 0.12 with p256;
  rage main is on p256 0.14 / hpke 0.14, unreleased; age 0.11.5 fails deny on proc-macro-error2. jcctl
  (`src/secrets/sops.rs`) and the Portal (`src/secret_files.rs`) use only `age::x25519::Identity`,
  `age::armor::ArmoredReader` and `age::Decryptor`; an x25519 identity unwraps only X25519 stanzas, so no p256
  code runs when they decrypt. Take the next age release.
- RUSTSEC-2026-0374 `basic-toml 0.1.10` (unmaintained) <- `i18n-config 0.4.8` <- `i18n-embed-fl 0.10.1`
  (proc-macro, unconditional dependency of age) <- `age 0.12.1` <- the same chain. Build time only.
- RUSTSEC-2023-0071 `rsa 0.9.10` (Marvin, no fixed release) <- `openidconnect 4.1.0` <- `joinedcontext-portal`
  <- `jc`. The timing leak is in RSA private-key operations. openidconnect calls rsa for signature
  verification with the issuer's public key (`core/crypto.rs` `verify_rsa_signature`, `BigUint` n and e) and,
  in `CoreRsaPrivateSigningKey` (`core/jwk/mod.rs:541`), for signing with a private key. It has no RSA
  decryption: an encrypted ID token is refused as `Unsupported` (`verification/mod.rs:303`). The Portal never
  constructs `CoreRsaPrivateSigningKey`; the only `RsaPrivateKey` in our sources is the test key of
  `crates/portal/src/auth/bearer.rs:1106` inside `#[cfg(test)]`. The vulnerable path is unreachable; the
  advisory stays red until openidconnect moves to rsa 0.10 or drops it. kaisatsu keeps openidconnect out for
  this reason (`crates/kaisatsu/server/Cargo.toml:51`).
- `cargo audit` also warns on `atomic-polyfill` (unmaintained, RUSTSEC-2023-0089) from a `heapless 0.7.17` lock
  entry that no target of the graph builds (`cargo tree -i heapless@0.7.17 --target all` prints nothing); a
  warning, not a failure.

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

- The Portal's own Postgres (`JC_*`): untested inside jc. The other modules run on Postgres since Stage 8.
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
- The three advisories still open in the dependency checks above.
- gitleaks over the branch reports 73 findings, all in upstream test fixtures and docs of the imported trees and
  `vendor/` (JWT/PEM test vectors, sample keys), none in `bins/jc`. These repositories have their own allow-lists
  upstream.
- 2 deprecation warnings in vendored `jcctl` (`vendor/joinedcontext-platform/crates/jcctl/src/secrets/sops.rs:635`),
  upstream code.
