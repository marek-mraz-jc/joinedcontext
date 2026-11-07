# Operation matrix: every surface operation mapped to one TS 104 175 operation and its policy check

Sources: Antares-NGSI-LD-Context-Broker `7185e1a` (branch `agent/worker-2/T-3436`), joinedcontext-platform `59a8f70`, ETSI TS 104 175 V1.1.1 and TS 104 176 V1.1.1 (2026-09) under `/workspace/etsi-specs/`, OData Version 4.01 Part 1 Protocol, Part 2 URL Conventions and JSON Format (docs.oasis-open.org, read 2026-10-10). Task T-3437. The machine-readable form is `operation-matrix.yaml` in this folder; this page renders it and documents its schema. When the two disagree, the yaml is the one tests read.

## 1. Scope and the clause numbering

The task text says "Clause 5 operations". That is the GS CIM 009 V1.9.1 numbering. TS 104 175 V1.1.1 moved the abstract operations out of clause 5 into seven clauses:

| TS 104 175 clause | Operations |
|---|---|
| 10 | core: 10.2 provision, 10.3 batch, 10.4 consumption, 10.5 subscriptions, 10.6 discovery |
| 11 | temporal |
| 12 | registry, registry subscriptions |
| 13 | @contexts |
| 14 | Entity Maps |
| 15 | Context Source identity |
| 16 | Snapshots |

Every row carries both numbers: `ts104175` and `cim009`, translated with `docs/analysis/cim009-to-ts104175.md`. The code still names CIM 009 clauses (`policy::FILTERABLE`, `policy::WHOLE_TENANT` and every `gate!` call), so the yaml also carries `gate_clause`: the exact string the handler passes to the seam. Two operations are new in TS 104 175 and have no CIM 009 clause: 10.2.6 Set Attribute Value and 10.4.4 Retrieve Attribute Value.

The HTTP binding of each operation is taken from TS 104 176, whose method clauses state the operation they bind, for example `ts-104176-ngsi-ld-http-binding/7/7.2.3.2.md`: "This method is bound to the operation ... clause 10.2.2". The clause file is named in every row.

Surfaces, one column each:

| column | surface | state at `7185e1a` |
|---|---|---|
| `ngsi_ld_http` | NGSI-LD HTTP binding under `/ngsi-ld/v1` | implemented, `crates/antares-api/src/lib.rs:315` `router` |
| `odata` | OData v4.01 under `/x/views/{view}/odata/v4` | proposal |
| `mcp` | MCP under `/x/views/{view}/mcp` | served by `antares-mcp` (T-3442, T-3514): one tool per row, `crates/antares-mcp/src/tools.rs` `TOOLS`; the platform gateway's tools (`endpoint_facade.rs`) were the proposal's source (T-3656) |
| `linkml` | LinkML schema surface under `/x/views/{view}/linkml`, read-only | implemented (T-3511) |
| `websocket` | WebSocket notifications under `/x/views/{view}/ws` and `/x/ws/ngsi-ld/v1` | implemented (ADR-0026, T-3517); no binding exists in the specs (section 2.4) |
| `internal_csr` | in-process Context Source registrations between tenants | proposal; the forward is `AppState::call` (`crates/antares-api/src/state.rs:571`) instead of HTTP |

Every surface beside the vanilla NGSI-LD root hangs off a view, `/x/views/{view}/...`, as ADR-0023 rule 10 decides (the view's own NGSI-LD root is `/x/views/{view}/ngsi-ld/v1`). The prefixes follow `crates/antares-api/src/surface.rs:42-48` `check_prefix`: a surface mounts under `/q`, `/x` or below `/x`, "never under the NGSI-LD API root".

## 2. Method

### 2.1 Rows

1. The operation list is the TS 104 175 README (clauses 10 to 16), one row per operation clause, notification behaviours included (10.5.7, 12.4.7, 16.8). Query Entities and Query Temporal Evolution each have a second row for their POST form (`entityOperations/query`, `temporal/entityOperations/query`), because TS 104 176 binds them as separate resources (`7/7.12.3.1.md`, `8/8.7.3.1.md`) and the code gates them at separate sites. 69 rows.
2. Each row's HTTP method and path is the TS 104 176 resource-method clause that names that TS 104 175 clause (grep of `clause 1[0-6]\.` in every `*.3.N.md` file of chapters 7 to 13).
3. Each route of `router()` (`crates/antares-api/src/lib.rs:315-460`) was matched to its handler, and each handler to the `gate!` call it reaches, with the Cypher in section 2.2. The fields the handler fills into `policy::Operation` were read off the `gate!` call itself (`crates/antares-api/src/lib.rs:27-46` is the macro: the named members go into the `Operation`, the rest keep `Operation::new`'s empty default).
4. `filterable` is `gate_clause in policy::FILTERABLE` (`crates/antares-api/src/policy.rs:68`), `whole_tenant` is `gate_clause in policy::WHOLE_TENANT` (`crates/antares-api/src/policy.rs:59`).
5. Routes that are reachable but are not a TS 104 175 operation get a row of their own (section 7), so no route is missing.
6. OData features with no NGSI-LD operation behind them get a `rejected` row (section 6).

### 2.2 Cypher

Run from the Antares clone (`cg` graph built at `7185e1a`):

```cypher
// every handler that asks the policy engine, with the line of the gate (54 rows)
MATCH (c:Symbol)-[r:CALLS]->(g:Symbol {path:'antares_api::policy::gate'})
WHERE c.is_test = false RETURN c.path, c.file, r.line ORDER BY c.file, r.line;
// the route handlers' extents, to tie each route to its gate site
MATCH (f:File)-[:DEFINES]->(s:Symbol)
WHERE f.path STARTS WITH 'crates/antares-api/src/' AND s.kind IN ['Function','Method','StaticMethod']
  AND s.is_test = false AND s.name IN ['create_entity','query_entities', /* ... every handler named in router() */ 'source_identity','remote_notify']
RETURN s.path, f.path, s.line, s.end_line ORDER BY f.path, s.line;
// the one notification gate
MATCH (c:Symbol)-[r:CALLS]->(g:Symbol {path:'antares_api::policy::pre_notify'})
WHERE c.is_test = false RETURN c.path, c.file, r.line;
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol {path:'antares_api::notify::deliver_as'}) RETURN c.path, c.file, r.line;
// who reads FILTERABLE / WHOLE_TENANT
MATCH (c:Symbol)-[r:REFERENCES]->(g:Symbol)
WHERE g.path IN ['antares_api::policy::FILTERABLE','antares_api::policy::WHOLE_TENANT'] AND c.is_test = false
RETURN g.name, c.path, c.file, r.line;
// which handlers forward to Context Sources (the internal_csr column)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path STARTS WITH 'antares_api::federation::' AND NOT c.path STARTS WITH 'antares_api::federation::' AND c.is_test = false
RETURN c.path, count(DISTINCT f.name) AS n ORDER BY c.path;
// who walks a join (the linked-entity check on reads)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path IN ['antares_api::repr::inline_join','antares_api::repr::inline_join_beyond','antares_api::repr::lookup_joined']
RETURN f.name, c.path, r.line;
// existing ApiSurface implementations
MATCH (t:Symbol)-[:IMPLEMENTS]->(i:Symbol {name:'ApiSurface'}) RETURN t.path, t.file, t.line;
```

Run from the platform clone (`cg` graph built at `59a8f70`):

```cypher
MATCH (s:Symbol {path:'context_gateway::mcp::endpoint_facade::TOOLS'}) RETURN s.file, s.line, s.end_line;
GREP name: "[a-z_]+",\s*$ IN crates/context-gateway/src/mcp/endpoint_facade.rs;
```

The tool names and their `operations:` lists were then read in `endpoint_facade.rs:108-426`.

### 2.3 OData

Option names and section numbers were checked against OData 4.01 Part 2 URL Conventions (system query options in 5.1: `$filter` 5.1.2, `$expand` 5.1.3, `$select` 5.1.4, `$orderby` 5.1.5, `$top`/`$skip` 5.1.6, `$count` 5.1.7, `$search` 5.1.8, `$format` 5.1.9, `$compute` 5.1.10, `$index` 5.1.11, `$schemaversion` 5.1.12; `$ref` 4.4, actions 4.5.1, functions 4.5.2, property 4.6, property value 4.7, `/$count` 4.8, derived types 4.11, `$each` 4.13, media stream 4.14, `$crossjoin` 4.15, `$all` 4.16) and Part 1 Protocol (create 11.4.2, deep insert 11.4.2.2, update 11.4.3, upsert 11.4.4, delete 11.4.5, references 11.4.6.1 to 11.4.6.3, media entities 11.4.7, stream properties 11.4.8, properties 11.4.9, delta update of a collection 11.4.12, delete members 11.4.14, functions 11.5.4.1, actions 11.5.5.1, batch 11.7). `$apply` is not in either document: it belongs to the OData Data Aggregation extension.

The status code for a refused feature is Protocol 9.3.1: "the service responds with 501 Not Implemented and SHOULD include a response body describing the functionality not implemented." The body is the JSON Format 21.1 error object: "The OData error object MUST contain name/value pairs with the names code and message". Protocol 6.1 states that a service MUST fail a request carrying an OData query option of its version it does not support, so a refused option is never silently ignored.

Two OData rules decide a design here:

- Deep insert, Protocol 11.4.2.2: "On success, the service MUST create all entities and relate them. On failure, the service MUST NOT create any of the entities." NGSI-LD has no transactional behaviour (`cim009-to-ts104175.md` section 3, row 5.5.14), so the OData surface has to decide every Create Entity gate of a deep insert before the first write, and still cannot roll back a store failure half way. Recommendation in section 8.
- Change sets, Protocol 11.7.2: members of an atomicity group "MUST either all succeed, or all fail". The same reason makes atomicity groups `rejected` (501), while a `$batch` without one is a list of independent single-Entity operations.

### 2.4 WebSocket

`grep -rli websocket /workspace/etsi-specs` finds one hit: `ts-104178-ngsi-ld-information-model/4/4.1.md:45`, a passing mention of transports ("using HTTP, WebSockets, etc."). TS 104 175, TS 104 176 and TS 104 243 define no WebSocket binding. `docs/adr/ADR-0003-websocket-deferred.md` keeps it out of v1 and scopes it to a future `antares-ws` crate behind feature `ws`, plugged into the notification sink registry. ADR-0026 (T-3517) built it: the `websocket` column's binding is the `x-op` of the TS 104 243 envelope `antares-ws` reads (`crates/antares-ws/src/envelope.rs`, `OpName`): `x-subscribe`, `x-update`, `x-unsubscribe`, plus the server push of a notification. `x-subscribe` and `x-update` answer with the stored Subscription, so each also asks Retrieve Subscription (5.8.3). The engine decides every one of these on the socket's own Subscription as Query Entities (5.7.2), not as its row's 5.8.x clause (ADR-0026 rule 4, T-4755). `x-resume` (ADR-0026 rule 7) is no operation and asks no engine; the yaml lists it under `surfaces.websocket.session_messages`. The column offers subscriptions and their notifications only.

### 2.5 What a cell means

A surface is one more client of the operation layer. A `supported` cell means: the surface translates its request into exactly that TS 104 175 operation and runs it through the same handler, so the same `gate!` call fires with the same `Operation` members. The in-process handle already works this way: `crates/antares-api/tests/facade_seam_f0.rs` (module doc) states "there is NO second data path: the inner request takes the same route as one off the socket", and `AppState::call` (`crates/antares-api/src/state.rs:556-557`) copies "every header `ANTARES_POLICY_SUBJECT_HEADERS` names, so the policy engine is asked about the caller rather than about the façade". The `checks.operation_fields` of a non-HTTP cell are therefore the NGSI-LD handler's, not a wish list; what a handler should fill and does not is listed under `gaps` (section 5).

## 3. The yaml schema

Top level:

| key | type | meaning |
|---|---|---|
| `schema_version` | int | 1 |
| `generated_for` | string | `T-3437` |
| `sources` | map | commit or document version of each source |
| `surfaces` | map surface -> `{prefix, status}` | the six columns |
| `operations` | list of operation rows | one per TS 104 175 operation (69) |
| `odata_without_ngsi_ld_operation` | list | OData features with no operation behind them (16) |
| `non_operation_routes` | list | reachable routes and tools that are not an operation (9) |

An operation row:

| key | type | meaning |
|---|---|---|
| `id` | snake_case string, stable | the key tests use; never reused |
| `ts104175` | string | TS 104 175 V1.1.1 clause |
| `cim009` | string or null | GS CIM 009 V1.9.1 clause, null for 10.2.6 and 10.4.4 |
| `title` | string | the TS 104 175 title |
| `kind` | `read` \| `write` \| `subscription` \| `registration` \| `context` \| `entitymap` \| `snapshot` \| `identity` | |
| `gate_clause` | string or null | the clause string the handler passes to `gate!`; null when no on_request gate exists |
| `write` | bool | the operation changes broker state |
| `linked_entity_checks` | list of strings | what must hold for every Entity the operation reaches besides the one it names |
| `pre_image`, `post_image` | string, write rows only | what must hold before and after the write; `[not enforced today ...]` marks a check the seam cannot make yet |
| `note`, `gaps` | string, optional | facts about the code; `gaps` lists `Operation` members the handler leaves empty |
| `surfaces` | map column -> cell | the six cells |

A cell:

| key | type | meaning |
|---|---|---|
| `status` | `supported` \| `not_offered` \| `rejected` | `not_offered`: the surface has no binding, the request falls through to the surface's 404/405; `rejected`: a binding exists in that protocol and is refused with a stated code |
| `binding` | string or null | HTTP method + path (NGSI-LD, OData), tool name (MCP), envelope op (WebSocket), forwarded op (internal CSR) |
| `binding_clause`, `gate_site` | string, `ngsi_ld_http` only | TS 104 176 clause file; `file:line` of the `gate!` call |
| `checks.adr0020_phase` | `on_request` \| `pre_notify` \| `none` | |
| `checks.operation_fields` | list of `clause`, `ids`, `types`, `attrs`, `q`, `scope_q`, `geo`, `body` | the `policy::Operation` members filled (`crates/antares-api/src/policy.rs:97-110`) |
| `checks.filterable` | bool | `gate_clause` is in `policy::FILTERABLE` |
| `checks.whole_tenant` | bool | `gate_clause` is in `policy::WHOLE_TENANT` |
| `checks.linked_entity_checks` | list of strings | the row's list, plus the surface's own (deep insert, `$ref`, `$expand`) |
| `pre_image`, `post_image` | string | repeated on every non-`not_offered` cell of a write row |
| `http_status`, `odata_error` | int, `{code, message}` | `rejected` OData rows only |

The yaml is generated from one table in a script so the 69 x 6 cells stay consistent; the script is not part of the repository, the yaml is the source.

## 4. NGSI-LD HTTP column (the reference every other column maps onto)

Paths are relative to `/ngsi-ld/v1`; HEAD is served by every GET route (axum `get()`), and is the same operation.

| id | TS 104 175 | CIM 009 | kind | HTTP method + path (TS 104 176 clause file) | gate clause | `Operation` fields filled | phase | FILTERABLE | WHOLE_TENANT | gate site |
|---|---|---|---|---|---|---|---|---|---|---|
| `create_entity` | 10.2.2 | 5.6.1 | write | `POST /ngsi-ld/v1/entities` (ts-104176-ngsi-ld-http-binding/7/7.2.3.2.md) | 5.6.1 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/entities.rs:114 |
| `update_attributes` | 10.2.3 | 5.6.2 | write | `PATCH /ngsi-ld/v1/entities/{entityId}/attrs` (ts-104176-ngsi-ld-http-binding/7/7.4.3.2.md) | 5.6.2 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/attrs.rs:150 |
| `append_attributes` | 10.2.4 | 5.6.3 | write | `POST /ngsi-ld/v1/entities/{entityId}/attrs` (ts-104176-ngsi-ld-http-binding/7/7.4.3.1.md) | 5.6.3 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/attrs.rs:150 |
| `partial_attribute_update` | 10.2.5 | 5.6.4 | write | `PATCH /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}` (ts-104176-ngsi-ld-http-binding/7/7.5.3.2.md) | 5.6.4 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/attrs.rs:803 |
| `set_attribute_value` | 10.2.6 | null | write | `PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}/value` (ts-104176-ngsi-ld-http-binding/7/7.6.3.2.md) | 5.6.19 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/attrs.rs:1149 |
| `delete_attribute` | 10.2.7 | 5.6.5 | write | `DELETE /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}` (ts-104176-ngsi-ld-http-binding/7/7.5.3.4.md) | 5.6.5 | clause, ids, types, attrs | on_request | no | no | crates/antares-api/src/attrs.rs:1361 |
| `delete_entity` | 10.2.8 | 5.6.6 | write | `DELETE /ngsi-ld/v1/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/7/7.3.3.4.md) | 5.6.6 | clause, ids, types | on_request | no | no | crates/antares-api/src/entities.rs:1395 |
| `merge_entity` | 10.2.9 | 5.6.17 | write | `PATCH /ngsi-ld/v1/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/7/7.3.3.2.md) | 5.6.17 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/entities.rs:1859 |
| `replace_entity` | 10.2.10 | 5.6.18 | write | `PUT /ngsi-ld/v1/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/7/7.3.3.3.md) | 5.6.18 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/entities.rs:2233 |
| `replace_attribute` | 10.2.11 | 5.6.19 | write | `PUT /ngsi-ld/v1/entities/{entityId}/attrs/{attrId}` (ts-104176-ngsi-ld-http-binding/7/7.5.3.3.md) | 5.6.19 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/attrs.rs:975 |
| `purge_entities` | 10.2.12 | 5.6.21 | write | `DELETE /ngsi-ld/v1/entities` (ts-104176-ngsi-ld-http-binding/7/7.2.3.3.md) | 5.6.21 | clause, scope_q | on_request | no | yes | crates/antares-api/src/entities.rs:1579 |
| `batch_create` | 10.3.2 | 5.6.7 | write | `POST /ngsi-ld/v1/entityOperations/create` (ts-104176-ngsi-ld-http-binding/7/7.7.3.1.md) | 5.6.7 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/batch.rs:747 |
| `batch_upsert` | 10.3.3 | 5.6.8 | write | `POST /ngsi-ld/v1/entityOperations/upsert` (ts-104176-ngsi-ld-http-binding/7/7.8.3.1.md) | 5.6.8 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/batch.rs:747 |
| `batch_update` | 10.3.4 | 5.6.9 | write | `POST /ngsi-ld/v1/entityOperations/update` (ts-104176-ngsi-ld-http-binding/7/7.9.3.1.md) | 5.6.9 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/batch.rs:747 |
| `batch_merge` | 10.3.5 | 5.6.20 | write | `POST /ngsi-ld/v1/entityOperations/merge` (ts-104176-ngsi-ld-http-binding/7/7.10.3.1.md) | 5.6.20 | clause, ids, types, attrs, body | on_request | no | no | crates/antares-api/src/batch.rs:747 |
| `batch_delete` | 10.3.6 | 5.6.10 | write | `POST /ngsi-ld/v1/entityOperations/delete` (ts-104176-ngsi-ld-http-binding/7/7.11.3.1.md) | 5.6.10 | clause, ids, types | on_request | no | no | crates/antares-api/src/batch.rs:1252 |
| `retrieve_entity` | 10.4.2 | 5.7.1 | read | `GET /ngsi-ld/v1/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/7/7.3.3.1.md) | 5.7.1 | clause, ids, types, attrs | on_request | yes | no | crates/antares-api/src/entities.rs:271 |
| `query_entities` | 10.4.3 | 5.7.2 | read | `GET /ngsi-ld/v1/entities` (ts-104176-ngsi-ld-http-binding/7/7.2.3.1.md) | 5.7.2 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/entities.rs:565 |
| `query_entities_post` | 10.4.3 | 5.7.2 | read | `POST /ngsi-ld/v1/entityOperations/query` (ts-104176-ngsi-ld-http-binding/7/7.12.3.1.md) | 5.7.2 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/batch.rs:1481 |
| `retrieve_attribute_value` | 10.4.4 | null | read | `GET /ngsi-ld/v1/entities/{entityId}/attrs/{attrId} and .../attrs/{attrId}/value` (ts-104176-ngsi-ld-http-binding/7/7.5.3.1.md, 7/7.6.3.1.md) | 5.7.1 | clause, ids, attrs | on_request | yes | no | crates/antares-api/src/entities.rs:2441 |
| `create_subscription` | 10.5.2 | 5.8.1 | subscription | `POST /ngsi-ld/v1/subscriptions` (ts-104176-ngsi-ld-http-binding/7/7.13.3.2.md) | 5.8.1 | clause, ids, types, attrs, q, scope_q, geo | on_request | no | no | crates/antares-api/src/subscriptions.rs:820 |
| `update_subscription` | 10.5.3 | 5.8.2 | subscription | `PATCH /ngsi-ld/v1/subscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/7/7.14.3.2.md) | 5.8.2 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:962 |
| `retrieve_subscription` | 10.5.4 | 5.8.3 | read | `GET /ngsi-ld/v1/subscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/7/7.14.3.1.md) | 5.8.3 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:892 |
| `query_subscriptions` | 10.5.5 | 5.8.4 | read | `GET /ngsi-ld/v1/subscriptions` (ts-104176-ngsi-ld-http-binding/7/7.13.3.1.md) | 5.8.4 | clause | on_request | yes | no | crates/antares-api/src/subscriptions.rs:1028 |
| `delete_subscription` | 10.5.6 | 5.8.5 | subscription | `DELETE /ngsi-ld/v1/subscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/7/7.14.3.3.md) | 5.8.5 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:1022 |
| `notification` | 10.5.7 | 5.8.6 | subscription | `POST {notification.endpoint.uri} (broker-initiated)` (ts-104176-ngsi-ld-http-binding/6/6.5.2.md) | none | - | pre_notify | no | no | crates/antares-api/src/notify.rs:2084 |
| `retrieve_entity_types` | 10.6.3 | 5.7.5 | read | `GET /ngsi-ld/v1/types` (ts-104176-ngsi-ld-http-binding/7/7.15.3.1.md) | 5.7.5 | clause | on_request | yes | no | crates/antares-api/src/types_attrs.rs:264 |
| `retrieve_entity_type_details` | 10.6.4 | 5.7.6 | read | `GET /ngsi-ld/v1/types?details=true` (ts-104176-ngsi-ld-http-binding/7/7.15.3.1.md) | 5.7.5 | clause | on_request | yes | no | crates/antares-api/src/types_attrs.rs:264 |
| `retrieve_entity_type_info` | 10.6.5 | 5.7.7 | read | `GET /ngsi-ld/v1/types/{type}` (ts-104176-ngsi-ld-http-binding/7/7.16.3.1.md) | 5.7.7 | clause, types | on_request | yes | no | crates/antares-api/src/types_attrs.rs:324 |
| `retrieve_attributes` | 10.6.6 | 5.7.8 | read | `GET /ngsi-ld/v1/attributes` (ts-104176-ngsi-ld-http-binding/7/7.17.3.1.md) | 5.7.8 | clause | on_request | yes | no | crates/antares-api/src/types_attrs.rs:389 |
| `retrieve_attribute_details` | 10.6.7 | 5.7.9 | read | `GET /ngsi-ld/v1/attributes?details=true` (ts-104176-ngsi-ld-http-binding/7/7.17.3.1.md) | 5.7.8 | clause | on_request | yes | no | crates/antares-api/src/types_attrs.rs:389 |
| `retrieve_attribute_info` | 10.6.8 | 5.7.10 | read | `GET /ngsi-ld/v1/attributes/{attrId}` (ts-104176-ngsi-ld-http-binding/7/7.18.3.1.md) | 5.7.10 | clause, attrs | on_request | yes | no | crates/antares-api/src/types_attrs.rs:453 |
| `temporal_upsert` | 11.2.2 | 5.6.11 | write | `POST /ngsi-ld/v1/temporal/entities` (ts-104176-ngsi-ld-http-binding/8/8.2.3.2.md) | 5.6.11 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:79 |
| `temporal_add_attributes` | 11.2.3 | 5.6.12 | write | `POST /ngsi-ld/v1/temporal/entities/{entityId}/attrs` (ts-104176-ngsi-ld-http-binding/8/8.4.3.1.md) | 5.6.12 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:2502 |
| `temporal_delete_attribute` | 11.2.4 | 5.6.13 | write | `DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}` (ts-104176-ngsi-ld-http-binding/8/8.5.3.1.md) | 5.6.13 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:2671 |
| `temporal_modify_instance` | 11.2.5 | 5.6.14 | write | `PATCH /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}` (ts-104176-ngsi-ld-http-binding/8/8.6.3.1.md) | 5.6.14 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:2887 |
| `temporal_delete_instance` | 11.2.6 | 5.6.15 | write | `DELETE /ngsi-ld/v1/temporal/entities/{entityId}/attrs/{attrId}/{instanceId}` (ts-104176-ngsi-ld-http-binding/8/8.6.3.2.md) | 5.6.15 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:2997 |
| `temporal_delete` | 11.2.7 | 5.6.16 | write | `DELETE /ngsi-ld/v1/temporal/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/8/8.3.3.2.md) | 5.6.16 | clause, ids | on_request | no | no | crates/antares-api/src/temporal.rs:2456 |
| `temporal_retrieve` | 11.3.2 | 5.7.3 | read | `GET /ngsi-ld/v1/temporal/entities/{entityId}` (ts-104176-ngsi-ld-http-binding/8/8.3.3.1.md) | 5.7.3 | clause, ids | on_request | yes | no | crates/antares-api/src/temporal.rs:2211 |
| `temporal_query` | 11.3.3 | 5.7.4 | read | `GET /ngsi-ld/v1/temporal/entities` (ts-104176-ngsi-ld-http-binding/8/8.2.3.1.md) | 5.7.4 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/temporal.rs:1393 |
| `temporal_query_post` | 11.3.3 | 5.7.4 | read | `POST /ngsi-ld/v1/temporal/entityOperations/query` (ts-104176-ngsi-ld-http-binding/8/8.7.3.1.md) | 5.7.4 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/temporal.rs:3092 |
| `register_context_source` | 12.2.2 | 5.9.2 | registration | `POST /ngsi-ld/v1/csourceRegistrations` (ts-104176-ngsi-ld-http-binding/9/9.2.3.2.md) | 5.9.2 | clause, ids, types, attrs | on_request | no | no | crates/antares-api/src/csource.rs:1017 |
| `update_registration` | 12.2.3 | 5.9.3 | registration | `PATCH /ngsi-ld/v1/csourceRegistrations/{registrationId}` (ts-104176-ngsi-ld-http-binding/9/9.3.3.2.md) | 5.9.3 | clause, ids | on_request | no | no | crates/antares-api/src/csource.rs:1327 |
| `delete_registration` | 12.2.4 | 5.9.4 | registration | `DELETE /ngsi-ld/v1/csourceRegistrations/{registrationId}` (ts-104176-ngsi-ld-http-binding/9/9.3.3.3.md) | 5.9.4 | clause, ids | on_request | no | no | crates/antares-api/src/csource.rs:1397 |
| `retrieve_registration` | 12.3.2 | 5.10.1 | registration | `GET /ngsi-ld/v1/csourceRegistrations/{registrationId}` (ts-104176-ngsi-ld-http-binding/9/9.3.3.1.md) | 5.10.1 | clause, ids | on_request | no | no | crates/antares-api/src/csource.rs:1022 |
| `query_registrations` | 12.3.3 | 5.10.2 | registration | `GET /ngsi-ld/v1/csourceRegistrations` (ts-104176-ngsi-ld-http-binding/9/9.2.3.1.md) | 5.10.2 | clause, scope_q | on_request | yes | no | crates/antares-api/src/csource.rs:1135 |
| `create_csource_subscription` | 12.4.2 | 5.11.2 | subscription | `POST /ngsi-ld/v1/csourceSubscriptions` (ts-104176-ngsi-ld-http-binding/9/9.4.3.1.md (headed GET/HEAD, see open questions)) | 5.11.2 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:776 |
| `update_csource_subscription` | 12.4.3 | 5.11.3 | subscription | `PATCH /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/9/9.5.3.2.md) | 5.11.3 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:962 |
| `retrieve_csource_subscription` | 12.4.4 | 5.11.4 | subscription | `GET /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/9/9.5.3.1.md) | 5.11.4 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:892 |
| `query_csource_subscriptions` | 12.4.5 | 5.11.5 | subscription | `GET /ngsi-ld/v1/csourceSubscriptions` (ts-104176-ngsi-ld-http-binding/9/9.4.3.2.md (headed POST, see open questions)) | 5.11.5 | clause | on_request | yes | no | crates/antares-api/src/subscriptions.rs:1028 |
| `delete_csource_subscription` | 12.4.6 | 5.11.6 | subscription | `DELETE /ngsi-ld/v1/csourceSubscriptions/{subscriptionId}` (ts-104176-ngsi-ld-http-binding/9/9.5.3.3.md) | 5.11.6 | clause, ids | on_request | no | no | crates/antares-api/src/subscriptions.rs:1022 |
| `csource_notification` | 12.4.7 | 5.11.7 | subscription | `POST {notification.endpoint.uri} (broker-initiated)` (ts-104176-ngsi-ld-http-binding/6/6.5.3.md) | none | - | pre_notify | no | no | crates/antares-api/src/notify.rs:2084 |
| `add_context` | 13.2 | 5.13.2 | context | `POST /ngsi-ld/v1/jsonldContexts` (ts-104176-ngsi-ld-http-binding/10/10.2.3.2.md) | 5.13.2 | clause | on_request | no | no | crates/antares-api/src/contexts.rs:237 |
| `list_contexts` | 13.3 | 5.13.3 | context | `GET /ngsi-ld/v1/jsonldContexts` (ts-104176-ngsi-ld-http-binding/10/10.2.3.1.md) | 5.13.3 | clause | on_request | no | no | crates/antares-api/src/contexts.rs:299 |
| `serve_context` | 13.4 | 5.13.4 | context | `GET /ngsi-ld/v1/jsonldContexts/{contextId}` (ts-104176-ngsi-ld-http-binding/10/10.3.3.1.md) | 5.13.4 | clause, ids | on_request | no | no | crates/antares-api/src/contexts.rs:390 |
| `delete_context` | 13.5 | 5.13.5 | context | `DELETE /ngsi-ld/v1/jsonldContexts/{contextId}` (ts-104176-ngsi-ld-http-binding/10/10.3.3.2.md) | 5.13.5 | clause, ids | on_request | no | no | crates/antares-api/src/contexts.rs:478 |
| `retrieve_entity_map` | 14.2 | 5.14.1 | entitymap | `GET /ngsi-ld/v1/entityMaps/{entityMapId}` (ts-104176-ngsi-ld-http-binding/11/11.4.3.1.md) | 5.14.1 | clause, ids | on_request | no | no | crates/antares-api/src/entity_maps.rs:50 |
| `update_entity_map` | 14.3 | 5.14.2 | entitymap | `PATCH /ngsi-ld/v1/entityMaps/{entityMapId}` (ts-104176-ngsi-ld-http-binding/11/11.4.3.2.md) | 5.14.2 | clause, ids | on_request | no | no | crates/antares-api/src/entity_maps.rs:73 |
| `delete_entity_map` | 14.4 | 5.14.3 | entitymap | `DELETE /ngsi-ld/v1/entityMaps/{entityMapId}` (ts-104176-ngsi-ld-http-binding/11/11.4.3.3.md) | 5.14.3 | clause, ids | on_request | no | no | crates/antares-api/src/entity_maps.rs:104 |
| `create_entity_map` | 14.5 | 5.14.4 | entitymap | `GET\|POST /ngsi-ld/v1/entityMaps` (ts-104176-ngsi-ld-http-binding/11/11.2.3.1.md, 11/11.2.3.2.md) | 5.14.4 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/entity_maps.rs:167 |
| `create_temporal_entity_map` | 14.6 | 5.14.5 | entitymap | `GET\|POST /ngsi-ld/v1/temporal/entityMaps` (ts-104176-ngsi-ld-http-binding/11/11.3.3.1.md, 11/11.3.3.2.md) | 5.14.5 | clause, types, attrs, q, scope_q, geo | on_request | yes | no | crates/antares-api/src/entity_maps.rs:167 |
| `source_identity` | 15.2 | 5.15.1 | identity | `GET /ngsi-ld/v1/info/sourceIdentity` (ts-104176-ngsi-ld-http-binding/12/12.2.3.1.md) | 5.15.1 | clause | on_request | no | no | crates/antares-api/src/lib.rs:1300 |
| `create_snapshot` | 16.2 | 5.16.1 | snapshot | `POST /ngsi-ld/v1/snapshots` (ts-104176-ngsi-ld-http-binding/13/13.2.3.1.md) | 5.16.1 | clause | on_request | no | yes | crates/antares-api/src/snapshots.rs:544 |
| `clone_snapshot` | 16.3 | 5.16.2 | snapshot | `POST /ngsi-ld/v1/snapshots/{snapshotId}/clone` (ts-104176-ngsi-ld-http-binding/13/13.4.3.1.md) | 5.16.2 | clause, ids | on_request | no | yes | crates/antares-api/src/snapshots.rs:1127 |
| `retrieve_snapshot_status` | 16.4 | 5.16.3 | snapshot | `GET /ngsi-ld/v1/snapshots/{snapshotId}` (ts-104176-ngsi-ld-http-binding/13/13.3.3.1.md) | 5.16.3 | clause, ids | on_request | no | no | crates/antares-api/src/snapshots.rs:1020 |
| `update_snapshot_status` | 16.5 | 5.16.4 | snapshot | `PATCH /ngsi-ld/v1/snapshots/{snapshotId}` (ts-104176-ngsi-ld-http-binding/13/13.3.3.2.md) | 5.16.4 | clause, ids | on_request | no | no | crates/antares-api/src/snapshots.rs:1046 |
| `delete_snapshot` | 16.6 | 5.16.5 | snapshot | `DELETE /ngsi-ld/v1/snapshots/{snapshotId}` (ts-104176-ngsi-ld-http-binding/13/13.3.3.3.md) | 5.16.5 | clause, ids | on_request | no | no | crates/antares-api/src/snapshots.rs:1106 |
| `purge_snapshots` | 16.7 | 5.16.7 | snapshot | `DELETE /ngsi-ld/v1/snapshots` (ts-104176-ngsi-ld-http-binding/13/13.2.3.2.md) | 5.16.7 | clause | on_request | no | yes | crates/antares-api/src/snapshots.rs:940 |
| `snapshot_notification` | 16.8 | 5.16.6 | snapshot | `POST {snapshot notification endpoint} (broker-initiated)` (none: TS 104 176 has no resource for it; snapshots.rs:883) | none | - | none | no | no | crates/antares-api/src/snapshots.rs:883 |

Notes on this table:

- `retrieve_attribute_value` is gated as `"5.7.1"` (Retrieve Entity) at `crates/antares-api/src/entities.rs:2314`, so it is FILTERABLE. Both `GET .../attrs/{attrId}` and `GET .../attrs/{attrId}/value` reach it; TS 104 176 binds both to 10.4.4 (`7/7.5.3.1.md`, `7/7.6.3.1.md`).
- `set_attribute_value` (10.2.6, `PUT .../attrs/{attrId}/value`, `7/7.6.3.2.md`) is served since T-3450 and gated as `"5.6.19"` (Replace Attribute), the write it is: the engine sees the stored instance with the value replaced as the body, so a rule written for Replace Attribute holds here too. The other surfaces stay `not_offered`.
- 10.6.4 and 10.6.7 (the `details=true` forms) are gated with the clause of the plain list, `"5.7.5"` (`crates/antares-api/src/types_attrs.rs:220`) and `"5.7.8"` (`crates/antares-api/src/types_attrs.rs:333`); `"5.7.6"` and `"5.7.9"` are never passed to the seam.
- The three notification rows have no on_request gate. 10.5.7 and 12.4.7 pass `pre_notify` at `crates/antares-api/src/notify.rs:2084` (`deliver_as`, reached from `deliver` and `deliver_csource`). 16.8 does not: `deliver_as` is the only non-test caller of `policy::pre_notify` and no function in `snapshots.rs` calls it; the snapshot notification (`crates/antares-api/src/snapshots.rs:883`) carries status, not Entity data.
- TS 104 176 itself swaps two headings: `9/9.4.3.1.md` is headed "GET/HEAD" but reads "This method is bound to the operation \"Create Context Source Registration Subscription\" ... clause 12.4.2", and `9/9.4.3.2.md` is headed "POST" but binds "Query Context Source Registration Subscriptions ... clause 12.4.5". The matrix uses POST for create and GET for query, as `crates/antares-api/src/lib.rs:397-401` does and as every other collection resource of the binding does (open question 8.9).

## 5. The other five columns

The MCP column names the tool `antares-mcp` serves for the row (`crates/antares-mcp/src/tools.rs`, `TOOLS`; T-3656), and `crates/antares-broker/tests/operation_matrix.rs` holds the two to each other. The WebSocket column names the envelope `x-op` (T-3696); the same test holds every `x-op` `antares-ws` parses to a row or to `session_messages`, and each to its row's gate clause. Every LinkML path is a proposal.

| id | TS 104 175 | OData v4 | MCP | LinkML | WebSocket | internal CSR |
|---|---|---|---|---|---|---|
| `create_entity` | 10.2.2 | supported: POST /x/views/{view}/odata/v4/{EntitySet} (Protocol 11.4.2); deep insert 11.4.2.2 | supported: create_entity | not offered | not offered | supported: Create Entity forwarded to the registered tenant via AppState::call |
| `update_attributes` | 10.2.3 | supported: POST\|PUT\|DELETE /x/views/{view}/odata/v4/{EntitySet}('{id}')/{nav}/$ref (Protocol 11.4.6.1-11.4.6.3) | supported: update_attributes | not offered | not offered | supported: Update Attributes forwarded |
| `append_attributes` | 10.2.4 | not offered | supported: append_attributes | not offered | not offered | supported: Append Attributes forwarded |
| `partial_attribute_update` | 10.2.5 | supported: PATCH /x/views/{view}/odata/v4/{EntitySet}('{id}')/{complexProperty} (Protocol 11.4.9) | supported: update_attribute | not offered | not offered | supported: Partial Attribute update forwarded |
| `set_attribute_value` | 10.2.6 | supported: PUT …/attrs/{attrId}/value | not offered | not offered | not offered | supported: forwarded to replaceAttrs sources |
| `delete_attribute` | 10.2.7 | supported: DELETE /x/views/{view}/odata/v4/{EntitySet}('{id}')/{property} (Protocol 11.4.9; OData sets it to null, the mapping to Delete Attribute is a proposal) | supported: delete_attribute | not offered | not offered | supported: Delete Attribute forwarded |
| `delete_entity` | 10.2.8 | supported: DELETE /x/views/{view}/odata/v4/{EntitySet}('{id}') (Protocol 11.4.5) | supported: delete_entity | not offered | not offered | supported: Delete Entity forwarded |
| `merge_entity` | 10.2.9 | supported: PATCH /x/views/{view}/odata/v4/{EntitySet}('{id}') (Protocol 11.4.3); upsert on a missing key (11.4.4) is refused, not a create | supported: merge_entity | not offered | not offered | supported: Merge Entity forwarded |
| `replace_entity` | 10.2.10 | supported: PUT /x/views/{view}/odata/v4/{EntitySet}('{id}') (Protocol 11.4.3) | supported: replace_entity | not offered | not offered | supported: Replace Entity forwarded |
| `replace_attribute` | 10.2.11 | supported: PUT /x/views/{view}/odata/v4/{EntitySet}('{id}')/{property} (Protocol 11.4.9) | supported: replace_attribute | not offered | not offered | supported: Replace Attribute forwarded |
| `purge_entities` | 10.2.12 | rejected: DELETE /x/views/{view}/odata/v4/{EntitySet}/$each (Protocol 11.4.14): 501 | rejected: no tool; a whole-tenant delete is not offered to an agent | not offered | not offered | supported: Purge Entities forwarded (entities.rs purge_inner calls federation) |
| `batch_create` | 10.3.2 | supported: POST /x/views/{view}/odata/v4/$batch (Protocol 11.7) with one POST per member, each mapped to create_entity | supported: batch_create | not offered | not offered | supported: Batch Entity creation forwarded per registration (batch.rs forward_batch) |
| `batch_upsert` | 10.3.3 | not offered | supported: batch_upsert | not offered | not offered | supported: Batch upsert forwarded |
| `batch_update` | 10.3.4 | not offered | supported: batch_update | not offered | not offered | supported: Batch update forwarded |
| `batch_merge` | 10.3.5 | rejected: PATCH /x/views/{view}/odata/v4/{EntitySet} with a delta payload (Protocol 11.4.12): 501 | supported: batch_merge | not offered | not offered | supported: Batch merge forwarded |
| `batch_delete` | 10.3.6 | supported: POST /x/views/{view}/odata/v4/$batch with one DELETE per member (Protocol 11.7) | supported: batch_delete | not offered | not offered | supported: Batch delete forwarded |
| `retrieve_entity` | 10.4.2 | supported: GET /x/views/{view}/odata/v4/{EntitySet}('{id}') with $select (URL 5.1.4) and $expand (URL 5.1.3) | supported: get_entity | not offered | not offered | supported: Retrieve Entity forwarded and merged (federation fed_retrieve) |
| `query_entities` | 10.4.3 | supported: GET /x/views/{view}/odata/v4/{EntitySet} with $filter (5.1.2), $select (5.1.4), $orderby (5.1.5), $top/$skip (5.1.6), $count (5.1.7), $expand (5.1.3); GET /{EntitySet}/$count (URL 4.8) | supported: query_entities | not offered | not offered | supported: Query Entities forwarded with the narrowed q/scopeQ (fed_query) |
| `query_entities_post` | 10.4.3 | not offered | supported: batch_query | not offered | not offered | supported: forwarded as Query Entities (batch_query_inner calls federation) |
| `retrieve_attribute_value` | 10.4.4 | supported: GET /x/views/{view}/odata/v4/{EntitySet}('{id}')/{property} (URL 4.6) and .../$value (URL 4.7) | supported: get_attribute_value | not offered | not offered | supported: forwarded as Retrieve Entity with attrs |
| `create_subscription` | 10.5.2 | not offered | supported: create_subscription | not offered | supported: `x-subscribe`, the socket as endpoint; answers after Retrieve Subscription (5.8.3) | supported: forwarded subscription copy, subject stripped (distsub forward_sub) |
| `update_subscription` | 10.5.3 | not offered | supported: update_subscription | not offered | supported: `x-update` of a Subscription this connection made; answers after Retrieve Subscription (5.8.3) | supported: forwarded copy updated (distsub on_subscription_updated) |
| `retrieve_subscription` | 10.5.4 | not offered | supported: get_subscription | not offered | not offered | not offered |
| `query_subscriptions` | 10.5.5 | not offered | supported: list_subscriptions | not offered | not offered | not offered |
| `delete_subscription` | 10.5.6 | not offered | supported: delete_subscription | not offered | supported: `x-unsubscribe` of a Subscription this connection made | supported: forwarded copy deleted (distsub on_subscription_deleted) |
| `notification` | 10.5.7 | not offered | not offered | not offered | supported: server push of the Notification document in the envelope, after pre_notify | supported: notification of a forwarded copy remapped to the original subscriber at /ex/v1/remote-notify |
| `retrieve_entity_types` | 10.6.3 | supported: GET /x/views/{view}/odata/v4/ (service document, entity sets = visible types) | supported: list_types | not offered (the index of 10.6.4 lists the classes) | not offered | not offered |
| `retrieve_entity_type_details` | 10.6.4 | supported: GET /x/views/{view}/odata/v4/$metadata (CSDL built from the visible types and their attributes) | supported: list_type_details | supported: GET /x/views/{view}/linkml/schema/index.json and .../schema/v{major}/{document} | not offered | not offered |
| `retrieve_entity_type_info` | 10.6.5 | not offered | supported: get_type | supported: GET /x/views/{view}/linkml/classes/{type} | not offered | not offered |
| `retrieve_attributes` | 10.6.6 | not offered | supported: list_attributes | not offered (the documents of 10.6.4 list the slots) | not offered | not offered |
| `retrieve_attribute_details` | 10.6.7 | not offered | supported: list_attribute_details | not offered (the documents of 10.6.4 list the slots with ranges) | not offered | not offered |
| `retrieve_attribute_info` | 10.6.8 | not offered | supported: get_attribute | supported: GET /x/views/{view}/linkml/slots/{attr} | not offered | not offered |
| `temporal_upsert` | 11.2.2 | not offered | not offered | not offered | not offered | supported: forwarded (temporal write_plan) |
| `temporal_add_attributes` | 11.2.3 | not offered | not offered | not offered | not offered | supported: forwarded |
| `temporal_delete_attribute` | 11.2.4 | not offered | not offered | not offered | not offered | supported: forwarded |
| `temporal_modify_instance` | 11.2.5 | not offered | not offered | not offered | not offered | supported: forwarded |
| `temporal_delete_instance` | 11.2.6 | not offered | not offered | not offered | not offered | supported: forwarded |
| `temporal_delete` | 11.2.7 | not offered | not offered | not offered | not offered | supported: forwarded |
| `temporal_retrieve` | 11.3.2 | not offered | supported: retrieve_temporal | not offered | not offered | supported: forwarded (fed_retrieve_temporal) |
| `temporal_query` | 11.3.3 | not offered | supported: query_temporal | not offered | not offered | supported: forwarded (fed_query_temporal) |
| `temporal_query_post` | 11.3.3 | not offered | supported: batch_query_temporal | not offered | not offered | supported: forwarded |
| `register_context_source` | 12.2.2 | not offered | not offered | not offered | not offered | supported: internal registration between tenants (proposal): target is a tenant of this process, forwards go through AppState::call |
| `update_registration` | 12.2.3 | not offered | not offered | not offered | not offered | supported: internal registration updated (proposal) |
| `delete_registration` | 12.2.4 | not offered | not offered | not offered | not offered | supported: internal registration removed (proposal) |
| `retrieve_registration` | 12.3.2 | not offered | not offered | not offered | not offered | not offered |
| `query_registrations` | 12.3.3 | not offered | not offered | not offered | not offered | not offered |
| `create_csource_subscription` | 12.4.2 | not offered | not offered | not offered | not offered | not offered |
| `update_csource_subscription` | 12.4.3 | not offered | not offered | not offered | not offered | not offered |
| `retrieve_csource_subscription` | 12.4.4 | not offered | not offered | not offered | not offered | not offered |
| `query_csource_subscriptions` | 12.4.5 | not offered | not offered | not offered | not offered | not offered |
| `delete_csource_subscription` | 12.4.6 | not offered | not offered | not offered | not offered | not offered |
| `csource_notification` | 12.4.7 | not offered | not offered | not offered | not offered | not offered |
| `add_context` | 13.2 | not offered | not offered | not offered | not offered | not offered |
| `list_contexts` | 13.3 | not offered | not offered | not offered | not offered | not offered |
| `serve_context` | 13.4 | not offered | not offered | not offered (the narrowed schema names its own prefixes) | not offered | not offered |
| `delete_context` | 13.5 | not offered | not offered | not offered | not offered | not offered |
| `retrieve_entity_map` | 14.2 | not offered | not offered | not offered | not offered | not offered |
| `update_entity_map` | 14.3 | not offered | not offered | not offered | not offered | not offered |
| `delete_entity_map` | 14.4 | not offered | not offered | not offered | not offered | not offered |
| `create_entity_map` | 14.5 | supported: server-driven paging of GET /{EntitySet} (@odata.nextLink) may use a map internally (proposal) | not offered | not offered | not offered | supported: forwarded (fed_entity_maps) |
| `create_temporal_entity_map` | 14.6 | not offered | not offered | not offered | not offered | supported: forwarded |
| `source_identity` | 15.2 | not offered | not offered | not offered | not offered | supported: answered for the internal source's alias (federation alias_for) |
| `create_snapshot` | 16.2 | not offered | not offered | not offered | not offered | supported: fill federates (snapshots run_query -> fed_query) |
| `clone_snapshot` | 16.3 | not offered | not offered | not offered | not offered | not offered |
| `retrieve_snapshot_status` | 16.4 | not offered | not offered | not offered | not offered | not offered |
| `update_snapshot_status` | 16.5 | not offered | not offered | not offered | not offered | not offered |
| `delete_snapshot` | 16.6 | not offered | not offered | not offered | not offered | not offered |
| `purge_snapshots` | 16.7 | not offered | not offered | not offered | not offered | not offered |
| `snapshot_notification` | 16.8 | not offered | not offered | not offered | not offered | not offered |

Notes on this table:

- OData: the entity set is one NGSI-LD Entity type (`/{EntitySet}`), the key is the Entity id, a navigation property is a Relationship. PATCH maps to Merge Entity (10.2.9) and PUT to Replace Entity (10.2.10), because Protocol 11.4.3 makes PATCH a merge and PUT a replace, which is what those two NGSI-LD operations are; Update Attributes (10.2.3) is reached only through `$ref` writes. Upsert by PATCH/PUT on a missing key (Protocol 11.4.4) is refused: the clause allows a service to "fail the request altogether", and an upsert would need two decisions (Create Entity and Merge Entity) under one request.
- MCP: one tool per row, the 32 of `antares-mcp` (T-3442, T-3514): the `details=true` forms are tools of their own (`list_type_details`, `list_attribute_details`), and 10.3.3 is `batch_upsert`. Paging is by cursor over Query Entities, so no tool makes an EntityMap (`create_entity_map` not offered). The platform's `describe_schema` and `describe_access` are not single operations and are listed in section 7.
- LinkML: read-only by definition; every write row is `not_offered`. The schema is built from 10.6.3 to 10.6.8 and the served `@context` (13.4).
- WebSocket: subscriptions and their notifications only (section 2.4).
- internal CSR: `supported` means the handler already forwards that operation to Context Sources today (the federation Cypher in 2.2 lists its callers); the in-process form replaces the HTTP hop with `AppState::call` and keeps the operation. Discovery (10.6) is answered locally and only marked partial (`types_attrs::mark_partial`), registry, @context, subscription-registry and snapshot operations are not forwarded.

### 5.1 Gaps in the `Operation` an engine is handed

The seam can only judge what it is given. These handlers leave members empty that the request carries:

| id | TS 104 175 | gap |
|---|---|---|
| `create_entity` | 10.2.2 | 409 AlreadyExists on an id that exists but is out of view is an existence oracle |
| `batch_upsert` | 10.3.3 | an upsert on an existing out-of-view id must not become an overwrite: the engine needs the pre-image to decide |
| `query_subscriptions` | 10.5.5 | closed by T-3673: FILTERABLE (5.8.4, and 5.11.5 for `query_csource_subscriptions`); a Filter selects which Subscriptions are listed, counted and paged, read over the document with the creating subject as `subject` (`policy::record_image`), so `q=subject=="<caller>"` lists the caller's own; a Filter that projects is a Deny (`RECORD_LISTS`) |
| `query_registrations` | 12.3.3 | closed by T-3673 for selection and T-3692 for the owner: FILTERABLE (5.10.2), the Filter selects over the Registration's members and `subject`, the subject that created it, which the broker records on 5.9.2 (`__subject`, as a Subscription does), keeps across 5.9.3, never serves and never lets a body or a `csf` set or read; `q=subject=="<caller>"` lists the caller's own, count and pages included |

`batch_create` to `batch_delete` ask the engine once per member through `PolicyEngine::decide_each` (`crates/antares-api/src/batch.rs:747`, `:1252`): each member is shown its id, its types (stored and named), its Attributes and its expanded body, a refused member is reported in E with the AccessDenied problem and the others are applied (207); a batch refused whole is answered 403 (T-3480).

## 6. Write rows: pre-image, post-image and linked-entity checks

The ADR-0020 seam is called before the operation with the request (clause, ids, sometimes types, attrs, body) and, on a write by id, the stored image `Operation::stored` (types, scope, Attribute names; T-3483, T-3607 for the temporal writes by id). The check on the Entity as the write leaves it runs through a view, inside the store's write (`views::WriteGuard`, `views::admits_new`; ADR-0023 rule 7, T-3498). A check still missing is marked `[not enforced today ...]` in the yaml. `Filter` on any of these clauses is already `Deny` (`crates/antares-api/src/policy.rs:366-371` `resolve`), which is what ADR-0020 says: "A narrowing is refused where it cannot be applied."

The linked-entity checks, as the task defines them:

- `$expand` and `join`/`joinLevel`: the target is visible. On the NGSI-LD reads, the policy projection is carried down the join walk (ADR-0020; `crates/antares-api/tests/policy_filter_5_7.rs` `a_join_does_not_carry_a_narrowed_member_out_of_the_answer`), but the target's own visibility under the Filter's `q` and `scopeQ` is not checked: `lookup_joined` reads it with `st.store.get(tenant, Kind::Entity, id)` (`crates/antares-api/src/repr.rs:841-846`), and so does `collect_flat_walk` (`crates/antares-api/src/repr.rs:1047`). Callers: `retrieve_entity_inner`, `query_entities_inner`, `batch_query_inner` and `notify::build_data` (`crates/antares-api/src/notify.rs:943`).
- Relationship write: the `object` target is visible AND the relationship attribute is writable.
- Deep insert: each created Entity is its own Create Entity under the view.
- `$ref` add/remove: Update Attributes on the source plus visibility of the target.
- Batch: per Entity.

| id | TS 104 175 | pre-image | post-image | linked-entity checks | extra on OData |
|---|---|---|---|---|---|
| `create_entity` | 10.2.2 | the id does not exist in the tenant; the engine allows the expanded body (types, attrs); a Filter answer is Deny (not FILTERABLE) | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::admits_new` in entities.rs `create_entity_inner` refuses a create that would land outside the view, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | deep insert: each nested Entity is its own Create Entity (10.2.2) under the view, the parent's relationship attribute is a relationship write; OData 11.4.2.2 is all-or-nothing, so every gate is decided before the first write |
| `update_attributes` | 10.2.3 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the body names is writable under the view [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in attrs.rs `write_attrs`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | $ref add/remove/change: Update Attributes (10.2.3) on the source Entity's relationship attribute plus visibility of the target Entity under the view |
| `append_attributes` | 10.2.4 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the body names is writable under the view [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in attrs.rs `write_attrs`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `partial_attribute_update` | 10.2.5 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in attrs.rs `partial_update_inner`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `set_attribute_value` | 10.2.6 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `delete_attribute` | 10.2.7 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored and named), attrs and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the Entity is still inside the view without the Attribute (a view keyed on that Attribute would lose it) [enforced through a view: `views::WriteGuard::post` in attrs.rs `delete_attr_inner`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship delete: removing a Relationship needs the relationship attribute writable; the target is not read, so its visibility is not required | - |
| `delete_entity` | 10.2.8 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020) [the engine decides on clause, ids, types (stored and named) and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the Entity is absent; the change notifications it triggers pass pre_notify per subscriber | delete entity: Relationships in other Entities that point at the deleted one are not touched (NGSI-LD has no referential integrity), so no linked check | - |
| `merge_entity` | 10.2.9 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the body names is writable under the view [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in entities.rs `merge_entity_inner`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `replace_entity` | 10.2.10 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the stored Entity holds and every one the body names is writable (a replace deletes the ones it omits) [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in entities.rs `replace_entity`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view<br>relationship delete: removing a Relationship needs the relationship attribute writable; the target is not read, so its visibility is not required | - |
| `replace_attribute` | 10.2.11 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored and named), attrs, body and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard::post` in attrs.rs `replace_attr`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `purge_entities` | 10.2.12 | the engine answers Allow; any Filter is Deny (WHOLE_TENANT) | no Entity matching the purge selection remains | delete entity: Relationships in other Entities that point at the deleted one are not touched (NGSI-LD has no referential integrity), so no linked check | - |
| `batch_create` | 10.3.2 | per Entity: as create_entity; one gate call for the whole array today (ids of every member) | per Entity: as create_entity [enforced through a view: `views::WriteGuard` per member in batch.rs `view_guards` and `batch_write`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | batch: each Entity of the array is its own single-Entity operation under the view (one decision per entity, one per-entity error)<br>relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | deep insert: each nested Entity is its own Create Entity (10.2.2) under the view, the parent's relationship attribute is a relationship write; OData 11.4.2.2 is all-or-nothing, so every gate is decided before the first write |
| `batch_upsert` | 10.3.3 | per Entity: create_entity pre-image when absent, update pre-image (visible, attrs writable) when present [the engine decides on ids/clause only and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | per Entity: the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard` per member in batch.rs `upsert_in_view`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | batch: each Entity of the array is its own single-Entity operation under the view (one decision per entity, one per-entity error)<br>relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `batch_update` | 10.3.4 | per Entity: as update_attributes [the engine decides on ids/clause only and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | per Entity: the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard` per member in batch.rs `view_guards` and `batch_write`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | batch: each Entity of the array is its own single-Entity operation under the view (one decision per entity, one per-entity error)<br>relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `batch_merge` | 10.3.5 | per Entity: as merge_entity [the engine decides on ids/clause only and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | per Entity: the stored Entity after the write is still inside the view (q, scopeQ, type), so a write cannot move an Entity out of the caller's sight [enforced through a view: `views::WriteGuard` per member in batch.rs `view_guards` and `batch_write`, on the document the store is about to commit (ADR-0023 rule 7, T-3498); without a view a Filter answer on a write is a Deny] | batch: each Entity of the array is its own single-Entity operation under the view (one decision per entity, one per-entity error)<br>relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `batch_delete` | 10.3.6 | per Entity: as delete_entity [the engine decides on ids/clause only and the stored image, Operation::stored (types, scope, attribute names, T-3483)] | per Entity: absent | batch: each Entity of the array is its own single-Entity operation under the view (one decision per entity, one per-entity error)<br>delete entity: Relationships in other Entities that point at the deleted one are not touched (NGSI-LD has no referential integrity), so no linked check | - |
| `create_subscription` | 10.5.2 | the engine allows the subscription (the ids it names); a Filter answer is Deny; the subject is stored in __subject | every notification of it passes pre_notify under the stored subject (notify.rs:2084) | notification join: a subscription with join renders joined targets in notify.rs:943 (build_data -> inline_join); each target must be visible to the subscriber at pre_notify, today only the projection applies | - |
| `update_subscription` | 10.5.3 | the Subscription exists and is the caller's (visibility of a Subscription is unknown in the seam today) | the stored subject is unchanged; a client cannot set __subject (ADR-0020) | notification join: a subscription with join renders joined targets in notify.rs:943 (build_data -> inline_join); each target must be visible to the subscriber at pre_notify, today only the projection applies | - |
| `delete_subscription` | 10.5.6 | the Subscription exists and is the caller's | no notification of it is sent; forwarded copies are deleted | none: the operation names no Entity and writes no Relationship | - |
| `temporal_upsert` | 11.2.2 | when the Entity exists: target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the body names is writable under the view [not enforced today: the Operation carries no stored image, the engine decides on ids/clause only] | every written instance lies inside the temporal view [not enforced today: no post-image check runs on a temporal write; the engine sees the stored image since T-3607 (`policy::temporal_pre_image`)] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `temporal_add_attributes` | 11.2.3 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); every Attribute the body names is writable under the view [the engine decides on clause, ids, types (stored) and the stored image, Operation::stored (types, scope, attribute names) of the temporal evolution, else of the current state, T-3607] | every written instance lies inside the temporal view [not enforced today: no post-image check runs on a temporal write; the engine sees the stored image since T-3607 (`policy::temporal_pre_image`)] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `temporal_delete_attribute` | 11.2.4 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored) and the stored image, Operation::stored (types, scope, attribute names) of the temporal evolution, else of the current state, T-3607] | no instance of the Attribute remains | relationship delete: removing a Relationship needs the relationship attribute writable; the target is not read, so its visibility is not required | - |
| `temporal_modify_instance` | 11.2.5 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored) and the stored image, Operation::stored (types, scope, attribute names) of the temporal evolution, else of the current state, T-3607] | the instance lies inside the temporal view [not enforced today: no post-image check runs on a temporal write; the engine sees the stored image since T-3607 (`policy::temporal_pre_image`)] | relationship write: for each Relationship in the body, the object target Entity is visible under the view AND the relationship attribute is writable under the view | - |
| `temporal_delete_instance` | 11.2.6 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020); the named Attribute is writable [the engine decides on clause, ids, types (stored) and the stored image, Operation::stored (types, scope, attribute names) of the temporal evolution, else of the current state, T-3607] | the instance is absent | relationship delete: removing a Relationship needs the relationship attribute writable; the target is not read, so its visibility is not required | - |
| `temporal_delete` | 11.2.7 | target Entity exists and is visible under the view (else 404 ResourceNotFound, never 403: a hidden Entity is indistinguishable from an absent one, ADR-0020) [the engine decides on clause, ids, types (stored) and the stored image, Operation::stored (types, scope, attribute names) of the temporal evolution, else of the current state, T-3607] | no temporal evolution remains | delete entity: Relationships in other Entities that point at the deleted one are not touched (NGSI-LD has no referential integrity), so no linked check | - |
| `register_context_source` | 12.2.2 | the engine allows the registration; the registrant may claim only Entity types/ids it may write | forwards built from it carry the caller's narrowing and no subject header (ADR-0020) | registration information: the Entity ids/types a registration claims are judged as the registrant's write; forwarded requests carry the caller's q/scopeQ narrowing (ADR-0020) and never the subject headers | - |
| `update_registration` | 12.2.3 | the registration exists and is the caller's | as register_context_source | registration information: the Entity ids/types a registration claims are judged as the registrant's write; forwarded requests carry the caller's q/scopeQ narrowing (ADR-0020) and never the subject headers | - |
| `delete_registration` | 12.2.4 | the registration exists and is the caller's | no request is forwarded over it | none: the operation names no Entity and writes no Relationship | - |
| `create_csource_subscription` | 12.4.2 | as create_subscription | every csource notification passes pre_notify (notify.rs deliver_csource -> deliver_as) | none: the operation names no Entity and writes no Relationship | - |
| `update_csource_subscription` | 12.4.3 | as update_subscription | as update_subscription | none: the operation names no Entity and writes no Relationship | - |
| `delete_csource_subscription` | 12.4.6 | as delete_subscription | no csource notification of it is sent | none: the operation names no Entity and writes no Relationship | - |
| `add_context` | 13.2 | the engine allows; tenancy per ADR-0021 | the stored @context is served only to the tenant ADR-0021 assigns | none: an @context names no Entity | - |
| `delete_context` | 13.5 | the @context exists for the tenant | the @context is absent or reloaded | none: an @context names no Entity | - |
| `update_entity_map` | 14.3 | the map exists; only the expiry may change, by any subject (ADR-0020) | the id set and __subject are unchanged | none: only expiresAt changes | - |
| `delete_entity_map` | 14.4 | the map was built for this subject, else ResourceNotFound | the map is absent | none: no Entity is touched | - |
| `create_snapshot` | 16.2 | the engine answers Allow; any Filter is Deny (WHOLE_TENANT) | the snapshot records __subject; the fill runs after the response | snapshot fill: copies what the queries match, whole tenant; no per-Entity check | - |
| `clone_snapshot` | 16.3 | the engine answers Allow; any Filter is Deny (WHOLE_TENANT) | the clone records __subject | whole tenant copy; no per-Entity check | - |
| `update_snapshot_status` | 16.5 | the snapshot exists | a status notification is sent when an endpoint is set (snapshots.rs:1081) | none: the operation names no Entity and writes no Relationship | - |
| `delete_snapshot` | 16.6 | the snapshot exists | the snapshot and its frozen tenant are absent | none: the operation names no Entity and writes no Relationship | - |
| `purge_snapshots` | 16.7 | the engine answers Allow; any Filter is Deny (WHOLE_TENANT) | no snapshot remains | none: the operation names no Entity and writes no Relationship | - |

## 7. Rows with no NGSI-LD operation

### 7.1 OData features refused

Revised with T-3441 (`odata-linkml-mapping.md` section 11): the yaml now holds 30 rows here, the 16 below plus 13 refusals and one mapped form (`odata_query_segment`, `POST .../$query`) added there; `odata_search` is served since T-3750, `odata_compute` since T-3936 and `odata_apply` since T-3937; `odata_upsert` answers 404 `NotFound` and `odata_schemaversion` accepts `*` and the current version. Deep insert is refused 501 in round 1, and `$batch` members map to the single-Entity rows. The yaml is authoritative where this table lags.

Each is refused before any gate or store access with `501 Not Implemented` (Protocol 9.3.1) and a JSON Format 21.1 error object, `{"error": {"code": "NotImplemented", "message": "<feature> is not offered: no NGSI-LD operation stands behind it"}}`. The `code` value is a proposal: OData leaves the code service-defined.

| id | OData feature | OData 4.01 section | request | HTTP status | OData error code |
|---|---|---|---|---|---|
| `odata_apply` | $apply | OData Data Aggregation extension (not part of Protocol or URL Conventions 4.01) | `query option $apply` | served since T-3937 and T-3985 (`query_entities`, one bounded scan aggregated by the surface); `expand`, `nest`, the join and hierarchy transformations and rollup 501 | - |
| `odata_search` | $search | URL 5.1.7, Protocol 11.2.6.6 | `query option $search` | served since T-3750 (`query_entities`, a residual `$filter` conjunct) | - |
| `odata_compute` | $compute | URL 5.1.10, Protocol 11.2.5.3 | `query option $compute` | served since T-3936 (the read's operation, a residual value per Entity) | - |
| `odata_index` | $index | URL 5.1.11 | `query option $index` | 501 | `NotImplemented` |
| `odata_schemaversion` | $schemaversion | URL 5.1.12 | `query option $schemaversion` | 501 | `NotImplemented` |
| `odata_actions` | bound and unbound actions | URL 4.5.1, Protocol 11.5.5.1 | `POST {resource}/{namespace}.{action}` | 501 | `NotImplemented` |
| `odata_functions` | bound and unbound functions | URL 4.5.2, Protocol 11.5.4.1 | `GET {resource}/{namespace}.{function}(...)` | 501 | `NotImplemented` |
| `odata_crossjoin` | $crossjoin | URL 4.15 | `GET /$crossjoin(A,B)` | 501 | `NotImplemented` |
| `odata_all` | $all | URL 4.16 | `GET /$all` | 501 | `NotImplemented` |
| `odata_each` | $each (members of a collection) | URL 4.13, Protocol 11.4.14 | `PATCH\|DELETE /{EntitySet}/$each` | 501 | `NotImplemented` |
| `odata_delta_update` | update a collection with a delta payload | Protocol 11.4.12 | `PATCH /{EntitySet}` | 501 | `NotImplemented` |
| `odata_media_entities` | media entities | URL 4.14, Protocol 11.4.7 | `POST\|PUT\|DELETE media entity, GET .../$value` | 501 | `NotImplemented` |
| `odata_stream_properties` | stream properties | Protocol 11.4.8 | `PUT\|DELETE {entity}/{streamProperty}` | 501 | `NotImplemented` |
| `odata_batch_atomicity` | atomicity groups / change sets in $batch | Protocol 11.7.2, 11.7.7.4 | `POST /$batch with atomicityGroup` | 501 | `NotImplemented` |
| `odata_upsert` | upsert by PATCH/PUT on a missing key | Protocol 11.4.4 | `PATCH\|PUT /{EntitySet}('{new-id}')` | 404 | `NotFound` (the Merge or Replace Entity it is answers, gated) |
| `odata_derived_types` | derived type cast segments | URL 4.11 | `GET /{EntitySet}/{namespace}.{DerivedType}` | 501 | `NotImplemented` |

### 7.2 Routes and tools that are not an operation

Reachable today or in the platform, and not one of the 69 rows:

| route | handler | answer |
|---|---|---|
| `PATCH\|PUT /ngsi-ld/v1/entities` | crates/antares-api/src/lib.rs:1331 missing_entity_id | 400 BadRequestData, no store access, no gate |
| `OPTIONS any /ngsi-ld/v1 route, HEAD on every GET` | axum MethodRouter + conformance layer (lib.rs comment above API_ROOT nest) | OPTIONS 204 + Allow; HEAD = the GET operation, same gate |
| `POST /ex/v1/remote-notify` | crates/antares-api/src/distsub.rs:853 remote_notify | public to peers, by key: the body's `subscriptionId` is a random UUID minted per forwarded copy and sent only to that Context Source (ADR-0019), so it is the peer's credential for one subscription; an unknown key is 404 before any store write (tests/dist_subs_5_8.rs); no on_request gate; the remapped delivery passes pre_notify (unknown: not traced in this analysis) |
| `/q/health, /q/ready` | crates/antares-api/src/lib.rs:248 Admin::router | public: liveness and readiness probes, open even with an admin secret (T-3569); outside the seam by ADR-0020; no surface may proxy to /q |
| `/q/metrics, /q/tenants, /q/tenants/{tenant}, /q/dead-letters, /q/dead-letters/{id}, /q/dead-letters/{id}/replay` | crates/antares-api/src/lib.rs:248 Admin::router + admin_auth.rs require | outside the seam by ADR-0020 ('/q is outside the seam: the admin surface is the gateway's to protect'); with `ANTARES_ADMIN_TOKEN_FILE` set, 401 without the admin bearer secret (T-3569, tests/admin_auth.rs; `/q/metrics` open when `ANTARES_ADMIN_METRICS=open`); no surface may proxy to /q |
| `MCP describe_access` | joinedcontext-platform crates/context-gateway/src/mcp/endpoint_facade.rs:257 (operations: none) | proposal: answered from the Filter of a 10.4.3 gate, never from the engine's rule text |
| `MCP describe_schema` | endpoint_facade.rs:275 (operations QueryEntity, RetrieveEntity, RetrieveEntityTypes) | maps to the LinkML surface: 10.6.4 + 10.6.7 under the view |
| `MCP list_endpoints (hub)` | joinedcontext-platform crates/context-gateway/src/mcp/hub.rs:447 | platform-side endpoint directory, not a broker operation: stays in the platform (T-3436 decides) |
| `MCP initialize, tools/list, ping (protocol methods)` | joinedcontext-platform crates/context-gateway/src/mcp/server.rs | protocol only; tools/list lists only tools whose operation the view allows (proposal) |
| `OData GET /x/views/{view}/odata/v4/ and /$metadata` | proposal | mapped to retrieve_entity_types and retrieve_entity_type_details |

Coverage: the 68 method and path pairs (36 paths) of `router()` (`crates/antares-api/src/lib.rs:315-460`) each map to a row of section 4 or of this table (checked by a script that parses the router and the yaml: only `PATCH` and `PUT /entities` fall to this table); the admin surface and `/ex/v1/remote-notify` are here. A follow-up test (filed by T-3445) reads the yaml and asserts that every route of `router()` and of every surface `ApiSurface::router` has a row.

## 8. Open questions

Each has a recommendation, and the matrix assumes it. Where a choice had to be made it is recorded as decided (owner order 2026-10-10: decisions are taken, not parked); the owner may override any of them.

1. **Joined targets outside the view (filed as T-3470).** `lookup_joined` and `collect_flat_walk` embed a target Entity without checking it against the Filter's `q`/`scopeQ` (section 6). An Entity hidden from a query is still reachable as a join target of a visible one, on NGSI-LD today and on any surface that maps `$expand` onto `join`. Recommendation: treat it as a defect of the merged seam, file a task, and have the walk drop a target that the narrowed query would not select (the Relationship stays, the target is not embedded). No test was found for the out-of-view target; `policy_filter_5_7.rs` covers the projection only.
2. **Pre-image checks need the stored Entity.** Writes are judged on clause and ids; for `update_attributes`, `delete_entity`, `replace_entity` and the batch writes not even `types` is filled, and ADR-N-041 (CLAUDE.md, owner order 2026-10-06) forbids deriving a right from URN segments. Options: (a) the handler loads the stored Entity and passes its type, scope and attribute names into a new `Operation` member before the gate; (b) a second gate after the load. Recommendation (a): one gate per request (ADR-0014 rule 1), the stored image read once. **Done (T-3483):** option (a), `Operation::stored` (`StoredImage`: types, scope, attribute names, expanded) on every write by id and on each batch member; read only with an engine attached, the read the type lookup already made.
3. **Post-image (an Entity leaving the view).** Nothing stops a write that changes `scope` or the attribute a view keys on, so the caller writes an Entity out of its own sight or into another view. Recommendation: refuse a write whose post-image the view's `q`/`scopeQ` would not select, with the same answer as a deny.
4. **Existence oracle on create.** `create_entity` and `batch_create` answer 409 AlreadyExists for an id that exists but is out of view. ADR-0020 makes a hidden Entity look absent on reads (404). Options: answer 409 regardless (the id space is shared), or 403 for every create the engine has not allowed. Decided (worker-2, owner may override): 409 AlreadyExists, because TS 104 175 10.2.2.4 fixes that answer ("If the Entity already exists locally this shall result in an error of type AlreadyExists") and GW33's rule holds here too (a narrowing does not change a status code the specification fixes); any answer other than 201 tells the caller the id is taken, so a 403 would be the same oracle under a non-standard status. What the oracle reveals is that an id is in use in the tenant, never its type or content; a deployment that needs ids unguessable across views gives each view its own id prefix (ADR-N-041 keeps identity as (Context Space, URN), and a Context Space is a tenant, so ids never collide across spaces). Options were: 409 regardless, 403 for every create the engine did not allow. The 404 versus 403 question for reads is decided in the endpoint-views ADR (404, as ADR-0020 already answers a hidden retrieve).
5. **Discovery under a view.** 10.6.3 to 10.6.8 are not FILTERABLE, so an engine that narrows the type or attribute list gets a `Deny`. The platform's `list_types` and `list_attributes` promise "as the caller may see them" (`endpoint_facade.rs:154-178`), and the LinkML and OData `$metadata` documents are built from these operations. Recommendation: add 5.7.5 to 5.7.10 to FILTERABLE with `pick`/`omit` applied to the type and attribute names, and a test that a hidden type is absent from every one of them. Done by T-3479: `omit` hides a named type (its Entities) or Attribute, `pick` keeps the Attributes it names, q/scopeQ select the Entities the statistics come from.
6. **One decision per batch.** `batch.rs:672` asks once for the whole array. Recommendation: one `Operation` per member in one engine call (a batch-shaped `decide`), so ADR-0014 rule 1 still holds and per-member errors become possible.
7. **Deep insert atomicity.** OData demands all-or-nothing (Protocol 11.4.2.2), NGSI-LD has no transaction. Recommendation: decide every gate first; on a store failure after the first write, delete what was created and answer 500 with the ids created and deleted; or refuse deep insert (501) in round 1. Safest default in the yaml: supported only with every gate decided before the first write.
8. **Subject on internal CSR.** `federation::forward` strips the subject headers (ADR-0020, "The subject never travels"), while `AppState::call` copies them so the inner request is judged under the caller (`state.rs:556-557`). An in-process registration between tenants needs one rule: judged under the caller in the target tenant, under the registrant, or under the source tenant's Filter carried as `q`/`scopeQ`. Recommendation: the source tenant's narrowing travels as on HTTP forwards, and the target tenant's engine is asked again about the caller.
9. **TS 104 176 heading swap.** `9/9.4.3.1.md` and `9/9.4.3.2.md` (quoted in section 4). Closed: POST stays for create, and the swap is raised as [etsi-raises.md 24](../upstream/etsi-raises.md#24-spec-ts-104-176-9431-and-9432-carry-each-others-headings) (T-3674).
10. **`describe_access`.** It reveals what the grant lets the caller do. Recommendation: answer only from the `Filter` of a 10.4.3 gate (types and attributes the caller would get), never from rule text.
11. **Snapshot notification outside `pre_notify`.** Unknown whether a status notification can carry anything a narrowing should remove; the body is status only (`snapshots.rs:883`). Recommendation: leave it outside the seam and record that in ADR-0020's ledger.
12. **Notification join (T-3470 covers it).** A subscription with `join` renders joined targets at `notify.rs:943`; `pre_notify` projects but does not check visibility (same defect as question 1).
13. **`remote_notify` path.** Whether the remapped notification passes `pre_notify` for the original subscriber was not traced in this analysis: unknown.
14. **Doc comment drift.** `crates/antares-api/src/policy.rs:568` names the test `every_route_asks_the_policy_engine_once`; the test is `every_route_asks_the_policy_engine_exactly_once` (`crates/antares-api/tests/policy_gate_seam.rs:210`). **Done (T-3483).**

## 9. Related

- `docs/analysis/cim009-to-ts104175.md` (clause translation)
- `docs/adr/ADR-0014-extension-hooks-model.md`, `docs/adr/ADR-0020-policy-seam.md`, `docs/adr/ADR-0003-websocket-deferred.md`, `docs/adr/ADR-0019-remote-notification-endpoint.md`
- `crates/antares-api/src/policy.rs`, `crates/antares-api/src/lib.rs`, `crates/antares-api/src/surface.rs`, `crates/antares-api/src/state.rs`
- tasks T-3436 (what moves into the broker), T-3443 (architecture ADR), T-3444 (test plan), T-3445 (files the first round, including the yaml route test)
