---
title: Final validation against Coraine's functional suite and the ETSI suite
task: T-3549, T-3677
---

# Final validation

Antares, the vanilla binary and the all-features binary from one dev commit, against two suites it did not write, on each store it ships (memory, file, postgres, timescale). The job is `.github/workflows/external-suites.yml`; the harness and how it maps each suite onto Antares is `tests/external/README.md`. The full run happens once every other T-3446 dependency is finished, and again after any later round.

## 1. What runs

| suite | source | cells |
|---|---|---|
| Coraine functional | SEAMWARE/coraine `test/funcTests/cases` at `d313d87fabd74f67e0d4d1f222ccf0c45239a0a9` (831 cases), corTest from SEAMWARE/corLibs | 2 binaries × 4 stores |
| ETSI (a) | `ngsi-ld-test-suite/` as this repository carries it at the commit under test | 2 binaries × 4 stores |
| ETSI (b) | forge.etsi.org `cim/ngsi-ld-test-suite`, default branch on the run day | 2 binaries × 4 stores |
| cross-surface oracle | `crates/antares-broker/tests/surfaces_equivalence.rs` (T-3522) | all-features binary |

## 2. Data sources

Every number in section 3 comes from the artifacts of the external-suites run named there:

- Coraine counts: `coraine-<binary>-<store>/coraine-report.json`, field `counts` (compare.py); a failure's first difference is its `reason`.
- ETSI counts per suite commit: `tp-status.csv` of `etsi-<pinned|latest>-<binary>-<store>`, column `status`, folded by `tests/external/etsi/tp-diff.py`; the suite commit is the cell's `suite.txt`.
- Oracle: `oracle/oracle.log`.

## 3. Results

Full run: external-suites run 38860287033, workflow_dispatch on `74b72185` (2026-10-16), every cell on both binaries and all four stores. The run before it, 38849534862 on `612a061b`, failed nearly every Coraine case on the postgres and timescale stores in INIT: the harness's per-case `DROP DATABASE IF EXISTS` printed a NOTICE on stderr, and corTest fails an INIT on any stderr. `74b72185` silences the NOTICE and changes `tests/external/coraine/antares.sh` only; its ETSI, file and memory numbers equal this run's.

### 3.1 ETSI

| suite | suite commit | per cell (vanilla and full × memory, file, postgres, timescale) |
|---|---|---|
| (a) pinned | vendored `ngsi-ld-test-suite/` at `74b72185` | 1823 PASS, 0 FAIL; peak-RSS gate (350 MiB) PASS |
| (b) latest | forge `56e721d0` (5 cells) and `5c398518` (3 cells) | 995 PASS, 98 FAIL |

The 98 TPs (b) fails are the ratchet `tests/external/etsi/forge-latest-ratchet.txt`, each with its triage from 4.1; since T-4855 a `latest` cell that fails another TP, or passes a listed one, fails the job (`docs/conformance/suite-edits.md` section 4). All 8 cells of a suite fail the same TPs, so no store and no binary differs; the forge's main moved from `5c398518` to `56e721d0` during the run and the failing set stayed the same. Run 38806308784 on `214c75a4` ran forge `5c398518` and had 97 failures on (b). The one new failure is 028_06: `be3244ae` initialises an absent Subscription `jsonldContext` (5.8.1.4), and the forge TP compares the whole Subscription read back, where its siblings 028_01 and 030_03 ignore that member. Run 38793032271 on `cb88e44` had 149 failures on (b); 60 of them were harness failures (48 IOP setups that sent `/ngsi-ld/v1/ngsi-ld/v1`, 12 MQTT TPs given an unreachable broker host), gone since `214c75a4`. Triage of the 8 IOP TPs that the doubled path had hidden found two Antares bugs in distributed queries, both under 4.3.6.1 and fixed on this branch (`65d53b87`, `be403960`). The forge IOP tree, run locally on `be403960` (memory store), gives 40 PASS / 8 FAIL, the same 8 TPs, each failing on its test bug alone.

#### T-5495 re-run (2026-10-24)

Local `dev/etsi-pipeline.sh`, vanilla image, memory store, `STOP_ON_ERROR=0`, the forge tree made forge-free as
external-suites.yml does. Results stay out of git; rerun the same way to reproduce.

| forge ref | sha | antares | TPs | not passing | of them MQTT 058_* (sandbox) |
|---|---|---|---|---|---|
| develop (default branch) | `cc8711ce` | dev `98e7f38` | 1101 | 101 | 12 |
| `integration/all-fixes` | `6475ccb9` | `bafe084e` (dev `98e7f38` + T-5486 + T-5495) | 1158 | 94 | 12 |

- develop, against the ratchet: 13 listed TPs pass because forge fixed them (`b3bb243f` asserts 504: 043_01_01/02/03/05,
  028_07_02; `25b3e0fa` adds objectType: 018_10_01/02, 018_13_01/02, 019_13_01/02, 019_16_01/02). They came off the
  ratchet, 85 remain. New failures: 019_03_07_05 and 019_03_07_08 (Antares bug, fixed, 4.2), D002_02_01_inc and
  D002_02_02_inc (T-5486, fixed on its branch, which this one carries), and the 12 MQTT TPs.
- The 12 MQTT TPs fail in the sandbox only: `eclipse-mosquitto:2` resolved to 2.1.2, which in this docker-in-docker
  host cannot open even its own default config, so no TP reached a broker. CI's `latest` cells are the arbiter.
- all-fixes, against the ratchet: 67 of the 85 TPs pass, which confirms their test-bug verdicts by an independent suite
  fix; 31 listed TPs fail on both refs and keep the 4.1 rows. 51 TPs fail on all-fixes only (4.1, rows marked
  "all-fixes"). The first all-fixes run failed all 1158 TPs: `dev/etsi-run.sh` rewrote the multi-line
  `ngsild_test_suite_context` assignment with a one-line sed; the harness now appends the assignment.

### 3.2 Coraine functional

831 cases per cell. The verdicts come from `compare.py`; "pass" counts the cases that match after its normalisation.

Current numbers: weekly external-suites run 40501072590 (schedule, dev `e48eaf35`, 2026-10-27, Coraine `d313d87`).

| cell | pass | fail | not applicable | error |
|---|---:|---:|---:|---:|
| full-file | 335 | 312 | 180 | 4 |
| full-memory | 331 | 312 | 183 | 5 |
| full-postgres | 334 | 313 | 180 | 4 |
| full-timescale | 334 | 313 | 180 | 4 |
| vanilla-file | 336 | 311 | 180 | 4 |
| vanilla-memory | 331 | 312 | 183 | 5 |
| vanilla-postgres | 332 | 315 | 180 | 4 |
| vanilla-timescale | 334 | 313 | 180 | 4 |

Against run 38860287033, vanilla-file case by case: 20 cases pass now and none passes there that fails here. The 20:
append_scope_union_single, csource-reg-distops-append-exclusive, csource-reg-distops-create-207,
csource-reg-distops-create-409, csource-reg-distops-create-excl-no-op, csource-reg-distops-retrieve-exclusive,
distop_retrieve_timeout_504, duplicate_id_type_alias_every_level, entity_batch_upsert_distops_exclusive,
entity_batch_upsert_distops_inclusive, entity_patch_geoproperty_validation, expanded_core_terms_input, json_property,
linked_entities_objecttype_gate, troe_entity_modifiedat_attr_update, troe_timescale_multi_type,
troe_timescale_subattrs_and_instance_delete, troe_timescale_temporal_deleted_kinds,
troe_timescale_temporal_linked_projection and value_type_subattr (the T-5213 group fixes T-5463 to T-5467).
Twelve cases differ between cells, none between the two binaries: the memory-store and timing cases below, plus
notify_rapid_patches, snapshot_write and subscription_notif_show_changes_delete, each failing on one or two SQL-store
cells only. In the same run all eight ETSI latest cells failed only the ratchet: 038_05_01 passes since forge commit
`c5a1343` (suite `c5a13430`; the run of 2026-10-25 had `cc8711ce`) added `Library DateTime` to 038_05.robot, whose
`Get Current Date` keyword was missing before. No Antares change was involved; T-5852 took the TP off the ratchet.

History: run 38860287033 (2026-10-16, `74b72185`).

| cell | pass | fail | not applicable | error |
|---|---:|---:|---:|---:|
| full-file | 315 | 332 | 180 | 4 |
| full-memory | 311 | 332 | 183 | 5 |
| full-postgres | 316 | 331 | 180 | 4 |
| full-timescale | 315 | 332 | 180 | 4 |
| vanilla-file | 316 | 331 | 180 | 4 |
| vanilla-memory | 310 | 333 | 183 | 5 |
| vanilla-postgres | 316 | 331 | 180 | 4 |
| vanilla-timescale | 316 | 331 | 180 | 4 |
| harness | csr_subscription_on_delete_notif, csr_subscription_on_sub_patch_notif, csr_subscription_on_update_notif | expected JSON with REGEX(\.\|\d) inside a string not parsed | same compare.items "Invalid \escape" fallback; review again after the fix |
| harness | csr_subscription_initial_notif | expected JSON with REGEX(\.\|\d) inside a string not parsed | compare.items: json raw_decode fails on "Invalid \escape" (e.g. notifiedAt REGEX(...(\.[0-9]{1,9})?Z), Content-Length REGEX(\d+)), so the whole expected block degrades to text lines; quote_bare_regex should double backslashes that are not valid JSON escapes inside strings when expected=True. Next diffs likely: Notification id scheme, User-Agent, attributeNames fold |

Of the 831 cases, 11 do not give the same verdict in all 8 cells, and none of them is a difference between the two binaries:

- Memory store, 7 cases: query_q_value_percent, snapshot_subscriptions, subscription_mongoc_persist and tenant_persistence restart the broker without Coraine's `persist` tag, so they fail on memory, which keeps nothing across a restart, and pass on the other stores. snapshot_persistence, subscription_periodic_q_after_restart and subscription_type_after_restart carry the tag and count as not applicable on memory; the first two fail on file, postgres and timescale (triaged in 4.4).
- troe_entity_modifiedat_attr_update fails on file and memory and passes on postgres and timescale: the temporal representation's entity `modifiedAt` stays at the create on the in-process stores, an open Antares bug (4.4).
- Timing and order, 3 cases: subscription_triggers (two notifications of one deletion arrive in either order; fails on full-file, full-timescale and vanilla-memory), snapshot_distop_capture (query result order without `orderBy` on the SQL stores; fails on full-postgres, full-timescale and vanilla-timescale) and subscription_notify_batch_merge (no notification yet when the case read the mock; vanilla-postgres only). Each fails on at most three cells and passes on both binaries elsewhere.

### 3.3 Cross-surface oracle

`surfaces_equivalence`: 2 passed, 0 failed (`count_and_paging_agree_on_every_surface`, `the_same_read_returns_the_same_entities_on_every_surface`).

## 4. Triage

Each failure goes into exactly one bucket, with the clause quoted (CIM 009 V1.9.1 numbering unless TS 104 175 or 176 is named):

- Antares bug: Antares breaks the clause. Fixed on this branch with a red-first test (4.2), or open (4.4); an open bug needs a task in T-3446 `depends-on`.
- Suites disagree: ETSI and Coraine assert opposite answers (4.3).
- Spec unclear: the spec does not decide; Antares keeps its behaviour.
- Antares per spec: the clause supports Antares' answer and the case expects another.
- Coraine-specific: the case checks a choice of Coraine's own that the spec leaves open, such as an id scheme, an error text, a delivery order or a cursor.
- Test bug or harness: the TP, the case or this repository's harness is wrong. ETSI test bugs go upstream (forge, or `docs/upstream/etsi-raises.md`).

### 4.1 ETSI

| suite | case or TP | bucket | clause | task or upstream report |
|---|---|---|---|---|
| ETSI (b) | IOP_003_03_02_01/02 | test bug, after an Antares fix | first an Antares bug: b1 answered with `Vehicle:2` from b4, whose registration names OffStreetParking only, against TS 104 175 4.3.6.1 "Ultimately, all constraints specified in the registration shall be respected"; fixed in `65d53b87` (red-first `tests/fed_query_entity_types_4_3_6_1.rs`). The TP then fails on its last check, which compares b1's Vehicle:1, queried with `attrs=location`, against the full Vehicle; 10.4.3.5 (5.7.2.5): "For each matching Entity, only the Attributes specified by the Attribute list parameter shall be included" (the TP builds `first_vehicle_location_expected` and never uses it) | forge |
| ETSI (b) | IOP_003_02_02_01/02 | test bug | its first entity check compares b2's own OffStreetParking:1, which the setup creates from `full-version-of-OffStreetParking1.jsonld` (location -8.5, 41.2), against `offStreetParking1-with-location-and-name-only.jsonld` (-8.9, 42.2, `observedAt`): no broker can pass it. Triaging it found a second 4.3.6.1 bug: two registrations of one source folded to the union of their scopes, (OffStreetParking × two Attributes) + (Vehicle × all) = (both × all); fixed in `be403960` (red-first `registrations_of_one_source_fold_into_one_forward`, `two_registrations_of_one_source_do_not_cross_their_scopes`) | forge |
| ETSI (b) | IOP_002_04_01_01/02 | test bug | the TP documentation says b3's exclusive registration provides b5's `location`, but `context-source-registration-exclusive-1.jsonld` registers `urn:ngsi-ld:OffStreetParking:2` `totalSpotsNumber`; b1's only other path runs through b2's auxiliary registration, scoped to `availableSpotsNumber`/`totalSpotsNumber` (4.3.6.1), so no `location` is due | forge |
| ETSI (b) | IOP_002_04_02_01/02 | test bug | TS 104 175 5.9.2.4 (redirect): "If an existing Entity already matches the Context Source Registration, an error of type Conflict shall be raised." The setup creates OffStreetParking:1 on b2, then registers redirect-2 (type OffStreetParking) on b2 | forge |
| ETSI (b) | 043_01_01/02/03/05, 028_07_02 | test bug | TS 104 176 Table 6.3.2-1: `LdContextNotAvailable` is 504; the TPs assert 503 | `docs/upstream/etsi-raises.md` §3 |
| ETSI (b) | 018_10_01/02, 018_13_01/02, 019_13_01/02, 019_16_01/02 | test bug | TS 104 175 C.2.2.1.2: "any Relationships which target Entities stored locally or include an objectType Attribute are returned in an expanded format" (the example keeps `objectType`) | forge: expectations drop `objectType` |
| ETSI (b) | 021_23_01, 021_23_03..09 | test bug | TS 104 175 11.3.3.4: "If the ordering parameter is present and refers an entity name other than "id", then an error of type BadRequestData shall be raised." | forge |
| ETSI (b) | 021_08_01 | test bug | TS 104 175 11.3.3.4: only the Attribute instances "which meet the temporal, query and geoquery restrictions" | `etsi-raises.md` §18 |
| ETSI (b) | 021_25 | test bug | TS 104 175 11.3.3.4: "(all Entities stored locally, in case only a local scope is specified)"; the shared 021-06 expectation lists one of the two Entities the TP creates | forge |
| ETSI (b) | 020_17_01..03, 020_18_01..03, 020_19_01, 020_20_01 | test bug | TS 104 175 10.2.7.4: an Entity not held locally gives ResourceNotFound; 5.2.7: `urn:ngsi-ld:null` "shall be only used and only appear in case of deleted scopes" | `etsi-raises.md` §15, §16 |
| ETSI (b) | 060_05_01 | test bug | TS 104 175 10.2.12.4: "If none of the above is provided, then an error of type BadRequestData shall be raised (too wide query)." | forge |
| ETSI (b) | 008_01_01 | test bug | TS 104 175 11.2.2.4: "all the Attribute instances included by the Temporal Evolution shall be added to the existing Entity" | `etsi-raises.md` §21 |
| ETSI (b) | 031_01_01 | test bug | TS 104 175 5.2.6.5.2: the `notificationTrigger` default is not echoed; the TP's ignore pattern is quoted and never matches | forge |
| ETSI (b) | 031_02_02 | test bug | TS 104 176 6.4.7.2: Link "as mandated by IETF RFC 8288 section 3" (whitespace around `;` allowed); the TP compares raw text | forge |
| ETSI (b) | 046_05_01 | test bug | TS 104 175 7.2.3.2: a filter is URI-encoded only "whenever the string acting as a filter is part of the HTTP binding's URI" | `etsi-raises.md` §23 |
| ETSI (b) | 046_22_08 | test bug | TS 104 175 10.5.7: a deleted Attribute with datasetIds is notified per instance (an array) | `etsi-raises.md` §22 |
| ETSI (b) | 037_07_01 | test bug | TS 104 175 5.2.6.5.3: a CSR `location` is a "GeoJSON Geometry", not a GeoProperty | forge |
| ETSI (b) | 037_07_02 | test bug | RFC 7946 3.1.6 (TS 104 175 7.2.4): the polygon is not a closed linear ring | fixed in the vendored suite |
| ETSI (b) | 037_10_01/02 | test bug | TS 104 176 6.2.3: an `application/json` response carries the context as a Link; the TP passes CSR ids as the entity `id` filter | forge |
| ETSI (b) | 037_11_01/02, 041_03_01/02 | test bug | TS 104 176 6.4.7.2: the Link `type` "shall be exactly equal to the media type resulting from the original request"; the TPs send no Accept | `etsi-raises.md` §19 |
| ETSI (b) | 033_01_02, D012_01_exc, D013_01_exc, D013_02_exc, D014_01_exc, D014_02_exc, D015_01_exc, D016_01_exc | test bug | TS 104 175 9.3.3: "An exclusive registration shall always relate to specific Attributes found on a single Entity" | `etsi-raises.md` §2 |
| ETSI (b) | D006_01_red, D006_02_red, D012_01_red, D013_01_red, D013_02_red, D014_01_red, D014_02_red, D015_01_red, D016_01_red, D001_02_inc, D001_03_03_inc | test bug | knock-on of the refused exclusive CSRs above: their setups leave Vehicles behind, and TS 104 175 12.2.2.4 then requires Conflict ("If an existing Entity already matches the Context Source Registration") | forge (test isolation) |
| ETSI (b) | 047_01, 047_02, 047_04, 047_05, 047_06, 047_08, 047_09 | test bug | the CSR fixture registers Vehicle/OffStreetParking while the subscription watches Building: no notification is due (TS 104 175 5.11.7 matching) | forge |
| ETSI (b) | 047_16_01, 047_16_03, D003_01_exc, D003_02_exc | test bug | TS 104 176 6.2.4: "In the absence of such Link Header, then the @context shall be the default @context."; the fragments are sent as `application/json` without a Link | forge |
| ETSI (b) | 028_06 | test bug | 5.8.1.4: "If not present, the jsonldContext field shall be initialized with the @context applicable for the Subscription"; the TP compares the whole Subscription read back, while 028_01 and 030_03 ignore `jsonldContext` | forge |
| ETSI (b) | 039_01_01 | test bug | TS 104 175 5.2.6.5.2: `status` is "Provided by the system when querying the details of a subscription" | forge |
| ETSI (b) | D011_02_red_02, D011_03_inc_02, D011_04_inc_02 | test bug | TS 104 176 6.2.4: a JSON-LD Link header with `application/ld+json` "shall result in an HTTP error response of type BadRequestData" | forge |
| ETSI (b) | D011_02_exc_02 | test bug | TS 104 175 Table 9.2-2: `redirectionOps` has queryEntity, not queryBatch; only the batch URL is stubbed | forge |
| ETSI (b) | D001_01_inc | test bug | TS 104 175 9.2: without `operations` the default is "federationOps", which has no createEntity | forge |
| ETSI (b) | D002_02_exc | test bug | TS 104 175 10.2.8.4: a partly failed delete is a partial success (207) | forge |
| ETSI (b) | D005_01_inc | test bug | TS 104 175 10.2.5.4: no default instance of `speed` locally gives ResourceNotFound for that part, so 207 | forge |
| ETSI (b) | 038_05_01, 042_02_01, 042_03_01, D004_01_inc, D010_01_exc, D016_01_inc | test bug | tooling: a keyword library not imported, `expected_status=any` missing, an unset `$payload`, a stub on another URL than the one counted, `Should Contain` on a list | forge |
| ETSI (b) | 019_03_07_05, 019_03_07_08 | Antares bug, fixed | TS 104 175 7.2.5: "For logical and grouping parenthesis are needed"; `ScopeQLevel = unicodeLetter *ScopeQLevelChar` | 4.2 (T-5495) |
| ETSI (b) | 043_01_01/02/03/05, 028_07_02, 018_10_01/02, 018_13_01/02, 019_13_01/02, 019_16_01/02 | test bug, fixed upstream | forge `b3bb243f` (504) and `25b3e0fa` (objectType) | off the ratchet (T-5495) |
| ETSI (b) | D002_02_01_inc, D002_02_02_inc | Antares bug, fixed | 6.3.17: a source's 404 is "not abnormal" only for HTTP GET | T-5486 |
| all-fixes | 054_02_02, 056_02_02 | Antares deviates, held by forge develop | TS 104 176 6.3.2: 405 "shall be raised when a client invokes a recognized but unsupported HTTP verb over a resource"; develop expects 400 | T-5584, `etsi-raises.md` §25 |
| all-fixes | 003_06_01, 003_08_01 | Antares deviates, held by forge develop | TS 104 176 6.2.4: "if the request payload body (as JSON) contains a @context term, then an HTTP error response of type BadRequestData shall be raised"; develop expects 207 | T-5584, §26 |
| all-fixes | 047_03_01, 047_07_01/02, 047_10_01, 047_11_01/02, 047_12_01, 047_13_01, 047_14_01, 047_15_01, 047_16_02; knock-on 041_01_01, 041_02_03, 041_03_03 | Antares deviates, held by forge develop | TS 104 175 12.4.7: "the csource notification shall be sent on initial subscription"; Antares skips it while nothing matches | T-5584, §27 |
| all-fixes | 019_26_01..06 | test bug | TS 104 175 7.2.3.2 defines q, expandValues, jsonKeys; the TPs send Coraine's `langProperties` (400 unknown parameter). Without it Antares answers each `street[fr]`/`street[*]` query as 7.2.3.2 reads | §28 |
| all-fixes | 019_02_08_05, 019_27_05 | Antares per spec | TS 104 175 5.2.6.4.10: `json` is "Raw unexpandable JSON which shall not be interpreted as JSON-LD using the supplied @context" | §28 |
| all-fixes | 020_13_01..04/06..09, 021_15_01..04/06, 021_16_01 | test bug | RFC 8288 3 (TS 104 176 6.4.7.3): OWS around `;`; the assertion regex wants `>;rel=`. Antares' Links carry offsetN | §28 |
| all-fixes | 020_13_05, 020_14_01/02, 021_15_05 | harness | TS 104 176 6.4.7.3: the page is "lastN or the maximum limit configured, whichever is smaller"; the ETSI stack configures 9 | §28 |
| all-fixes | D011_01_exc, D011_01_03_inc, D011_02_exc_01 | test bug | TS 104 176 7.2.3.1 defines `attrs`; the stub matches only `pick=` | §28 |
| all-fixes | 001_02_02, 016_02_06, D002_02_01_inc, D002_02_02_inc, D004_01_inc | suites disagree, spec unclear | InvalidRequest vs BadRequestData for `{}`; 405 vs 400 for an empty path segment; 6.3.17 on a source's 404 to a write | T-5497 |

### 4.2 Antares bugs fixed on this branch

Each fix has a red-first test and a note in its `docs/spec/` clause file. The Coraine case or ETSI TP named is the one that found it.

| commit | clause | what was wrong | found by |
|---|---|---|---|
| `65d53b87` | TS 104 175 4.3.6.1 | a forwarded query answer outside the registered types was imported | ETSI IOP_003_03_02 |
| `be403960` | 4.3.6.1 | two registrations of one source folded into the union of their scopes | ETSI IOP_003_02_02 |
| `4a20f531` | 5.5.8, 5.5.12 | partial update and merge kept members set to the NGSI-LD Null | the first T-3677 triage (worker-5) |
| `be3244ae`, `5ab1a741` | 5.8.1.4, 5.11.2.4 | an absent Subscription jsonldContext was not initialised; the CSR Subscription then gained one it must not have | Coraine subscription_jsonldcontext_autofill; ETSI 044/045 locally |
| `3de63385` | 5.6.5.4 | deleting an Attribute or scope the Entity no longer has answered 204 | Coraine |
| `b2b3d729` | Table 5.2.12-2 | a failed delivery turned an entity Subscription `failed`; only a CSR Subscription does (5.11.7) | Coraine |
| `9475d0a6` | 5.2.12 | an update could delete both `entities` and `watchedAttributes` | Coraine subscription_patch_validation_errors |
| `4c8ef405` | 4.5.2.2 | an Attribute `type` that is no string was accepted | Coraine |
| `9aab7adb` | 4.6.3 | a comma DateTime fraction was echoed with the comma ("In requests" only) | Coraine datetime_comma_separator |
| `af1bb939` | 5.2.12, 4.15 | a Subscription's `lang` was not applied to its notifications | Coraine langprop_get_none and 2 more |
| `6f0568ab` | 5.8.6 | an Entity deletion notified every Attribute as the Null beside `deletedAt` | Coraine subscription_notify_update_delete |
| `20cb9949` | Table 6.8.3.2-1 | Query CSRs refused `pick` and `omit` | Coraine csource-reg-discovery-filters |
| `849443ac` | Table 6.4.3.2-1 | a `datasetId` URL parameter that is no URI was accepted | Coraine url_param_datasetid_validation |
| `8488433b` | Table 5.2.12-1 | `ngsildConformance` that is not major.minor was accepted | Coraine subscription_ngsild_conformance |
| `ea8d6859`, `ee74ed99` | 4.5.18.2 | a partial update's languageMap went unchecked; keys that are no RFC 5646 language tag were accepted | Coraine langprop_patch_attribute, languagemap_key_rfc5646 |
| `310f7ea8` | 4.17 | a type selection outside the grammar (`type=(A;)`) was accepted | Coraine query_entities_errors |
| `17533938` | 4.10 | `maxDistance` was accepted with `within` | Coraine query_entities_geo_errors |
| `e699bf3f` | 5.13.2.4 | a stored @context's local identifier was a bare UUID, not a URI | Coraine jsonld_contexts_crud and 2 more |
| `c5b80741` | 5.5.12, 4.5.1 | Update Attributes stored a Null `scope`; a Null `type` was added to the Entity's types | Coraine entity_member_delete_and_null |
| `278231ff` | JSON-LD 1.1 9.5, 4.6.3 | a Property value object with a non-string `@type`, a foreign member or a DateTime `@value` that is no DateTime was stored | Coraine value_type_atvalue_validation, entity_attr_typed_value_object |
| `80ce647b` | 4.8 | the temporal representation showed sub-Attribute `createdAt`/`modifiedAt` without `options=sysAttrs` | Coraine troe_timescale_routes_single_entity and 2 more |
| `5ad2b5a7` | 5.6.14.4 | an instance PATCH stored the EntityTemporal Fragment itself as the value | Coraine troe_timescale_temporal_instance |
| `612a061b` | 4.5.2.2 | `valueType` read back as the expanded IRI | Coraine value_type_subattr |
| T-5495 | TS 104 175 7.2.5 | a scopeQ with a bare `;` conjunction or a level not starting with a letter was answered instead of refused (BadRequestData) | forge 019_03_07_05, 019_03_07_08 |

### 4.3 Where the two suites disagree

Antares follows the side the spec supports and lists the other. Where the spec does not decide, Antares keeps its behaviour.

| question | ETSI | Coraine | Antares, and why |
|---|---|---|---|
| a JSON body that is no object (`[]`, `[1,2,3]`) on Create Entity, Create CSR or POST Query | InvalidRequest (001_02_02, Create Entity) | BadRequestData | InvalidRequest on every path; the pinned suite gates CI and TS 104 175 5.5.4 does not single out an array |
| PATCH or PUT `/entities` without an id | 400 (054_02, 056_02) | 405 with `Allow` | 400, as ETSI asserts |
| an unquoted `q` value that is no Number, date or URI (`q=category==commercial`) | accepted (5743_01) | 400 | accepted; the 4.9 grammar refuses it, but the pinned suite relies on it |
| a CSR Subscription read back with a `jsonldContext` it was not given | absent (044/045) | not tested | absent; 5.8.1.4 initialises it for entity Subscriptions |

### 4.4 Coraine failures

Bucket counts for the vanilla binary on the file store (the reference cell); the other cells differ only by the cases in 3.2. `final-validation-coraine.md` lists every failing case with its first difference and clause.

| bucket | cases |
|---|---:|
| Antares bug | 45 |
| suites disagree | 5 |
| spec unclear | 86 |
| Antares per spec | 90 |
| Coraine-specific | 91 |
| harness | 18 |
| total | 335 |

Open Antares bugs. Filing tasks is frozen for this task (only GOAL-assistant files), so none has a task yet; T-3677 is blocked on that, and each row becomes one task in T-3446 `depends-on` once the freeze lifts.

| case | what is wrong | clause |
|---|---|---|
| append_scope_union_single, attr_single_element_array_reduction, context_remote_parkingspot_complex_payload, langprop_array_reduction, property_value_opaque_json, vocab_property | a single-element array is not reduced to its element on output | 4.5.0: "For the use with the API, the compacted JSON-LD representation is used"; JSON-LD compaction reduces [x] to x outside @list, @set and @json |
| array_reduction_entity_data | a single-element array is not reduced to its element on output (step 01, a JsonProperty json ["a"], answers 201 since T-5580: TS 104 175 5.2.6.4.10 allows any JSON value) | 4.5.0: "For the use with the API, the compacted JSON-LD representation is used"; JSON-LD compaction reduces [x] to x outside @list, @set and @json |
| context_embedded | @context inside a Property value accepted | 5.5.7: user @context shall not "Be embedded into NGSI-LD Attributes, i.e. there cannot be parts of the user @context other than at the top level"; "should result in an error of type BadRequestData" |
| context_volatile_hosting_local | application/json response compacted with inline @context but Link names the core context | 6.3.6 "If the target response's MIME type is "application/json" such response shall include a Link to the associated JSON-LD @context"; 5.13.1 ImplicitlyCreated: the broker hosts an ex-novo @context to serve it at a URL |
| csource-reg-distops-append-exclusive | append forwards non-matching Attributes to an exclusive source | 5.6.3.4 "If an exclusive or redirect Context Source Registration matches against the input data, the Attributes from matching input data are forwarded"; CSR claims speed only, brand reached CP1 |
| csource-reg-distops-create-207 | distributed Create Entity 207 body is not a BatchOperationResult | 6.4.3.1: 207 body is BatchOperationResult; 5.2.16 success "Array of valid URIs ... Entity IDs" (Antares puts "distributed operation ... returned 201"); 5.2.17 BatchEntityError entityId "Entity ID corresponding to the Entity in error" and registrationId missing |
| csource-reg-distops-create-409 | distributed Create Entity 207 body is not a BatchOperationResult | same as above: errors without entityId/registrationId |
| csource-reg-distops-create-excl-no-op | distributed Create Entity 207 body is not a BatchOperationResult | same as above (success "created locally" instead of urn:V1); error type InternalError with status 409 where 5.6.1.4 asks Conflict |
| csource-reg-distops-retrieve-upstream-error | source error answered as Warning 199 (timeout) and 404 | 6.3.17 Warning "299 - Miscellaneous Persistent Warning: An error response (such as 403 - Forbidden) was received from the registration endpoint"; Antares says 199 "no response was received"; 502 for the exclusive single source ("fails for any other reason") is the Coraine expectation |
| csource-reg-distops-updated-dedup | remote 207 UpdateResult not unfolded: Attributes a source updated reported as notUpdated | 5.2.18 UpdateResult updated: Attributes "successfully updated"; CSR:Color updated color (and speed) but Antares lists color, speed, paint under notUpdated with "returned 207" |
| csource_reg_delete_marker | raw JSON null in a CSR update fragment accepted | 5.9.3.4 "If the data types and restrictions expressed by clause 5.2.9 are not met by the Context Source Registration Fragment, then an error of type BadRequestData shall be raised" (description is a String); 4.5.0 "null cannot be used as a value in JSON-LD", NGSI-LD Null is "urn:ngsi-ld:null" |
| distop_delete_inclusive_source_404 | 404 from an inclusive source on delete turns 204 into 207 | 6.3.17 "a registration endpoint responding with no data and the HTTP status code 404 - Not Found should not be considered as abnormal behaviour for distributed operations" |
| distop_exclusive_priority_merge | inclusive source's newer speed beats the exclusive source's | 4.3.6.3 exclusive: "all of the registered context data is held in a single location external to the Context Broker"; Antares merges by 4.5.5.3 recency (most recent observedAt) instead of taking the exclusive source for the registered Attribute |
| distop_read_foreign_context | source answer not interpreted with the registration's jsonldContext: attribute dropped | 4.3.6.6 jsonldContext: the broker forwards "with this JSON-LD context"; the reply (velocity in ctxCP = veh/speed) comes back as default-context/velocity and is lost; step 05 shows type T unexpanded too |
| distop_retrieve_timeout_504 | single redirect/exclusive source timeout on GET answered 404 + NGSILD-Warning 199 instead of 504 | 6.3.17: "In the case of an exclusive or redirect registration, where all of the data is held ... in a single registered source, the following errors shall be returned: ... 504 Gateway Timeout - if the single registered source fails to respond in time" (no method restriction; its 404 bullet is a read case) |
| duplicate_id_type_alias_every_level | "id" and "@id" in one object accepted (Subscription entities[]) | core context maps id -> @id; JSON-LD 1.1 API expansion raises "colliding keywords"; 5.5.4: "not syntactically correct, according to the @context ... BadRequestData" (top level already refused) |
| entity_batch_upsert_distops_exclusive | distributed upsert success array lists an id twice (local part + forwarded part) | 5.6.8.4: S "shall contain a list of Entity IDs, one for each NGSI-LD Entity which was successfully processed" |
| entity_batch_upsert_distops_inclusive | success array lists each id twice (local + forwarded) | as entity_batch_upsert_distops_exclusive |
| entity_create_local_reg_conflict | ?local=true create stores an Attribute an exclusive/redirect CSR claims | 4.3.6.3: "For proxied registrations, the Context Broker itself is not permitted to hold context data about the registered Entities and Attributes locally" |
| entity_patch_geoproperty_validation | type-less merge/partial update stores an invalid geometry in a GeoProperty | 4.7 / 4.5.16.1 GeoProperty value is a GeoJSON geometry; the typed update (step 04) is refused, the type-less one (02, 03) stores {"type":"Polygon"} |
| entity_update_local_reg_conflict | ?local=true append stores an Attribute an exclusive/redirect CSR claims | 4.3.6.3 (as entity_create_local_reg_conflict) |
| expanded_core_terms_input | core member sent as full IRI (`https://uri.etsi.org/ngsi-ld/observedAt`) stored as a sub-Property | JSON-LD expansion maps observedAt and its full IRI to the same IRI; 5.5.7 expansion: the input is the same document as one using observedAt |
| jsonld_json_typed_is_verbatim | CSR member whose term the request @context types @json is not expanded to its IRI | 4.5.0: short terms "are expanded by the component implementing the NGSI-LD API using a JSON-LD @context" |
| linked_entities_objecttype_gate | join does not follow objectType-annotated links to registered (non-local) entities | 4.5.23.1: "Relationships annotated with an objectType whose object is an Internal Linked Entity are considered to be retrievable" |
| linked_entities_objecttype_split | joined entity misses its remote (registered) part | 4.5.23.1 Internal Linked Entity retrieval; same cause as linked_entities_objecttype_gate |
| multi_attribute_concise_simplified | concise multi-instance output not lossless: Antares cannot read back its own concise array (bare-value instance) | 4.5.2.3: "represented in a concise but lossless representation" |
| patch_entity_url_params | Merge Entity under ?format=simplified does not preserve the pre-existing Attribute type | 5.6.17.4: "If the Attribute ... to be merged is represented in a simplified representation, the type of any pre-existing Attribute in the target entity shall be preserved" |
| post_query_errors | non-object POST Query body gives InvalidRequest | 5.5.4: InvalidRequest only "If the request payload body is not a valid JSON document"; [1,2,3] is valid JSON not matching the data type -> BadRequestData |
| simplified_input_matrix | Merge Entity under ?format=simplified does not preserve the pre-existing Attribute type | 5.6.17.4: "the type of any pre-existing Attribute in the target entity shall be preserved" |
| subscription_periodic_other_tenant | periodic Subscription on a non-default tenant gets no initial notification (the default tenant does: subscription_periodic_filters passes) | TS 104 175 5.5.10: an operation applies to the specified Tenant; Antares' timeInterval behaviour differs by tenant |
| tenant_implicit_create | Batch Upsert on a nonexistent tenant creates it (201) | TS 104 175 5.5.10: implicit creation lists Create Entity, Batch Entity Creation, ... ; 'All other NGSI-LD operations ... that target a non-existing Tenant should raise an error of type NonexistentTenant.' |
| troe_entity_modifiedat_attr_update | temporal representation's entity modifiedAt stays at the create after attribute updates | TS 104 175 4.8: modifiedAt is when 'the Entity ... was last modified' |
| troe_timescale_multi_type | Add Attributes to Temporal Evolution ignores a new Entity Type in the fragment | TS 104 175 5.6.12.4: 'If type is included in the EntityTemporal Fragment and it includes Entity Type names that are not yet in the target ..., add them to the list of Entity Type names' |
| troe_timescale_routes_batch | a replace upsert removes attribute instances without a deletedAt tombstone in the temporal history | 4.5.2.3: the NGSI-LD Null is used "in temporal evolutions (for encoding a deleted Property)" |
| troe_timescale_subattrs_and_instance_delete | deleting the urn:ds:1 instance leaves no deletedAt tombstone in the temporal history | 4.5.2.3: the NGSI-LD Null is used "in temporal evolutions (for encoding a deleted Property)" |
| troe_timescale_temporal_deleted_kinds | a merge-patch NGSI-LD Null on a Relationship leaves no deletedAt tombstone in the temporal history | 4.5.2.3: the NGSI-LD Null is used "in temporal evolutions (for encoding a deleted Property)" |
| troe_timescale_temporal_distop_exclusive | temporal retrieval through an exclusive CSR still merges local history of the claimed attribute | TS 104 175 4.3.6.3: 'all registered context data is obtained from the external registered sources' |
| troe_timescale_temporal_distop_query_exclusive | temporal query through an exclusive CSR still merges local history of the claimed attribute | TS 104 175 4.3.6.3: 'all registered context data is obtained from the external registered sources' |
| troe_timescale_temporal_linked_projection | attrs=owner{object} on Retrieve Temporal Evolution answered 404 | TS 104 175 5.7.3.4: 'If projection attributes are present and indicate the use of Linked Entity retrieval, an error of type BadRequestData shall be raised.' |
| value_type_subattr | after 612a061b: valueType on a Relationship is accepted | 5.2.6 Relationship has no valueType member; 4.5.2.3 valueType belongs to a Property value |

## Related

- `final-validation-coraine.md`: every Coraine failure, case by case
- `docs/analysis/coraine-comparison.md` (T-3548)
- `docs/upstream/etsi-raises.md`
- `docs/analysis/surfaces-test-plan.md`, section 3 (the oracle)
