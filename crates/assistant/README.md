# `assistant` (`jc-assistant` / `ai`) — Knowledge Assistant, Hybrid RAG & LLM Proxy

## Scope
`assistant` (`ai`) powers conversational search, document & context-data RAG, scoped MCP tool execution, and the multi-tenant LLM gateway:
- Crawls and extracts structured text from PDFs, HTML, and office documents (`kreuzberg`).
- Indexes chunks into `org_id`-partitioned PostgreSQL tables (`ai.chunks`) combining `pgvector` HNSW semantic search and PostgreSQL full-text search via Reciprocal Rank Fusion (RRF).
- Executes authorized operations through the Portal operation registry and scoped MCP connectors (`/c/{id}`).
- Proxies LLM requests with per-organization token/spend budgets (`bill`), Bring-Your-Own-Key (BYOK) envelope encryption, and per-organization prompt-cache salting (`ISO-AI`).

## Public API
- HTTP: `/api/v1/assistant/chat`, `/api/v1/assistant/collections/*`, `/c/{connector_id}/mcp`
- gRPC / In-Process: `jc_assistant_api::AssistantService`

## Standalone Binary
`jc-assistant`

## Configuration
- `DATABASE_URL` (role `jc_ai`, schema `ai`, requires `pgvector`)
- `EMBEDDER_URL` (`jc-embedder` gRPC/HTTP address)
- `ASSISTANT_LISTEN_ADDR` (default `127.0.0.1:8088`)

## Standards Implemented
- Model Context Protocol (MCP 2025-03-26)
- OpenAI / Anthropic compatible streaming chat completions wire format

## Origin Today
Replaces **jc-assistant** and **agent-proxy**.
