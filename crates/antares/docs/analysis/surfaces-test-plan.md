---
title: Test and security plan for the Antares surfaces
task: T-3444
---

# Test and security plan for the surfaces

What every surface crate of ADR-0027 ships with, which tool runs it and which CI job fails when it
breaks. T-3445 turns each section into tasks; a task cites the section and copies the test names.
Sources: ADR-0023 (views), ADR-0025 (internal context sources), ADR-0026 (WebSocket), ADR-0027
(architecture), `operation-matrix.yaml` (T-3437: 69 operation rows, 30 OData-only rows),
`surfaces-threat-model.md` (T-3447: 112 STRIDE rows, module rules M1 to M4),
`surfaces-conformance.md` (T-3449). Antares at `e87ea39`. A test marked "exists" is a `fn` under
`crates/` today; every other name is the name the implementing task must use.

CI jobs named below (all in Antares `.github/workflows/`):

| job | workflow | when | blocks merge |
|---|---|---|---|
| `workspace-tests` | `workspace.yml` (called by `ci.yml` and `full.yml`) | every pull request into `dev` | yes |
| `features` | `workspace.yml` (T-3465 creates it) | every pull request | yes |
| `coverage-surfaces` | `workspace.yml`, new | every pull request touching a new crate | yes |
| `fuzz` | `fuzz.yml` (exists, `cargo +nightly fuzz run <target> -- -max_total_time=300`) | nightly | files an issue |
| `fuzz-smoke` | `workspace.yml`, new: each target 30 s | every pull request touching a parser crate | yes |
| `miri` | `strict.yml` (exists, `cargo +nightly miri test`) | daily | files an issue |
| `etsi-*`, `mcp-*`, `odata-*`, `linkml-*`, `ws-*` | `surfaces-conformance.yml`, new (`surfaces-conformance.md` section 7) | per that table | the merge-request subset |
| `bench-surfaces` | `perf-weekly.yml` + criterion in `workspace.yml` nightly | nightly / weekly | after T-3467's noise profile (ADR-0027 section 6) |

## 1. Coverage gate

- Tool: `cargo llvm-cov` (already installed by `etsi-coverage.yml` through `taiki-e/install-action`,
  tool `cargo-llvm-cov`), nightly toolchain for branch coverage:
  `cargo +nightly llvm-cov --branch -p <crate> --all-features --json --summary-only`.
- Scope: every new crate of ADR-0027 section 1 (`antares-ops`, the six surface crates, the six
  protocol crates), measured by its own tests plus the broker integration tests that run with its
  feature (`cargo +nightly llvm-cov --branch -p antares-broker --features <f> --tests` merged with
  `cargo llvm-cov report`).
- Target: 100 % of lines and 100 % of branches. Every line or branch below that is listed in
  `ci/coverage-exclusions.toml` with `crate`, `file`, `function`, the uncovered count and a reason
  (for example "unreachable after a checked conversion the type system cannot see"). The job
  computes `required = 100 % − listed / total` per crate and fails when the measured value is
  below it, or when a listed entry is covered now (the list cannot rot, as
  `check_expected_failures.py` does for ETSI).
- Script: `dev/coverage-gate.py` (new), self-test `--self-test` with one synthetic report per red
  condition.
- Job: `coverage-surfaces`, blocking. Existing crates keep T-3468's 95 % floor; this gate does not
  apply to them.

## 2. Operation-matrix test

- Crate and file: `crates/antares-broker/tests/operation_matrix.rs`, `--features full`.
- Input: `docs/analysis/operation-matrix.yaml` read at test time (`serde_yaml` is not a dependency
  today; the test reads it through `serde-saphyr` 1.3.0, the dev-dependency threat model 11.2
  already lists).
- Tests:
  - `every_route_of_every_surface_is_a_matrix_cell`: walks the router of each surface (the
    route walk of `policy_gate_seam.rs::every_route_asks_the_policy_engine_exactly_once`, exists)
    and asserts each method + path is the `binding` of a cell, a row of
    `odata_without_ngsi_ld_operation`, or a `non_operation_routes` entry. A new route without a
    row is red.
  - `every_cell_is_reachable_or_rejected_as_stated`: for each of the 69 rows and each surface
    column, sends the `binding` request against a seeded broker and asserts the stated outcome:
    `supported` reaches the handler (the counting engine of `policy_gate_seam.rs` sees exactly one
    decision with the row's `gate_clause`), `refused`/`not_applicable` answers the status the cell
    names before any gate (count 0).
  - both tests run twice: under view `all` (every type, every attribute, every operation) and under
    view `restricted` (one type, one hidden attribute, read rows only), where a write cell must
    answer ADR-0023 rule 4's 405/422 and a read cell must not return the hidden attribute.
  - `the_30_odata_only_rows_answer_501_before_any_gate` (the matrix 7.1 rows; threat model O13,
    O19).
- Job: `workspace-tests` (the file runs in the all-features step `dev/unit-junit.sh
  all-features`).

## 3. Equivalence across surfaces

- File: `crates/antares-broker/tests/surfaces_equivalence.rs`, `--features full`.
- Corpus: five views (`all`; `types-only` selecting two types; `hidden-attr` hiding one attribute;
  `q-narrowed` with a numeric `q`; `scope-geo` with `scopeQ` and `geoQ`) over a fixed seed of
  200 Entities in three types with Relationships between them (`tests/fixtures/equivalence.jsonld`,
  new).
- Test `the_same_read_returns_the_same_entities_on_every_surface`: for each view and each read
  shape (by id, by type, by `q`, paged with `limit`/`offset`, `count`), the set of Entity ids and
  the attribute names and values are equal across:
  - NGSI-LD `/x/views/{v}/ngsi-ld/v1/entities` (normalized form),
  - OData `/x/views/{v}/odata/v4/{EntitySet}` (after the JSON Format to NGSI-LD mapping of
    `odata-linkml-mapping.md`),
  - MCP `tools/call` of the matching read tool (`mcp-surface.md` tool table),
  - WebSocket: a Subscription on the view receives the Entities of an upsert of the corpus, and the
    notified Entities equal the NGSI-LD read after the same upsert.
- Test `count_and_paging_agree_on_every_surface`: `NGSILD-Results-Count`, `@odata.count` and the
  MCP result count are equal; paging through each surface yields the same ids in the same order.
- Job: `workspace-tests`.

## 3a. View and policy matrix (T-3777)

- File: `crates/antares-broker/tests/views_matrix.rs`, `--features full,jc-policy`; corpus
  `crates/antares-broker/tests/views-matrix/`: one JSON file per scenario (a view, the
  joinedcontext engine's policies, a caller, optional `steps`), a shared seed of 11 Entities
  (`_seed.json`: three polygons, an edge point, scopes with several paths and none, an Attribute on
  some Entities only, a multi-attribute, a LanguageProperty) and its LinkML schema (`_schema.json`).
- Each scenario's `expect` (Entity id to Attribute names, or `denied`) is written by hand from the
  seed, never read from the code under test.
- Test `every_surface_answers_what_ngsi_ld_admits_in_every_scenario`: each scenario in a Tenant of
  its own, read page by page through the NGSI-LD view (Link `next`), OData (`odata.maxpagesize`,
  `@odata.nextLink`), OGC items (`next` links), MCP `query_entities` (cursor) and the NGSI-LD
  temporal query (11.3.3 over `modifiedAt`), and notified over a WebSocket and an HTTP Subscription made
  through the view as the caller before the seed is written (the broker wired with the engine,
  as `main` wires it, so notifications are decided by it). Each must answer
  exactly the expected ids and Attribute names, its count (`NGSILD-Results-Count`, `@odata.count`,
  `numberMatched`, MCP `total`) the expected size. A retrieve by id (NGSI-LD and MCP `get_entity`)
  of a seed Entity the scenario does not admit answers the bytes of an absent id; a denied caller
  gets a refusal naming no seed Entity.
- The temporal answer is the snapshot's less the Entities shown with their frame alone, unless the
  scenario states `temporal`: a policy condition holds per instance (ADR-0023 rule 11), so a
  `scopeQ` condition (scope has no history) hides every instance and a moved Entity keeps the
  instances recorded while its rule held.
- Query-language rows, each against the NGSI-LD query that means the same through the same view
  and caller: OData `$filter=speed gt 40&$select=location,speed` against `q=speed>40` with `pick`,
  and OGC `bbox=0,0,7,7&properties=speed` against a `geoQ` intersects with `pick` plus the
  Vehicles without a shown `location` (`bbox` keeps them, Part 1 Req 24 C). A surface may refuse a
  name the view hides with `400` where NGSI-LD omits it. Every pager stops after 50 pages, so a
  next link without end is a mismatch, not a hang.
- LinkML (a build with `linkml`, as CI's all-features): `classes/{class}` of each type through the
  view as the caller, each kept class with its slot names. By default each admitted type with the
  names its admitted Entities show, and none for a denied caller; a scenario states `linkml` where a
  rule's condition drops the Entities but not the class (linkml.md 2).
- A mismatch with a fix task filed is listed in the scenario's `known` (surface to task id) and
  must still fail, so the fix removes the entry; the test prints the list.
- Job: `workspace-tests` (all-features).
- Test `every_surface_answers_what_ngsi_ld_admits_for_generated_views_and_policies`, the
  differential: up to eight generated Vehicles and Sensors (each Attribute, a point on a 0..=12
  grid and a scope present or not), a generated view (`*`, or one or both types with a
  projection, and a `q`, an area `within` or `intersects` and a `scopeQ`, each or not) and none
  or up to three generated policies for the caller (a type or both, `propertyNames` or the whole
  Entity, the same conditions, one in four a prohibition). OData, OGC and MCP must answer what
  NGSI-LD answers through the same view. All cases share one Tenant, the seed deleted after each.
  `VIEWS_DIFF_CASES` sets the cases (200 in `fast`, 2 000 in ci's `views-differential` job, which
  `publish-dev` waits for), `VIEWS_DIFF_SEED` the seed (3777, printed). A failure shrinks and
  prints the minimal case as a corpus file with its own `seed`: commit it with its `expect`
  written by hand and file the fix task. Mismatches with a task filed are listed in `KNOWN_DIFF`
  (none since T-3784).

## 4. Property tests (`proptest` 1.11.0, already a workspace dependency: `Cargo.lock:2329`, used by `antares-sql`)

| property | crate and file | generator |
|---|---|---|
| SQL compilation and in-memory evaluation of the same AST agree | `antares-sql/tests/ql_props.rs` (T-3462, in review) | random `QNode` over a generated Entity set; compare `antares_ql::eval` with the rows the SQLite/Postgres compiled query returns |
| view ∧ consumer filter never widens | `antares-ops/tests/views_props.rs::the_conjunction_never_widens` | random view predicate V and request `q` Q over a random Entity set E: `select(V ∧ Q, E) ⊆ select(V, E)` and `⊆ select(Q, E)` |
| a hidden attribute behaves as absent | `antares-ops/tests/views_props.rs::q_over_hidden_equals_q_over_absent` (threat model V1) | random `q` naming hidden and visible attributes: the selection over E equals the selection over E with the hidden attributes deleted |
| projection never shows a hidden attribute | `antares-ops/tests/views_props.rs::projection_never_shows_a_hidden_attribute` | random Entity (sub-attributes, datasetIds, `join` targets, `previousValue`): no hidden name in the rendered output in any format (normalized, concise, keyValues) |
| OData `$filter` translation agrees with the reference evaluator | `antares-odata/tests/filter_props.rs::odata_filter_translation_agrees_with_reference_eval` (threat model O2) | random `$filter` within the mapped subset |
| LinkML emitted YAML parses back to the same model | `linkml-model/tests/props.rs::emitted_yaml_parses_back_to_the_same_model` (LK3) | random model |
| CSDL XML and JSON describe one model | `odata-csdl/tests/props.rs::xml_and_json_describe_one_model` (O17) | random EDM model |

Job: `workspace-tests` (default 256 cases; nightly `PROPTEST_CASES=10000` in `strict.yml`).

## 5. Fuzz targets (`cargo-fuzz`, `fuzz/fuzz_targets/`)

Existing: `expand_entity`, `geo_params`, `parse_q`, `q_to_sql`, `scope_q`. New, one per parser of a
new crate (threat model 10.2):

| target | parser | crate |
|---|---|---|
| `odata_url_parse` | resource path and system query options incl. `$filter`, `$expand`, `$orderby` | `odata-url` |
| `odata_batch_multipart`, `odata_batch_json` | `$batch` bodies | `odata-batch` |
| `odata_json_body` | entity bodies, `@odata.bind` | `odata-json` |
| `odata_csdl_read` | CSDL XML/JSON reader (only if reading is kept, `odata-library.md` 3.2) | `odata-csdl` |
| `mcp_tool_arguments`, `mcp_cursor` | tool input, cursor decoder | `antares-mcp` |
| `ws_client_message` | WebSocket client envelope | `antares-ws` |
| `view_config_load` | view definition | `antares-ops` |
| `linkml_read` | LinkML JSON at load (LK4) | `linkml-model` |

Each target asserts "no panic, no allocation past the `Limits` passed, error or value"; seeds from
the conformance pins (ABNF test cases for `odata_url_parse`). Jobs: `fuzz` (nightly, 300 s each,
exists) and `fuzz-smoke` (30 s per touched target, blocking).

## 6. Security suite

Each case is one named test. Files under the crate that owns the behaviour; the threat-model id is
the row whose mitigation the test proves.

| case | test(s) | crate | threat |
|---|---|---|---|
| inference via `q` on hidden attributes | `views_a_hidden_attribute_does_not_exist_for_q`, `views_q.rs::a_hidden_term_never_reaches_the_store`, `views_props.rs::q_over_hidden_equals_q_over_absent` | `antares-views`, `antares-ops` | V1, V2 |
| inference via `$filter` | `odata_filter_on_hidden_attribute_equals_filter_on_unknown_name`, `odata_bisecting_a_hidden_value_learns_nothing` | `antares-odata` | O1 |
| count and paging leaks | `views_count_and_paging_describe_the_narrowed_set`, `odata_count_is_the_narrowed_count`, `odata_view.rs::odata_a_skiptoken_of_another_view_is_400` | `antares-views`, `antares-odata` | V3, O3, O4 |
| ordering leak | `views_q.rs::order_by_a_hidden_attribute_orders_as_absent`, `odata_orderby_hidden_is_400_like_unknown` | | V4, O1 |
| `/types` and discovery leaks | `views_discovery_is_computed_from_the_view`, `metadata_never_names_a_hidden_type_or_attribute`, `mcp_discovery.rs::input_schemas_list_only_visible_names`, `linkml_view.rs::linkml_prefixes_and_context_hold_only_visible_terms` | views, odata, mcp, linkml | V7, O17, MC2, LK1, LK6 |
| 404 vs 403 | `views_an_out_of_view_entity_is_not_found` (404 for out of view, equal bytes to absent), `views_writes.rs::a_create_of_an_out_of_view_id_reveals_only_that_the_id_is_taken` (409), engine `Deny` = 403 (`policy_gate_seam.rs`, exists) | `antares-views` | V5, V6 |
| `$expand` / join to out-of-view | `views_join_omits_out_of_view_targets_and_keeps_the_relationship`, `odata_expand.rs::odata_expand_omits_out_of_view_targets`, `mcp_gate.rs::join_is_not_an_argument_until_the_target_is_checked`, `views_q.rs::a_linked_q_path_does_not_cross_into_an_out_of_view_entity` | views, odata, mcp | V8, V9, O5, MC16 (T-3470, T-3474) |
| relationship write / deep insert / `$ref` to an invisible or read-only target | `views_writes.rs::a_relationship_target_must_be_visible_and_absent_answers_alike`, `odata_bind_to_hidden_or_missing_target_is_400_and_writes_nothing`, `odata_a_reference_to_a_hidden_or_missing_entity_is_400_and_writes_nothing`, `odata_ref.rs::odata_bind_to_an_absent_target_answers_like_a_hidden_one`, `odata_deep_insert.rs::odata_deep_insert_is_501` | views, odata | V13, O14, O15 |
| write laundering (post-image leaves view) | `views_writes_check_pre_and_post_image_and_the_allowlist`, `views_writes.rs::a_replace_is_refused_when_the_stored_entity_holds_hidden_attributes`, `views_batch_is_decided_per_entity`, `odata_batch_members_are_gated_one_by_one` | views, odata | V10, V11, V12, O12 |
| hidden-only change suppression in notifications | `views_notifications_drop_hidden_changes_and_strip_previous_value`, `mcp_listen.rs::a_change_to_a_hidden_attribute_sends_no_resource_update`, `ws_notifications_pass_pre_notify_under_the_subscriber` | views, mcp, ws | V14, V15, MC17, W5 |
| cross-tenant via internal CSR | `icsr_grant.rs::icsr_a_registration_into_a_tenant_that_granted_nothing_is_refused`, `icsr_gate.rs::icsr_cross_tenant_read_without_an_engine_grant_is_refused`, `icsr_gate.rs::icsr_the_subject_is_the_callers_and_never_comes_from_the_registration`, `icsr_grant.rs::icsr_a_revoked_grant_stops_forwards_and_notifications` | `antares-icsr` | C1, C2, C3, C7 |
| WS cross-connection delivery | `ws_a_subscription_is_delivered_only_to_the_connection_that_created_it`, `ws_an_http_create_naming_a_ws_endpoint_is_refused`, `ws_a_tenant_mismatch_is_never_written` | `antares-ws` | W6 |
| slow consumer | `ws_a_slow_consumer_is_disconnected_not_buffered`, `mcp_listen.rs::listen_streams_are_capped_per_subject_tenant_and_process` | ws, mcp | W4, MC7 |
| token expiry | `ws_a_connection_ends_at_its_max_lifetime`, `ws_resume_with_another_subject_is_not_found` | `antares-ws` | W7 |
| tenant switch and snapshots on a view path | `views_pin_their_tenant`, `views_tenant.rs::a_snapshot_read_through_a_view_is_narrowed`, `odata_batch.rs::odata_batch_part_headers_cannot_change_tenant_or_subject`, `mcp_gate.rs::a_tenant_or_subject_in_the_arguments_is_refused_and_changes_nothing` | views, odata, mcp | V17, V21, O9, MC15 |
| error bodies | `views_errors.rs::views_errors_carry_no_hidden_name_and_no_inner_detail`, `odata_errors.rs::odata_errors_carry_no_hidden_name_and_no_inner_detail`, `mcp_errors.rs::errors_carry_no_hidden_name_and_no_inner_detail`, `linkml_view.rs::linkml_errors_carry_no_hidden_name`, `ops_errors.rs::an_inner_5xx_detail_never_reaches_the_surface` | each | G9, O20, MC18, LK8 |

Job: `workspace-tests` (each surface crate's tests, and the broker tests under `--all-features`).

## 7. ETSI

From `surfaces-conformance.md` section 2, unchanged here:

| run | binary | root | expected | job |
|---|---|---|---|---|
| E1 | vanilla | `/ngsi-ld/v1` | the committed per-TP baseline of T-3466 | `etsi-vanilla` (existing matrix cells) |
| E2 | full, view `all` | `/x/views/all/ngsi-ld/v1` | E1, TP by TP | `etsi-full-all` |
| E3 | full | `/ngsi-ld/v1` | E1, TP by TP | `etsi-full-direct` |
| E4 | full, view `etsi-restricted` | `/x/views/etsi-restricted/ngsi-ld/v1` | E1 except the generated `expected_view_refusals.json` | `etsi-full-restricted` |
| IOP | vanilla, full | five brokers (`dev/run-five.sh`) | equal | `etsi-iop` |

TS 104 188 interoperability tests are the `IOP_TP` tree (57 robot files); the internal-CSR
equivalence of the 105 DistributedOperations TPs is ADR-0025's plan
(`crates/antares-icsr/tests/icsr_equivalence.rs`).

## 8. Threat-model tests and the rule jobs

- Every STRIDE row of `surfaces-threat-model.md` sections 2 to 8 (G1 to G10, V1 to V27, O1 to O27,
  MC1 to MC20, LK1 to LK8, W1 to W11, C1 to C10: 112 rows) names its test in its own row; the
  implementing task of each crate copies those names. A check keeps them true:
  `dev/threat-tests.py` (new) parses the test column of every row and fails when a named test
  does not exist as a `fn` under `crates/` (or a fuzz target / script under its path) once the
  crate that owns it exists. Job: `workspace-tests`, step "Threat-model tests exist".
- Rule jobs (threat model section 9, ADR-0027 section 2): "Module rules"
  (`dev/check-module-rules.py --self-test` then the check), "No addon in the shipped build"
  (`dev/check-no-addon.sh`, extended), "Vanilla binary links no surface"
  (`dev/check-vanilla-symbols.sh`), the shelf test
  `api_surface_tests::the_vanilla_shelf_is_admin_alone`, `surfaces_fail_closed.rs` (M4), the vanilla
  tree diff (`ci/vanilla-tree.txt`).
- Lint job: `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` and the
  per-crate `[lints]` presence check in `dev/check-module-rules.py`.
- Miri: `strict.yml` `miri` gains `-p odata-url -p odata-csdl -p odata-json -p odata-batch -p
  linkml-model`.
- Supply chain (threat model 11.1): `cargo deny check licenses advisories bans sources`,
  `cargo audit --deny warnings` and `cargo audit bin`, `cargo machete`, two CycloneDX SBOMs with
  the vanilla one grepped for surface crates. All steps in `workspace.yml`; `cargo audit bin` in
  `build-image.yml`.

## 9. External conformance suites

`surfaces-conformance.md` (T-3449) is this section: ETSI (section 7 above), MCP (official
`@modelcontextprotocol/conformance@0.1.16`, `--requirements 2026-07-28` and `2025-11-25`, plus the
three retargeted suites), OData (`tests/odata/`: ABNF test cases live, CSDL XML/JSON validation,
.NET and SAP clients, `$filter`/`q` parity, Capabilities, linked-entity checks), LinkML
(`linkml==1.12.0`, `pyshacl==0.40.1`), WebSocket (`tests/ws/`, AsyncAPI validation), internal CSR
(equivalence). Jobs and binaries: its section 7 table.

## 10. Benchmarks

Budgets: ADR-0027 section 6.

| bench | tool | crate / script | job |
|---|---|---|---|
| view conjunction and projection cost per request | criterion | `antares-ops/benches/views.rs` | `bench-surfaces` (nightly) |
| `$filter` parse + translate, JSON Format render per 100 Entities | criterion | `odata-url/benches/parse.rs`, `antares-odata/benches/render.rs` | nightly |
| MCP tool call framing per 100 Entities | criterion | `antares-mcp/benches/call.rs` | nightly |
| LinkML document build for a 100-class schema | criterion | `antares-linkml/benches/schema.rs` | nightly |
| WebSocket fan-out, memory per idle connection | criterion + `dev/perf/rss.sh` | `antares-ws/benches/fanout.rs` | weekly |
| internal CSR forward vs HTTP forward | criterion | `antares-icsr/benches/forward.rs` | nightly |
| end to end: shapes `view`, `odata`, `mcp` beside `entities` and `entity` | k6, `dev/perf/shapes.sh` | `dev/perf/k6-shapes.js` | `perf-weekly.yml` |
| binary size per feature | `ls -l` of the stripped release binary | feature-matrix step | `features` |

A result past its budget fails the job once T-3467's noise profile exists (ten runs on the pinned
instance type, `docs/src/performance.md`); before that the job reports the number in its summary.

## Related

- [surfaces-conformance.md](surfaces-conformance.md), [surfaces-threat-model.md](surfaces-threat-model.md),
  [operation-matrix.md](operation-matrix.md), [operation-matrix.yaml](operation-matrix.yaml),
  [ts104175-delta.md](ts104175-delta.md)
- ADR-0023, ADR-0025, ADR-0026, ADR-0027 in `docs/adr/`
