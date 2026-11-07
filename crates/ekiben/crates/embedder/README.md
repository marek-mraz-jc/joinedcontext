# `embedder` (`jc-embedder`) — Shared Stateless Embedding & Reranking Service

## Scope
`embedder` is a stateless, shared text and multimodal embedding and cross-encoder reranking service:
- Loads quantized `int8` ONNX / GGUF embedding models via `mmap` so model weights are shared in OS page cache across workers within a tiny resident footprint (< 150 MiB RSS).
- Retains **zero** state, cache, or logs between requests, making it safe to share across all organizations on a node (`assistant` and `opendata` call it over gRPC or HTTP).
- Enforces strict input token length caps and per-org concurrency fairness.

## Public API
- HTTP: `POST /v1/embeddings`, `POST /v1/rerank`, `GET /healthz`
- gRPC / In-Process: `jc_embedder_api::EmbedderService`

## Standalone Binary
`jc-embedder`

## Configuration
- `EMBEDDER_MODEL_PATH` (path to `mmap`-able int8 model weights and tokenizer)
- `EMBEDDER_MAX_BATCH_TOKENS` (default `8192`)
- `EMBEDDER_LISTEN_ADDR` (default `127.0.0.1:8089`)

## Standards Implemented
- OpenAI `/v1/embeddings` JSON wire compatibility

## Origin Today
Replaces standalone Python/C++ embedding containers.
