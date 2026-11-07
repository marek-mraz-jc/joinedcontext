# `dash` (`jc-dash`) — Grafana-Compatible Dashboards, Panels & Alerting

## Scope
`dash` is the platform's native Rust dashboard, visualization, and alerting server, replacing Grafana for organization users while maintaining 5-level Grafana compatibility:
1. Imports and exports Grafana classic and v2 dashboard JSON schemas (and CNCF Perses schemas).
2. Serves Grafana-compatible `/api/dashboards/*`, `/api/datasources/proxy/*`, `/api/ds/query`, and `/apis/dashboard.grafana.app/*` endpoints.
3. Queries NGSI-LD Endpoints, OData 4.01, read-only SQL views, Prometheus (`PromQL`), and logs (`LogQL`) over a built-in S3/Parquet columnar telemetry store.
4. Evaluates multi-dimensional alert rules with contact points and silences.
5. Renders panels using the Portal SDK chart/grid/map components and sandboxes custom visualization plugins on an isolated origin (`viz.{domain}`).

## Public API
- HTTP: `/api/dashboards/*`, `/api/ds/query`, `/api/v1/provisioning/*`, `/apis/dashboard.grafana.app/*`
- gRPC / In-Process: `jc_dash_api::DashService`

## Standalone Binary
`jc-dash` (standalone dashboard server over any NGSI-LD, OData, SQL, or Prometheus data source).

## Configuration
- `DATABASE_URL` (role `jc_dash`, schema `dash`)
- `OBJECT_STORE_URL` (for `org/{OrgId}/telemetry/` Parquet segments)
- `DASH_LISTEN_ADDR` (default `127.0.0.1:8087`)

## Standards Implemented
- Grafana Dashboard JSON Schema v1 & v2, CNCF Perses specification
- PromQL, LogQL, OpenMetrics

## Origin Today
Replaces **Grafana** for organization users.
