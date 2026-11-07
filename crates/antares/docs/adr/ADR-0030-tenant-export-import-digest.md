# ADR-0030 — Tenant export, import and digest on the admin surface

Date: 2026-10-12. Status: accepted (owner may override). Decided by worker-6
for the platform's space move between brokers (ADR-N-046 §2.6, PF-115).
Task T-4001.

## Context

The platform moves a Context Space, one Tenant here, from one broker to
another: hold the writers, copy, compare an entity count and a hash over
(id, `modifiedAt`) read on both sides, then switch. The NGSI-LD API cannot
carry that copy:

- `entityOperations/upsert` stamps a fresh `modifiedAt` and `createdAt`
  on the target (5.2.2: "shall not be provided by the client"), so the
  hash of the copy never equals the hash of the source;
- the history (`attr_instances`) is not an Entity: a copy through the
  current-state API leaves it behind, and the temporal API restamps it.

`/q/` already lists, reads and purges a Tenant (`docs/src/admin-api.md`);
the store already pages every document kind by id, `Kind::Temporal`
included (`list_page`), and creates a document as given.

## Decision

1. **Three routes under `/q/tenants/{tenant}`**, behind the admin secret
   like the rest of `/q/`, the Tenant named by the path only:
   - `GET …/digest` → `{tenant, entities, attrInstances, sha256}`.
   - `GET …/export` → `application/x-ndjson`.
   - `POST …/import` → `204`, or a problem naming the line.
2. **The digest** is a multiset hash: the sum modulo 2²⁵⁶ of
   `sha256(id ‖ 0x00 ‖ modifiedAt)` over the Tenant's Entities, written as
   64 hex digits. A sum does not depend on the order a backend pages in,
   so a memory store and a Postgres store with different collations agree,
   and it streams in one page of memory. `attrInstances` is the temporal
   driver's own count.
3. **The export** is one line per record: a header
   `{"antaresExport":1,"tenant":…}`, then `{"kind":"entity","doc":…}` for
   every Entity and `{"kind":"temporal","doc":…}` for every history
   document, each as stored (system attributes kept), then a trailer
   `{"end":true,"records":n}`. A stream without the trailer is a cut stream.
   The export is not a point-in-time snapshot: the caller holds the
   writers first (the platform's gateway answers them `503`), and the
   digest proves the copy.
4. **The import** takes an export into a Tenant that holds nothing: no
   Entity, history, Subscription, registration, Snapshot, EntityMap,
   distributed subscription or hosted @context (`409` naming each count
   otherwise). It reads line by line, at most 16 MiB a line, writes each
   Entity as given, then replaces each history document with the exported
   one (writing an Entity records a history instance through the change
   hook; the replace makes the target's history the source's). A bad line,
   a Tenant mismatch or a missing trailer stops it and deletes, by id, the
   Entities and history it wrote, and nothing else; a Tenant the import
   created goes again once nothing is left in it. All or nothing, without a
   transaction across backends, and never at the cost of data another
   writer owns (T-4016).
5. **v1 moves data only.** A Tenant holding Subscriptions, Context Source
   Registrations or their Subscriptions, distributed subscriptions or
   Snapshots is refused by the export with `409` and the counts: each of
   those has a writer outside the store (subscription mirrors, the
   registration index, remote copies) that a raw copy would bypass. In the
   platform the reconciler writes registrations per space and re-renders
   views, policy documents and schemas for the target broker; moving
   Subscriptions is a later decision.

## Consequences

- A move is export → import → digest on both sides, all on `/q/`.
- A driver whose `list_page` cannot walk `Kind::Temporal` exports no
  history; the `attrInstances` count of the digest then differs and the
  move fails rather than lose it.
- `list_page` over `Kind::Temporal` on Postgres still slices the whole
  list (its ponytail note in `any.rs`): a very large history makes the
  export refuse for volume until a keyset reader over instance rows lands.
