# Context gateway to broker map (T-3436)

Sources: joinedcontext-platform `59a8f70` (`crates/context-gateway`, `crates/jc-core`), Antares-NGSI-LD-Context-Broker `7185e1a` (ADR-0014, ADR-0020, `crates/antares-api/src/policy.rs`, `docs/analysis/cim009-to-ts104175.md`), joinedcontext-portal `bd3a68a8` (`src/`, `sdk/`, `ui/`, `apps/`), joinedcontext-deployment `09fa18b` (`components/`, `charts/`), docs `4238cf0`. Code graphs: platform graph built `2026-10-10T11:31:20Z @ 59a8f705b9`, Antares graph built from `7185e1a`. Every `file:line` below was checked with `grep -n`/`sed -n` against those commits. Paths are relative to the repository named in the row (platform paths start with `crates/context-gateway/` unless another crate is named; that prefix is dropped as `src/...` and `tests/...` in the tables).

The CIM 009 clause numbers in `policy::FILTERABLE`, `policy::WHOLE_TENANT` and `Operation.clause` are translated to TS 104 175 V1.1.1 with `docs/analysis/cim009-to-ts104175.md`; both numbers are given where a clause decides something.

## 1. Method and reading guide

1. Routes come from the one router, `router()` at `src/app.rs:411`. Every `.route(...)` is listed in section 2 with its line. The router nests nothing (`grep -n '\.nest(\|\.merge('` finds no call in `src/`); three handlers own a resource tree of their own behind a `{*rest}` or `any(...)`: the NGSI-LD tree (`src/operations.rs:16`, table 2.2), OGC API Features (`src/handlers/ogc.rs:32`) and SensorThings (`src/handlers/sta.rs:33`). The MCP endpoint façade has its own tool table (2.3).
2. "PDP" names the functions of `src/pdp/` the handler reaches (cg, `CALLS|DISPATCHES` up to 6 hops). `Pdp::decide` is the per-request verdict (`src/pdp/mod.rs:26`, implemented by `PolicyPdp::decide`, `src/pdp/mod.rs:40`); `evaluator::effective` (`src/pdp/evaluator.rs:466`) is the policy-set view used by documents that describe grants rather than answer a request.
3. "Broker call" names the call site of `proxy::Broker::send` (`src/proxy.rs:157`), the only function that talks to the broker (cg: all 9 non-test callers are listed in the query result under table 2.1).
4. "Tests": every integration test reaches a handler only through `router()` and an HTTP request (`tower::ServiceExt::oneshot`), so the graph has no edge from a test to one handler: the query "tests reaching the handler within 3 hops, not through `router`" returns 0 rows for every handler. Tests are therefore named from the route literal: `git grep` of the path in `tests/*.rs`, then the enclosing `#[tokio::test]` function. A route with no literal in any test is "untested".
5. Decisions (section 4) use two words. **MOVES**: the decision goes into the broker, as a policy-engine addon behind the ADR-0020 seam or as a surface crate behind a cargo feature (GOAL point 2), and the platform code is deleted afterwards (GOAL point 11). **STAYS**: the gateway keeps it. A third outcome, **STAYS, re-based**, is a representation the GOAL does not list as a broker feature (files, OGC, SensorThings): the translator stays in the gateway, but it stops deciding and reads the broker's narrowed endpoint view instead; its PDP calls are what moves and is deleted.

```cypher
// handler symbols and their lines (cg lines include the doc comment; fn lines checked with grep -n)
MATCH (s:Symbol) WHERE s.package='context-gateway' AND s.is_test=false
  AND s.kind IN ['Function','Method','StaticMethod']
  AND s.name IN ['endpoint_record','ngsi_ld_entry','ngsi_ld','access','mcp_message','message',
    'protected_resource','access_check','preview','deliver','file_geojson','file_json','file_csv',
    'file_xlsx','file_zip','ogc_features','sensorthings','catalog_feed_jsonld','catalog_feed_turtle',
    'space_catalog','space_record','space_ngsi_ld','space_mcp_message','space_schema_index','space_dump',
    'space_schema_artifact','schema_index','openapi','schema_artifact','ok','metrics','missing',
    'enforce','scrub','record']
RETURN s.path, s.file, s.line, s.end_line ORDER BY s.file, s.line;

// what each handler reaches: the verdict, the policy-set view, the broker, the pipeline
MATCH (h:Symbol)-[:CALLS|DISPATCHES*1..6]->(f:Symbol)
WHERE h.package='context-gateway' AND h.is_test=false AND h.kind='Function'
  AND f.path IN ['context_gateway::pdp::Pdp::decide','context_gateway::pdp::evaluator::effective',
    'context_gateway::proxy::Broker::send','context_gateway::app::serve_ngsi_ld',
    'context_gateway::app::authenticate','context_gateway::middleware::tenancy::pin_tenant',
    'context_gateway::middleware::tenancy::strip_client_headers',
    'context_gateway::pdp::projection::project_by_type','context_gateway::app::project_answer']
RETURN h.path, collect(DISTINCT f.name) ORDER BY h.path;

// every call site of the broker
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol {path:'context_gateway::proxy::Broker::send'})
RETURN c.path, c.file, r.line ORDER BY c.file, r.line;

// who enters the NGSI-LD pipeline
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol) WHERE f.path IN
  ['context_gateway::app::ngsi_ld_request','context_gateway::app::serve_ngsi_ld'] AND c.is_test=false
RETURN f.name, c.path, c.file, r.line;

// tests that reach a handler other than through router(): 0 rows for every handler
MATCH (t:Symbol)-[:CALLS*1..3 (r, n | WHERE n.name <> 'router')]->(h:Symbol)
WHERE t.is_test=true AND h.package='context-gateway' AND h.is_test=false
  AND h.name IN ['endpoint_record','ngsi_ld','serve_ngsi_ld','file_csv','ogc_features','sensorthings','deliver']
RETURN h.path, count(DISTINCT t);
```

## 2. Route table

### 2.1 Every route `router()` registers

Router: `src/app.rs:411`. Abbreviations: `decide` = `pdp::Pdp::decide` (verdict, `src/pdp/mod.rs:26`); `effective` = `pdp::evaluator::effective` (policy set in force for the subject, `src/pdp/evaluator.rs:466`); `admit` = `app::admit_by` (`src/app.rs:3244`, slug lookup, audience and token, which every `/api/endpoint/{slug}` handler calls first) and `admit_space` (`src/app.rs:3517`). The decision column refers to section 4.

| # | Method + path (registered at) | Handler | PDP verdict / function | Broker call | Covering tests | Decision |
|---|---|---|---|---|---|---|
| R1 | GET `/api/endpoint/{slug}` and `/api/endpoint/{slug}/` (`src/app.rs:422`, `:423`) | `app::endpoint_record` `src/app.rs:3412` | `admit`; `effective` (which grants exist decides `restricted` and the offer) | none: DCAT-AP record built from the endpoint table (`resolver::Endpoint`, `src/resolver.rs`) | `tests/endpoint_record_tests.rs::a_restricted_endpoint_says_restricted_and_points_at_its_access_document`, `tests/endpoint_record_tests.rs::the_digest_in_the_record_is_the_one_the_artifact_answers_with` | STAYS (D1) |
| R2 | GET `/api/endpoint/{slug}/ngsi-ld/v1` and `.../ngsi-ld/v1/` (`:425`, `:426`) | `app::ngsi_ld_entry` `src/app.rs:556` | `admit`; `effective` via `schema::visible` | none: types from the schema projection | `tests/ngsi_ld_entry_tests.rs::the_advertised_access_url_answers_an_entry_document`, `tests/ngsi_ld_entry_tests.rs::the_entry_names_only_the_types_the_caller_may_read` | MOVES (D3) |
| R3 | ANY `/api/endpoint/{slug}/ngsi-ld/v1/{*rest}` (`:427`) | `app::ngsi_ld` `src/app.rs:535` into `app::serve_ngsi_ld` `src/app.rs:747` | `admit`; `decide` at `src/app.rs:811`; then every rule kind of section 3 | `Broker::send` `src/app.rs:1302` (the operation, narrowed); `pdp::conditional::retrieve` `src/pdp/conditional.rs:267` (read before a conditional write); `HeldDelete::apply` `src/app.rs:2049` (relationship rule of a delete) | `tests/edge_app_ngsi_ld_tests.rs::a_refusal_is_rendered_the_way_an_ngsi_ld_client_reads_one`, `tests/leak_sweep_tests.rs::no_surface_leaks_with_an_honest_broker`, `tests/leak_sweep_tests.rs::no_surface_leaks_with_a_broker_that_answers_more_than_it_was_asked` | MOVES (D2); per operation in 2.2 |
| R4 | GET `/api/endpoint/{slug}/access` (`:428`) | `handlers::access_routes::access` `src/handlers/access_routes.rs:20` | `admit`; `effective`, `granted_operations`, `granted_attrs` | none: AuthZEN, ODRL 2.2, Turtle or UCAST rendering of the grants | `tests/access_authzen_tests.rs::the_access_document_lists_the_callers_grants`, `tests/access_authzen_tests.rs::a_type_the_caller_may_not_touch_is_absent_from_the_document` | MOVES (D4) |
| R5 | POST `/api/endpoint/{slug}/mcp` (`:429`) | `app::mcp_message` `src/app.rs:3270` into `mcp::endpoint_facade::handle`; tools re-enter through `app::ngsi_ld_request` `src/app.rs:621` (called at `src/mcp/endpoint_facade.rs:1613`, `:1788`) | `admit` per call; `decide` inside `serve_ngsi_ld`; `effective` for `describe_access` | `Broker::send` `src/app.rs:1302` through `serve_ngsi_ld` | `tests/mcp_tools_tests.rs::the_catalogue_covers_the_read_surface_and_annotates_every_tool`, `tests/mcp_tools_tests.rs::an_unknown_argument_is_a_tool_error_and_never_a_broker_call`, `tests/mcp_read_security_tests.rs::a_join_never_inlines_an_ungranted_entity` | MOVES (D5); per tool in 2.3 |
| R6 | POST `/api/mcp` (`HUB_PATH`, `src/app.rs:3071`; route `:431`) | `mcp::hub::message` `src/mcp/hub.rs:39` | `decide` with an empty `Request` per Endpoint to list what the token reaches; each tool call is handed to R5's façade | through the façade, as R5 | `tests/mcp_hub_tests.rs::an_endpoint_the_token_may_not_read_is_never_listed_and_answers_like_an_unknown_one`, `tests/mcp_hub_tests.rs::endpoint_is_one_string_and_no_other_argument_routes` | STAYS (D6), owner decision O-2 |
| R7 | GET `/api/mcp/.well-known/oauth-protected-resource` (`:432`) | `mcp::hub::protected_resource` `src/mcp/hub.rs:492` | none | none | `tests/mcp_hub_tests.rs::a_client_without_a_token_is_told_where_to_get_one` | STAYS (D7) |
| R8 | POST `/api/mcp/{project}/{name}` (`:437`) | `mcp::server::message` `src/mcp/server.rs:87` | `authenticate`; `decide` per member Endpoint; fan-out through the façade | through the façade, as R5 | `tests/mcp_server_tests.rs::the_server_answers_a_subset_of_the_members_own_urls_and_a_refused_member_is_silent`, `tests/mcp_server_tests.rs::the_same_urn_in_two_members_stays_two_entities` | STAYS (D6), O-2 |
| R9 | GET `/api/mcp/{project}/{name}/.well-known/oauth-protected-resource` (`:438`) | `mcp::server::protected_resource` `src/mcp/server.rs:487` | none | none | `tests/mcp_server_tests.rs::a_public_server_answers_nobody_and_another_says_where_to_get_a_token` | STAYS (D7) |
| R10 | POST `/api/endpoint/{slug}/access/check` (`:442`) | `handlers::access_routes::access_check` `src/handlers/access_routes.rs:110` | `admit`; `effective`, `granted_operations`, `granted_types` (AuthZEN evaluation of one prospective request) | none | `tests/edge_app_access_check_tests.rs::the_decision_is_the_callers_own`, `tests/edge_app_access_check_tests.rs::a_forged_tenant_header_does_not_change_the_decision` | MOVES (D4) |
| R11 | POST `/api/endpoint/{slug}/preview` (`:443`) | `handlers::preview::preview` `src/handlers/preview.rs:63` | `admit_space`; a copy of the record with the draft `ModelProjection` and `hiddenAttributes`, run through `serve_ngsi_ld` (`src/handlers/preview.rs:108`), so `decide` and every rule kind | `Broker::send` `src/app.rs:1302` | `tests/filter_preview_tests.rs::a_caller_the_space_does_not_admit_is_told_the_slug_does_not_exist`, `tests/filter_preview_tests.rs::a_bad_draft_is_refused_naming_the_member_at_fault` | MOVES (D8), GAP G13 |
| R12 | POST `/api/endpoint/{slug}/egress/notifications` (`:449`) | `egress::notifications::deliver` `src/egress/notifications.rs:190` | the sealed subject is opened (`src/egress/subject.rs`) and `PolicyPdp::decide` runs again for it per delivery; `projection` over `data` | `Broker::send` `src/egress/notifications.rs:752` (reads the stored subscription), `:802` (POST to the subscriber's own endpoint) | `tests/notification_egress_tests.rs::a_created_subscription_is_narrowed_and_its_delivery_routed_back_through_the_gateway`, `tests/notification_egress_tests.rs::a_revoked_grant_stops_the_subscription_it_allowed`, `tests/attack_notification_targets_tests.rs::a_redirect_from_the_subscriber_is_never_followed` | MOVES (D9) |
| R13 | GET `/api/endpoint/{slug}/file.geojson` (`:453`) | `handlers::files::file_geojson` `src/handlers/files.rs:27` | `admit`; `decide`; `projection::project_by_type`, `geo::Areas` via `handlers::reads::paged_entities` `src/handlers/reads.rs:281` | `Broker::send` `src/handlers/reads.rs:336` (paged `GET /ngsi-ld/v1/entities`), `:34` (`GET /types` fallback selector) | `tests/edge_app_file_geojson_tests.rs::an_endpoint_that_does_not_serve_geojson_is_not_found_and_the_broker_is_not_asked`, `tests/edge_app_file_geojson_tests.rs::an_unknown_slug_and_a_representation_not_served_answer_the_same_bytes` | STAYS, re-based (D10) |
| R14 | GET `/api/endpoint/{slug}/file.json` (`:454`) | `handlers::files::file_json` `src/handlers/files.rs:88` | as R13 | as R13 | `tests/record_children_are_served_tests.rs::file_json_serves_the_projected_entities_as_an_attachment`, `tests/record_children_are_served_tests.rs::file_json_is_not_served_by_an_endpoint_that_does_not_enable_it` | STAYS, re-based (D10) |
| R15 | GET `/api/endpoint/{slug}/file.csv` (`:455`) | `handlers::files::file_csv` `src/handlers/files.rs:140` | as R13 (through `tabular_download`) | as R13 | `tests/tabular_translator_tests.rs::an_ungranted_attribute_is_not_a_column`, `tests/tabular_translator_tests.rs::the_gateway_pages_through_the_broker` | STAYS, re-based (D10) |
| R16 | GET `/api/endpoint/{slug}/file.xlsx` (`:456`) | `handlers::files::file_xlsx` `src/handlers/files.rs:149` | as R15 | as R13 | `tests/tabular_translator_tests.rs::the_workbook_is_served_as_an_attachment` | STAYS, re-based (D10) |
| R17 | GET `/api/endpoint/{slug}/file.zip` (`:457`) | `handlers::files::file_zip` `src/handlers/files.rs:252` | as R13, plus `effective` for the schemas and catalogue record in the bundle | as R13 | `tests/zip_export_tests.rs::the_bundle_carries_the_data_the_schemas_and_the_catalogue_record`, `tests/zip_export_tests.rs::the_three_shapes_describe_the_same_entities` | STAYS, re-based (D10) |
| R18 | ANY `/api/endpoint/{slug}/ogc/features`, `.../ogc/features/`, `.../ogc/features/{*rest}` (`:460`, `:461`, `:466`) | `handlers::ogc::ogc_features` `src/handlers/ogc.rs:32` | `admit`; `decide` via `handlers::reads::query_entities` `src/handlers/reads.rs:70`; CQL2 translated to `q` (`src/translators/cql2.rs`) | `Broker::send` `src/handlers/reads.rs:114` | `tests/ogc_translator_tests.rs::the_landing_page_links_to_conformance_and_collections`, `tests/attack_policy_widening_tests.rs::a_cql2_filter_on_a_hidden_attribute_is_not_an_oracle` | STAYS, re-based (D10) |
| R19 | ANY `/api/endpoint/{slug}/sta/v1.1`, `.../sta/v1.1/`, `.../sta/v1.1/{*rest}` (`:463`, `:464`, `:465`) | `handlers::sta::sensorthings` `src/handlers/sta.rs:33` | `admit`; `decide` via `reads::query_entities` and `reads::query_temporal` `src/handlers/reads.rs:158`; `temporal::keep_windows` | `Broker::send` `src/handlers/reads.rs:114`, `:192` | `tests/sta_translator_tests.rs::the_service_document_lists_every_sensing_entity_set`, `tests/edge_gateway_sta_tests.rs::a_datastream_links_back_to_its_thing_and_on_to_its_observations` | STAYS, re-based (D10) |
| R20 | GET `/catalog.jsonld` (`:471`) | `app::catalog_feed_jsonld` `src/app.rs:3480` | `effective` for the anonymous subject (public records only, `public_records` `src/app.rs:3454`) | none | `tests/catalog_feed_tests.rs::the_feed_lists_every_public_endpoint_and_nothing_else`, `tests/catalog_feed_tests.rs::an_organization_that_publishes_nothing_answers_an_empty_catalogue` | STAYS (D1) |
| R21 | GET `/catalog.ttl` (`:472`) | `app::catalog_feed_turtle` `src/app.rs:3490` | as R20 | none | `tests/catalog_feed_tests.rs::the_turtle_says_the_same_and_a_members_token_widens_nothing` | STAYS (D1) |
| R22 | GET `/cs` (`:473`) | `handlers::space_routes::space_catalog` `src/handlers/space_routes.rs:18` | `authenticate`; `decide` through `app::discoverable` `src/app.rs:3545` (a space is listed when some operation is not denied) | none | `tests/edge_app_space_catalog_tests.rs::the_catalog_is_json_ld_whatever_the_caller_accepts`, `tests/edge_app_space_catalog_tests.rs::a_second_authorization_header_adds_nothing` | STAYS (D1) |
| R23 | GET `/cs/{space}` (`:474`) | `handlers::space_routes::space_record` `src/handlers/space_routes.rs:46` | `admit_space` (`authenticate`, `decide`) | none | `tests/space_surface_tests.rs::the_space_record_is_a_dcat_ap_dataset`, `tests/space_surface_tests.rs::the_space_record_is_also_turtle` | STAYS (D1) |
| R24 | ANY `/cs/{space}/ngsi-ld/v1/{*rest}` (`:475`) | `app::space_ngsi_ld` `src/app.rs:594` into `serve_ngsi_ld` | `admit_space`; then as R3 | as R3 | `tests/edge_app_space_ngsi_ld_tests.rs::the_collapsed_refusal_is_still_an_ngsi_ld_error_document`, `tests/edge_app_space_ngsi_ld_tests.rs::the_space_table_is_not_reachable_through_the_endpoint_table`, `tests/hub_federation_tests.rs::the_entity_map_header_crosses_the_gateway_in_both_directions` | MOVES (D2) |
| R25 | POST `/cs/{space}/mcp` (`:476`) | `app::space_mcp_message` `src/app.rs:3328` | `admit_space`; then as R5 | as R5 | `tests/edge_app_space_mcp_message_tests.rs::no_token_of_any_shape_changes_the_refusal`, `tests/edge_app_space_mcp_message_tests.rs::a_valid_token_without_a_grant_does_not_find_the_instance` | MOVES (D5) |
| R26 | GET `/cs/{space}/schema/index.json` (`:480`) | `handlers::schema_routes::space_schema_index` `src/handlers/schema_routes.rs:35` | `admit_space` (`decide`); `effective` for the class and slot projection | none: models from the repository (`resolver::Model`) | `tests/record_children_are_served_tests.rs::the_schema_surface_answers_under_both_prefixes` | MOVES (D11) |
| R27 | GET `/cs/{space}/dump/` (`:481`) | `handlers::files::space_dump` `src/handlers/files.rs:282` | `admit_space`; `decide`; projection via `paged_entities` | as R13 | `tests/space_dump_tests.rs::the_dump_holds_what_the_callers_grants_read_and_nothing_else`, `tests/space_dump_tests.rs::a_caller_with_no_grant_gets_the_404_of_a_space_that_does_not_exist` | STAYS, re-based (D10) |
| R28 | GET `/cs/{space}/schema/{version}/{artifact}` (`:482`) | `handlers::schema_routes::space_schema_artifact` `src/handlers/schema_routes.rs:70` | as R26 | none | `tests/record_children_are_served_tests.rs::the_schema_surface_answers_under_both_prefixes` | MOVES (D11) |
| R29 | GET `/api/endpoint/{slug}/.well-known/oauth-protected-resource` (`:486`) | `app::protected_resource` `src/app.rs:3352` | none | none | `tests/edge_app_protected_resource_tests.rs::only_the_exact_well_known_path_answers`, `tests/edge_app_protected_resource_tests.rs::the_same_request_twice_at_once_is_the_same_document` | STAYS (D7) |
| R30 | GET `/api/endpoint/{slug}/schema/index.json` (`:490`) | `handlers::schema_routes::schema_index` `src/handlers/schema_routes.rs:21` | `admit`; `effective` | none | `tests/schema_surface_tests.rs::the_index_describes_what_this_endpoint_publishes`, `tests/schema_surface_tests.rs::an_endpoint_never_names_what_it_hides` | MOVES (D11) |
| R31 | GET `/api/endpoint/{slug}/openapi.json` (`:492`) | `handlers::openapi_doc::openapi` `src/handlers/openapi_doc.rs:20` | `admit`; `effective` | none | `tests/openapi_doc_tests.rs::the_document_is_valid_openapi_3_1_and_describes_only_what_the_caller_may_read`, `tests/openapi_doc_tests.rs::a_projection_and_a_hidden_attribute_narrow_the_document_as_they_narrow_the_data` | MOVES (D3) |
| R32 | GET `/api/endpoint/{slug}/schema/{version}/{artifact}` (`:496`) | `handlers::schema_routes::schema_artifact` `src/handlers/schema_routes.rs:56` | `admit`; `effective` | none | `tests/edge_app_schema_artifact_tests.rs::a_forbidden_class_and_slot_are_projected_out`, `tests/schema_surface_tests.rs::the_bare_model_is_negotiated_and_labelled` | MOVES (D11) |
| R33 | GET `/healthz` (`:506`) | `app::ok` `src/app.rs:524` | none | none | `tests/metrics_tests.rs::the_scrape_does_not_count_as_traffic` | STAYS (D12) |
| R34 | GET `/livez` (`:507`) | `app::ok` `src/app.rs:524` | none | none | `tests/antares_integration_tests.rs::the_health_probes_need_no_endpoints_and_no_broker` | STAYS (D12) |
| R35 | GET `/metrics` (`:512`) | `telemetry::metrics` `src/telemetry.rs:84` | none (counts `decided`, `src/pdp/mod.rs:58`) | none | `tests/metrics_tests.rs::the_scrape_target_answers_the_text_format_prometheus_reads`, `tests/metrics_tests.rs::a_policy_decision_is_counted_with_its_operation_and_verdict` | STAYS (D12) |
| R36 | any other path: `.fallback(missing)` (`:514`) | `app::missing` `src/app.rs:529` | none | none | `tests/edge_app_protected_resource_tests.rs::only_the_exact_well_known_path_answers` (its `/.well-known/oauth-protected-resource` probe is off the surface) | STAYS (D12) |
| L1 | layer on R1..R32: `rate_limit::enforce` (`:502`) | `middleware::rate_limit::enforce` `src/middleware/rate_limit.rs:202` | none | none | `tests/rate_limit_tests.rs::a_burst_is_spent_once_and_then_refills_at_the_steady_rate`, `tests/rate_limit_tests.rs::one_callers_burst_is_not_another_callers` | STAYS (D13) |
| L2 | outer layer on every route: `response::scrub` (`:517`) | `middleware::response::scrub` `src/middleware/response.rs:48` | none | none | `tests/edge_app_ogc_features_tests.rs::the_narrowing_signal_is_only_sent_to_a_caller_who_asked`, `tests/edge_app_file_zip_tests.rs::a_forged_tenant_neither_reaches_the_broker_nor_comes_back` | STAYS (D13) |
| L3 | outermost layer: `telemetry::record` (`:521`) | `telemetry::record` `src/telemetry.rs:105` | none | none | `tests/metrics_tests.rs::the_scrape_does_not_count_as_traffic` | STAYS (D12) |

Count: 36 route rows (R1..R36, covering the 41 `.route(...)` calls and the fallback) and 3 layer rows. No route is untested.

### 2.2 The NGSI-LD operations behind R3 and R24

`operations::operation_of` (`src/operations.rs:16`) names the operation before the PDP is asked; a pair it does not know answers 404 (`src/app.rs:781`). Every row below is served by `serve_ngsi_ld` (`src/app.rs:747`), decided by `decide` (`src/app.rs:811`), and forwarded by `Broker::send` (`src/app.rs:1302`) to the same path under `/ngsi-ld/v1` with the narrowed query (`query::upstream`, `src/query.rs:341`) or body. The Antares column is the clause the broker gates the same operation under (`policy::gate` call sites, Antares cg) and whether ADR-0020 lets it narrow (`policy::FILTERABLE`, `crates/antares-api/src/policy.rs:68`).

| Method + path under `ngsi-ld/v1` (`src/operations.rs` line) | Gateway `Operation` | CIM 009 / TS 104 175 | In `FILTERABLE`? | Gateway-only treatment before the broker | Tests | Decision |
|---|---|---|---|---|---|---|
| GET `entities` (:19) | `QueryEntity` | 5.7.2 / 10.4.3 | yes | `query::malformed`, `query::unselected` (400, `src/app.rs:788`, `:797`); type/attrs/q/geo/temporal narrowing; dropped-type warning; post-filter by type, id and area | `tests/filter_oracle_tests.rs::bisecting_a_hidden_value_learns_nothing`, `tests/answer_type_guard_tests.rs::the_count_of_a_narrowed_answer_is_not_the_brokers_count` | MOVES |
| POST `entities` (:20) | `CreateEntity` | 5.6.1 / 10.2.2 | no (Filter = Deny) | write guard, units, relationships, form creates (minted id, daily cap) | `tests/write_guard_route_tests.rs::a_batch_delete_of_an_ungranted_type_is_refused_without_a_broker_hop`, `tests/form_creates_tests.rs::a_caller_chosen_id_is_not_used` | MOVES (guard); creates cap STAYS |
| GET `entities/{id}` (:21) | `RetrieveEntity` | 5.7.1 / 10.4.2 | yes | id outside grant = 404 without asking (`src/app.rs:911`); answer type judged (`projection::permitted`) | `tests/answer_type_guard_tests.rs::the_dropped_element_counts_towards_the_restricted_answer`, `tests/edge_pdp_projection_permitted_tests.rs::an_exact_id_grant_is_anchored_to_that_one_entity` | MOVES |
| PATCH `entities/{id}` (:22) | `MergeEntity` | 5.6.17 / 10.2.9 | no | write guard, conditional write | `tests/conditional_write_tests.rs::an_if_match_that_agrees_with_the_stored_tag_is_forwarded_carrying_it` | MOVES |
| PUT `entities/{id}` (:23) | `ReplaceEntity` | 5.6.18 / 10.2.10 | no | write guard | `tests/write_guard_route_tests.rs::a_mixed_batch_delete_applies_the_granted_entity_and_answers_the_other` (route family; no PUT-specific test found: unknown) | MOVES |
| DELETE `entities/{id}` (:24) | `DeleteEntity` | 5.6.6 / 10.2.8 | no | id grant; relationship delete rule (DM-71) | `tests/relationship_delete_tests.rs::a_desk_another_teacher_has_is_refused_and_nothing_is_written` | MOVES |
| POST `entities/{id}/attrs` (:25) | `AppendAttrs` | 5.6.3 / 10.2.4 | no | write guard (fragment) | `tests/write_guard_route_tests.rs::a_batch_delete_of_an_ungranted_type_is_refused_without_a_broker_hop` (family) | MOVES |
| PATCH `entities/{id}/attrs` (:26) | `UpdateAttrs` | 5.6.2 / 10.2.3 | no | write guard | as above | MOVES |
| PATCH `entities/{id}/attrs/{attr}` (:27) | `UpdateAttrs` (the gateway does not separate 5.6.4) | 5.6.4 / 10.2.5 | no | `targeted_attribute` (`src/operations.rs:109`) checked against the grant in `judge_write` (`src/app.rs:2337`) | unknown | MOVES |
| PUT `entities/{id}/attrs/{attr}` (:28) | `ReplaceAttrs` | 5.6.19 / 10.2.11 | no | as above | unknown | MOVES |
| DELETE `entities/{id}/attrs/{attr}` (:29) | `DeleteAttrs` | 5.6.5 / 10.2.7 | no | as above | unknown | MOVES |
| POST `entityOperations/create` (:31) | `CreateBatch` | 5.6.7 / 10.3.2 | no | per-entity division into a 207 (`Judged::Divided`, `src/app.rs:1012`) | `tests/write_guard_route_tests.rs::a_mixed_batch_delete_applies_the_granted_entity_and_answers_the_other` | MOVES, GAP G9 |
| POST `entityOperations/upsert` (:32) | `UpsertBatch` | 5.6.8 / 10.3.3 | no | as above | as above | MOVES, G9 |
| POST `entityOperations/update` (:33) | `UpdateBatch` | 5.6.9 / 10.3.4 | no | as above | as above | MOVES, G9 |
| POST `entityOperations/merge` (:34) | `MergeBatch` | 5.6.20 / 10.3.5 | no | as above | as above | MOVES, G9 |
| POST `entityOperations/delete` (:35) | `DeleteBatch` | 5.6.10 / 10.3.6 | no | as above, plus relationship delete rule | `tests/write_guard_route_tests.rs::a_batch_delete_of_an_ungranted_type_is_refused_without_a_broker_hop` | MOVES, G9 |
| POST `entityOperations/query` (:36) | `QueryBatch` | Query Entities via POST: the gateway comment cites 5.6.9; the broker gates it as `"5.7.2"` (`crates/antares-api/src/batch.rs:1346`) / 10.4.3 | yes (as 5.7.2) | body narrowed (`narrowed_batch_query`, `src/app.rs:2564`); empty body refused (`src/app.rs:1110`) | `tests/batch_query_body_tests.rs::a_batch_query_under_a_type_grant_sends_the_type_in_the_body_only`, `tests/batch_query_body_tests.rs::a_body_selects_nothing_the_grant_does_not` | MOVES |
| GET `temporal/entities` (:38) | `QueryTemporal` | 5.7.4 / 11.3.3 | yes | temporal clamp, window gaps, `lastN` cap | `tests/temporal_tests.rs::the_callers_window_is_clamped_to_the_grant`, `tests/edge_gateway_temporal_tests.rs::between_without_both_ends_is_no_window` | MOVES |
| POST `temporal/entityOperations/query` (:42) | `QueryTemporal` | 5.7.4 / 11.3.3 (POST form) | yes | body narrowed as `entityOperations/query` | `tests/batch_query_body_tests.rs::a_body_selects_nothing_the_grant_does_not` (family) | MOVES |
| POST `temporal/entities` (:45) | `UpsertTemporal` | 5.6.11 / 11.2.2 | no | write guard; refused on a form endpoint | unknown | MOVES |
| GET `temporal/entities/{id}` (:46) | `RetrieveTemporal` | 5.7.3 / 11.3.2 | yes | `query::upstream_by_id` (no `type`, per-type attrs, `src/query.rs:364`) | `tests/edge_gateway_temporal_tests.rs::a_relation_the_vocabulary_does_not_know_is_no_window_at_all` | MOVES |
| DELETE `temporal/entities/{id}` (:47) | `DeleteTemporal` | 5.6.16 / 11.2.7 | no | id grant | unknown | MOVES |
| POST `temporal/entities/{id}/attrs` (:48) | `AppendAttrsTemporal` | 5.6.12 / 11.2.3 | no | write guard | unknown | MOVES |
| DELETE `temporal/entities/{id}/attrs/{attr}` (:51) | `DeleteAttrsTemporal` | 5.6.13 / 11.2.4 | no | targeted attribute | unknown | MOVES |
| PATCH `temporal/entities/{id}/attrs/{attr}/{instance}` (:54) | `UpdateAttrInstanceTemporal` | 5.6.14 / 11.2.5 | no | targeted attribute | unknown | MOVES |
| DELETE `temporal/entities/{id}/attrs/{attr}/{instance}` (:57) | `DeleteAttrInstanceTemporal` | 5.6.15 / 11.2.6 | no | targeted attribute | unknown | MOVES |
| GET `types` (:62), `types?details=true` (:61) | `RetrieveEntityTypes`, `RetrieveEntityTypeDetails` | 5.7.5, 5.7.6 / 10.6.3, 10.6.4 | no | `vocabulary::narrow` over `typeList`/`typeNames` (`src/pdp/vocabulary.rs:115`) | `tests/types_attributes_tests.rs::a_hidden_attribute_is_not_found_and_the_broker_is_not_asked` | MOVES, GAP G10 |
| GET `types/{type}` (:63) | `RetrieveEntityTypeInfo` | 5.7.7 / 10.6.5 | no | `vocabulary::reaches` before asking (`src/app.rs:891`) | `tests/types_attributes_tests.rs::a_name_outside_the_projection_is_not_found_like_a_name_that_does_not_exist` | MOVES, G10 |
| GET `attributes` (:65), `attributes?details=true` (:64) | `RetrieveAttrTypes`, `RetrieveAttrTypeDetails` | 5.7.8, 5.7.9 / 10.6.6, 10.6.7 | no | as `types` | as above | MOVES, G10 |
| GET `attributes/{attr}` (:66) | `RetrieveAttrTypeInfo` | 5.7.10 / 10.6.8 | no | as `types/{type}` | `tests/types_attributes_tests.rs::a_hidden_attribute_is_not_found_and_the_broker_is_not_asked` | MOVES, G10 |
| GET `subscriptions` (:68) | `QuerySubscription` | 5.8.4 / 10.5.5 | no | passthrough query | unknown | MOVES |
| POST `subscriptions` (:69) | `CreateSubscription` | 5.8.1 / 10.5.2 | no | `narrowed_subscription` (`src/app.rs:2527`): selection narrowed to the grants, `notification.endpoint.uri` rewritten to R12, subject sealed | `tests/notification_egress_tests.rs::a_subscription_that_watches_nothing_granted_is_refused`, `tests/edge_egress_notifications_narrow_subscription_tests.rs::the_grants_filter_is_conjoined_on_creation_and_a_silent_patch_keeps_what_was_stored` | MOVES, GAP G11 |
| GET `subscriptions/{id}` (:70) | `RetrieveSubscription` | 5.8.3 / 10.5.4 | no | passthrough | unknown | MOVES |
| PATCH `subscriptions/{id}` (:71) | `UpdateSubscription` | 5.8.2 / 10.5.3 | no | as POST | `tests/edge_egress_notifications_narrow_subscription_tests.rs::the_grants_filter_is_conjoined_on_creation_and_a_silent_patch_keeps_what_was_stored` | MOVES, G11 |
| DELETE `subscriptions/{id}` (:72) | `DeleteSubscription` | 5.8.5 / 10.5.6 | no | passthrough | unknown | MOVES |

Not mapped by the gateway, so 404 at `src/app.rs:781` although the broker serves them: `GET entities/{id}/attrs/{attr}` (TS 104 175 10.4.4, new), `csourceRegistrations*` (12.x), `csourceSubscriptions*` (12.4.x), `jsonldContexts*` (13.x), `entityMaps*` (14.x, although `jc_core::kinds::Operation` names them, `crates/jc-core/src/kinds/policy.rs`), `snapshots*` (16.x), `csourceIdentity` (15.2), `DELETE entities?purge` (`PurgeEntity` is in the vocabulary but has no path). Moving the surface into the broker makes these reachable through a view unless the engine denies them (section 6, S16).

### 2.3 The MCP tools behind R5, R25 (and R6, R8 through the façade)

`mcp::endpoint_facade` dispatches JSON-RPC `initialize` (`src/mcp/endpoint_facade.rs:1443`), `ping` (`:1460`) and the tools below; each data tool builds the NGSI-LD request it stands for and calls `ngsi_ld_request` (`src/mcp/endpoint_facade.rs:1613`), so its PDP and broker call are the 2.2 row named. All MOVE with the MCP surface (D5).

| Tool (declared at `src/mcp/endpoint_facade.rs`) | NGSI-LD request (dispatch line) | Broker | Tests |
|---|---|---|---|
| `query_entities` (:110) | GET `entities` (:2253) | via 2.2 | `tests/mcp_tools_tests.rs::the_catalogue_covers_the_read_surface_and_annotates_every_tool` |
| `get_entity` (:136) | GET `entities/{id}` (:2399) | via 2.2 | `tests/mcp_read_security_tests.rs::an_inlined_entity_of_a_granted_type_is_projected_like_any_other` |
| `list_types` (:154), `list_attributes` (:172) | GET `types`, `attributes` (:2259) | via 2.2 | `tests/mcp_tools_tests.rs::the_catalogue_covers_the_read_surface_and_annotates_every_tool` |
| `get_type` (:354), `get_attribute` (:371) | GET `types/{t}`, `attributes/{a}` (:2290, :2299) | via 2.2 | unknown |
| `query_temporal` (:190), `retrieve_temporal` (:197) | GET `temporal/entities[/{id}]` (:2413) | via 2.2 | unknown |
| `batch_query` (:212) | POST `entityOperations/query` (:2359) | via 2.2 | `tests/mcp_read_security_tests.rs::an_id_pattern_cannot_leave_the_grant` |
| `batch_query_temporal` (:336) | POST `temporal/entityOperations/query` (:2317) | via 2.2 | unknown |
| `list_subscriptions` (:238), `get_subscription` (:391) | GET `subscriptions[/{id}]` (:2269, :2308) | via 2.2 | unknown |
| `create_subscription` (:411) | POST `subscriptions` after an elicitation (`src/mcp/elicitation.rs`) (:2346) | via 2.2 | `tests/mcp_hub_tests.rs::an_elicitation_asked_on_one_endpoint_does_not_confirm_a_call_on_another` |
| `upsert_entity` (:320) | POST `entityOperations/upsert` (:2440, :2449) | via 2.2 | `tests/conditional_write_tests.rs::a_state_dependent_grant_reads_first_and_writes_conditionally` (reaches `mcp::server::message`, cg) |
| `describe_access` (:257) | none: `effective` rendering (:1530) | none | `tests/mcp_tools_tests.rs::the_catalogue_covers_the_read_surface_and_annotates_every_tool` |
| `describe_schema` (:275) | none: schema index (:1539) | none | unknown |

## 3. PDP rule kinds and their ADR-0020 equivalent

The gateway's verdict is `Verdict::Deny` or `Verdict::Rewrite(Constraints)` (`src/pdp/evaluator.rs:190`); `Constraints` (`src/pdp/evaluator.rs:94`) has 20 members, the seam's `Filter` (`crates/antares-api/src/policy.rs:165`) has five: `q` (a `QNode`, conjoined on the AST), `scope_q`, `omit`, `pick`, `restricted`. `Decision` is `Allow`, `Deny(String)` or `Filter(Filter)` (`policy.rs:150`); a non-empty `Filter` on a clause outside `FILTERABLE = ["5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.14.4", "5.14.5"]` (TS 104 175 10.4.2, 10.4.3, 11.3.2, 11.3.3, 14.5, 14.6; `policy.rs:68`) is answered `Deny` by `resolve` (`policy.rs:366`), and `WHOLE_TENANT = ["5.6.21", "5.16.1", "5.16.2", "5.16.7"]` (10.2.12, 16.2, 16.3, 16.7; `policy.rs:59`) gets the whole-tenant reason. A `pre_notify` answer is `Deliver`, `Filter` (projection only; `q`/`scope_q` makes it a `Drop`) or `Drop` (`policy.rs:287`). The engine receives `Operation { clause, ids, types, attrs, q, scope_q, geo, body }` (`policy.rs:97`), every name expanded.

"Engine" below means the joinedcontext policy engine as an addon crate behind the seam (ADR-0020: "Every engine is an addon", `docs/adr/ADR-0020-policy-seam.md:187`). A rule that the engine can decide from `Operation` alone needs no `Filter` member; a rule that changes what the store returns does.

| # | Rule kind | Evaluated at | ADR-0020 equivalent | FILTERABLE / WHOLE_TENANT | Status | Tests |
|---|---|---|---|---|---|---|
| K1 | Assignee: user, group, role, service account, DID; anonymous = role `public` | `Subject::is` `src/pdp/evaluator.rs:55`; `Subject::anonymous` `:47`; subject built by `app::subject_of` `src/app.rs:3084` | engine-internal, keyed on `Subject.headers` (`policy.rs:76`); the broker parses no credential (`ADR-0020:57-58`) | n/a | covered, needs identity headers (GAP G15) | `tests/pdp_tests.rs::an_anonymous_caller_gets_the_public_grant_as_a_rewrite` |
| K2 | Endpoint roles: `callerRole`, named roles, App client roles | `EndpointRoles::held_by` `src/resolver.rs:70`; `with_endpoint_roles` `src/app.rs:3049` | engine-internal (assignee resolution) | n/a | covered (engine data) | `tests/app_endpoint_grants_tests.rs` (file; test name unknown) |
| K3 | Operations: single CIM 009 names and Table 4.20-2 groups | `grants_operation` `src/pdp/evaluator.rs:264`; `granted_operations` `:479` | `Allow`/`Deny` on `Operation.clause` | the gateway's names map to clauses (2.2) | covered; the vocabulary names CIM 009 wire names, the seam clause numbers: a name-to-clause table is needed in the engine | `tests/pdp_tests.rs::an_operation_group_grant_covers_its_table_members`, `tests/edge_pdp_evaluator_granted_operations_tests.rs::a_read_group_carries_no_write` |
| K4 | Validity window (`validity.from`/`to`) | `in_force` `src/pdp/evaluator.rs:272` | engine-internal | n/a | covered | `tests/pdp_tests.rs::a_policy_outside_its_validity_window_grants_nothing` |
| K5 | Prohibition first, over the request's types | `evaluate` `src/pdp/evaluator.rs:231`; `covers_request` `:283` | `Deny` | n/a | covered | `tests/pdp_tests.rs::a_prohibition_overrides_a_permission_that_would_have_matched` |
| K6 | Default deny (no grant matched) | `src/pdp/evaluator.rs:245` | `Deny`; the seam's built-in is `AllowAll`, so the deny-by-default lives in the engine | n/a | covered | `tests/pdp_tests.rs::an_empty_policy_set_denies` |
| K7 | Entity types: request ∩ union of granted types; nothing left = empty answer | `intersect` `src/pdp/evaluator.rs:296`, `:316`, `:331`; `narrow` `:438`; forwarded as `type=` `src/query.rs:381` | none | FILTERABLE reads only | **GAP G1** | `tests/model_projection_tests.rs::an_attribute_outside_is_dropped_and_a_type_outside_is_refused` |
| K8 | Entity id and anchored `idPattern` | compiled `src/pdp/evaluator.rs:372`; judged on the answer `projection::id_permitted` `src/pdp/projection.rs:259`; write `write_guard::check_granted_id` `src/pdp/write_guard.rs:86`; read of an ungranted id = 404 unasked `src/app.rs:911` | none for reads; `Deny` from `Operation.ids` for writes | FILTERABLE reads only | **GAP G2** | `tests/pdp_tests.rs::an_exact_id_selector_compiles_into_a_pattern_that_matches_only_it`, `tests/mcp_read_security_tests.rs::an_id_pattern_cannot_leave_the_grant` |
| K9 | Attribute whitelist (`propertyNames` + `relationshipNames`; a grant without a list is the whole entity) | `src/pdp/evaluator.rs:304`; `granted_attrs` `:502`; answer `projection::project_entity` `src/pdp/projection.rs:39` | `Filter.pick` | FILTERABLE | covered (one level; see K18 for joins) | `tests/edge_pdp_evaluator_granted_attrs_tests.rs::no_whitelist_anywhere_means_the_whole_entity` |
| K10 | Per-policy `q` and scopes as one term, policies OR-ed, caller's `q` AND-ed; an unbalanced caller `q` is dropped | `policy_filter` `src/pdp/evaluator.rs:410`; `scope_folding::fold` `src/pdp/scope_folding.rs:20`; `conjoin` `src/pdp/evaluator.rs:521`, `:535` | `Filter.q` (AST conjunction, `policy.rs:246`) and `Filter.scope_q` (`policy.rs:257`) | FILTERABLE | **GAP G3** for the OR of (q ∧ scope) pairs; single-policy case covered | `tests/scope_folding_tests.rs::children_match_and_siblings_and_prefixes_do_not`, `tests/edge_pdp_evaluator_conjoin_tests.rs::a_caller_in_front_of_several_grants_faces_their_union` |
| K11 | Granted scope tree for writes (the payload's `scope` must sit inside) | `granted_scopes` `src/pdp/evaluator.rs:392`; `check_scope` `src/pdp/write_guard.rs:221` | `Deny` from `Operation.body` | not FILTERABLE: Filter = Deny | covered (engine reads the body) | `tests/write_guard_route_tests.rs::a_batch_delete_of_an_ungranted_type_is_refused_without_a_broker_hop` (family) |
| K12 | Geo grant: caller area ∩ grant areas; answer post-filtered by every grant area; write location inside; `geoproperty`/`geometryProperty` refused beside a grant area | `geo::intersect` `src/pdp/geo.rs:123`; `Areas::admits` used at `src/app.rs:2927`, `:2942`; `check_location` `src/pdp/write_guard.rs:145`; `query::geo_refusal` `src/query.rs:270` | none (`Operation.geo` is read, `Filter` has no geo member) | FILTERABLE reads; writes Deny | **GAP G5** | `tests/geo_tests.rs::polygon_containment`, `tests/geo_parameters_tests.rs::a_geoproperty_beside_a_grants_own_area_is_refused_by_name` |
| K13 | Temporal clamp: caller window ∩ grant windows (relative windows resolved at `now`); hull forwarded, gaps dropped; `lastN` capped at 1000 | `temporal::clamp` `src/pdp/temporal.rs:75`; `keep_windows` `:260`; `LAST_N_CAP` `src/query.rs:444` | none | 5.7.3, 5.7.4 FILTERABLE | **GAP G6**; the cap STAYS (request transform) | `tests/temporal_tests.rs::a_relative_grant_window_slides_with_now`, `tests/temporal_tests.rs::the_callers_window_is_clamped_to_the_grant` |
| K14 | "Answer is nothing" (`Constraints.empty`): 200 `[]` for a query, 404 for an addressed read, broker not asked | `src/app.rs:877`; `empty_list` `src/app.rs:2749` | none | FILTERABLE | **GAP G7** | `tests/types_attributes_tests.rs::a_hidden_attribute_is_not_found_and_the_broker_is_not_asked` |
| K15 | Restricted signal (opt-in by the caller's own `NGSILD-Results-Restricted: true` request header) | computed `src/pdp/evaluator.rs:363`; set `src/app.rs:2843`; opt-in `src/middleware/response.rs:37`, removed otherwise `:67` | `Filter.restricted` → `Antares-Results-Restricted: true` (`policy.rs:276`), sent whenever the engine sets it | FILTERABLE | covered with a different name and no opt-in (migration risk M1) | `tests/edge_app_ogc_features_tests.rs::the_narrowing_signal_is_only_sent_to_a_caller_who_asked` |
| K16 | Hidden attributes of the Endpoint (EP-61): never served, and filtering on one empties the answer | `PolicyPdp::decide` `src/pdp/mod.rs:64`; `drop_types_that_may_not_be_filtered` `:195` | `Filter.omit` for the projection; the "filter on hidden = nothing" half needs G7 | FILTERABLE | covered (omit) + G7 | `tests/types_attributes_tests.rs::a_hidden_attribute_is_not_found_and_the_broker_is_not_asked`, `tests/filter_oracle_tests.rs::bisecting_a_hidden_value_learns_nothing` |
| K17 | Model projection (MP-02): classes, slots per class, residual `q`/`geoQ`/`temporalQ` | `project_constraints` `src/pdp/mod.rs:99` | classes: none (G1); slots per class: none (**GAP G4**, `pick` is one list); residual `q`: `Filter.q` | FILTERABLE | G1, G4, G5, G6 | `tests/model_projection_tests.rs::a_query_through_the_projection_returns_only_the_projected_attributes` |
| K18 | Filter oracle (T-1862): a type is dropped when the request filters or orders on an attribute it may not serve; the dropped types are named in `NGSILD-Warning: 199` | `drop_types_that_may_not_be_filtered` `src/pdp/mod.rs:180`; `ALWAYS_SERVED` `:153`; `warn_about_dropped_types` `src/app.rs:2773` | engine can read `Operation.q`; the per-type drop needs G1 + G4; the warning has no seam member (**GAP G8**) | FILTERABLE | G1, G4, G8 | `tests/filter_oracle_tests.rs::q_on_an_attribute_of_one_type_matches_that_type_only`, `tests/filter_oracle_tests.rs::an_answer_says_which_types_the_filter_left_out` |
| K19 | Answer judged after the broker: type and id of every entity, area; `NGSILD-Results-Count` removed when anything was dropped | `projection::permitted` `src/pdp/projection.rs:212`; `project_answer` `src/app.rs:2834`, count removed `:2933` | the broker narrows in the store, so the count and paging describe the narrowed set (`ADR-0020:20-27`) | FILTERABLE | covered by construction once G1, G2, G5 exist | `tests/answer_type_guard_tests.rs::the_count_of_a_narrowed_answer_is_not_the_brokers_count` |
| K20 | Joins: an inlined entity is judged and cut like a top-level one, depth 3; `joinLevel` capped | `narrow_joined` `src/pdp/projection.rs:108`, `JOIN_DEPTH` `:99`; `JOIN_LEVEL_CAP` `src/query.rs:434` | ADR-0020 "the policy projection ... travels down the join walk unchanged. A Relationship the narrowing removes is not traversed either" (`ADR-0020:89-93`) | 5.7.1, 5.7.2 | covered for `pick`/`omit`/`q`; type/id/area of the joined entity need G1, G2, G5; cap STAYS | `tests/mcp_read_security_tests.rs::a_join_never_inlines_an_ungranted_entity`, `tests/mcp_read_security_tests.rs::a_join_level_is_bounded_on_both_surfaces` |
| K21 | Discovery narrowing: types/attributes lists cut to reachable names; an unreachable name is 404 before asking | `vocabulary::reaches` `src/pdp/vocabulary.rs:38`; `vocabulary::narrow` `:115`; call sites `src/app.rs:891`, `:2906` | none: 5.7.5 to 5.7.10 (10.6.3 to 10.6.8) are not FILTERABLE, a `Filter` there is `Deny` | not FILTERABLE | **GAP G10** | `tests/types_attributes_tests.rs::a_name_outside_the_projection_is_not_found_like_a_name_that_does_not_exist` |
| K22 | Write guard: the payload falls whole inside one grant (id, type, attributes, scope, location, no policy vocabulary smuggled) | `write_guard::check` `src/pdp/write_guard.rs:72`; `check_no_smuggled_policy` `:206`; `judge_write` `src/app.rs:2325` | `Allow`/`Deny` from `Operation.body`, `types`, `attrs`, `ids` | not FILTERABLE (writes are whole) | covered for single writes | `tests/write_guard_route_tests.rs::a_batch_delete_of_an_ungranted_type_is_refused_without_a_broker_hop` |
| K23 | Batch division: permitted entities forwarded, refused ones merged into a 207 | `Judged::Divided` `src/app.rs:1012`; `merged_batch` `src/app.rs:2461` | none: one `Decision` per request | 10.3.x not FILTERABLE | **GAP G9** | `tests/write_guard_route_tests.rs::a_mixed_batch_delete_applies_the_granted_entity_and_answers_the_other` |
| K24 | Conditional write: a grant with `q` or a polygon is evaluated against the stored entity, then forwarded with `If-Match`; a batch under such a grant is refused | `conditional::state_dependent` `src/pdp/conditional.rs:36`, `required` `:44`, `batch_refusal` `:63`; read `:267` | none: the engine sees the request body, never the stored entity | not FILTERABLE | **GAP G12** | `tests/conditional_write_tests.rs::a_write_with_no_condition_and_no_if_match_is_forwarded_without_a_read`, `tests/conditional_write_tests.rs::a_state_dependent_grant_reads_first_and_writes_conditionally` |
| K25 | Subscription narrowing at create/update, delivery routed back through R12, subscriber sealed with `JC_GATEWAY_DELIVERY_KEY`, delivery re-decided under the policies in force | `narrowed_subscription` `src/app.rs:2527`; `src/egress/subject.rs`; `deliver` `src/egress/notifications.rs:190` | `__subject` stored with the subscription and `pre_notify` per subscriber (`ADR-0020:150-157`, `policy.rs:319`); `NotifyDecision::Filter` projects members; a `q` is a `Drop` | 5.8.1/5.8.2 not FILTERABLE | per-entity drop and the stored-state `q` re-check: **GAP G11** | `tests/notification_egress_tests.rs::a_revoked_grant_stops_the_subscription_it_allowed` |
| K26 | View mapping (DM-51): read only, query inverted to the source model, answer rebuilt, grants judged in the source model | `src/app.rs:774`; `invert_query` `src/app.rs:2183`; `Constraints::in_source_model` `src/pdp/evaluator.rs:179`; `translated` `src/app.rs:2218` | none: a model transform, not a narrowing | n/a | **GAP G18** (needs the endpoint-view surface crate) | `tests/view_mapping_tests.rs::a_filter_written_in_the_target_model_reaches_the_broker_in_the_source_model` |
| K27 | Declared types (DM-61): a write of a type the space's model lacks is 400 | `undeclared_type` `src/app.rs:2509`, called `:2394` | `Deny` from `Operation.types` (the 400 becomes a 403) | not FILTERABLE | covered with a status change (M7) | `tests/declared_types_tests.rs::a_type_the_model_lacks_is_a_400_naming_it_and_the_broker_is_not_asked` |
| K28 | Units (DM-06): a wrong `unitCode` is 400, a missing one is filled | `filled_units` `src/app.rs:2091`; `src/units.rs` | none: a body rewrite; the seam cannot change a body | not FILTERABLE | **GAP G19** | `tests/unit_code_tests.rs::a_wrong_unit_code_is_a_400_naming_both_codes_and_the_broker_is_not_asked`, `tests/unit_code_tests.rs::a_missing_unit_code_is_filled_and_the_answer_says_so` |
| K29 | Relationships (DM-70, DM-71): target type, cardinality, required end, target exists and is readable by the writer; delete rules | `HeldWrite::check` (struct `src/app.rs:1345`), `HeldDelete` (`apply` `src/app.rs:2049`); `src/relationships.rs` | none: needs the visibility of the linked entity under the same subject | not FILTERABLE | **GAP G17** (GOAL point 3) | `tests/relationship_write_tests.rs::a_target_of_another_type_is_refused_naming_it`, `tests/relationship_delete_tests.rs::a_desk_another_teacher_has_is_refused_and_nothing_is_written` |
| K30 | Core `@context` only (no caller context in body or `Link`) | `context_guard::check_link`, `check_body`, called `src/app.rs:952`, `:953` | not needed for engine rules: names are read "in the CORE `@context` and never in the request's own" (`ADR-0020:112-123`) | n/a | covered by the seam's design | `tests/context_guard_route_tests.rs::an_authenticated_write_with_a_context_of_its_own_is_refused_the_same_way` |
| K31 | Form creates (EP-97): minted id, one entity per create, daily cap per form and per caller | `creates::minted` `src/app.rs:974`; `DailyCreates::reserve` `src/app.rs:1273` | none (body rewrite, quota) | n/a | STAYS (D13) | `tests/form_creates_tests.rs::a_caller_chosen_id_is_not_used`, `tests/form_creates_tests.rs::one_sender_takes_a_tenth_of_the_day_and_the_form_stays_open_to_others` |
| K32 | Domain verification gate on writes (PF-41) | `DomainGate::refusal` `src/app.rs:834`; `src/domain_gate.rs` | none needed (deployment state, not data) | n/a | STAYS (D13) | `tests/domain_gate_tests.rs::enforce_refuses_a_write_until_the_domain_is_verified_and_never_asks_the_broker` |
| K33 | Federation identity: a space registered with `caller` identity answers 501 | `caller_identity_members` `src/app.rs:847` | none (token exchange) | n/a | STAYS (D13) | unknown |
| K34 | Audience (EP-14, EP-15) and representations served (EP-05) | `Endpoint::admits` `src/resolver.rs:212`, `serves` `:205`; `admit_by` `src/app.rs:3244` | engine-internal for audience (`Deny`); representations are surface features | n/a | audience: covered (engine data); representations: per surface feature | `tests/admits_refusal_log_tests.rs::a_token_naming_no_admitted_project_is_refused_and_the_refusal_is_logged`, `tests/edge_app_file_geojson_tests.rs::an_unknown_slug_and_a_representation_not_served_answer_the_same_bytes` |
| K35 | Rate limits per endpoint and caller; hub per subject | `rate_limit::enforce` `src/middleware/rate_limit.rs:202`; `HUB_RATE_LIMIT` `src/app.rs:139` | none | n/a | STAYS (D13) | `tests/rate_limit_tests.rs::one_callers_burst_is_not_another_callers` |
| K36 | Tenant pinned from the endpoint; client identity and tenancy headers stripped first | `tenancy::strip_client_headers` `src/middleware/tenancy.rs:33`, `pin_tenant` `:46` | the broker reads `NGSILD-Tenant` and the subject headers it is given (`policy.rs:490`) | n/a | STAYS (D13) | `tests/attack_foreign_space_ids_tests.rs::a_forged_tenant_header_on_a_write_is_replaced_by_the_endpoints_own_space` |
| K37 | Answer scrub: tenant removed, restricted signal removed unless asked, no shared cache, `describedby` links | `response::scrub` `src/middleware/response.rs:48` | none | n/a | STAYS (D13) | `tests/edge_app_file_zip_tests.rs::a_forged_tenant_neither_reaches_the_broker_nor_comes_back` |
| K38 | Data-space agreements: transfer token checks and ODRL agreements compiled into policies | `src/auth/dataspace_token.rs:172`; `odrl_compiler::compile` `src/federation/odrl_compiler.rs:97` | token: none (authn); compiled policies are engine data | n/a | token STAYS; compiled rules feed the engine | `tests/dataspace_token_tests.rs::a_transfer_token_minted_for_another_endpoint_is_refused`, `tests/odrl_compiler_tests.rs` (file) |
| K39 | Revocation: the rule tables are re-read every second | `reaper::INTERVAL` `src/pdp/reaper.rs:28` | ADR-0014 rule 3: "Configuration is data, code is compile-time ... may be reloaded at runtime from the stores the broker already has" (`ADR-0014:45-49`) | n/a | MOVES with the engine (its data source becomes Postgres or NATS KV) | `tests/reaper_tests.rs` (file; test name not read) |
| K40 | Query validation the gateway does before the PDP: `malformed`, `unselected` (400 for a query with no selector) | `query::malformed` `src/query.rs:64`, `unselected` `:312`; `src/app.rs:788`, `:797` | none needed: the broker validates 5.7.2 itself | n/a | MOVES (deleted: the broker's own 400) | `tests/edge_app_ngsi_ld_pipeline_tests.rs` (file; test name unknown) |
| K41 | Broker failure masking: a broker 5xx body is logged, not served; a broker 404 on a read becomes the gateway's own 404 | `project_answer` `src/app.rs:2850`, `:2867` | none needed in-process (the broker renders its own ProblemDetails) | n/a | MOVES (deleted) | `tests/download_upstream_failure_tests.rs` (file) |
| K42 | MCP elicitation for a destructive tool (`create_subscription`) | `src/mcp/elicitation.rs` | none (MCP protocol) | n/a | MOVES with the MCP surface | `tests/mcp_hub_tests.rs::an_elicitation_asked_on_one_endpoint_does_not_confirm_a_call_on_another` |

```cypher
// the PDP's own functions and who calls them from outside pdp/ (rule-kind call sites)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path STARTS WITH 'context_gateway::pdp::' AND c.package='context-gateway' AND c.is_test=false
  AND NOT c.path STARTS WITH 'context_gateway::pdp::'
RETURN f.path, collect(DISTINCT c.path + ':' + toString(r.line)) ORDER BY f.path;

// the serve_ngsi_ld pipeline in call order (rule-kind order of evaluation)
MATCH (h:Symbol {path:'context_gateway::app::serve_ngsi_ld'})-[r:CALLS]->(f:Symbol)
WHERE f.package='context-gateway'
RETURN f.path, min(r.line) AS line ORDER BY line;

// the seam's call sites in Antares (the clause each handler gates under)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol {path:'antares_api::policy::gate'}) WHERE c.is_test=false
RETURN c.path, c.file, r.line ORDER BY c.file, r.line;
```

### 3.1 Gaps: what has no `Filter` equivalent today, and what would be needed

| Gap | Rule kinds | What is missing | What would be needed (proposal, for T-3443) |
|---|---|---|---|
| G1 | K7, K17, K18, K20 | `Filter` cannot narrow entity types. `Operation.types` is read, but a query naming no type (or `q` alone) would return every type. | `Filter.types: Vec<String>` (expanded IRIs), intersected with the request's types in the store push-down; an empty intersection is G7's "nothing". Also compared by full IRI, not by the local term the gateway uses (`projection::term`, `src/pdp/projection.rs:249`, section 6 S9). |
| G2 | K8, K20 | No id or `idPattern` narrowing. | `Filter.id_patterns` (anchored, conjoined with the request's own `id`/`idPattern`), pushed down; bounded pattern size and evaluation (the gateway refuses catastrophic patterns: `tests/mcp_read_security_tests.rs::an_oversized_or_catastrophic_id_pattern_never_reaches_the_broker`). |
| G3 | K10 | `Filter.q` and `Filter.scope_q` are conjoined separately, so two policies `(q1 ∧ s1) ∨ (q2 ∧ s2)` can only be written as `(q1 ∨ q2) ∧ (s1 ∨ s2)`, which is wider: the cross-policy bleed the gateway's scope folding exists to prevent (`src/pdp/scope_folding.rs:1`). | A disjunction of branches, `Filter.any: Vec<Branch { q, scope_q, types, id_patterns, geo }>`, each branch conjoined inside and OR-ed whole on the `antares-ql` AST; or a `q` grammar that can test `scope` (whether `antares-ql` accepts `scope` in `q`: unknown). |
| G4 | K17, K18 | `pick`/`omit` are one list for the whole answer; MP-02 needs slots per type. | `Filter.pick_by_type: Map<type IRI, Vec<attr IRI>>`, applied per entity by its own types, at every join level (`ADR-0020:86-93`). Built as `Filter.rules` (types + condition + attributes, union over the rules an Entity matches), ADR-0023 rule 11, T-3528. |
| G5 | K12, K17, K20 | No geo narrowing. | `Filter.geo: Vec<GeoQuery>` (union of grant areas), intersected with the request's `geoQ` in the store; the `geoproperty` choice judged by the engine from `Operation.geo`. |
| G6 | K13, K17 | No temporal narrowing of 5.7.3/5.7.4 (11.3.2/11.3.3). | `Filter.temporal: Vec<Window>` applied to attribute instances by the temporal store, so no hull is fetched and no gap is filtered afterwards. |
| G7 | K14, K16, K18 | No way to say "the answer is nothing" (200 `[]`, 404 on retrieve) without asking the store. | `Filter.nothing: bool` (or a `Decision::Empty`), served as an empty list for queries and `ResourceNotFound` for 5.7.1/5.7.3, with no store call (no timing difference between empty and denied). |
| G8 | K18 | No advisory beside the data except `restricted`. | Either drop the dropped-types warning (owner decision O-4) or add `Filter.warning: Option<String>` sent as `NGSILD-Warning` (TS 104 176 header; the gateway already uses that name, `src/middleware/response.rs:27`). |
| G9 | K23 | One `Decision` per request: a batch (10.3.x) is allowed or denied whole. | `Decision::Partial { refused: Vec<(id, reason)> }` honoured by the batch handlers as per-entity errors in the 10.3 BatchOperationResult, or accept whole-batch refusal (behaviour change, O-5). |
| G10 | K21 | 5.7.5 to 5.7.10 (10.6.3 to 10.6.8) are outside `FILTERABLE`, so a narrowed engine answer is a `Deny`. | Extend `FILTERABLE` with the six discovery clauses and narrow their documents by `Filter.types` and the attribute lists by `pick`/`omit`/`pick_by_type`; an addressed name outside them is `ResourceNotFound`, decided before the fold. |
| G11 | K25 | `NotifyDecision::Filter` projects members only; it cannot remove one entity of `data` by type, id, area or a re-checked `q`. The engine can mutate `notification` itself (`&mut Value`, `policy.rs:323`), but `run_policy_contract` does not hold it to "never widens" there. | `NotifyDecision::Filter` gains entity selection (G1, G2, G5 members, evaluated per entity of `data`), with the 5.8.6 / 10.5.7 count rules for a notification left empty (same as `Drop`). |
| G12 | K24 | The engine never sees the stored entity, so a grant that depends on stored state cannot be decided. | `Decision::AllowIf(Filter)` on writes: the store applies the write only if the stored entity matches the filter, inside the write transaction (no read-then-If-Match race), answering `ResourceNotFound` or 403 otherwise. |
| G13 | R11 | No dry-run: the endpoint view cannot be evaluated with a draft projection. | A broker-side preview call of the endpoint-view surface that takes a candidate view definition, admitted only for the space's canonical audience (SP-06, SP-11). |
| G14 | (all) | The gateway's restricted signal is opt-in (`src/middleware/response.rs:37`); Antares sends `Antares-Results-Restricted` whenever the engine sets `restricted` (`policy.rs:662`). | Decide whether the engine sets `restricted` only when the request carried an opt-in subject header (engine-side), or accept the change (M1, O-3). |
| G15 | K1, K2, K34, K38 | The broker sees only `Subject.headers` named by `ANTARES_POLICY_SUBJECT_HEADERS` (`policy.rs:984`); nothing in the deployment sets it (`git grep ANTARES_POLICY` in joinedcontext-deployment: no hit). | The gateway (STAYS: token verification) forwards a verified, broker-only subject header (user, service account, groups, roles, DID, agreement, endpoint or view id), stripped from the client first (K36), and the broker is reachable only from the gateway (NetworkPolicy). |
| G16 | R1..R32 | The broker has no notion of an Endpoint slug, an audience or a view; the gateway resolves slug to space (`resolver::SlugResolver`, `src/resolver.rs`). | The endpoint-view surface crate names the view in its path or in the subject header; which of the two is T-3443's decision. |
| G17 | K29 | No way to ask, while serving a write, whether the subject may read the linked entity (GOAL point 3). | The write handlers call `policy::gate` for 5.7.1 (10.4.2) on each relationship target under the same subject before the commit; a target the subject cannot see is refused like a missing one. |
| G18 | K26 | A view of another model is a transform of query and answer, not a narrowing. | The endpoint-view surface crate runs the compiled Mapping IR (today `src/translators/view_mapping.rs`) on the `antares-ql` AST, not on the query string. |
| G19 | K28 | The seam may not rewrite a body; filling a missing `unitCode` is a rewrite. | Keep it in the gateway (STAYS, request transform) or add a schema-surface write hook (`on_request` cannot change the body today). Owner decision O-6. |

## 4. Move or stay

### 4.1 The ADR lines the decisions rest on

| Ref | Quote (verbatim) | Source |
|---|---|---|
| A1 | "**HTTP-level middleware is the tower `Layer` stack** — Rust already has the phase chain, and gateway concerns (authn, authz, rate limiting, request transforms) stay in the gateway in front of the broker, outside the conformance surface." | `docs/adr/ADR-0014-extension-hooks-model.md:50-53` (rule 4) |
| A2 | "**Configuration is data, code is compile-time.** Which hooks are active, and their per-scope settings, may be reloaded at runtime from the stores the broker already has (Postgres, NATS KV)" | `docs/adr/ADR-0014-extension-hooks-model.md:45-48` (rule 3) |
| B1 | "Reverses the \"no policy code in the broker\" reading of the standing PEP decision" | `docs/adr/ADR-0020-policy-seam.md:4-5` |
| B2 | "It stops holding where the decision is about data the gateway never sees as data:" | `ADR-0020:17-18` |
| B3 | "\"This consumer sees only entities in scope `/BB/Traffic`\" has to narrow `GET /entities` before the store answers. A gateway can only rewrite `q=` on the way in — string surgery on a grammar with `;`, `\|` and parentheses, where a wrong parenthesisation widens the result instead of narrowing it — or filter the response after the fact, which has already paid for the rows, and which leaves `NGSILD-Results-Count` and the paging links describing a result set the caller is not allowed to see." | `ADR-0020:20-27` |
| B4 | "5.8.6 delivery is broker-initiated: the notification goes from the broker to the subscriber's endpoint. A gateway in front of the broker is not on that path at all." | `ADR-0020:28-30` |
| B5 | "A narrowing reaches every document the answer carries, not only the one the request named: 4.5.23 `join=` appends Entities reached over a Relationship, and a member the subject may not see is not less hidden one hop away." | `ADR-0020:86-89` |
| B6 | "The conjunction is made on the `antares-ql` AST, never on the query string, so the precedence trap a gateway rewrite has to distribute around cannot occur." | `ADR-0020:81-83` |
| B7 | "It is read in the CORE `@context` and never in the request's own" | `ADR-0020:115` |
| B8 | "`Subject { tenant, headers }` carries the identity headers named by `ANTARES_POLICY_SUBJECT_HEADERS`, opaque to the broker — it parses no credential and validates no token." | `ADR-0020:57-59` |
| B9 | "The gateway keeps everything it had. Authentication, token validation, rate limiting and quotas do not move; the broker still takes no authorization decision of its own, it asks the engine and obeys." | `ADR-0020:229-231` |
| B10 | "A policy engine's rule language, identity model and data source are deployment choices." | `ADR-0020:193-194` |
| B11 | "Every other operation is performed whole — a create writes its Entity or does not — so a `Filter` there would be dropped, and the broker would do in full what the engine believed it had narrowed." | `ADR-0020:98-101` |

ADR-0014 rule 4 (A1) names "authz" among the gateway concerns; ADR-0020 (B1, B2) takes the data-shaped part of authorization into the broker and leaves the rest (B9). The decisions below read A1 as binding for identity, quotas and transforms of the request line, and B2 to B5 as binding for every decision about which entities, members or notifications an answer holds. Where the GOAL names a surface as a broker feature (`tasks/GOAL-antares-surfaces.md` point 2: endpoint views, OData, MCP, LinkML schema surface, WebSocket notifications), the surface moves even where it carries no data decision of its own.

### 4.2 Per route

| Decision id | Routes | Decision | Reason |
|---|---|---|---|
| D1 | R1, R20, R21, R22, R23 (endpoint and space DCAT records, catalogue feeds, `/cs` catalogue) | STAYS | Built from the manifest repository, no broker call (2.1); they describe the platform's registry, which is "outside the conformance surface" (A1). They read the grants only to say whether a record is restricted and which types it advertises, so after the move they read that from the engine's access description (D4) and keep no evaluator of their own. |
| D2 | R3, R24 and all 2.2 rows | MOVES (endpoint-view surface crate + joinedcontext engine addon) | Every narrowing here is the data-shaped case of B2/B3: types, ids, `q`, scopes, geo and time narrow the query before the store answers, joins are B5, and the gateway's own after-the-fact filtering is the one B3 names (`project_answer` removes `NGSILD-Results-Count`, `src/app.rs:2933`). `conjoin` builds the narrowed `q` as a string (`src/pdp/evaluator.rs:521`), the rewrite B6 replaces. |
| D3 | R2, R31 (NGSI-LD entry document, OpenAPI of the read surface) | MOVES (endpoint-view surface) | They describe what the view serves to this caller, computed from the same grants (`schema::visible`); the gateway's own rule is "discovery can never drift from enforcement" (`src/app.rs:613-616`), which holds only if they move with the enforcement (B2). |
| D4 | R4, R10 (access document, AuthZEN check) and `describe_access` | MOVES (joinedcontext engine addon) | They render the engine's rule language (K1 to K13) and must stay one implementation with the enforcement (`src/handlers/access_odrl.rs:6`: "Two implementations of \"what may this caller do\" would be the bug (EP-60)"); the rule language is the engine's (B10). |
| D5 | R5, R25 and the 2.3 tools | MOVES (MCP surface crate) | GOAL point 2 names MCP as a broker feature and point 3 makes every surface operation an NGSI-LD operation through the seam; the tools already are NGSI-LD requests (`ngsi_ld_request`, `src/app.rs:621`), so their narrowing is D2's (B2). |
| D6 | R6, R8 (MCP hub, named MCP servers) | STAYS, owner decision O-2 | Both list the Endpoints a token reaches across projects (`src/mcp/hub.rs:1-8`, `src/mcp/server.rs:1-8`) from token audiences and `McpServer` manifests: an identity question the broker cannot answer, since its subject is "opaque to the broker" (B8) and authn stays (A1, B9). Each tool call they forward goes to the broker's MCP surface (D5) instead of the gateway's façade. |
| D7 | R7, R9, R29 (RFC 9728 protected-resource metadata) | STAYS | OAuth discovery for clients that must fetch a token: authentication (A1, B9). |
| D8 | R11 (filter preview) | MOVES (endpoint-view surface), GAP G13 | It runs the view pipeline with a draft projection (`src/handlers/preview.rs:4-8`); once the pipeline is the broker's, a gateway copy would be a second implementation (B2). |
| D9 | R12 and K25 (notification egress) | MOVES (`pre_notify` in the engine addon) | B4: the gateway is on the delivery path today only because it rewrites `notification.endpoint.uri` to itself; the broker's `__subject` and `pre_notify` (`ADR-0020:150-157`) decide every delivery for the subscriber. The sealed subject (`src/egress/subject.rs`) and `JC_GATEWAY_DELIVERY_KEY` go with it. GAP G11. |
| D10 | R13 to R19, R27 (`file.*`, `dump/`, OGC Features, SensorThings) | STAYS, re-based; O-1 (OGC Features: MOVES, ADR-0029) | The GOAL does not list them as broker features; as translators they are "request transforms ... outside the conformance surface" (A1). Their narrowing is D2's and moves (B3): they read the broker's endpoint view with the forwarded subject and stop calling `Pdp::decide`, `projection` and `geo` (`src/handlers/reads.rs`). |
| D11 | R26, R28, R30, R32 (`schema/index.json`, `schema/{version}/{artifact}`) and `describe_schema` | MOVES (LinkML schema surface crate) | GOAL point 2 names the LinkML schema surface; its class and slot narrowing is the projection of the same engine (B2). |
| D12 | R33 to R36, L3 (probes, metrics, fallback, telemetry) | STAYS | Process-level, no data (A1). The broker keeps its own `/q/health` (`ADR-0020:285`) and `/q` stays "the gateway's to protect" (`ADR-0020:184-185`). |
| D13 | L1, L2, and K31 to K38 (rate limits, form quotas, domain gate, federation identity, audience, tenancy, scrub, data-space tokens) | STAYS | A1 ("authn, authz, rate limiting, request transforms") and B9 ("Authentication, token validation, rate limiting and quotas do not move"). The tenant and subject headers are what the gateway hands the broker (G15). |

### 4.3 Per rule kind

| Rule kind | Decision | Reason (ref) |
|---|---|---|
| K1 assignee, K2 endpoint roles | MOVES as engine rule data; the identity they match on STAYS | The engine matches the assignee in its own rule language (B10); the broker sees only forwarded headers (B8), so the gateway keeps verifying the token and forwards the verified principal, groups and roles (G15, B9). |
| K3 operations | MOVES | `Allow`/`Deny` on `Operation.clause` is the seam's own verdict (`ADR-0020:60-62`). |
| K4 validity window | MOVES | engine rule data (B10). |
| K5 prohibition first | MOVES | engine-internal, answered `Deny`. |
| K6 default deny | MOVES | engine-internal; the shipped engine stays `AllowAll` (`ADR-0020:192-196`). |
| K7 types | MOVES, G1 | B3. |
| K8 ids and patterns | MOVES, G2 | B3. |
| K9 attribute whitelist | MOVES | `Filter.pick` (`ADR-0020:77`). |
| K10 per-policy `q` with folded scopes | MOVES, G3 | B6. |
| K11 write scope tree | MOVES | engine `Deny` from the expanded body; writes are whole (B11). |
| K12 geo | MOVES, G5 | B3. |
| K13 temporal clamp | MOVES, G6; the `lastN` cap STAYS | clamp: B3; the cap bounds what one request costs the shared broker (`src/query.rs:429`), a quota (B9). |
| K14 empty answer | MOVES, G7 | B3 (answering before the store is asked is the store's decision once it narrows). |
| K15 restricted signal | MOVES; the opt-in is O-3 | `Filter.restricted` (`ADR-0020:78`); header name changes (M1). |
| K16 hidden attributes | MOVES | `Filter.omit` plus G7. |
| K17 model projection | MOVES, G1 G4 G5 G6 | B3. |
| K18 filter oracle | MOVES, G8 | the engine reads `Operation.q` (expanded, `policy.rs:105`); B2. |
| K19 answer judged after the broker | MOVES (deleted) | B3: a narrowing in the store makes the after-the-fact check and the count removal unnecessary. |
| K20 joins | MOVES; the `joinLevel` cap STAYS | B5; cap: quota (B9). |
| K21 discovery narrowing | MOVES, G10 | B2. |
| K22 write guard | MOVES | B11: the engine decides `Allow`/`Deny` from the whole body. |
| K23 batch division | MOVES, G9 | B11 (today a narrowing of a batch is a `Deny`). |
| K24 conditional write | MOVES, G12 | the stored entity is data only the broker holds (B2); a read-then-`If-Match` from outside is the race the store transaction removes. |
| K25 subscription narrowing and delivery | MOVES, G11 | B4. |
| K26 view mapping | MOVES (endpoint-view surface), G18 | `invert_query` rewrites the query string (`src/app.rs:2183`), the case B6 excludes. |
| K27 declared types | MOVES | engine `Deny` on `Operation.types` for writes; model data is configuration (A2). |
| K28 units | STAYS, O-6 | a body rewrite is a "request transform" (A1); the seam cannot rewrite a body (G19). |
| K29 relationships | MOVES, G17 | GOAL point 3 (relationship writes checked against the linked entity's visibility); only the broker can see the target under the same subject (B2). |
| K30 core-context guard | MOVES (deleted) | the seam reads rule names in the core context whatever the caller sends (B7); the guard's reason is gone once the rules are evaluated in the broker. A view's own names (projection, mapping) are evaluated the same way. |
| K31 form creates (minted id, daily cap) | STAYS | quota and request transform (A1, B9). |
| K32 domain gate | STAYS | deployment state of the Organization, not data; a gate in front of the broker (A1). |
| K33 federation caller identity (501) | STAYS | token exchange is authentication (B9). |
| K34 audience, representations | audience STAYS (token audience check, B9); representations follow their surface (D2 to D11) | A1. |
| K35 rate limits | STAYS | B9. |
| K36 tenancy pinning and stripping | STAYS | A1 (request transform); it is also what makes G15 safe. |
| K37 answer scrub | STAYS | header sanitising of the gateway's own answers (A1). |
| K38 data-space tokens; ODRL compiler | token STAYS (B9); the compiled policies MOVE as engine rule data (B10) | |
| K39 revocation reload | MOVES with the engine | A2: the engine's rules reload from the broker's stores, not from the gateway's file mount. |
| K40 query validation | MOVES (deleted) | the broker answers 5.7.2's 400 itself; a second validator is a second opinion on the grammar (B6). |
| K41 broker-failure masking | MOVES (deleted) | in-process there is no upstream text to mask; the broker renders its own errors. |
| K42 MCP elicitation | MOVES with D5 | GOAL point 2. |

## 5. Migration risk: callers that depend on gateway-only behaviour

Search: `git grep -n -E '/api/endpoint|/cs/'` (and the header names `NGSILD-Results-Restricted`, `NGSILD-Warning`, `NGSILD-Results-Count`) over joinedcontext-portal `src/`, `sdk/src/`, `ui/src/`, `apps/`, over the other platform crates (`crates/assistant`, `crates/agent-proxy`, `crates/functions`), and over joinedcontext-deployment `components/`; test files, fixtures and `e2e/` mocks are left out unless they are the only user. Header search result: no non-test code in the portal reads `NGSILD-Results-Restricted` or `NGSILD-Warning`; the SDK reads `NGSILD-Results-Count` (row M2). "Gateway-only behaviour" names what the caller relies on that the broker does not do today.

| # | Caller (repo: file:line) | Gateway path or header it uses | Gateway-only behaviour it depends on | Risk when the surface moves |
|---|---|---|---|---|
| M1 | (all callers that opt in; none found in the portal) | request header `NGSILD-Results-Restricted: true` (`src/middleware/response.rs:37`) | the restricted signal is opt-in and named `NGSILD-Results-Restricted` | Antares sends `Antares-Results-Restricted` unasked (`policy.rs:662`, `ADR-0020:205-220`); any external client keyed on the old name loses the signal (O-3). |
| M2 | portal: `sdk/src/grid/source.ts:71` | `ngsild-results-count` | a narrowed answer carries no count (`src/app.rs:2933`), so the grid pages by "page came back full" | a broker narrowing in the store returns a correct count; the grid then shows totals. Not a leak (the count is over the narrowed set) but a visible change; the comment at `sdk/src/sdk/transport.ts:17` documents the old rule. |
| M3 | portal: `sdk/src/grid/source.ts:230`, `sdk/src/ngsi.ts:88`, `sdk/src/sdk/client.ts:133`, `:279`, `sdk/src/write.ts:147` | `/api/endpoint/{slug}/ngsi-ld/v1/...` | slug addressing, admission by token audience, the PDP | the broker has no slug (G16): the SDK base URL changes, or the gateway keeps the slug path as a pass-through (decision for T-3443). Every App built on the SDK follows (M20). |
| M4 | portal: `sdk/src/grid/source.ts:235`, `ui/src/api/gateway.ts:27`, `ui/src/pages/endpoints/FilterProof.tsx:146`, `ui/src/pages/pipelines/SpaceChanges.tsx:24`, `ui/src/pages/spaces/FormView.tsx:253`, `ui/src/pages/spaces/SpaceInside.tsx:561`, `ui/src/pages/spaces/importRows.ts:596` | `/cs/{space}/ngsi-ld/v1/...` | space-name addressing with the canonical audience (SP-01, SP-03) | as M3; the space name is the tenant today (`src/resolver.rs` `space`), so `/cs/{space}` maps onto `NGSILD-Tenant` most directly. |
| M5 | portal: `src/reconciler/subscriptions.rs:266`, `:287` | POST/PATCH `/cs/{space}/ngsi-ld/v1/subscriptions` | the gateway narrows the subscription and rewrites `notification.endpoint.uri` to R12 (`src/app.rs:2527`) | after the move the broker delivers directly under `__subject`; the deployment's broker egress rule allows only the gateway (`joinedcontext-deployment: components/context-broker/networkpolicies.yaml:31-42`) and must open to subscriber hosts with the broker's own egress check (ADR-0019), or every delivery fails. |
| M6 | portal: `src/reconciler/drift.rs:166`, `:191` | GET `/cs/{space}/ngsi-ld/v1/entities[/{id}]` | read through the space surface with the Portal's token | path change (M4). |
| M7 | portal: `ui/src/pages/apps/EntityWriteCard.tsx:89`, `ui/src/pages/spaces/FormView.tsx:253`, `ui/src/pages/spaces/importRows.ts:596` | writes through the gateway | 400 answers of the gateway's model checks (declared types K27, units K28, relationships K29, write guard refusals naming the member) | an engine `Deny` is a 403 `urn:antares:error:AccessDenied` (`policy.rs:976`); UI messages keyed on the gateway's 400 `code` slug (`src/app.rs:726` keeps it as `code`) change. Which UI code reads `code`: unknown. |
| M8 | portal: `ui/src/components/dashboards/TemporalChart.tsx:86` | `/api/endpoint/{slug}/ngsi-ld/v1/temporal/entities/{id}` | `upstream_by_id` sends no `type` (`src/query.rs:364`) and clamps windows (K13) | windows narrowed by the store (G6) instead of after the fact. |
| M9 | portal: `ui/src/pages/apps/KpiCard.tsx:149`, `ui/src/pages/pipelines/PipelineStudio.tsx:267`, `:465`, `ui/src/pages/apps/ChangeFollowCard.tsx:96`, `ui/src/pages/workspaces/copyIntoPreview.ts:39`, `:40` | `/api/endpoint/{slug}/ngsi-ld/v1/entities` | as M3 | as M3. |
| M10 | portal: `ui/src/pages/endpoints/FilterProof.tsx:196` | POST `/api/endpoint/{slug}/preview` | the draft-projection dry run (R11) | G13: no broker equivalent yet. |
| M11 | portal: `ui/src/components/entities/EndpointDataView.tsx:44`, `ui/src/pages/apps/AppGenerator.tsx:56`, `ui/src/pages/endpoints/SchemaProjectionPanel.tsx:133`, `sdk/src/sdk/client.ts:506`, `src/agents/oneshot/model.rs:526`, `:537` | `/api/endpoint/{slug}/schema/...`, `schema/index.json` | schema surface narrowed by the grants (R30, R32) | moves with the LinkML schema surface (D11): path and index format must be kept or every App generator run breaks. |
| M12 | portal: `src/agents/held.rs:132`, `ui/src/pages/access/EffectivePermissions.tsx:196`, `ui/src/components/entities/AccessPanel.tsx:64` | GET `/api/endpoint/{slug}/access` | the access document from the gateway's PDP (R4) | moves with the engine (D4); the assistant's held-run check reads it. |
| M13 | portal: `ui/src/pages/mcp/mcp.ts:181`; platform: `crates/assistant/src/chat/mcp.rs:55` | POST `/api/endpoint/{slug}/mcp` | the gateway's MCP façade and its tool names (2.3) | the broker's MCP surface must keep the tool names and argument schemas, or the assistant's chat and the Portal's MCP page break. |
| M14 | portal: `ui/src/components/endpoints/links.tsx:20-25`, `:56`, `:89`, `ui/src/routes/DashboardsPage.tsx:131`, `ui/src/pages/spaces/importRows.ts:647`, `ui/src/pages/endpoints/CatalogSection.tsx:70`; portal apps: `apps/banskabystrica-data/ui/src/datasets.ts:47`, `apps/bbsk-data/ui/src/datasets.ts:63`, `apps/helsinki-data/ui/src/datasets.ts:71`, `apps/praha-data/ui/src/datasets.ts:67` | `file.geojson`, `file.csv`, `file.xlsx`, `file.zip`, `ogc/features`, `sta/v1.1`, catalogue records | gateway translators (D10) | stay in the gateway (re-based): no path change, but each now depends on the broker view being correct. |
| M15 | portal: `src/api/catalogue.rs:803`, `:858`, `src/api/ckan.rs` (file), `src/apps/reconciler.rs:699`, `src/ops/previews.rs:273`, `ui/src/routes/EndpointsPage.tsx:1670` | the `/api/endpoint/{slug}/` URL shape itself | the record URL is the dataset's identity (DCAT) and the App's `JC_ENDPOINT_URL` | D1 keeps the record; the App URL must keep resolving. |
| M16 | portal: `src/reconciler/edge_file.rs:408`, `:464` | APISIX route `/api/endpoint/{slug}/*` generated per App | the gateway is the upstream of every App data call | the generated route must point at whatever serves the view after the move. |
| M17 | portal: `src/pipeline_expiry.rs:279`, `src/reconciler/streams.rs:1266`, `:1992` | `${JC_GATEWAY_URL}/api/endpoint/{slug}/ngsi-ld/v1/entityOperations/...` in generated Bento pipelines | batch division into 207 (K23), upsert through the write guard | G9: a whole-batch `Deny` instead of a 207 changes what a pipeline retries. |
| M18 | portal: `ui/src/api/pipelineTest.ts:76`, `ui/src/pages/apps/previewBridge.ts:101` | origin and prefix check `/api/endpoint/` | the gateway's path is the only data origin the UI trusts | path change (M3). |
| M19 | platform: `crates/agent-proxy/src/routes/data.rs:156`, `crates/functions/src/endpoint.rs:74`, `:107` | `{gateway}/api/endpoint/{slug}/...` | path confinement to one Endpoint (the functions test list at `crates/functions/src/endpoint.rs:165-185` probes traversal) | path change; the confinement then has to hold against the new path. |
| M20 | deployment: `components/gitea/apps/air-quality/src/lib.rs:42`; the Apps under `components/gitea/apps/*` (e.g. `air-weather-explorer/e2e/serve.ts:60`, `alerts-desk/e2e/serve.ts:64`) | `/api/endpoint/{slug}/` through the SDK | as M3 | every App is rebuilt against the new SDK base, or the gateway keeps the slug path. |
| M21 | deployment: `components/context-gateway/seed/helsinki/helsinki-datasource-vehicles-live.yaml:14`, `components/context-gateway/seed/helsinki/helsinki-pipeline-vehicles-reaper-bento.yaml:25` | hard-coded slugs under `/api/endpoint/{slug}/ngsi-ld/v1/...` | slug addressing | seeds rewritten with the new path. |
| M22 | portal: `src/config.rs:1249` (`JC_PORTAL_BROKER_URL`), `src/reconciler/registrations.rs` (`RegistrationSync`) | the broker directly, port 9090 allowed by `components/context-broker/networkpolicies.yaml:49-55` | the broker holds no policy, so the Portal's own calls are never narrowed | with an engine attached, the Portal's direct calls are judged too: the Portal needs its own subject header and rules, or its registration sync is denied. |

```text
GREP used (portal, non-test): git grep -n -E '/api/endpoint|"/cs/|`/cs/|/cs/\$|/cs/\{' -- '*.rs' '*.ts' '*.tsx' | grep -v -E '(^|/)tests?/|\.test\.|e2e|fixtures'
GREP used (headers):          git grep -n -i -E 'ngsild-warning|ngsild-results|results-restricted' -- '*.rs' '*.ts' '*.tsx'
GREP used (platform crates):  git grep -n -E '/api/endpoint|/cs/\{|/api/mcp' -- crates/assistant/src crates/agent-proxy/src crates/functions/src crates/jcctl/src crates/jc-app-sdk/src
```

## 6. Security: where the gateway, not the broker, is the only guard today

The broker runs `AllowAll` in every shipped build (`ADR-0020:194-196`), so each row below is enforced nowhere else. "Bypassed" means a request that reaches the broker without passing the gateway: from a pod the broker's NetworkPolicy admits (the gateway and the Portal, `joinedcontext-deployment: components/context-broker/networkpolicies.yaml:43-55`), or a broker exposed by mistake.

| # | Where (platform `file:line`) | What it guards | If the gateway is bypassed |
|---|---|---|---|
| S1 | `src/middleware/tenancy.rs:33` (`strip_client_headers`), `:46` (`pin_tenant`) | the tenant is the Endpoint's space, never the client's `NGSILD-Tenant` | any tenant is readable and writable by naming it. |
| S2 | `src/auth/token.rs` (`Verifier`), `app::authenticate_by` `src/app.rs:2979` | issuer, signature, expiry and audience of the token | the broker serves anyone; it validates no token (`ADR-0020:57-59`). |
| S3 | `app::admit_by` `src/app.rs:3244`; `Endpoint::admits` `src/resolver.rs:212` | Endpoint audience and allowed projects (EP-14, EP-15) | any project's space is reachable. |
| S4 | `src/app.rs:811` to `:827` (`decide`, `refused`) | operation grants, prohibitions, validity, default deny (K3 to K6) | every operation is allowed, deletes and purges included. |
| S5 | `query::upstream` `src/query.rs:341` | type, attribute, `q`, scope, geo and time narrowing (K7 to K13) | the whole tenant is served. |
| S6 | `app::project_answer` `src/app.rs:2834`; `projection::permitted` `src/pdp/projection.rs:212` | entities of an ungranted type or id or outside a granted area are dropped; the count is removed | other types and ids are served; the count leaks withheld totals. |
| S7 | `PolicyPdp::decide` `src/pdp/mod.rs:64` | the Endpoint's hidden attributes (EP-61) | hidden attributes are served. |
| S8 | `drop_types_that_may_not_be_filtered` `src/pdp/mod.rs:180` | inference of a value not served, by bisecting `q` (T-1862) | an attribute value that is not served is learnable by `q` on it. |
| S9 | `projection::term` `src/pdp/projection.rs:249`, used by `type_granted` `:229` | answer types compared by local name | a weakness inside the gateway, not only on bypass: an entity typed `https://other.example/Depot` passes a grant on `Depot`. The engine must compare expanded IRIs (G1). |
| S10 | `write_guard::check` `src/pdp/write_guard.rs:72`, `check_granted_id` `:86` | a write sits whole inside one grant (id, type, attributes, scope, location, no policy vocabulary) | writes land anywhere in the tenant. |
| S11 | `pdp::conditional` `src/pdp/conditional.rs:36`, `:44`, `:63` | grants that depend on stored state | entities outside such a grant are overwritten. |
| S12 | `HeldWrite`, `HeldDelete` (`src/app.rs:1345`, `:2049`), `src/relationships.rs` | relationship targets exist, have the right type and are readable by the writer; delete rules | dangling or out-of-view links, and an existence oracle through write errors. |
| S13 | `context_guard::check_link`, `check_body` (`src/app.rs:952`, `:953`) | a caller `@context` cannot remap the names a rule was written against | rules written as short names can be walked around; in the broker B7 removes the need, but only for names read through the seam. |
| S14 | `narrowed_subscription` `src/app.rs:2527`; `egress::notifications::deliver` `src/egress/notifications.rs:190`; `JC_GATEWAY_EGRESS_PRIVATE_HOSTS` | a subscription is narrowed and every delivery is re-decided for the subscriber, to a target the gateway allows | notifications carry unnarrowed entities to any URI the subscriber names; with the gateway as the only allowed egress (`networkpolicies.yaml:31-42`) the network still limits the target, the content not. |
| S15 | `vocabulary::reaches`, `narrow` (`src/pdp/vocabulary.rs:38`, `:115`) | discovery lists only reachable type and attribute names | `/types` and `/attributes` are a directory of the tenant. |
| S16 | `operations::operation_of` `src/operations.rs:16` with the 404 at `src/app.rs:781` | Context Source registrations and their subscriptions, `@context` management, EntityMaps, Snapshots, purge are unreachable through an Endpoint | a caller can register a Context Source (the broker then forwards to an address the caller chose; ADR-0020's 4.3.6.5 note is about exactly that), add or delete `@context` documents, or snapshot the tenant. The engine must deny every clause a view does not serve. |
| S17 | `LAST_N_CAP` `src/query.rs:444`, `JOIN_LEVEL_CAP` (`src/query.rs:434`), `MAX_SKIP` `src/app.rs:85`, request body limit `Gateway::max_request_body` `src/app.rs:380`, `MAX_BODY` `src/app.rs:57` | cost one request may impose on the shared broker | the broker's own bounds (`crate::bounds`, Antares) apply; whether they are as tight: unknown. |
| S18 | `rate_limit::enforce` `src/middleware/rate_limit.rs:202`; `DailyCreates::reserve` `src/app.rs:1273` | rate limits per caller, the form's daily creates | unlimited requests and creates. |
| S19 | `DomainGate::refusal` `src/app.rs:834` | writes wait for a verified domain (PF-41) | writes before verification. |
| S20 | view-endpoint read-only check `src/app.rs:774` | a view of another model takes no write | writes in the source model through a view's grant. |
| S21 | `undeclared_type` `src/app.rs:2509`; `filled_units` `src/app.rs:2091` | the space's model types and units | entities of undeclared types and wrong units are stored. |
| S22 | `response::scrub` `src/middleware/response.rs:48` | the tenant name and the unrequested restricted signal never leave | internal space names reach the caller if the broker echoes `NGSILD-Tenant` (whether it does on every answer: unknown). |
| S23 | `project_answer` `src/app.rs:2867` | a broker 5xx's own text (ids, query fragments) is not served | internal error text reaches the caller. |
| S24 | `conjoin` `src/pdp/evaluator.rs:535` | an unbalanced caller `q` is dropped, not wrapped, so it cannot regroup the grants' terms | a gateway-only rule: the caller's own filter silently disappears and the grants alone apply. The broker's AST conjunction (B6) makes this unnecessary; the broker refuses an unparsable `q` with 400 instead (behaviour change, harmless). |
| S25 | `egress::subject` (`src/egress/subject.rs`), `JC_GATEWAY_DELIVERY_KEY` | the subscriber's identity travels sealed through the broker | in the broker the subject is `__subject`, stripped from every representation and forward (`ADR-0020:150-157`); the seal is no longer needed. |
| S26 | the broker NetworkPolicy admits the Portal as well as the gateway (`components/context-broker/networkpolicies.yaml:49-52`) | none: the Portal is a second client that no gateway rule covers today | the Portal's direct calls (M22) bypass every row above by design today; once an engine is attached they are judged like any other caller. |

## 7. Removal inventory (GOAL point 11)

Owner addition of 2026-10-10: every MOVES row is deleted from joinedcontext-platform once its Antares replacement is live on `dev` and every caller uses it; no dual path, no fallback flag, no stub. One table per surface. Platform paths are under `crates/context-gateway/` unless named; deployment paths are joinedcontext-deployment; caller rows refer to section 5. "Tests that only cover the old path" lists test files whose subject is the deleted code, with sample test functions; the per-surface Cypher enumerates every test function that still reaches a deleted symbol, which is the list a removal task deletes. STAYS rows (section 4) are not listed. Order per surface, from the GOAL: switch the callers (a test per caller against Antares), delete, `dev` smoke green.

Shared preconditions for every surface: the subject header the gateway forwards (G15) and `ANTARES_POLICY_SUBJECT_HEADERS` set in the broker chart (no such key exists today: `git grep ANTARES_POLICY` in joinedcontext-deployment finds nothing); the joinedcontext policy engine addon built into the deployed broker image (`ADR-0020:192-196`).

### 7.1 NGSI-LD endpoint view (D2, D3, D8; rule kinds K3 to K27, K29, K30, K39 to K41)

| Item | Contents |
|---|---|
| (a) Files and symbols to delete | `src/app.rs`: `ngsi_ld` (:535), `ngsi_ld_entry` (:556), `space_ngsi_ld` (:594), `ngsi_ld_request` (:621), `ngsi_ld_error_type` (:679), `as_ngsi_ld_error` (:698), `serve_ngsi_ld` (:747 to :1340), `HeldWrite` (:1345) and `HeldDelete` with their impls, `storing` (:1631), `too_far` (:2080), `filled_units` and `with_filled_units` only if O-6 moves units, `as_geojson` (:2141), `invert_query` (:2183), `translated` (:2218), `accepted_payload`, `unsupported_media_type`, `refused`, `divides`, `batch_entry_id`, `judge_write` (:2325), `refuse_entity`, `refusal_entries`, `batch_result`, `merged_batch` (:2461), `undeclared_type` (:2509), `narrowed_subscription` (:2527, see 7.3), `narrowed_batch_query` (:2564), `attrs_of_the_selected_types`, `empty_list`, `empty_answer`, `warn_about_dropped_types` (:2773), `names_no_tenant`, `empty_space`, `project_answer` (:2834). Whole files: `src/operations.rs`, `src/query.rs` (except what the re-based translators still call: `query::encode`/`decode`, to be checked with the Cypher below), `src/context_guard.rs`, `src/relationships.rs`, `src/translators/view_mapping.rs`, `src/handlers/preview.rs`, `src/handlers/openapi_doc.rs`, `src/pdp/` whole directory (`conditional.rs`, `evaluator.rs`, `geo.rs`, `mod.rs`, `projection.rs`, `reaper.rs` (the rule reload; the endpoint table reload for D1 stays and is moved to `resolver`/`store`), `scope_folding.rs`, `temporal.rs`, `vocabulary.rs`, `write_guard.rs`), `src/units.rs` if O-6 moves units. `Gateway::pdp` (`src/app.rs:94`) and every `Box<dyn Pdp>` construction in `src/main.rs`. `jc-core`: nothing (the `Operation`, `PolicySpec` kinds are manifest kinds the engine addon still reads). |
| (b) Routes that go away | R2 `/api/endpoint/{slug}/ngsi-ld/v1[/]` (`src/app.rs:425`, `:426`), R3 `.../ngsi-ld/v1/{*rest}` (`:427`), R24 `/cs/{space}/ngsi-ld/v1/{*rest}` (`:475`), R11 `.../preview` (`:443`), R31 `.../openapi.json` (`:492`). Whether the gateway keeps a path-preserving pass-through for the slug URL is T-3443's decision (G16, M3); GOAL point 11 forbids it as a fallback, so it may only exist as the one path. |
| (c) Tests that only cover the old path | `tests/`: `answer_type_guard_tests.rs`, `batch_query_body_tests.rs`, `conditional_write_tests.rs`, `context_guard_route_tests.rs`, `declared_types_tests.rs`, `edge_app_ngsi_ld_pipeline_tests.rs`, `edge_app_ngsi_ld_tests.rs` (`a_refusal_is_rendered_the_way_an_ngsi_ld_client_reads_one`), `edge_app_space_ngsi_ld_tests.rs`, `edge_gateway_conditional_tests.rs`, `edge_gateway_evaluator_tests.rs`, `edge_gateway_temporal_tests.rs`, `edge_gateway_view_mapping_tests.rs`, `edge_gateway_write_guard_tests.rs`, `edge_pdp_*_tests.rs` (20 files), `edge_query_decode_tests.rs`, `edge_query_upstream_tests.rs`, `empty_space_tests.rs`, `filter_oracle_tests.rs` (`bisecting_a_hidden_value_learns_nothing`), `filter_preview_tests.rs`, `foreign_model_tests.rs`, `geo_parameters_tests.rs`, `geo_tests.rs`, `identity_attrs_tests.rs`, `model_projection_tests.rs`, `ngsi_ld_entry_tests.rs`, `ngsi_ld_errors_tests.rs`, `ngsi_ld_geojson_tests.rs`, `openapi_doc_tests.rs`, `pdp_tests.rs`, `policy_ref_tests.rs`, `preview_tests.rs`, `projection_tests.rs`, `reaper_tests.rs`, `relationship_delete_tests.rs`, `relationship_write_tests.rs`, `scope_folding_tests.rs`, `temporal_tests.rs`, `type_of_answer_tests.rs`, `types_attributes_tests.rs`, `unit_code_tests.rs` (if O-6 moves units), `view_mapping_tests.rs`, `write_guard_route_tests.rs`, `write_guard_tests.rs`; the NGSI-LD probes inside `leak_sweep_tests.rs::every_representation_and_tool_has_a_probe` and `parity_integration_tests.rs` (both also cover STAYS surfaces: edit, not delete). Their security cases are the input of T-3444's test plan before deletion: each becomes an Antares test first (GOAL point 5). |
| (d) Deployment keys | `components/context-gateway/values/gateway/base-values.yaml.gotmpl:97` `JC_GATEWAY_REPO_DIR`, `:101` `JC_GATEWAY_PROJECTS_DIR`, `:104` `JC_GATEWAY_ASSEMBLY_DIR` (policies source: they move to the engine's data source, A2; the endpoint table for D1 still needs a source, so these stay unless D1 reads from the Portal; decision for T-3443); `:109` `JC_GATEWAY_PREVIEWS_URL`, `:110` `JC_GATEWAY_PREVIEWS_DIR` (workspace previews served through the NGSI-LD pipeline, `src/previews.rs`); `components/context-gateway/apisix-routes.yaml:6`, `:29`, `:65`, `:86` (`/cs/*`), `:126`, `:184` (`/api/endpoint/*`): re-pointed, not deleted, while R1, R20 to R23 and the translators stay; `components/context-broker/networkpolicies.yaml:43-55` (ingress: the edge or the gateway must reach the broker's view surface). New: `ANTARES_POLICY_SUBJECT_HEADERS` and the engine's rule source in the broker chart. |
| (e) Callers to switch | M2, M3, M4, M6, M7, M8, M9, M10 (preview), M15, M16, M17, M18, M19, M20, M21 (section 5, with `file:line`). |

```cypher
// zero remaining references: run in joinedcontext-platform after the deletion, expect 0 rows each
MATCH (s:Symbol) WHERE s.package='context-gateway' AND (
     s.path STARTS WITH 'context_gateway::pdp::'
  OR s.path STARTS WITH 'context_gateway::operations::'
  OR s.path STARTS WITH 'context_gateway::context_guard::'
  OR s.path STARTS WITH 'context_gateway::relationships::'
  OR s.path STARTS WITH 'context_gateway::translators::view_mapping::'
  OR s.path STARTS WITH 'context_gateway::handlers::preview::'
  OR s.path STARTS WITH 'context_gateway::handlers::openapi_doc::'
  OR s.path IN ['context_gateway::app::serve_ngsi_ld','context_gateway::app::ngsi_ld',
     'context_gateway::app::ngsi_ld_entry','context_gateway::app::space_ngsi_ld',
     'context_gateway::app::ngsi_ld_request','context_gateway::app::project_answer',
     'context_gateway::app::judge_write','context_gateway::app::narrowed_batch_query',
     'context_gateway::app::invert_query','context_gateway::app::as_ngsi_ld_error'])
RETURN s.path, s.file, s.line;
// every remaining caller of the old verdict, from any package
MATCH (c:Symbol)-[:CALLS|REFERENCES]->(f:Symbol)
WHERE f.path IN ['context_gateway::pdp::Pdp::decide','context_gateway::pdp::evaluator::evaluate',
                 'context_gateway::pdp::evaluator::effective']
RETURN c.package, c.path;
// tests still reaching a deleted symbol (the list for (c)), run BEFORE the deletion
MATCH (t:Symbol)-[:CALLS*1..4]->(f:Symbol)
WHERE t.is_test=true AND (f.path STARTS WITH 'context_gateway::pdp::'
   OR f.path STARTS WITH 'context_gateway::operations::' OR f.path='context_gateway::app::serve_ngsi_ld')
RETURN DISTINCT t.file, t.name ORDER BY t.file;
GREP ngsi-ld/v1 IN crates/context-gateway/src/**;
```

### 7.2 MCP endpoint surface (D5; K42)

| Item | Contents |
|---|---|
| (a) Files and symbols to delete | `src/mcp/endpoint_facade.rs` (whole; except what the hub and named servers still need if O-2 keeps them, see the Cypher), `src/mcp/elicitation.rs`, `src/app.rs`: `mcp_message` (:3270), `mcp_answer` (:3301), `space_mcp_message` (:3328), `Gateway::elicitations` (field, `src/app.rs:124`). |
| (b) Routes that go away | R5 `POST /api/endpoint/{slug}/mcp` (`src/app.rs:429`), R25 `POST /cs/{space}/mcp` (`:476`). R6 to R9 stay (D6, D7) and forward to the broker's MCP surface. |
| (c) Tests that only cover the old path | `tests/mcp_facade_tests.rs` (`the_handshake_names_the_protocol_the_server_and_the_one_space_it_serves`), `tests/mcp_tools_tests.rs` (`the_catalogue_covers_the_read_surface_and_annotates_every_tool`), `tests/mcp_read_security_tests.rs` (`a_join_never_inlines_an_ungranted_entity`), `tests/mcp_schema_tests.rs`, `tests/mcp_default_tests.rs`, `tests/edge_app_mcp_message_tests.rs`, `tests/edge_app_space_mcp_message_tests.rs`; `tests/mcp_hub_tests.rs` and `tests/mcp_server_tests.rs` stay but switch their member calls to a broker stub. |
| (d) Deployment keys | `components/context-gateway/keycloak-clients.yaml:99` (the App asks for `resource=https://{host}/api/endpoint/{slug}/mcp`): the resource indicator changes with the path; `components/context-gateway/apisix-routes.yaml:126`, `:184` (the `/api/endpoint/*` routes that carry `/mcp`). |
| (e) Callers to switch | M13 (`ui/src/pages/mcp/mcp.ts:181`, `crates/assistant/src/chat/mcp.rs:55`), and the hub/server forwarders R6, R8 (`src/mcp/hub.rs`, `src/mcp/server.rs`, calls into `endpoint_facade` listed by the Cypher). |

```cypher
MATCH (s:Symbol) WHERE s.path STARTS WITH 'context_gateway::mcp::endpoint_facade::'
   OR s.path STARTS WITH 'context_gateway::mcp::elicitation::'
   OR s.path IN ['context_gateway::app::mcp_message','context_gateway::app::mcp_answer','context_gateway::app::space_mcp_message']
RETURN s.path, s.file, s.line;
// who still calls the façade (expect only the broker MCP client after the switch)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol) WHERE f.path STARTS WITH 'context_gateway::mcp::endpoint_facade'
  AND NOT c.path STARTS WITH 'context_gateway::mcp::endpoint_facade'
RETURN DISTINCT c.path, f.path;
GREP /mcp" IN crates/context-gateway/src/**;
```

### 7.3 Notification egress (D9; K25)

| Item | Contents |
|---|---|
| (a) Files and symbols to delete | `src/egress/` whole (`mod.rs`, `notifications.rs`, `subject.rs`); `src/app.rs`: `narrowed_subscription` (:2527), `Gateway::egress_base` (:403), fields `egress_url`, `private_hosts`, `delivery_key` (`src/app.rs:113`, `:115`, `:118`); their parsing in `src/config.rs` and `src/main.rs`. |
| (b) Routes that go away | R12 `POST /api/endpoint/{slug}/egress/notifications` (`src/app.rs:449`). |
| (c) Tests that only cover the old path | `tests/notification_egress_tests.rs` (`a_created_subscription_is_narrowed_and_its_delivery_routed_back_through_the_gateway`, `a_revoked_grant_stops_the_subscription_it_allowed`), `tests/edge_egress_notifications_narrow_subscription_tests.rs`, `tests/attack_notification_targets_tests.rs`, `tests/attack_notification_amplifier_tests.rs`. |
| (d) Deployment keys | `components/context-gateway/values/gateway/base-values.yaml.gotmpl:32` (`JC_GATEWAY_DELIVERY_KEY`), `:79` (`JC_GATEWAY_EGRESS_URL`), `:55` (its comment block); `components/context-gateway/secrets.yaml:8` (`context-gateway-delivery-key`); `components/context-gateway/networkpolicies.yaml:126` (`context-gateway-notification-egress`); `components/context-gateway/apisix-plugins.yaml:269`, `:370` (the edge block of `/egress/`); `components/context-broker/networkpolicies.yaml:31-42` (broker egress only to the gateway: replaced by the broker's own egress allowance). `JC_GATEWAY_EGRESS_PRIVATE_HOSTS` and `JC_GATEWAY_EGRESS_CA_BUNDLE` are read by the gateway but set in no deployment value (grep: no hit in `values/`). |
| (e) Callers to switch | M5 (`src/reconciler/subscriptions.rs:266`, `:287`), and every subscription created through R3/R24 today (their stored `notification.endpoint.uri` points at R12: a data migration of the stored subscriptions, owner decision O-7). |

```cypher
MATCH (s:Symbol) WHERE s.path STARTS WITH 'context_gateway::egress::'
   OR s.path IN ['context_gateway::app::narrowed_subscription','context_gateway::app::Gateway::egress_base']
RETURN s.path, s.file, s.line;
GREP egress/notifications|JC_GATEWAY_DELIVERY_KEY|JC_GATEWAY_EGRESS IN crates/**;
```

### 7.4 Schema surface (D11)

| Item | Contents |
|---|---|
| (a) Files and symbols to delete | `src/handlers/schema.rs`, `src/handlers/schema_routes.rs`, `src/handlers/formalisms.rs` (called only from `handlers::schema::render` and `declares_dsd`, cg). Not deletable while L2 stays as it is: `response::describedby` (`src/middleware/response.rs:84`, called by `scrub`) imports `handlers::schema::Artifact` (`src/middleware/response.rs:10`) to write the `describedby` links; the links then point at the broker's schema surface and `Artifact` moves to a small shared type, or the links are written by the broker, the `describe_schema` tool (goes with 7.2). |
| (b) Routes that go away | R26 (`src/app.rs:480`), R28 (`:482`), R30 (`:490`), R32 (`:496`). |
| (c) Tests that only cover the old path | `tests/schema_surface_tests.rs` (`the_index_describes_what_this_endpoint_publishes`, `an_endpoint_never_names_what_it_hides`), `tests/schema_cube_tests.rs`, `tests/edge_app_schema_index_tests.rs`, `tests/edge_app_schema_artifact_tests.rs`, `tests/record_children_are_served_tests.rs::the_schema_surface_answers_under_both_prefixes`, `tests/describedby_link_tests.rs` (if the links move). |
| (d) Deployment keys | none of its own: it reads the models from the same repository mount as 7.1 (`base-values.yaml.gotmpl:97`, `:101`, `:104`). |
| (e) Callers to switch | M11 (`ui/src/components/entities/EndpointDataView.tsx:44`, `ui/src/pages/apps/AppGenerator.tsx:56`, `ui/src/pages/endpoints/SchemaProjectionPanel.tsx:133`, `sdk/src/sdk/client.ts:506`, `src/agents/oneshot/model.rs:526`). |

```cypher
MATCH (s:Symbol) WHERE s.path STARTS WITH 'context_gateway::handlers::schema'
   OR s.path STARTS WITH 'context_gateway::handlers::formalisms::'
RETURN s.path, s.file, s.line;
MATCH (c:Symbol)-[:CALLS|REFERENCES]->(f:Symbol) WHERE f.path STARTS WITH 'context_gateway::handlers::schema::'
  AND NOT c.path STARTS WITH 'context_gateway::handlers::schema'
RETURN DISTINCT c.path;
GREP schema/index\.json IN crates/context-gateway/src/**;
```

### 7.5 Access documents (D4; engine rule data K1 to K6, K38 compiled policies)

| Item | Contents |
|---|---|
| (a) Files and symbols to delete | `src/handlers/access.rs`, `src/handlers/access_odrl.rs`, `src/handlers/access_ucast.rs`, `src/handlers/access_routes.rs`; `src/federation/odrl_compiler.rs` moves into the engine addon (it produces the engine's rules) and is deleted here. Finding: `odrl_compiler::compile` (`src/federation/odrl_compiler.rs:97`) has no non-test caller in the platform (cg: 0 callers outside its file; `git grep odrl_compiler` finds only `src/federation.rs:17`), so where agreements are compiled in production is unknown from this reading. |
| (b) Routes that go away | R4 `GET /api/endpoint/{slug}/access` (`src/app.rs:428`), R10 `POST .../access/check` (`:442`). |
| (c) Tests that only cover the old path | `tests/access_authzen_tests.rs`, `tests/access_odrl_tests.rs`, `tests/access_ucast_tests.rs`, `tests/edge_app_access_tests.rs`, `tests/edge_app_access_check_tests.rs`, `tests/odrl_compiler_tests.rs`, `tests/edge_federation_odrl_compiler_compile_tests.rs`. |
| (d) Deployment keys | none of their own. |
| (e) Callers to switch | M12 (`src/agents/held.rs:132`, `ui/src/pages/access/EffectivePermissions.tsx:196`, `ui/src/components/entities/AccessPanel.tsx:64`), `crates/functions/src/endpoint.rs:169` (an allowed path in its confinement list). |

```cypher
MATCH (s:Symbol) WHERE s.path STARTS WITH 'context_gateway::handlers::access'
   OR s.path STARTS WITH 'context_gateway::federation::odrl_compiler::'
RETURN s.path, s.file, s.line;
MATCH (c:Symbol)-[:CALLS|REFERENCES]->(f:Symbol) WHERE (f.path STARTS WITH 'context_gateway::handlers::access'
   OR f.path STARTS WITH 'context_gateway::federation::odrl_compiler::')
  AND NOT (c.path STARTS WITH 'context_gateway::handlers::access' OR c.path STARTS WITH 'context_gateway::federation::odrl_compiler::')
RETURN DISTINCT c.path, f.path;
```

### 7.6 Translators that stay, re-based (D10): what is deleted from them

The translators stay; their own decision code is deleted.

| Item | Contents |
|---|---|
| (a) Symbols to delete | in `src/handlers/reads.rs`: the `Pdp::decide` calls and the `projection`, `geo`, `temporal` post-filters inside `query_entities` (:70), `query_temporal` (:158), `paged_entities` (:281), `dataset_types` (:27); in `src/handlers/files.rs`, `src/handlers/ogc.rs`, `src/handlers/sta.rs`: every `Verdict`/`Constraints` use; `src/translators/cql2.rs` keeps translating CQL2 to `q`, the hidden-attribute oracle refusal it does today (`tests/attack_policy_widening_tests.rs::a_cql2_filter_on_a_hidden_attribute_is_not_an_oracle`) moves to the engine (K18). |
| (b) Routes that go away | none (R13 to R19, R27 stay). |
| (c) Tests that only cover the old path | none whole; the decision assertions inside `tests/leak_sweep_tests.rs`, `tests/tabular_translator_tests.rs::an_ungranted_attribute_is_not_a_column`, `tests/space_dump_tests.rs::the_dump_holds_what_the_callers_grants_read_and_nothing_else`, `tests/attack_policy_widening_tests.rs` become tests against a broker stub that narrows. |
| (d) Deployment keys | `JC_GATEWAY_BROKER_URL` (`base-values.yaml.gotmpl:76`) stays and points at the broker's view surface. |
| (e) Callers | none: the paths do not change (M14). |

```cypher
MATCH (c:Symbol)-[:CALLS|REFERENCES]->(f:Symbol)
WHERE c.path STARTS WITH 'context_gateway::handlers::' AND (f.path STARTS WITH 'context_gateway::pdp::')
RETURN DISTINCT c.path, f.path;   // expect 0 rows after the re-base
```

## 8. Decisions and unknowns

Decided (worker-2, owner may override), under the owner order of 2026-10-10 that a design decision is taken and not parked. Each row names the options; the decision is what the tables above already assume.

| # | Question | Options | Decided |
|---|---|---|---|
| O-1 | Files, OGC Features and SensorThings: broker features or gateway translators? | (a) stay in the gateway, re-based on the broker view; (b) become broker surface crates like OData | (a): the GOAL's feature list (point 2) does not name them; revisit after the endpoint-view crate exists. For OGC Features superseded by ADR-0029 (owner order 2026-10-11): a broker surface crate `antares-ogc`; files, dumps and SensorThings stay (a). |
| O-2 | MCP hub `/api/mcp` and named servers `/api/mcp/{project}/{name}` | (a) stay in the gateway as catalogues that forward each call to the broker's MCP surface; (b) move, with the broker learning the Endpoint registry and token audiences | (a): listing what a token reaches is an identity question (B8, B9). |
| O-3 | Narrowing marker | (a) Antares header, always sent when the engine sets `restricted`; (b) the engine sets `restricted` only when the request carried the old opt-in | (b) for the joinedcontext engine: today's opt-in exists because the signal "tells a prober that something was there to hide" (`src/middleware/response.rs:17-19`). |
| O-4 | Dropped-types warning (`NGSILD-Warning: 199`, K18) | (a) drop it; (b) add `Filter.warning` | (b): it is what lets a developer tell an oracle refusal from an empty space (T-1862). |
| O-5 | Batch writes under a grant that divides them (K23) | (a) per-entity verdict in the seam (G9); (b) whole-batch deny | (a): pipelines (M17) retry on a whole-batch refusal. |
| O-6 | Units (K28) | (a) stay in the gateway as a request transform; (b) a schema-surface write hook in the broker | (a) until a write hook exists; then (b), since the model lives with the LinkML surface. |
| O-7 | Subscriptions stored with `notification.endpoint.uri` pointing at R12 | (a) a migration rewrites each to its `to` target and stores `__subject`; (b) recreate through the Portal reconciler | (b) for reconciler-owned subscriptions (`src/reconciler/subscriptions.rs:266`), (a) for the rest. |
| O-8 | ADR-0020 403 for out-of-view writes against the gateway's 400/404 answers (M7) | (a) 403 `urn:antares:error:AccessDenied`; (b) keep the gateway's statuses in the engine's `Deny` text only | Split by what is refused, as ADR-0023 rules 3 and 7 decide: a write naming an Entity outside the view is 404 `ResourceNotFound`, indistinguishable from an absent one; a refusal of the operation itself (the engine's `Deny`, a post-image leaving the view, a hidden Attribute written) is 403 `urn:antares:error:AccessDenied`. The UI maps both. |

Items marked unknown in this document:

- Test coverage: PUT `entities/{id}` (no PUT-specific test found), the three single-attribute writes (`entities/{id}/attrs/{attr}` PATCH, PUT, DELETE), every temporal write, `subscriptions` GET/DELETE, `get_type`, `get_attribute`, `query_temporal`, `retrieve_temporal`, `batch_query_temporal`, `list_subscriptions`, `get_subscription`, `describe_schema` MCP tools: no test names the route literal; they may be covered by table-driven tests not found by the literal search. K2 (`app_endpoint_grants_tests.rs`), K33, K40, K41 name no specific test function.
- Whether `antares-ql` accepts `scope` inside `q` (G3).
- Whether the broker's own bounds (`crate::bounds`) are as tight as the gateway's caps (S17).
- Whether the broker echoes `NGSILD-Tenant` on answers (S22).
- Which portal UI code reads the gateway's `code` member of a refusal (M7).
- Where data-space agreements are compiled into policies in production (`odrl_compiler::compile` has no non-test caller, 7.5).
- The port difference between `JC_GATEWAY_BROKER_URL` (`:8080`, `base-values.yaml.gotmpl:76`) and the broker NetworkPolicy ingress port (`9090`, `components/context-broker/networkpolicies.yaml:55`) was not traced to the Service definition.

## 9. Related

- `docs/adr/ADR-0014-extension-hooks-model.md`, `docs/adr/ADR-0020-policy-seam.md`
- `docs/analysis/cim009-to-ts104175.md`
- `crates/antares-api/src/policy.rs`
- `/workspace/tasks/GOAL-antares-surfaces.md`, `/workspace/tasks/T-3436-analysis-context-gateway-inventory-move-or-stay.md`
- joinedcontext docs: `Architecture/05-context-gateway.md`, `Architecture/04-context-spaces-and-endpoints.md`, `API/02-endpoint-representations.md`, `Requirements/gateway-firewall.md`, `Requirements/endpoints.md` (to be deleted or rewritten per surface in the removal tasks, GOAL point 11)
