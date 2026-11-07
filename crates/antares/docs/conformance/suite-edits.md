---
title: Local edits of the vendored ETSI suite, and the forge ratchet
task: T-4855
---

# Local edits of the vendored ETSI suite

`ngsi-ld-test-suite/` is the ETSI NGSI-LD test suite (forge.etsi.org `cim/ngsi-ld-test-suite`, GS CIM 009 V1.9.1) as this repository carries it; `dev/etsi-baseline.json` names its 1833 TPs (+12 MQTT) and every per-commit ETSI cell must pass all of them. This page lists every commit that changed a file of the suite after it was vendored, why, under which clause, and where the change stands upstream. A suite edit that is not on this page is a defect of this page.

## 1. Data sources

- The commits: `git log -- ngsi-ld-test-suite` on dev `0109b05f` (14 commits: the import `7a656bda`, the @context move `65a37be1`, and the 12 below it).
- Which TP files forge does not carry: the `source` column of `tp-status.csv` in the artifacts `etsi-pinned-vanilla-file` and `etsi-latest-vanilla-file` of external-suites run 38860287033 (antares `74b72185`, forge `5c398518`). The pinned run has 726 TP files, forge 567; 193 files exist only here, 34 only on forge.
- The upstream reports: `docs/upstream/etsi-raises.md`, one section per raise. None is filed on forge yet: filing needs a forge account, which no agent holds (owner action). Until then each raise text is the justification, and the forge default branch still fails the TP (`tests/external/etsi/forge-latest-ratchet.txt`).

## 2. Edits of files forge carries

Each row changes a forge TP, fixture or library. The TP's verdict on forge's default branch is in the ratchet (section 4).

| commit | files | change | clause | upstream |
|---|---|---|---|---|
| `65a37be1` | `README.md`, `052_07.robot`, 40 fixtures under `data/` | the fixtures' @context URL names this repository's copy instead of `raw.githubusercontent.com/marek-mraz/ngsi-ld-test-suite`, which no longer exists and answered 404 | TS 104 176 Table 6.3.2-1: a context that cannot be fetched is LdContextNotAvailable (504), so every cell failed 653 TPs on a missing file | not for forge: forge's own fixtures name forge; `forge-free.sh` serves them locally for run (b) |
| `4f8784d9` | `020_17`..`020_20.robot`, `vehicle-different-attribute-types.jsonld`, `vehicle-with-scope.jsonld`, two 020 expectations | the setup creates the current-state Entity before deleting its Attribute; a deleted scope reads `urn:ngsi-ld:null` | TS 104 175 10.2.7.4 (5.6.5.4): ResourceNotFound when no Entity is held locally; 5.2.7: `urn:ngsi-ld:null` for a deleted scope | raises §15, §16; carries forge commits `f7c78f94`, `94789a66`, not on the default branch |
| `1051b8ef` | `vehicles-temporal-representation-021-08.jsonld` | the expectation drops the speed instances that fail the query | TS 104 175 11.3.3.4 (5.7.4.4): only the instances "which meet the temporal, query and geoquery restrictions" | raise §18 |
| `607683ec` | `037_11.robot`, `041_03.robot` | the request sends `Accept: application/ld+json`, the media type the expected pagination Link carries | TS 104 176 6.4.7.2: the Link `type` "shall be exactly equal to the media type resulting from the original request" | raise §19; forge `c4cdb0cf` |
| `802d37fd` | `051_04.robot` | 051_04_01 expects ResourceNotFound for an unknown @context id with `reload=true` | 5.13.5.4 | raise §20; forge `39921641` |
| `f9a7cd56` | `vehicle-temporal-representation-update.jsonld` | a second temporal upsert appends its instances | TS 104 175 11.2.2.4 (5.6.11.4): "all the Attribute instances included by the Temporal Evolution shall be added" | raise §21; forge `167fa185` |
| `0713e111` | `046_22_08.robot` | a deleted three-instance Attribute is notified as one tombstone per `datasetId` | TS 104 175 10.5.7 (5.8.6) | raise §22; forge `75ba471d` |
| `b676b19c` | `subscription-building-entities-active-watchedAttributes-query.jsonld` | the Subscription `q` in the body is written as is, not percent-encoded | TS 104 175 7.2.3.2: a filter is URI-encoded only "whenever the string acting as a filter is part of the HTTP binding's URI" | raise §23; forge `cd2ef2ba` |
| `00a67a78` | `libraries/FederationReset.py` | the reset's Link header names `SUITE_CONTEXT`, the context the run serves, instead of a fixed remote URL | harness, no clause: a run fetches no context off the run (`dev/etsi-local-context.sh`) | not for forge: run plumbing |
| `6281bcbd` | `itb/.../pom.xml`, `requirements.txt` | jackson 2.21.7 through Spring Boot 3.5.12, deepdiff 8.6.2 | none: five fixable HIGH CVEs in the ITB runner and its Python deps | not for forge in this form; no TP changes |
| `d24fefce` | `019_01_06.robot`, new `019_03_07.robot` | forge develop `cc8711ce` taken unchanged: QueryWithAndScope sends `(/a;/b)`, the rows get ids `019_01_06_01`..`08` (two new wildcard rows), and `019_03_07` refuses eight scopeQs outside the grammar | TS 104 175 7.2.5: "For logical and grouping parenthesis are needed"; ScopeQLevel = unicodeLetter *ScopeQLevelChar | forge `cc8711ce` (upstream already) |

## 3. Edits of Antares' own TPs

These files exist only here (138 carry an "Antares extension" note); a forge run never contains them, so no upstream report applies. They change with the Antares decision they test.

| commit | file | change | reason |
|---|---|---|---|
| `22486f88` | `DistributedOperations/.../436_05.robot` | a `contextSourceInfo` pair naming `NGSILD-Tenant` is refused | TS 104 175 9.5.1: tenant information "shall not" be given as contextSourceInfo |
| `1fccb27c` | `DistributedOperations/Subscription/5814_01.robot` | the Subscription `q` is JSON (`brandName=="Tesla"`), not URL-encoded | follows `b676b19c` (5.2.12, 7.2.3.2) |
| `e931d3b7` | `ContextInformation/Provision/TransientEntities/422_07.robot` | 422_07_03: an Entity its own `expiresAt` removes loses its Temporal Evolution | CIM 009 is silent on cascading expiry to history; platform decision DM-97 (owner 2026-10-18, T-4414) |

## 4. Forge's default branch as a ratchet

`external-suites.yml` runs forge's default branch of the run day beside the vendored copy. Its failures are `tests/external/etsi/forge-latest-ratchet.txt`: one TP id per line with its triage (bucket, clause, report), taken from section 4.1 of `final-validation.md`. Each `latest` cell is checked by `tests/external/etsi/ratchet.py`:

- a TP that fails and is not listed fails the job (a regression, or a new forge TP to triage);
- a listed TP that passes fails the job until it is taken off the list, so the list only shrinks;
- a listed TP that did not run fails the job (forge dropped or renamed it), except in the smoke preset.

The list holds 98 TPs (run 38860287033). Every `pinned` cell is checked by `dev/etsi-baseline.py check` and the pipeline's own gate (peak RSS), as the per-commit matrix does.

## 5. The platform image with its engine deciding

`image-jc.yml` runs the suite on the platform image (`full,jc-policy,...`) twice beside the allow-all cell, through `dev/etsi-pipeline.sh POLICY=<file>`: the overlay `compose-files/docker-compose-policy.yml` selects `ANTARES_POLICY=joinedcontext` on every broker and puts a subject proxy (`compose-files/haproxy-policy.cfg`) in the context-gateway's place, which strips client `x-jc-*` headers and names the suite `x-jc-client: etsi-suite`.

| cell | policy | expected |
|---|---|---|
| grant | `ci/policy/etsi/grant-all.json`: every clause to `etsi-suite` in the suite's fixed tenants; every suite | every baseline TP runs, and passes except the 14 of `ci/policy/etsi/grant-all.expected`, which must fail (`dev/etsi-baseline.py check` with `EXPECTED`) |
| restricted | `ci/policy/etsi/restricted.json`: the same grant and a prohibition of Retrieve and Query Entities (5.7.1, 5.7.2) on `Vehicle`; the Consumption suites | exactly the TPs of `ci/policy/etsi/restricted.expected` fail (`ratchet.py`), and every 403 of the run is the AccessDenied problem with one of the engine's two reasons and no URN (`ci/policy/etsi/refusals.py`) |

A listed TP that passes is red in both cells: a refusal that stops happening is as wrong as a new one. A tenant without a policy document denies every platform caller (`addons/joinedcontext-policy/README.md` section 2), so the 14 grant-cell TPs, each an Antares extension TP that names a tenant made up at run time, are refused under any document. Every forge TP passes in the grant cell as in the baseline.

The first grant run found two gaps, both fixed with this change: Upsert Temporal (5.6.11) asked the engine with the id alone, so no grant scoped to a type could ever admit it and the access-control member check never saw its body (`crates/antares-api/tests/policy_temporal_upsert_5_6_11.rs`); and a grant without information and without a condition refused a write that names no type, such as Delete Temporal of an absent Entity, which answered 403 for 404 (`decide::tests::a_grant_of_every_type_admits_a_write_naming_none`).

## Related

- `docs/conformance/final-validation.md`
- `docs/upstream/etsi-raises.md`
- `tests/external/README.md`
- `addons/joinedcontext-policy/README.md`
