# Antares

**An NGSI-LD Context Broker in Rust.** ETSI CIM 009 V1.9.1, one native
binary, about 35 MiB of memory under the full conformance suite, and a
WebAssembly build that runs the same broker inside a web page.

📖 **Docs:** <https://antaresbroker.joinedcontext.com/>

🚀 **Demo:** <https://antaresbroker.joinedcontext.com/demo/> — the context
broker running in your browser, yes in the browser 😅

[![ci](https://github.com/joinedcontext/Antares-NGSI-LD-Context-Broker/actions/workflows/ci.yml/badge.svg)](https://github.com/joinedcontext/Antares-NGSI-LD-Context-Broker/actions/workflows/ci.yml)
[![strict](https://github.com/joinedcontext/Antares-NGSI-LD-Context-Broker/actions/workflows/strict.yml/badge.svg)](https://github.com/joinedcontext/Antares-NGSI-LD-Context-Broker/actions/workflows/strict.yml)
[![ETSI conformance](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge.json)](https://antaresbroker.joinedcontext.com/reports/latest/)
[![NGSI-LD conformance](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-postgres.json&label=NGSI-LD%20conformance)](#etsi-conformance-results)
[![unit tests](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-unit.json)](https://antaresbroker.joinedcontext.com/reports/unit/)
[![coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fcoverage-badge.json)](https://antaresbroker.joinedcontext.com/reports/coverage/)
[![docs](https://img.shields.io/badge/docs-antaresbroker.joinedcontext.com-blue)](https://antaresbroker.joinedcontext.com/)
[![license: EUPL-1.2](https://img.shields.io/badge/license-EUPL--1.2-blue)](LICENSE)
[![release](https://img.shields.io/github/v/release/joinedcontext/Antares-NGSI-LD-Context-Broker?include_prereleases)](https://github.com/joinedcontext/Antares-NGSI-LD-Context-Broker/releases)

## Why Antares

- **Small.** About 35 MiB average RSS while the complete ETSI suite runs,
  about 9 MiB idle. The whole store ladder fits where a JVM heap alone
  would not.
- **Conformant.** 1833/1833 ETSI CIM 009 V1.9.1 test cases green in every
  native store mode, and 1821/1821 in the browser build, where the MQTT
  cases have no socket to run against
  ([per-store report with Robot drill-down](https://antaresbroker.joinedcontext.com/reports/latest/)).
  The ledger covers the whole spec text, one file per clause, in `docs/spec/`.
- **Runs anywhere.** Zero infrastructure by default, PostgreSQL for
  production, NATS JetStream for scale-out, and a 4 MB wasm artifact that
  serves `/ngsi-ld/v1/*` from a Service Worker with nothing installed.
- **Tenants without a ceiling.** A tenant is a row in one shared schema,
  isolated by `tenant_id` and Row-Level Security, and exists from the
  first request that names it. Nothing in the broker caps the count, so
  one deployment can hand every employee, department and use case a
  tenant of its own, 100,000 of them on one Postgres cluster.

## ETSI conformance results

The latest full run of the ETSI NGSI-LD test suite (CIM 009 V1.9.1), one
cell per store mode. The badges read the live report, so they always show
the newest run.

The suite in [`ngsi-ld-test-suite/`](ngsi-ld-test-suite/) started from the
[ETSI NGSI-LD test suite](https://forge.etsi.org/rep/cim/ngsi-ld-test-suite)
and was corrected and extended; no test was weakened.

| Store | Setup | Result |
|---|---|---|
| `memory` | in-RAM, zero dependencies | [![memory](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-memory.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `file` | redb file store | [![file](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-file.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `postgres` | PostGIS | [![postgres](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-postgres.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `timescale` | PostGIS + TimescaleDB | [![timescale](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-timescale.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `postgres-nats` | PostGIS + NATS JetStream, ten containers rolled during the run | [![postgres-nats](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-postgres-nats.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `timescale-nats` | TimescaleDB + NATS JetStream, same rolling fleet | [![timescale-nats](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-timescale-nats.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| `wasm-file` | WebAssembly build over the file store, MQTT excluded | [![wasm-file](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-wasm-file.json)](https://antaresbroker.joinedcontext.com/reports/latest/) |
| unit + integration | `cargo test` | [![unit](https://img.shields.io/endpoint?url=https%3A%2F%2Fantaresbroker.joinedcontext.com%2Freports%2Fbadge-unit.json)](https://antaresbroker.joinedcontext.com/reports/unit/) |

## Quickstart

```bash
docker run --rm -p 9090:9090 ghcr.io/joinedcontext/antares-broker:latest
```

```bash
curl -i -X POST localhost:9090/ngsi-ld/v1/entities \
  -H 'Content-Type: application/ld+json' \
  -d '{
    "id": "urn:ngsi-ld:TemperatureSensor:001",
    "type": "TemperatureSensor",
    "temperature": {"type": "Property", "value": 21.5, "unitCode": "CEL"},
    "@context": "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld"
  }'
# HTTP/1.1 201 Created

curl -s 'localhost:9090/ngsi-ld/v1/entities?type=TemperatureSensor'
```

Without Docker, `cargo run -p antares-broker` serves the same API on port
9090 (Rust 1.97 or newer). Continue with the
[Getting started](docs/src/getting-started.md) chapter.

## Store modes

One binary, one setting (`ANTARES_STORE`), the same API in every mode.

| Mode | Backend | Durability | Config |
|---|---|---|---|
| `memory` (default) | in-memory maps | none | |
| `file` | memory + [redb](https://www.redb.org/) write-through shadow, fsync before every ack | process restarts | `ANTARES_DATA_DIR` on a mounted volume |
| `postgres` | PostgreSQL + PostGIS | WAL, ordinary Postgres backup and PITR | `ANTARES_DATABASE_URL` |
| `timescale` | PostgreSQL + TimescaleDB for history | WAL | `ANTARES_DATABASE_URL` |

`memory` for tests and demos, `file` for a durable single node without a
database, `postgres` for production, `timescale` when temporal queries
dominate. `ANTARES_BUS=nats` adds JetStream for multi-pod scale-out and
rolling updates; MQTT notifications are built in and chosen per
subscription endpoint. Details, measured costs and backup procedures:
[Storage drivers](docs/src/storage.md) and [Operations](docs/src/operations.md).

```bash
# durable single node, no Postgres
docker run --rm -p 9090:9090 -e ANTARES_STORE=file -e ANTARES_DATA_DIR=/data \
  -v antares-data:/data ghcr.io/joinedcontext/antares-broker:latest

# postgres / timescale
docker run --rm -p 9090:9090 -e ANTARES_STORE=postgres \
  -e ANTARES_DATABASE_URL=postgresql://antares:antares@db:5432/antares \
  ghcr.io/joinedcontext/antares-broker:latest

# local stacks: broker + PostGIS + NATS + mosquitto
docker compose -f compose-files/docker-compose.yml up       # one broker
docker compose -f compose-files/docker-compose-ha.yml up    # two replicas + haproxy + NATS
```

Image tags: `:dev` is the latest green `dev`, `:dev-<run>` one CI run,
`:0.1.3` one release, `:0.1` the newest patch of a minor, `:latest` the
latest release. Images are multi-arch (amd64, arm64). The
role-split fleet (`--roles api,matcher,notifier,temporal,registry`) and
the Kubernetes manifests are in [Deployment](docs/src/deployment.md).

## Antares without the platform

Antares is its own product: the joinedcontext platform's surfaces and
extensions are Cargo features, and the default build carries none of them.
`cargo build -p antares-broker` and the `ghcr.io/joinedcontext/antares-broker`
image (signed with cosign, SPDX and CycloneDX SBOMs on every release) serve
NGSI-LD (ETSI GS CIM 009 V1.9.1) and the `/q` admin surface, nothing else.
The platform builds `antares-broker-jc` with
`--features full,jc-policy,linkml,parquet,tabular,ical,feed`.

```bash
docker compose -f compose-files/docker-compose-postgres.yml up   # broker + PostGIS only
```

What the vanilla build leaves out, and the feature that adds it back:

| Platform behaviour | Feature |
|---|---|
| `JC-Provenance-Channel`, `JC-Provenance-Actor`, `JC-Reason`, `X-Request-Id` recorded into history; temporal `versions=all` (a 400 `unknown query parameter` in vanilla) | `jc` (implied by `jc-policy`) |
| the platform's Policy engine (`ANTARES_POLICY=joinedcontext`) | `jc-policy` |
| Endpoint views and their surfaces: MCP, LinkML, OData, OGC API Features, SensorThings, iCalendar, WebSocket | `views`, `mcp`, `linkml`, `odata`, `ogc`, `sta`, `ical`, `ws`, `full` |

What the vanilla build keeps, each an implementation choice the
specification leaves to the broker:

- **Page sizes**: 100 Entities when the request names no `limit`, at most
  1000 (`ANTARES_DEFAULT_LIMIT`, `ANTARES_MAX_LIMIT`); 6.3.9 leaves both to
  the broker; a larger `limit` is a 403 TooManyResults (5.5.6).
- **Pre-adopted routes**: `GET /entities/{id}/attrs/{attr}` and
  `/entities/{id}/attrs/{attr}/value` (NGSI-LD 2.0 items 14 and 15, TS 104 176
  7.6.3.2). They add read paths and change no 1.9.1 operation.
- **`POST /ex/v1/remote-notify`**: the callback of distributed subscriptions
  (5.8.1.4). The specification requires the forwarding and leaves the
  callback URL to the broker; `/ex/` keeps it outside `/ngsi-ld/v1`.

CI holds the line: `dev/check-vanilla-symbols.sh` refuses a vanilla binary
that links a surface crate or the `jc` code, `ci/vanilla-tree.txt` pins its
dependency tree, and the ETSI quick matrix runs on the vanilla image every
commit. Each release's notes carry the compliance statement: suite version
and tree, pass count, and the local suite edits.

## Browser build

The broker compiles to `wasm32-unknown-unknown` and answers `/ngsi-ld/v1/*`
from a Service Worker; `www/index.html` is the playground.

```bash
./dev/install-wasm-tools.sh   # wasm-bindgen + wasm-opt
./dev/wasm-build.sh           # → www/pkg
node www/node-shim.mjs 9090   # the same .wasm behind a TCP port
./dev/wasm-test.sh            # Node smoke + headless-Chromium page test
```

Scope: memory store and local bus only. See [Browser & WebAssembly](docs/src/wasm.md).

## Observability

```bash
curl -s localhost:9090/q/health    # liveness, store mode, bus state
curl -s localhost:9090/q/ready     # readiness: store ping + bus connected
curl -s localhost:9090/q/metrics   # Prometheus, antares_ prefixed
```

Traces and logs export over OTLP/HTTP when `ANTARES_OTLP_ENDPOINT` is set.
Every route under `/q/` is in the [Admin API](docs/src/admin-api.md) chapter.

## Conformance

The [ETSI NGSI-LD test suite](https://forge.etsi.org/rep/cim/ngsi-ld-test-suite)
is vendored in `ngsi-ld-test-suite/` and runs in CI against every store
mode, including a ten-container role-split fleet that rolls under the
suite and the browser build behind a Node shim. The vendored copy carries a
few test-side fixes, each proven from the clause text and listed in
[docs/upstream/etsi-raises.md](docs/upstream/etsi-raises.md); no test is
weakened to fit the broker.

```bash
cargo test --workspace -j 2                 # unit and integration tests
dev/etsi-local.sh                           # workspace tests + the suite, memory store
STORE=postgres dev/etsi-local.sh            # the store mode you are touching
```

How to read the matrix and the ledger: [Conformance](docs/src/conformance.md).

## Documentation

The book is published at <https://antaresbroker.joinedcontext.com/>
and built from `docs/src`. Maintainers start at
[ARCHITECTURE.md](ARCHITECTURE.md).

| Chapter | Covers |
|---|---|
| [Getting started](docs/src/getting-started.md) | install, first entity, first subscription, first federation pair |
| [Configuration](docs/src/configuration.md) | every `ANTARES_*` variable, with defaults |
| [Deployment](docs/src/deployment.md) | Docker, compose stacks, Kubernetes, the role split |
| [Subscriptions](docs/src/subscriptions.md) | HTTP and MQTT delivery, retries, dead letters |
| [Temporal API](docs/src/temporal.md) | recording, querying, aggregation, retention |
| [Federation](docs/src/federation.md) | registrations, forwarding, loop protection |
| [Operations](docs/src/operations.md) | health, tenants, backup and restore, bulk load, rolling updates |
| [Storage drivers](docs/src/storage.md) | the store ladder, the temporal driver, migrations |
| [Performance](docs/src/performance.md) | the weekly shape and scale runs and how to reproduce them |
| [Shared crates](docs/src/shared-crates.md) | the parser, query engine and matcher as libraries |
| [Extending Antares](docs/src/extending.md) | features, the driver registry, hooks, adding a backend |
| [Decisions](docs/adr/README.md) | the architecture decision records |

## Design targets

| Dimension | Target |
|---|---|
| Entities | 100,000,000 current-state, one PostgreSQL cluster |
| Tenants | 10,000, one shared schema, `tenant_id` + Row-Level Security |
| Subscriptions | 100,000 per broker, HTTP + MQTT |
| CSource registrations | 100,000+ per broker, index-shaped matching, bounded fan-out |
| HA | stateless broker pods, NATS JetStream, Postgres primary/replica |

The weekly scale run measures a fixed fraction of these rows on rented
hardware and publishes the broker's and Postgres's memory and CPU per
phase; the [Performance](docs/src/performance.md) chapter says how each
number is produced.

## The NGSI-LD crates

`ngsild-model`, `ngsild-jsonld`, `ngsild-ql`, `ngsild-matcher` and `ngsild-residual` (folders
`crates/antares-*`, `antares_*` inside this workspace through `package =`, T-3962) hold the
NGSI-LD data model, JSON-LD expansion, the query languages, subscription matching and the
residual evaluator with no server, store or network in their default features (role `ngsild`,
ADR-0027). Each has a README with a runnable example and the CIM 009 clauses it implements, and
builds from its packaged files alone (`cargo publish --dry-run` in `workspace.yml`).

They are released through their own repository,
[marek-mraz-jc/ngsild](https://github.com/marek-mraz-jc/ngsild), and taken from there as a git
dependency at a `v*` tag (no crates.io for now, owner decision 2026-10-18, T-3913). They are
still changed here only: `python3 dev/ngsild-export.py <dir>` writes that repository's tree
from this checkout (the five crates, a root manifest of the workspace entries they inherit, the
pruned lockfile), and `dev/ngsild-export.test.sh` proves the tree builds and passes its tests on
its own. To release: replace the clone's content with the export, commit, tag
`v<workspace version>`, push; the tag's workflow there tests the workspace and publishes a GitHub
release whose source archive carries SLSA build provenance.

A bad release: never move or delete a pushed tag. Mark its GitHub release "do not use" with the
defect, open an issue naming the version, and release the fix as the next patch version from a
new tag.

## Repository layout

```
crates/antares-model      NGSI-LD types
crates/antares-ql         q= / scopeQ / geoQ parsers and evaluator
crates/antares-jsonld     @context cache and core-context fast path
crates/antares-sql        storage drivers, SQL compiler, migrations
crates/antares-bus        change-event bus: in-process or NATS JetStream
crates/antares-api        HTTP binding (axum) and the operations
crates/antares-broker     composition root, the `antares` binary
crates/antares-wasm       the browser build
docs/                     conformance ledger, ADRs, user book source
dev/                      run and test scripts
compose-files/            local stacks
```

## Related brokers

Antares is a compliant peer of Orion-LD, Scorpio and Stellio, not a fork of
any of them: it shares no code with them and federates with them over the
standard distributed-operations API. Where it fits and how it compares:
[Ecosystem](docs/src/ecosystem.md).

## Contributing

Changes are accepted into `dev` only. Fork, branch from `dev`, open the
pull request into `dev`; one aimed at `main` fails its first check and
gets retargeted. `main` holds released code and moves only when the
maintainers promote `dev` into it, and releases are the `v*` tags cut
there. Commits carry a DCO sign-off (`git commit -s`), one change each.

The whole process, from the first commit to a release, is in
[CONTRIBUTING.md](CONTRIBUTING.md). Report a vulnerability the way
[SECURITY.md](SECURITY.md) describes, not in a public issue.

## License

[EUPL-1.2](LICENSE), the European Union Public Licence. Commercial
licensing: contact@marek-mraz.com.
