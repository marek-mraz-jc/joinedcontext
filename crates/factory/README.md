# `factory` (`jc-factory`) — Code Factory, AI Builders & MicroVM Build Sandbox

## Scope
`factory` is the platform's automated software and configuration builder:
- **AI Builders:** Spec-driven builders for Apps, `ekiben` Pipelines (Bloblang & LinkML-Map), LinkML DataModels, Endpoints, and Dashboards using a CaMeL capability-separated architecture (Planner, Test-Writer, and Builder agents).
- **Build & Test Lane:** Compiles, tests (`ekiben` golden/unit tests, LinkML validation, WASM component builds), scans, signs, and publishes artifacts inside ephemeral Firecracker microVMs or gVisor sandboxes.
- **Package Mirror & Evidence Store:** Maintains a 7-day cooldown package registry mirror and stores 10-year Product Liability Directive (PLD) build/test evidence in `org/{OrgId}/builds/`.
- **Agent Work Board:** Optional per-organization autonomous task board and `jc-agent` harness.

## Public API
- HTTP: `/api/v1/projects/{project}/builds/*`, `/api/v1/factory/tasks/*`
- gRPC / In-Process: `jc_factory_api::FactoryService`

## Standalone Binary
`jc-factory`

## Configuration
- `DATABASE_URL` (role `jc_factory`, schema `factory`)
- `OBJECT_STORE_URL` (`org/{OrgId}/builds/`)
- `SANDBOX_DRIVER` (`firecracker`, `gvisor`, or `process` for local dev)

## Standards Implemented
- WASI 0.2 (`wasm32-wasip2`) Component Model
- SLSA v1.0 Provenance & in-toto attestations
- LinkML specification

## Origin Today
Replaces **OpenHands agent runner, Gitea runners, app-builder**, and the swarm orchestration scripts.
