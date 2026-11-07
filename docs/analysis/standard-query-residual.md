---
title: Standard queries over a view, pushdown and residual per standard
task: T-3748
---

# Standard queries over a view: pushdown and residual per standard

What each standard surface pushes into the NGSI-LD Query Entities request,
what the residual evaluator of ADR-0028 serves, and what stays 501, with
the task that builds each row. Sources: `odata-linkml-mapping.md` (row ids
F*, Q*), `crates/antares-odata/src/filter.rs` and `read.rs` on dev
`fbef380`, TS 104 175 / 104 176 V1.1.1 (`/workspace/etsi-specs/`), and the
task bodies of T-3743 to T-3745, T-3749 and T-3750 for the OGC and OData
scope.

## 1. Code relations behind this analysis

Rerun from an Antares clone (`cg build --reuse` first):

```cypher
// the one entry every surface read goes through
MATCH (c:Symbol)-[:CALLS]->(f:Symbol {path:'antares_ops::ViewOps::run'}) WHERE NOT c.is_test RETURN DISTINCT c.path, c.file;
```

Result on dev `fbef380` (graph built 2026-10-11 03:47): `antares_odata::read::run_request`,
`antares_views::serve`, `antares_mcp::server::Server::run`,
`antares_mcp::server::Closing::drop`, `antares_ws::socket::run`. Only the
OData read builds a query from a second language, so it is the only
caller `ViewOps::scan` replaces today. The OGC surface (T-3744) is the
second.

Platform side (graph of the platform clone, built 2026-10-10):
`MATCH (c:Symbol)-[:CALLS]->(f:Symbol) WHERE f.file='crates/context-gateway/src/translators/cql2.rs' AND NOT c.is_test AND c.file <> f.file RETURN DISTINCT c.path, f.name`
gives `handlers::ogc::ogc_items` → `cql2::compile` only.

## 2. Pushdown targets

The NGSI-LD request a pushdown can fill (TS 104 176 7.2.3.1, Query Entities):

| parameter | carries | note |
|---|---|---|
| `type` | the set's or collection's type | always |
| `id` | an id conjunct (OData F28) | |
| `q` | comparisons with a literal, `;` and, `\|` or, ranges `..`, value lists `,`, `~=` | one `q`; `ngsild.q` joins with `;` |
| `scopeQ` | `ngsild.scopeQ` only | no standard maps scope |
| `georel` `geometry` `coordinates` `geoproperty` | one geo relation: near, within, contains, overlaps, intersects, equals, disjoint (TS 104 175 7.2.4) | one per request; `ngsild.geo*` wins it |
| `attrs` (`pick`) / `omit` | projection ∪ residual reads, narrowed by `ngsild.pick`/`ngsild.omit` | ADR-0028 rule 3 and 6 |
| `orderBy` | sort keys that are members (`id`, `type`, `scope`, an Attribute) | ends in `id` under a residual (rule 4) |
| `options=sysAttrs` | when the residual reads `createdAt`/`modifiedAt` | F33 |
| `limit` `offset` `count` | paging, the pushdown's count | rule 4 and 5 |

## 3. OData 4.01

Rows refer to `odata-linkml-mapping.md`. "Pushdown" keeps today's
translation in `filter.rs`; "residual" is new. T-3758 routes OData reads
through `ViewOps::scan`; T-3749 and T-3750 add the constructs.

| row | construct | pushdown | residual | stays 501 | task |
|---|---|---|---|---|---|
| F1 to F10, F12, F15, F22, F26, F28, F29, F31, F32 | the mapped subset | yes, as today | | | done |
| F11 | `not` other than `not (p eq null)` | | exact three-valued `not` | | T-3749 |
| F13 | `has` | | over the enum flags, once the model has enum types (section 3.4 of the mapping) | until enums exist | T-3749 |
| F14 | `add` `sub` `mul` `div` `divby` `mod`, negation | | checked arithmetic, overflow is `unknown` | | T-3749 |
| F16 | `matchesPattern` | | `regex` crate, ECMAScript-shared subset | look-around, back-references | T-3749 |
| F17 | string functions | | yes | | T-3749 |
| F18 | date and time functions, `now` | | yes; `now` is the request's start time, one value per request | | T-3749 |
| F19 | `round` `floor` `ceiling` `cast` `isof` `case` | | yes | | T-3749 |
| F20 | complex path `p/a/b` | `q` path once P2 types the ComplexType (T-3686) | until then | | T-3749 |
| F23 | `all`, `any` with any other body | | lambda over a list value | | T-3749 |
| F24 | navigation path in a filter | | | gated on T-3470 (ADR-0023 rule 6) | T-3470 |
| F25 | property against property | | yes | | T-3749 |
| F27 | guid, duration, binary, enum, complex and collection literals | | compared as typed values | geography literals other than F29's polygon, until T-3756 | T-3749 |
| F29 beyond a top-level conjunct | `geo.intersects` anywhere | the top-level conjunct, as today | elsewhere, through T-3756 | | T-3749 |
| F30 | `geo.distance`, `geo.length` | | | ADR-0028 rule 8 (unit unstated) | none |
| F33 | `createdAt`, `modifiedAt`, `_types`, `_datasets` | | with `options=sysAttrs` | | T-3749 |
| Q9 | `$orderby` on an expression | members only | sort within the budget (rule 5) | | T-3758 |
| Q13, `$count=true` | count with a residual | the pushdown's count when the residual is empty | exact within the budget, else 400 `TooManyResults` | | T-3758 |
| Q14 | `$search` | | free text over the String Attributes the view shows | | T-3750 |
| Q16 | `$compute` | | from T-3749's expressions | | T-3750 |
| Q19 | `$apply` | `filter(...)` steps that push | `groupby`, `aggregate`, `topcount` and the other transformations | | T-3750 |
| Q27 | custom query options | `ngsild.*` (rule 6) | | every other name stays 400 | T-3758 |
| Q17, Q21, Q24, Q36 | `$index`, `$deltatoken`, `$root`, `$crossjoin`/`$all` | | | ADR-0028 rule 8 | none |

## 4. OGC API Features with CQL2

T-3744 builds the core surface, T-3759 routes its `items` through
`ViewOps::scan` with the `ngsild.` parameters, and T-3745 adds Part 3 (CQL2).

| CQL2 class | pushdown | residual | stays 501 |
|---|---|---|---|
| Basic CQL2: comparison with a literal, `AND`, `OR`, `NOT`, `IS NULL` | comparison and `AND`/`OR` as `q`; `NOT` only as `!p` for `IS NULL` | `NOT` in general, as F11 | |
| Advanced comparison: `LIKE`, `BETWEEN`, `IN` | `BETWEEN` as a `q` range, `IN` as a `q` value list | `LIKE` (wildcards compiled to a `regex` pattern, rule 7) | |
| Case- and accent-insensitive comparison (`CASEI`, `ACCENTI`) | | yes | |
| Basic spatial (`S_INTERSECTS` with a point or bbox) | `georel=intersects` | when `ngsild.geo*` holds the slot | |
| Spatial: `S_WITHIN`, `S_CONTAINS`, `S_EQUALS`, `S_DISJOINT`, `S_OVERLAPS` | the matching georel | a second geo predicate in the same filter | |
| Spatial: `S_TOUCHES`, `S_CROSSES` | | T-3756 (no NGSI-LD georel) | |
| Temporal `T_*` on instants | `T_BEFORE`/`T_AFTER`/`T_EQUALS` against an instant literal as a `q` comparison | intervals and every other `T_*` | |
| Array `A_*` | | yes | |
| Property-property comparison | | yes | |
| Functions (`/functions`) | | the closed table of T-3755 | |
| Navigation into a related Feature | | | gated on T-3470, as F24 |

## 5. Other surfaces

- **NGSI-LD view** (`antares-views`): its own language, nothing to push or
  evaluate.
- **MCP** (`antares-mcp`): the query tool takes NGSI-LD parameters
  (`mcp-surface.md`), so it has no second language. Nothing to do.
- **LinkML** (`antares-linkml`): serves the schema, not Entities.
- **SensorThings API**: no STA surface exists and none is in this epic's
  board, so no slice is filed (GOAL point 10: no placeholder tasks). The
  STA surface task, once filed, compiles its `$filter` (an OData dialect)
  through T-3755 and pages through `ViewOps::scan`.

## 6. Slices

| task | what | depends on |
|---|---|---|
| T-3755 | `antares-residual`: the intermediate form, three-valued evaluator over a normalized NGSI-LD Entity, the function table T-3749 and T-3745 need (string, date/time, math, checked arithmetic, regex subset, lambdas over lists), bounds (depth 64, nodes 4096); pure, `forbid(unsafe_code)`, Miri | |
| T-3756 | geometry predicates in `antares-residual`: intersects, within, contains, equals, disjoint, overlaps, touches, crosses over GeoJSON values (the `geo` crate's `relate`). As built: `Expr::Spatial(Spatial, a, b)` with a `Lit::Geometry` parsed once at `compile` (RFC 7946 shapes, WGS84 range, a closed ring of four or more positions, as the write path); past `MAX_GEO_VERTICES` (1 024, the `geoQ` bound) a literal is `TooComplex` and a value unknown; held against the pushed `georel` matcher by `residual_relation_equals_pushed_georel`. The surface compilers (OData `geo.intersects`, CQL2 `S_*`) map onto it in T-3749 and T-3745 | T-3755 |
| T-3757 | `ViewOps::scan` in `antares-ops`: pushdown request, residual, fetch-until-filled within `RESIDUAL_SCAN`, upstream cursor, exact count and sort within the budget, 400 `TooManyResults` beyond it. As built: `scan(caller, Pushdown, Option<&Program>, Page)`; without a residual one Query Entities with `limit`/`offset` (and `count` for a whole answer); with one, `orderBy` ends in `id`, upstream pages of `max_limit` from `Page::start`, `next` the upstream offset past the last Entity served, a page short of its size when the 10 000 budget runs out, a whole answer counted first and refused over the budget (no count is refused too); `attrs` gains what the residual reads and `options=sysAttrs` when it reads a system member, both stripped before the page leaves; an evaluation past `MAX_STEPS` is 400. Held by `tests/view_scan.rs` (`residual_never_widens`, `residual_pages_concatenate` over page sizes, upstream pages and budgets, `residual_scan_is_bounded`, `hidden_attribute_is_absent_in_residual`) | T-3755 |
| T-3758 | OData reads through `ViewOps::scan`: conjunct split in `filter.rs` (`Refused::Later` becomes "residual" for what T-3755 covers), cursor skiptoken, `$count` and `$orderby` with a residual, `ngsild.*` custom options (row Q27); mapping rows first | T-3757 |
| T-3759 | OGC `items` through `ViewOps::scan`: `ngsild.*` parameters, `next` from the cursor, `numberMatched` exact within the budget and omitted beyond it | T-3757, T-3744 |

T-3749 then depends on T-3755, T-3756 and T-3758; T-3750 on T-3758;
T-3745 on T-3756 and T-3759.

## Related

- ADR-0028, the decision this analysis feeds
- `odata-linkml-mapping.md`, rows F1 to F34 and Q1 to Q39
- ADR-0023 rule 2 (hidden Attributes are absent while `q` is evaluated)
- `surfaces-test-plan.md`, where the ADR's property tests join the plan
