# OGC API - Features endpoint of a view

An `ogc` build (`cargo build -p antares-broker --features ogc`) serves
every loaded [view](views.md) to OGC API - Features clients (QGIS, GDAL,
OpenLayers, the OGC test suite):

```text
GET /x/views/{view}/ogc/features/
GET /x/views/{view}/ogc/features/conformance
GET /x/views/{view}/ogc/features/api
GET /x/views/{view}/ogc/features/collections
GET /x/views/{view}/ogc/features/collections/{collectionId}
GET /x/views/{view}/ogc/features/collections/{collectionId}/schema
GET /x/views/{view}/ogc/features/collections/{collectionId}/sortables
GET /x/views/{view}/ogc/features/collections/{collectionId}/queryables
GET /x/views/{view}/ogc/features/functions
GET /x/views/{view}/ogc/features/collections/{collectionId}/items
GET /x/views/{view}/ogc/features/collections/{collectionId}/items/{featureId}
GET /x/views/{view}/ogc/features/collections/{type}-history/items
POST   /x/views/{view}/ogc/features/collections/{collectionId}/items
PUT    /x/views/{view}/ogc/features/collections/{collectionId}/items/{featureId}
PATCH  /x/views/{view}/ogc/features/collections/{collectionId}/items/{featureId}
DELETE /x/views/{view}/ogc/features/collections/{collectionId}/items/{featureId}
OPTIONS any of them
```

Mount it with `ANTARES_API_SURFACES=admin,views,ogc`. The design is
ADR-0029 and `docs/analysis/ogc-features-mapping.md`; this page is what
the build serves today (T-3744, T-3754, T-3746, T-3745): Part 1 Core,
`geojson` and `oas30`, Part 2 CRS84 and CRS84h, Part 3 filtering with every
CQL2 conformance class but `functions` (section 4; no custom function is
defined, so it is not declared), from the Part 4 draft (OGC
20-002r1) create, replace, update and delete, and from the Part 5 draft
(OGC 23-058r1) schemas, core roles, sortables and feature references.
The Part 4 and Part 5 classes are declared under the drafts' `1.0/conf`
ids, which can change when OGC publishes them: re-check them against the
published standard and ETS 1.9 then (watch row in `ogc-compliance.md`).
`sortby` and `properties` follow the Part 8 (sorting) and Part 6 (property
selection) drafts; their classes are not declared until those drafts have
stable ids, so a client finds them in the OpenAPI document only.
`HEAD` answers like `GET` without the body.

## 1. Collections

The collections are the view's Tenant's LinkML schema
(`/q/schemas/{tenant}`) intersected with what the view shows:

- one collection per class whose type the view shows, its `collectionId`
  the class name (a name that is no plain path segment is left out); a
  view of `*` (ADR-0023 rule 12) shows every class with every slot;
- a Feature's `geometry` is the slot annotated `ogc_role:
  primary-geometry`, else `location` when the view shows it, else `null`;
- `properties` are the other Attributes the view shows, by slot name;
- a slot annotated `ogc_role: primary-instant`, or
  `primary-interval-start` / `primary-interval-end`, is the time
  `datetime` filters on.

Roles come from the model. A slot's `ogc_role` annotation wins; where the
schema annotates no time or no place, the type's query guide
(`antares-ops/src/guide.rs`, keyed by class name: the Smart Data Models
types such as `Event` with `startDate`/`endDate`) gives the default.
A type no guide names gets its time and place from annotations alone, with
no code change. A role must fit its slot: `primary-geometry` only on a
`GeoProperty`, the time roles only on a scalar text Property (a `date`,
`datetime` or `string` range); a role on any other slot is ignored. Of two
slots with one role, the first in the class's slot order wins. A tenant
class that happens to share a guided class name inherits its defaults:
annotate the class to override them. The per-Endpoint role mapping of
named outputs is T-5207's contract.

A Feature carries each value in the shape `/schema` declares, so a GET
then a PUT stores it unchanged:

- an Attribute with several instances (`datasetId`) is served as its
  default instance (the one without a `datasetId`, as the broker's own
  GeoJSON picks), `null` when it has none; the other instances are not
  served on this surface (read them through NGSI-LD);
- a `VocabProperty` is its term, a `JsonProperty` its JSON, a
  `LanguageProperty` its language map (every language: the NGSI-LD `lang`
  filter is not mapped);
- a reference written back in the `rel-as-uri` or `rel-as-link` form is
  read back to its URN (§3).

A slot of a kind NGSI-LD does not define is refused when the schema loads
(`linkml-model`); the collection build keeps only the eight kinds, so such
a slot could never be written as a `Property`.

Not served, by design or not yet:

- per-instance `observedAt`/`unitCode` and the Entity's
  `createdAt`/`modifiedAt` are not Feature members, and `datetime` reads
  only the time role; the unit is the slot's `x-ogc-unit`;
- `scope` is reachable only as `ngsild.scopeQ`;
- a GeoProperty other than the geometry (`observationSpace`,
  `operationSpace`) is a property (`format: geometry-any`); a spatial CQL2
  predicate on it runs in the residual, within its 10 000-Entity budget, and
  a type with no `location` and no `primary-geometry` role has
  `geometry: null`;
- a type with data but no LinkML class, or a class name that is no path
  segment, has no collection, without a log line;
- history is served only as `{type}-history` for views that offer
  `temporal_query` (§7).

A view without a schema, or with one the build cannot read, has no
endpoint (`404`). A collection's `extent` is the bounding box (CRS84) of
the Features the caller reads through the view, read as the caller, so the
view's rules, its `q` and the Policy narrow it as they narrow `items`. One
read takes at most 1 000 Features, asks for no count (T-6508) and is kept for
60 s per view, collection and caller; a collection filling that read, a
`{type}-history` collection and one without a geometry state no `extent`. A `FeatureCollection` carries
`timeStamp`, the time the answer was made (T-5909).

A view's `representations.ogc` list ([Views](views.md)) adds a collection per
named output (T-5216): its id is the output's `name`, its `title` and
`description` the output's in the caller's `Accept-Language`, over the
collection of its one source class. The source's roles give the primary
geometry (`geometry`, the first path: `location` or a GeoProperty slot), the
temporal geometry (`instant`, or `start` and `end`) and the properties
(`properties`, with the time Attributes, which `datetime` reads). The
output's `filter` `q` and `scopeQ` are a conjunct of every read of the
collection, its items and its tiles; a geo-query is the caller's `bbox`, so an
OGC output names none. An output collection is read only (`405`), and one whose
roles name an Attribute the collection does not show, or a geometry that is no
GeoProperty, is not served. A class keeps its own id when an output is named
like it.

## 2. Items

`/items` is a Query Entities of the collection's type through the view,
paged by `ViewOps::scan` (ADR-0028 rules 4 to 6), so the view's `q` and the
policy engine narrow Features, `numberMatched` and the next link alike. A
property is named in the request's `@context` by its property name (by
`p{n}` when the core context defines that name or the query grammar cannot
read it), so an `ngsild.` option names it as `properties` does.
`numberMatched` is exact; with a residual (`bbox` beside `ngsild.georel`) it
is counted within the scan budget of 10 000 Entities and omitted beyond it,
while the page and its `next` link are still served.

| Parameter | Meaning |
|---|---|
| `limit` | 1 to 10 000, default 10; a page holds up to `limit` Features, read from the store in pages of its `max_limit` |
| `bbox` | `minx,miny,maxx,maxy` or the 6-number form, CRS84; a box with `minx > maxx` crosses the antimeridian. A `geoQ` `intersects` on the geometry; Features without a geometry are kept (Part 1 Req 24 C), served after the ones inside |
| `datetime` | an RFC 3339 instant or `start/end`, `..` or empty for an open end; both ends open is `400`. Features without a time are kept. Times compare as instants: a stored value with an offset (`2026-11-11T08:00:00+01:00`) is the instant it names, a date-only value (`2026-10-31`) is the whole UTC day, fractions past the microsecond are dropped. The store's `q` reads text, so it is sent 48 h wider and the residual decides; `numberMatched` is then counted within the scan budget |
| `next` | the token of a `next` link; it is bound to the view, the collection and the query, any other use is `400` |
| `sortby` | `[+\|-]name` by commas, names from `/sortables`; an NGSI-LD `orderBy`. With `bbox`, the Features without a geometry follow the sorted ones. A time stored as text with mixed offsets sorts by its text, not its instant |
| `properties` | property names by commas, empty for none; an NGSI-LD `pick` that keeps `id`, `type` and the geometry |
| `profile` | `rel-as-key` (default), `rel-as-uri` or `rel-as-link`, below |
| `crs`, `bbox-crs` | CRS84 or CRS84h (the URI or `[OGC:CRS84]`); any other is `400` naming both. The answer carries `Content-Crs` |
| `ngsild.q`, `ngsild.scopeQ` | an NGSI-LD `q` (joined to the request's own with `;`) or `scopeQ`; a broken one is `400` |
| `ngsild.georel`, `ngsild.geometry`, `ngsild.coordinates`, `ngsild.geoproperty` | the request's one NGSI-LD `geoQ`; a `bbox` beside it is evaluated on what the store answers |
| `ngsild.pick`, `ngsild.omit` | an NGSI-LD `pick` (not beside `properties`, `400`) or `omit` (beside `properties`, it removes from the selection) |

Every coordinate, in `bbox` and in a CQL2 literal, is longitude then
latitude (CRS84). A box written lat,lon is valid and usually holds no
located Feature, so the answer is only the Features without a geometry:
check the axis order before reading an empty map as no data.

`f=json` (and `f=geojson` on `/items`) asks for the JSON this surface
writes, whatever `Accept` says; any other `f` than the file encodings below
is `400`. A `HEAD` answers the `Content-Length` of its `GET` here; through
the platform gateway it has read `0` (the gateway half is tracked outside
this repository).

Any other parameter, a repeated one, or a parameter on a resource that
takes none is `400`; a property the view hides is refused like a
misspelt one. `/items/{featureId}` takes `properties`, `profile` and `crs` and
is one Query Entities by `id` and type: an Entity the view hides, of another type, absent, or an id
that is no URI is `404`.

A `parquet` build (`--features full,parquet`; not in `full`, kept out under
its ADR-0027 size budget) also writes `/items` as one file (T-4465,
EP-119): `f=parquet` or `Accept: application/vnd.apache.parquet` returns
the whole answer of the request's filters (`bbox`, `datetime`, `filter`,
`properties`, the `ngsild.` options) as Apache Parquet,
`Content-Disposition: attachment; filename="{collectionId}.parquet"`. The
file has an `id` column, a column per property the view shows (typed from
the collection's schema: text, 64-bit integer, double, boolean, anything
structured as JSON text) and, when the collection has a geometry, a
`geometry` column of ISO WKB with GeoParquet 1.1 `geo` metadata (encoding,
geometry types, bounding box; no `crs`, which means CRS84). A collection
without a geometry is plain Parquet. The surface reads the answer page by
page (1 000 Features) and writes each page as one row group, ZSTD
compressed, so it holds one page of Features at a time. A file larger than
the answer ceiling (64 MiB, `limits.maxAnswerBytes` in `/q/health`) is `413`, never a cut
file; a `crs` other than CRS84 is `400`, and so is `f=parquet` on a
single Feature. Every GeoJSON page of `/items` carries an `alternate` link
to its file. `/q/health` lists `"encodings": ["parquet"]` under the
surface, empty in a build without the feature.

A `tabular` build (`--features full,tabular`, beside `full` for the same
budget) writes `/items` as CSV too (T-4477, EP-08, EP-41; no Excel, owner
2026-10-18, so `f=xlsx` is `400`):
`f=csv` or `Accept: text/csv` returns RFC 4180 CSV
(`text/csv; charset=utf-8; header=present`, CRLF, UTF-8 without a byte
order mark). The columns are the Parquet file's: `id`, `geometry` as GeoJSON
text, a column per property the view shows; a text cell that starts with
`=`, `+`, `-`, `@`, a tab or a carriage return gets a leading `'`, so no
spreadsheet runs it as a formula. A CSV or Parquet file holds one
collection: `f=csv` or `f=parquet` on `/collections` is `400`. Several
collections come as one download from `/collections`: `f=zip` (or
`Accept: application/zip`) a ZIP of `{collectionId}.csv` per collection
(and `{collectionId}.parquet` in a build with `parquet` too), a
`README.md` naming every file and a `provenance.json`. It takes
`collections` (a comma list of collection ids; one the view does not show
is `400`; without it every collection but the `{type}-history` ones of
section 7, which join only when named), `bbox` and `datetime`, applied to
every collection; any other
parameter is `400`. The provenance names the view, the collections, the
query, the row count, the time of export and the broker version. The
answer ceiling holds for the whole file: past it, `413`. Every GeoJSON
page of `/items` carries an `alternate` link per file type the build
writes; `/q/health` lists them under `encodings` and the `/collections`
downloads under `bundles`.

A file is held whole in memory until it is sent, so the surface writes at
most `ANTARES_EXPORT_FILES` files at once (default 2,
[Configuration](configuration.md)), Parquet, CSV and ZIP together.
One more is `503` with `Retry-After: 6`, never queued; `/q/health` shows
the cap as `limits.exportFiles`.

## 3. Schemas and references

`/schema` is a JSON Schema 2020-12 document (`application/schema+json`)
of the Features of a collection: `id` (role `id`), `geometry` (role
`primary-geometry`) and every property the view shows, in Feature order
(`x-ogc-propertySeq`), each with its type, `x-ogc-definition` (the
Attribute IRI), `x-ogc-unit` (the slot's `unit.ucum_code`), its
description, and `x-ogc-role` from the slot's `ogc_role` annotation.
`/sortables` lists the properties `sortby` may name: one scalar value, no
Relationship, no geometry, no array.

A Relationship is a property with role `reference`; when its range names a
class the view shows, `x-ogc-collectionId` names that collection and the
profile decides how a Feature writes it: the URN (`rel-as-key`), the
target Feature's URI (`rel-as-uri`) or `{"href", "title"}`
(`rel-as-link`). A Relationship to a class the view does not show stays its
URN in every profile.

## 4. Filtering (Part 3, CQL2)

`/collections/{c}/queryables` is the collection's schema with
`additionalProperties: false`: `id`, `geometry` and every property the view
shows. `items` takes `filter` (`filter-lang` `cql2-text`, the default, or
`cql2-json`; `filter-crs` CRS84 or CRS84h) and each scalar queryable as a
parameter (`?speed=60` is `speed = 60`), `AND`ed with `bbox`, `datetime`
and the `ngsild.` options. A name that is no queryable, hidden or
misspelled, is the same `400`; so is a function call, as `/functions`
lists none. Parsing is bounded by `ogc-cql2` (16 KiB, 32 levels, 4 096
nodes).

Each top-level conjunct is lowered with the meaning CQL2 gives it (three
values: a missing or hidden value is unknown, also under `NOT`; arrays are
sets; the temporal functions are relations of start and end, `..` an open
end) and goes where NGSI-LD can say it exactly:

| conjunct | where |
|---|---|
| `=`, `<>`, `<`, `<=`, `>`, `>=`, `BETWEEN`, `IN` of a scalar property and literals; `AND`/`OR` of such | `q` |
| `IS NULL`, `IS NOT NULL` of a property | `q` (`!p`, `p`) |
| `LIKE`, `CASEI(p) = CASEI('x')`, `CASEI(p) LIKE CASEI('x%')` | `q` `~=` with an anchored pattern, `(?i)` for `CASEI` |
| `T_AFTER`, `T_BEFORE`, `T_DURING`, `T_INTERSECTS` and ordering comparisons of a time property and a `DATE`/`TIMESTAMP` literal | a `q` bound 48 h wider, decided by the residual: `q` compares text, the residual instants (an offset as written, a `DATE` the whole UTC day, a date-only value against a timestamp by its day) |
| `T_EQUALS`, `=`, `<>` and `IN` of a time property and such literals | the residual (two spellings of one instant are no text equality) |
| `A_OVERLAPS` of a `ListProperty` and literals | `q` `==a,b` |
| one `S_INTERSECTS`, `S_WITHIN`, `S_CONTAINS`, `S_EQUALS`, `S_DISJOINT`, `S_OVERLAPS` of `geometry` and a literal, when neither `bbox` nor an `ngsild.geo*` option holds the request's one `geoQ` | `geoQ` |
| everything else: `NOT`, `ACCENTI`, `S_TOUCHES`, `S_CROSSES`, a second spatial predicate, the other temporal and array functions, property-property comparisons, arithmetic (`+ - * / % div ^`) | the residual over the view's answer (ADR-0028) |

The residual only drops Features the view already returned, so a filter
never widens the view or its policy. Its scan reads at most 10 000
Entities per request; `numberMatched` is omitted past that. A `LIKE`
pattern that is no literal is `501`; a filter larger than the evaluator
allows is `400`.

## 5. Writes

A view that offers `create_entity`, `replace_entity`, `merge_entity` or
`delete_entity` takes `POST /items`, `PUT`, `PATCH` and `DELETE
/items/{featureId}`, each one Create, Replace, Merge or Delete Entity
through the view, so the view's rules and the Policy decide it. `OPTIONS`
and a `405` list in `Allow` the methods the view offers on the resource;
`/api` documents exactly those.

- `POST` and `PUT` take a GeoJSON Feature (`application/geo+json` or
  `application/json`), `PATCH` an RFC 7396 merge patch
  (`application/merge-patch+json`, a `null` deletes); anything else is
  `415`, a body above 1 MiB `413`.
- The geometry becomes the collection's GeoProperty, each property an
  Attribute of its LinkML kind; a property the view hides is refused like
  a misspelt one (`400`).
- `POST` uses the Feature's `id` when it has one, else mints
  `urn:ngsi-ld:{Type}:{uuid}`, and answers `201` with `Location`; the
  others answer `204`. An absent or hidden Feature is `404`.
- `If-Match` and `If-Unmodified-Since` are `501`: the broker has no
  atomic conditional write, and ignoring one could lose an update. A
  `Content-Crs` other than CRS84 or CRS84h is `400`.

## 6. Refusals and errors

A method the view does not offer on a resource is `405` with `Allow`, a
read whose `Accept` takes no JSON `406`. An error is `application/problem+json` (RFC 7807) with a fixed
`detail`: it names no hidden type or Attribute and echoes no request
value. A view that does not allow `query_entities` refuses `/items` with
the status the view gives (`405`).

## 7. History collections

A view that offers `temporal_query` serves, beside each collection
`{type}`, a read-only collection `{type}-history` (T-5100) for desktop GIS
clients that animate time (QGIS Temporal Controller, the ArcGIS Pro time
slider), which read neither NGSI-LD temporal answers nor SensorThings:

- one Feature per Entity and stored `observedAt`, its `id`
  `{entityId}@{observedAt}`;
- `properties` are the values valid then: each Attribute's latest instance
  at or before that `observedAt`; `geometry` the location valid then, or
  the Entity's current location when its history holds none (a station's
  place written once without `observedAt`);
- `entityId`, `validFrom` (the `observedAt`) and `validTo` (the Entity's
  next `observedAt`, `null` for its newest) are added; the queryables mark
  `validFrom`/`validTo` `primary-interval-start`/`primary-interval-end`;
- `datetime` keeps the Features valid in the window (`validFrom <= end`
  and `validTo > start`, an open `validTo` valid to the end of time),
  `bbox` and `filter` are evaluated in process over each Feature,
  `ngsild.q` and `ngsild.scopeQ` reach the Temporal Query; `sortby` and the
  other `ngsild.` options are `400`; `limit` and the `next` link page as
  in section 2, a page reads at most 100 Entities and ends short with its
  `next` link past them; no `numberMatched`;
- the Features come from Query Temporal Evolution of Entities through the
  view, so the view's rules and the Policy hide in the history what they
  hide in the current state; every write is `405`, `/api` lists none.

Each Entity's newest 1,000 instances per Attribute are read (the broker's
default `ANTARES_TEMPORAL_INSTANCE_LIMIT`): an older window answers no
Feature. A type whose space keeps no history (`ANTARES_TEMPORAL_RECORD=ruled`
without a rule) has an empty history collection.

## Related

- [Endpoint views](views.md)
- [OData endpoint of a view](odata.md)
- [OGC API - Features compliance](ogc-compliance.md): the OGC CITE ETS, its baseline and pass rate per class
- `docs/analysis/ogc-features-mapping.md` (rows M, C, R, F, W, S)
- ADR-0029 (the OGC surface and the `ogc-cql2` crate)
