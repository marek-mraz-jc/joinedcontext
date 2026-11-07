# `edge` (`jc-edge`) — Multi-Tenant TLS, Routing, Rate-Limiting & Pre-Check Proxy

## Scope
`edge` is the single public ingress point of the platform. It terminates TLS 1.3 (`rustls` + `aws-lc-rs` with `X25519MLKEM768` post-quantum hybrid key exchange and `instant-acme` automated certificates), serves HTTP/1.1, HTTP/2, and WebSockets, scrubs untrusted client headers, verifies OIDC/OAuth 2.1/DPoP tokens, enforces per-organization token buckets and weighted fair queues (WFQ), checks real-time spend caps (`bill`), performs NGSI-LD write pre-checks (URN syntax, UN/CEFACT unit codes, `@context`), signs `RequestCtx`, and routes requests to `broker`, `portal`, `identity`, `forge`, `opendata`, `dsc`, `dash`, and `assistant`.

## Public API
- Public HTTPS/WSS ingress: `/api/endpoint/{slug}/*`, `/api/v1/projects/{project}/{plural}`, `/c/{id}` (MCP), `/.well-known/*`
- Internal gRPC / In-Process: `jc_edge_api::RouteTableControl` for live route and quota updates

## Standalone Binary
`jc-edge`

## Configuration
- `EDGE_HTTPS_ADDR` (default `0.0.0.0:443`), `EDGE_HTTP_ADDR` (default `0.0.0.0:80`)
- `EDGE_ACME_DIRECTORY`, `EDGE_ACME_STORE`
- `EDGE_SIGNING_KEY_FILE` (Ed25519 key for signing internal `RequestCtx`)
- `EDGE_DEFAULT_ORG_RPS`, `EDGE_WFQ_QUANTUM_BYTES`

## Standards Implemented
- RFC 8446 (TLS 1.3), RFC 8555 (ACME), RFC 9113 (HTTP/2), RFC 6455 (WebSocket)
- RFC 9449 (OAuth 2.0 DPoP), ETSI NGSI-LD entity validation rules

## Origin Today
Replaces **APISIX, Traefik, cert-manager**, and the ingress/PEP layer of **context-gateway**.
