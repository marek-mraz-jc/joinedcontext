# Views of another model: the Mapping IR on the antares-ql AST (gap G18, T-3639)

Sources: Antares-NGSI-LD-Context-Broker `45fd388` (dev), joinedcontext-platform `4db3004` (main), both read on 2026-10-11 (read only). Inputs: `docs/analysis/gateway-to-broker-map.md` rows K26, G18, S20 and B6 ("the map"), `docs/analysis/platform-switch-plan.md` sections 2.1 and 3.1 ("the plan"), ADR-0020, ADR-0023. Platform paths are under `crates/context-gateway/src/` unless another crate is named.

## 1. Scope and summary

An Endpoint with `spec.viewMappingRef` serves a space in another model: the broker holds the source class, the caller reads the target class of a Mapping. The plan keeps such Endpoints on the gateway (plan section 2.1, `viewMappingRef` row) because a view of another model transforms the query and the answer, which an ADR-0023 view, a narrowing, does not do (map G18). This page specifies the broker side so the gateway path can go.

Decisions:

- **The IR travels inside the view.** `View` gains an optional `mapping` member holding the compiled Mapping IR (version 2, the document the gateway reads today). `/q/views` validates it at load, as it validates every other member (section 3).
- **The query is inverted on the AST.** The view root parses each attribute-naming parameter, rewrites names and values on the `antares-ql` AST and renders it back with the round-trip `Display` (section 4). Nothing is split on characters, which removes the B6 case for this surface.
- **Effective = mapping(view ∧ engine).** The view and the engine judge the request in the source model, as they would without a mapping, and the mapping rebuilds whatever they let through (section 6). The mapping adds no row and no source value: a mapped view serves at most what the same view without a mapping serves.
- **Read only (S20), refused at load.** A view with a `mapping` whose `operations` name a write, a subscription, or anything outside the reads of section 4.1 is not loaded (section 7).
- **Tasks:** T-3669 (the interpreter and the entity reads), T-3670 (temporal reads and discovery through a mapped view), T-3671 (the Portal writes the IR into the view and the Endpoint leaves the gateway), T-3672 (defects of the gateway's string rewrite, section 2.6).

## 2. What the gateway did before T-4547 (historical)

Historical: this section describes the gateway path that T-4547 retired. The gateway's `view_mapping.rs` now only checks the IR version; its line cites below are to code that no longer exists.

### 2.1 The IR

`ViewMapping::parse` reads one JSON document, `version` 2 exactly (`translators/view_mapping.rs:29-30`, `:357-381`), with `sourceClass`, `targetClass` and a list of slots. A slot names its `target`, an optional `source`, a `kind` and whether it is `filterable` (`:333-344`, `:724-795`). The kinds (`Derivation`, `:61-85`):

| kind | value in the target model | inverse |
|---|---|---|
| `rename` | the source attribute as is | the name |
| `unitConversion` | `value * factor + offset` | `(value - offset) / factor` (`:561-582`) |
| `valueMappings` | a `forward` table; values outside it pass unchanged (`:596-601`) | the preimage under `forward`; the IR's `inverse` table is the compiler's and the broker does not read it |
| `cast` | Python `int()`, `float()` or `str()` of the value (`:323-331`) | none in the interpreter; the value passes as written (`:580`) |
| `constant` | a fixed value | none |
| `expr` | a typed expression tree over the entity's attributes (`:93-128`, evaluated `:193`) | none |

`filterable` is the compiler's verdict, never the interpreter's (`:786-791`). A slot without `source` is refused unless it is a constant or an expression (`:780-782`).

The IR lives beside its Mapping manifest as `spec.artifacts.gatewayIr`; `view_mappings` loads every one at repository load and leaves out one it cannot parse, with a warning (`store.rs:455-490`). The Endpoint finds it by `viewMappingRef` (`store.rs:413-420`; the field `jc-core/src/kinds/endpoint.rs:400`, a `Mapping` kind, `:508-523`).

### 2.2 The query

`invert_query` rewrites four parameters and passes the others through (`app.rs:2195-2224`):

- `attrs`: each target name to its source name; a constant drops out, an expression contributes the names it reads; a name the mapping does not define is 400 (`view_mapping.rs:457-476`).
- `q`: `invert_q` splits the string on `;`, `|`, `(` and `)` and rewrites each term: the name to its source, each value through `invert_value`; a name not in the mapping or not `filterable` is 400 (`:494-549`).
- `geoproperty`: the source name of a filterable slot (`:479-487`).
- `type`: replaced by `sourceClass`, whatever it named (`app.rs:2210`).

### 2.3 The answer

After the grants' projection, `translated` rewrites every entity of a successful JSON answer (`app.rs:2226-2250`): `translate_entity` keeps `id`, `@id`, `@context`, `scope` and the system members, sets `type` to the target class, produces constants and expressions, forwards each derived slot, and drops every member the mapping does not derive (`view_mapping.rs:394-450`). A document without `id`, such as a type list or an attribute list, passes unchanged (`:415-418`).

### 2.4 The grants

The grants are judged in the source model: the answer is projected with the constraint set before it is translated (`app.rs:1330-1344`). The one exception is the type a grant names, which is the target class; `Constraints::in_source_model` swaps it for the source class (`pdp/evaluator.rs:194-208`, T-2241).

### 2.5 Read only (S20)

A view endpoint answers every method outside GET, HEAD and OPTIONS with 405 and `Allow: GET, HEAD, OPTIONS`, before the request is decided (`app.rs:774-783`; the problem `view_mapping.rs:916-922`). The reason given there holds for the broker too: a mapping is invertible per slot, not per entity, so a write has no source entity to reconstruct.

### 2.6 What the string rewrite gets wrong

Each row is a reading of the code above, filed as T-3672 for as long as the gateway path serves. None widens the grants, which are judged separately (section 2.4); each answers the caller's own query wrongly.

| defect | where | effect |
|---|---|---|
| A quoted value holding `;`, `|`, `(` or `)` is split as if it were syntax | `view_mapping.rs:497-506` | the term after the split is read as an attribute name: 400, or a different query |
| A negative `factor` keeps `<` and `>` | `:574-579` | a range in the target model selects its complement in the source model |
| A many-to-one `forward` table inverts through one precomputed source value | `:568-573` | `==` misses the other source values that map to the same target value |
| `type` is replaced whatever it named | `app.rs:2210` | `type=Other` answers the source class |
| `pick`, `omit` and `orderBy` are not inverted | `app.rs:2206-2215` | a target name reaches the broker as an unknown attribute: nothing picked, nothing omitted, no order |
| A discovery answer passes untranslated | `view_mapping.rs:415-418` | `/types` and `/attributes` list the source model's names (fixed in the broker by section 5.1) |

## 3. The Mapping IR in a view

`View` (`crates/antares-ops/src/views.rs`, struct `View`, `deny_unknown_fields`) gains `mapping: Option<Value>`. At load, `View::load` parses it into the IR types of section 2.1, in a new module `antares-ops/src/mapping.rs`, and refuses the view when:

- the version is not 2, a slot is malformed or names an unknown kind (the gateway's own refusals, `view_mapping.rs:724-795`);
- the document or a slot carries a member version 2 does not define (T-5207): a slot takes `target`, `source`, `kind`, `filterable` and the members of its kind (`factor`/`offset`, `forward`/`inverse`, `range`, `value`, `expression`); a typo such as `ofset` is refused with the slot index and the member's name, never read as absent;
- `sourceClass` is not one of the view's `selector` types: the view would select Entities the mapping does not describe;
- an `operations` entry is not one of the reads of section 4.1 (section 7);
- the document exceeds the `/q/views` body cap the other members already share.

Names in the IR are written as the Mapping's compiler wrote them. The writer (T-3671) expands them to IRIs before it writes the view, as it does for `selector` and `rules` (plan section 2.1, `@context` row), so a caller's `@context` cannot rebind a slot name.

## 4. A query in the target model becomes an AST in the source model

### 4.1 Parameters

The view root (`antares-views`, `serve` in `crates/antares-views/src/lib.rs`) rewrites the query string before it builds the `OpsRequest`, for the reads a mapped view serves: Retrieve Entity, Query Entities (GET), Retrieve Attribute Value and, since T-3670, Retrieve and Query Temporal Evolution (GET); discovery is section 5.1. Of `negotiate::QUERY_PARAMS` (`crates/antares-api/src/negotiate.rs`), the ones that name attributes or types:

| parameter | rewrite |
|---|---|
| `type` | the target class becomes the source class; any other type selects nothing, answered as an empty result without asking the store (fixes 2.6 row 4) |
| `attrs`, `pick` | each target name to its source name; a constant drops out; an expression contributes the names it reads (`view_mapping.rs:165-185`); an unknown name is 400 |
| `omit` | a renamed or converted slot to its source name; a constant or an expression is removed from the rebuilt answer instead |
| `q` | section 4.2 on the AST |
| `geoproperty`, `geometryProperty` | the source name of a filterable slot, else 400 |
| `orderBy` | the source name of a `rename` slot, or of a `unitConversion` with its direction reversed when `factor < 0`; anything else is 400 |
| `timerel`, `timeAt`, `endTimeAt`, `timeproperty`, `lastN` | passed as written: a time or a count, and `timeproperty` names a member NGSI-LD puts on every instance |
| `aggrMethods`, `aggrPeriodDuration`, `options=aggregatedValues` | refused (400): an aggregate of a translated value is not the translation of the aggregate (a `unitConversion` offset summed n times, a `valueMappings` table averaged) |
| `datasetId`, `lang`, `expandValues`, `join`, `joinLevel` | refused (400) through a mapped view: the IR does not say how a dataset, a language map, a vocabulary or a joined Entity of another class maps |

A path segment that names an Attribute (`/entities/{id}/attrs/{attr}/value`) is rewritten as `attrs` is. A view `scopeQ`, `geoQ` and `q` are the view's own, written in the source model, and are not rewritten.

### 4.2 Terms

`parse_q` gives a `QNode` (`crates/antares-ql/src/lib.rs`, enum `QNode`): `And`, `Or`, `Cmp { path, op, value }` and `Exists { path, negated }`. The rewrite walks it and maps each leaf. A leaf names its slot by the path's first segment. A compound member path (`a.b`, `a[b]`) keeps its tail after the renamed first segment on a `rename` slot only; on any other kind it is 400 (`DEEP_PATH`), and `orderBy` refuses every member path, `rename` included. A path with a linked-entity hop (`attr{...}`) is 400: the IR maps one class, and the Entity the hop reaches is of another. The result is rendered with `QNode`'s `Display`, whose round trip `parse_q(&node.to_string()) == node` holds (`crates/antares-ql/src/render.rs:1-12`).

| slot kind | `Exists` | `==`, `!=` (value or list) | `<`, `<=`, `>`, `>=` | `~=` |
|---|---|---|---|---|
| `rename` | name | name | name | name |
| `unitConversion` | name | number through the inverse; a non-number is 400 | number through the inverse; `<` and `>` swap, as do `<=` and `>=`, when `factor < 0` (fixes 2.6 row 2) | 400 |
| `valueMappings` | name | the preimage under `forward`: every source value whose forward value equals the literal, as an `Or` of `==` (an `And` of `!=` for `!=`); a literal no forward value equals stays as written, since values outside the table pass unchanged (fixes 2.6 row 3) | 400: a table keeps no order | 400 |
| `cast` | name | 400 | 400 | 400 |
| `constant`, `expr` | 400 | 400 | 400 | 400 |

Every cell that maps also requires the slot to be `filterable`; the compiler's verdict can only take a cell away, never add one. Decided (worker-1, owner may override): `cast` is not filterable here even when the IR says so, because `int()` truncates and a source range would have to be derived per target value; the options were to send the value as written, as the gateway does (`view_mapping.rs:580`, which misses `3.7` for `==3`), or to derive the range. A refusal is 400 BadRequestData naming the target attribute, as the gateway words it (`:901-913`).

## 5. The answer rebuilt

The view root rebuilds the answer after `view.run` returns, so after the view and the engine have narrowed it (section 6). The rebuild is `translate_entity` (section 2.3), ported, on every Entity of the body in each representation the read can produce: normalized, concise and simplified (`options=keyValues`). Members the mapping does not derive are dropped. A value outside a `valueMappings` table passes unchanged, as in the gateway, so that the inverse rule of section 4.2 stays true. `NGSILD-Results-Count` stays as the store answered: the mapping changes the shape of each Entity, not which Entities are served.

Headers that name the API root are rebased as they are today (`rebase` in `antares-views/src/lib.rs`).

A temporal answer (11.3.2, 11.3.3) is rebuilt instance by instance (`Mapping::rebuild_history`): each instance of a `rename`, `unitConversion`, `valueMappings` or `cast` slot in the normalized form, and each `[value, time]` pair of the `temporalValues` form (4.5.9). A `constant` or `expr` slot has no history and is left out: a constant never changed, and an expression would read instances of several source Attributes that do not share their times. Decided (worker-2, owner may override): left out, because inventing a history for a value nobody stored would be data the store does not hold; options were an instance per source instance with the expression evaluated over the latest value of the others, or one instance at the Entity's `createdAt`.

### 5.1 Discovery

The six discovery reads (10.6.3 to 10.6.8) run through the view as they do without a mapping, so ADR-0023 rule 5 narrows them first; `Mapping::rebuild_discovery` then names the answer in the target model. The source class becomes the target class and any other type is dropped; a source Attribute becomes every slot that reads it. A `rename`, `unitConversion`, `valueMappings` or `cast` slot shows when its source does, an `expr` slot when every source it reads does, a `constant` when the answer shows the class or one of its Attributes; a slot without a source is listed as a `Property`. `/types/{type}` names the target class (any other type is 404 without the store) and `/attributes/{attrId}` a slot with a source (a `constant` or `expr` slot is 400, as for Retrieve Attribute Value). Only `details`, `local` and `count` pass. `entityCount` and `attributeCount` are the store's numbers under the view.

## 6. Composition with the view and the engine

The request the view root hands to `view.run` is in the source model, so everything the broker already does applies unchanged: the view's narrowing in the store query (ADR-0023 rule 1), a hidden attribute absent while `q` is evaluated (rule 2), an out-of-view Entity not found (rule 3), and the engine's decision on an `Operation` that names source types and attributes (ADR-0020). The answer the mapping rebuilds is therefore the answer the same view without a mapping serves, and the rebuild adds nothing taken from the source:

- a derived slot reads one attribute that is in the narrowed Entity, or is absent;
- an expression reads the attributes that are in the narrowed Entity; one it needs and the view hides is absent, and the expression is left out (`view_mapping.rs:433-438`);
- a constant is the Mapping's own value, not data of the Entity.

So a mapped view never widens what the source view serves. The grants are written in the source model, as the gateway assumes (`app.rs:2226-2229`). The one target-model name a grant carries today, the class (section 2.4), is rewritten by the writer: the view's `selector` and `rules[].types` carry the source class (T-3671), so the broker has no `in_source_model` step to make.

Two consequences, both inside that bound:

- An inverted `q` can select a little more than the caller's target-model reading would: a `unitConversion` equality compared in floating point, or a `valueMappings` value outside the table. The rebuilt answer may then hold an Entity whose target value does not satisfy the caller's literal; it is still an Entity the view serves.
- A refusal (400) names only target attributes the caller wrote, never a source name.

## 7. Read only (S20)

The broker refuses at load what the gateway refuses per request (section 2.5): a view with a `mapping` may list only `retrieve_entity`, `query_entities`, `retrieve_attribute_value`, the temporal reads `temporal_retrieve` and `temporal_query`, and the six discovery reads (`mapping::READS`, T-3670). A write, a subscription, a batch, the `POST` queries (`query_entities_post`, `temporal_query_post`) whose body would need its own inversion, an Entity Map and a Context Source operation are not loadable. Requests for them reach `refusal` (`refusal` in `antares-views/src/lib.rs`), which answers 405 with the `Allow` the view serves, the answer S20 asks for. A write through the source model with the view's grant (map S20, last column) cannot happen: the view does not offer it, and the root's own routes are not the view's.

## 8. Tasks

| id | repo | what | depends on |
|---|---|---|---|
| T-3669 | Antares | `antares-ops/src/mapping.rs` (IR parse, AST inversion of section 4, rebuild of section 5) and the `View.mapping` member with the load checks of sections 3 and 7; the view root inverts and rebuilds for Retrieve Entity, Query Entities and Retrieve Attribute Value | T-3639 |
| T-3670 | Antares | the temporal reads (11.3.2, 11.3.3) and discovery (10.6.3 to 10.6.8) through a mapped view: discovery answered from the mapping's target names | T-3669 |
| T-3671 | portal | the view writer embeds the Mapping's `gatewayIr`, expands its names and writes source types into `selector` and `rules`; the plan's `viewMappingRef` exception is removed | T-3669 |
| T-3672 | platform | the defects of section 2.6, fixed only if the gateway path still serves when it is taken | none |

T-3669, T-3670 and T-3671 go into T-3446's and T-3644's `depends-on:`.

## 9. As built (T-3669)

T-3669 implements sections 3 to 7 in `crates/antares-ops/src/mapping.rs` and `crates/antares-ops/src/mapped.rs`. It decides the following beyond the text above, or against it.

- **Names.** IR names are expanded through the view's `@context` over the core at load, and the target model's terms are the view's own. A request to a mapped view that sends a `Link` header is 400.
- **The inner request.** The inner request runs under an inline `@context` (`OpsRequest::context`) of generated terms (`jcSrc0`, …, `jcSrcClass`), one per source IRI. A `q` name is a 7.2.3.2 name, so no IRI can appear in it. The rebuild reads each source Attribute under the key the broker compacts it to under that context.
- **Other surfaces.** `Ops::view` returns `None` for a mapped view, so OData, OGC and WebSocket answer it as an unknown view. The view root and MCP take it through `Ops::mapped_view`, and `ViewOps::run` inverts and rebuilds for both (`crates/antares-ops/src/mapped.rs`, T-3817): MCP offers the reads of `mapping::READS` alone, and its input schemas name the target class and the slots the view can produce (`ViewOps::target_names`).
- **Selector.** A mapped view's `selector` is the mapping's `sourceClass` and nothing else, which is stricter than section 3. Any other type would be served rebuilt as the target class.
- **`valueMappings` inversion.** `==x` inverts to every key whose forward value is x, plus x itself when x is no key of the table. This is the precise form of the 4.2 cell: a value outside the table passes unchanged.
- **`geoproperty` and `orderBy`.**
  - `geoproperty` and `geometryProperty` name a filterable `rename` slot only. A geometry has no unit and no value table.
  - `orderBy` names a `rename` slot, or a `unitConversion` slot by `asc` or `desc`. `dist-*` applies to `rename` only.
- **`pick` and `omit`.** These two apply to the rebuilt answer and are never forwarded. `attrs` also restricts the rebuilt answer to the slots it names, so an input fetched for an expression is not served as an attribute of its own.
- **Unlisted parameters.** A parameter section 4.1 does not list, and that names no attribute (`id`, `limit`, `options`, …), passes through as written. Every other parameter, a parameter named twice, `format=geojson` and `Accept: application/geo+json` are 400.
- **Refusals.** Every refusal is a fixed text (`Refusal::Malformed`). The broker's own refusals (a 404 naming an attribute) are reworded from generated terms into the target names before they leave the root.
- **Retrieve Attribute Value.** Through a mapped view it serves a slot with a source Attribute. A constant or computed slot there is 400, and is read through Retrieve Entity.
- **Tests.**
  - `mapping/tests.rs` holds the gateway's cases, ported, and every cell of 4.2.
  - `views_mapped.rs` runs the root end to end.
  - `views_mapped_props.rs` is the section 6 property: a mapped view serves no id, value or Attribute the same view without its mapping does not.

## 10. Since T-5210

Found by the mapping validation of 2026-10-22 (`evidence/supervisor-2026-10-22-mapping-validation/coraine-views.md`, B9 to B18).

- **Default geo-query (B9).** A geo-query (`georel`) without `geoproperty` filters the target model's `location` (4.10): `Mapping::invert` adds the `geoproperty` of the filterable `rename` slot that serves `location`, and refuses the request with `NOT_FILTERABLE` when there is none. Before, the inner request filtered the source model's `location`.
- **Attribute kinds (B10).** The rebuild translates `value`, `object`, `vocab` and each element of `valueList` and `objectList`, in the normalized, concise and simplified forms and in `temporalValues` (`values`, `objects`, `vocabs`, `valueLists`, `objectLists`). A `languageMap` or `json` value under a `unitConversion`, `valueMappings` or `cast` slot is not served: the IR does not say how to translate it. A `rename` serves every kind.
- **Hidden slots (B11).** The loader notes each slot whose source Attribute no rule of the view shows (`Mapping::hidden_slots`); such a slot was and is never served. Decided (worker-t5210, owner may override): a note, not a refusal, because the Portal writes the rules from the Policies, and a Policy that hides one source Attribute must not take the whole view down; the check never widens a view. The check against the Tenant's stored LinkML schema is not in the broker: `antares-ops` may not depend on `linkml-model` (module rule M2), so it belongs to the Portal reconciler, which holds the model and writes both documents.
- **Unknown members (B12).** Each slot kind has an allowlist of members (`target`, `source`, `kind`, `filterable`, and `factor`, `offset`; `forward`, `inverse`; `range`; `value`; `expression`). Any other member refuses the view with the slot's index.
- **Scalars (B13).** A `unitConversion` serves nothing for a text or boolean source value; a number past the JSON range keeps its value, and an object (a geometry) passes. A `valueMappings` table keyed by a number also matches the same number stored as a float (`"1"` and `1.0`).
- **IR version (B14).** The broker runs version 2 exactly and refuses any other at load. Rule for v2: a new member is a new IR version, since the broker refuses unknown members.
