# jc-embedder

The embedding model one deployment loads in process, the service that serves it to several
deployments, and that service's client, in one crate (`jc_embedder`). The model is
`intfloat/multilingual-e5-small` at 614241f6, the int8 ONNX export and its tokenizer, each file
checked against its SHA-256 before it loads (`MODEL_FILES`).

## Why a binary of its own

The owner asked for an embedder several deployments share. A `jc --role embedder` module would
run inside a deployment's `jc`, with that deployment's config, setup and secrets. A separate
binary has none of them: a TOML file that names its callers, the model and ONNX Runtime, and an
image without kreuzberg, PDFium or the other modules. It checks callers with the code jc's door
uses (`crates/shared/jc-door`).

## API

`POST /v1/embeddings` with a bearer token:

```json
{"model": "multilingual-e5-small-int8", "kind": "passage", "input": ["text", "..."]}
```

answers

```json
{"model": "multilingual-e5-small-int8", "model_sha256": "dd476dd0...", "data": [{"index": 0, "embedding": [384 numbers]}]}
```

`kind` is `query` (a question, embedded before every waiting passage) or `passage` (a text to
index); the service adds e5's `query: ` / `passage: ` prefix. Each text runs through the model
alone, because the int8 export's vectors depend on what else is in the batch. `GET /healthz`
answers `ok`.

Refusals are JSON `{"error": "<fixed sentence>"}` and never echo a text: 401 (no token, or not
one of a configured deployment), 400 (not a request, another model), 413 (more than `max_texts`
texts, a text over `max_text_bytes`, a body over the cap), 429 (the deployment's
`max_in_flight` requests already run, or its `max_texts_per_minute` are spent; `Retry-After`),
408 (not done within `timeout_secs`), 503 (the model failed).

## Configuration

`jc-embedder --config <file>` or `EMBEDDER_CONFIG`; ONNX Runtime from `ORT_DYLIB_PATH`.

```toml
listen_addr = "0.0.0.0:8089"
metrics_addr = "0.0.0.0:9089"   # optional: Prometheus /metrics on its own listener
model_path = "/models/e5-small"
threads = 2                     # ONNX Runtime threads, 1 to 16

[limits]                        # every value at least 1; defaults shown
max_texts = 64
max_text_bytes = 8192
timeout_secs = 20
max_in_flight = 4               # per deployment

[[deployments]]
name = "dev"
issuer = "https://jc.dev.example/id/realms/jc"
jwks_url = "https://jc.dev.example/id/realms/jc/protocol/openid-connect/certs"
clients = ["jc-assistant"]
max_texts_per_minute = 1200
```

A token is admitted when its own deployment's kaisatsu signed it (`iss`, the JWKS of that
deployment), for audience `embedder`, unexpired, for a listed client. The issuer picks the
deployment; every count (quota, in flight, metrics) is per deployment. No cache: a cache shared
across deployments would tell one deployment what another embedded.

Metrics: `embedder_texts_total{deployment,kind}`, `embedder_refusals_total{deployment,reason}`,
`embedder_request_seconds{deployment}`.

## The assistant

`assistant::embed::Embedder` is the local model (`JC_ASSISTANT_MODEL_DIR`) or this service
(`JC_ASSISTANT_EMBEDDER_URL`, `JC_ASSISTANT_EMBEDDER_TOKEN_URL`, `JC_ASSISTANT_EMBEDDER_CLIENT_ID`,
default `jc-assistant`, `JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE`). With the URL set, a failing
service is an error, never a switch to the local model. The client sends at most 16 texts a
request and refuses an answer of another model (`model_sha256`), of the wrong count or order, or
with numbers that are not 384 finite ones.

## Tests

`cargo test -p jc-embedder` (the model tests need `JC_ASSISTANT_TEST_MODEL_DIR` and
`ORT_DYLIB_PATH`, as the assistant's do).
