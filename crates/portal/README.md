# `portal` (`jc-control`) — Control Plane, Reconciler & Portal UI

## Scope
`portal` (`control`) owns the management API, manifest store, Green/Yellow/Red review lanes, stream and endpoint reconciler (rendering approved `Pipeline`, `DataSource`, and `Endpoint` manifests into `ekiben` stream configs), usage and quota configuration, hash-chained audit log (`audit_*`), and serves the embedded React 19 Portal UI.

## Public API
- HTTP REST & OData 4.01:
  - `/api/v1/organizations/{org}/...`
  - `/api/v1/projects/{project}/{plural}` (`datasources`, `endpoints`, `pipelines`, `datamodels`, `policies`, `apps`, `dashboards`, `connectors`, `changes`)
  - `/api/v1/ops/*` (operation registry for UI, CLI, and MCP assistant)
- gRPC / In-Process: `jc_control_api::ControlPlane`

## Standalone Binary
`jc-portal`

## Configuration
- `DATABASE_URL` (role `jc_control`, schema `control`)
- `EKIBEN_URL` (streams API endpoint for pipeline deployment)
- `FORGE_URL` (Git store for reviewed manifest revisions)
- `IDENTITY_URL` (OIDC + Zanzibar `Check`)

## Standards Implemented
- OASIS OData 4.01 (management queries)
- OpenAPI 3.1, RFC 9457 ProblemDetails
- LinkML data model validation

## Origin Today
Grows from **joinedcontext-portal** (`src/` Rust backend, `jcctl` reconciler, and `ui/` + `sdk/` TypeScript packages).
