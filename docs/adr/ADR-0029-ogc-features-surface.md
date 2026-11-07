# ADR-0029 — OGC API - Features as a view surface: `antares-ogc` and the protocol crate `ogc-cql2`

Date: 2026-10-11. Status: accepted. Decided by worker-4 under the owner
order of 2026-10-11 (via supervisor): OGC API - Features becomes a surface
of the Antares view endpoint like OData, usable by Apps, serving its full
query feature set over the Entities the NGSI-LD view and its Policy admit.
The owner may override any rule below. Built on ADR-0023, ADR-0027 and
ADR-0028 (residual query evaluation, T-3748). The row-by-row mapping is
`docs/analysis/ogc-features-mapping.md`. Supersedes, for OGC Features
only, decision O-1 and row D10 of `docs/analysis/gateway-to-broker-map.md`
(files, dumps and SensorThings stay as decided there, T-3651). Task T-3743.

## Context

`docs/analysis/gateway-to-broker-map.md` decided (O-1, option a) that OGC
Features stays a gateway translator re-based on the broker view, because
the surfaces GOAL did not list it. The owner order of 2026-10-11 lists it.
Today the platform serves it from `context-gateway`:

| file | lines | what it holds |
|---|---|---|
| `src/handlers/ogc.rs` | 381 | routes, PDP and tenancy calls, `limit`/`next`, RFC 7807 errors |
| `src/translators/ogc.rs` | 642 | `bbox` → `georel=intersects`, `datetime` → `timerel` on `observedAt`, feature conversion |
| `src/translators/geojson.rs` | 176 | geometry from `location`, sub-attributes flattened to `{name}_unitCode` |
| `src/translators/cql2.rs` | 1061 | CQL2-text lexer, parser and compiler straight to `q`/`geoQ` strings; refuses `NOT` over spatial, a second spatial predicate, spatial or temporal under `OR` |

`cg` (platform graph, worker-1 clone, built 2026-10-10):

```cypher
MATCH (s:Symbol) WHERE (s.file CONTAINS 'ogc' OR s.file CONTAINS 'cql2') AND s.file CONTAINS 'src/' AND NOT s.is_test RETURN s.file, count(*);
// handlers/ogc.rs 7, translators/ogc.rs 37, translators/cql2.rs 96
MATCH (c:Symbol)-[:CALLS]->(f:Symbol) WHERE f.file='crates/context-gateway/src/translators/cql2.rs' AND NOT c.is_test AND c.file <> f.file RETURN DISTINCT c.path, f.name;
// only handlers::ogc::ogc_items calls cql2::compile
```

The broker already writes the GeoJSON representation of Query Entities and
Retrieve Entity (4.5.16, `crates/antares-api/src/repr.rs:1509`), relates
every DE-9IM geometry (`crates/antares-ql/src/geo.rs`), orders
(`orderBy`), counts (`count=true`) and picks.

## Decision

1. **Crate `antares-ogc`**, role `surface`, cargo feature `ogc =
   ["views", "dep:antares-ogc"]` of `antares-broker`, added to `full`;
   route root `/x/views/{view}/ogc/features` under the shared `/x/views`
   nest (ADR-0027 section 1). It depends in the workspace on
   `antares-ops`, `ogc-cql2` and `linkml-model` only (module rule M2). It
   serves Part 1 Core with GeoJSON and OpenAPI 3.0, Part 2 for CRS84 and
   CRS84h, Part 3, Part 4 and Part 5 as the mapping page's rows say. Every
   request is one or more `ViewOps::run` calls as the caller; no route
   reads the store.
2. **Crate `ogc-cql2`**, role `protocol`, beside `odata-url`: CQL2 text
   and JSON parsed into one AST, with the depth and size bounds as
   constants, no evaluation and no NGSI-LD knowledge (module rule M1: no
   `antares-*` dependency). `antares-ogc` lowers the AST into the pushed
   `q`/`geoQ` and the ADR-0028 residual expression.
3. **What moves and what is rewritten.** Nothing moves as code:
   - `cql2.rs` is rewritten as `ogc-cql2`. Its output type (`Compiled{q,
     geo, temporal}`) is the pushdown step, which belongs to the surface,
     not to a protocol crate; its errors are `ogc::ParamError`; and it
     reads CQL2-text only. Its test cases are ported as parser and
     pushdown tests (each gateway test that asserts a `q` string becomes an
     `antares-ogc` pushdown test with the same input).
   - `translators/ogc.rs` and `geojson.rs` are replaced by the broker's own
     GeoJSON writer (`format=simplified`, `geometryProperty`), so the
     `{name}_unitCode` flattening goes away (units are in the Part 5
     schema, mapping row M5).
   - `handlers/ogc.rs` is replaced by forwarding (T-3752); its PDP and
     tenancy calls go away with the gateway's PDP (map section 7.6).
4. **Collections** are the types the view shows, **Features** the Entities
   of the type the view returns, **`featureId`** the Entity URN, and the
   **primary geometry** the slot annotated `ogc_role: primary-geometry`
   in the Tenant's LinkML, else `location` (mapping M1 to M4). The temporal
   geometry is a declared slot only (M6).
5. **Paging.** `limit` default 10, clamped to the broker's `max_limit`; the
   `next` token is offsets plus a hash of the normalised query and the
   view id, re-validated per request. No signing key: the token grants
   nothing, since the query runs as the caller.
6. **Counts.** `numberMatched` only when known exactly (all predicates
   pushed down, or the residual read the whole set within budget);
   otherwise omitted.
7. **Not offered:** HTML and GML encodings (406), CRS other than CRS84
   and CRS84h (400 with the list), optimistic locking (`If-Match`,
   `If-Unmodified-Since` → 501, no atomic conditional write).
8. **Slices.** T-3744 Part 1 Core; T-3753 `ogc-cql2`; T-3745 Part 3 and
   CQL2 (depends on T-3744, T-3753, T-3748); T-3746 Part 4 and Part 2;
   T-3754 Part 5 schemas, sortables, `sortby`, `properties`, feature
   references; T-3759 items through `ViewOps::scan` with the `ngsild.*`
   options (ADR-0028); T-3747 the CITE ETS job; T-3751 the SDK and App sample;
   T-3752 the gateway switch. T-3446 depends on each.

Decided (worker-4, owner may override): a new surface crate plus a
protocol crate, because ADR-0027 makes every standard on a view a
feature-gated surface and the module rules keep parsers free of broker
types; options: (a) a surface crate with the parser inside, (b) surface
plus protocol crate (chosen), (c) keep the gateway translator (O-1 as it
was).

Decided (worker-4, owner may override): rewrite `cql2.rs` rather than
move it, because its output is NGSI-LD strings and its error type is the
gateway's, so a moved copy would keep neither its interface nor its
callers; options: move and adapt, rewrite with its tests ported (chosen).

## Consequences

- An App reads Features straight from its view (`/x/views/{view}/ogc/
  features/collections/{c}/items` as a MapLibre GeoJSON source), with the
  same Policy as the NGSI-LD and OData surfaces.
- The gateway loses about 2260 lines and its second decision point for OGC
  once T-3752 lands.
- Operators the residual engine does not serve yet answer 501 naming the
  operator; the conformance page lists them.

## Confirmation

| decision | check |
|---|---|
| 1, 2 | `dev/check-module-rules.py` (roles, M1, M2); `ci/vanilla-tree.txt` unchanged; `dev/check-vanilla-symbols.sh` |
| 3 | `cg` in T-3752: no caller of the deleted gateway symbols |
| 4 to 7 | the `ogc/*` and `cql2/*` tests named in the mapping page |
| 8 | T-3446 `depends-on` lists every slice; T-3747's ETS baseline |
