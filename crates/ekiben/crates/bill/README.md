# `bill` (`jc-bill`) — Usage Metering, Spend-Cap Admission & Billing

## Scope
`bill` tracks per-organization resource consumption and enforces real-time spend caps:
- Maintains low-latency in-memory + PostgreSQL outbox counters for requests, egress bytes, stored entity bytes, `ekiben` pipeline CPU/messages, and LLM tokens.
- Provides a fast admission gate to `edge`, `ekiben`, and `assistant` so an organization exceeding its hard spend cap or quota is stopped cleanly (`429` / stream halt) without affecting other organizations.
- Compacts raw usage events into `org/{OrgId}/billing/` Parquet files, runs hourly rating rollups, and integrates with Stripe / invoicing webhooks.

## Public API
- HTTP: `/api/v1/organizations/{org}/usage`, `/api/v1/organizations/{org}/invoices`, `/webhooks/stripe`
- gRPC / In-Process: `jc_bill_api::BillingService` (`CheckAdmission`, `RecordUsage`)

## Standalone Binary
`jc-bill`

## Configuration
- `DATABASE_URL` (role `jc_bill`, schema `bill`)
- `OBJECT_STORE_URL` (`org/{OrgId}/billing/`)
- `BILL_LISTEN_ADDR` (default `127.0.0.1:8090`)

## Standards Implemented
- CloudEvents 1.0 / OpenMeter usage event schema

## Origin Today
Replaces ad-hoc quota checks across Portal and Gateway.
