# Portal calls to its pipeline runner

Every HTTP call `crates/portal/src` makes to the pipeline runner (Bento 1.22 `/streams` API, ekiben in `jc`), from
branch `jc-one-binary-2` (b978ad1bf). Paths are relative to `crates/portal/src/`; test modules excluded.
`tests/portal-contract` and `crates/ekiben-server/tests/portal_contract.rs` check ekiben's answer to each.

## 1. Configuration

- Runner URL: `JC_PORTAL_PIPELINE_RUNNER_URL` (`config.rs:109`, read at `config.rs:1792`), a template where
  `{project}` becomes the project name; every caller does `.replace("{project}", p).trim_end_matches('/')`.
  `StreamDeployer::runner_for` (`reconciler/streams.rs:578`) refuses a project name that is not DNS-1123.
- Wiring: `state.rs:832-875` `wire_streams`: `StreamDeployer::new(url)`, `with_pipeline_identity(config.pipeline_identity)`,
  `with_concurrency`, `with_run_ends`, `with_validation(pipeline_test_capture_url, model_schemas)` when the internal
  listener address is set, then `spawn_queue()`. No URL: pipelines stay Pending (`reconciler/daemon.rs:1985-1989`).
- Client A (`StreamDeployer`, `reconciler/streams.rs:334-337`): reqwest, 30 s timeout; stream PUT/POST/DELETE override it
  with 180 s (`REPLACE_TIMEOUT`, line 33).
- Client B (`api::pipelines::http()`, `api/pipelines.rs:290-299`): 2 s timeout, `pool_max_idle_per_host(0)`; metrics,
  rejected replay, source deletes and pipeline tests.
- Bodies: JSON only (`reqwest .json(..)`, `Content-Type: application/json`). Stream id: `{project}.{name}`
  (`stream_id`, `reconciler/streams.rs:142`).
- **Auth to the runner: none.** No call sets `Authorization`. ekiben in `jc` refuses every request without a token
  (`require_auth: true`). ekiben now admits two: the static admin token (`JCBIN_EKIBEN_TOKEN_FILE`) and, when the host
  sets `ServerAuthConfig::oidc`, a kaisatsu JWT for audience `ekiben` whose `azp` is an admitted client
  (`OidcAuth`, `crates/ekiben-server/src/oidc.rs`). The Portal must still send one of them on every runner call (gap,
  Portal side), and `jc` must set `oidc` (gap, `bins/jc/src/modules/ekiben.rs`).
- **Writes to the in-process broker.** Streams refuse loopback targets. A host lets them reach exactly its own listener
  under named mounts with `ResourceManager::with_host_target(HostTarget { addr, path_prefixes })`: plain HTTP to that
  literal address, whole path segments, no redirect followed; every other loopback or private address and every other
  mount stays refused (`a_stream_reaches_the_hosts_broker_mount_and_no_other_loopback_target`). `jc` must pass its
  listener and the broker mount (gap, `bins/jc/src/modules/ekiben.rs`).

## 2. StreamDeployer (reconciler)

| # | file:line | Call | Portal reads | Purpose |
|---|---|---|---|---|
| 1 | `reconciler/streams.rs:1321-1338` `running()` | `GET /streams` | 2xx else "no list"; `HashMap<String, Value>`, only `state.active` (missing = running) | once per project per pass; also `retire_unqualified` (line 1278) |
| 2 | `reconciler/streams.rs:1105-1136` `holds_as_rendered()` | `GET /streams/{p}.{n}` | non-2xx = not held; `active` (default true), `config` compared to the render with `${VAR}` / `${VAR:d}` as wildcards (`held_as_rendered`, line 274) | adopt a running stream after a Portal restart |
| 3 | `reconciler/streams/identity.rs:157-185` `declare_egress()` (from `reconciler/streams.rs:1355`) | `POST /resources/rate_limit/{p_}__pipeline_egress` body `{"local":{"count":500,"interval":"1s"}}` | non-2xx = stream Error; transport error = back-off | before every stream create (deploy, replay, test) |
| 4 | `reconciler/streams.rs:1358-1367` `deploy_stream()` | `PUT /streams/{p}.{n}` (rendered JSON) | 2xx Live; **404 retries as POST**; other = `runner answered {status}: {body ≤ 500}`; transport = back-off 60 s doubling to 15 min | resident stream, expiry sweep, scheduled run start (`Runner::start`, line 1446) |
| 5 | `reconciler/streams.rs:1369-1378` | `POST /streams/{p}.{n}` | as 4 | create after a 404 |
| 6 | `reconciler/streams.rs:1212-1262` `remove()` | `DELETE /streams/{p}.{n}` | 2xx or 404 = gone; other = warning; timeout = "held, stopped in name only" | retire; run queue delete (`Runner::delete`, line 1492; `reconciler/run_queue.rs:913, 922, 933`) |
| 7 | `reconciler/streams.rs:1287-1308` `retire_unqualified()` | `DELETE /streams/{n}`, `DELETE /streams/{n}.expiry` | 2xx or 404 | once per project, ids from before T-3002 |
| 8 | `reconciler/streams.rs:1453-1469` `Runner::state()` | `GET /streams/{p}.{n}` | **404 = Gone**; 2xx with `active == false` = Finished; other 2xx = Running; error = Running until 15 min (`run_queue.rs:27`) | the run queue polls every 5 s |
| 9 | `reconciler/streams.rs:548-556` `metrics()` | `GET /metrics` | 2xx body text (Prometheus) | stall detection (`reconciler/daemon.rs:1879`); `dropped_reports()` (line 560) sums `output_sent{label="outcome_drop",stream="<id>"}` |

Converge sends at most 8 streams at once (`SENDS_AT_ONCE`, line 38).

## 3. Metrics route

`api/pipelines.rs:472-523` `metrics_for()`: `GET /metrics`, client B. Callers: `GET /api/v1/projects/{p}/pipelines/{n}/metrics`
(line 457), `.../stats` (line 542-553), `ops/views.rs:374`, `api/assistant.rs:473`, `api/internal/agent_runs.rs:99`. Any failure
is 503. `scrape(body, "{p}.{n}", now)` (line 195) keeps lines with `stream="<id>"` and matches families by suffix, a `bento_`
prefix and `_total` suffix ignored (lines 98-117): `input_received`, `output_sent`, `output_error`, `processor_error`,
`buffer_backlog`, `output_latency_ns`, `processor_latency_ns`, `*_latency_ns_bucket`.

## 4. Rejected replay

`api/pipelines.rs:1063` `retry_rejected`: `declare_egress` (line 1180), `POST /streams/rejected-replay-{slug}` (line 1183;
only 2xx checked, anything else returns the records and 503), `DELETE` of it after 15 s (`REPLAY_WINDOW`, lines 1041, 1216-1221).
Config `replay_stream()` (line 1254): `generate{count:1, interval:""}` of base64 records, unarchive / split 1000 / archive,
`http_client` upsert output (`gateway_output`), validation stage, outcomes, own credential, pipeline identity.

## 5. Source deletes

`pipeline_source_deletes.rs:1197` `launch`: `POST /streams/{p}.{n}.source-deletes.{u64:016x}` (line 1254, URL at 1271-1278;
2xx required), `DELETE` after 120 s (`STREAM_WINDOW`, line 40). No egress declared. Config `render()` (line 1304):
`generate{count:1}` of id chunks, `http` POST `.../entityOperations/query?options=sysAttrs,keyValues&limit=100`, `http`
POST `.../entityOperations/delete` with `JC-Reason`, `http_client` POST `{internal}/internal/pipelines/{p}/{n}/source-deletes`.

## 6. Pipeline test, DataSource test, streaming sample

`api/pipeline_test.rs:470` `run_stream_within()` (callers `api/pipeline_test.rs:345, 412`, `api/datasource_test.rs:250`,
`ops/pipeline_steps.rs:601`), stream `pipeline-test-{p}` (line 511), client B:

| file:line | Call | Portal reads |
|---|---|---|
| 495-503 | `POST /resources/rate_limit/{p_}__pipeline_egress` | any refusal = 503 |
| 606-617 `create()` | `POST /streams/pipeline-test-{p}` | 2xx: collect posts for 3 s; **4xx: body split into lint errors**, one per line, `line N` parsed (`jcctl::pipeline_test::lint_errors`); 5xx: 503 |
| 620-622 `held()` | | **400 with body exactly `Stream already exists`** = still held |
| 627-640 `left_over_removed()` | `GET /streams/pipeline-test-{p}` | `uptime` (f64) ≥ 60 s: `DELETE` (2xx required), create again; else 409 |
| 523-529 | `DELETE /streams/pipeline-test-{p}` | 2xx or 404 |

## 7. The runner calling the Portal

`auth/internal.rs`: `Authorization: Bearer <JWT>` with audience `portal-internal`, admitted on `azp`:
`authenticate_pipeline_runner` (line 145-163: `JC_PORTAL_PIPELINE_RUNNER_CLIENT_ID` or `{p}-pipelines`,
`{p}-pl-pipeline-test`) for `POST /internal/pipeline-tests/{id}`; `authenticate_pipeline_of` (line 121-138:
`{p}-pipelines`, `{p}-pl-{pipeline}`) for `POST /internal/pipelines/{p}/{n}/{rejected|outcomes|samples|source-deletes|source-events}`.

## 8. What a rendered stream holds

- Top level: `input`, `pipeline`, `output` only. Caches and rate limits come from the runner's resources file
  (deployment `components/pipeline-runner/streams/resources.yaml`) plus the declared `<p>__pipeline_egress`.
- Writes: `http` processors inside a `branch` (`upsert()`, `reconciler/streams/identity.rs:303-316`): POST
  `${JC_GATEWAY_URL}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/upsert[?options=update]`, `timeout: 30s`,
  `rate_limit: <p>__pipeline_egress`, `drop_on: [400, 413, 422]`, `oauth2`. No `NGSILD-Tenant` header.
- Output: `written()` switch (`identity.rs:422`): `errored()` to `reject: "${! error() }"`, else `drop`; wrapped by
  `pipeline_log::with_outcomes` (`pipeline_log.rs:270-331`) in a `broker` `fan_out_sequential_fail_fast` with an
  `outcome` `fallback` of `http_client` POST `{internal}/internal/pipelines/{p}/{n}/outcomes` and `outcome_drop: drop`.
- Validation (`pipeline_validation.rs:184` `insert_stage`): a `switch` per class with `json_schema` and a rejected sink.
- Token: Bento `oauth2` client credentials on each `http` / `http_client`. Project identity
  (`identity.rs:41-48`): `client_key: {p}-pipelines`, `client_secret: ${JC_CLIENT_SECRET_<P>}`, `token_url: ${JC_TOKEN_URL}`.
  Pipeline identity (`JC_PORTAL_PIPELINE_IDENTITY`, `identity.rs:62-86`): `client_key: {p}/{pipeline}`,
  `token_url: ${JC_PIPELINE_TOKEN_URL}`, no secret (the token sidecar presents the pipeline's own token).
- Variables a stream names: `JC_GATEWAY_URL`, `JC_TOKEN_URL`, `JC_PIPELINE_TOKEN_URL`, `JC_CLIENT_SECRET_<P>`,
  `JC_ORG_DOMAIN`. In `jc` (`EnvAccess::AllowListed`) each `${NAME}` reads `EKIBEN_VAR_NAME`.
