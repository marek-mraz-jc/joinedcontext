# `gateway` (`jc-gateway`) — Context Gateway Compatibility Layer

## Scope
`gateway` provides drop-in endpoint routing, OAuth2 audience validation, PDP policy grant resolution, and NGSI-LD `/api/endpoint/{slug}/ngsi-ld/v1/*` proxying compatible with today's `context-gateway` deployment configs while delegating ingress to `jc-edge` and storage/query execution to `jc-data`.

## Public API
- HTTP: `/api/endpoint/{slug}/ngsi-ld/v1/*`, `/api/endpoint/{slug}/ogc/*`, `/api/endpoint/{slug}/sta/*`, `/api/endpoint/{slug}/odata/*`, `/healthz`, `/readyz`

## Standalone Binary
`jc-gateway`

## Configuration
- `GATEWAY_LISTEN_ADDR` (default `127.0.0.1:8081`)
- `BROKER_URL`, `IDENTITY_URL`, `CONTROL_URL`

## Standards Implemented
- ETSI NGSI-LD API gateway routing, OGC API Features, OGC SensorThings, OData 4.01

## Origin Today
Replaces **context-gateway** (`joinedcontext-gateway` Rust repository).
