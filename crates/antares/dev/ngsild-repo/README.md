# ngsild

The NGSI-LD crates of [Antares](https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker),
released on their own: no server, store or network in their default features.

| crate | what it holds |
|---|---|
| `ngsild-model` | NGSI-LD data model types (CIM 009 V1.9.1) |
| `ngsild-jsonld` | pinned core contexts, expansion, compaction and structural validation; the caching HTTP loader behind the `fetch` feature |
| `ngsild-ql` | `q=`, `scopeQ` and `geoQ` parsers to a typed AST |
| `ngsild-matcher` | subscription matching against an in-memory entity |
| `ngsild-residual` | the residual of a query over a view and its three-valued evaluator |

Each crate has its own README with an example and the clauses it implements.

## Use

Take them as a git dependency at a release tag, never a branch:

```toml
[dependencies]
ngsild-jsonld = { git = "https://github.com/marek-mraz-jc/ngsild", tag = "v0.3.0" }
# or keep the name Antares uses inside its own code
antares-jsonld = { package = "ngsild-jsonld", git = "https://github.com/marek-mraz-jc/ngsild", tag = "v0.3.0" }
```

Each release carries the source archive with SLSA build provenance:
`gh attestation verify ngsild-v0.3.0.tar.gz --repo marek-mraz-jc/ngsild`.

## Where changes go

This repository holds released copies only. `SOURCE` names the Antares commit a release came
from; the crates are changed, reviewed and tested there, and `dev/ngsild-export.py` in Antares
writes this tree for the next tag.

## A bad release

A pushed tag is never moved or deleted: a lockfile that pinned it would point at another commit.
Mark the GitHub release of the bad version as "do not use" with the defect in its notes, open an
issue naming the version and the defect, and release the fix as the next patch version from a new
tag. Consumers move by changing `tag =` and running `cargo update -p <crate>`.

## Licence

EUPL-1.2, see `LICENSE`.
