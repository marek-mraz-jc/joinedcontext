# `dsc` (`jc-dsc`) — Multi-Participant Eclipse Dataspace Connector

## Scope
`dsc` implements a multi-tenant data space connector where each organization is a distinct dataspace participant (`did:web:{orgDomain}`) without running a separate JVM or pod per participant:
- Publishes per-organization dataspace catalogues from approved `Endpoint` and dataset definitions.
- Executes contract negotiations and transfer processes over the Eclipse Dataspace Protocol.
- Verifies Decentralized Claims Protocol (DCP 1.0) verifiable presentations.
- Evaluates W3C ODRL 2.2 access and contract policies, compiling active agreements into `Policy` grants enforced at `edge` and `broker`.

## Public API
- HTTP (DSP endpoints per organization): `/dsp/{org}/catalog/request`, `/dsp/{org}/negotiations/*`, `/dsp/{org}/transfers/*`, `/.well-known/did.json`
- gRPC / In-Process: `jc_dsc_api::DataspaceConnector`

## Standalone Binary
`jc-dsc`

## Configuration
- `DATABASE_URL` (role `jc_dsc`, schema `dsc`)
- `DSC_LISTEN_ADDR` (default `127.0.0.1:8084`)
- `IDENTITY_URL`, `BROKER_URL`

## Standards Implemented
- Eclipse Dataspace Protocol (DSP 2025-1)
- Decentralized Claims Protocol (DCP 1.0)
- W3C DID Core 1.0 (`did:web`)
- W3C ODRL 2.2

## Origin Today
Replaces **EDC (Eclipse Dataspace Components)** Java connectors.
