# Architecture

A map of the Antares NGSI-LD context broker for anyone who has to change
it: what lives where, how a request and a change travel, which invariants
hold the system together, and what to touch for a given kind of change.
The user-facing story is the book under `docs/src/`; irreversible
decisions are in `docs/adr/`; the clause-by-clause conformance ledger is in
`docs/spec/`. This file is the index that links them to code.

Every path below is relative to the repository root. Line counts are
approximate and mark weight, not authority.

## 1. Shape in one paragraph

One binary (`crates/antares-broker`) serves ETSI CIM 009 V1.9.1 over HTTP
(`crates/antares-api`, axum). Handlers negotiate the request
(`negotiate.rs`), await a store trait
(`crates/antares-store`), and return. The store fires a change hook on
every write and awaits it, so the history that write produced is there for
the next read; the hook hands the change
to the notification pipeline (`notify.rs`), which matches subscriptions
from an in-memory mirror and delivers over HTTP or MQTT
(`crates/antares-notifier`). Storage is a ladder behind one trait:
process maps, redb file, PostgreSQL/PostGIS, TimescaleDB
(`crates/antares-sql`). With `ANTARES_BUS=nats` the same binary splits
into roles (api, matcher, notifier, temporal, registry) joined by a
JetStream stream fed from a transactional outbox. The browser build
(`crates/antares-wasm`) runs the same router behind a Service Worker.

## 2. Crates and their contracts

Dependency direction runs one way — `broker → api → {bus, sql, store,
jsonld, ql, matcher, notifier, model}` — and cargo refuses a cycle in it.
What CI adds (`.github/workflows/workspace.yml`, "Shared crates
standalone") is the stronger rule for the five crates a gateway can take on
its own, `model`, `ql`, `jsonld`, `matcher` and `store`: each is built,
tested and documented alone, and its dependency tree may name neither the
broker nor a storage backend (`antares-api`, `antares-broker`,
`antares-sql`, `sqlx`, `axum`, `redb`).

| Crate | Owns | Must not |
|---|---|---|
| `antares-model` | CIM 009 types verbatim (`Entity`, `NgsiError` = Table 6.3.2-1), publishable | depend on anything Antares |
| `antares-ql` | `q=`, `scopeQ`, `geoQ` parsers → typed AST; the in-memory evaluator (`eval`); `PRIMER`, the NGSI-LD primer every MCP surface sends (T-4532, re-exported by `antares-ops`) | know HTTP or SQL |
| `antares-jsonld` | `@context` loader with caches and pinned core contexts, expansion/compaction, structural validation, the one outbound `client_builder` | do business logic |
| `antares-matcher` | subscription vs entity: selector, conditions, activity, throttling | touch a store |
| `antares-store` | `CurrentStateDriver`, `TemporalDriver` (object-safe, `lib.rs:140`, `:385`), `Kind`, filters/paging (`filter.rs`) | pull a backend |
| `antares-sql` | AST → SQL compiler (`compile/`), migrations, the sqlx drivers (`store/pg/`), the memory/redb drivers (`store/mem/`), `AnyStore` facade (`store/any.rs`) | be called from handlers directly (see §7) |
| `antares-bus` | `ChangeEvent`, the JetStream bus, subjects, core request/reply | decide who consumes |
| `antares-notifier` | `NotificationSink` (schemes, `parse_endpoint`, `deliver`, `network`) chosen from `SinkRegistry` by endpoint scheme: http (`http.rs`), mqtt behind the feature, delivery policy, `Outbound` | match or store |
| `antares-api` | the HTTP binding: routers, handlers, negotiation, federation, notification pipeline, snapshots, bounds | own a backend or a transport |
| `antares-broker` | composition root: env → config, roles, bus wiring (`wiring.rs`), telemetry, shutdown | contain clause logic |
| `antares-wasm` | the router under a Service Worker, OPFS-backed file store | diverge from the native router |
| `antares-ops` | the operation layer every surface serves through (ADR-0027 section 1): `Ops` over `AppState::call` (or `AppState::call_in_context` when a surface defines its terms with `OpsRequest::context`, so the outer `Link` never redefines them), `OperationId` (the matrix rows), `Refusal`, `Surface`, `Visible` (what a caller may see through a view, `view ∧ engine`); `ViewOps::scan` (`scan.rs`, T-3757, ADR-0028 rules 3 to 5): a standard's `Pushdown` run as Query Entities through the view with the `antares-residual` program (re-exported as `residual`) applied to what it answered, a page filled from an upstream cursor ordered to end in `id` within `RESIDUAL_SCAN` 10 000 Entities read, a whole answer counted first and refused over the budget (`Refusal::TooManyResults`), the Attributes and system members only the residual read stripped; `names.rs` (T-3758, ADR-0028 rule 6): the NGSI-LD names a caller writes beside a standard's query (`ngsild.q`, `pick`, `omit`, `geoproperty`) expanded under the core and the view's own `@context` and rewritten on the `antares-ql` AST into the surface's terms; the `ForwardTransport` re-export, `with_forward_transport` and `Ops::forward_in_process` (ADR-0025 rule 8, ADR-0027 section 9); the view table (`views.rs`: `View`, load and validation, `/q/views`, reload, rule M4 closing a surface with no view; `views/rule_documents.rs`: the engine's rule documents at `/q/policy`); each Tenant's `LinkML` schema (`/q/schemas/{tenant}`, bounded by `ANTARES_SCHEMA_MAX_BYTES`, handed to a surface as bytes by `ViewOps::schema`); a view of another model (`mapping.rs`, gap G18, `docs/analysis/view-mapping.md`): the Mapping IR version 2 compiled at load against the view's `@context` (a view with a `mapping` selects its source class alone and allows only the reads of `mapping::READS`: Retrieve Entity, Query Entities, Retrieve Attribute Value, the temporal reads and discovery), the request inverted on the `antares-ql` AST into generated terms of an inline `@context`, the answer rebuilt in the target model; `ViewOps::run` serves such a view to every surface that takes it through `Ops::mapped_view` (`mapped.rs`: the view root and MCP, T-3817), `Ops::view` hides it from the others | be linked by a vanilla build, name the store, SQL, bus or notifier |
| `antares-views` | the NGSI-LD view root `/x/views/{view}/ngsi-ld/v1` (ADR-0023, role surface, feature `views`): the reads and the rule-7 writes through `ViewOps` in the view's Tenant, 405/422/404 by rule 4, `Link`/`Location` rebased to the view root, no entity maps; a view of another model through `ViewOps::run` (`antares-ops` `mapped.rs`: the request inverted, a `Link` `@context` and a GeoJSON answer refused, a `type` the view does not serve answered empty without the store, the answer rebuilt, a history instance by instance, a discovery answer named by the mapping, T-3670, and a refusal reworded into the target model) | name the store, the policy engine or any crate but `antares-ops` |
| `antares-mcp` | the MCP endpoint of a view `/x/views/{view}/mcp` (T-3442, role surface, feature `mcp`, rmcp Streamable HTTP, stateless): 15 read tools, 17 write tools run only after the person confirmed (MRTR elicitation, sealed single-use requestState, T-3514) the type and Entity resources, and resource subscriptions through subscriptions/listen, each an internal Subscription made through the view (T-3515); each one `ViewOps` operation; the tool list filtered by the view and one engine probe, input schemas from the view's names, byte-budget paging with an opaque cursor, no sessions | name the store, the policy engine or any crate but `antares-ops`; log an argument value |
| `antares-ws` | the WebSocket notification channel (ADR-0026, role surface, feature `ws`): `GET /x/ws/ngsi-ld/v1` for a Tenant and `GET /x/views/{view}/ws` for a view, every message the TS 104 243 envelope, each Subscription an ordinary Create, Update or Delete Subscription through the operation layer with the endpoint `urn:antares:ws:<connection>` | open a delivery path of its own beside the notifier, or name the store |
| `linkml-model` | the LinkML subset the surfaces read (odata-linkml-mapping 8 and 9, ADR-0024): LinkML's resolved JSON under caller `Limits`, every metaslot outside the subset refused (LK4); IRIs, ranges, NGSI-LD kinds and class slots; deterministic writers to LinkML YAML, JSON Schema, a JSON-LD `@context`, SHACL, OWL, RDFS and Markdown that quote every name (LK3) | know its host (no `antares-*` dependency, rule M1) or read YAML |
| `antares-linkml` | the LinkML schema of a view `/x/views/{view}/linkml` (T-3511, role surface, feature `linkml`): the view's `linkml` read with `linkml-model`, narrowed to `Visible` (classes, slots, `is_a`, ranges, enums, prefixes), rendered as the platform's seven documents and index, per class and per slot, strong ETag, cached per view, schema and visible set | name the store, the policy engine or any crate but `antares-ops` and `linkml-model`; render from the unnarrowed schema |
| `antares-odata` | the OData endpoint of a view `/x/views/{view}/odata/v4` (T-3508, role surface, feature `odata`): its EDM model built per request from the Tenant's `LinkML` schema (`ViewOps::schema`, parsed once per revision) intersected with the types and Attributes the view shows (`edm.rs`, odata-linkml-mapping 3 and 7.2: names mangled per 3.3, value types per 3.4, R1/R2 navigations between visible types, R5 URL strings to hidden ones), the service and metadata documents through `odata-axum`, its Capabilities restrictions written from the same test the reads apply (C11 to C13; advisory, every request still gated, O25); reads (`read.rs`, 5.1 Q13/Q29/Q30): an EntitySet as one Query Entities (also `POST /$query`, its body served as the `GET` it stands for, O24), an Entity by key as one Retrieve Entity, `/$count`, below one Entity (`paths.rs`, Q31 to Q33) a property and its `$value` as one Retrieve Attribute Value, the Entities a navigation names as one Retrieve Entity or Query Entities `id=` and their `$ref`, with `$filter` and `$orderby` (`filter.rs`, 5.2 F1 to F10, F12, F15, F24 one single-valued hop under a `join`, F26, F28, Q9 `orderBy` on `id` and scalar Attributes: a `q` over terms `t{n}` the request's own @context defines, `OpsRequest::context`; split by top-level conjunct, T-3758: what `q` cannot say and the evaluator covers, F11 `not` with OData's null rules and a second F29 or F28 conjunct, is the `antares-residual` program `ViewOps::scan` applies, a page filled from the upstream cursor, `$count` and a `createdAt`/`modifiedAt` `$orderby` over the whole answer within `RESIDUAL_SCAN`, else 400 `TooManyResults`; the custom options `ngsild.q`, `scopeQ`, `georel`, `geometry`, `coordinates`, `geoproperty`, `pick`, `omit` ANDed into the pushdown, Q27), `$select`/`$top`/`$skip`/`$count`, `$expand` one level deep as `join=inline` over the expanded Relationships only (Q3, Q6 to Q8: `*`, `/$ref`, a nested `$select`; a target the view hides reads like an absent one, O5; a walk past the broker's lookup budget 400, O6), `Prefer: odata.maxpagesize` and a next link with a `$skiptoken` (`2.{cursor}.{skip}.{top}.{digest}`) bound to the view and the query (S5.2, O4), each through `ViewOps::run` and mapped back through the model; writes of one Entity (`write.rs`, section 6 W1, W2, W4, W6, W7, W9: Create, Merge, Replace and Delete Entity of the set's type in normalized form, a single-valued bind as a Relationship after its target is read under the view, O15; an operation the view does not offer 405 before its body is read; deep insert and collection binds 501, O14; `If-Match` 501 for any key, O18); writes below one Entity (`below.rs`, W10, W11, W13, W16, W17, W20: a property by Replace Attribute, Partial Attribute Update or Delete Attribute in its member's kind, a navigation's `$ref` by Append Attributes or Delete Attribute after every target is read under the view; one `$ref` in a collection 501); `$batch` (`batch.rs`, W24 to W26: multipart and JSON through `odata-batch`, each member served by the same router as a request of its own under the outer request's tenant and subject, O12; a member outside the service root, a member header outside the OData allowlist or a bound passed refuses the whole batch before any member runs, O8, O9, O11; the first failure ends it unless `continue-on-error`, a failed `dependsOn` 424); the model built once per key (view, what it shows, schema revision, OData version; a bounded cache, O21); no schema or an unreadable one closes the endpoint (404) | name the store, the policy engine or any crate but `antares-ops` and the protocol crates; name a hidden type or Attribute in any document |
| `antares-icsr` | internal context sources (ADR-0025, T-3520, role surface, feature `internal-csr`): `SelfUrls` (`ANTARES_SELF_URLS`, exact match after the HTTP client's normalisation), `ExportGrant` under the reserved Tenant `antares-grants` with `/q/grants` on the admin surface, and the `ForwardTransport` that serves a forward to a self URL through `Ops::forward_in_process` (or a grant's `ViewOps`) after the target's grant allows it, and refuses a registration at 12.2.2/12.2.4 without one; a 5.8.1.4 copy's notifications to the receiver path under a self URL are posted through the router with no subject, after the grant is read again on every attempt (rule 7, T-3700) | name the store or any crate but `antares-ops`; send a forward to a self URL over HTTP; take the subject from a registration |
| `antares-ogc` | the OGC API - Features endpoint of a view `/x/views/{view}/ogc/features` (T-3744, ADR-0029, role surface, feature `ogc`): Part 1 Core, `geojson` and `oas30`, Part 5 draft schemas, core roles, sortables and feature references (T-3754); Part 2 CRS84/CRS84h (`crs.rs`: `crs`, `bbox-crs`, `Content-Crs`, others 400 with the list) and Part 4 draft writes (`write.rs`, T-3746: `POST`/`PUT`/`PATCH` merge-patch/`DELETE` as Create/Replace/Merge/Delete Entity through `ViewOps::run`, only the operations the view offers, else 405 with `Allow` before the body is read; `OPTIONS`; body 415/413/400 checks, a hidden property the same 400 as an unknown one; `If-Match` 501); the collections built per request from the Tenant's `LinkML` schema (`ViewOps::schema`) intersected with the types and Attributes the view shows (`model.rs`, mapping M1 to M7: the `ogc_role` annotation or `location` the geometry, `primary-instant`/interval the time); `/items` as Query Entities of the type through `ViewOps::scan` (`items.rs`, T-3759, ADR-0028 rules 4 to 6: `limit` clamped at 1 000, `bbox` as a `geoQ` intersects plus the Features without geometry, across the antimeridian a MultiPolygon, or as the residual when an `ngsild.geo*` option holds the one `geoQ`, `datetime` as a `q` that keeps Features without time, the `ngsild.` options `q`, `scopeQ`, `georel`, `geometry`, `coordinates`, `geoproperty`, `pick`, `omit` ANDed into the pushdown and declared in `/api`, Attributes named in the request's @context by their property names, a `next` token of the part and its upstream offset bound to the view and the query, `numberMatched` exact or, past the residual budget, omitted); Part 3 filtering (`filter.rs`, T-3745: `filter` in `cql2-text`/`cql2-json`, the scalar queryables as parameters, every CQL2 class, each conjunct lowered into an ADR-0028 residual expression and pushed into `q` or the one `geoQ` where NGSI-LD says it exactly; `/queryables`, an empty `/functions`); `/items/{featureId}` as Query Entities by id and type; `/collections/{c}/schema` and `/sortables` JSON Schema 2020-12 of what the view shows (`schema.rs`, `x-ogc-role`, `-unit`, `-definition`, `-propertySeq`, `-collectionId`); `sortby` as `orderBy` on scalar properties, `properties` as `pick`, `profile` `rel-as-key`/`-uri`/`-link` rendering a Relationship only to a class the view shows as a link; `/api` the `OpenAPI` 3.0 document of the shown collections only (`api.rs`); errors RFC 7807 with fixed details; no schema or an unreadable one closes the endpoint (404); with feature `parquet`, `f=parquet` or `Accept: application/vnd.apache.parquet` on `/items` writes the whole answer as one file through `antares-export` (`export.rs`, T-4465), 413 past `max_answer_bytes`; with feature `tabular`, `f=csv` on `/items` and `f=zip` on `/collections` (a CSV per collection, no Excel, `collections`, `bbox`, `datetime`, provenance and README, T-4477), `f=csv`/`f=parquet` on `/collections` 400 | name the store, the policy engine or any crate but `antares-ops` and the protocol crates; name a hidden type or Attribute in any document |
| `antares-sta` | the OGC SensorThings API v1.1 endpoint of a view `/x/views/{view}/sta/v1.1`, Sensing profile, read only (T-4462, role surface, feature `sta`): the eight sets from the view's Query Entities naming the types it shows (`ViewOps::scan`) and its Temporal Retrieve/Query for HistoricalLocations and a Datastream's Observations; the ISO 19156 mapping in `mapping.rs` (`docs/analysis/sta-mapping.md`: ids as the gateway made them, a `datasetId` its own Datastream, an ObservedProperty its expanded IRI, Sensors the `Device` of `refDevice`, FeaturesOfInterest `refPointOfInterest` or the own `location`); `$filter` to `q` and the temporal window, `$top` at most 100, `$expand` at most 2 deep, `$filter` at most 4 096 bytes (`query.rs`); pages cut at 1 MiB between Entities with `@iot.nextLink`; OData JSON errors with fixed messages | name the store, the policy engine or any crate but `antares-ops`; read around the view; name a hidden type, Attribute or Device in any set, count or id |
| `antares-ical` | the iCalendar (RFC 5545) feed of a view `/x/views/{view}/ical/v1/events.ics` (T-4487, role surface, feature `ical`): the view's `Event` types from its Query Entities (`ViewOps::scan`), one `VEVENT` each per `docs/analysis/ical-mapping.md` (`UID` the id, `DTSTART`/`DTEND` from `startDate`/`endDate`, dates exclusive-ended, date-times in UTC), `from`/`to` window, an Event without `startDate` or `name` left out and counted (EP-126), lines escaped and folded at 75 octets | name the store, the policy engine or any crate but `antares-ops`; read around the view; serve an invalid `VEVENT` or a hidden type or Attribute |
| `antares-geojson` | GeoJSON of a view `/x/views/{view}/geojson/v1/{type}` (role surface, feature `geojson`, generated by `dev/new-surface.py`): the Entities of one type of a view as an RFC 7946 `FeatureCollection`, each `location` its geometry and the other Attributes its properties. | name the store, the policy engine or any crate but `antares-ops`; read around the view; write |
| `antares-feed` | Feeds of a view `/x/views/{view}/feed/v1/{name}/{atom.xml\|rss.xml\|feed.json}` (T-5202, role surface, feature `feed`, generated by `dev/new-surface.py`): the outputs of `representations.feed`, each one Query Entities of its source types through the view (`ViewOps::scan`), its filter joined before the request `q`; entries newest first with tag-URI ids, text escaped, http(s) links only, RFC 5005 paging, ETag/304; an Entity without title or `updated` left out and counted (EP-126) | name the store, the policy engine or any crate but `antares-ops`; read around the view; write |
| `antares-gbfs` | GBFS of a view `/x/views/{view}/gbfs/v1/{name}/{file}.json` (role surface, feature `gbfs`, generated by `dev/new-surface.py`): the bike-hire docking stations of a view as a GBFS 3.0 feed: `gbfs.json`, `system_information.json`, `station_information.json` and `station_status.json`. | name the store, the policy engine or any crate but `antares-ops`; read around the view; write |
| `antares-gtfsrt` | GTFS Realtime of a view `/x/views/{view}/gtfsrt/v1/vehicle-positions.{pb,json}` (role surface, feature `gtfsrt`, generated by `dev/new-surface.py`): the live `Vehicle` Entities of a view as a GTFS Realtime 2.0 `VehiclePositions` feed, in protobuf and JSON. | name the store, the policy engine or any crate but `antares-ops`; read around the view; write |
| `antares-jsonstat` | JSON-stat of a view `/x/views/{view}/jsonstat/v1/{table}` (role surface, feature `jsonstat`, generated by `dev/new-surface.py`): the `KeyPerformanceIndicator` Entities of one source table of a view as a JSON-stat 2.0 dataset. | name the store, the policy engine or any crate but `antares-ops`; read around the view; write |
| `odata-csdl` | OData 4.01 EDM model, CSDL XML and CSDL JSON writers, model validation under caller `Limits` (ADR-0024) | know its host (no `antares-*` dependency, rule M1) or read CSDL |
| `odata-url` | OData 4.01 relative URLs (resource path, system query options, `$filter`) to a typed AST under caller `Limits`, against a `Model` the caller supplies; canonical writer; the subset of T-3441, the rest `NotImplemented` (ADR-0024) | know its host (no `antares-*` dependency, rule M1) or decide a status code |
| `odata-json` | OData 4.01 JSON format: entity, collection, reference, service-document and error writers at `metadata=none\|minimal\|full` in 4.0 or 4.01 spelling; the body reader of the mapped writes (W1, W2, W4, W6, W10, W11, W16, W20) under caller `BodyLimits`; `Prefer` and version negotiation; the rest `NotImplemented` (ADR-0024) | know its host (no `antares-*` dependency, rule M1), decide a status beyond `BodyError::status`, or echo a request value in an error |
| `odata-batch` | OData 4.01 `$batch`: `multipart/mixed` (Protocol 11.7.7) and JSON (JSON Format 19) request parsers to one `Batch` under caller `Limits`, strict framing (CRLF only, no preamble, epilogue or folding), `dependsOn` as indices of earlier members, the response writers; change sets, `atomicityGroup`, Content-ID references and media bodies `NotImplemented` (ADR-0024) | know its host (no `antares-*` dependency, rule M1), drop or trust a member header (the surface keeps the OData ones, O9), or run a member |
| `odata-axum` | OData 4.01 over axum (ADR-0024 decision 1): `metadata_router` serves `GET /` (the service document, JSON) and `GET /$metadata` (CSDL XML, CSDL JSON on 4.01 when `$format` or `Accept` asks, 406 on 4.0; a strong `ETag` per serialisation, `Cache-Control: private, no-cache`, `If-None-Match` 304; a write method an OData 405 through `read_only`); the `ODataRequest` extractor parses a URL against the model and negotiates `OData-Version`/`OData-MaxVersion`; `url_error` maps a refused URL to its status (400, 404, 414, 501); `ODataErrorResponse` writes the error body; custom query options 400 (Q27); `table_router` serves any `odata-table` `Table` read-only (`GET /{set}` with a `$skiptoken` next link, `/{set}/$count`, `/{set}('key')`, `/{set}/$export`, `/{set}/$allowed`, the source's `ETag`; every call runs `allowed` first; the `Scope` a caller's middleware put in the request extensions, 403 without one; a source failure 500 with a fixed text); every limit from the caller's `ODataState` | know its host (no `antares-*` dependency, rule M1), echo a request value in an error, or apply a default limit |
| `odata-table` | OData 4.01 entity sets over any row source (T-6642): one `Service`/`EntityDef` definition per set (fields, operations, the permission each needs; no OData type in it) giving the CSDL model, the OpenAPI 3.1 document, the `TableInfo` and `allowed` (the decision a call runs, answerable without running it; a hidden entity answers as a missing one); the `Table` trait, `plan()` deciding once for every source what is answered (400 unknown property, 501 outside the subset: `$filter` comparisons, `and`/`or`/`not`, `in`, `contains`/`startswith`/`endswith`, null and nested complex paths; `$orderby` nulls first with the key as tiebreak; top-level `$select`; `$top`/`$skip`/`$count`/`$skiptoken`), a `Scope` applied before `$filter`, and three sources: `MemoryTable` (the reference evaluator), `PgTable` (feature `postgres`: every request value a bound parameter, nested names a bound `text[]`, on the caller's pool so its RLS holds) and `GitTable` (feature `git`: YAML/JSON files of a repository at one commit through `git ls-tree`/`cat-file`, the commit as `ETag`; `..` and symlinks refused) | know its host (no `antares-*` dependency, rule M1), splice a request value into SQL, read outside the repository's object store, or answer a property the CSDL model does not declare |
| `antares-export` | file encodings of an answer read page by page (T-4465, EP-119, role protocol): `parquet` (feature `parquet`) writes `GeoJSON` Features as Apache Parquet, a row group per page, ZSTD, the geometry as ISO WKB with GeoParquet 1.1 `geo` metadata, a column per schema property typed by `Kind::of_schema`; `csv` RFC 4180, `bundle` (feature `zip`) a ZIP of named files (T-4477), formula-leading text prefixed with `'`; refuses past the byte ceiling it is given (`Error::TooLarge`), never writes a cut file | know its host (no `antares-*` dependency, rule M1), read a request, or page an answer itself |
| `ogc-cql2` | OGC CQL2 1.0 filters, `cql2-text` and `cql2-json`, to one AST in the shape of the JSON encoding (`Expr::Op(Op, args)`), every conformance class, bounded by `MAX_LEN`, `MAX_DEPTH`, `MAX_NODES`; a `cql2-text` writer that parses back to the same tree (ADR-0029 decision 2) | know its host (no `antares-*` dependency, rule M1), evaluate a filter, or decide what a backend can push down |
| `antares-residual` | the residual of a standard query over a view (ADR-0028 rules 2 and 7, role ops, re-exported by `antares-ops`): `Expr` (Entity paths, typed literals, comparison, three-valued `and`/`or`/`not`, `IS NULL`, `IN`, `any`/`all` over a list value, the closed `Func` table: OData F14 and F16 to F19 string, date/time and math functions, CQL2 `CASEI`/`ACCENTI`/`LIKE`), `compile` bounded by `MAX_DEPTH` 64 and `MAX_NODES` 4096 with `now` folded and patterns built once (`pattern.rs`: an ECMAScript pattern translated to its `regex` twin, look-around, back-references and `\b` refused, programs capped by `MAX_PROGRAM_BYTES`), `eval` over a normalized Entity with missing, hidden and mistyped values unknown and checked arithmetic, bounded by `MAX_STEPS`; spatial relations (`geometry.rs`, T-3756: `Spatial` intersects to crosses by `geo`'s DE-9IM `relate`, a `Lit::Geometry` read once at `compile` as the NGSI-LD write path reads `GeoJSON`, a value that is no geometry unknown, at most `MAX_GEO_VERTICES` 1 024 positions) | depend on an `antares-*` crate, do I/O, or widen what the view returned |

## 3. Module map of `antares-api/src`

One module per NGSI-LD resource family; the clause range is in each
module's header comment.

| Module | Surface |
|---|---|
| `lib.rs` | module declarations and re-exports, `spawn`, `wire`, the 5.5.4 document fragment, shared helpers |
| `router.rs` | router assembly: the `/ngsi-ld/v1` route table (`api_routes`), `router`, `ops_router`, the request layers (tenant existence and echo, CORS, metrics, trailing slash, 405/501/OPTIONS, 404) |
| `ops.rs` | the `/q/*` operational surface (`Admin`): health, readiness, metrics, tenant list, size and purge, the dead-letter admin |
| `negotiate.rs` | 6.3.4–6.3.6: tenant, Accept, Content-Type, `@context` resolution, parameter allow-lists (the query set of 6.4.3.2 included) and the RFC 7230 header-syntax checks. Every handler passes through `tenant_from`, `check_params`, `parse_body`, `request_context` |
| `entities.rs` | 5.6.1–5.6.6, 5.7.1–5.7.2 `/entities`, distributed write fan-out |
| `paging.rs` | 4.12, 6.3.10 limit/offset/count and the next/prev links, 4.23 ordering and ICU collation, 6.3.17 `NGSILD-Warning`, the query body of 5.2.23 lifted into parameters; every list operation pages through it |
| `admin_auth.rs` | the optional `/q` admin credential (T-3569): one bearer secret from `ANTARES_ADMIN_TOKEN_FILE`, compared without an early exit; health and readiness stay open, metrics configurable |
| `attrs.rs` | 5.6.2–5.6.5 attribute operations, 10.2.6 Set Attribute Value |
| `channels.rs` | inbound bridge Channels (ADR-0031, feature `channels`): `/x/channels`, the loopback and MQTT bridges, bind and relay through the policy seam |
| `batch.rs` | 5.6.7–5.6.10, 5.6.20 `/entityOperations/*` |
| `temporal.rs` | 5.6.11–5.6.16, 5.7.3–5.7.4 `/temporal/entities` |
| `temporalq.rs` | 5.2.21 `TemporalQ` from the timerel/timeAt/endTimeAt/timeproperty parameters and the 4.11 instance match; named by temporal, csource, federation and subscriptions |
| `types_attrs.rs` | 5.7.5–5.7.10 `/types`, `/attributes` |
| `subscriptions.rs` | 5.8, 5.11 `/subscriptions`, `/csourceSubscriptions` |
| `notify/` | 5.8.6 notifications: `mod.rs` wiring, the change queue and temporal recording; `matching.rs` candidate evaluation; `payload.rs` the body; `throttle.rs` the drain, lanes and delivery slots; `interval.rs` periodic sweeps; `delivery.rs` one send (egress check, headers); `csource.rs` csource notifications; `retry.rs` retries and dead letters |
| `distsub.rs` | 5.8.1.4 distributed subscriptions, consumer half |
| `csource.rs` | 5.9, 5.10 registrations, `csource_index` maintenance |
| `registry.rs` | matching over a registration document: `CsrSpec`, the 4.3.6.1 information match, csf and scope filters, the temporal interval and expiry of a registration, the 5.11.2 subscription match; named by federation, notify, csource and every resource module that forwards |
| `federation.rs` | 4.3.6, 5.12, 6.3.17–6.3.19 forwarding, fan-out, result merge; `ForwardTransport` and `BuiltForward`, asked by every forward before the egress check (ADR-0025 rule 8), and by every notification attempt whether it delivers in process (rule 7) |
| `entity_maps.rs` | 5.14 the `/entityMaps` resources (6.32, 6.34, 6.35): create from a query, retrieve, update, delete |
| `entity_map.rs` | one EntityMap document (5.2.39): store it under its tenant with a lifetime, read it back while it lives, take a page's candidate ids from it, merge what a distributed query reached, and serve a retrieve through a presented map |
| `snapshots.rs` | 5.16 snapshots under synthetic `snap-…` tenants |
| `tenant_move.rs` | ADR-0030 tenant digest, export and import on `/q/tenants/{tenant}/…`: what a platform needs to move one tenant to another broker with its system attributes and history, and to prove the copy from both sides |
| `contexts.rs` | 5.13 `/jsonldContexts` |
| `conformance.rs` | 6.3.21 version negotiation |
| `repr.rs` | 6.3.7, 4.5.4 representations: normalized, concise, keyValues, sysAttrs; 4.5.23 Linked Entity Retrieval (inline and flat, the lookup budget) and the 4.5.16 GeoJSON Feature/FeatureCollection shapes — what every module needs to render a document |
| `history.rs` | the producer side of temporal recording: the per-request change buffer and the 4.5.7/4.5.8 delete mirrors that record a deleted Entity or Attribute in history |
| `provenance.rs` | who and what wrote a request's history (platform ADR-N-054 2.3): the gateway's `JC-Provenance-*`, `JC-Reason` and `X-Request-Id` headers, believed only from the mesh identity `ANTARES_PROVENANCE_FROM` names in `l5d-client-id`; otherwise `channel: api` with no actor |
| `erasure.rs` | erasure of personal data (ADR-N-054 2.6) from the current state, every history version and the dead letters; `antares-ops` `views/erasure.rs` serves `/q/erasures/{tenant}`, keeps the content-free tombstone and re-applies it on start |
| `stamp.rs` | 4.8, 5.2.4 system attributes of a write: `createdAt`/`modifiedAt` on the entity and on every attribute instance |
| `policy/metadata.rs` | the metadata members a read's engine passes per Attribute (platform ADR-N-066 2.4): `Metadata` on a `Filter`, held by the gate for the layer that refuses a `q`/`orderBy`/`timeproperty` on a stripped member (400) and strips the answer once, whichever surface asked; `strip_entity` for a notification |
| `mirror.rs` | the change event and the two document mirrors (`Change`, `DocMirror`, `SubMirror`, `TenantIndex`) that state, notify and history share, with the registration index (`reg_keys`, `bucketed`, `matching`) the federation read path selects candidates through; names no other module |
| `bounds.rs` | every cap: body, URI, JSON depth, batch, fan-out, in-flight, regex program size, and the notification pipeline's queue depth and delivery widths; all reported by `/q/health`, which a test over this module's own source keeps true |
| `egress.rs` | SSRF wall and per-destination circuit breakers for notifications, forwards, `@context` fetches |
| `surface.rs` | `ApiSurface`: HTTP surfaces mounted beside the API root, on the reserved prefixes `/q` and `/x` |
| `policy.rs` | the policy seam (ADR-0020): `PolicyEngine`, `Subject`/`Operation`/`Decision`/`Filter` (per-rule allowlists, ADR-0023 rule 11), the `Condition` an `AllowIf` holds a write by id to inside the store (gap G12), the built-in `AllowAll` engine, the clauses a narrowing can reach and the refusal everywhere else, the fail-closed calls that deny on a panic or a timeout, and the stored subject a notification is decided under |
| `secret.rs` | `Secret`: a configured credential (the admin token, the database URL) whose `Debug` and `Display` print `[redacted]`; `expose()` is the one way to read it, and dev/security.cypher secret-exposed lists every caller |
| `state.rs` | `AppState`: store, bus flag, mirror, HTTP clients, delivery policy, sinks, surfaces, hooks, the forward transport, and the endpoint view a state is narrowed to (feature `views`), and the task-local @context a surface calls in (`call_in_context`) |
| `fuzzing.rs` | the request parsers the `fuzz/` targets drive, behind the `fuzzing` feature that no broker build enables (T-3463) |
| `views.rs` | feature `views` (ADR-0023, ADR-0027 section 5): `CompiledView` and the composition `gate!` applies after the engine answers, effective = view ∧ engine, neither side widening, rule pairs keeping both id narrowings (G2); `WriteGuard`, rule 7 checked inside the store's read-modify-write; the `ViewTable` a pod serves from (views and each Tenant's schema bytes with their revision) and the `Kind::View` documents under `antares-views`; never in a vanilla build |
| `schema_writes.rs` | feature `views` (T-5912): the hook behind `views::schema_admits`, which every NGSI-LD write route asks after its policy gate whether the expanded Entity or fragment keeps to its Tenant's `LinkML` schema; the check is a fn a schema-reading surface registers (`Ops::check_writes`, `linkml_model::check`), so a vanilla build links no protocol crate; `ANTARES_SCHEMA_WRITES` `enforce`/`report`/`off` |
Geoquery evaluation, the `q=` evaluator and the caches of compiled client
query text are not in that table because they are not this crate's: they are
`antares_ql::geo`, `::eval` and `::regex`, named by those paths at every call
site here, and the evaluation lives in the crate a gateway can use on its
own. That includes the 4.9 precedence between `expandValues` and `jsonKeys`
(`antares_ql::eval::expansion_list`): the entity query, the temporal query
and a Subscription's notification condition (Table 5.2.12-1) all carry the
pair, and the matcher cannot reach the broker crate, so the rule lives beside
the coercion it feeds.

## 4. A request, end to end

```
TCP accept (broker main.rs, TCP_NODELAY)
 → axum router (api router.rs)
 → negotiate: tenant header → TenantId, Accept/Content-Type, params allow-list,
   Link/@context → antares_jsonld::Loader (cache, pinned core context)
 → bounds: body size/depth, URI length (rejects are counted in /q/health)
 → handler <resource>.rs::<operation>_inner
     expand (jsonld) → validate (model) → store call (CurrentStateDriver)
     for distributed operations: federation.rs::forward under FED_INFLIGHT
 → respond: compact, representation (repr.rs), paging Link headers, status
```

Handlers await the store: every driver method is an `async fn`
(ADR-0022), object-safe through `#[async_trait::async_trait]`, so a caller
waiting on Postgres holds no thread and store parallelism is bounded by the
connection pool rather than by an OS-thread ceiling. The pool (`ANTARES_PG_POOL`, 20) answers
503 with `Retry-After` after its 5 s acquire timeout — the driver marks a
`PoolTimedOut` with `antares_model::error::DB_OVERLOADED` and
`negotiate::ApiError::Overloaded` renders it — and every transaction opens
through `store/pg/mod.rs::begin`, which times the wait into
`antares_pg_transaction_begin_seconds`; see §8.

## 5. A change, end to end

```
store write commits
 → ChangeHook (set at wiring; api role only)
     history.rs: temporal auto-recording, synchronous, same request
     buffer_change → per-request change buffer → change_flush at response
 → notify/mod.rs::wire: bounded mpsc (CHANGE_QUEUE); a full queue drops and
   counts (antares_notification_changes_dropped_total, warn per thousand)
 → drain task: ONE task, batch → process_changes, the next batch taken
   only once this one's deliveries have all settled (which is what keeps
   per-subscription order); the changes of a batch are matched up to
   MATCH_WIDTH at once (notify/throttle.rs: the cores, at most 8, T-6371)
   and leave in change order
     match against SubMirror (in memory, per tenant; store list is the
     never-wired fallback), candidates handed over as shared documents and
     each `@context` resolved once per drain and per tenant → group per
     subscription → JoinSet under
     DELIVERY_SLOTS (ANTARES_DELIVERY_WIDTH, default 64)
 → deliver_as: one record_delivery per attempt (timesSent,
   lastNotification, lastSuccess, status), mirror updated from the document
   it returns,
   egress check + breaker for a binding that opens a socket, the
   NotificationSink the registry holds for the endpoint's scheme (never a
   fall-through, ADR-0016), retries as transport (ADR-0015), dead letter on
   exhaustion
```

With `ANTARES_BUS=nats` (`broker/src/wiring.rs`): the api role writes an
outbox row in the same transaction, a drain publishes to the
`ANTARES_CHANGES` stream with `Nats-Msg-Id` = outbox seq; matcher pods
consume, subscriptions travel through a KV bucket to every mirror;
interval firings are claimed through the store so one pod fires. A message
over the 256 KB ceiling carries claim-check references instead of the
bodies: the drain keeps that outbox row (stamped `published_at`, out of
later pages) and the matcher reads the whole event back by its seq, because
the store's current row is the after-image and holds no before-image to
diff against. The maintenance pass frees stamped rows 24 hours on.

## 6. Storage

`ANTARES_STORE=memory|file|postgres|timescale` (ADR-0004), one trait
pair, one schema (`crates/antares-sql/migrations/`: `0001_init.sql`,
`0002_dead_letters.sql`, `0003_comma_seconds_fraction.sql`,
`0004_drop_entity_maps.sql`, `0005_service_escape_by_command.sql`,
`0006_context_tenant.sql`, `0007_outbox_claim_check.sql`, `0008_views.sql`).

- Tenancy: one shared schema, `tenant_id` on every row, Row-Level
  Security with `FORCE` on every tenant table (ADR-0001). RLS is a belt
  only when the role is not a superuser: `ANTARES_REQUIRE_RLS=1` refuses
  to boot otherwise, and the shipped manifests set it. The one hole in
  the belt is the transaction-scoped `antares.service` GUC, which the
  outbox drain and the two 4.22 reaps arm to work across tenants; it
  opens `SELECT` and `DELETE` on `entities`, `outbox` and (plain mode)
  `attr_instances`, and nothing else — the setting is not privileged, so
  the commands it reaches are the whole of its blast radius.
- Documents other than entities (subscriptions, registrations, entity
  maps, snapshots, dist-sub mappings, dead letters) are `Kind`-tagged
  JSON rows with the same trait surface (ADR-0012).
- Queries: `antares-sql/compile/` turns the AST into SQL that NARROWS;
  the in-memory evaluator (`antares-ql::eval`) is the arbiter
  (`qprefilter.rs`). A compiled predicate may be partial; it may never
  drop a candidate.
- Temporal: `attr_instances`, range-partitioned by `observed_at`
  (hypertable under timescale, no RLS there by columnstore constraint,
  ADR-0006); recording is synchronous in the write path (ADR-0007).
- Registrations: candidate matching goes through `csource_index`, never
  a scan of a tenant's registrations. Its rows are a pure function of the
  registration document, rebuilt inside the transaction that writes it and
  only when that write moved a member they are built from: the Table
  5.2.9-2 forward counters move on every distributed operation and are not
  among them.

## 7. Invariants (break one and the suite tells you)

1. Spec names: Rust types and function names come from CIM 009 (`Entity`,
   `create_entity` = 5.6.1). Error variants = Table 6.3.2-1.
2. Doc comments on normative code cite the clause number and the rule.
3. Everything inbound is bounded (`bounds.rs`); everything outbound goes
   through `antares_jsonld::client_builder` and `egress.rs`.
4. No dynamic SQL outside the compiler (CI gate); `unsafe` forbidden;
   `unwrap`/`expect` denied outside tests.
5. Every env variable is in `KNOWN_KEYS` (`broker/src/main.rs`), or, for
   a surface, in its crate's `ENV_KEYS`, which `SURFACE_KEYS` names, and
   in `docs/src/configuration.md` (`dev/check-env-docs.sh`). Every version is
   in `[workspace.dependencies]`; the `wasm32` target sections, which
   cannot say `workspace = true` because cargo features are additive, are
   held equal to it by `dev/check-wasm-pins.py`.
6. `/q/health` lists every cap; a new cap is added to the health snapshot
   and its test (`bounds::tests`).
7. A store returns `NgsiError`, never a backend error; a handler never
   formats SQL.
8. Tests that wait for a delivery scale their wait by
   `antares_api::state::slow_factor()` (sanitizer builds run ten times
   slower).
9. The ETSI Robot suite is the oracle, never the requirements source;
   the clause text in `docs/spec/` is.

## 8. Known structural debts

Stated so they are not rediscovered. Each is measured, not guessed.

- Every trait method allocates: `#[async_trait::async_trait]` boxes one
  future per store call (ADR-0022), which is what makes `Arc<dyn
  CurrentStateDriver>` possible at all. Native `async fn` in a trait is
  object-safe only behind a nightly feature; when it lands on stable the
  attribute comes off and the allocation with it, without touching a call
  site.
- Query evaluation is synchronous and the store is not, so the 4.9
  linked-entity terms resolve by cache-and-retry (`notify::linked_eval`):
  the evaluator runs against a cache, the URIs it missed are fetched
  together, and it runs again. A link chain of depth *n* costs *n* passes
  over the expression. The alternative is an async evaluator, which would
  put a store on `antares-ql` and `antares-matcher`.
- `antares-api` is acyclic: no module reaches itself through
  `crate::<module>` references, so the largest strongly connected
  component of its 31 modules is one. What two resources share lives in
  a leaf that names no resource module: the change event and the two
  document mirrors in `mirror.rs`, paging, ordering and the query-body
  lift in `paging.rs`, the request's temporal query in `temporalq.rs`,
  registration matching in `registry.rs`, the system attributes of a
  write in `stamp.rs`, one EntityMap document in `entity_map.rs`, and
  4.5.23 Linked Entity Retrieval with the 4.5.16 GeoJSON shapes in
  `repr.rs`. `dt_key`, `check_attr_name`, `scope_matches`,
  `type_selection_matches` and `redact_userinfo` are named from the
  crates that own them. The one call that pointed back up the pipeline —
  the delivery path handing a notification on the internal endpoint to
  `distsub` — is a seam: `AppState::csource_notification`, installed by
  `antares_api::install_csource_notification`, absent and therefore dropped
  on a state nobody wired. Every root installs it, and the broker does so
  above its bus branch rather than inside one: `antares_api::wire` carries
  the in-process matcher, the role-split fleet wires its matcher through
  `wiring::wire_nats`, and a handler installed in only one of them leaves
  the other accepting distributed Subscriptions that forward no copy.
  `cargo modules` does not show such a cycle because it records call edges
  and not field types, so the source-level count is the one to measure: `dev/module-graph.py` prints
  it per crate and `xray.yml` ratchets the largest component of every
  crate against `dev/module-baseline.json` (`antares-jsonld` holds a
  second one, `context` ↔ `loader`, of two). Three numbers in it are
  measured, not chosen (T-3602): the gate had never run on this fork, and
  `antares-api` already held a component of 7 when the baseline said 1; it
  holds 22 now (`admin_auth` to `views`, the handlers reaching each other
  through `state` and back), recorded so that it can only shrink. The 22nd
  is `router` (T-5810): T-4773 moved the router out of `lib.rs`, which the
  count never sees, and `AppState::call` serves an in-process call through
  that router, so the cycle it closes was there before the move.
  `antares-ogc` reads a collection through `features` (current state in
  `items`, `{type}-history` in `history`, which builds on `items`), and the
  `channels` write intent and `notify::endpoint_uri` live with the code
  that honours them, so neither joins the component.
  `odata-csdl` holds 3 (`error`, `model`, `validate`) through two doc
  links alone, `model` and `error` naming `validate` for the reader; its
  code edges are acyclic.
- `antares-api` names no storage backend in a normal build: `antares-sql`
  is an optional dependency behind the dev-only `test-kit` and `postgres`
  features (`AppState::new` over the built-in store, the Postgres-backed
  integration tests), the roots compose their drivers with
  `AppState::with_drivers`, and the temporal query's paging decision asks
  `TemporalDriver::q_pushdown_exact`, which only the Postgres arm answers
  from its prefilter. `workspace.yml` checks the dependency tree.
- `csource.rs` serialises registration writes per tenant on process-local
  `tokio::sync::Mutex`es (`REGISTRATION_WRITE`), so the 5.9.2.4 overlap
  check of an idPattern-only registration, which walks up to
  `MAX_UNDECIDED_ROWS` under it, stalls only its own tenant. Two broker
  pods do not exclude each other: a database-level lock held across the
  check-then-write section is the remaining step.
- Every distributed write takes its registrations and the `Via` loop
  answer from `federation::write_plan` (`WritePlan::Forward` or
  `Answered`); the read-path prologue (csf, the two-binding rule) is
  still spelled per query path.
- Oversized functions carrying one clause's whole validation matrix:
  `purge_inner`, `batch_write`, `query_temporal_inner`,
  `normalize_subscription`, `expand_instance`, `normalize_registration`,
  `deliver_as`. Split by member on touch.
- Misplaced helpers: the error constructor `bad` lives in
  `snapshots.rs` and is used crate-wide.
- The pipeline is bounded by matches, not by delivery concurrency. One
  match is one (subscription, entity) pair. `dev/perf/deliver.py` holds
  above 30 000 matches/s on twelve cores at 2.7 cores of broker with
  nothing dropped, and the harness runs out of load before the broker
  does — its writers are threads in one interpreter. Delivery width does
  not govern it either: over a hundred-tenant shape, widths of 8 to 1 024
  deliver the same 7 027 matches/s and drop nothing, so the knob has
  nothing to arbitrate while the offered load is absorbed whole. This run
  predates T-6371: matching was then sequential on the single drain task,
  and is now up to MATCH_WIDTH changes at once on that task's batch.
- Goodput goes backwards past the write knee, and nothing sheds load
  before it. Against PostgreSQL the same ladder that delivers 1 671
  changes per second when offered 2 000 delivers 1 140 when offered
  4 000, with broker CPU falling from 1.39 cores to 1.26 as it does: the
  writers keep arriving, the database is past what it can commit, and the
  broker is starved rather than busy. The bounded matcher queue protects
  the notification path, and nothing plays that role for writes. This
  sits under the delivery path rather than in it — no change to matching
  or delivery moves it. The rented rig shows the same shape from the
  other side: at 500 rps offered, where the shape asks for about 8 900
  notifications a second, the pipeline delivers 561 a second and sheds
  26 398 changes, against the 3 251 a second the same build sustains at
  200 rps. Past the queue's capacity the system delivers an order of
  magnitude less than its own sustained rate rather than settling at it.
- Per-notification cost is one row update. The Postgres arm writes it as
  a single statement (`pg::doc::record_delivery`), so the row lock is held
  for the statement rather than across a round trip and the Rust closure —
  at fan-out that hold time is what serializes delivery on a hot
  subscription. Batching several attempts into one statement was recorded
  here as the next lever; it is not, on the evidence above — it shortens a
  per-delivery cost that sits downstream of the ceiling. It stays unbuilt,
  and `set_config` being the RLS context still makes any such batch
  per tenant by construction.
- Registration candidate selection reads on the order of a thousand
  rows per federated query on a 10 000-registration registry; the
  `csource_index` needs a type-first shape.
- Duplication has a measured floor rather than a target of zero.
  `dev/dup-check.sh` ratchets the workspace at 79 token clones over 958
  lines (`dev/dup-baseline.json`), 727 of them in `antares-api`, at 16
  groups of functions sharing one signature and at 6 names defined in more
  than one crate (`from_env`, `from_params`, `of`, `parse`, `parse_q`,
  `serve`: same word, different job). The floor was 68 over 805 in
  September; `xray.yml` had no run on this fork since, and once
  `odata-csdl` landed nightly rustdoc refused its ambiguous `validate`
  links, so the gate stopped before it measured. T-3602 fixed the links
  and recorded the numbers as found; from here they can only shrink. What the ratchet still allows:
  - 35 lines are `use` blocks in files serving neighbouring clauses. A
    `use` list is not logic, and a shared prelude would hide what each
    module depends on.
  - 51 lines are the Entity Type List and Attribute List discovery
    operations (5.7.5, 5.7.6). That parallel is the specification's: the
    two build different representations from different tables
    (5.2.24/5.2.25 against 5.2.27/5.2.28), so one generic over both would
    take more parameters than it removes and would let a change to one
    table reshape the other.
  - The largest remaining share is per-operation handler heads and tails,
    nine to fifteen lines each. What those share is already extracted --
    `tenant_from`, `check_params`, `request_context`,
    `ParsedBody::object`, `attach_paging`, `entity_maps::retrieve_with_map`
    -- and what remains is the part that differs: the parameter
    vocabulary, the error type of Table 6.3.2-1, the forwarded operation
    name. Fifty `check_params` call sites carry twenty-one distinct
    allowlists, and only two share the eleven-name query prefix; one
    allowlist for operations whose allowlists deliberately differ would
    widen what each endpoint accepts.
  - Four signature groups are coincidences of shape rather than shared
    logic: `check_ring`, `check_position` and `check_vertex_budget` all
    take a `&Value` and return `Result<(), String>` while validating three
    unrelated rules (RFC 7946 ring closure, WGS84 range, a vertex budget),
    `temporal_key` and `pattern_regex` are both
    `(&str) -> Option<String>` over different vocabularies,
    `TenantId::new_internal` and `MqttEndpoint::parse` are both
    `(&str) -> Result<Self, NgsiError>` because each is how its type is
    built from text (the NGSILD-Tenant grammar, the 7.2 endpoint URI), and
    `SubMirror::apply` and `DocMirror::apply` are the two implementations
    of `Mirror::apply`, so that signature is the trait's rather than a
    repetition: one keeps an inverted candidate index in step with the
    document it stores, the other only stores the document. Merging any of
    them would name one function after two rules.
  - One signature group is the reference plugin's, and stays: because
    `examples/plugin-example` implements the same two driver traits as the
    in-binary memory store, its `live`, `rows` and `emit` necessarily carry
    the memory store's signatures. That is the seam being demonstrated, so
    the ratchet holds it at fourteen rather than thirteen.
  - Four clusters are real debt, each reading one rule more than once:
    the temporal writes (80 lines inside `temporal.rs`, `add_temporal_attrs`
    against `upsert_temporal` and `delete_temporal_instance` against
    `delete_temporal_attr`), the federation entry points (72 lines,
    `fed_query`/`fed_query_temporal`, `fed_retrieve`/`fed_retrieve_temporal`
    and `import_entity`/`import_temporal`) and the snapshot document
    handlers (24 lines across `retrieve_snapshot`, `update_snapshot` and
    `clone_snapshot`) and the two 4.22 collectors (17 lines,
    `sweep_expired_registrations` against `sweep_expired_maps`: the same
    walk, collect and delete over a different `Kind` and liveness
    predicate).
- Re-measured 2026-10-12 (T-4020) at 94 clones over 1156 lines, 28
  signature groups and 43 names in more than one crate. Nightly rustdoc
  refused three ambiguous `eval` links in `antares-residual` from 10-11 on
  (T-3984 fixed them), so the gate again stopped before it measured while
  the surface crates landed (`antares-ogc`, `ogc-cql2`, `antares-residual`,
  `antares-icsr`, `antares-ws`, `antares-mcp` growth). What was one rule
  written twice is merged: percent-coding is `antares_ops::percent` for
  OData, OGC and MCP; the every-slot map a `*` view shows is
  `linkml_model::Model::slot_iris_by_class`; `policy::Condition` is
  `repr::PolicyReach`, the one selection a narrowing makes; OData's two
  dispatchers moved into `route`, which also broke the module cycle
  `read`/`write`/`below`/`paths` (and `antares-ws` keeps
  `MAX_NOTIFICATION_BYTES` in `limits`, breaking `hub`/`limits`). What the
  new floor holds beyond 10-10:
  - 52 lines are benchmark fixtures (`benches/` of `antares-mcp`,
    `antares-ops`, `antares-linkml`, `antares-icsr`, `antares-ws`): each
    bench builds its own broker state, and a shared bench crate would be a
    workspace member for setup code alone.
  - 24 lines are `linkml_model::RawSchema` mirroring `Schema`: serde's
    `try_from` reads the raw form with `deny_unknown_fields` and the two
    declarations the subset implies, then checks them; the mirror is the
    price of refusing unknown members.
  - The rest are the surface crates' own entry points and token codecs:
    `serve`, `respond`, `routes`, `problem`, `read_token`, `digest` and
    `parse` exist once per protocol (OData, OGC API Features, MCP, the
    WebSocket and view roots) because each answers its own wire format.
    The paging tokens differ in what they bind (OData's remaining `$top`,
    OGC's part index, MCP's tool and arguments), so one codec would carry
    every protocol's fields. Same-name and same-signature groups of this
    kind are the per-crate vocabulary, not repetition; MCP's `encode`
    keeps `:` unescaped on purpose and stays its own.
- Re-measured again 2026-10-12 with dev's retention and residual-scan work
  merged (T-4020): clones stay at 94 over 1156 lines. Two new clones were
  merged. Reading a stored schema or retention document is
  `views::stored_json`, and the memory store's expiry prune and retention
  sweep share `attribute_names`. Names in more than one crate rose from 43
  to 48 by five words that each crate uses for its own job: `params` (OGC's
  items query against the scan's pushdown string), `refusal` and `refused`
  (an OData filter error, a view's 405/422/404, a retention document's
  refusal), `retain` (the retention run, the outbox drain, one bounded
  retention statement) and `selection` (temporal `attrs`/`pick` against
  OData's entity selection). No two of them take or return the same thing.
- Re-measured 2026-10-19 (T-4722) with dev's MCP summary, iCal, STA, OData
  `$apply`, quality and erasure work merged: the gate stood at 110 clones
  over 1339 lines and 59 names. Merged: a view document is stored, deleted
  and keyed by tenant through `views::store`, `remove_stored` and
  `tenant_key`, and quality and retention documents share one
  `views::TextDoc` route; iCal and STA answer `OPTIONS`, refused methods
  and `HEAD` through `antares_ops::read_only`; OData's read and `$apply`
  build their page links with `read::page_link`; `AnyStore`'s per-tenant
  statements open one `tenant_tx`; and `PgDocStore::list` and
  `list_digests` read through one bounded `every_row`. That leaves 100
  clones over 1222 lines. What stays above the 10-12 floor:
  - 20 lines are `antares-mcp`'s argument tables (`QUERY`, `BATCH_QUERY`,
    the three temporal tools): each tool lists its own arguments with its
    own required set, and stable Rust cannot concatenate const slices, so
    a shared table would be a macro that hides the list a reviewer checks.
  - 20 lines are two `examples/suite_*` runners pairwise (`antares-model`
    against `antares-residual`, `antares-jsonld` against `antares-ql`):
    each example is a crate's own self-contained conformance runner.
  - The rest are 9 to 13 line blocks inside one handler each in
    `antares-api` (`attrs`, `temporal`, `federation`, `tenant_move`), two
    test tables in `odata-url`, the surface `router` boilerplate of OGC and
    STA, and the argument check that opens MCP's `call` and `summarize`.
  Names in more than one crate rose from 48 to 59: six left the list and
  seventeen joined it, again one word per crate's own job:
  `admit`, `check`, `collection`, `erase`, `feature`, `fit`, `fold`,
  `members`, `negotiate`, `one`, `profile`, `scanned`, `steps`, `text`,
  `visible` and `window` take and return different things in each crate,
  and `mirrored` flips OData's `CompareOp` and CQL2's `Cmp`, two enums of
  two grammars.
- Re-measured 2026-10-26 (T-5810) with dev's feed, GBFS, GTFS-RT, JSON-stat,
  GeoJSON, OGC history and T-5787 discovery work merged: xray run
  40367890016 stood at 107 clones over 1348 lines, 42 signature groups, 99
  names and one struct twice. Merged: the feed surfaces' refusal is
  `antares_ops::Problem` (iCal and Atom/JSON Feed had one enum each, GBFS,
  GTFS-RT, JSON-stat and GeoJSON one `problem` body each); the NGSI-LD name
  and plain-text checks are `antares_ops::outputs::is_name` and `is_plain`;
  the feed output's `Filter` is `antares_ops::outputs::Filter` (the feed
  keeps its own `fixes_area`, where a `geoproperty` alone fixes no area);
  the MQTT client options of both protocol versions come from one
  `options!`; and a discovery walk is opened by `Walk::of`. That leaves 100
  clones over 1218 lines and no struct twice. Signature groups stand at 41
  and names at 98: each new surface crate brings its own `serve`, `page`,
  `asked`, `plan`, `preset`, `output`, `preview` and `render`, and the
  shared shapes (`(&str) -> String`, `(&Value) -> Option<String>`) join
  escapes and codecs of different grammars. Feed's `instance`, `value_of`
  and `at` read fewer attribute kinds than iCal's (no `vocab`, lists or
  `json`): merging them changes what a feed shows, so they stay two until
  that change is its own task.
- Re-measured 2026-11-02 (T-6497) after a week of merges: xray run
  41522025780 stood at 102 clones over 1245 lines, 47 signature groups, 114
  names and a largest `antares-api` cycle of 28. Merged:
  `antares_ops::outputs::is_uri` (SensorThings and its MQTT bridge held one
  copy each) and `antares_ops::ngsi_problem` (the `/grants` and
  decided-question routes), which leaves 100 clones over 1217 lines. Cut: the
  cycle is back at 22. `metadata` is `policy::metadata`, since it is a member
  of the policy `Filter`; `schema_admits` lives in `views`, so `schema_writes`
  holds only the hook and every write route stays out of it; the request's
  provenance task-local lives in `provenance`, so `stamp` reads no `history`;
  and a `notify` test reads a Subscription through the route, which took
  `subscriptions`, `distsub` and `contexts` out. Signature groups stand at 46
  and names at 113, recorded as measured. Of the 16 names left, SensorThings'
  residual engine (T-5918) brought `order`, `rank`, `sort`, `navigation`,
  `references` and `expanded` beside OData's. `sort` and `rank` were read side
  by side with OData's `sort` and `compare` and rank values of two kinds
  differently (JSON text, kind order), and `model_refusal` passes a stripped
  metadata member on OData and not on OGC, so those stay two. The rest, such
  as `connect` (NATS, MQTT) and `publish` (notifier, WebSocket bridge), sit
  in crates of different jobs.
  `dev/preflight.sh` runs `dev/module-graph.py` since this fix, so a cycle
  stops at the branch; the duplication gate needs a workspace rustdoc and
  stays weekly.

## 9. To change X, touch Y

| Change | Touch | Also |
|---|---|---|
| a new query parameter | `negotiate.rs` allow-list, the handler, `antares-ql` if it has grammar | a `docs/spec/` ledger entry, a Robot TP |
| a new NGSI-LD resource | `<resource>.rs` + route in `router.rs` | `Kind` in `antares-store` if it is stored |
| an error status | `antares-model` `NgsiError` | Table 6.3.2-1 in the doc comment |
| a cap | `bounds.rs` + env key + `docs/src/configuration.md` | `/q/health` test |
| a store backend | implement both driver traits in a new crate | a name on `store_shelf()` behind its feature (`examples/plugin-example` is the worked one); a built-in also takes a `StoreMode` value, an `AnyStore` arm and a CI cell |
| a notification transport | `NotificationSink` impl (own crate, or `antares-notifier`) | `SinkRegistry::register` at the composition root; `network()` false only if it opens no socket |
| outbound HTTP anywhere | `antares_jsonld::client_builder` only | egress policy and breaker; the DNS pin and the redirect cap live there and nowhere else, and `workspace.yml` fails on a client built any other way. reqwest is built provider-less, so a client built by another spelling panics rather than merely escaping the policy; `main` installs the provider before the OTLP exporter builds one of its own |
| a schema change | a new numbered migration; never edit an applied one | RLS policy + `FORCE` on the table |
| federation behaviour | `federation.rs` (`forward` is the one outbound chokepoint) | 4.3.6 narrowing is spec-mandated; keep it |
| the bus or roles | `broker/src/wiring.rs` | ADR-0002 |
| a role's HTTP surface | `router.rs` router construction by `roles` | a worker must 404 the API |
| another standard's API beside NGSI-LD (SensorThings, OGC API, WFS) | an `ApiSurface` under `/x/…` in its own crate that calls `AppState::call` — the in-process handle over this broker's own router, built once per state on first use | never under `/ngsi-ld/`; every façade request is an NGSI-LD request, so negotiation, bounds and tenancy are not repeated. The handle carries `NGSILD-Tenant`, `NGSILD-Snapshot`, `Link` and the policy subject headers from the caller, so a façade cannot be the way around any of them |
| an authorization decision | no engine in the broker: `policy.rs` is the seam one attaches to, and the gateway keeps authentication and rate limiting (the broker's one credential of its own is the optional `/q` admin secret, `admin_auth.rs`) (`docs/src/shared-crates.md`, "The PEP boundary") | conformance is asserted against the built-in allow-all engine, the way `surface.rs` refuses a surface under the API root |
| an NGSI-LD operation | one `policy::gate` call where the request enters its handler, naming the clause | exactly once per request: the shared query engines below a handler (`query_entities_inner`, `query_temporal_inner`) never gate, and `tests/policy_gate_seam.rs` walks every route of `router()` to prove both halves |
| a broker-held record of one query that outlives its request (an EntityMap, a Snapshot, a Subscription) | the subject it was made for, stored in `__subject` and read back through `policy::belongs_to` | the record is reused later, by a request the seam decides separately: a map's `entityMap` member is the id set a narrowing withholds, a snapshot's fill runs after its request is answered, and a notification has no request at all. The `__` prefix is the broker's — `policy::strip_internal` is the one serve-side filter, so a member added later is hidden by construction |
| a narrowing policy decision | the `Filter` the gate returned, applied where the request's own query and representation are built: `Filter::narrow_params` before the store is asked, `repr::narrow_projection` before the document is rendered | a served document is never re-walked to strip a member — the request's own 5.5.2 `pick`/`omit` machinery carries the policy's, so the two projections cannot disagree, and the `q` is conjoined on the `antares-ql` AST so the store push-down, the 5.7.2.4 re-check and the forwarded query read one query. `Filter::project` is the reference semantics the addon contract holds an engine against, not the served path |

## 10. Verification ladder

Unit (`cargo test -p <crate> <filter> -j 2`) → clause Robot TPs against
one local memory broker (`ANTARES_HTTP_PORT=9377 target/debug/antares`,
`ngsi-ld-test-suite/`) → `ci.yml` (dispatch: workspace tests, clippy
wall, quick ETSI matrix) → `full.yml` (seven store cells, twice weekly)
→ `strict.yml` (TSan, Miri, fuzz replay, coverage floor) →
`perf-weekly.yml` / `scale-weekly.yml` (rented hardware, `dev/perf/`).
`dev/code-xray.sh` writes module and call graphs under `results/x-ray/`.

## 11. Reading order for a newcomer

`docs/src/introduction.md` → this file → `docs/adr/` (0001, 0002, 0004,
0005, 0013, 0015) → `crates/antares-api/src/negotiate.rs` →
`entities.rs::create_entity` → `notify/mod.rs::wire` →
`crates/antares-sql/src/store/pg/entity.rs` → `broker/src/wiring.rs`.
