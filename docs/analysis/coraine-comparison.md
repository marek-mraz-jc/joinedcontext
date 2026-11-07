---
title: Antares and Coraine (Seamware), side by side
task: T-3548
---

# Antares and Coraine

> Superseded in part by [Coraine's plugins and Antares' seams](coraine-architecture.md) (T-5063, refreshed by T-5210 on 2026-10-22). Rows this page got wrong since: Coraine now claims "CIM 009 v1.9.1 fully implemented" (its README), has 905 functional test cases, and has an in-broker authorization category (`fdsc`, `oidc`) although its `SECURITY.md` still says access control belongs to a gateway. On the Antares side, views, OData, MCP, LinkML and ICSR are built and on `SURFACE_SHELF`, not designed.

Coraine is an NGSI-LD broker written in C by Seamware, Apache-2.0, part of FIWARE (incubating). This page compares it with Antares on compliance, architecture, footprint and speed, and security, with the evidence for each claim on both sides, and ends with what Antares takes from it. The owner decided that Antares adopts tests and ideas only, no Coraine code.

Sources: Coraine at `19cff1619816459f269a63a8f54267811d3067ed` (main, 2026-10-10, "Merge pull request #328 from SEAMWARE/release/post-0.5.0"), image `quay.io/seamware/coraine:0.5.0@sha256:a102b7c7992f60e4b9f2885d4bef9099acf73613b489c4254cc5b7a32de93a6e` (SEAMWARE re-pushed the 0.5.0 tag on 2026-10-12 as index `sha256:5e2e519a7d0c9c4bfea6cb75113c8de5e8bc892fc281c06125cd67c4004fcaff` and quay no longer serves the old digest; the workflow pins the new one); the ETSI suite at forge.etsi.org; Antares at the commit this page lands on. Coraine paths below are relative to its repository root.

## 1. Compliance

Both brokers run the ETSI test suite, each its own copy, and each states compliance against a different text.

| Concern | Coraine | Antares |
|---|---|---|
| Specification it states against | TS 104 175/176 (the commits and `testsuite-doubts.md` cite them) | CIM 009 V1.9.1, one ledger file per clause (`docs/spec/`, 947 sections: 476 implemented, 3 partial, 468 informative); the TS 104 175 family checked clause by clause in [ts104175-delta.md](ts104175-delta.md) |
| TS 104 243 and TS 104 179 | MQTT notifications by the bridge plugin, written to TS 104 243 (`doc/mqtt-bridge.md:9`); TS 104 179 is named nowhere in the repository (`grep -rI '104.179\|ngsildProof'`: no hit) | MQTT is built in (ledger 7.2, TS 104 243 5.2 unchanged in substance); TS 104 179 is outside the ledger and tracked in delta 3.9 (T-3454) |
| Statement matrix | `doc/coverage.md`: 2314 statements of TS 104 175, 1051 asserted by a test | every implemented clause names its evidence and its robot test cases; `python3 dev/spec.py check` fails a clause without them |
| ETSI suite | the forge branch `integration/all-fixes` at `3f772fd`, develop `5c39851` plus 109 commits; `robot TP/NGSI-LD/`, 1106 cases, all pass | vendored from develop `196d184e` (2026-06-20), 1537 TP plus 286 IOP_TP, 1823 cases, all pass (`dev/etsi-baseline.json`) |
| Overlap of the two runs | 981 identical cases. 51 cases share a file and an id but test different things, so a TP id alone names no test across the two | 35 cases develop added after 2026-06-20 are missing; 527 cases are Antares' own |
| Own functional tests | 831 `test/funcTests/cases/*.test`, the whole HTTP exchange as text (section 6) | API, store and federation tests per crate; 16 fuzz targets; the policy seam walks all 92 routes |

### 1.1 What Coraine changed in the suite

The forge branch changes 161 files over 47 groups of commits. Read against CIM 009 V1.9.1, they sort into:

| Kind | Groups | Examples |
|---|---:|---|
| Suite bug fixed | 29, and one likely | 503 to 504 for LdContextNotAvailable (5.5.2); an assertion that checked the client's own request URL (D003); `Should Contain` on a list testing membership, not keys (D007, D013, D016) |
| Contested, the spec does not decide | 10 | the forwarded parameter `pick=` or `attrs=` (D011); 207 or 204 when a source misses on a write (D002_02_exc); verbs on `/entities/` with no id (002_02) |
| Relaxation | 4 | temporal pagination rewritten to TS 104 176 without the 206 and Content-Range checks of 6.3.10 (020_13, 020_14, 021_15, 021_16); 207 relaxed to 204 for a source error on a write, against 6.3.17 (D002_02_01_inc); a registration accepted filtered or whole (037_05, 037_08); propertyNames and relationshipNames folded into attributeNames (5.2.10) |
| Harness | 3 | HttpCtrl stub fixes, MQTT and logging |
| New cases | 1 | MR !301, 61 cases in 019 |

Antares carries 15 of the 29 already and two in part, several as raises in [etsi-raises.md](../upstream/etsi-raises.md) (504, exclusive registrations pinned to entity ids, D018_01, the D003 and D010 to D016 assertions). It does not carry the relaxations. Where a fix exposes an Antares defect, the vendored suite passed because its assertion was wrong too:

| TP | Clause | Antares answer | Task |
|---|---|---|---|
| 020_17 to 020_20 | 5.6.5.4 | Delete Attribute on an entity held only in the temporal store answers 204, not 404 | T-3557 |
| 020_19, 020_20 | 4.18 | a deleted scope in the temporal representation is `[]`, not `urn:ngsi-ld:null` | T-3558 |
| 021_08 | 5.7.4.4 | a temporal query returns instances that fail `q` | T-3559 |
| 041_03, 037_11 | 6.3.10 | the Link on csourceRegistrations pages always says `application/ld+json` | T-3560 |
| 051_04_01 | 5.13.5.4 | an unknown @context id with `reload=true` answers 400, not 404 | T-3561 |
| 008_01 | 5.6.11.4 | the temporal upsert replaces an instance with the same observedAt instead of adding it | T-3562 |
| 046_22_08 | 5.8.6 | `deleteAll` notifies one null without datasetIds, not one per instance | T-3563 |
| 046_05 | 4.9 | a Subscription `q` is percent-decoded although a JSON body is not URL-encoded | T-3564 |

Each was checked live on a memory-store build. Six of them (T-3557, T-3558, T-3560, T-3561, T-3563, T-3564) sit in code whose comment or ledger note cites the TP: Antares had been fitted to the assertion. Each task corrects the vendored TP with the fix and adds the correction to the raises.


## 2. Architecture

| Concern | Coraine | Antares |
|---|---|---|
| Extension unit | Shared libraries loaded at startup: `--database` (one current-state driver), `--troe` (one history driver), `--apiPlugins` (any number), `--bridges` (foreign transports for notifications), `--transports` (the API over WebSocket); README "Plugin architecture", `doc/plugin-architecture.md` | Cargo features compiled into one binary; the vanilla build carries no addon (ADR-0027, `dev/check-module-rules.py`) |
| Storage seam | `src/lib/db/DbDriver.h` (50 function pointers; NULL answers 501) and `src/lib/troe/TroeDriver.h`; plugins share structs with the broker by layout, so `tools/dbAbiStamp.sh` stamps the ABI | `CurrentStateDriver` and `TemporalDriver` traits (`crates/antares-store/src/lib.rs`, traits `CurrentStateDriver` and `TemporalDriver`), checked by the compiler; stores: memory, file (redb), postgres, timescale |
| Notification seam | HTTP and `cor://` built in; MQTT, Modbus, DDS as bridge plugins in their own repositories (`doc/plugin-architecture.md:296`) | `NotificationSink` registry (ADR-0016, `crates/antares-notifier/src/lib.rs`, trait `NotificationSink`); HTTP and MQTT ship |
| Extra API surfaces | `admin` API plugin (health, version, log, tenants, plugins); WebSocket transport (`doc/websocket.md`) | `ApiSurface` (`crates/antares-api/src/surface.rs:18`) under `/q` and `/x`; the admin surface ships; views, OData, MCP, LinkML and ICSR are designed (ADR-0023, ADR-0027) and not built yet |
| Authorization | None in the broker: "Access control belongs to an API gateway" (`SECURITY.md`); `doc/authorization.md` is "working notes, not built" for an optional JWT plus ODRL layer, operation, entity type and tenant first, attribute rules in a second version | `PolicyEngine` seam (ADR-0020, `crates/antares-api/src/policy.rs`, trait `PolicyEngine`) asked exactly once per operation (`tests/policy_gate_seam.rs`); the shipped engine allows all; endpoint views with per-rule attribute allowlists are designed (ADR-0023 rule 11), not built |
| Tenants | "a namespace, not a security boundary" (`SECURITY.md`) | `tenant_id` on every row plus Postgres row-level security, `ANTARES_REQUIRE_RLS` (`SECURITY.md`); guards in `crates/antares-sql/tests/` |
| Where the NGSI-LD rules live | Four repositories: `corNgsild`, `corRest`, `corJsonld` are static archives from SEAMWARE/corLibs whole-archived into the binary (`doc/coverage.md`) | One workspace, twelve crates |

Overlap with the Antares surfaces plan: Coraine's WebSocket transport carries the NGSI-LD API and its notifications over one connection, the same idea as the Antares `ws` surface; Coraine has nothing like views, OData, MCP, LinkML or ICSR, and no per-attribute allowlist.

## 3. Footprint and speed

Measured by `dev/perf/compare-coraine.sh` on a GitHub-hosted runner (workflow `compare-coraine.yml`, run 38076117618). Each broker gets CPU 0 to itself and k6 the others; both keep current state in memory and no history (Antares `ANTARES_STORE=memory ANTARES_TEMPORAL=none`, Coraine `--database corDB --troe none`); each shape runs three times and the table gives the median. Antares is the release build with default features; Coraine is the 0.5.0 image pinned by digest.

| | Antares | Coraine |
|---|---:|---:|
| binary, MiB | 29.1 | 5.1 |
| mapped files, MiB | 32.4 | 24.4 |
| idle RSS, MiB | 22 | 10 |
| process start to first answer, ms | 70 | 130 |
| RSS with 100 000 five-attribute entities, MiB | 1163 | 321 |

| Entities | Shape (k6, 50 VUs) | Antares req/s | p99 | Coraine req/s | p99 |
|---:|---|---:|---:|---:|---:|
| 1 000 | query `type=Vehicle&limit=20` | 317 | 176 ms | 4764 | 11 ms |
| 1 000 | query with `q` | 280 | 206 ms | 2539 | 21 ms |
| 1 000 | retrieve by id | 9367 | 10 ms | 20515 | 11 ms |
| 1 000 | upsert | 2279 | 40 ms | 3879 | 26 ms |
| 1 000 | create subscription | 3472 | 27 ms | 2185 | 27 ms |
| 100 000 | query | 13 | 25.7 s | 2669 | 12 ms |
| 100 000 | query with `q` | 12 | 28.4 s | 1538 | 22 ms |
| 100 000 | retrieve by id | 4905 | 10 ms | 11113 | 11 ms |

Read the ratios, not the absolute numbers: a hosted runner varies by a factor of two between runs (Coraine's retrieve at 1 000 entities measured 41 428 req/s in the run before this one, 20 515 here), and both brokers ran on the same runner within minutes.

- Retrieve, upsert and subscription creation are within a factor of two either way; Antares creates subscriptions faster.
- The query shapes are not. Antares' memory store hands every query a deep copy of every entity of the tenant (`crates/antares-sql/src/store/any.rs:700`, `store/mem/mod.rs:762`) and filters the copy, so a query costs the size of the tenant: 13 req/s at 100 000 entities. T-3556 fixed it for queries without `q` (table below). The Postgres store, which deployments run, pushes the filter into SQL and is not measured here.

After T-3556 (run 38078752333, same rig, Antares at 905da4e): the memory store filters on borrowed documents and copies only the page.

| Entities | Shape (k6, 50 VUs) | Antares req/s | p99 | Coraine req/s | p99 |
|---:|---|---:|---:|---:|---:|
| 1 000 | query `type=Vehicle&limit=20` | 2665 | 23 ms | 4834 | 11 ms |
| 1 000 | query with `q` | 255 | 228 ms | 2515 | 21 ms |
| 100 000 | query | 1459 | 21 ms | 2032 | 13 ms |
| 100 000 | query with `q` | 12 | 29.1 s | 1172 | 21 ms |
| 100 000 | retrieve by id | 4629 | 11 ms | 8285 | 12 ms |

A query with `q` took the old path: the memory arm left `q` undecided, handed the API a copy of the tenant and let it filter. T-3572 runs the API's own evaluator on the borrowed documents for a `q` without linked terms (run 38081147511, same rig, Antares 3385b28):

| Entities | Shape (k6, 50 VUs) | Antares req/s | p99 | Coraine req/s | p99 |
|---:|---|---:|---:|---:|---:|
| 1 000 | query with `q` | 1758 | 31 ms | 2523 | 21 ms |
| 100 000 | query with `q` | 923 | 37 ms | 1125 | 21 ms |

The shape's `q` (`speed>100;brand=="Tesla"`) is selective, so a page of 20 walks many documents; each walk resolves the query's terms against the @context again per document.
- Antares holds 3.6 times Coraine's memory per entity: the expanded JSON-LD form with IRI keys and instance arrays, in `serde_json` values.
- The first run kept Antares' history on (`ANTARES_TEMPORAL_RECORD=all`, the default) against Coraine without one: RSS at 100 000 entities was 2355 MiB, twice the parity figure. A comparison with history needs Coraine's `--troe` driver and is not done here.

## 4. Security

| Concern | Coraine | Antares |
|---|---|---|
| Memory safety | C. The 0.5.0 changelog fixes, among others: a request with more than 15 headers "freed memory it did not own" (crash on the first forwarded request of a registration with 14 or more `contextSourceInfo` entries), forwarded responses that "pointed into its connection's buffer, which could be reused by another request or freed", a notification `Link` header "from a buffer that had gone out of scope", allocations not 8-byte aligned (`CHANGELOG.md:157-175`) | Rust, `unsafe_code = "forbid"` for the workspace (`Cargo.toml:163`) |
| Sanitizers and valgrind | The functional suite under ASan, UBSan and LeakSanitizer before a release (`.github/workflows/prerelease.yml:130`), valgrind shards nightly (`nightly.yml` job `valgrind`) | Not applicable to safe Rust; `cargo-audit`/`cargo-deny` on dependencies (`advisories.yml`, weekly) |
| Fuzzing | None found (`grep -ri fuzz` over workflows, scripts, docs: no hits) | 16 cargo-fuzz targets (`fuzz/fuzz_targets/`), twice a week (`fuzz.yml`) |
| Mutation testing | None found | `mutants.yml`, weekly |
| Soak | `test/soak/soak.py`: hours of mixed load per database, RssAnon bound after warm-up, every notification delivered, before a release | None (roll-weekly restarts, perf-weekly and scale-weekly load); adopted as T-3551 |
| Upgrade | `test/upgrade/upgradeCheck.sh`: the previous release's data opened by this one | `dev/upgrade-path.sh`: the previous release's file and postgres stores served by this one |
| Disclosure | Private GitHub advisories; no published advisory (`GET /repos/SEAMWARE/coraine/security-advisories` returns `[]`, 2026-10-10) | E-mail to the maintainer (`SECURITY.md`) |

## 5. What Antares adopts

| Idea | Coraine evidence | Decision |
|---|---|---|
| Soak before a release: hours of mixed load, RSS bounded after warm-up, every notification delivered | `test/soak/soak.py` | adopted, T-3551 |
| The suite's @context served by the run itself, not fetched from GitHub | Coraine lost a whole run to forge answering 500 on 2026-09-26 and serves the context locally since; Antares fetched `raw.githubusercontent.com` (`ngsi-ld-test-suite/resources/variables.py:3`) | adopted, T-3550 |
| Carry the suite fixes that expose a broker defect | section 1.1 | adopted, T-3557 to T-3564 |
| Run Coraine's functional suite against Antares | 831 `.test` cases (section 6) | adopted as the adapter T-3549 uses |
| Query scaling on the in-memory store | section 3: Coraine 4828 req/s at 100k entities | adopted as a fix, T-3556 |
| A statement matrix of TS 104 175 (2314 statements, 1051 asserted) | `doc/coverage.md` | not adopted: the Antares ledger is per clause with evidence and robot cases, and [ts104175-delta.md](ts104175-delta.md) carries every clause's TS 104 175 verdict |
| Sanitizers and valgrind shards | `prerelease.yml`, `nightly.yml` | not applicable: `unsafe_code = "forbid"` |
| Plugins as shared libraries with an ABI stamp | `tools/dbAbiStamp.sh` | not adopted: Cargo features and traits checked by the compiler (ADR-0027) |
| API over WebSocket | `doc/websocket.md` | already planned as the `ws` surface (T-3517) |
| Broker without authorization, tenants as namespaces | `SECURITY.md` | not adopted: the policy seam and row-level security stay |

## 6. Running Coraine's functional suite against Antares (T-3549)

The suite is `test/funcTests/cases/*.test`, 831 files. Each has `--NAME--`, `--TAGS--`, `--INIT--`, `--RUN--`, `--EXPECT--` and `--TEARDOWN--` sections; `--RUN--` is bash calling `corCurl`, and `--EXPECT--` is the whole HTTP exchange as text, with `REGEX(...)` for the parts that vary (`test/funcTests/cases/subscription_notif_format.test`). The runner is `corTest` from SEAMWARE/corLibs (`bin/corTest`, called at `.github/workflows/ci.yml:421`), and it sources `test/funcTests/corTestFunctions.sh` for the broker's lifecycle: `coraineStart` (`:190`), `coraineStop` (`:323`), `corDbDrop` (`:442`), `corTroeInit` (`:556`), `corSnapDrop` (`:620`).

The adapter in `tests/external/coraine/` (Apache-2.0 notice kept, nothing vendored beyond the harness):

1. A shim sourced after `corTestFunctions.sh` redefines `coraineStart`/`coraineStop` to run the Antares binary on the role's port (`ANTARES_HTTP_PORT`, `ANTARES_STORE`), and `corDbDrop`, `corTroeInit`, `corTroeDrop`, `corSnapDrop` to reset the Antares store of the chosen mode (memory: restart; file: a fresh `ANTARES_DATA_DIR`; postgres and timescale: drop and recreate the database).
2. The comparison cannot be Coraine's text diff: the expected text carries Coraine's header set and order and its error `detail` wording. The shim compares the status line, the headers NGSI-LD defines (`Location`, `Content-Type`, `Link`, `NGSILD-*`, `Content-Range`) and the JSON body parsed, with `detail` and `title` masked; a case passes when those agree.
3. Not applicable, listed by name with the reason: 115 cases at `19cff16` by a first count (REQUIRE_FEATURE of a Coraine-only feature: SERVICE_EXECUTION, RESPONSE_BUDGET, ADMIN_API, METRICS, HEALTH, AUTO_ENTITY_MAP, MONGOC, ICU_COLLATION, 31; `--bridges` 45, of which the MQTT ones can run against Antares' own MQTT binding; `--apiPlugins` 14; `/admin/` 10; `mosquittoWait` 6; `--transports` 4; `coraineImport` 2; `corHttpsCertGen` 2; Modbus 1). The command is in the appendix. About 716 cases are candidates to run.

## Related

- [ts104175-delta.md](ts104175-delta.md): Antares against TS 104 175/176/243/179.
- [../src/conformance.md](../src/conformance.md): the Antares ledger, suite and matrix.
- [../upstream/etsi-raises.md](../upstream/etsi-raises.md): suite and spec defects Antares raises.
- `tasks/T-3549`: the final validation that runs both suites.

## Appendix. Commands

```sh
# the footprint and speed run (section 3), one CPU per broker
gh workflow run compare-coraine.yml --ref dev     # .github/workflows/compare-coraine.yml
BIN=target/release/antares CORAINE_IMAGE=quay.io/seamware/coraine:0.5.0@sha256:5e2e519a7d0c9c4bfea6cb75113c8de5e8bc892fc281c06125cd67c4004fcaff \
  OUT=compare-coraine dev/perf/compare-coraine.sh

# the forge suite, Coraine's branch against develop
git clone https://forge.etsi.org/rep/cim/ngsi-ld-test-suite.git suite && cd suite
git merge-base origin/integration/all-fixes origin/develop          # 5c398518
git log --oneline --stat 5c398518..origin/integration/all-fixes      # 109 commits
git diff --stat 5c398518 origin/integration/all-fixes                # 161 files

# Coraine's functional cases that need a Coraine-only feature (section 6)
grep -lE 'REQUIRE_FEATURE|--bridges|--apiPlugins|/admin/|mosquittoWait|--transports|coraineImport|corHttpsCertGen|modbus' \
  test/funcTests/cases/*.test | wc -l
```
