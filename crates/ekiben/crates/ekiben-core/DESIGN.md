# `ekiben` Architecture & Design Specification

**Name:** `ekiben` (駅弁, Japanese railway-station bento: Bento's job, carried in transit).
**Compatibility Target:** Drop-in replacement for `github.com/warpstreamlabs/bento` (commit `63c54da7`, v1.22 semantics) for every stream configuration rendered by `joinedcontext-portal` (`src/reconciler/streams/render.rs`) and all 1,732 seed files in `joinedcontext-deployment`.

---

## 1. Crate Layering & Modularity Rules

Every crate enforces `#![forbid(unsafe_code)]`, stays well under 250k tokens of source, and follows a strict acyclic dependency graph:

```text
ekiben-bloblang          (pure sync library: lexer, parser, AST/plan compiler, VM, functions, methods; NO tokio)
       │
       ▼
  ekiben-core            (Part, Batch, PartsView, Transaction, SortGroup, Processor/Input/Output/Cache/RateLimit traits,
       │                  Registry, ${VAR:default} interpolation, Secret redaction, OrgQuotas, Pipeline, Metrics)
       ├──────────────────────┬──────────────────────┬──────────────────────┐
       ▼                      ▼                      ▼                      ▼
ekiben-processors      ekiben-io-http         ekiben-io-file         ekiben-io-nats
(mapping, branch,      (reqwest+rustls pool,  (file, stdout,         (nats, nats_jetstream,
 switch, workflow,      SSRF guard, OAuth2,    generate, broker,      nats_kv cache/proc,
 archive, unarchive,    http proc,             switch, fallback,      durable pull/push,
 try, catch, while,     http_client in/out)    retry, drop, reject)   explicit ack/nak)
 for_each, split,
 xml, sleep, log,
 rate_limit, cache)
       │                      │                      │                      │
       └──────────────────────┴──────────┬───────────┴──────────────────────┘
                                         ▼
                                  ekiben-server
                        (axum streams-mode HTTP API, cron scheduler,
                         one-shot runner, linter, `ekiben` CLI binary)
```

---

## 2. Bento Go Source Mapping Table

| `ekiben` Module / Type | Bento Go Source File(s) Followed | Key Semantics Preserved |
|---|---|---|
| `ekiben_bloblang::Value` | `internal/value/*`, `internal/message/util.go` | `Null`, `Bool`, `Int(i64)`, `Uint(u64)`, `Float(f64)`, `String`, `Bytes`, `Timestamp(DateTime<FixedOffset>)`, `Array`, `Object`, plus internal control values `Delete` (`deleted()`) and `Nothing` (`nothing()`). |
| `ekiben_bloblang::parser` | `internal/bloblang/parser/*.go` | Complete Bloblang grammar: `import`, `from`, `map`, `let`, `meta`, `root`, implicit `root`, quoted path segments (`~0`/`~1` escaping), root-level `if`/`else if`/`else` statements, expression `if`, `match` (with and without context expression), `x -> expr` lambdas, `.()` context brackets, `[i]` and `[start:end:step]` slicing, dynamic object keys `{ (expr): val }`, `${! expr }` field interpolation. |
| `ekiben_bloblang::arithmetic` | `internal/bloblang/query/arithmetic.go` | Four-pass operator precedence: (1) `*`, `/`, `%`, `|` (coalesce!), (2) `+`, `-`, (3) `==`, `!=`, `<`, `<=`, `>`, `>=`, (4) `&&`, `||`. Number degradation: `int op int -> int` except `/` which always yields `f64`; `string + string -> string`. |
| `ekiben_bloblang::mapping` | `internal/bloblang/mapping/{executor,assignment,statement,target}.go` | `MapPart` (fresh `root = Nothing`; if still `Nothing` at end, original part is unchanged; if `Delete`, part is filtered out) vs `MapOnto` (`mutation`: starts `root` with mutable structured clone of target part). Max map recursion stack cap = 5,000 (`exceeded maximum allowed stacks`) guarded with `stacker::maybe_grow` and cycle-safe `QueryTargets` walking. |
| `ekiben_core::Part`, `Batch` | `internal/message/{part,data,message,sort_group}.go`, `internal/batch/count.go` | Zero-copy `bytes::Bytes` payload, lazy JSON parse (`AsStructured` / `AsStructuredMut`), copy-on-write metadata (`HashMap<String, Value>`), error flag (`DetailedError` with `label`, `typ`, `path`), `SortGroup` lineage tags, `collapsed_count`, binary serialization format (4-byte BE count + 4-byte BE length per part). |
| `ekiben_core::PartsView<'a>` | Rust orphan-rule wrapper (`&'a [Part]`) | Implements `ekiben_bloblang::MessageBatch` on a local struct in `ekiben-core`, never on `[Part]` or `Vec<Part>`. |
| `ekiben_core::Transaction` | `internal/message/transaction.go`, `internal/transaction/tracked.go`, `internal/batch/{combined_ack_func,error}.go` | Couples a `Batch` with an `AckFunc` (`FnOnce(Option<BatchError>)`). `TrackedTransaction` maps indexed `BatchError`s back to the source batch via `SortGroup`. |
| `ekiben_core::interpolate` | `internal/config/env_*.go`, `docs/configuration/interpolation.md` | Replaces `${VAR}` and `${VAR:default}` in config text; unescapes `${{foo}}` -> `${foo}` and `${{!foo}}` -> `${!foo}`; leaves `${! ... }` untouched for Bloblang field parser; reports lint errors on undefined `${VAR}` without `:`. |
| `ekiben_processors::branch` | `internal/impl/pure/processor_branch.go` | Clears error flags on branch request copy, runs `request_map` (`deleted()` skips part, error records `request mapping failed`), runs child processors on non-skipped parts, checks count alignment, skips `result_map` for parts that errored in children (`processors failed`), overlays `result_map` via `MapOnto` onto original batch. |
| `ekiben_processors::workflow` | `internal/impl/pure/processor_workflow*.go` | Explicit 2D `order` tiers or automatic Kahn layered topological sort from `request_map` `QueryTargets` and `result_map` `AssignmentTargets`; circular dependency detection; reads/writes `<meta_path>.{succeeded,skipped,failed,previous,apply}`. |
| `ekiben_processors::switch` | `internal/impl/pure/processor_switch.go` | Evaluates each case's `check` per part; groups matching parts per case; supports `fallthrough`; restores original batch ordering via `SortGroup`. |
| `ekiben_processors::try_proc`, `catch_proc` | `internal/impl/pure/processor_{try,catch}.go`, `internal/component/processor/execute.go` | `try` splits batch into 1-part batches and skips any batch where `batch[0].is_errored()`. `catch` splits batch into 1-part batches, runs children only on `caught` items, and clears `ErrorSet(None)` on all resulting parts upon exit. |
| `ekiben_processors::while_proc` | `internal/impl/pure/processor_while.go` | Checks `check.query_part(0, &batches[0])` (first part of first batch!), honors `at_least_once` and `max_loops` (with hard safety ceiling). |
| `ekiben_processors::archive`, `unarchive` | `internal/impl/pure/processor_{archive,unarchive}.go` | Formats: `json_array`, `json_documents`, `json_map`, `lines`, `concatenate`, `binary`, `tar`, `zip`, `csv`, `csv:<delim>`, `xml_documents_to_json`, `xml_documents_to_json:cast`. Enforces zip-bomb member count and total unpacked byte limits. |
| `ekiben_io_http` | `internal/httpclient/*.go`, `internal/impl/io/{processor_http,input_http_client,output_http_client}.go` | Connection-pooled `reqwest` with `rustls`, OAuth2 `client_credentials` token caching, `backoff_on`/`drop_on`/`successful_on`, `extract_headers`, `http_status_code` metadata on both 2xx and error responses, SSRF private/loopback IP guard, URL userinfo & header redaction. |
| `ekiben_io_file` | `internal/impl/io/{input_file,output_file}.go`, `internal/impl/pure/{input_*,output_*}.go` | `generate`, `file`, `stdout`, `broker` (fan-in input & `fan_out`/`fan_out_fail_fast`/`fan_out_sequential`/`round_robin`/`greedy` output), `switch`, `fallback` (sets `fallback_error` metadata), `retry`, `drop`, `reject`, `reject_errored`, `resource`. |
| `ekiben_io_nats` | `internal/impl/nats/*.go` | `nats` core pub/sub + request-reply, `nats_jetstream` durable/ephemeral consumers with explicit `Ack`/`Nak` tied to `Transaction`, `nats_kv` cache and processor, NKey/JWT credentials, and per-org subject namespace enforcement. |
| `ekiben_server` | `internal/api/api.go`, `internal/stream/{type,manager/api,manager/type}.go` | `/ready`, `/ping`, `/version`, `/endpoints`, `/metrics`, `/stats`, `/streams`, `/streams/{id}`, `/streams/{id}/stats`, `/resources/{type}/{id}`, plus `/bento/*` root-path aliases. Freezes `uptime` when a finite stream finishes and resets `uptime` on `PUT /streams/{id}`. |

---

## 3. Public Types, Traits & Signatures (`ekiben-core`)

```rust
// Tenancy & Quotas
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrgId(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgQuotas {
    pub max_concurrent_streams: usize,
    pub max_in_flight_messages: usize,
    pub max_memory_bytes_per_run: usize,
    pub max_cpu_time_ms_per_run: u64,
    pub max_messages_per_sec: u64,
    pub max_http_requests_per_sec: u64,
    pub allow_private_ips: bool,
    pub allowed_http_hosts: Option<Vec<String>>,
    pub subject_prefix: Option<String>,
}

// Message Model
#[derive(Clone, Debug)]
pub struct DetailedError {
    pub message: String,
    pub label: String,
    pub component_type: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct Part { /* Arc-shared CoW raw Bytes, lazy structured Value, metadata, DetailedError, SortTag, collapsed_count */ }

pub type Batch = Vec<Part>;
pub struct PartsView<'a>(pub &'a [Part]); // Implements ekiben_bloblang::MessageBatch

pub struct Transaction {
    pub payload: Batch,
    pub ack: AckSender,
}

// Core Component Traits
#[async_trait]
pub trait Processor: Send + Sync {
    fn processor_type(&self) -> &'static str;
    async fn process_batch(&self, ctx: &ProcContext, batch: Batch) -> Result<Vec<Batch>, CoreError>;
    async fn close(&self) -> Result<(), CoreError> { Ok(()) }
}

#[async_trait]
pub trait Input: Send + Sync {
    fn input_type(&self) -> &'static str;
    async fn connect(&self) -> Result<(), CoreError>;
    async fn read_batch(&self, ctx: &StreamContext) -> Result<(Batch, AckResponder), CoreError>;
    fn is_connected(&self) -> bool { true }
    async fn close(&self) -> Result<(), CoreError> { Ok(()) }
}

#[async_trait]
pub trait Output: Send + Sync {
    fn output_type(&self) -> &'static str;
    async fn connect(&self) -> Result<(), CoreError>;
    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError>;
    fn is_connected(&self) -> bool { true }
    async fn close(&self) -> Result<(), CoreError> { Ok(()) }
}

#[async_trait]
pub trait Cache: Send + Sync {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError>;
    async fn set(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError>;
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError>;
    async fn delete(&self, key: &str) -> Result<(), CoreError>;
    async fn exists(&self, key: &str) -> Result<bool, CoreError>;
}

#[async_trait]
pub trait RateLimit: Send + Sync {
    async fn access(&self, parts: &[Part]) -> Result<Duration, CoreError>;
}
```

---

## 4. Ack/Nack, Error-Flag Flow & Child Processor Execution

1. **At-Least-Once Transaction Lifecycle:**
   - Every `Input::read_batch` returns `(Batch, AckResponder)`.
   - The engine wraps it in a `Transaction` and sends it over a bounded `mpsc::channel(max_in_flight)` to the `Pipeline` worker pool (`pipeline.threads` workers).
   - If processors filter all parts (`vec![]`), the transaction is immediately acknowledged (`Ack`) upstream.
   - If processors split 1 batch into $N$ batches (e.g., via `split`), `CombinedAcker` derives $N$ child transactions and only resolves the upstream `AckResponder` once all $N$ outputs have completed, propagating a `BatchError` (with `SortGroup` part indices) if any child failed.
   - When an output fails and nacks, inputs with `auto_replay_nacks: true` (or `AsyncPreserver`) replay only the failed parts tracked by `SortGroup`, while broker inputs (`nats_jetstream`) call `msg.Nak()` for server-side redelivery.
2. **Error Flag Flow (`DetailedError`):**
   - When a processor fails on a part (e.g., `throw("...")` in `mapping`, or a failed HTTP request in `http`), the part's payload is **kept unchanged** and its `error` slot is populated with `DetailedError { message, label, component_type, path }`.
   - Subsequent normal processors (`mapping`, `http`, `log`, etc.) **still execute** on errored parts unless guarded by `try` or `switch`.
   - `try` splits a batch into single-part batches and skips any part that already has an error flag (`part.is_errored()`) before or during the `try` sequence.
   - `catch` splits a batch into single-part batches, runs its child processors **only** on parts that entered the `catch` with an error flag, and clears `part.clear_error()` on all parts when exiting the `catch` block.
3. **Child Processor Runners:**
   - `branch`: Clones the batch, clears error flags on the clone so prior errors don't abort the branch, applies `request_map` (`deleted()` marks the index as skipped; mapping error marks the index as failed), executes child processors on the remaining parts, verifies that `len(results) + len(skipped) + len(failed) == original_len`, checks if any child output part has an error flag (recording `processors failed: ...` and skipping `result_map` for that index), and applies `result_map` via `MapOnto` onto the original parts.
   - `switch`: Evaluates `check` on each part in the batch; parts matching case $i$ are batched together and passed to case $i$'s child processors. If `fallthrough: true`, resulting parts join the candidate list for case $i+1$. Finally, `SortGroup` restores the original relative order of parts.
   - `workflow`: Resolves the branch DAG (either from explicit `order` tiers or automatic layered topological sort over `request_map` query targets and `result_map` assignment targets). Executes each tier's branches in parallel on shallow copies of the batch, overlays each branch's result in deterministic order, and writes `{"succeeded": [...], "skipped": [...], "failed": {...}, "previous": ...}` to `meta_path`.
   - `for_each`: Splits the incoming batch into 1-part batches, runs child processors on each 1-part batch sequentially, and concatenates the resulting parts back into a single batch.
   - `while`: Evaluates `check` on `batches[0][0]` (the first message of the first batch), loops child processors while true up to `max_loops` (and hard limit), exiting immediately if the batch becomes empty.

---

## 5. Streams API State Machine, One-Shot Runs & Scheduler Architecture

```text
  POST /streams/{id} ──► [ Active (is_running = true, uptime ticking) ]
                                │                         │
       Input returns EndOfInput │                         │ PUT /streams/{id}
       (e.g. generate count,    ▼                         ▼
        file EOF, run: once)  [ Stopped ]           [ Stop old, Start new ]
                              (active = false,       (createdAt reset,
                               uptime FROZEN at       uptime restarts at 0)
                               stopped_after)
```

- **Why In-Process Isolated Tasks for Scheduler Default (with Optional Child Process Mode):**
  In `ekiben schedule`, each due run executes as an isolated async task with its own `StreamResources` and `StreamMetrics` that are completely dropped when the one-shot stream finishes. In-process execution avoids fork/exec + TLS init overhead (~15 ms and ~18 MiB per child process), keeping the 2 GiB edition well within budget when running hundreds of scheduled open-data pipelines. For organizations requiring hard OS-level address-space isolation, `--isolation process` (or per-org config `isolation: process`) spawns `ekiben run` as a child process per run.
  - **Threats stopped by in-process mode:** Cross-org resource/cache/rate-limit access, cross-org API access, SSRF to private networks, secret leakage in logs/APIs, unbounded recursion/loops/allocations in Bloblang, zip bombs, and noisy-neighbor starvation (via per-org quotas & token buckets).
  - **Additional threats stopped by child-process mode:** OS-level memory/CPU cgroup enforcement (`RLIMIT_AS` / `RLIMIT_CPU`) and defense-in-depth against hypothetical memory-safety bugs in third-party dependencies.

---

## 6. Memory, Performance & Security Rules

1. **Zero-Copy & Lazy Parsing:** `Part` stores `bytes::Bytes` and `Option<Value>`. When two consecutive `mapping` or `mutation` processors run, the structured `Value` is passed directly without serializing to JSON bytes or re-parsing.
2. **Compiled Bloblang Plan:** `Environment::compile_mapping` resolves all field paths (`SmallVec<[String; 4]>`), folds constant arithmetic literals at parse time, and resolves function/method names to enum/function dispatches at parse time.
3. **Interpreter Safety Limits:**
   - Max map recursion depth: `5,000` (with `stacker::maybe_grow` so 5,000 frames never overflow a 2 MiB thread stack even in debug builds).
   - Max while loops: `10,000` hard cap unless lower `max_loops` configured.
   - Max string repeat / output size: `64 MiB`.
   - Max archive members: `10,000` files; max unpacked archive bytes: `128 MiB`; max compression ratio check (zip-bomb defense).
   - XML parser (`quick-xml`): DTD and external entity expansion refused.
4. **Outbound HTTP Security:**
   - `rustls` TLS verification always on.
   - Schemes restricted to `http` and `https`.
   - Connect timeout (default 5s), total request timeout (default 30s), response body size cap (default 64 MiB), redirect cap (default 5, stripping `Authorization`/`Cookie` headers across host changes).
   - SSRF guard resolves/checks literal IP and host targets against loopback (`127.0.0.0/8`, `::1`), RFC 1918 private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), link-local (`169.254.0.0/16`, `fe80::/10`), and CGNAT (`100.64.0.0/10`) ranges unless `allow_private_ips` is enabled.
5. **Secret Redaction:**
   - URL userinfo (`https://user:pass@host` -> `https://user:REDACTED@host`), `Authorization` / secret headers, `client_secret`, `password`, `user_jwt`, `user_nkey_seed`, and secret env values are redacted in `Debug` impls, log lines, and `GET /streams` / `GET /streams/{id}` responses.

---

## 7. Dependencies & Rationale

| Crate | Why Used |
|---|---|
| `serde_norway` | Maintained drop-in replacement for the unmaintained `serde_yaml` crate (used by `joinedcontext-portal` as well). |
| `tokio` | Async runtime, bounded channels, timers, `spawn_blocking` for file/archive I/O. |
| `axum` | Streams-mode HTTP REST API server. |
| `reqwest` (`rustls-tls`) | Outbound HTTP client with connection pooling, HTTP/2, gzip/brotli, and pure-Rust TLS. |
| `stacker` | Growable stack segments so deep recursive Bloblang maps hit the 5,000 logical stack limit cleanly on small threads. |
| `regex` | Linear-time regular expressions (immune to catastrophic backtracking ReDoS). |
| `chrono` & `chrono-tz` | Timestamp parsing (`ts_parse`, `ts_strptime`), formatting (`ts_format`, `ts_strftime`), ISO-8601 durations, and IANA timezones. |
| `quick-xml`, `csv`, `tar`, `zip`, `flate2` | Safe streaming parsers/writers for XML, CSV, TAR, ZIP, and gzip/zlib compression. |
| `sha2`, `sha1`, `md-5`, `hmac`, `crc32fast`, `twox-hash`, `base64`, `hex` | Bloblang `.hash()`, `.encode()`, and `.decode()` implementations. |
| `subtle` | Constant-time bearer token and secret comparisons. |
| `async-nats`, `nkeys` | Pure-Rust NATS Core, JetStream, Key-Value, and NKey/JWT authentication. |
| `criterion` | Microbenchmarks for hot paths (Bloblang parse, mapping exec, JSON round-trip, branch). |

---

## 8. Complete Bento Component Inventory Disposition (PART A2)

The `ekiben_core::Registry` accepts any `Input`, `Output`, `Processor`, `Cache`, `RateLimit`, `Buffer`, or `Scanner` constructor without changing `ekiben-core`. Every component from PART A2 is classified below:

### 8.1 Buffers
| Component | Disposition | Crate / Notes |
|---|---|---|
| `none` | **Built in this answer** | `ekiben-core` (default pass-through) |
| `memory` | **Built in this answer** | `ekiben-core` (bounded memory batch buffer with `batch_policy`) |
| `sqlite` | Next | `ekiben-io-sql` |
| `system_window` | Next | `ekiben-processors` |

### 8.2 Caches
| Component | Disposition | Crate / Notes |
|---|---|---|
| `memory`, `lru`, `ttlru`, `noop`, `multilevel` | **Built in this answer** | `ekiben-processors` |
| `nats_kv` | **Built in this answer** | `ekiben-io-nats` |
| `file` | **Built in this answer** | `ekiben-io-file` |
| `aws_dynamodb`, `aws_s3`, `gcp_cloud_storage`, `redis`, `memcached`, `mongodb`, `couchbase`, `ristretto`, `sql` | Next | `ekiben-io-cloud` / `ekiben-io-sql` |

### 8.3 Inputs
| Component | Disposition | Crate / Notes |
|---|---|---|
| `generate`, `file`, `csv`, `stdin`, `inproc`, `broker`, `batched`, `resource`, `read_until`, `sequence`, `dynamic` | **Built in this answer** | `ekiben-io-file` |
| `http_client`, `http_server`, `websocket` | **Built in this answer** (`http_client`) / Next (`http_server`, `websocket`) | `ekiben-io-http` |
| `nats`, `nats_jetstream`, `nats_kv` | **Built in this answer** | `ekiben-io-nats` |
| `nats_object_store` | Next | `ekiben-io-nats` |
| `nats_stream` | Not planned | Deprecated NATS Streaming (STAN) EOL June 2023; replaced by `nats_jetstream`. |
| `kafka_franz`, `kafka` | Next | `ekiben-io-kafka` (behind `kafka` cargo feature; pure-Rust or `rdkafka` consumer groups mapped to `Transaction` acks). |
| `mqtt`, `mqtt_v5` | Next | `ekiben-io-mqtt` (or bridged via `jc-mqtt` into NATS JetStream). |
| `sql_select`, `sql_raw`, `cockroachdb_changefeed`, `cassandra`, `mongodb`, `cypher`, `parquet` | Next | `ekiben-io-sql` |
| `aws_s3`, `aws_sqs`, `aws_kinesis`, `azure_blob_storage`, `azure_cosmosdb`, `azure_queue_storage`, `azure_service_bus_queue`, `azure_table_storage`, `gcp_bigquery_select`, `gcp_cloud_storage`, `gcp_pubsub`, `gcp_spanner_cdc`, `amqp_0_9`, `amqp_1`, `pulsar`, `redis_list`, `redis_pubsub`, `redis_scan`, `redis_streams`, `sftp`, `hdfs`, `etcd`, `grpc_client`, `kubernetes_watch`, `nsq`, `nanomsg`, `zmq4`, `zmq4n`, `socket`, `socket_server`, `file_tail`, `fsevent`, `s2`, `beanstalkd`, `discord`, `twitter_search` | Next | `ekiben-io-cloud` / `ekiben-io-messaging` |
| `subprocess` | Not planned | Forbidden by security policy (no subprocess/shell execution in multi-tenant runner). |

### 8.4 Outputs
| Component | Disposition | Crate / Notes |
|---|---|---|
| `http_client` | **Built in this answer** | `ekiben-io-http` |
| `file`, `stdout`, `drop`, `drop_on`, `switch`, `fallback`, `broker`, `retry`, `reject`, `reject_errored`, `resource`, `cache`, `sync_response`, `inproc`, `dynamic` | **Built in this answer** | `ekiben-io-file` |
| `nats`, `nats_jetstream`, `nats_kv` | **Built in this answer** | `ekiben-io-nats` |
| `nats_object_store` | Next | `ekiben-io-nats` |
| `nats_stream` | Not planned | Deprecated STAN EOL; replaced by `nats_jetstream`. |
| `kafka_franz`, `kafka` | Next | `ekiben-io-kafka` |
| `mqtt`, `mqtt_v5` | Next | `ekiben-io-mqtt` |
| `sql_insert`, `sql_raw`, `sql`, `duckdb_append`, `questdb`, `snowflake_put`, `cassandra`, `mongodb`, `couchbase`, `cypher` | Next | `ekiben-io-sql` |
| `aws_s3`, `aws_s3_stream`, `aws_sqs`, `aws_sns`, `aws_kinesis`, `aws_kinesis_firehose`, `aws_dynamodb`, `azure_blob_storage`, `azure_cosmosdb`, `azure_queue_storage`, `azure_table_storage`, `gcp_bigquery`, `gcp_bigquery_write_api`, `gcp_bigtable`, `gcp_cloud_storage`, `gcp_pubsub`, `elasticsearch`, `elasticsearch_v2`, `opensearch`, `amqp_0_9`, `amqp_1`, `pulsar`, `redis_hash`, `redis_list`, `redis_pubsub`, `redis_streams`, `sftp`, `hdfs`, `grpc_client`, `websocket`, `http_server`, `splunk_hec`, `datadog_logs`, `slack_webhook`, `pusher`, `discord`, `nsq`, `nanomsg`, `zmq4`, `zmq4n`, `socket`, `s2`, `beanstalkd` | Next | `ekiben-io-cloud` / `ekiben-io-messaging` |
| `subprocess` | Not planned | Forbidden by security policy (no subprocess execution). |

### 8.5 Processors
| Component | Disposition | Crate / Notes |
|---|---|---|
| `mapping`, `bloblang`, `mutation`, `branch`, `switch`, `try`, `catch`, `for_each`, `while`, `split`, `archive`, `unarchive`, `sleep`, `log`, `resource`, `rate_limit`, `xml`, `workflow`, `parallel`, `retry`, `noop`, `processors`, `cache`, `cached`, `bounds_check`, `group_by`, `group_by_value`, `insert_part`, `select_parts`, `dedupe`, `compress`, `decompress`, `metric`, `json_schema`, `sync_response` | **Built in this answer** | `ekiben-processors` |
| `http` | **Built in this answer** | `ekiben-io-http` |
| `nats_kv`, `nats_request_reply` | **Built in this answer** | `ekiben-io-nats` |
| `nats_object_store` | Next | `ekiben-io-nats` |
| `avro`, `protobuf`, `schema_registry_decode`, `schema_registry_encode`, `parquet_decode`, `parquet_encode`, `msgpack`, `grok`, `parse_log`, `jmespath`, `jq`, `template`, `opensnowcat` | Next | `ekiben-processors-formats` |
| `wasm` | Next | `ekiben-wasm` (Wasmtime component host) |
| `sql`, `sql_insert`, `sql_raw`, `sql_select`, `aws_bedrock_chat`, `aws_bedrock_invoke`, `aws_dynamodb_partiql`, `aws_lambda`, `aws_s3`, `azure_cosmosdb`, `couchbase`, `elasticsearch`, `file`, `gcp_bigquery_select`, `mongodb`, `redis`, `redis_script`, `sentry_capture`, `nlp_*` | Next | `ekiben-io-sql` / `ekiben-io-cloud` |
| `command`, `subprocess`, `javascript`, `python`, `awk` | Not planned | Subprocess (`command`, `subprocess`) and un-sandboxed scripting (`javascript`, `python`, `awk`) are replaced by Bloblang and sandboxed `wasm`. |

### 8.6 Rate Limits, Scanners, Metrics & Tracers
| Category | Built in this answer | Next / Not planned |
|---|---|---|
| **Rate Limits** | `local` (`count`, `byte_size`, `interval`) in `ekiben-processors` | `redis` (Next in `ekiben-io-cloud`) |
| **Scanners** | `lines`, `to_the_end`, `csv`, `json_documents`, `tar`, `chunker`, `decompress`, `skip_bom`, `xml_documents`, `re_match`, `switch` in `ekiben-io-file` | `avro` (Next in `ekiben-processors-formats`) |
| **Metrics** | `prometheus`, `json_api`, `none`, `logger` in `ekiben-core` / `ekiben-server` | `statsd`, `influxdb`, `aws_cloudwatch` (Next) |
| **Tracers** | `none`, structured tracing spans in `ekiben-core` | `open_telemetry_collector` (Next), `jaeger` / `gcp_cloudtrace` (Not planned; superseded by OTLP) |
````

Now let's carefully design and build `jc/crates/ekiben-bloblang`.
Let's review everything `ekiben-bloblang` needs:
- `Cargo.toml`
- `src/lib.rs`: Public API (`Environment`, `Executor`, `FieldExpression`, `Value`, `MessageBatch`, `PartAccess`, `PartMetaMut`, `TargetPath`, `TargetType`, `AssignTargetPath`, `AssignTargetType`, `BloblangError`, `ParseError`).
- `src/value.rs`:
  - `Value` enum:
    - `Null`
    - `Bool(bool)`
    - `Int(i64)`
    - `Uint(u64)`
    - `Float(f64)`
    - `String(String)`
    - `Bytes(Vec<u8>)`
    - `Timestamp(chrono::DateTime<chrono::FixedOffset>)`
    - `Array(Vec<Value>)`
    - `Object(BTreeMap<String, Value>)` (Note: `BTreeMap` gives deterministic sorted JSON output matching Go's `encoding/json` which sorts map keys alphabetically!)
    - `Delete` (`deleted()`)
    - `Nothing` (`nothing()`)
  - Conversion between `serde_json::Value` and `Value` (preserving integers vs floats!).
  - Coercion & type helpers matching Bento's `internal/value`:
    - `type_name(&self) -> &'static str` (`"null"`, `"bool"`, `"number"`, `"string"`, `"bytes"`, `"timestamp"`, `"array"`, `"object"`, `"delete"`, `"nothing"`)
    - `to_string_val(&self) -> String` (`IToString`)
    - `to_bytes_val(&self) -> Vec<u8>` (`IToBytes`)
    - `to_number(&self) -> Result<f64, String>` (`IToNumber`: parses strings with `f64::from_str`, converts ints/uints/floats)
    - `to_int64(&self) -> Result<i64, String>` (`IToInt`: base-0 parsing for strings: `0x`/`0X` hex, `0b`/`0B` binary, `0o`/`0O` or leading `0` octal—so `"08"` and `"09"` fail as in Go's `strconv.ParseInt(s, 0, 64)`! Floats with non-zero fractional part fail!)
    - `to_uint64(&self) -> Result<u64, String>` (`IToUint`)
    - `to_bool(&self) -> Result<bool, String>` (`IToBool`: numbers `!= 0`, strings `"1"`, `"t"`, `"T"`, `"TRUE"`, `"true"`, `"True"`, `"0"`, `"f"`, `"F"`, `"FALSE"`, `"false"`, `"False"`)
    - `to_timestamp(&self) -> Result<DateTime<FixedOffset>, String>` (`IGetTimestamp`: RFC3339 string or unix seconds int/float!)
    - `compare_eq(&self, other: &Value) -> bool` (`ICompare`: numeric equality across `Int`/`Uint`/`Float`, deep equality on arrays/objects, strict type match on `String` vs `Number` vs `Bool` vs `Null`).
    - Dot-path get/set/delete/merge/assign/flatten/without/with helpers (matching `gabs` semantics, including `~0` -> `~` and `~1` -> `.` unescaping in `dot_path_to_slice` and `slice_to_dot_path`).
- `src/ast.rs`:
  - Statements (`SingleStatement`, `IfStatement`), Assignments (`JsonAssignment`, `VarAssignment`, `MetaAssignment`), Expressions (`Literal`, `ArrayLiteral`, `ObjectLiteral`, `FieldRef`, `VarRef`, `MetaRef`, `FunctionCall`, `MethodCall`, `MapExpr`, `IndexExpr`, `SliceExpr`, `NotExpr`, `UnaryNeg`, `BinaryOp`, `IfExpr`, `MatchExpr`, `LambdaExpr`).
- `src/parser.rs`:
  - Hand-written recursive-descent / combinator parser matching Bento's `internal/bloblang/parser`:
    - `parse_mapping(ctx, input)`
    - `parse_field(ctx, input)` (for `${! ... }` and `${{! ... }}`)
    - Handles comments (`# ...`), triple-quoted strings (`"""..."""`), double-quoted strings with escapes, numbers (int64 & float64), booleans, `null`, arrays, objects (with dynamic key expressions!), `import`, `from`, `map name { ... }`, `let $x = ...` or `let x = ...`, `meta k = ...` or `meta = ...`, `root.a.b = ...` or implicit `a.b = ...`, `if ... { ... } else if ... { ... } else { ... }`, `match ... { ... }`, lambdas `x -> ...`, method chains `.foo(...)`, bracketed context `. ( ... )`, indexing `[i]`, slicing `[a:b:c]`, arithmetic operators with 4-pass precedence (`*`/`/`/`%`/`|`, `+`/`-`, comparisons, `&&`/`||`).
    - Refuses unknown functions (`unrecognised function '<name>'`) and unknown methods (`unrecognised method '<name>'`) at parse time!
- `src/functions.rs`:
  - All Bloblang functions.
- `src/methods.rs` (split into small per-family dispatch functions so stack frames are tiny!):
  - `methods_general.rs` / `methods_strings.rs` / `methods_numbers.rs` / `methods_structured.rs` / `methods_time.rs`:
  - All Bloblang methods, Go layout time parser/formatter, `strftime`/`strptime` with rollover, Go `fmt.Sprintf` `.format()` verb formatter, regex methods, CSV/JSON/YAML/XML/URL/form parsing & formatting, hashing & encoding & AES encryption/decryption, compression/decompression.
- `src/executor.rs`:
  - `Executor` and `FieldExpression`, `stacker::maybe_grow` at every recursive entry, `QueryTargets` with visited-map cycle prevention, `AssignmentTargets`.

