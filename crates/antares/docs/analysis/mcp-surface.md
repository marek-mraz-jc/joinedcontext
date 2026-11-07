# MCP surface inside the broker: tools from the view's operation allowlist

Sources: Antares-NGSI-LD-Context-Broker `7185e1a` (branch `agent/worker-2/T-3436`), joinedcontext-platform `59a8f70` (`crates/context-gateway/src/mcp/`, `crates/assistant/src/chat/mcp.rs`), joinedcontext-portal `bd3a68a8` (`src/mcp/mod.rs`), ETSI TS 104 175 V1.1.1 under `/workspace/etsi-specs/`, the MCP specification revisions 2026-07-28 and 2025-11-25 (modelcontextprotocol.io, the Markdown form of each page fetched 2026-10-10), the `rmcp` crate 3.5.1 (crates.io API and the `rmcp-v3.5.1` tag of github.com/modelcontextprotocol/rust-sdk, read 2026-10-10). Code graphs: platform graph built `2026-10-10T11:31:20Z @ 59a8f705b9`, Antares graph built from `7185e1a`. Task T-3442. Inputs: `operation-matrix.yaml` and `operation-matrix.md` (T-3437, the `mcp` column), `gateway-to-broker-map.md` (T-3436, decisions D5, D6, D7, K42), `cim009-to-ts104175.md`.

Every platform path below is relative to `crates/context-gateway/` unless another crate is named; every Antares path is relative to the Antares repository root. MCP spec pages are cited by their path under `https://modelcontextprotocol.io/specification/`. Decisions follow the GOAL rule "No questions to the owner": each is written "Decided (worker-2, owner may override): ...".

## 1. Scope and summary of the decisions

The broker gains one surface crate, `antares-mcp`, behind cargo feature `mcp`, mounted through the existing `ApiSurface` trait (`crates/antares-api/src/surface.rs:17-32`) at `/x/views/{view}/mcp` (the matrix prefix, `operation-matrix.md` section 1; `check_prefix`, `crates/antares-api/src/surface.rs:38-50`, admits it). Every tool call becomes one NGSI-LD request that runs through `AppState::call` (`crates/antares-api/src/state.rs:535-636`), so the TS 104 175 operation, the `gate!` call of that operation and the ADR-0020 seam fire exactly as for an HTTP client. The surface holds no store handle.

| # | Decision | Section |
|---|---|---|
| D-1 | 32 tools, each bound to exactly one `operation-matrix.yaml` row; `create_entity_map` (row 33 of the `mcp` column) is used by the paging cursor and is not a tool; `purge_entities` stays rejected | 3 |
| D-2 | Three tool names of the matrix are corrected: `list_types` and `list_attributes` each stood for two rows and are split (`list_type_details`, `list_attribute_details`); `upsert_entity` becomes `batch_upsert` | 3.3 |
| D-3 | `tools/list` = the tool table filtered by the view's operation allowlist (T-3438) and by one batch-shaped engine probe; a hidden tool and an unknown tool answer byte-identically | 3.1 |
| D-4 | Input schemas carry the view's visible types and attributes as `enum`s, filtered by a name pattern; the schema is a convenience, the gate is the enforcement | 3.2 |
| D-5 | Resources `ngsi-ld://{view}/entities/{id}` (Retrieve Entity) and `ngsi-ld://{view}/types/{type}` (Retrieve Entity Type Information, not "the entities of the type" as the platform reads it) | 4 |
| D-6 | `cacheScope: "private"` on every view-narrowed cacheable result; `"public"` only on `server/discover` | 4.3 |
| D-7 | Resource subscriptions through 2026-07-28 `subscriptions/listen` (`resourceSubscriptions`), each URI backed by one internal NGSI-LD Subscription created through `AppState::call`, delivered by an in-process sink through `deliver_as` and therefore `pre_notify` | 5 |
| D-8 | Protocol: modern 2026-07-28 stateless first; dual-era, legacy `initialize` accepted for 2025-11-25 and 2025-06-18 without sessions, without resource subscriptions and without write tools; Streamable HTTP POST only; `rmcp = "=3.5.1"` | 6 |
| D-9 | Auth: the broker validates no token; the subject is `ANTARES_POLICY_SUBJECT_HEADERS` as the gateway in front sets them; the gateway stays the OAuth resource server | 7 |
| D-10 | Output: pagination with an opaque cursor and a per-result byte budget; never truncation | 8 |
| D-11 | Writes need the person's confirmation through MRTR elicitation (`InputRequiredResult`), integrity-protected `requestState`, single use | 3.4 |
| D-12 | Moves: the façade's tools, resources, elicitation, schema validation. Stays: hub, named MCP servers, RFC 9728 metadata, token verification, rate limits | 9 |
| D-13 | `join`/`joinLevel` are not tool arguments until T-3470 makes the join walk apply the narrowing | 10 |

## 2. The platform data MCP today

### 2.1 Routes and the call path

The platform serves MCP on four routes (T-3436 table 2.1: R5, R6, R8, R25). The endpoint façade `mcp::endpoint_facade::handle` (`src/mcp/endpoint_facade.rs:1413-1483`) has two callers (cg, Q1 below): `app::mcp_answer` (`src/app.rs:3320`), reached from R5 `POST /api/endpoint/{slug}/mcp` and R25 `POST /cs/{space}/mcp`, and `mcp::hub::Hub::forward` (`src/mcp/hub.rs:326`), reached from R6 `POST /api/mcp` and, through the hub, R8 `POST /api/mcp/{project}/{name}`.

The façade is hand-rolled JSON-RPC, no `rmcp` dependency, and declares `PROTOCOL_VERSION = "2025-06-18"` (`src/mcp/endpoint_facade.rs:53-54`). It answers `initialize` (`:1443`), `ping` (`:1460`), `tools/list`, `tools/call`, `resources/list`, `resources/templates/list` and `resources/read`; every other method is `-32601` (`:1481`). It is stateless: no session, `listChanged: false` and `subscribe: false` (`:1449-1451`).

A data tool call takes this path (cg, Q2 and Q3):

1. `call_tool` (`src/mcp/endpoint_facade.rs:1485-1685`) finds the tool by name; a tool the caller is not granted answers `-32602 "unknown tool"`, the same bytes as a tool that does not exist (`:1500-1508`).
2. A selector argument (`space`, `tenant`, `contextspace`, `slug`, `endpoint`; `SELECTOR_ARGUMENTS`, `:56-57`) is refused (`:1509-1520`).
3. The arguments are validated against the tool's published JSON Schema (`validate`, `:1135-1163`; compiled draft-7 validators, `VALIDATORS`, `:1122-1133`); a refusal names the broken rule and never repeats the value (`broken_rule`, `:1165-1220`).
4. A write tool goes through the elicitation gate (`:1553-1604`, `src/mcp/elicitation.rs`).
5. `request_for` (`:2239-2456`) builds the NGSI-LD method, path, query string and body; `ngsi_ld_request` (`src/app.rs:621`, called at `src/mcp/endpoint_facade.rs:1613`) runs it through `admit_by`/`admit_space` and `serve_ngsi_ld` (`src/app.rs:663`, `:656`, `:671`), which is where `Pdp::decide` and the broker call `proxy::Broker::send` happen (6 call paths from `call_tool` to `Broker::send`, Q3).
6. The answer becomes `answered` (`:1904-1938`): `structuredContent` = `{<result_key>: payload, restricted?, jc:source?}` plus the payload serialized as one text block; `paged` (`:1964-1984`) adds `total` (from `NGSILD-Results-Count`) and `nextCursor` (the `offset` of `Link rel="next"`, `next_offset`, `:1940-1962`); `warned` (`:1986-2002`) adds the warning header values. A non-2xx answer is `refused` (`:2004-2017`): `isError: true` with the ProblemDetails as `structuredContent`.

Discovery is the PDP: `tools_for` (`:1222-1241`) lists a tool when `granted` (`:1290-1307`) finds any of its operations not answered `Verdict::Deny` by `gateway.pdp.decide(subject, operation, &PolicyRequest::default(), endpoint)` (`:1303`), that is, a probe with an empty request.

### 2.2 Tool map

"Operations" is the tool's `operations:` list (`jc_core::kinds::Operation`); "Input schema source" is the `schema` closure; "NGSI-LD request" is the `request_for` arm (dispatch lines from T-3436 table 2.3, rechecked). PDP: every data tool reaches `Pdp::decide` twice, once in `granted` (`:1303`) for discovery and once inside `serve_ngsi_ld` for the request; "tests" names one or more `#[tokio::test]` functions whose body names the tool (method of section 2.4).

| Tool (decl. line) | Operations | Input schema source | NGSI-LD request | Result key | Tests |
|---|---|---|---|---|---|
| `query_entities` (:110) | QueryEntity | `properties_of(SELECTOR_ARGS, REPRESENTATION_ARGS)` (:119), `anyOf` one of type/id/attrs/q/georel (:123-129), `additionalProperties: false` | GET `/entities` (:2253) | `entities` | `tests/mcp_tools_tests.rs::every_read_parameter_of_the_table_reaches_the_broker_under_its_rest_name`, `tests/mcp_read_security_tests.rs::a_join_never_inlines_an_ungranted_entity`, `tests/mcp_read_security_tests.rs::a_hostile_q_is_bounded_before_the_broker_is_asked` |
| `get_entity` (:136) | RetrieveEntity | `PROJECTION_ARGS` + `REPRESENTATION_ARGS` + `id_schema()` (:142-147) | GET `/entities/{id}` (:2399) | `entity` | `tests/mcp_facade_tests.rs::a_refusal_comes_back_as_a_tool_error_and_never_as_an_empty_answer`, `tests/mcp_facade_tests.rs::an_id_argument_is_a_urn_and_a_plain_type_name_still_passes` |
| `list_types` (:154) | RetrieveEntityTypes, RetrieveEntityTypeDetails | inline `details` boolean (:163) | GET `/types[?details=true]` (:2259) | `entityTypes` | `tests/mcp_tools_tests.rs::the_listing_and_batch_tools_make_the_requests_they_stand_for` |
| `list_attributes` (:172) | RetrieveAttrTypes, RetrieveAttrTypeDetails | inline (:181) | GET `/attributes[?details=true]` (:2259) | `attributes` | `tests/mcp_tools_tests.rs::the_listing_and_batch_tools_make_the_requests_they_stand_for` |
| `query_temporal` (:190) | QueryTemporal | `temporal_schema` (:1028-1041, `TEMPORAL_ARGS` :1043-1120) | GET `/temporal/entities` (:2413) | `entities` | `tests/mcp_tools_tests.rs::the_temporal_tools_forward_the_whole_grammar_to_the_temporal_api` |
| `retrieve_temporal` (:197) | RetrieveTemporal | `temporal_schema` + `id_schema()`, required id, timerel, timeAt (:201-206) | GET `/temporal/entities/{id}` (:2413) | `entity` | `tests/mcp_tools_tests.rs::the_temporal_tools_forward_the_whole_grammar_to_the_temporal_api` |
| `batch_query` (:212) | QueryBatch | `SELECTOR_ARGS` + `REPRESENTATION_ARGS` + `ids` (:219-230) | POST `/entityOperations/query` (:2359) | `entities` | `tests/mcp_tools_tests.rs::the_batch_query_carries_the_same_selectors_in_the_body_and_pages_in_the_query` |
| `list_subscriptions` (:238) | QuerySubscription | inline `limit` 1..1000 (:246-247) | GET `/subscriptions` (:2269) | `subscriptions` | untested: no test file names the tool (section 2.4) |
| `describe_access` (:257) | none | inline `format` enum (:266) | none: `access_document` (:1871-1891) renders `access::permissions`, `access_odrl::policy`, `access_ucast::grant_ast` | `access` | `tests/mcp_facade_tests.rs::describe_access_answers_each_format_with_the_document_the_http_surface_serves` |
| `describe_schema` (:275) | QueryEntity, RetrieveEntity, RetrieveEntityTypes (:278) | inline, `entityType` one or up to 64 (:291-310) | none: `describe_schema` (:2139-2199) from `schema::visible` (:2149) | `schema` | `tests/mcp_tools_tests.rs::describe_schema_narrows_to_a_type_the_grant_covers_and_tells_no_tales_about_the_rest` |
| `upsert_entity` (:320) | UpsertBatch | inline `entity: {type: object}`, unconstrained (:326-329) | POST `/entityOperations/upsert` (:2440, :2449) | `entity` | `tests/mcp_facade_tests.rs::a_write_tool_asks_the_person_before_it_changes_anything`, `tests/mcp_read_security_tests.rs::every_listed_tool_is_callable_and_every_absent_one_is_unknown` |
| `batch_query_temporal` (:336) | QueryTemporal | `SELECTOR_ARGS` + `REPRESENTATION_ARGS` + `TEMPORAL_ARGS` (:342-347) | POST `/temporal/entityOperations/query` (:2317) | `entities` | `tests/mcp_tools_tests.rs::the_registry_reads_and_the_temporal_batch_make_the_requests_they_stand_for` |
| `get_type` (:354) | RetrieveEntityTypeInfo | `type_schema()` (:363) | GET `/types/{t}` (:2290) | `entityType` | `tests/mcp_read_security_tests.rs::the_registry_reads_of_the_matrix_stay_inside_the_grant` |
| `get_attribute` (:371) | RetrieveAttrTypeInfo | inline `attrId` maxLength 256 (:377-384) | GET `/attributes/{a}` (:2299) | `attribute` | `tests/mcp_tools_tests.rs::the_registry_reads_and_the_temporal_batch_make_the_requests_they_stand_for` |
| `get_subscription` (:391) | RetrieveSubscription | inline `id` maxLength 512 (:397-404) | GET `/subscriptions/{id}` (:2308) | `subscription` | `tests/mcp_read_security_tests.rs::the_registry_reads_of_the_matrix_stay_inside_the_grant` |
| `create_subscription` (:411) | CreateSubscription | inline `subscription` object (:417-420) | POST `/subscriptions` after elicitation (:2346) | `subscription` | `tests/mcp_tools_tests.rs::creating_a_subscription_needs_the_operators_confirmation_first`, `tests/mcp_tools_tests.rs::a_declined_subscription_creates_nothing`, `tests/mcp_tools_tests.rs::an_answer_does_not_carry_over_to_other_arguments` |

16 tools. Shared argument tables: `SELECTOR_ARGS` (`:487-724`, includes `limit` 1..1000 at `:703-712` and `cursor` = `offset` at `:713-723`), `PROJECTION_ARGS` (`:726-814`), `REPRESENTATION_ARGS` (`:816-856`), `TEMPORAL_ARGS` (`:1043-1120`). Bounds the façade puts on input: `q` maxLength 4096 (`:541`), `coordinates` 8192 (`:585`), `joinLevel` 1..3 (`:655`), id lists 50 to 100 items (`:632`, `:666`, `:877`).

### 2.3 Resources and protocol methods

| Method / URI | Answered by | Enforcement |
|---|---|---|
| `resources/list` | `resources` (`:1687-1729`): one `ngsi-ld://{space}/types/{t}` per type `schema::visible_types` returns, `access://{slug}`, one `schema://{slug}/v{major}/{artifact}` per rendered artifact | `schema::visible` (`:1693`), the grant projection |
| `resources/templates/list` | inline (`:1467-1477`): `ngsi-ld://{space}/entities/{id}` | none needed |
| `resources/read` | `read_resource` (`:1731-1809`), `parse_resource` (`:1823-1866`): `types/{t}` is run as `query_entities type=t` (`:1759-1763`), `entities/{id}` as `get_entity` (`:1764-1768`), `access://` and `schema://` from the projections | the tool's `granted` (`:1777`) then `ngsi_ld_request` (`:1788`); a URI of another space answers `-32602 "unknown resource"` like an unknown one (`:1769-1771`) |
| `initialize`, `ping` | inline (`:1443-1460`) | none |

Both `call_tool` and `read_resource` read the broker answer with `axum::body::to_bytes(..., 8 * 1024 * 1024)` (`:1663`, `:1799`): a larger answer becomes `Value::Null`, not an error. `describe_schema` refuses a document over `DOCUMENT_BOUND = 1024 * 1024` bytes with the way to narrow it (`within_bound`, `:2201-2218`). There is no other output bound in the façade; the platform assistant then cuts a result at `MAX_RESULT_CHARS = 20_000` characters on its side (`joinedcontext-platform crates/assistant/src/chat/mcp.rs:11-12`), which is truncation.

### 2.4 Cypher and test method

Run from the platform clone:

```cypher
// every symbol of the MCP module (116 non-test items)
MATCH (f:File)-[:DEFINES]->(s:Symbol)
WHERE f.path STARTS WITH 'crates/context-gateway/src/mcp/' AND s.is_test=false
  AND s.kind IN ['Function','Method','StaticMethod','Constant','StaticVariable','Struct','Enum']
RETURN s.path, s.kind, s.line, s.end_line ORDER BY s.file, s.line;
// Q1: who calls the façade
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol {path:'context_gateway::mcp::endpoint_facade::handle'})
RETURN c.path, c.file, r.line ORDER BY c.file;
// Q2: the PDP, projection and gateway calls the MCP module makes
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE c.path STARTS WITH 'context_gateway::mcp::' AND c.is_test=false
  AND f.path IN ['context_gateway::app::ngsi_ld_request','context_gateway::pdp::Pdp::decide',
    'context_gateway::pdp::evaluator::effective','context_gateway::handlers::schema::visible',
    'context_gateway::proxy::Broker::send']
RETURN c.path, f.path, r.line ORDER BY c.path;
// Q3: call_tool reaches the broker (6 paths)
MATCH (c:Symbol)-[:CALLS|DISPATCHES*1..6]->(f:Symbol {path:'context_gateway::proxy::Broker::send'})
WHERE c.path='context_gateway::mcp::endpoint_facade::call_tool' RETURN count(*) AS paths;
// what ngsi_ld_request and access_document call
MATCH (c:Symbol {path:'context_gateway::app::ngsi_ld_request'})-[r:CALLS]->(f:Symbol) RETURN f.path, r.line;
MATCH (c:Symbol {path:'context_gateway::mcp::endpoint_facade::access_document'})-[r:CALLS]->(f:Symbol) RETURN f.path, r.line;
```

Tests: T-3436 section 1 point 4 established that every integration test reaches a handler only through `router()` and an HTTP request, so the graph has no test edge to a tool. Tests were therefore named by the tool literal: for each tool name, the `fn` enclosing each line of `tests/mcp_*.rs` and `tests/leak_sweep_tests.rs` that contains `"<tool>"` (awk over the files). `list_subscriptions` appears in no test file: untested. `tests/leak_sweep_tests.rs` names every tool inside a helper `probes`, not a test function, so it is not cited.

## 3. Tools

### 3.1 Generation from the view's operation allowlist (task point 1)

A view (ADR-0023, `docs/adr/ADR-0023-endpoint-views.md`, T-3438: a selector, `q`, `scopeQ`, `geoQ`, an attribute allowlist and an operation allowlist written in operation-matrix row ids, addressed at `/x/views/{view}/`) is the input. The surface holds one static table `TOOLS: [ToolDef; 32]`, each entry naming one `operation-matrix.yaml` row id. `tools/list` for a request is computed, never stored:

1. Keep the entries whose row id is in the view's operation allowlist. Decided (worker-2, owner may override): the allowlist is written in matrix row ids (`retrieve_entity`, `batch_upsert`, ...), because the row id is already the stable key every surface and test uses (`operation-matrix.md` section 3: "the key tests use; never reused"), and a clause string is ambiguous (`"5.7.1"` gates both `retrieve_entity` and `retrieve_attribute_value`, `operation-matrix.md` section 4 notes). This is input to T-3438.
2. Ask the engine once, with every remaining entry's gate clause: an engine answering `Deny` for a clause hides its tools. This is what the platform does per operation with an empty request (`granted`, `src/mcp/endpoint_facade.rs:1290-1307`), but the seam has no such call today: `PolicyEngine` has `decide` and `pre_notify` only (`docs/adr/ADR-0020-policy-seam.md`, Decision). Calling `decide` once per tool would break ADR-0014 rule 1 ("Hooks fire per request or per drained batch, never per attribute or per matched subscription", `docs/adr/ADR-0014-extension-hooks-model.md:37-40`). Decided (worker-2, owner may override): add a batch-shaped default method `PolicyEngine::offers(&self, subject, clauses: &[&'static str]) -> Vec<bool>` whose default calls `decide` with an empty `Operation` per clause inside one call and maps `Deny` to `false` and everything else to `true`; an engine overrides it to answer "could any request of this clause be allowed". Same fail-closed rule as `decide`: an error, panic or timeout hides every tool. With no engine attached (`AppState::policy` is `None`, ADR-0020 Consequences) every tool the view allows is listed. This is an ADR-0020 amendment, to be written in T-3443.
3. Sort by tool name. The 2026-07-28 changelog, minor change 3: "Servers SHOULD return tools from `tools/list` in a deterministic order".

A hidden tool does not exist for the caller: `tools/call` of a hidden tool and of a name nobody defined both answer the same JSON-RPC error (`-32602`, "unknown tool"), as the platform does (`src/mcp/endpoint_facade.rs:1500-1508`, test `tests/mcp_facade_tests.rs::an_ungranted_tool_is_indistinguishable_from_one_that_does_not_exist`). The listing is a convenience: the `gate!` call of the operation still runs on every call (section 3.5), so a stale client cache or a guessed tool name changes nothing.

### 3.2 Input schemas restricted to visible types and attributes

Each schema is built per request from the view, in JSON Schema 2020-12 (2026-07-28 changelog, minor change 10: "Loosen `inputSchema` and `outputSchema` to allow any JSON Schema 2020-12 keywords"), with `additionalProperties: false` on every object, as the platform does (`src/mcp/endpoint_facade.rs:1135-1163`). Building blocks used in the table below:

| Key | Schema | Derived from |
|---|---|---|
| `T` | `type`: string, `enum` = the view's visible Entity types, compact names under the view's `@context` | the view's type selector; when the view selects no type, `pattern` `^[A-Za-z][A-Za-z0-9_-]*$` (platform `TYPE_NAME`, `src/mcp/endpoint_facade.rs:59-65`) and no enum |
| `TS` | type selection expression string, maxLength 1024, pattern of platform `TYPE_SELECTION` (`:67-68`), or an array of `T` (maxItems 50) | as `T` |
| `A` | attribute name: string, `enum` = the view's attribute allowlist; else pattern `^[A-Za-z][A-Za-z0-9_\-.:/#]*$` (platform `ATTR_NAME`, `:73-78`), maxLength 256 | the view's attribute allowlist |
| `ID` | string, maxLength 512, pattern `^urn:ngsi-ld:[^\s]+$` (platform `ENTITY_URN`, `:70-71`); any NGSI-LD URN, no segment carries a right (ADR-N-041, CLAUDE.md non-negotiables) | fixed |
| `E` | Entity body: object, `id` = `ID`, `type` = `T`, `propertyNames` = `A` plus the core members the view allows (`scope`, `location`, `observationSpace`, `operationSpace`), no `@context` | the view's attribute allowlist |
| `Q` | `q` maxLength 4096, `scopeQ` maxLength 1024, `georel`/`geometry`/`coordinates`/`geoproperty` as platform `SELECTOR_ARGS` (`:487-724`) | fixed; attribute paths inside `q` are not schema-checked (a grammar), the broker parses them |
| `P` | `limit` integer 1..`max_limit`, default 20; `cursor` opaque string maxLength 2048; `count` boolean | `AppState::max_limit` = 1000 (`crates/antares-api/src/state.rs:425`) |
| `J` | `join` enum `flat`/`inline`, `joinLevel` 1..3 (platform `:655`; broker ceiling `MAX_JOIN_LEVEL = 10`, `crates/antares-api/src/bounds.rs:109`) | fixed; not offered until T-3470 is done (section 10 rule 4) |
| `TMP` | `timerel`, `timeAt`, `endTimeAt`, `lastN` 1..100, `aggrMethods`, `aggrPeriodDuration` (platform `TEMPORAL_ARGS`, `:1043-1120`, with `lastN` bound proposed here) | fixed |

Rules:

- No tool body carries `@context`. The surface sets the `Link` header of the inner request to the view's `@context`, so a caller cannot rebind a term and move a `pick`/`omit` rule or a view attribute off its target (ADR-0020: "a caller that binds the rule's term ... would move the rule off its target and walk out of the narrowing"). `AppState::call` copies `Link` from the outer request only when the inner one does not set it (`PROPAGATED`, `crates/antares-api/src/state.rs:63-67`; the copy loop, `:604-611`), so the surface setting it wins.
- An enum entry must also match its pattern; a name that does not (spaces, punctuation, control characters) is left out of the enum and stays reachable through the pattern-free NGSI-LD API only. Enum values are text a model reads, and a view's names come from deployment configuration, but a pattern on top keeps an instruction-shaped attribute name out of the schema whatever the configuration holds.
- An enum is bounded at 256 entries; a larger set falls back to the pattern, and the tool description says so. The bound is a proposal, to be measured against prompt size in T-3467.
- A value outside an enum and a hidden name inside the pattern get the same refusal shape (`isError: true`, the broken rule, never the value, as `broken_rule` does at `src/mcp/endpoint_facade.rs:1165-1220`), so the schema is no oracle for hidden types: a hidden type is "not one of" the enum exactly as a type that does not exist.

### 3.3 Corrections to the matrix's tool names

The task requires each tool to be one matrix row. The `mcp` column (T-3437) has 33 `supported` cells and one `rejected` (`purge_entities`), read with the awk in section 3.6. Three proposed names do not meet the rule and are corrected here; the yaml is not edited by this task, T-3445 carries the change into it:

| Matrix row | Matrix binding | Corrected tool | Why |
|---|---|---|---|
| `retrieve_entity_types` | `list_types` | `list_types` | unchanged |
| `retrieve_entity_type_details` | `list_types` (same tool, `details=true`) | `list_type_details` | one tool, one row; the two forms are gated with the same clause today (`"5.7.5"`, `crates/antares-api/src/types_attrs.rs:220`), but TS 104 175 keeps them as two operations (10.6.3, 10.6.4) and a view may allow one without the other |
| `retrieve_attributes` | `list_attributes` | `list_attributes` | unchanged |
| `retrieve_attribute_details` | `list_attributes` (`details=true`) | `list_attribute_details` | as above (10.6.6, 10.6.7, gate `"5.7.8"` at `crates/antares-api/src/types_attrs.rs:333`) |
| `batch_upsert` | `upsert_entity` (one Entity, platform `src/mcp/endpoint_facade.rs:320`) | `batch_upsert` (1..N Entities) | the row is 10.3.3 Batch Entity Create or Update; the platform tool sends a one-element array to `entityOperations/upsert` (`:2440`, `:2449`). A single-Entity name for a batch operation hides the array form, and the other four batch rows already use `batch_*` |
| `create_entity_map` | "used internally by query_entities paging" | no tool | it stays the cursor's mechanism (section 8) and is reached only through `query_entities` with `entityMap=true`, gated under `"5.7.2"`; not listed |
| `notification` (10.5.7) | `not offered` | served as `notifications/resources/updated` on `subscriptions/listen` | section 5; the cell should read `supported` with that binding |

`describe_access` and `describe_schema` are not operations (`operation-matrix.md` section 7.2) and are not tools of the broker MCP: see section 9.

### 3.4 Write tools need the person (MRTR elicitation)

The platform rule stays: a write is confirmed by the person, not by the model (AG-08, `src/mcp/endpoint_facade.rs:1553-1604`; tests `tests/mcp_tools_tests.rs::a_subscription_is_created_only_after_the_person_answered_the_servers_question`, `tests/mcp_facade_tests.rs::a_write_tool_asks_the_person_before_it_changes_anything`). The platform does it with its own result document and a `params.elicitation` member (`src/mcp/elicitation.rs`), which no standard client reads. 2026-07-28 standardises the round trip: `basic/patterns/mrtr`: "Servers return an `InputRequiredResult` (`resultType: "input_required"`) whose `inputRequests` field carries the requests", and the client retries with `inputResponses` and the echoed `requestState`.

Decided (worker-2, owner may override):

- The first call of a write tool runs the gate (so a denied write is refused before the person is asked) and returns `InputRequiredResult` with one `elicitation/create` request describing the operation, the Entity ids and the attribute names, never attribute values (they are data, section 10).
- `requestState` is an HMAC-SHA256 over (tool, digest of the canonical arguments, subject digest, view, tenant, expiry 10 minutes, nonce) with a key drawn at process start. `basic/patterns/mrtr` rule 4: "servers MUST treat `requestState` as an attacker-controlled input. If `requestState` influences authorization ... servers MUST protect its integrity (e.g. HMAC or AEAD)". The nonce is spent in an in-process set, since a confirmation is a one-time redemption ("Servers for which a given `requestState` must be consumed at most once ... MUST enforce that invariant server-side", rule 5).
- The retry runs the gate again, then the operation. The confirmation never replaces the gate.
- A client that does not declare `elicitation` in `io.modelcontextprotocol/clientCapabilities` cannot be asked (rule 7: "Servers MUST NOT send an `inputRequests` that the client has not declared support for"); its write call is answered `isError: true` "this tool needs a client that can ask the person". Write tools are still listed, so `tools/list` does not vary with the client's capabilities.
- Limitation: the key and the spent set are per process; a retry that reaches another pod fails verification and the client is asked again. A shared key from a Kubernetes Secret through `secretRef` is the follow-up (CLAUDE.md, Secrets).

### 3.5 The tool table

Every tool runs one NGSI-LD request through `AppState::call` (`crates/antares-api/src/state.rs:535`): path below `/ngsi-ld/v1`, the gate clause the handler passes to `gate!`, and the `Operation` members it fills are the matrix's (`operation-matrix.yaml`, `surfaces.mcp.checks.operation_fields`). Phase: every tool is `on_request` (one `gate!` per call); `pre_notify` is listed where the tool creates notifications. "Out" is `structuredContent`; every result also carries the same JSON as one text block (`server/tools`: "a tool that returns structured content SHOULD also return the serialized JSON in a TextContent block"). "Page" refers to section 8. W = write, confirmed per 3.4.

| # | Tool | Matrix row (TS 104 175) | NGSI-LD request | Gate clause / `Operation` fields | Input schema derivation | Output, limits and pagination | ADR-0020 phase |
|---|---|---|---|---|---|---|---|
| 1 | `create_entity` W | `create_entity` (10.2.2) | POST `/entities` | 5.6.1 / clause, ids, types, attrs, body | `{entity: E}` | `{created: ID}`; body ≤ `MCP_MAX_ARGS_BYTES` | on_request |
| 2 | `update_attributes` W | `update_attributes` (10.2.3) | PATCH `/entities/{id}/attrs` | 5.6.2 / clause, ids | `{id: ID, attributes: E without id/type}` | `{updated: [A], notUpdated: [...]}` (the 207 body) | on_request |
| 3 | `append_attributes` W | `append_attributes` (10.2.4) | POST `/entities/{id}/attrs` | 5.6.3 / clause, ids | `{id: ID, attributes: E without id/type, noOverwrite?: bool}` | as 2 | on_request |
| 4 | `update_attribute` W | `partial_attribute_update` (10.2.5) | PATCH `/entities/{id}/attrs/{attrId}` | 5.6.4 / clause, ids | `{id: ID, attrId: A, attribute: object}` | `{}` on 204 | on_request |
| 5 | `delete_attribute` W | `delete_attribute` (10.2.7) | DELETE `/entities/{id}/attrs/{attrId}` | 5.6.5 / clause, ids | `{id: ID, attrId: A, datasetId?, deleteAll?}` | `{}` | on_request |
| 6 | `delete_entity` W | `delete_entity` (10.2.8) | DELETE `/entities/{id}` | 5.6.6 / clause, ids | `{id: ID}` | `{}` | on_request |
| 7 | `merge_entity` W | `merge_entity` (10.2.9) | PATCH `/entities/{id}` | 5.6.17 / clause, ids, body | `{id: ID, fragment: E without id}` | `{}` | on_request |
| 8 | `replace_entity` W | `replace_entity` (10.2.10) | PUT `/entities/{id}` | 5.6.18 / clause, ids | `{id: ID, entity: E}` | `{}` | on_request |
| 9 | `replace_attribute` W | `replace_attribute` (10.2.11) | PUT `/entities/{id}/attrs/{attrId}` | 5.6.19 / clause, ids | `{id: ID, attrId: A, attribute: object}` | `{}` | on_request |
| 10 | `batch_create` W | `batch_create` (10.3.2) | POST `/entityOperations/create` | 5.6.7 / clause, ids, types, attrs | `{entities: [E], maxItems: MCP_MAX_BATCH}` | `{success: [ID], errors: [...]}` (the 201/207 body) | on_request |
| 11 | `batch_upsert` W | `batch_upsert` (10.3.3) | POST `/entityOperations/upsert` | 5.6.8 / clause, ids, types, attrs | `{entities: [E], options?: update\|replace}` | as 10 | on_request |
| 12 | `batch_update` W | `batch_update` (10.3.4) | POST `/entityOperations/update` | 5.6.9 / clause, ids, types, attrs | `{entities: [E], noOverwrite?}` | as 10 | on_request |
| 13 | `batch_merge` W | `batch_merge` (10.3.5) | POST `/entityOperations/merge` | 5.6.20 / clause, ids, types, attrs | `{entities: [E]}` | as 10 | on_request |
| 14 | `batch_delete` W | `batch_delete` (10.3.6) | POST `/entityOperations/delete` | 5.6.10 / clause, ids | `{ids: [ID], maxItems: MCP_MAX_BATCH}` | as 10 | on_request |
| 15 | `get_entity` | `retrieve_entity` (10.4.2) | GET `/entities/{id}` | 5.7.1 / clause, ids; FILTERABLE | `{id: ID, attrs?: [A], pick?: [A], omit?: [A], lang?}`; `J` added only after T-3470 (section 10 rule 4) | `{entity}`; over the byte budget: `isError` naming `pick`/`attrs` | on_request |
| 16 | `query_entities` | `query_entities` (10.4.3) | GET `/entities` | 5.7.2 / clause, q, scope_q; FILTERABLE | `{type?: TS, id?: [ID], idPattern?, attrs?: [A], pick?, omit?, Q, P}` (`J` after T-3470), `anyOf` one of `type`, `attrs`, `q`, `georel`, `local` (TS 104 175 `10/10.4.3.4.md`: "At least one of the following input data shall be provided"; the platform's `anyOf` at `:123-129` also accepts `id`, which the clause does not list, and lacks `local`) | `{entities, total?, nextCursor?, restricted?}`; page | on_request |
| 17 | `batch_query` | `query_entities_post` (10.4.3) | POST `/entityOperations/query` | 5.7.2 / clause only (matrix gap: the Query body's members never reach the engine) | as 16 with `ids: [ID]` maxItems 100 | as 16 | on_request |
| 18 | `get_attribute_value` | `retrieve_attribute_value` (10.4.4) | GET `/entities/{id}/attrs/{attrId}` | 5.7.1 / clause, ids; FILTERABLE | `{id: ID, attrId: A}` | `{attribute}` | on_request |
| 19 | `create_subscription` W | `create_subscription` (10.5.2) | POST `/subscriptions` | 5.8.1 / clause, ids; subject stored in `__subject` | `{subscription}`: `entities[].type` = `T`, `watchedAttributes` = `[A]`, `q`, `notification.attributes` = `[A]`, `notification.endpoint.uri` restricted to `ANTARES_MCP_SUBSCRIPTION_ENDPOINTS` (section 10) | `{created: subscription id}` | on_request; its notifications `pre_notify` |
| 20 | `update_subscription` W | `update_subscription` (10.5.3) | PATCH `/subscriptions/{id}` | 5.8.2 / clause, ids | `{id, fragment}` with the same restrictions as 19 | `{}` | on_request; `pre_notify` |
| 21 | `get_subscription` | `retrieve_subscription` (10.5.4) | GET `/subscriptions/{id}` | 5.8.3 / clause, ids | `{id: string maxLength 512}` | `{subscription}` | on_request |
| 22 | `list_subscriptions` | `query_subscriptions` (10.5.5) | GET `/subscriptions` | 5.8.4 / clause; FILTERABLE (matrix 5.1, T-3673: narrowed to the caller's own by `subject`) | `{P}` | `{subscriptions, nextCursor?}`; page | on_request |
| 23 | `delete_subscription` W | `delete_subscription` (10.5.6) | DELETE `/subscriptions/{id}` | 5.8.5 / clause, ids | `{id}` | `{}` | on_request |
| 24 | `list_types` | `retrieve_entity_types` (10.6.3) | GET `/types` | 5.7.5 / clause; not FILTERABLE | `{}` | `{entityTypes}` | on_request |
| 25 | `list_type_details` | `retrieve_entity_type_details` (10.6.4) | GET `/types?details=true` | 5.7.5 / clause; not FILTERABLE | `{}` | `{entityTypes}`; byte budget | on_request |
| 26 | `get_type` | `retrieve_entity_type_info` (10.6.5) | GET `/types/{type}` | 5.7.7 / clause | `{type: T}` | `{entityType}` | on_request |
| 27 | `list_attributes` | `retrieve_attributes` (10.6.6) | GET `/attributes` | 5.7.8 / clause; not FILTERABLE | `{}` | `{attributes}` | on_request |
| 28 | `list_attribute_details` | `retrieve_attribute_details` (10.6.7) | GET `/attributes?details=true` | 5.7.8 / clause; not FILTERABLE | `{}` | `{attributes}`; byte budget | on_request |
| 29 | `get_attribute` | `retrieve_attribute_info` (10.6.8) | GET `/attributes/{attrId}` | 5.7.10 / clause | `{attrId: A}` | `{attribute}` | on_request |
| 30 | `retrieve_temporal` | `temporal_retrieve` (11.3.2) | GET `/temporal/entities/{id}` | 5.7.3 / clause, ids; FILTERABLE | `{id: ID, attrs?: [A], TMP}`, required `timerel`, `timeAt` | `{entity}`; byte budget, `lastN` ≤ 100 | on_request |
| 31 | `query_temporal` | `temporal_query` (11.3.3) | GET `/temporal/entities` | 5.7.4 / clause, scope_q; FILTERABLE | `{type?: TS, attrs?: [A], Q, TMP, P}` | `{entities, nextCursor?}`; page over Entities, `lastN` ≤ 100 per Attribute | on_request |
| 32 | `batch_query_temporal` | `temporal_query_post` (11.3.3) | POST `/temporal/entityOperations/query` | 5.7.4 / clause only (matrix gap) | as 31 | as 31 | on_request |

32 tools, 32 distinct matrix rows. Discovery tools 24, 25, 27 and 28 are not FILTERABLE (`crates/antares-api/src/policy.rs:68`): under an engine that narrows, the gate answers `Deny` (`resolve`, `crates/antares-api/src/policy.rs:359-383`), and the probe of 3.1 hides them. That is fail-closed and stays so until matrix open question 5 (add 5.7.5 to 5.7.10 to FILTERABLE) is implemented; after that their answer is the view's types and attributes only (T-3438 point 5).

The matrix gaps (`operation-matrix.md` section 5.1) are the engine's blind spots on every surface and are not repeated per row; they are why the security rules in section 10 do not rely on the engine seeing more than the HTTP handler hands it.

### 3.6 Rows not offered

`operation-matrix.yaml` has 69 operation rows; 37 are not tools: `purge_entities` (rejected: "no tool; a whole-tenant delete is not offered to an agent"), `set_attribute_value` (no route yet, T-3450), the 6 temporal writes (11.2.2 to 11.2.7: not offered by the matrix; history is written by the broker, an agent writes current state), the registry and registry subscription rows (12.2.2 to 12.4.7, 11 rows, `csource_notification` included), the 4 `@context` rows (13.x), the 4 Entity Map rows besides `create_entity_map` (14.2 to 14.4, 14.6), `source_identity` (15.2), the 7 snapshot rows (16.x), `notification` (served by section 5, not as a tool), and `create_entity_map` (cursor only). Adding any of them later is one table entry and one matrix cell change.

```bash
# the mcp column of the matrix: 33 supported, 1 rejected, 35 not_offered
awk '/^- id: /{id=$3} /^    mcp:/{m=1;next} m&&/status:/{print id, $2; m=0}' docs/analysis/operation-matrix.yaml
```

## 4. Resources (task point 2, first half)

### 4.1 Resource table

`{view}` is the view's id as ADR-0023 rule 10 addresses it (`/x/views/{view}/...`); the MCP endpoint is one per view (section 6.3), so a URI naming another view, an unknown view or a malformed URI all answer the same error. 2026-07-28 `server/resources`: "code `-32602` (Invalid Params)" for a resource that is not found (the changelog, minor change 6, moved it from `-32002`). `{id}` is the Entity id, percent-encoded as one RFC 3986 path segment and decoded once.

| URI or template | Method | Matrix row (TS 104 175) | NGSI-LD request | Gate clause | Listed by `resources/list` | Result |
|---|---|---|---|---|---|---|
| `ngsi-ld://{view}/entities/{id}` | `resources/read` | `retrieve_entity` (10.4.2) | GET `/entities/{id}` | 5.7.1, FILTERABLE | no: published as a template only, the Entity set is unbounded | one `contents` item, `mimeType` `application/ld+json`, `text` = the projected Entity; over the byte budget: JSON-RPC error `-32000` (the implementation-defined range, 2026-07-28 changelog minor change 12) whose message names `get_entity` with `pick`/`attrs` (section 8) |
| `ngsi-ld://{view}/types/{type}` | `resources/read` | `retrieve_entity_type_info` (10.6.5) | GET `/types/{type}` | 5.7.7 | yes, one per visible type, paged with `cursor` (2026-07-28 `server/utilities/pagination`) | the EntityTypeInfo document |
| `ngsi-ld://{view}/entities/{id}` | `resources/templates/list` | none (protocol method) | none | none | n/a | the template, `mimeType` `application/ld+json` |
| any URI of the two shapes | `subscriptions/listen` `resourceSubscriptions` | section 5 | section 5 | section 5 | n/a | `notifications/resources/updated` |

Decided (worker-2, owner may override): `types/{type}` is Retrieve Entity Type Information, not the platform's "entities of this type" (`src/mcp/endpoint_facade.rs:1759-1763` runs it as `query_entities type=t`). `resources/read` has no cursor, so an unbounded Entity list there is either cut (the platform reads up to 8 MiB and turns a larger answer into `null`, `:1799-1803`) or refused; a type's description is bounded and is what a client wants to attach as context. Listing the Entities of a type is `query_entities`, with paging.

The `access://` and `schema://` resources of the platform are not broker resources (section 9).

### 4.2 Discovery of resources

`resources/list` lists the visible types only: the view's type selector when it has one; otherwise the answer of `list_types` (row `retrieve_entity_types`, gate 5.7.5) under the caller, which a narrowing engine answers with `Deny` today (section 3.5), so the list is then empty rather than wide. A resource the caller cannot read and one that does not exist are both absent from the list and both `-32602` on read.

### 4.3 Caching (`CacheableResult`)

2026-07-28 `server/utilities/caching` makes `ttlMs` and `cacheScope` mandatory on `server/discover`, `tools/list`, `prompts/list`, `resources/list`, `resources/templates/list` and `resources/read`, and defines the scope: `"public"`: "The response does not contain user-specific data. Any client, shared gateway, or caching proxy MAY store and serve the cached response to any user"; `"private"`: "Cached responses MAY be reused for the same authorization context. Caches MUST NOT be shared across authorization contexts". Its own guidance: "`private` is appropriate for `resources/read` results that depend on the authenticated user, or for filtered list results that vary per user."

| Result | `cacheScope` | `ttlMs` | Why |
|---|---|---|---|
| `server/discover` | `public` | 3 600 000 | versions and capabilities are the same for every caller of the binary |
| `tools/list` | `private` | 60 000 | filtered by the view and by the engine's answer for this subject (3.1); the schema enums carry the view's names (3.2) |
| `resources/templates/list` | `private` | 300 000 | carries the view id |
| `resources/list` | `private` | 0 | the visible types of this subject |
| `resources/read` | `private` | 0 | a view-narrowed Entity |

Why `private` for every view-narrowed result: the narrowing is a function of the subject (`Subject { tenant, headers }`, `crates/antares-api/src/policy.rs:70-79`), so the same request bytes produce different answers for two subjects. A `public` answer may be served by a shared gateway "to any user": the answer narrowed for subject A, served to subject B, hands B the members A may see and B may not (and B loses members it may see, which a model then reports as absent). The cache key is "the request method together with the request parameters" and does not contain the subject, so only the scope rule keeps two subjects' answers apart. `ttlMs: 0` on data because a write by anyone changes it and the notification path (section 5) is opt-in.

`rmcp` 3.5.1 makes this a trap: `CacheScope` derives `Default` as `Public` ("Defaults to [`CacheScope::Public`] when absent from the wire", `crates/rmcp/src/model.rs:1680-1693` at tag `rmcp-v3.5.1`), while `DiscoverResult::new` sets `Private` (`:1343-1354`). The surface therefore never relies on a default: every cacheable result is built by one function taking an explicit scope, and a unit test asserts that every result of `tools/list`, `resources/list`, `resources/templates/list` and `resources/read` serializes `"cacheScope": "private"`.

## 5. Resource subscriptions (task point 2, second half)

### 5.1 What 2026-07-28 changed

The task text names `resources/subscribe`. Revision 2026-07-28 removed it. Changelog, major change 4: "Replace the HTTP GET endpoint and `resources/subscribe`/`resources/unsubscribe` with `subscriptions/listen`: a single long-lived POST-response stream for opted-in server-to-client change notifications. Clients opt in to specific types (`toolsListChanged`, `promptsListChanged`, `resourcesListChanged`, `resourceSubscriptions`); the server acknowledges and tags notifications with `io.modelcontextprotocol/subscriptionId`." `basic/patterns/subscriptions` adds: "The server MUST NOT send notification types the client has not explicitly requested", "The `notifications` field in the acknowledgment reflects the subset the server agreed to honor", and the notification itself carries only the URI (`notifications/resources/updated` with `"uri"`). `rmcp` 3.5.1 implements it: `ServerHandler::listen` (`crates/rmcp/src/handler/server.rs:147-181`) and a sink "scoped to one `subscriptions/listen` request" (`crates/rmcp/src/service/server.rs:135`); `resources/subscribe` is marked "legacy-only; use subscriptions/listen for protocol version 2026-07-28" (`crates/rmcp/src/model.rs:2000`).

### 5.2 Design

Capabilities: `resources: { subscribe: true, listChanged: false }`, `tools: { listChanged: false }`. A tool list changes when a grant changes, which the broker is not told about; `ttlMs` (4.3) bounds staleness instead, and a stale tool fails at the gate.

1. **Open.** The client sends `subscriptions/listen` with `resourceSubscriptions: [uri, ...]`. Bounds: at most 64 URIs per stream, at most 4 open streams per subject per process (proposals, section 8). Every other notification type is omitted from the acknowledgment.
2. **Admit each URI.** Only `ngsi-ld://{view}/entities/{id}` is accepted; a `types/{type}` URI is omitted from the acknowledgment (a type description changes with the data of every Entity of that type, and there is no NGSI-LD subscription for it). For an entity URI the surface runs `retrieve_entity` (gate 5.7.1) through `AppState::call` under the caller. A 404, whether the Entity is absent or hidden by the view, omits the URI from the acknowledgment without a reason, so the acknowledgment is no existence oracle beyond what `resources/read` already answers.
3. **One internal NGSI-LD Subscription per admitted URI**, created by `POST /ngsi-ld/v1/subscriptions` through `AppState::call`, so it is `create_subscription` (10.5.2, gate 5.8.1) under the caller's subject, which the broker stores in `__subject` (ADR-0020: "The subject is the subscriber's, taken from the creating request and stored with the subscription in `__subject`"). Body: `entities: [{id, type}]` (the type from step 2), `watchedAttributes` = the view's attribute allowlist (absent when the view has none), `q`/`scopeQ` = the view's predicate until T-3438 point 9 applies the view at match time, `notification.attributes` = the same allowlist, `notification.endpoint.uri` = `x-antares-mcp:<stream id>` (128 random bits), `expiresAt` = now + the stream's maximum lifetime (1 hour, proposal), `throttling` = 1 second (proposal).
4. **Delivery.** A change runs the broker's own matcher (TS 104 175 10.5.7: "the notification shall be sent whenever there is a change in the watched Attributes"), then `notify::deliver_as` (`crates/antares-api/src/notify.rs:1982-2318`), which calls `policy::pre_notify` (`notify.rs:2084`) under the stored subject: `Drop` sends nothing, `Filter` projects. The sink for scheme `x-antares-mcp` (`McpSink`, a `NotificationSink`, `crates/antares-notifier/src/lib.rs:301-332`) does not forward the Notification: it maps the subscription id to its stream and URI and pushes `notifications/resources/updated {uri}` onto that stream. The client then reads the resource with `resources/read`, which is gated again (4.1). The Notification's data never reaches the client through this path, so a `pre_notify` projection gap cannot leak data here; what the client learns is "this Entity changed", decided per change by the matcher and `pre_notify`.
5. **Close.** When the stream ends (client closes the SSE response, transport drop, server shutdown with the graceful-closure result of `basic/patterns/subscriptions`), the surface deletes each internal Subscription with `DELETE /ngsi-ld/v1/subscriptions/{id}` through `AppState::call` (row `delete_subscription`, gate 5.8.5). A process that dies leaves them to `expiresAt`.

Rows used: `retrieve_entity`, `create_subscription`, `notification`, `delete_subscription`. ADR-0020 phases: `on_request` at steps 2, 3 and 5, `pre_notify` at step 4.

### 5.3 Rules that keep the stream inside the view

- **A hidden change makes no event.** With `watchedAttributes` = the view's allowlist, a change to a hidden attribute is not a change in a watched Attribute (TS 104 175 `10/10.5.2.4.md`: "Watched Attributes are those that trigger a new notification when they are changed"), so neither the event nor its timing tells the client anything about a hidden member (T-3438 point 9, "suppress a notification whose only change was hidden").
- **An Entity that leaves the view stops its events.** The subscription's `q`/`scopeQ` are the view's, and 10.5.7 includes only Entities "that match ... the query, geoquery and Scope query conditions".
- **No client can write into a stream.** `McpSink::parse_endpoint` refuses every `x-antares-mcp:` URI whose stream id is not open in this process, so a client creating a Subscription with that scheme over NGSI-LD HTTP or `create_subscription` gets `BadRequestData` (ADR-0016 decision 3). `McpSink::deliver` additionally drops a Notification whose `subscriptionId` is not one the surface created for that stream, so a stream id read from `list_subscriptions` (which is not FILTERABLE and lists the tenant's Subscriptions) cannot be reused.
- **Bounded memory.** One pending flag per (stream, URI): a burst of changes to one Entity is one `notifications/resources/updated` until the client has been sent it. The stream's queue is therefore bounded by its URI count.
- **The egress guard.** `McpSink` opens no socket and declares `network() == false` (default `true`, `crates/antares-notifier/src/lib.rs:322-324`), so `deliver_as` skips the egress check (`crates/antares-api/src/notify.rs:2047`). The unit test `every_shipped_sink_is_policed` (`crates/antares-notifier/src/lib.rs:470-489`) asserts that every sink shipped in that crate is policed; `McpSink` lives in `antares-mcp` and is registered only by a binary built with feature `mcp`. T-3443 has to restate the rule as "every network sink is policed; the only non-network sink is the in-process MCP sink" and give it a test in the full binary.

### 5.4 Legacy clients and several pods

A 2025-11-25 or 2025-06-18 client would subscribe with `resources/subscribe` and receive over the GET SSE stream of its session. The server answers `initialize` with `resources.subscribe: false` and `GET` with `405` (2025-11-25 `basic/transports`: "The server MUST either return `Content-Type: text/event-stream` in response to this HTTP GET, or else return HTTP 405 Method Not Allowed"). No session is issued (2025-11-25: "A server using the Streamable HTTP transport MAY assign a session ID"). So a legacy client reads resources and calls read tools, and does not subscribe.

Several pods: unknown which pod delivers a Notification under `bus=nats`. The subscription mirror is "A per-instance tenant-keyed document mirror (bus=nats)" (`crates/antares-api/src/mirror.rs:14-18`), and the change-queue consumer that runs delivery was not traced here. If delivery for a Subscription can run on a pod that does not hold its stream, `McpSink` there has no stream and the event is lost. Recommendation for T-3443: `McpSink` publishes the event on a NATS subject keyed by stream id and the pod holding the stream relays it; with one pod (the dev cluster) the in-process map suffices.

## 6. Protocol revision and transport (task point 3)

### 6.1 What our clients speak

| Client | Revision it negotiates | Evidence |
|---|---|---|
| Platform gateway façade (server today) | `2025-06-18` | `PROTOCOL_VERSION`, `src/mcp/endpoint_facade.rs:53-54` |
| Portal MCP (server) | prefers `2026-07-28`, accepts `2025-11-25`, `2025-06-18`, `2025-03-26` | `PROTOCOL_VERSIONS`, `joinedcontext-portal src/mcp/mod.rs:590`, `negotiated_version` `:592-603` |
| Platform assistant (client of the façade) | none: it sends bare `tools/list` and `tools/call` JSON-RPC with no `initialize`, no `_meta`, no `MCP-Protocol-Version` header | `joinedcontext-platform crates/assistant/src/chat/mcp.rs:48-57` (`rpc`), `:91`, `:129` |
| Claude (claude.ai connectors, Claude Code) | unknown: not read from any source today; to be measured against the broker by the MCP conformance suite (T-3449) | none |

The assistant is the one caller that breaks: 2026-07-28 `basic/transports/streamable-http`: "Every POST request to the MCP endpoint MUST include an `MCP-Protocol-Version` header", and "A server that does not support such clients MUST reject a request without the header" (a missing header is read as 2025-03-26 only by servers that support that revision). Per GOAL point 11 its switch is a caller task: send `_meta` with `io.modelcontextprotocol/protocolVersion: "2026-07-28"` and the `MCP-Protocol-Version`, `Mcp-Method`, `Mcp-Name` headers, with a test against the broker surface.

### 6.2 Decision

Decided (worker-2, owner may override): the broker MCP is a dual-era server, modern first.

- **Modern, `2026-07-28`**, stateless per the changelog (major changes 1 and 2: no `Mcp-Session-Id`, no `initialize`; "Every request now carries its protocol version and client capabilities in `_meta`"). `server/discover` is implemented (major change 3: "servers MUST implement this RPC"). Every result carries `resultType` (major change 8). `Mcp-Method` and `Mcp-Name` are required and checked against the body (minor change 4; a mismatch is `400` with `HeaderMismatch`, code `-32020` per minor change 12). No `ping`, no `logging/setLevel` (major change 5). Tasks extension not offered.
- **Legacy, `2025-11-25` and `2025-06-18`**, served when the first message is `initialize` (2026-07-28 `basic/versioning`: "An `initialize` request selects legacy semantics ... A dual-era server MAY serve both eras concurrently on the same endpoint"). `rmcp` names the newest handshake revision `ProtocolVersion::LATEST_WITH_INITIALIZE = V_2025_11_25` and the first stateless one `NO_INITIALIZE = V_2026_07_28` (`crates/rmcp/src/model.rs:169-201` at `rmcp-v3.5.1`); `initialize` is answered with `2025-11-25` unless the client asks `2025-06-18`. Served without a session, without resource subscriptions (5.4) and without write tools (3.4: legacy elicitation is a server-initiated request on an SSE stream, which this server does not hold open). `2025-03-26` and older are refused, naming the supported versions ("A server that supports only modern versions SHOULD name the protocol versions it supports in any error it returns to an `initialize` request").
- **Transport: Streamable HTTP, POST only**, one endpoint per view (6.3). A response is `application/json`; `text/event-stream` only for `subscriptions/listen` and for a call that emits progress. `GET` and `DELETE` answer `405`.

### 6.3 Endpoint and `rmcp` configuration

Endpoint: `POST /x/views/{view}/mcp`, beside the view's NGSI-LD root `/x/views/{view}/ngsi-ld/v1` (ADR-0023 rule 10), mounted by the `antares-mcp` crate's `ApiSurface` below `/x` (`crates/antares-api/src/surface.rs:38-50` admits any prefix below `/x`), selected by `ANTARES_API_SURFACES` (`crates/antares-broker/src/main.rs:463-482`). The tenant is the one the view pins (ADR-0023 rule 10: an `NGSILD-Tenant` naming another tenant is 400), copied into every inner request by `AppState::call` (`PROPAGATED`, `crates/antares-api/src/state.rs:63-67`).

Pin: `rmcp = { version = "=3.5.1", default-features = false, features = ["server", "transport-streamable-http-server"] }`. Facts (crates.io API, read 2026-10-10): 3.5.1 published 2026-10-05, licence Apache-2.0, `rust-version` 1.88 (the broker's is 1.97, `Cargo.toml:28`); 3.5.0 (2026-09-28) moved `LATEST` to 2026-07-28 and added `LATEST_WITH_INITIALIZE` (rust-sdk PR #1105), 3.5.1 fixed the discover lifecycle (PR #1248). An exact pin because a minor release already changed which revision `LATEST` names; Renovate or a task bumps it with the conformance suite (T-3449) green. Feature `server` pulls `schemars` 1.x (`crates/rmcp/Cargo.toml:66`, `:128`), which the broker's lockfile does not hold yet; `cargo deny` and `cargo audit` cover it (T-3447). Whether `rmcp` contains `unsafe`: unknown (T-3447 runs the check).

`StreamableHttpServerConfig` (`crates/rmcp/src/transport/streamable_http_server/tower.rs:78-187`, defaults `:195-212`, at the tag) has unsafe defaults for a public broker, so each is set explicitly and tested:

| Field | `rmcp` default | Set to | Reason |
|---|---|---|---|
| `legacy_session_mode` | `true` | `false` | no sessions in either era (5.4) |
| `json_response` | `false` | `true` | plain JSON answers where no stream is needed |
| `allowed_hosts` | `localhost`, `127.0.0.1`, `::1` | the deployment's host names (env) | Host validation against DNS rebinding |
| `allowed_origins` + `enforce_origin_validation()` | empty list, which "disables Origin validation" | the deployment's allowed origins, validation on | 2026-07-28 `basic/transports/streamable-http`: "Servers MUST validate the `Origin` header on all incoming connections to prevent DNS rebinding attacks" |
| `max_request_body_bytes` | 4 MiB (`DEFAULT_MAX_REQUEST_BODY_BYTES`, `tower.rs:55`) | `MCP_MAX_REQUEST_BYTES` = 1 MiB (section 8) | input bound |
| `stateless_protocol_metadata_required` | `false` ("an absent header is treated as protocol version `2025-03-26`") | `true` | 2025-03-26 is not supported (6.2) |

`rmcp` puts the request's `http::request::Parts` into the handler's extensions (`tower.rs:1536-1544`), which is where the handler reads the subject and tenant headers it hands to `AppState::call` (section 7).

## 7. Authentication and the subject (task point 4)

Decided (worker-2, owner may override): the broker MCP parses no token and holds no OAuth role; the subject is exactly what ADR-0020 already defines.

- **Where the subject comes from.** `ANTARES_POLICY_SUBJECT_HEADERS` is read once into `policy::SUBJECT_HEADERS` (`crates/antares-api/src/policy.rs:473-486`) and turned into a `Subject` by `policy::subject_of` (`:487-503`) inside `gate` (`:567-584`, call at `:579`). Its non-test readers (cg, query below): `subject_of`, `AppState::call` (`crates/antares-api/src/state.rs:612`) and `federation::forward` (`crates/antares-api/src/federation.rs:1130`, which drops them from forwards). ADR-0020: "`Subject { tenant, headers }` carries the identity headers named by `ANTARES_POLICY_SUBJECT_HEADERS`, opaque to the broker — it parses no credential and validates no token."
- **How a tool call carries it.** The MCP handler passes the outer request's `HeaderMap` (from `http::request::Parts`, 6.3) as `caller` to `AppState::call`, which copies "every header `ANTARES_POLICY_SUBJECT_HEADERS` names, so the policy engine is asked about the caller rather than about the façade" (`crates/antares-api/src/state.rs:556-557`, loop `:612-622`), plus `NGSILD-Tenant`, `NGSILD-Snapshot` and `Link` (`PROPAGATED`, `:63-67`). The surface sets no subject header itself and never builds one from a tool argument; the selector-argument rule of the platform (`SELECTOR_ARGUMENTS`, `src/mcp/endpoint_facade.rs:56-57`) is kept, so `tenant`, `space` and their spellings are refused as arguments.
- **The `Authorization` header stops at the gateway.** `AppState::call` copies only the propagated and subject headers, so a bearer token on the outer request does not reach the inner request. A deployment must not name `authorization` in `ANTARES_POLICY_SUBJECT_HEADERS`: the engine would then parse tokens, which ADR-0020 keeps out of the broker. T-3447 adds the startup check (refuse `authorization` and `cookie` as subject headers).
- **Who is the OAuth resource server.** 2026-07-28 `basic/authorization`: "Authorization is OPTIONAL for MCP implementations", but a protected MCP server "acts as an OAuth 2.1 resource server" and "MUST validate that access tokens were issued specifically for them as the intended audience ... Invalid or expired tokens MUST receive a HTTP 401 response". In a deployment that role is the gateway's: it validates the token and audience, answers 401, serves the RFC 9728 protected-resource metadata (T-3436 R7, R9, D7: STAYS), strips any client-sent copy of the subject and tenant headers (`middleware::tenancy::strip_client_headers`, `src/middleware/tenancy.rs:33`; `pin_tenant`, `:46`) and sets them from the verified token (T-3436 gap G15). ADR-0020: "Authentication, token validation, rate limiting and quotas do not move". The broker never answers 401.
- **What makes the headers trustworthy.** Only the gateway may reach `/x/views/{view}/mcp`: the same assumption the NGSI-LD API under ADR-0020 already makes, enforced by the deployment's NetworkPolicy (joinedcontext-deployment). A broker reachable directly would believe any subject header; this is the threat T-3447 names for every surface, not an MCP one.
- **Logs.** `Subject`'s `Debug` prints header names, not values (`crates/antares-api/src/policy.rs:81-92`); the surface logs tool name, row id, outcome and duration, never arguments, subject values or results.

```cypher
// readers of the subject header list, and who builds a Subject (Antares clone)
MATCH (c:Symbol)-[r:REFERENCES]->(g:Symbol {path:'antares_api::policy::SUBJECT_HEADERS'})
WHERE c.is_test=false RETURN c.path, c.file, r.line;
MATCH (c:Symbol)-[r:CALLS]->(g:Symbol {path:'antares_api::policy::subject_of'})
WHERE c.is_test=false RETURN c.path, c.file, r.line;
// the seam and the in-process call
MATCH (s:Symbol) WHERE s.path IN ['antares_api::state::AppState::call','antares_api::surface::ApiSurface',
  'antares_api::surface::check_prefix','antares_api::policy::gate','antares_api::policy::pre_notify',
  'antares_api::policy::resolve','antares_api::notify::deliver_as']
RETURN s.path, s.file, s.line, s.end_line;
MATCH (t:Symbol)-[:IMPLEMENTS]->(i:Symbol {name:'ApiSurface'}) RETURN t.path, t.file, t.line;
// sinks a subscription can be delivered by
MATCH (t:Symbol)-[:IMPLEMENTS]->(i:Symbol {name:'NotificationSink'}) RETURN t.path, t.file, t.line;
```

`subject_of` callers (6): `policy::gate`, `subscriptions::create` (`crates/antares-api/src/subscriptions.rs:821`, stores `__subject`), `entity_maps::mine`, `entity_map::map_if_accessible`, `entity_map::stamp_subject`, `snapshots::new_meta`. `ApiSurface` implementations today: `Admin` (`crates/antares-api/src/lib.rs:199`), `ExampleSurface` (`examples/plugin-example/src/surface.rs:17`) and three test surfaces; `antares-mcp` will be the first shipped `/x` surface.

## 8. Output size limits and pagination (task point 5)

Rule: a tool result is never cut. A result that does not fit is a shorter page with a cursor, and a single item that does not fit is a tool error saying how to narrow the call. Half an Entity is a wrong Entity, and a model cannot tell a cut answer from a complete one; the platform holds the same rule for schema documents ("A document over this is refused with the way to narrow it rather than cut", `src/mcp/endpoint_facade.rs:2044-2050`) but not for data (8 MiB read into `null`, `:1663`; the assistant cuts at 20 000 characters, `crates/assistant/src/chat/mcp.rs:11-12`).

| Limit | Value (proposal; measured in T-3467) | Applies to | On breach |
|---|---|---|---|
| `MCP_MAX_REQUEST_BYTES` | 1 MiB | every POST (`rmcp` `max_request_body_bytes`) | HTTP 413 from the transport |
| `MCP_MAX_BATCH` | 100, and never above `MAX_BATCH_ITEMS` (1000, `crates/antares-api/src/bounds.rs:42-49`) | tools 10 to 14, `batch_query` `ids` | schema refusal (`maxItems`) |
| string, list and depth bounds | section 3.2; JSON depth ≤ `MAX_JSON_DEPTH` 64 (`bounds.rs:29`) | every argument | schema refusal, or the broker's 400 |
| `limit` | default 20, 1..1000 (`max_limit`, `crates/antares-api/src/state.rs:425`; above it the broker answers 403 `TooManyResults`, `crates/antares-api/src/paging.rs:84-88`) | tools 16, 17, 22, 31, 32 | schema refusal |
| `MCP_MAX_RESULT_BYTES` | 64 KiB of serialized `structuredContent`, env `ANTARES_MCP_MAX_RESULT_BYTES` | every tool result and `resources/read` | shorter page; one item over it: `isError` naming `pick`/`attrs`/`lastN` |
| `lastN` | 1..100 | tools 30 to 32 | schema refusal |
| call deadline | 10 s per tool call, 15 s for a fan-out (the platform's `MEMBER_DEADLINE`, `CALL_DEADLINE`, `src/mcp/server.rs:32-36`) | every call | `isError` "timed out" |
| `subscriptions/listen` | 64 URIs per stream, 4 streams per subject per process, 1 h lifetime | section 5 | URI omitted from the acknowledgment; a fifth stream answered with an error |
| rate | none in the broker | | the gateway's (T-3436 D13, K35: STAYS) |

Paging algorithm for a list tool:

1. Ask the broker for `limit` rows with `offset` from the cursor and `count=true` when the caller asked for `count`.
2. Serialize the rows in order and stop before the first row that would take the result over `MCP_MAX_RESULT_BYTES`.
3. `nextCursor` = the cursor of the first row left out: `offset + rows included`, not `offset + limit`. With the narrowing in the store (ADR-0020: the `q` and `scopeQ` "are conjoined into the query the store runs"), the offset counts narrowed rows, so this is exact; the platform needed to read the offset back from `Link` because it narrowed after the broker (`src/mcp/endpoint_facade.rs:1648-1662`).
4. No `nextCursor` on the last page (`server/utilities/pagination`: "An optional `nextCursor` field if more results exist").
5. `total` = `NGSILD-Results-Count`, which the store computes after the narrowing, so it counts what the caller may see.

Cursor: base64url of `{v: 1, tool, argsDigest, offset, entityMap?}`, at most 2048 characters, opaque to the client ("The cursor is an opaque string token", `server/utilities/pagination`). A cursor for another tool or other arguments is refused. Integrity protection is not needed: a forged cursor can only choose another offset of the same gated query, which the caller can ask anyway (compare `basic/patterns/mrtr` rule 4, "Integrity protection MAY be omitted only when tampering can cause nothing worse than request failure"). When the caller wants a stable page sequence it passes `consistent: true` on the first call: the surface sends `entityMap=true`, keeps the returned `NGSILD-EntityMap` (`crates/antares-api/src/entities.rs:955-962`) in the cursor and sends it back on the next pages. That is row `create_entity_map` (14.5) used by `query_entities`; the map records `__subject`, and a map presented by another subject "is answered exactly as an expired one is" (ADR-0020), so a cursor passed to another caller gives that caller nothing.

Temporal: TS 104 176 temporal paging (`intervalafter`/`intervalbefore` Link) is not implemented today (T-3456: the broker signals 206 with `Content-Range`). Round 1 pages `query_temporal` over Entities with the algorithm above and bounds each Entity's history with `lastN` and the byte budget; a history over the budget is `isError` naming a narrower `timerel`/`lastN`. Instance paging follows T-3456.

## 9. What moves into the broker and what stays in the platform MCP (task point 6)

Decisions refer to `gateway-to-broker-map.md` (T-3436): D5 (MCP tools MOVE), D6 (hub and named servers STAY, owner decision O-2), D7 (RFC 9728 metadata STAYS), D4 (access description MOVES to the engine addon), D11 (schema surface MOVES to LinkML), D13 (rate limits, audience, tenancy STAY), K42 (elicitation MOVES). Platform paths are under `crates/context-gateway/`.

| Platform item | Decision | Broker side | Caller switch before the delete (GOAL point 11) |
|---|---|---|---|
| The 14 data tools of `TOOLS` (`src/mcp/endpoint_facade.rs:108-426`, all but `describe_access`, `describe_schema`) and `request_for` (`:2239-2456`) | MOVES (D5) | tools 11, 15 to 17, 19, 21, 22 and 24 to 32 (section 3.5): 16 broker tools, because `list_types` and `list_attributes` are split and `upsert_entity` is renamed (3.3) | hub (`Hub::forward`, `src/mcp/hub.rs:286-334`) and named servers (`Hub::fan_out`, `src/mcp/server.rs:285-427`) call the broker's `/x/mcp/{view}` instead of `endpoint_facade::handle` |
| Tools that do not exist in the platform (tools 1 to 10, 12 to 14, 18, 20, 23) | new in the broker | section 3.5, confirmed per 3.4 | none |
| `tools_for`, `granted` (`:1222-1241`, `:1290-1307`): discovery by PDP probe | MOVES (D5) | 3.1: view allowlist plus `PolicyEngine::offers` | the joinedcontext engine addon implements `offers` |
| `validate`, `VALIDATORS`, `broken_rule` (`:1122-1220`), argument tables (`:487-1120`) | MOVES (D5) | 3.2, JSON Schema 2020-12 built per view | none |
| `SELECTOR_ARGUMENTS` refusal (`:56-57`, `:1509-1520`) | MOVES (D5) | kept as is (section 7) | none |
| `answered`, `paged`, `warned`, `refused`, `output_schema` (`:1904-2042`) | MOVES (D5) | same result shape; `nextCursor` from the byte-bounded page (8) | the assistant reads `nextCursor` instead of cutting at `MAX_RESULT_CHARS` (`crates/assistant/src/chat/mcp.rs:11-12`) |
| `resources`, `read_resource`, `parse_resource` for `ngsi-ld://` (`:1687-1866`) | MOVES (D5), `types/{type}` changes meaning (4.1) | section 4 | none known; no platform caller reads `ngsi-ld://.../types/` (unknown for external clients) |
| `access://` resource and `describe_access` (`:257`, `:1868-1891`) | MOVES (D4) to the engine addon, not an MCP tool | not served by `antares-mcp` round 1 | the portal's access views read the engine addon's document (T-3436 D4) |
| `schema://` resources and `describe_schema` (`:275`, `:2076-2199`) | MOVES (D11) to the LinkML surface | not served by `antares-mcp` round 1; T-3441 decides whether `antares-mcp` lists the LinkML document as a resource when both features are built | callers read `/x/linkml` |
| `elicitation.rs` (`src/mcp/elicitation.rs`, 306 lines) | MOVES (K42), replaced by MRTR `InputRequiredResult` | 3.4 | clients answer the standard elicitation instead of `params.elicitation` |
| `initialize`, `ping` handling, `PROTOCOL_VERSION = "2025-06-18"` | MOVES, replaced | section 6: 2026-07-28 plus legacy 2025-11-25 and 2025-06-18 through `rmcp` | assistant sends 2026-07-28 metadata (6.1) |
| R5 `POST /api/endpoint/{slug}/mcp`, R25 `POST /cs/{space}/mcp` (`app::mcp_message` `src/app.rs:3270`, `space_mcp_message` `:3328`) | the façade behind them MOVES; the public route STAYS as the gateway's authenticated entry | none | the route keeps authenticating (`admit_by`, `admit_space`), pins the tenant, sets the subject headers and forwards the POST to the broker; its tests switch to a broker double |
| `mcp::hub` (R6 `/api/mcp`, `list_endpoints`, `Hub::spend` bucket) | STAYS (D6) | none | its tool calls go to the broker (row 1 of this table) |
| `mcp::server` (R8 named MCP servers, `fan_out`, `FAN_OUT`, deadlines) | STAYS (D6) | none | as above |
| `protected_resource` (R7, R9; `src/mcp/hub.rs:491-505`, `src/mcp/server.rs:486-506`) | STAYS (D7) | none | none |
| Token verification, audience, tenancy pinning, header stripping, rate limits, audit line | STAYS (D13, K34 to K36) | section 7 relies on them | none |
| Federation member names in `instructions`, `described`, `jc:source` (`:1243-1288`, `:1914-1928`) | STAYS in the hub and named servers (they know the platform's members); the broker reports partial federated answers through its own warnings, carried as `warnings` | `warnings` from `NGSILD-Warning` | none |

What the broker MCP adds that the platform MCP never had: `subscriptions/listen` (section 5), `CacheableResult` (4.3), `server/discover`, cursors that do not depend on reading `Link` (8), and the write tools.

## 10. Security

Every rule names the test T-3444 is to plan for it (names are proposals).

1. **Entity content is data, never instructions.** A tool result is `structuredContent` (the NGSI-LD JSON as the broker rendered it, under the tool's result key) plus one text block that is the same JSON serialized; no entity value is ever rendered into prose. No entity value, Entity id or attribute value is interpolated into a tool `description`, the server `instructions`, an error message, an elicitation message or a resource `name`. Tool descriptions and `instructions` are static strings; schema enums are view names filtered by a name pattern (3.2). A refusal's text is the status and the ProblemDetails `title` only; the ProblemDetails document (whose `detail` may echo the caller's own input) travels in `structuredContent`. MCP `server/tools` Security Considerations: "Servers MUST: Validate all tool inputs; Implement proper access controls; Rate limit tool invocations; Sanitize tool outputs". Test: `an_entity_value_shaped_like_an_instruction_is_returned_as_json_and_appears_in_no_description_error_or_message`.
2. **No tool bypasses the view.** Every tool and every resource read is one request through the `antares-ops` operation layer (built over `AppState::call` into `router()`, `surfaces-threat-model.md` section 9), so the operation's own `gate!` runs once. The `antares-mcp` crate depends on `antares-ops` and `rmcp` only, never on `antares-api` internals, `AppState`'s store fields or any store or driver crate (module rule M2); T-3447 makes that a CI rule (`cargo tree` plus a `cg` query: no `CALLS` edge from `antares_mcp::` to a store symbol). Tests: a counting engine walks all 32 tools and both resource shapes and asserts exactly one gate call each, the way `every_route_asks_the_policy_engine_exactly_once` (`crates/antares-api/tests/policy_gate_seam.rs:209`) walks the router; `a_denied_operation_is_a_tool_error_and_never_an_empty_result`.
3. **Discovery is no oracle.** A hidden tool, an unknown tool, a hidden resource and an unknown resource answer byte-identically; a hidden type and a non-existent type get the same schema refusal (3.1, 3.2, 4.2). Test: `a_hidden_and_an_absent_name_answer_the_same_bytes_on_every_method`.
4. **Joins wait for T-3470.** The join walk does not apply the Filter's `q` and `scopeQ` to the target (T-3470; `operation-matrix.md` open question 1). Decided (worker-2, owner may override): `join` and `joinLevel` are left out of every tool schema until T-3470 is done, so the MCP surface cannot be used to reach an out-of-view Entity one hop away. Test: `join_is_not_an_argument_until_the_target_is_checked`, then the T-3470 test on the MCP path.
5. **Inference through `q`, `count` and paging.** The surface passes `q`, `scopeQ` and the geo arguments through unchanged; the view's rule that a hidden attribute does not exist during `q` evaluation (ADR-0023 rule 2) therefore holds on MCP as on HTTP. `total` and `nextCursor` are computed after the store's narrowing (8). Tests: `q_on_a_hidden_attribute_answers_as_on_an_attribute_that_does_not_exist`, `total_and_next_cursor_count_only_visible_entities`.
6. **Writes stay in the caller's sight.** Writes are judged as on HTTP; the pre-image, post-image and per-Entity batch checks the matrix marks "not enforced today" (`operation-matrix.md` section 6) are the operation layer's, and the MCP surface adds no write path around them. The confirmation (3.4) is in addition to the gate, never instead of it, and runs after it. Tests: `a_write_the_engine_denies_never_asks_the_person`, `a_confirmation_answers_one_call_once`.
7. **Subscriptions cannot exfiltrate.** `create_subscription` and `update_subscription` accept only `notification.endpoint.uri` values under a deployment allowlist (`ANTARES_MCP_SUBSCRIPTION_ENDPOINTS`, empty by default, so the tools refuse every endpoint until a deployment names one), on top of the egress guard (ADR-0010) and the confirmation. A model that read an instruction in an Entity cannot have data sent to an address it chose. Test: `a_subscription_to_an_endpoint_outside_the_allowlist_is_refused_before_the_gate`.
8. **Resource-update streams leak neither data nor hidden changes.** Events carry only the URI; a change to a hidden attribute triggers nothing; an Entity outside the view triggers nothing; no client can write into a stream (5.3). Tests: `a_change_to_a_hidden_attribute_sends_no_resource_update`, `a_subscription_naming_a_stream_it_did_not_open_is_refused`.
9. **No answer is shared across subjects.** `cacheScope: "private"` on every view-narrowed result (4.3). Test: `every_cacheable_result_but_discover_is_private`.
10. **Forged state is refused.** `requestState` is HMAC-protected and single use (3.4); a cursor can only re-ask the same gated query (8). Tests: `a_tampered_request_state_is_refused`, `a_replayed_request_state_is_refused`, `a_cursor_of_another_tool_is_refused`.
11. **Transport.** Origin and Host validation on, `Mcp-Method`/`Mcp-Name` checked against the body, request body bounded, no sessions (6.3). Tests: `a_foreign_origin_is_403`, `a_header_that_disagrees_with_the_body_is_400_header_mismatch`.
12. **Per-call limits.** Every input and output bound of section 8 is fail-closed and tested at the bound and one past it; the tool probe (3.1) denies every tool when the engine fails. Tests: one per row of the section 8 table; `an_engine_that_fails_the_probe_lists_no_tool`.
13. **The subject is not forgeable through MCP.** No tool argument becomes a header; `tenant`/`space` arguments are refused; `authorization` cannot be a subject header (7). Test: `a_tenant_or_subject_in_the_arguments_is_refused_and_changes_nothing`.
14. **New code standard.** `antares-mcp` follows GOAL point 8: `#![forbid(unsafe_code)]`, the clippy deny set, bounded allocation for every input-driven size (the bounds above), and a fuzz target for the tool-argument to NGSI-LD request translation and for the cursor decoder (T-3444).

## 11. Unknowns and open items

| Item | State | Who resolves it |
|---|---|---|
| Which revision Claude (claude.ai connectors, Claude Code) negotiates | unknown | T-3449 conformance run with a real client |
| Which pod delivers a Notification under `bus=nats`, and therefore whether `McpSink` needs a relay | unknown (5.4) | T-3443 |
| Whether `rmcp` 3.5.1 serves both eras on one `StreamableHttpService` with `legacy_session_mode: false` | not verified beyond the config doc comment (`tower.rs`, `legacy_session_mode`: "Only applies to legacy protocol versions") | T-3449 |
| Whether `rmcp` contains `unsafe` | unknown | T-3447 |
| The view's addressing and its operation allowlist format | settled by ADR-0023: `/x/views/{view}/` and operation-matrix row ids, as proposed in 3.1; the MCP endpoint of a view is `/x/views/{view}/mcp` | done |
| `PolicyEngine::offers` (ADR-0020 amendment) | proposed (3.1) | T-3443 |
| `every_shipped_sink_is_policed` restated for the in-process sink | proposed (5.3) | T-3443 |
| Discovery tools under a narrowing engine | hidden until 5.7.5 to 5.7.10 are FILTERABLE (matrix open question 5) | T-3445 files it |
| `join`/`joinLevel` on MCP | off until T-3470 | T-3470 |
| Temporal instance paging | after T-3456 | T-3456 |
| Every number in section 8 | proposals | T-3467 measures |
| `list_subscriptions` on the platform | untested there (2.2); the broker tool gets its own tests | T-3444 |
| Matrix name corrections (3.3) | to be applied to `operation-matrix.yaml` | T-3445 |

## 12. Related

- `docs/analysis/operation-matrix.yaml`, `docs/analysis/operation-matrix.md` (T-3437), `docs/analysis/gateway-to-broker-map.md` (T-3436), `docs/analysis/cim009-to-ts104175.md`
- `docs/adr/ADR-0014-extension-hooks-model.md`, `docs/adr/ADR-0016-notification-bindings-behind-the-sink-registry.md`, `docs/adr/ADR-0020-policy-seam.md`, `docs/adr/ADR-0010-egress-allow-private-by-default.md`
- `crates/antares-api/src/policy.rs`, `crates/antares-api/src/state.rs`, `crates/antares-api/src/surface.rs`, `crates/antares-api/src/notify.rs`, `crates/antares-notifier/src/lib.rs`
- TS 104 175 `10/10.5.2.4.md`, `10/10.5.7.md`
- MCP 2026-07-28: `changelog`, `basic/versioning`, `basic/transports/streamable-http`, `basic/authorization`, `basic/patterns/subscriptions`, `basic/patterns/mrtr`, `server/tools`, `server/resources`, `server/discover`, `server/utilities/caching`, `server/utilities/pagination`; MCP 2025-11-25: `basic/transports`, `server/resources`
- tasks T-3438 (views), T-3441 (OData, LinkML), T-3443 (architecture ADR), T-3444 (test plan), T-3445 (implementation tasks), T-3447 (threat model), T-3449 (conformance), T-3456, T-3470
