---
title: Audit of the Antares own test suite against the NGSI-LD spec
task: T-5496
---

# Audit of the Antares own test suite against the NGSI-LD spec

The Antares suite (`crates/antares-api/tests`, the `it` binary and the separate `[[test]]` binaries, `src/tests`) passed while the Coraine run (T-5494) and the forge-latest run (T-5495) found real defects. This page checks whether the suite pins wrong behaviour: which tests cover the routes of those defects, 42 status assert sites checked against a quoted clause, the vacuous asserts that were tightened, and the flaky tests of the last 50 CI runs.

## 1. Data sources

- Spec text: ETSI GS CIM 009 V1.9.1 as split per clause in `docs/spec/<chapter>/<clause>.md` (pages in the front matter), ETSI TS 104 175 V1.1.1 and TS 104 176 V1.1.1 as split in `/workspace/etsi-specs`. etsi.org/deliver answers 403 from the build sandbox, so these splits are the source; "CIM" below means CIM 009 V1.9.1, "TS 176" TS 104 176.
- Defect lists: `docs/conformance/final-validation-coraine.md` section 4.1 on branch `agent/worker-t5494/T-5494` (18 rows `Antares wrong (both versions)`), and the T-5495 task body (one Antares bug, scopeQ grammar, fixed in `d24fefce`).
- Tests: this branch, dev `ca9755ba` plus the T-5495 branch. The covering tests were found with `grep -rlE '<route or member>' crates/antares-api/tests crates/antares-api/src` per row (patterns in section 2).
- CI history: `gh run list -w fast.yml -L 50` and `-w ci.yml -L 50` with `--json attempt,conclusion` on 2026-10-24; for each run whose second attempt went green, `gh run view <id> --attempt 1 --log-failed`.
- ETSI TPs: the vendored `ngsi-ld-test-suite` (forge `5c398518`).

## 2. Defect rows and the tests covering their routes

None of the 19 defects had a test asserting the wrong status or body on the failing path. 16 had no test reaching that path at all; 3 had a test whose assert was too loose to see the defect. Each Coraine row is still owned by the fix task named in its row (T-5463 to T-5466); this audit adds no second fix.

| defect (source) | fix task | covering test | verdict |
|---|---|---|---|
| append_scope_union_single (Coraine) | T-5463 | none reads `options=keyValues` after a scope union (`grep 'keyValues.*scope'`: no hit) | gap |
| context_volatile_hosting_local (Coraine) | T-5463 | none: inline-@context Link is exercised only in `negotiate.rs` unit tests on an IRI context | gap |
| csource-reg-distops-append-exclusive (Coraine) | T-5464 | `csf_forwarding_5_7.rs` and `federation_loop.rs` forward appends but never check what the source received per attribute | gap |
| csource-reg-distops-create-207 (Coraine) | T-5464 | `federation_loop.rs:471 inclusive_partial_failure_is_207` asserts only that `success` is non-empty (line 499), so the "deleted locally" text Antares puts there passes | loose assert, noted in T-5464 |
| csource-reg-distops-create-409 (Coraine) | T-5464 | same test; asserts `errors[0].error.status` only, not `entityId` | loose assert, noted in T-5464 |
| csource-reg-distops-create-excl-no-op (Coraine) | T-5464 | `src/tests/validation.rs:84` asserts 207 only | loose assert |
| csource-reg-distops-retrieve-upstream-error (Coraine) | T-5464 | `federation_loop.rs:432 single_proxy_source_failure_surfaces_as_502` covers DELETE only; GET /entities/{id} has no test | gap |
| csource-reg-distops-updated-dedup (Coraine) | T-5464 | none (`grep 'notUpdated.*207'`: no hit) | gap |
| distop_retrieve_timeout_504 (Coraine) | T-5464 | `federation_loop.rs:452 single_proxy_source_timeout_is_504` covers DELETE only | gap |
| entity_patch_geoproperty_validation (Coraine) | T-5466 | none for an untyped GeoProperty fragment | gap |
| expanded_core_terms_input (Coraine) | T-5466 | none sends `https://uri.etsi.org/ngsi-ld/observedAt` | gap |
| linked_entities_objecttype_gate (Coraine) | T-5464 | `policy_filter_5_7.rs`, `policy_notify_5_8_6.rs` join only local targets | gap |
| patch_entity_url_params (Coraine) | T-5466 | none sends `format=simplified` on PATCH /entities | gap |
| simplified_input_matrix (Coraine) | T-5466 | same | gap |
| troe_entity_modifiedat_attr_update (Coraine) | T-5465 | `temporal_sub_attr_sys_4_8.rs` checks attribute sysAttrs, not the entity's `modifiedAt` | gap |
| troe_timescale_routes_batch (Coraine) | T-5465 | `temporal_matrix.rs` covers DELETE attr tombstones only | gap |
| troe_timescale_temporal_deleted_kinds (Coraine) | T-5465 | same; no merge-with-null tombstone test | gap |
| troe_timescale_temporal_distop_query_exclusive (Coraine) | T-5464 | `federation_loop.rs` temporal forwarding tests use inclusive registrations | gap |
| scopeQ grammar (forge 019_01_06, 019_03_07) | T-5495, fixed `d24fefce` | `scope_q_grammar_4_19.rs`, added by T-5495, red first | fixed |

## 3. Status asserts checked against the clause

42 assert sites across the route groups. "ok" means the asserted status is the one the clause names. File paths are under `crates/antares-api/`.

| # | test site | request | asserted | clause, page: quoted sentence | verdict |
|---|---|---|---|---|---|
| 1 | tests/no_overwrite_5_6_3.rs:68 | POST attrs?options=noOverwrite, attr exists | 207 | TS 176 7.4.3.1 p.48-49: "207 Multi-Status Only the Attributes included in the response payload body were successfully appended." | ok |
| 2 | tests/no_overwrite_5_6_3.rs:92 | same, second attribute | 207 | same | ok |
| 3 | tests/attr_path_name_5_6_4.rs:320 | PATCH attrs/speed | 204 | TS 176 7.5.3.2 p.52-53: "204 No Content The attribute was updated successfully." | ok |
| 4 | tests/csf_forwarding_5_7.rs:228 | GET entities?csf=((( | 400 | CIM 5.7.2.4 p.201-203: "If ... the query, geoquery or context source filter are not syntactically valid ... an error of type BadRequestData shall be raised." | ok |
| 5 | tests/csf_forwarding_5_7.rs:237 | POST entityOperations/query, bad csf | 400 | same | ok |
| 6 | tests/entity_maps_5_14.rs:118 | GET entityMaps?id=... only | 400 | CIM 5.6.21.4 p.194-195 (the too-wide rule entity maps reuse): "If none of the above is provided, then an error of type BadRequestData shall be raised (too wide query)." | ok |
| 7 | tests/set_attribute_value_10_2_6.rs:89 | PUT attrs/speed/value | 204 | TS 176 7.6.3.2 p.57-58: "204 No content The Attribute value was updated successfully." | ok |
| 8 | tests/temporal_projection_5_7_3.rs:140 | GET temporal/entities/{id}?pick=colour | 404 | CIM 5.7.3.4 p.205-206: "if the time series, for all the concerned Attributes of an Entity, does not include data corresponding to the temporal query interval, then such Entity shall be removed from S, thus it shall not appear in the final result set." | ok: an empty result for one id; TS 176 8.3.3.1 names no other status |
| 9 | tests/type_selector_context_5_6_6.rs:137 | DELETE entities/{id}?type=... | 204 | TS 176 7.3.3.4 p.47-48: "204 No Content" (Delete Entity) | ok |
| 10 | tests/dataset_none_4_5_5_1.rs:88 | DELETE attrs/{attr}?datasetId=@none | 204 | TS 176 7.5.3.4 p.54-55: "204 No Content" (Delete Attribute) | ok |
| 11 | tests/registration_listing_5_10_2.rs:382 | GET csourceRegistrations?pick=id&omit=endpoint | 400 | no clause: CIM 4.21 and TS 104 175 7.3.2 state no rule for pick with omit; ETSI TPs 018_18, 020_22, 028_08 refuse pick with attrs and one member in both | Antares stricter than spec; behaviour kept, false quote removed (`60bc12ff`), row for T-5497 |
| 12 | tests/contexts_5_13.rs:83 | POST jsonldContexts, wrong media type | 415, no body | CIM 6.3.4 p.270: "Unsupported Media Type, i.e. ..." in the list of errors that "shall result in just a ... HTTP status code (without any payload body)" | ok |
| 13 | tests/discovery.rs:213 | GET types/Nonexistent | 404 | TS 176 7.16.3.1 p.73-74: "404 Not Found It is used when a client provided an entity type not known to the system" | ok |
| 14 | tests/discovery_5_7_5.rs:107 | GET attributes, tenant tb | 200 | CIM 5.5.10 p.154-155: "If a Tenant is specified for an NGSI-LD operation, the operation shall only be applied to information related to the specified Tenant." | ok |
| 15 | tests/discovery_5_7_5.rs:119 | GET attributes/onlyInTa, tenant tb | 404 | TS 176 7.18.3.1 p.76-77: "404 Not Found It is used when a client provided an attribute name not known to the system" | ok |
| 16 | tests/entity_member_null_5_5_12.rs:62 | PATCH attrs {scope: urn:ngsi-ld:null} | 204 | TS 176 7.4.3.2 p.49-50: "204 No content All the Attributes were updated successfully." | ok |
| 17 | tests/no_change_no_notify_5_6.rs:121 | PATCH/PUT/DELETE an absent attribute | 404 | TS 104 175 10.2.7.4 p.177-178: "If the target Entity does not contain the target Attribute then an error of type ResourceNotFound shall be raised"; TS 176 7.5.3.2/7.5.3.3/7.5.3.4: "404 Not Found ... attribute name not known to the system" | ok (CIM 5.6.5.4's "not present" BadRequestData is the missing name parameter) |
| 18 | tests/notification_triggers_5_2_12.rs:142 | POST attrs | 204 | TS 176 7.4.3.1 p.48-49: "204 No content All the Attributes were appended successfully." | ok |
| 19 | tests/contexts_fleet_5_13.rs:157 | DELETE jsonldContexts/{id} | 204 | CIM 5.13.5.4 p.247-248 (delete succeeds on a known entry) | ok |
| 20 | tests/contexts_fleet_5_13.rs:159 | the same id after delete | 404 | CIM 5.13.5.4 p.247-248: "If the @context identifier does not correspond to any existing entry in the @context storage, then an error of type ResourceNotFound shall be raised." | ok |
| 21 | tests/policy_filter_5_7.rs:234 | GET entities/{id} hidden by policy | 404 | CIM 5.7.1.4 p.197-198: "If the NGSI-LD endpoint does not know about the target Entity ... an error of type ResourceNotFound shall be raised." Policy seam: a hidden Entity answers as absent | ok, security: same body as absent (line 256) |
| 22 | tests/policy_filter_by_id.rs:165 | GET a hidden and an absent id | 404, 404 | same | ok, security |
| 23 | tests/sub_attribute_null_5_6_4.rs:82 | PATCH attr with observedAt null | 204 | TS 176 7.5.3.2 p.52-53: "204 No Content The attribute was updated successfully." CIM 5.6.4.4 p.165: "NGSI-LD Nulls should be supported by this operation." | ok |
| 24 | tests/scope_q_grammar_4_19.rs:95 | GET entities?scopeQ=`<malformed>` | 400 | CIM 5.7.2.4 p.201-203, sentence of row 4, and TS 104 175 7.2.5 grammar | ok |
| 25 | tests/registration_expiry_5_9.rs:112 | GET an expired registration | 404 | CIM 5.10.1.4 p.231-232: "If the NGSI-LD endpoint does not know about the target context source registration ... an error of type ResourceNotFound shall be raised." | ok |
| 26 | tests/scope_merge_null_5_5_12.rs:72 | PATCH entities/{id} (merge), scope null | 204 | TS 176 7.3.3.2 p.44-46: "207 Multi-Status BatchOperationResult 204 No Content Possible non-error responses" | ok |
| 27 | tests/temporal_attr_writes_5_6_13.rs:95 | GET temporal of an entity never recorded | 404 | TS 176 8.3.3.1 p.81-83: "404 Not Found It is used when a client provided an Entity identifier (URI) not known to the system" | ok |
| 28 | tests/temporal_attr_writes_5_6_13.rs:116 | DELETE temporal/entities/{absent}, then {id} | 404, 204 | CIM 5.6.16.4 p.185-186: "If the NGSI-LD endpoint does not know about the target Entity ... ResourceNotFound shall be raised."; TS 176 8.3.3.2 p.83-84: "204 No Content" | ok |
| 29 | tests/subscription_status_5_2_12.rs:80 | PATCH subscription with conflicting members | 400 | TS 176 7.14.3.2 p.70-71: "400 Bad Request" for an incorrect request; CIM 5.8.2.4 p.221 BadRequestData | ok |
| 30 | tests/subscription_status_5_2_12.rs:88 | PATCH subscription, valid | 204 | TS 176 7.14.3.2 p.70-71: "204 No Content" | ok |
| 31 | tests/temporal_content_range_6_3_10.rs:121 | GET temporal/entities over the instance limit | 206 | CIM 6.3.10 p.274-276: "implementations shall use the Partial Content Response (206) as specified by IETF RFC 7233 [31], clause 4.1, if the implementation is not able to respond with the full representation at once." | ok |
| 32 | tests/temporal_instance_patch_5_6_14.rs:68 | PATCH temporal attrs/{attr}/{instanceId} | 204 | TS 176 8.6.3.1 p.87: "204 No Content" | ok |
| 33 | tests/tenant_header_6_3_14.rs:47 | two NGSILD-Tenant headers | 400 BadRequestData | CIM 6.3.14 p.277: the Tenant "is specified as the HTTP header "NGSILD-Tenant", whose value is the String identifying the Tenant" (one value); no clause names the error | Antares reading, kept: two values cannot name one Tenant |
| 34 | tests/scope_target_5_6_5.rs:57 | DELETE attrs/scope | 204 | TS 176 7.5.3.4 p.54-55: "204 No Content" | ok |
| 35 | tests/federation_loop.rs:425 | DELETE, single redirect source answers 404 | 404 | TS 176 6.3.5 p.26-27: "404 Not Found - if resources not found within the single registered source." | ok |
| 36 | tests/federation_loop.rs:444 | DELETE, single source answers 500 | 502 | TS 176 6.3.5 p.26-27: "502 Bad Gateway - if the single forwarded request fails for any other reason" | ok |
| 37 | tests/federation_loop.rs:463 | DELETE, single source silent | 504 | TS 176 6.3.5 p.26-27: "504 Gateway Timeout - if the single registered source fails to respond in time." | ok |
| 38 | tests/tenant_isolation.rs:76 | GET entity of tenant A as tenant B | 404 | CIM 5.5.10 p.154-155, sentence of row 14 | ok, security |
| 39 | src/tests/mod.rs:97 | TRACE, CONNECT, lower-case get | 501 | TS 176 6.3.2 p.25: ""Not Implemented" (501) which shall be raised when a client invokes an HTTP verb that is unrecognized or not implemented." | ok |
| 40 | src/tests/mod.rs:102 | GET entityOperations/create | 405 | TS 176 6.3.2 p.25: ""Method Not Allowed" (405) which shall be raised when a client invokes a recognized but unsupported HTTP verb over a resource." | ok |
| 41 | tests/api_surface.rs:272 | POST without Content-Length | 411, no body | CIM 6.3.4 p.270: ""Content-Length" HTTP header absence, shall result in just a 411 HTTP status code (without any payload body)." | ok |
| 42 | tests/policy_operation_members.rs:315 | batch, one member refused by policy | 207 | TS 176 7.8.3.1 p.60-61: "207 Multi-Status If only some or none of the entities have been successfully created or updated, a response body containing the result of each operation ... is returned in a BatchOperationResult" | ok |

Wrong asserts found: none. Two rows (11, 33) pin an Antares reading where the spec names no error; both refuse a request no clause defines, so neither hides data or weakens a check.

## 4. Vacuous asserts tightened

An assert of `is_success()` or of "success or 207" passes for the wrong 2xx code. Each one below now asserts the code the clause names. Every tightened assert passed on the current code, so no code fix followed (commit `aac38aaf`).

| test site | was | now | clause, page |
|---|---|---|---|
| tests/temporal_matrix.rs:329, 390 | `is_success()` on append | 204 | TS 176 7.4.3.1 p.48-49: "204 No content All the Attributes were appended successfully." |
| tests/temporal_matrix.rs:341, 352, 398, 504 | `is_success()` on PUT attrs/{attr} | 204 | TS 176 7.5.3.3 p.53-54: "204 No Content The attribute was replaced successfully." |
| tests/temporal_matrix.rs:494 | `is_success()` on append | 204 | TS 176 7.4.3.1 p.48-49 |
| tests/federation_loop.rs:876 | success or 207 on batch create, every forward 201 | 201 | TS 176 7.7.3.1 p.58-59: "If all entities have been successfully created, an array of Strings containing URIs is returned" (201 Created) |
| tests/policy_pre_image.rs:307 | success or 207 for upsert and delete | upsert 201, delete 207 | TS 176 7.8.3.1 p.60-61: "201 Created If all entities not existing prior to this request have been successfully created and the others have been successfully updated"; 7.11.3.1 p.64-65: "207 Multi-Status If some or all of the entities have not been successfully deleted, or did not exist" |
| tests/csi_credential_4_3_6_5.rs:233 | success or any 5xx on an inclusive query with two failing sources | 200 | TS 176 6.3.5 p.26-27: ""NGSILD-Warning" HTTP headers shall also be used to indicate instances of abnormal behaviour for distributed HTTP GET and HEAD operations" (the 502/504 rules apply only to a single exclusive or redirect source) |
| tests/scope_q_grammar_4_19.rs:126 | `is_success()` on POST query and subscription | 200 query, 201 subscription | TS 176 7.12.3.1 p.65-67: "Entity[] 200 OK"; 7.13.3.2 p.68-69: "201 Created" |
| tests/policy_filter_by_id.rs:243 | `is_success()` on POST temporal/entities | 201 | TS 176 8.2.3.2 p.80-81: "201 Created Upon creation success" |

Left loose on purpose:

- `federation_loop.rs:499` (`success` non-empty in a 207): the exact body (Entity ids in `success`, `entityId` in `errors`) is the Coraine defect csource-reg-distops-create-207; tightening it is the red-first test of T-5464.
- `security_regression.rs:120`, `entity_maps_5_14.rs:636`, `entity_map_store_failure_5_14.rs:190`: each pins "refused, not created" for a store or fetch failure whose status the clause leaves to the implementation; the test names that in its comment.
- `is_success()` on seed requests (`temporal_attr_writes_5_6_13.rs:70` and 22 others): setup, not the operation under test.

## 5. Flaky tests

Last 50 runs of `fast.yml` and `ci.yml` (2026-10-19 to 2026-10-24): four runs went green on a second attempt or failed one test that passes alone. Each one has a task.

| test | run (attempt 1) | cause | task |
|---|---|---|---|
| antares-ws `ws_queue::ws_coalescing_keeps_the_latest_single_entity_notification` | 39863214833 | five writes not coalesced under runner load | T-5245 (review) |
| antares-ws `ws_queue::ws_show_changes_is_never_coalesced` | 39812037884 | "no message within the wait" under load | T-5245 (review) |
| antares-api `types_list_pg::a_views_type_list_over_100k_entities_answers_in_under_200_ms` | 39226008940 | wall-clock budget on a shared runner | T-4736 (done: asserts the plan, the 200 ms moved to a bench) |
| antares-sql `pg_explain::a_type_page_reads_its_type_through_an_index_under_rls` | 39480294832 | planner chose a bitmap scan on skewed stats | T-4916 (done: `enable_bitmapscan = off`) |

## 6. Commits

- `aac38aaf` tightens the asserts of section 4.
- `60bc12ff` removes the 4.21 quote that CIM 009 V1.9.1 does not contain from `repr.rs`, `csource.rs` and `temporal.rs` (row 11); the refusal itself is unchanged.

## Related

- `docs/conformance/final-validation.md`, `docs/conformance/final-validation-coraine.md`
- `docs/conformance/suite-edits.md`
- Tasks T-5463 to T-5466 (Coraine fixes), T-5495 (forge triage), T-5497 (behaviour profiles)
