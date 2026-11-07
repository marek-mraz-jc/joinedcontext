---
title: Conformance suites per surface, run against the vanilla and the full broker binary
task: T-3449
---

# Conformance suites per surface

Every surface the broker serves gets at least one external suite that judges the running binary,
beside its unit tests (`tasks/GOAL-antares-surfaces.md`, point 7). This page maps what
`joinedcontext-conformance` already has, decides how each suite targets the broker instead of the
platform gateway, pins every external tool, and names the CI job per suite and binary. The test
plan (`surfaces-test-plan.md`, T-3444) includes this page as its conformance section.

Sources: Antares `agent/worker-2/T-3436` at `ac4c042`; joinedcontext-conformance `main` at
`f00d3f2`; `ngsi-ld-test-suite/` as vendored in Antares; npm registry and GitHub read 2026-10-10.
Unknown means unknown; nothing below was run for this page.

## 1. The two binaries and the views

| name | build | what it proves |
|---|---|---|
| `vanilla` | `cargo build --locked -p antares-broker --release` (default features) | today's NGSI-LD broker; the reference result |
| `full` | `cargo build --locked -p antares-broker --release --features full` | every surface; must equal `vanilla` where it narrows nothing |

Views used by the suites (ADR-0023), loaded through `/q` before a run:

| view | selector | attributes | operations | role |
|---|---|---|---|---|
| `all` | every type | every attribute | every operation-matrix row | "narrows nothing": results must equal `vanilla` |
| `etsi-restricted` | `Building`, `City` (expanded with the suite's `@context`) | all but `airQualityLevel` | the read rows of `operation-matrix.yaml` (10.4.x, 10.6.x, 11.3.x, 10.5.2 to 10.5.5) | the subset that applies through a restricted view, with listed refusals |
| `parity-restricted` | the types of `tests/fixtures/` in joinedcontext-conformance | a fixed allowlist per type | read rows | the MCP and OData parity cases under a view that sees less |

Decided (worker-2, owner may override): `Building` and `City` for the restricted ETSI view, because
they are the two most used entity types of the suite's data (`grep -rhoE '"type" *: *"[A-Za-z]+"'
ngsi-ld-test-suite/data/entities`: Building 152, City 47, Vehicle 44) and `airQualityLevel` is an
attribute of the Building fixtures; options: the most used types (chosen), every type with a hidden
attribute, a synthetic fixture set.

## 2. NGSI-LD: the ETSI `ngsi-ld-test-suite`

### 2.1 What exists

- Antares vendors the suite (`ngsi-ld-test-suite/`, last synced by `65a37be`; its upstream commit is
  not recorded in the tree, **unknown**) and runs it through `dev/etsi-pipeline.sh` in the
  `etsi-matrix.yml` cells (`memory`, `file`, `postgres`, `timescale`, `postgres-nats`,
  `timescale-nats`, `wasm-file`), gated per cell on `gate-status.txt = PASS`. The serial suites are
  the nine directories of `dev/etsi-suites.sh` `SERIAL_ALL`, plus `IOP_TP`; a directory the runner
  does not know fails the run (`check_suites_complete`).
- Test purposes per directory (`find ngsi-ld-test-suite/TP/NGSI-LD/<dir> -name '*.robot' | wc -l`):
  CommonBehaviours 18, ContextInformation/Consumption 158, ContextInformation/EntityMap 7,
  ContextInformation/Provision 169, ContextInformation/Snapshot 1, ContextInformation/Subscription
  99, ContextSource 83, jsonldContext 31, DistributedOperations 105; `IOP_TP` 57. Total 671 + 57.
- joinedcontext-conformance `tests/etsi-ttf/run.sh` fetches the mirror
  `https://github.com/marek-mraz/ngsi-ld-test-suite.git` at `TTF_REF=5da9091e6b8e63690c6b09e0245b56ca4c4b9152`
  and runs it against the platform gateway; `tests/etsi-ttf/expected_failures.json` is `[]`, and
  `tests/etsi/expected_failures_gateway.json` is `[]` (both read with `json.load`).

Decided (worker-2, owner may override): one pin for both repositories, the mirror at `TTF_REF`
(branch `develop`), because two copies of the same suite at unknown distance cannot prove "full
equals vanilla" against the platform's own numbers. The Antares job checks out the mirror at the
pin into `ngsi-ld-test-suite/` before `dev/etsi-pipeline.sh`; a diff of the vendored tree against
the pin is the first step of the implementation task, and the vendored copy goes once the diff is
empty or explained. Options: the mirror pin (chosen), keep the vendored copy as the reference,
track `develop` head.

### 2.2 Which test purposes still test GS CIM 009 V1.9.1 behaviour

The suite is written against GS CIM 009 (its `@context` URLs name `ngsi-ld-core-context-v1.6.jsonld`
three times and `v1.8.jsonld` once: `grep -rhoE 'ngsi-ld-core-context[-a-z0-9.]*jsonld' resources TP`).
TS 104 175 / 104 176 V1.1.1 changed the rules listed in `ts104175-delta.md` section 3. The TPs
that touch each changed rule, found by grepping the suite for the members and statuses involved:

| changed rule (delta section, gap task) | TPs that touch it | what they assert under CIM 009 | under TS 104 175 / 104 176 |
|---|---|---|---|
| temporal pagination (3.2, T-3456): 206 and `Content-Range` are gone; `firstN`, `offsetN`, `intervalafter`/`intervalbefore` Links are new | 020_13, 021_15, 021_16, 6311_01 (`grep -rlE 'Content-Range|\b206\b' TP`) | 206 with `Content-Range` on a truncated temporal answer | no 206 in either document; Links instead. **These four test the old behaviour.** No TP covers `firstN`, `offsetN` or the interval Links (`grep -rlE 'firstN|offsetN|intervalafter' TP`: none) |
| core `@context` (3.4, T-3458): 149 top-level terms, 23 scoped contexts | 557_01, 451_01 (delta 3.4) and every TP comparing a body with `ignore_core_context_version` (`resources/AssertionUtils.resource:165-195`) | the v1.6/v1.8 documents the suite names | the same URL, different content; a TP that compares `@context` members can change verdict when T-3458 lands |
| loop detection (3.5, T-3459): 9.7 adds an error for a loop on any registration | D018_01, D018_02, D018_03, 436_04 | 508 only for one exclusive/redirect source (D018_01 asserts 508 on `mode=inclusive`, a suite defect raised in `docs/upstream/etsi-raises.md` issue 1) | 9.7 "A suitable error shall be returned"; D018_01's expectation becomes arguable |
| query equality on arrays of objects (3.1, T-3455) | 019_28_*, 019_29_* | conditions 1 to 7 and 10 | conditions 8 and 9 are new; no TP covers them |
| `RegistrationInfo.attributeNames` (3.6, T-3451) | none: the member appears only in `data/entities/entity-type-023-01-0{1,2}.json`, which are entity-type details, not registrations | `propertyNames`/`relationshipNames` only | `attributeNames` preferred, the old two deprecated; no TP covers it |
| CSR subscription member applicability (3.7, T-3452) | 047_01 to 047_16 | Table 5.2.12-1 | eight members "not applicable and shall be ignored"; no TP sends them |
| HEAD 204 vs 200 (3.8, T-3460) | none (`grep -rli 'HEAD request\|method=HEAD' TP`: none) | | clause 5 says 204, the 7.3.3.1 table says 200 |
| Set Attribute Value `PUT .../attrs/{attrId}/value` (3.10, T-3450) | none of the seven TPs naming `/value` (033_12, 521_01, 586_01, 028_10, 4517_01, 454_01, 5744_07) sends a PUT to it (they read it) | route absent | new operation 10.2.6 |
| MQTT `metadata` (3.8, T-3453) | 058_* | CIM 009 7.2 | word for word the same in TS 104 243 5.2 |

Every other TP tests a rule whose normative text is unchanged in substance or not yet re-audited
(delta section 4, T-3461: 266 changed clauses). Until T-3461 closes, a TP is "CIM 009 and TS 104 175
agree" only for the clauses section 3 of the delta audited.

### 2.3 Runs and expected results

| run | binary | base URL | expected | quarantine |
|---|---|---|---|---|
| E1 vanilla | `vanilla` | `/ngsi-ld/v1` | today's per-TP verdicts of `etsi-matrix.yml` | empty |
| E2 full, view `all` | `full` | `/x/views/all/ngsi-ld/v1` | per-TP verdicts equal to E1, TP by TP (`dev/etsi-matrix-summary.py` diff of the two `failures.csv`) | empty |
| E3 full, direct | `full` | `/ngsi-ld/v1` | equal to E1 (features compiled in change nothing on the vanilla routes) | empty |
| E4 full, view `etsi-restricted` | `full` | `/x/views/etsi-restricted/ngsi-ld/v1` | the refusals below; everything else equal to E1 | `tests/etsi/expected_view_refusals.json` (new), each entry with the ADR-0023 rule and task id |

Expected refusals in E4 (ADR-0023 rules 3 and 4; generated, not hand-written: a script reads
`operation-matrix.yaml` and the view definition and writes the list, and the job fails when the list
and the run disagree in either direction):

| suite directory | operation | E4 answer |
|---|---|---|
| ContextInformation/Provision (169), ContextInformation/Provision/TemporalEntity\* | create, update, append, merge, replace, delete, purge, batch | 405 with `Allow` (method not offered by the view) or 422 `OperationNotSupported` where the method stays (rule 4) |
| ContextInformation/Consumption on a type other than Building/City | retrieve / query | 404 for a retrieve (rule 3); an empty list for a query |
| any TP reading `airQualityLevel` | retrieve / query with `attrs` or `q` on it | the attribute absent; `q` on it selects nothing (rule 2) |
| ContextSource, DistributedOperations, jsonldContext writes | registration and context writes | 405 (not in the view's operations) |
| CommonBehaviours | as the operation it uses | as above |

The setup step of most Consumption TPs creates its fixture with a POST, which E4 refuses. E4
therefore seeds the fixtures through `/ngsi-ld/v1` (outside the view) and runs only the TP bodies
through the view: a Robot variable `SETUP_URL` beside `NGSILD_URL`, the change named in section 8.

Decided (worker-2, owner may override): an empty quarantine for E1 to E3, because GOAL point 1
requires the vanilla ETSI result unchanged and the matrix already gates 100 % per cell; options:
empty (chosen), carry known failures with task ids.

### 2.4 TS 104 188 interoperability tests

`IOP_TP` (57 robot files) already runs in the pipeline against all five brokers of
`dev/run-five.sh`. It runs for `vanilla` and for `full` (E1/E3 shape); its tests address brokers by
URL, so it is not run through a view.

## 3. MCP

### 3.1 What exists and how it is retargeted

joinedcontext-conformance `tests/mcp/` holds three suites aimed at the platform's per-space MCP
surface (`MCP_URL=https://host/cs/{space}/mcp`, `tests/mcp/README.md`):

| suite | today | against the broker |
|---|---|---|
| `test_mcp_protocol.py` | Streamable HTTP framing, JSON-RPC 2.0, `initialize` negotiation, tool listing, annotations, sessions, RFC 9728 metadata | `MCP_URL=<broker>/x/views/{view}/mcp`. The cases that assume a session (`Mcp-Session-Id`) and RFC 9728 metadata are platform behaviour: the broker serves the 2026-07-28 stateless protocol and legacy `initialize` without sessions (`mcp-surface.md` D-8), and RFC 9728 is the gateway's (the broker parses no token, ADR-0020). Those cases get a marker `platform_only` and are deselected against the broker |
| `test_mcp_isolation.py` | space isolation, byte-identical cross-space errors | one view per space: `MCP_URL` = view A, `OTHER_SPACE_MCP_URL` = view B on the same broker; the configuration-plane cases (`CONFIG_MCP_URL`) are platform-only |
| `test_mcp_read_parity.py` | the same read over NGSI-LD and over the tool; `test_the_matrix_is_covered` reads `tools/list` | the REST door becomes the view's NGSI-LD root `/x/views/{view}/ngsi-ld/v1` (derived from the MCP URL by replacing the trailing `mcp`); run under view `all` and under `parity-restricted` (`MCP_NARROWED_TOKEN` is replaced by the second view, since the broker narrows by view, not by token) |

The parity table must have one case per entry of the broker's `tools/list` (32 tools,
`mcp-surface.md`); `test_the_matrix_is_covered` already fails on a missing tool or argument.

### 3.2 Official MCP conformance tooling

`@modelcontextprotocol/conformance` (repository `github.com/modelcontextprotocol/conformance`), npm
version 0.1.16 (registry `time.modified` 2026-10-01), binary `conformance`. Headless server mode,
quoted from the repository README (read 2026-10-10):

```
npx @modelcontextprotocol/conformance server --url http://localhost:3000/mcp --requirements 2026-07-28
```

`--requirements <revision>` runs exactly what that revision requires; `--expected-failures <path>`
takes a YAML baseline; results go to `results/server-<scenario>-<timestamp>/checks.json`. With a
baseline, exit 1 means an unexpected failure or a stale baseline entry. Without a baseline the
README gives no exit code for a failing run, **unknown**: the job passes an empty baseline file so
the exit code is defined.

Runs: `--requirements 2026-07-28` (the revision `mcp-surface.md` D-8 pins) and
`--requirements 2025-11-25` (the legacy era the surface accepts), each against
`/x/views/all/mcp` of `full`. Pinned as `npx --yes @modelcontextprotocol/conformance@0.1.16`.

As run (T-3516, 2026-10-11): the 0.1.16 binary has no `--requirements`; its flag is
`--spec-version`, which knows `2025-03-26`, `2025-06-18`, `2025-11-25`, `draft` and `extension`
(`draft` holds no server scenario). The job runs `--spec-version 2025-11-25` and `2025-06-18`;
the 2026-07-28 era is covered by the crate's own tests until a release carries it. With
`--expected-failures`, exit 1 is defined as the README says. Most server scenarios call the
fixture tools, prompts and resources of the SDKs' conformance test server (`test_simple_text`,
`test://static-text`), which a product server does not have: `ci/mcp-baseline.yaml` lists each
with its reason and upstream source, so it is not empty. The scenarios that test the surface
itself (`server-initialize`, `ping`, `tools-list`, `resources-list`, `prompts-list`,
`completion-complete`, `dns-rebinding-protection`) pass.

## 4. OData (new suite `tests/odata/`)

Pins from `odata-library.md` section 4 (T-3448), fetched at the SHA with a checksum, never
vendored (OASIS IPR licence):

| source | pin |
|---|---|
| `oasis-tcs/odata-abnf` | `a31c8db229a0d8e957bee86512b01efe7ce74173`: `abnf/odata-abnf-testcases.yaml` (840 cases, 79 with `FailAt`) |
| `oasis-tcs/odata-csdl-schemas` | `e6b66a96ec5f74edb0c0e192978a39f146f70d76`: `schemas/edmx.xsd`, `schemas/edm.xsd`, `schemas/csdl.schema.json`, `catalog.xml` |
| `oasis-tcs/odata-vocabularies` | `3e36ad2a4930a47feaf7a7cd0b78ed4f2bafcfb2`: Core, Capabilities, Measures, Validation |

Suite `tests/odata/` (pytest, in the conformance runner image), every case against
`<broker>/x/views/{view}/odata/v4` of `full`:

| case | what it checks | tool |
|---|---|---|
| `test_abnf_cases_live` | every ABNF test case whose rule is a URL entry point the service implements: a valid case answers non-400, a `FailAt` case answers 400 with the OData error object; a case for a rule outside the implemented subset answers 501 `NotImplemented`, never 200 | pytest + `httpx`, cases read from the pinned YAML |
| `test_metadata_xml_is_valid_csdl` | `$metadata` (XML) validates against `edmx.xsd` with `catalog.xml` | `xmllint --noout --catalogs --schema edmx.xsd` (libxml2 from the image's Debian base) |
| `test_metadata_json_is_valid_csdl` | `$metadata?$format=json` validates against `csdl.schema.json` (draft-07, `\p{L}` patterns) | `ajv-cli` with `ajv-formats`, npm, versions pinned in the image (the repository's own validator choice, `odata-library.md` 4 item 3) |
| `test_dotnet_client_reads_and_queries` | Microsoft.OData.Client 9.0.0 parses `$metadata` with no error and reads one entity with `$top=1` | job image `mcr.microsoft.com/dotnet/sdk:10.0` |
| `test_sap_cloud_sdk_reads_and_queries` | `@sap-cloud-sdk/generator@4.9.1` generates from `$metadata`, `@sap-cloud-sdk/odata-v4@4.9.1` reads one entity | `node:22` |
| `test_python_odata_reads` (non-blocking) | `python-odata==0.8.1` reflects the model and reads one entity | conformance image; non-blocking until its source repository is confirmed (`odata-library.md` section 5) |
| `test_filter_parity_with_q` | the same selection through `$filter` and through NGSI-LD `q` returns the same entity ids, for every row of the `$filter` mapping table of `odata-linkml-mapping.md` | pytest, both doors of one view |
| `test_capabilities_annotations_are_obeyed` | every request a `Capabilities` annotation forbids (e.g. `InsertRestrictions/Insertable=false`) is refused with the OData error object; a write without the NGSI-LD write row in the view is refused | pytest |
| `test_expand_skips_out_of_view_targets` | `$expand` of a relationship whose target is out of view returns no target (ADR-0023 rule 6) | pytest, view `parity-restricted` |
| `test_deep_insert_and_ref_check_the_linked_entity` | deep insert and `$ref` to an out-of-view or read-only target are refused before anything is written; the entity count before and after is equal | pytest |

## 5. LinkML

Suite `tests/linkml/` (pytest), against `<broker>/x/views/{view}/linkml` of `full` with `linkml`,
as built (T-3512):

| case | check | tool and pin |
|---|---|---|
| `test_served_schema_loads` | views `all` and `parity-restricted`: the served `schema/v{major}/model.linkml.yaml` loads with `SchemaView`, holds the classes the catalogue names (at least one), and `linkml-lint` reports no error | `linkml==1.12.0` (latest on PyPI 2026-10-10), added to `requirements.in` |
| `test_reference_generators_accept_the_schema` | both views: `gen-json-schema` and `gen-shacl` run on the served schema and describe every class | `linkml==1.12.0` |
| `test_served_json_schema_validates_the_view` | view `all`: every Entity of every visible type, in `keyValues` from the view's NGSI-LD root, validates against the served `model.schema.json` | `jsonschema==4.26.0` (already in `requirements.txt`) |
| `test_served_shacl_validates_the_view` | view `all`: the same Entities, read as JSON-LD through the core `id`/`type` terms and the served `context.jsonld`, conform to the served `model.shacl.ttl`, every target class has a focus node, and the shapes pass SHACL-SHACL | `pyshacl==0.40.1` (already in `requirements.txt`) |
| `test_hidden_attributes_are_absent_from_the_schema` | view `parity-restricted`: an attribute outside the view's allowlist (`NO2`, which view `all` shows) is in no class, slot, property, term or shape of the served documents, and the catalogue says `redacted` | pytest |

Decided (worker-1, owner may override): the data checks judge the served JSON Schema and SHACL,
and the generators only have to accept the schema, because LinkML inlines a class range without
an identifier (an NGSI-LD Relationship holds the target's id) and gives a GeoProperty its default
range `string` (only the broker's `ngsi_ld_kind` annotation says it is a geometry); options: the
generated documents as judges, which needs an identifier slot and a geometry type the subset does
not have. The SHACL check runs on view `all`, whose Entities include every Relationship target,
because `sh:class` cannot hold for a target the view's scope hides. A view that shows no class
fails every case instead of passing it vacuously. The first run against the served documents
found three defects in `linkml-model`'s writers, fixed by T-3780: the YAML did not declare
`imports: [linkml:types]` and `default_range: string` (two `no_undeclared_ranges` errors), the
`@context` did not coerce a Relationship to `@id` nor a typed value to its datatype, and a
GeoProperty was a string in L3 and L5.

## 6. WebSocket and internal context sources

- **WebSocket** (ADR-0026): no ETSI binding exists, so no ETSI TP judges the channel itself. The
  ETSI Subscription TPs that only read notifications through the suite's notification server stay
  on HTTP. The channel gets its own suite `tests/ws/` (pytest + `websockets`): the AsyncAPI document
  (`docs/api/asyncapi-ws.yaml`) is validated with `@asyncapi/cli@2.16.0 validate`, and every message
  the broker sends in a scripted session (subscribe, two notifications, resume after a forced close,
  overflow) is validated against the document's message schemas; the notification `body` is compared
  with the body the same Subscription delivers over HTTP to the suite's notification server.
- **Internal context sources** (ADR-0025): equivalence, not a suite of its own. The ETSI
  DistributedOperations directory runs against `full` with `ANTARES_SELF_URLS` set (E3 shape: the
  suite's Context Source mock is not a self URL, so its 105 TPs must keep their vanilla verdicts),
  and the 105 TPs rebuilt as tenant-to-tenant cases run in `crates/antares-icsr/tests/icsr_equivalence.rs`
  over HTTP and in process (ADR-0025, equivalence test plan).

## 7. CI jobs

All in the Antares repository (public; its Actions minutes are not the metered private ones of
CLAUDE.md protocol rule 2), new workflow `surfaces-conformance.yml`, one job per suite per binary,
artefacts kept 14 days (Robot `output.xml`, `log.html`, junit `xunit.xml`, `checks.json`,
pytest junit). Required checks are the merge-request subset; the rest runs nightly and on `v*` tags.

| job | binary | view | suite and command | merge request | nightly |
|---|---|---|---|---|---|
| `etsi-vanilla` | vanilla | none | `dev/etsi-pipeline.sh` (E1), cell `file` | yes (existing quick preset) | all 7 cells (existing `full.yml`) |
| `etsi-full-all` | full | `all` | `dev/etsi-pipeline.sh` with `NGSILD_BASE=/x/views/all` (E2) + diff with `etsi-vanilla` | `ContextInformation/Consumption`, `ContextInformation/Provision` subset | all suites, cells `file` and `postgres` |
| `etsi-full-direct` | full | none | E3 | no | `file` cell |
| `etsi-full-restricted` | full | `etsi-restricted` | E4 with `expected_view_refusals.json` | no | `file` cell |
| `etsi-iop` | vanilla, full | none | `IOP_TP` through `dev/run-five.sh` | no | yes |
| `etsi-icsr` | vanilla, full with `internal-csr` | none | `DistributedOperations` on broker1 through `dev/etsi-pipeline.sh`, `ANTARES_SELF_URLS` its own URL on full, diff with `dev/etsi-baseline.py compare` (ADR-0025 layer 3) | no (layers 1 and 2 are crate tests in every run) | yes |
| `mcp-conformance` | `mcp` | `all` | `npx --yes @modelcontextprotocol/conformance@0.1.16 server --url <root>/x/views/all/mcp --spec-version 2025-11-25 --expected-failures ci/mcp-baseline.yaml`, then `--spec-version 2025-06-18` (section 3.2) | yes | yes |
| `mcp-suites` | `mcp` | `all`, `parity-restricted`, `other` | the three suites of `tests/mcp` with `MCP_SURFACE=broker`, `-m "not platform_only"` | `test_mcp_read_parity.py` with `MCP_PARITY_CASES=5` | all three |
| `odata-conformance` | full | `all`, `parity-restricted` | `jc-conformance odata` | `test_abnf_cases_live`, both `$metadata` validations | all, both client jobs |
| `odata-clients-dotnet`, `odata-clients-sap` | full | `all` | section 4 rows | no | yes |
| `linkml-conformance` | full with `linkml` | `all`, `parity-restricted` | `jc-conformance linkml` (`tests/linkml/run.sh`) | yes | yes |
| `ws-conformance` | full | `all` | `jc-conformance ws` + AsyncAPI validation | yes | yes |
| `vanilla-has-no-surfaces` | vanilla | none | the symbol gate of the threat model section 9 (`dev/check-vanilla-symbols.sh`) | yes | yes |

A red required job blocks the merge to `dev`. The nightly jobs file an issue through the existing
`.github/actions/failure-issue` action, as `docs.yml` does.

Every job takes the conformance runner image by digest (`ghcr.io/marek-mraz/joinedcontext-conformance@sha256:…`,
the digest `image.yml` of joinedcontext-conformance publishes), so the Python, Robot and pyshacl
versions are the ones `requirements.txt` pins.

## 8. Changes these jobs need

| repository | change |
|---|---|
| joinedcontext-conformance | `requirements.in`: `linkml==1.12.0`, `websockets` (version pinned at implementation); `Dockerfile`: `libxml2-utils` (xmllint), npm `ajv-cli`, `ajv-formats`, `@asyncapi/cli@2.16.0`, `@modelcontextprotocol/conformance@0.1.16` installed in the node stage; new `tests/odata/`, `tests/linkml/`, `tests/ws/` and their `jc-conformance` subcommands; marker `platform_only` in `tests/mcp/`; `tests/etsi/expected_view_refusals.json` |
| Antares | `dev/etsi-pipeline.sh`: `NGSILD_BASE` (API root prefix) and `SETUP_URL` (fixture seeding outside a view) variables; checkout of the mirror at `TTF_REF`; `surfaces-conformance.yml`; view fixtures `ci/views/{all,etsi-restricted,parity-restricted}.json`; `ci/mcp-baseline.yaml` (empty) |
| Antares, as built (T-3516) | the MCP jobs build `--features mcp` (no `full` feature yet); `ci/views/{all,parity-restricted,other}.json` with their seed `ci/views/seed/<tenant>.json`, loaded by `dev/views-rig.sh` (`other` is a second Tenant for the isolation suite); the suites come from a checkout of the public joinedcontext-conformance at a pinned commit, because the runner image is not pullable without a credential; `ci/mcp-baseline.yaml` lists the fixture-server scenarios (section 3.2) |
| Antares, as built (T-3510) | the build job builds `--features mcp,odata` (artifact `broker-surfaces`); `ci/views/schemas/<tenant>.json` holds each Tenant's LinkML schema, which `dev/views-rig.sh` loads through `PUT /q/schemas/<tenant>` (every job starts the broker with `ANTARES_SCHEMA_MAX_BYTES`); the seed gives `bb-1` a Relationship `nearest` to `ba-1`, which `parity-restricted` hides by scope (the `$expand` case); job `odata-conformance` runs `tests/odata/run.sh`, `-k "abnf or metadata"` on pull requests. Decided (worker-3, owner may override): the ABNF cases run are those of `odataRelativeUri` and `resourcePath` (195), and a `FailAt` case may answer 404, because the cases name their own model (`Categories`), which the service resolves before the syntax; the exact failure offset is `odata_url_abnf_testcases`. The JSON `$metadata` validates with `jsonschema-rs` (pinned in `requirements.txt`) rather than `ajv-cli`, because it reads the `\p{L}` patterns and adds no npm install; options: ajv-cli and ajv-formats from npm. The nightly matrix `odata-clients` runs `tests/odata/clients/{dotnet,sap}/run.sh` (jobs `odata-clients-dotnet`, `odata-clients-sap`): Microsoft.OData.Client 9.0.0 parses and validates `$metadata` and materializes one page; the SAP Cloud SDK generator 4.9.1 builds a client from `$metadata` and its `odata-v4` 4.9.1 runtime reads one page; both then fail when the `parity-restricted` model holds `NO2`. Microsoft.OData.Core refuses a path-only context URL, so the service root is absolute (T-3688). python-odata 0.8.1 (non-blocking in ADR-0024) is not run |
| Antares, as built (T-3518) | the build job builds `--features full` (views, mcp, odata, ws). Job `ws-conformance` (merge requests and nightly) validates `docs/api/asyncapi-ws.yaml` with `npx @asyncapi/cli@2.16.0 validate` on the runner, then runs `tests/ws/run.sh` of joinedcontext-conformance (`c897b11`, websockets 17.1) against `/x/views/all/ws`, the broker started with `ANTARES_WS_QUEUE=4`, `ANTARES_WS_MAX_LIFETIME_SECS=45` and `ANTARES_EGRESS_ALLOW_PRIVATE=true`; every message the broker sends is validated against the document's message schemas, and the crate's own `tests/ws` read the same file (`common/asyncapi.rs`, `ws_contract.rs`), so a drift fails both. Decided (worker-3, owner may override): the document is validated by the job, not inside the runner image, because the image removes npm after `npm ci` and `@asyncapi/cli` would add its dependency tree for one check; options: `@asyncapi/cli` in the image's `package.json`. The closing reasons `overflow` and `idle` stay with the crate tests (a slow socket, a minute of silence); the suite closes by `view-removed` and `lifetime` |
| ngsi-ld-test-suite mirror | `SETUP_URL` support in the fixture keywords of `resources/ApiUtils/*.resource` (a branch of the mirror, offered upstream) |
| Antares, as built (T-3512) | the build job builds `--features full,linkml` (`full` has no `linkml` yet). Job `linkml-conformance` (merge requests and nightly) starts the broker with `ANTARES_API_SURFACES=admin,views,linkml` and runs `tests/linkml/run.sh` of joinedcontext-conformance (`5e3722f`: `linkml==1.12.0` in `requirements.in`, the lock recompiled on Python 3.12 with every existing pin kept; `jc-conformance --selftest` reports `linkml-lint`) with `LINKML_URL` and `LINKML_RESTRICTED_URL` the surfaces of views `all` and `parity-restricted` |
| Antares, as built (T-3501) | `dev/etsi-pipeline.sh` takes `VIEWS` (a directory of view documents it PUTs into broker1), `NGSILD_BASE` (the root the serial TPs' bodies reach) and `SETUP_URL` (default: the plain root when `NGSILD_BASE` is another); `dev/etsi-run.sh` hands `SETUP_URL` to the Robot listener `dev/etsi_setup_url.py`, which points `url` and `temporal_api_url` at it for every suite and test setup and teardown and back for the body, and resets the broker through it; an Entity a TP body created through the view (a local write, ADR-0023 rule 7) it deletes at that root with `local=true` after the TP's teardown, so the next TP's setup does not meet it (T-3954). E2's view is `ci/views/etsi/all.json` (Tenant `default`, `*`, every operation a view offers), apart from `ci/views/`, which the surface jobs load. Measured on `ContextInformation/Consumption/Entity/RetrieveEntity` (93 TPs, local debug broker): E1 0 failures; E2 through `all` 4 (454_01: 403 on the body's create; 5714_01_01 to 03: 400, EntityMaps); E2 without the listener 24. E3 is `image-jc.yml` job `etsi` (the `full,jc-policy` image's plain root, every suite, `dev/etsi-baseline.py check` TP by TP). The listener also writes every body request to `requests.jsonl`; `dev/etsi-view-diff.py` holds E2 against E1 of the same image TP by TP and generates the expected refusals from them: a TP may differ only when its body sent a request whose operation (the `ngsi_ld_http` binding of `operation-matrix.yaml`) the view does not offer, that no operation binds, that names an NGSILD-Tenant (rule 10) or an entity map (threat V22); it writes them to `owed-refusals.json`. Any other difference is red unless `dev/etsi-view-known.json` names the TP with the task fixing it, and a named TP that answers as E1 again is red too. Decided (worker-1, owner may override): job `etsi-full-all` in `surfaces-conformance.yml` runs E1 and E2 on the same `mqtt,views,mcp,odata,ws` image in cell `file` only (pull requests: Consumption and Provision; nightly: every serial suite), since a second store tests the store, not the view path; the diff is against E1 of the same image, not the committed vanilla baseline, which E3 already holds. Measured on RetrieveEntity: 3 owed (5714_01_01 to 03, entity maps), 1 known (454_01, T-3786: a Relationship to an absent Entity is refused through a view that shows every Entity), 0 red. E4 (`etsi-restricted`) is T-3787 |

## Related

- [operation-matrix.md](operation-matrix.md), [operation-matrix.yaml](operation-matrix.yaml) (T-3437)
- [ts104175-delta.md](ts104175-delta.md) (T-3435)
- [mcp-surface.md](mcp-surface.md) (T-3442), [odata-library.md](odata-library.md) (T-3448),
  [odata-linkml-mapping.md](odata-linkml-mapping.md) (T-3441)
- [surfaces-threat-model.md](surfaces-threat-model.md) (T-3447)
- ADR-0023 endpoint views, ADR-0025 internal context sources, ADR-0026 WebSocket notification channel
