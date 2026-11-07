---
title: Coraine's plugin categories against Antares' surfaces and drivers
task: T-5063
---

# Coraine's plugins and Antares' seams

[Antares and Coraine](coraine-comparison.md) (T-3548, Coraine at `19cff16`) compared the two brokers as a whole. This page takes Coraine's plugin architecture category by category, names the Antares seam that does the same work, and decides for each idea whether Antares adopts it. It ends with what made a new standard surface cheap here: a generator, `dev/new-surface.py`, and the line count of the surface it produced.

Sources: Coraine `main` at `44da1e7` (2026-10-21), read from `doc/plugin-architecture.md`, `doc/bridge-channels.md`, `doc/bridge-channels-details.md`, `doc/authorization.md` and `src/lib/plugin/*.h`; Antares `dev` at `a60ea0d3`. Refreshed by T-5210 (2026-10-22) against Coraine `3197d71` and Antares `dev` `4e68f990`: the cites below hold there, the Antares ones by symbol. Coraine changed since T-3548: authorization was "working notes, not built" at `19cff16` and is now a loaded plugin category with `fdsc` and `oidc` drivers.

## 1. Category by category

| Coraine category | Coraine evidence | Antares equivalent | Antares evidence |
|---|---|---|---|
| Current-state DB, one active (`mongoc`, `corDB`, `ramDB`) | `doc/plugin-architecture.md:15`, table at `:306-308`; seam `src/lib/db/DbDriver.h` | `CurrentStateDriver`, named drivers `memory`, `file`, `postgres`, `timescale` | `docs/src/storage.md:9`, ladder at `:18`; ADR-0013, ADR-0017 |
| History (TRoE), one active (`none`, `corDB`, `ramDB`, `timescale`) | `doc/plugin-architecture.md:20`, `:309-312` | `TemporalDriver`, `ANTARES_TEMPORAL` (`none`, `memory`, `file`, `postgres`, `timescale`), any pairing with the store | `docs/src/storage.md:48-60`; a second store that is down: current state served, history fails closed and the lost events are counted ([temporal.md](../src/temporal.md#when-the-history-store-is-down), T-5210). Coraine since `44da1e7`: `--troeSync` (8d25453), history database down serves current state, holds, retries, then loses and counts, `--troeDownExit` (e34f793, 4588d4a), `--troeIndexes` (9c0bd16), batched TimescaleDB writers (ab55811) |
| API services, up to 16 (`admin`) | `doc/plugin-architecture.md:32`; `src/lib/plugin/ApiPlugin.h:35-46`: a flat `services[]` table, verb in each entry | `Surface` (`crates/antares-ops/src/lib.rs:858`) mounted as `ApiSurface` (`crates/antares-api/src/surface.rs:18`), selected by `ANTARES_API_SURFACES`: `admin`, `views`, `mcp`, `linkml`, `odata`, `ogc`, `sta`, `ical`, `geojson`, `ws`, `ws-views`, `example` and the `jc-policy` access surface | `crates/antares-broker/src/main.rs`, `SURFACE_SHELF` |
| Communication protocols, up to 4 (`ws`) | `doc/plugin-architecture.md:37`; `src/lib/plugin/TransportDriver.h:58-82` | the `ws` surface (ADR-0026), the API and notifications over one WebSocket | `crates/antares-ws`, `docs/src/websocket.md` |
| Bridges, up to 8 (`loopback`, `mqtt`, `modbus`, `dds`, `ckan`, `opcua`): an endpoint mapped to an attribute by a Channel, direction `in`/`out`/`both`, retention `mirror`/`relay` | `doc/plugin-architecture.md:46-67`, `:78-102`; `doc/bridge-channels.md:20`, `:61-62`; `doc/bridge-channels-details.md:548-560` | outbound only: the `NotificationSink` registry delivers to `http` and `mqtt` endpoints; inbound data comes from outside the broker (Bento pipelines of the platform write through an Endpoint) | `crates/antares-notifier/src/lib.rs`, `crates/antares-notifier/src/mqtt.rs`; ADR-0016 |
| Auth, at most one (`fdsc`, `oidc`): authenticate, authorize, per-entity and per-notification decisions inside the broker | `doc/plugin-architecture.md:69-74`; `src/lib/plugin/AuthDriver.h:151-231`; `doc/authorization.md:3-25` | `PolicyEngine` seam (ADR-0020): `decide` per operation, `Filter` narrowing a query, `pre_notify` per notification; the platform's engine `joinedcontext` runs inside the broker on the subject the gateway verified | `crates/antares-api/src/policy.rs:1111`; `addons/joinedcontext-policy/README.md` |
| Loading: `dlopen` at startup, per-family ABI stamp (`dbAbiStamp.h`, `AUTH_ABI_VERSION`) | `src/lib/plugin/pluginLoader.c:77-78` (the DB stamp check), `src/lib/plugin/AuthDriver.h:45` (`AUTH_ABI_VERSION 6`); `doc/plugin-architecture.md:177` | Cargo features and traits, checked by the compiler; module rules M1 to M3 keep a surface off the store | `docs/src/extending.md:592` (layer 3 not built); `dev/check-module-rules.py` |

Both brokers already cut the broker along the same lines: storage, history, extra APIs, transports and authorization are separate seams in each. The differences are when the cut is made (Coraine at start-up, Antares at build time) and that Coraine has two seams Antares does not: inbound bridges and a Channel model.

## 2. What Antares adopts and what it does not

| Idea | Decision | Reason |
|---|---|---|
| Runtime plugin loading (`.so` + ABI stamp) | Not adopted | A loaded library is code no `cargo deny`, `cargo audit`, 7-day cooldown, SBOM or module-rule check has seen; Rust has no stable ABI, so it would need `#[repr(C)]` vtables and a stamp per family as Coraine carries. `ANTARES_API_SURFACES` already selects at start-up among what the build holds, which is the operational half of plugins. Same decision as T-3548 section 5 and `docs/src/extending.md:592`. |
| (a) Bridges as Channels, `relay` retention | Filed: `tasks/later/T-5079` | Filed: `tasks/later/T-5079` (inbound Channels and the no-current-state write of `relay`) | Coraine `relay` means "put it on the wire and keep nothing", the current state included (`doc/bridge-channels-details.md:552-556`). Half of it is built: T-5056 skips the history row per type (`ANTARES_TEMPORAL_RECORD=ruled` with `record: never`; deployment sets `ruled`, the binary default stays `all`). What T-5079 still adds is the attribute that is notified and not written to the current state; its Done when gets the test "a relay attribute is notified and absent from GET /entities/{id} and from history". Inbound Channels pay off for the Helsinki-vehicles class of load (T-5046) and need their own threat model (who may bind a topic to a tenant's attribute). 
| (b) An in-broker ODRL decision as `fdsc` | Not adopted as a second decider | Antares already decides in the broker: the `PolicyEngine` seam sees the parsed operation, narrows a query with a `Filter` and decides per notification, the things `doc/authorization.md:23-25` lists as the reason to decide inside. The platform's gateway PDP and its ODRL/AuthZEN model (T-5035, ADR-N-057) produce the rules the `joinedcontext` engine evaluates; ODRL input is T-5039. A second engine reading ODRL policies directly would be two sources of truth for one decision. |
| (c) An embedded store for small deployments (`corDB`, `ramDB`) | Already built | `ANTARES_STORE=memory` and `file` (redb write-through, about 3,100 fsynced writes per second, `docs/src/storage.md:40-46`) run with no PostgreSQL; `memory` is Coraine's `ramDB`, `file` its `corDB --dbDir`. |
| `loopback` reference bridge | Taken with (a) | A bridge that answers from its own thread is the test double T-5079 needs before a real MQTT bridge. |
| Server-sent events for notifications (`doc/sse.md`) | Not adopted | A subscriber that holds a connection already has the `ws` surface (ADR-0026) with the same notifications; HTTP and MQTT endpoints cover the rest. A second push transport is a second set of limits and auth paths for the same thing. |
| Service Execution per GR CIM-055 (`doc/service-execution.md`) | Not adopted | A group report, not a specification Antares states compliance against; calling a device or service is a Pipeline's or an App's job on this platform, behind its own Policy. |
| Several broker instances on one MongoDB (`doc/high-availability.md`) | Already built | Several Antares pods serve one PostgreSQL: views and schemas reload from the store with a revision every pod reports (`viewsRevision`), subscriptions and the outbox are store rows. |
| AssetChannels for OPC UA | Taken with (a) | The same inbound seam as MQTT Channels; T-5079 decides the threat model once for both. |
| `mongoc` storage format 2 (unreleased 0.6.0, `CHANGELOG.md:21-24`, no rolling upgrade) | Not applicable | Antares stores no MongoDB format. The lesson it carries is kept already: sqlx migrations never change once applied, and an unknown applied version refuses to start. |
| A flat service table per API plugin | Not needed | An axum `Router` per surface is the same table, typed. The cost of a new surface was the wiring, which section 3 removes. |

## 3. A new standard surface as a generated crate

Before T-5063 a new surface meant a crate by hand plus ten edits outside it: the workspace members, the workspace dependency, a release profile, the broker's optional dependency and feature, the `SURFACE_SHELF` entry, the `ARCHITECTURE.md` row, the book summary, the feature table in `docs/src/surfaces.md`, the vanilla-build gate (`dev/check-no-addon.sh`), and the tests that prove the view and the Policy decide. `dev/new-surface.py NAME --title … --description …` writes all of it from `dev/new-surface/*.in`:

- `crates/antares-NAME`: `Cargo.toml` with role `surface`, `antares-ops` as the only workspace dependency and the full lint table; `src/lib.rs` serving `GET /x/views/{view}/NAME/v1/{type}` through `ViewOps::scan` with `limit`, `offset`, `q`, `scopeQ` and `lang`, read only, and a `render` function the author replaces; `tests/NAME.rs` with seven tests that hold for any `render`: a hidden Attribute is absent, a request naming another Tenant is 400, a hidden type and a view without `query_entities` are 404, a `PolicyEngine` that denies reads gives 403 with no data, a write is 405 and stores nothing, paging and `q` narrow, bad parameters are 400.
- `docs/src/NAME.md` and the summary line; `tests/NAME/README.md`, the conformance pack: the template's rows and the place for the standard's own.
- The ten edits above, each anchored on one line and refused before anything is written when the anchor moved (`dev/new-surface.py --self-test`, run in CI).

Security: the generated crate has no store handle to misuse. Module rule M2 (`dev/check-module-rules.py`) fails the build if it names any workspace crate but `antares-ops`, and `the_policy_decides_every_read` proves each read reaches the policy seam.

The gateway's route for a representation (`crates/context-gateway/src/handlers/ogc.rs` in joinedcontext-platform: a `Representation`, a `Surface` constant and a route) stays a hand edit in that repository, listed in `docs/src/new-surface.md`: it is another repository with its own review, and a broker surface serves only after an Endpoint asks for it.

### 3.1 The proof: `antares-geojson`

`dev/new-surface.py geojson --struct GeoJson` generated `crates/antares-geojson`; the hand-written part is `render` as an RFC 7946 FeatureCollection (each `location` the geometry, the other Attributes plain values under `properties`) and its two tests.

| Part | Lines |
|---|---|
| Generated: `src/lib.rs` 176, `tests/geojson.rs` 240, `Cargo.toml` 48 | 464 |
| Hand-written in `src/lib.rs`: `render`, `feature`, `first`, `plain` (added 57, removed 5) | 57 |
| Hand-written in `tests/geojson.rs`: the two GeoJSON tests | 68 |
| Hand-written docs: `docs/src/geojson.md` 10, the pack's six RFC 7946 rows 6 | 16 |
| **Hand-written in all** | **141** (125 of them Rust) |

Counted with `git diff --numstat` between the commit that holds the crate as generated and the next one. The target was under 300 hand-written lines for a read-only surface. The generated part passed its seven tests, `clippy -D warnings`, module rule M2 and the 100 % line and branch coverage gate before a line was written by hand; with `render` added, nine tests and the coverage gate pass (`dev/coverage-gate.py --check antares-geojson`).

## Related

- [Antares and Coraine](coraine-comparison.md)
- [Surfaces architecture](../adr/ADR-0027-surfaces-architecture.md)
- [Policy seam](../adr/ADR-0020-policy-seam.md)
- [New surface](../src/new-surface.md)
