# Antares documentation index

| Document | What it holds |
|---|---|
| [../ARCHITECTURE.md](../ARCHITECTURE.md) | The map for maintainers: crates and their contracts, module map, request and change flow, invariants, structural debts, what to touch for a given change |
| [src/](src/) | The user book (mdBook, rendered to the root of the Pages site): getting started, configuration, deployment, federation, wasm, [operations runbook](src/operations.md) |
| [spec/](spec/) + [spec/README.md](spec/README.md) | The conformance ledger — ETSI CIM 009 V1.9.1 full text, ONE file per clause with `status`/`evidence`/`notes` frontmatter; tooling: `python3 dev/spec.py status\|gaps\|check` |
| [adr/](adr/) | Irreversible decisions, one file each (shared-schema tenancy, JetStream bus, store ladder, wasm build, …) |
| [upstream/etsi-raises.md](upstream/etsi-raises.md) | Ready-to-file upstream issues against the ETSI suite/spec (defects proven from clause text) |
| [openapi/](openapi/) | The ETSI NGSI-LD OpenAPI description served by the playground's API console, and the broker's own operational API (`/q/`, `/ex/v1/`) |

Live artifacts: [the ETSI conformance report page](https://antaresbroker.joinedcontext.com/reports/latest/)
· [the browser playground](https://antaresbroker.joinedcontext.com/demo/).
