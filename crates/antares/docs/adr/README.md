# Architecture Decision Records

One file per irreversible decision, numbered, never rewritten.

## Index

| ADR | decision | status |
|---|---|---|
| [ADR-0001](ADR-0001-shared-schema-tenancy.md) | Shared-schema multi-tenancy (tenant_id + RLS) | accepted, amended (tenant inventory) |
| [ADR-0002](ADR-0002-nats-jetstream-bus.md) | NATS JetStream as the change bus (with a local mode) | accepted |
| [ADR-0003](ADR-0003-websocket-deferred.md) | WebSocket binding deferred out of v1 | accepted, superseded by ADR-0026 |
| [ADR-0004](ADR-0004-store-ladder-redb-file-mode.md) | Store ladder, redb as the `file`-mode durability shadow | accepted |
| [ADR-0005](ADR-0005-anystore-sync-facade.md) | AnyStore enum + synchronous Pg facade | accepted · enum seam superseded by ADR-0013, sync facade by ADR-0022 |
| [ADR-0006](ADR-0006-rls-vs-timescale-compression.md) | RLS and Timescale compression collide on attr_instances | accepted |
| [ADR-0007](ADR-0007-synchronous-temporal-recording.md) | temporal auto-recording stays in the write path | accepted (reverses the earlier bus-consumer design) |
| [ADR-0008](ADR-0008-browser-wasm-build.md) | The browser build: one crate, the same router, no fourth backend | accepted |
| [ADR-0009](ADR-0009-temporal-read-cutover.md) | Temporal read cutover: attr_instances becomes the read path | accepted, implemented |
| [ADR-0010](ADR-0010-egress-allow-private-by-default.md) | Private-range egress allowed by default | accepted, implemented |
| [ADR-0011](ADR-0011-tenant-specific-context-source-alias.md) | The Via pseudonym identifies a (Context Source, Tenant) pair | accepted, implemented |
| [ADR-0012](ADR-0012-durable-internal-state-doc-kinds.md) | Internal broker state lives in the store as doc kinds, keyed under reserved tenants | accepted, implemented |
| [ADR-0013](ADR-0013-storage-drivers.md) | storage drivers — current-state and temporal as separate traits | accepted; supersedes the enum half of ADR-0005; the driver-identity consequence superseded by ADR-0017 |
| [ADR-0014](ADR-0014-extension-hooks-model.md) | extension hooks — fixed phases, batch granularity | accepted; sink paragraph superseded by ADR-0016; two phases given a named user, and rule 1 an exception, by ADR-0020 |
| [ADR-0015](ADR-0015-notification-delivery-policy.md) | Notification delivery policy: one attempt by default, retries as transport, dead letters in the store | accepted, implemented |
| [ADR-0016](ADR-0016-notification-bindings-behind-the-sink-registry.md) | Notification bindings behind the sink registry | accepted, implemented |
| [ADR-0017](ADR-0017-drivers-are-named-not-enumerated.md) | A driver is identified by its name, not by an enum value | accepted, implemented |
| [ADR-0018](ADR-0018-ci-actions-pinned-by-tag.md) | CI actions are pinned by tag, third-party binaries by version | accepted, implemented |
| [ADR-0019](ADR-0019-remote-notification-endpoint.md) | The distributed-subscription notification receiver lives outside the ETSI namespace | accepted, implemented |
| [ADR-0020](ADR-0020-policy-seam.md) | The policy seam: one trait, one built-in engine, every engine an addon | accepted, amended (the narrowing marker header) |
| [ADR-0021](ADR-0021-jsonld-context-tenancy.md) | A stored `@context` belongs to the Tenant that stored it, a Cached copy to none | accepted, implemented |
| [ADR-0022](ADR-0022-async-storage-drivers.md) | The storage drivers are async; nothing blocks on a store call | accepted, implemented; supersedes the sync-facade half of ADR-0005 |
| [ADR-0023](ADR-0023-endpoint-views.md) | Endpoint views: one policy-narrowed NGSI-LD endpoint that stays compliant | accepted (owner may override) |
| [ADR-0024](ADR-0024-odata-library.md) | OData 4.01 for Rust: build our own standalone `odata-*` crates | accepted (owner may override) |
| [ADR-0025](ADR-0025-internal-context-sources.md) | Internal context sources: a registration that names this broker is served in process | accepted (owner may override) |
| [ADR-0026](ADR-0026-websocket-notification-channel.md) | WebSocket notification channel: one more sink, the TS 104 243 envelope, bounded and resumable | accepted (owner may override); supersedes ADR-0003 |
| [ADR-0027](ADR-0027-surfaces-architecture.md) | Surfaces architecture: the vanilla binary, plus feature-gated surface crates in one binary | accepted (owner may override); amends ADR-0025 rule 8 and ADR-0026 rule 1 |
| [ADR-0028](ADR-0028-residual-query-evaluation.md) | Standard queries over a view: NGSI-LD pushdown plus a bounded residual | accepted (owner may override) |
| [ADR-0029](ADR-0029-ogc-features-surface.md) | OGC API - Features as a view surface: `antares-ogc` and the protocol crate `ogc-cql2` | accepted (owner may override); supersedes decision O-1 of `gateway-to-broker-map.md` for OGC Features |
| [ADR-0030](ADR-0030-tenant-export-import-digest.md) | Tenant export, import and digest on the admin surface | accepted (owner may override) |
| [ADR-0031](ADR-0031-inbound-bridge-channels.md) | Inbound bridge Channels | accepted (owner may override) |

## Format

Nygard's fields — **Title, Status, Context, Decision, Consequences** —
plus one borrowed from MADR:

- **Confirmation**: how compliance with the decision can be checked — a
  named test, a CI job, a grep, or an explicit "manual review only".
  Every new ADR names its own fitness check; a decision nobody can verify
  drifts silently.

Shorter ADRs fold Context/Decision/Consequences into prose sections, as
the existing files do; the five concerns must all be answerable from the
text either way.

## Immutability policy

Append-with-status. An accepted ADR's body is frozen; when a decision
changes, a NEW ADR supersedes it and the old one's Status line gains
`superseded by ADR-00XX` (see ADR-0005/ADR-0013 and the reversal recorded
in ADR-0007). Never edit an old ADR's Decision to match new reality —
the record of what was believed, and when it stopped being true, is the
point of keeping them.
