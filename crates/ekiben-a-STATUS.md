# ekiben worker A: status of E1, E2, E4, E3

This status covers the patch built on commit 08e8d340. NGSI-LD (E5–E7) belongs to the other
worker and was not touched.

The patch was written without being compiled or run. The sandbox allowed no cargo build and no
look at the cargo registry sources. Every "ported" entry below therefore means "written"; none
has been run.

## Items

| Item | State | Notes |
|---|---|---|
| E1: organizations | done | Covers: `EnvAccess` (`None` / `AllowListed` = `EKIBEN_VAR_*` / `Process` = `run --host-env`); `--org-vars <dir>` (`<org>.yaml`, applied at stream start); `--org-tokens-file` (`<org> <sha256 hex>`, re-read at most every 1 s, a removed file revokes all tokens); `--require-auth`. All routes but `/ping` and `/ready` need a token. Streams are keyed by (org, id), and a foreign stream answers 404 "Stream not found". `X-Jc-Org` is honoured for the admin token only. `/ready` names only the caller's streams. `GET` answers the config as posted, redacted. |
| E2: Bento metrics | partial | Bento names, labels (`label`, `path`, `stream`, plus `org` outside the default org) and HELP texts are done. Also done: summary or histogram timing; `metrics.mapping`; `--set metrics.*`, `-r` and `--metrics`. A deleted stream keeps its counters and drops its timers. `max_series` (default 20,000) evicts the longest-deleted streams first. `/metrics` is filtered per org. Missing: `buffer_backlog` is not instrumented. |
| E3: websocket input | done | tokio-websockets client running over ekiben's checked TCP connection (and TLS). Covers Bento's `open_message(s)`, `headers`, `basic_auth` and `oauth`, reconnect inside the read, and a bounded nack replay queue. |
| E4: http_server input | done | Shared ingest listener at `/{org}/{stream}{path}`. Covers bearer, hmac or public auth, the 413 / 503 / 429 caps with `Retry-After`, the sync response with headers, status and multipart, CORS, and an own listener (`address`) outside streams mode. |
| E8 | not written: needs tokio-postgres or sqlx added under the cooldown | — |

## Bento Go test cases

Test names follow `bento_<go file stem>_<snake_case name without Test>`.

### internal/impl/prometheus/metrics_prometheus_test.go → crates/ekiben-core/tests/metrics_tests.rs

| Go test | State |
|---|---|
| TestPrometheusNoPushGateway | ported |
| TestPrometheusWithPushGateway | ported (adapted: `push_url` is refused) |
| TestPrometheusWithPushGatewayAndPushInterval | ported (adapted: refused) |
| TestPrometheusMetrics | ported |
| TestPrometheusHistMetrics | ported |
| TestPrometheusWithFileOutputPath | ported (adapted: `file_output_path` is refused) |
| TestPrometheusDefaultRegistry | ported (adapted: the field parses; there is no Go default registry) |

### internal/component/metrics/mapping_test.go → metrics_tests.rs

| Go test | State |
|---|---|
| TestPathMapping | ported |
| TestStaticVars | not ported: ekiben's `MetricMapping` has no static-variables constructor |

### internal/component/metrics/namespaced_test.go → metrics_tests.rs

| Go test | State |
|---|---|
| TestNamespacedNothing | ported |
| TestNamespacedPrefix | ported |
| TestNamespacedPrefixStaticLabels | ported |
| TestNamespacedPrefixStaticLabelsWithMappings | ported |
| TestNamespacedPrefixStaticLabelsWithMappingLabels | ported |

### internal/impl/io/input_http_server_test.go → crates/ekiben-io-http/tests/http_server_tests.rs

| Go test | State |
|---|---|
| TestHTTPBasic | ported |
| TestHTTPServerLifecycle | ported (adapted: skipped in Bento; here a stopped input leaves the route table) |
| TestHTTPServerMetadata | ported (`http_server_request_path` is relative to the stream) |
| TestHTTPServerPathParameters | ported |
| TestHTTPServerPathParametersPrefixedMetadata | ported |
| TestHTTPServerPathIsPrefix | ported |
| TestHTTPServerPathParametersCustomServer | ported |
| TestHTTPServerPathParametersCustomServerPathIsPrefix | ported |
| TestHTTPBadRequests | ported |
| TestHTTPTimeout | ported |
| TestHTTPRateLimit | ported |
| TestHTTPServerWebsockets | ported (adapted: `ws_path` is refused) |
| TestHTTPServerWSRateLimit | not applicable (no websocket ingestion) |
| TestHTTPSyncResponseHeaders | ported |
| TestHTTPSyncResponseMultipart | ported |
| TestHTTPSyncResponseHeadersStatus | ported |
| TestHTTPServerInputEnableCORSOrigins | ported |

### internal/impl/io/input_websocket_test.go → crates/ekiben-io-http/tests/websocket_tests.rs

The websocket server in these tests is written by hand (RFC 6455) and runs in-process.

| Go test | State |
|---|---|
| TestWebsocketBasic | ported (fake) |
| TestWebsocketOpenMsg | ported (fake) |
| TestWebsocketHeaders | ported (fake) |
| TestWebsocketClose | ported (fake) |

### internal/stream/manager/api_test.go

None of these cases has a dedicated `bento_api_test_*` port. CRUD, linting and auth are
exercised by `server_and_scheduler_tests.rs`, which existed before and was updated, and by the
new `tenancy.rs`.

| Go test | State |
|---|---|
| TestTypeAPIDisabled | not ported |
| TestTypeAPIBadMethods | not ported |
| TestTypeAPIBasicOperations | not ported |
| TestTypeAPIPatch | not ported |
| TestTypeAPIBasicOperationsYAML | not ported |
| TestTypeAPIList | not ported |
| TestTypeAPISetStreams | not ported |
| TestTypeAPIStreamsDefaultConf | not ported |
| TestTypeAPIStreamsLinting | not ported |
| TestTypeAPIDefaultConf | not ported |
| TestTypeAPILinting | not ported |
| TestResourceAPILinting | not ported |
| TestTypeAPIGetStats | not ported |
| TestTypeAPISetResources | not ported |
| TestAPIReady | not ported (the per-org `/ready` behaviour is tested in tenancy.rs) |

### internal/impl/pure/output_sync_response_test.go

| Go test | State |
|---|---|
| TestSyncResponseWriter | not applicable here: the `sync_response` output predates this work. It is used end to end in `crates/ekiben-server/tests/ingest_e2e.rs`. |

## Security tests (red-test-worthy properties)

**crates/ekiben-core/tests/env_access_tests.rs**
- An `AllowListed` stream cannot read a process variable outside `EKIBEN_VAR_*`, neither through
  `${VAR}` nor through `env()`.
- The org map takes precedence over the process environment.
- A missing variable never falls back to the process value.

**crates/ekiben-core/tests/metrics_tests.rs**
- Label escaping, so a stream id cannot forge a series.
- Invalid names are ignored.
- The series cap holds and evicts deleted streams first.
- An organization sees only its own series.
- A poisoned lock does not take the metrics down.

**crates/ekiben-server/tests/tenancy.rs**
- Cross-org isolation of the same stream id.
- `X-Jc-Org` is refused for org tokens and ignored without auth; a malformed value gets 400.
- `require_auth` closes every route but the probes.
- A missing token and a wrong one get identical 401s.
- The token file stores only digests, revokes everything when removed, and its parse errors
  name no secret.
- Org variables never leak to another org or into GET; a bad vars file keeps the old map and
  shows no value.
- `/ready` names only the caller's streams.

**crates/ekiben-server/tests/server_and_scheduler_tests.rs** (updated)
- Cross-org GET and DELETE answer 404 "Stream not found".
- `/metrics` needs a token and is filtered per org.

**crates/ekiben-io-http/tests/http_server_tests.rs**
- A missing bearer and a wrong one get the same 401, and the token never becomes metadata.
- HMAC is bound to the body.
- Caps: 413, including chunked bodies, and 503 with `Retry-After`.
- Foreign-org and unknown paths get 404.
- A duplicate path in one stream is refused.
- Streams-mode lint.
- Auth `Debug` is redacted, and bad auth configs are refused.

**crates/ekiben-io-http/tests/websocket_tests.rs** and **outbound_tests.rs**
- Loopback and `localhost` are refused by default.
- Address classification covers metadata, IPv4-mapped, NAT64, 6to4 and Teredo addresses.
- `private_hosts` never opens loopback or metadata.
- Fields outside the outbound rule are refused.
- The basic-auth password is absent from `Debug`.
- OAuth secrets stay out of the header.

**crates/ekiben-server/tests/ingest_e2e.rs**
- An ingest request is acked only after `sync_response`.
- Wrong auth gets 401, and another organization's path gets 404.
- Deleting a stream removes its route.
- Configs without auth, or with an own listener, are refused in streams mode.

## Assumptions not verified

All of these could only be checked by compiling and running the patch, which was not possible
here.

1. **tokio-websockets 0.13.3 API, written from memory.** Not checked:
   - `ClientBuilder::from_uri(Uri)`, `.limits(Limits::default().max_payload_len(Some(n)))`, and
     `.add_header(HeaderName, HeaderValue)` returning `Result`;
   - `.connect_on(io)` returning `(WebSocketStream, Response)`;
   - `Message::text(String)` / `Message::binary(Vec<u8>)`, `as_payload()`, `is_text` /
     `is_binary` / `is_close`;
   - the crate's feature names (`client`, `rand`, `sha1_smol`) in the workspace `Cargo.toml`.
2. **Other crate APIs:**
   - rustls with the `ring` provider: `ClientConfig::builder_with_provider`;
   - the shape of `rustls_native_certs::load_native_certs()` (`.certs`);
   - tokio-rustls `TlsConnector::connect`.
3. **axum 0.8 behaviour:**
   - `Request` carries `ConnectInfo<SocketAddr>` in its extensions when served with
     `into_make_service_with_connect_info`;
   - `middleware::from_fn_with_state` applies to the nested `/bento` routes;
   - `axum::body::to_bytes` with a limit fails on a body over it.
4. **Rendering:**
   - the Go float formatting of rendered values (`1.3e-08`, `10.452`) matches Prometheus' Go
     client;
   - Bento's child component paths (`root.input.broker.inputs.0`, `root.output.fallback.1`,
     `root.output.switch.cases.0.output`, `root.input_resources.<label>`) follow
     `internal/component/*` naming.
5. **Test behaviour:**
   - a resident stream whose input cannot connect (TEST-NET-1) stays listed as not ready rather
     than ending;
   - a POST that fails lint answers a non-2xx status with the lint text;
   - the `stream` feature of reqwest is on in the workspace (it is, per `Cargo.toml`).
6. **Shared global state.** Streams mode and the ingest route table are process-wide statics.
   The tests rely on giving every test its own stream id, and on `ingest_e2e.rs` being its own
   test binary.
7. **Dependencies.** New workspace dependencies (`tokio-websockets`, `rustls`, `tokio-rustls`,
   `rustls-pki-types`) were taken as present in `Cargo.lock` at 08e8d340. Their release ages
   were not re-checked against the 7-day cooldown.
8. **Leftover file.** A stray file `c01.txt` was left in the output scratch directory by an
   earlier step. It is not part of the patch and could not be deleted with the tools allowed.
