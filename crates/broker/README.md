# `broker` (`jc-data`) — Multi-Tenant NGSI-LD Context Broker

## Scope
`broker` is the platform's multi-tenant context data store and query engine. It partitions entities, relationships, temporal history, and subscriptions by `OrgId` (`PRIMARY KEY (org_id, id)` with `FORCE ROW LEVEL SECURITY` on schema `data`), compiles Endpoint access views and ODRL/Zanzibar grants into SQL predicates, and compacts aged history windows into Apache Iceberg v3 / GeoParquet tables in object storage queryable via Apache DataFusion.

Sub-crates (each under 250k tokens):
- `jc-data-core`: storage engine, PostGIS/JSONB query planner, view compiler, history partition manager
- `jc-data-ngsild`: ETSI NGSI-LD HTTP REST API, `@context` JSON-LD expansion/compaction, batch `/entityOperations/*`
- `jc-data-odata`: OData 4.01 query and entity set representation
- `jc-data-ogc`: OGC API Features Parts 1–5, CQL2 filter parser, OGC API EDR
- `jc-data-sta`: OGC SensorThings API 1.1 & 2.0
- `jc-data-mcp`: Model Context Protocol read/query tools over Endpoint views
- `jc-data-ws`: WebSocket entity change fan-out and subscriptions
- `jc-data-graph`: SPARQL 1.1 and openCypher/GQL read query translators over `data.entity_triples` and `data.entity_edges`
- `jc-data-api`: protobuf/Arrow Flight trait contract (`DataBroker`)

## Public API
- HTTP: `/ngsi-ld/v1/entities`, `/ngsi-ld/v1/entityOperations/{upsert,create,update,delete,query}`, `/ngsi-ld/v1/temporal/entities`, `/ngsi-ld/v1/subscriptions`, `/ngsi-ld/v1/csourceRegistrations`, `/odata/v4/*`, `/ogc/features/*`, `/sta/v1.1/*`
- gRPC / In-Process: `jc_data_api::DataBroker` (Arrow `RecordBatch` streaming)

## Standalone Binary
`jc-data` (runs with only PostgreSQL 16+ / PostGIS and optional S3 object store).

## Configuration
- `DATABASE_URL` (role `jc_data`, schema `data`)
- `OBJECT_STORE_URL` (`s3://...` or `file://...` for `org/{OrgId}/lake/` cold history)
- `MAX_BODY_BYTES` (default `4194304`)
- `LISTEN_ADDR` (default `127.0.0.1:8080`)

## Standards Implemented
- ETSI GS CIM 009 / TS 104 175, 104 176, 104 178 (NGSI-LD)
- OASIS OData 4.01
- OGC API Features Parts 1–5, CQL2, OGC API EDR
- OGC SensorThings API 1.1 & 2.0 (23-019)
- W3C SPARQL 1.1 & ISO GQL / openCypher (read subset)

## Origin Today
Moves in today's **Antares** Rust broker (`antares` repository, ~188k LoC Rust) with `org_id` partitioning added.
