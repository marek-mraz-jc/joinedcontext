# ADR-0024 — OData 4.01 for Rust: build our own standalone `odata-*` crates

Date: 2026-10-10. Status: accepted. Decided by worker-2 under the owner
order of 2026-10-10 ("a design decision is yours"); the owner may override
any point. Task T-3448. The survey, the API sketch and the pinned sources
are in `docs/analysis/odata-library.md`; this record keeps the decisions.

## Context

The owner wants an OData v4 surface on Antares (`tasks/GOAL-antares-surfaces.md`
points 2 and 7) and said: "if you don't have a proper OData library, create
one." The OData surface itself is mapped in `docs/analysis/odata-linkml-mapping.md`
(T-3441): every OData request becomes exactly one TS 104 175 operation of the
operation matrix, under the ADR-0020 seam and the ADR-0023 view.

What the surface needs from a library: the URL conventions (resource path and
system query options to an AST, with limits), the CSDL model with CSDL XML and
CSDL JSON writers, the JSON format writer (control information, ETags,
errors), and `$batch` (multipart and JSON). The survey (analysis section 1,
14 published crates and 11 unpublished projects read on 2026-10-10) found:

- no crate or project that covers CSDL JSON, the JSON format writer and
  `$batch` together, and none that covers `$batch` at all;
- query parsers that are `$filter`-centred or very young, the one with input
  limits (`cf-gears-toolkit-odata`) bound to its own framework crates;
- server projects on OData v3 shapes (`datafusion-odata`: EDMX 1.0, Atom);
- the broadest `forbid(unsafe_code)` parser under LGPL-3.0, which `deny.toml`
  does not allow;
- xtofs/odata-rs (MIT, unpublished, 0.0.1): the best crate split, CSDL XML and
  JSON, but a URL parser without depth, node or length limits.

## Decision

**1. Build.** Four library crates and one adapter, each usable without
Antares:

| crate | does | depends on |
|---|---|---|
| `odata-url` | resource path and system query options to a typed AST; optional feature `edm` resolves names and type-checks against a model | `percent-encoding`, `thiserror`; `odata-csdl` only under `edm` |
| `odata-csdl` | EDM model, CSDL XML and CSDL JSON writers (readers under feature `read`), model validation | `regex` (amended by T-3502: both writers are hand-written over `std::io::Write`, so `quick-xml`, `serde_json`, `indexmap` and `thiserror` are not needed; `regex` holds the exact `TSimpleIdentifier` classes of `edm.xsd`) |
| `odata-json` | JSON format: context URL, entity and collection writers, `@odata.bind` and deep-insert body reader, `If-Match`, `Prefer`, version negotiation, the error body | `serde_json`, `odata-csdl`, `odata-url` (amended by T-3504: plus `serde` for the reader that counts `BodyLimits` while it reads; `serde_json` with `float_roundtrip`, so an `Edm.Double` reads back to the same bits; no `indexmap`, the writers stream members in the caller's order; `If-Match` is not parsed, the surface answers 501 on its presence, W32) |
| `odata-batch` | multipart (Protocol 4.01 11.7.7) and JSON batch (JSON Format 4.01 section 19) parsers and writers, `dependsOn` ordering | `memchr`, `serde_json` (amended by T-3505: `serde_json` with `float_roundtrip`, so a number in a member body reaches the member reader with its bits; no `execution_order`: a `dependsOn` may name only an earlier member, so array order is the execution order and a cycle or forward reference is `UnknownDependency`, 400) |
| `odata-axum` | HTTP to library types and back, `$metadata` router; behind feature `axum` | `axum`, `http`, the four above |

The Antares OData surface (T-3441) is a separate, thin crate that maps
`odata_url::ResolvedUri` to operation-matrix rows and the policy seam; no
`odata-*` crate names an `antares-*` crate, and a CI job enforces that
(T-3447 `module-rules`).

**2. Where they live.** Decided: inside the Antares workspace as
`crates/odata-*` with `publish = false`, extracted later to
`marek-mraz-jc/odata-rs` with `git subtree split` once the public API is
stable. Reason: one CI, one `deny.toml`, one fuzz workspace (`fuzz/`,
excluded at `Cargo.toml:21`) and one supply-chain gate, while the dependency
rule keeps the extraction mechanical. Options: inside the workspace, its own
repository from the start.

**3. Parser.** Decided: a hand-written recursive-descent parser for
`odata-url`, one function per ABNF rule of `odata-abnf-construction-rules.txt`
at the pinned commit, no parser-combinator dependency. Reason: every
combinator library surveyed carries `unsafe` in its own source (nom 4 lines,
winnow 44, chumsky 128, logos 11; analysis section 6), a hand parser carries
the depth and node counters where the recursion is, and its byte offsets can
match the OASIS `FailAt` values exactly. Fallback: `nom` 8.0.0, already in
`Cargo.lock`. The multipart parser is our own over a bounded byte slice with
`memchr`; `multer` was rejected for pulling `tokio` and `futures-util` into a
pure parser.

**4. Limits are parameters.** Every parser takes a `Limits` value (analysis
3.1, 3.3, 3.4: path and query length, segments, options, expression depth and
nodes, `$expand` depth and items, `$select`/`$orderby` items, `in` list
length, literal length, body bytes and depth, batch requests, change sets,
part headers, `dependsOn` edges). There is no hidden default on a request
path; `Limits::strict()` exists for tests and documentation. Every counter
uses checked arithmetic and answers `LimitExceeded`, never wraps.

**5. Scope: exactly what T-3441 maps.** Each crate implements the subset in
the table below and answers everything else with
`UrlErrorKind::NotImplemented` / `BatchError::NotImplemented`, which the
surface renders as 501 with the OData error body (Protocol 4.01 section 21.1
shape; no `innererror`, which is where servers leak stack traces; no request
value is echoed in a message).

The subset is the "mapped" rows of `docs/analysis/odata-linkml-mapping.md`
(T-3441), by row id; every "rejected" row there is the 501 (or the status it
names) of this point:

| crate | implements (T-3441 row ids) | answers `NotImplemented` (501) |
|---|---|---|
| `odata-url` | `$filter` (Q1, the F-row subset: `eq`/`ne`/`gt`/`ge`/`lt`/`le` and their `null` forms F1 to F6, `and`/`or`/`not`/grouping F7 to F10, `in` F12, `contains`/`startswith`/`endswith` F15, complex and annotation paths F20/F21, `any` over a primitive collection F22, `id` conjuncts F28, `geo.intersects` F29, Language and Vocab properties F31/F32, the F26 literals; navigation paths F24 parsed, refused by the surface until T-3474), `$select` (Q2), `$expand`, `$levels`, nested `$select`/`$expand`, `/$ref` and `*` (Q3, Q4, Q6 to Q8; parsed, refused by the surface until T-3470), `$orderby`, `$top`, `$skip`, `$count`, `/$count` (Q9 to Q13), `$format` (Q15), `$schemaversion` for `*` and the current version (Q18), `$skiptoken` (Q20), `/$query` (Q22), `$entity?$id=` (Q23), literal parameter aliases (Q26), the resource paths Q29 to Q35 | `$search`, `$compute`, `$index`, `$apply`, `$deltatoken`, `$root`, `$it`/`$this`, aliases bound to expressions, `/$filter(...)` segments, `$crossjoin`, `$all`, type casts, ordinal members (Q14, Q16, Q17, Q19, Q21, Q24 to Q26, Q28, Q36, Q37, Q39), options inside `$expand(...)` other than `$select`/`$expand` (Q5); custom query options are 400 (Q27) |
| `odata-csdl` | EntityType with key `id`, the property, navigation and sub-attribute constructs of T-3441 section 3.2, the Core, Capabilities and Measures annotations of its section 4, CSDL XML and CSDL JSON writers; readers under `read` | derived types beyond the abstract base of multi-target Relationships, actions, functions, media and stream properties |
| `odata-json` | entity, collection, entity-reference and service-document writers at `metadata=minimal\|full\|none`, `@odata.count`, `@odata.nextLink`, the error body; body reader for create, update, replace and single-valued binds (W1, W2, W4, W6, W9 to W11, W13, W16, W17, W20); `Prefer: odata.maxpagesize` | deep insert and deep update (W3, W8), collection-valued binds and `$ref` add/remove (W5, W18, W19), upsert on a missing key (W7, answered 404 by the surface, not 501), delta payloads and `$each` (W21, W22), `If-Match`/`If-None-Match` (W32), `$value` writes (W12), collection appends (W14) |
| `odata-batch` | multipart and JSON batch (W24, W25), `Prefer: odata.continue-on-error` (W26) | Content-ID references (W26, 501); a batch inside a batch is 400 |
| `odata-axum` | `GET /`, `GET /$metadata` (XML; JSON on 4.01 when asked), the extractors and responders | none of its own |

Actions, functions, media entities, stream properties and writes on read-only
resources (W27 to W31, W33) are refused by the surface before any library is
asked.

**6. Conformance sources, pinned** (analysis section 4; fetched in CI at the
pinned commit with a checksum, not vendored, since their licence is the OASIS
IPR policy and not an SPDX licence on our list):

| source | commit | used for |
|---|---|---|
| `oasis-tcs/odata-abnf` | `a31c8db229a0d8e957bee86512b01efe7ce74173` | `abnf/odata-abnf-testcases.yaml` (840 cases) run as a data-driven test of `odata-url`: accept, or reject at `FailAt`; a case for a rule outside the subset must return `NotImplemented`, never accept |
| `oasis-tcs/odata-csdl-schemas` | `e6b66a96ec5f74edb0c0e192978a39f146f70d76` | every `$metadata` we emit validates: CSDL XML against `schemas/edmx.xsd` with `catalog.xml`, CSDL JSON against `schemas/csdl.schema.json` (with `ajv`, the validator that repository uses; a Rust validator for its `\p{L}` patterns is not known) |
| `oasis-tcs/odata-vocabularies` | `3e36ad2a4930a47feaf7a7cd0b78ed4f2bafcfb2` | Core, Capabilities, Measures and Validation (Measures includes it) vocabulary files: every term we annotate with is read from them, so a misspelled term fails a test |

**7. Interop.** Decided: two required CI jobs, each reading our `$metadata`
and running one query: Microsoft.OData.Client 9.0.0 (.NET, MIT) and the SAP
Cloud SDK for JavaScript generator and `odata-v4` 4.9.1 (Apache-2.0); a third,
non-blocking job with python-odata 0.8.1 (MIT) until its source repository is
confirmed. Apache Olingo is not used: its repository is archived. Reason: two
vendors and two languages that are maintained today.

**8. Quality gates per crate** (GOAL point 8): `#![forbid(unsafe_code)]`;
the T-3447 `[lints]` table (clippy pedantic, `unwrap_used`, `expect_used`,
`panic`, `indexing_slicing`, `arithmetic_side_effects` denied); line and branch
coverage target 100 % with every uncovered line named in the report; one fuzz
target per parser (`odata_url::parse`, `odata_batch::parse_multipart`,
`odata_batch::parse_json`, `odata_csdl::read_xml`/`read_json` under `read`);
proptest round trips (AST to URL to AST, model to CSDL XML to model, model to
CSDL JSON to model); criterion benches for `parse` and `$metadata` build;
Miri on the parser crates' unit tests; a compiling example on every public
item. Dependencies with `unsafe` in their own source that the crates keep:
`memchr` (SIMD), `serde_json` and `regex` (`odata-csdl`, T-3502), all already in `Cargo.lock`.
Amended (T-3502): criterion is not in `Cargo.lock`, so the `$metadata` bench is a `harness = false`
bench on `std::time::Instant`; criterion replaces it when `bench-surfaces` brings criterion in.

## Consequences

- Five new workspace crates, none in the default `antares-broker` build: the
  OData surface feature pulls them in, so the vanilla binary and its ETSI
  result do not change (GOAL point 1; `dev/check-no-addon.sh` style gate).
- T-3445 files one implementation batch per crate, in dependency order
  `odata-csdl`, `odata-url`, `odata-json`, `odata-batch`, `odata-axum`, then
  the Antares surface crate.
- The extraction to its own repository is a later, mechanical step and needs
  no API change.

## Confirmation

- `odata_url_abnf_testcases` (data-driven over the pinned YAML, case counts
  per rule in the report);
- `odata_csdl_metadata_validates_against_oasis_schemas` (XML and JSON);
- `odata_csdl_xml_and_json_agree` (our two writers against the OASIS
  `odata-csdl-xml2json` converter);
- `odata_interop_dotnet`, `odata_interop_sap_js` (required CI jobs),
  `odata_interop_python` (non-blocking);
- the `module-rules` job of T-3447 goes red if an `odata-*` crate depends on
  an `antares-*` crate;
- `cargo deny check` passes with no new licence exception.
