# Endpoint views

A `views` build (`cargo build -p antares-broker --features views`) serves
each loaded view as an NGSI-LD API root of its own:

```text
/x/views/{view}/ngsi-ld/v1/...
```

Mount it with `ANTARES_API_SURFACES=admin,views` and load views through
[`/q/views`](admin-api.md#get-qviews). Until the first view loads, every
path under `/x/views` answers `404` and `/q/health` reports the surface
`closed` (ADR-0027 rule M4, ADR-0023: "no view means nothing served"); a
vanilla build has no `/x/views` at all.

## 1. What a view serves

Every request below the view root runs as the broker's own operation
(`antares-ops`), narrowed by the view (ADR-0023) and in the view's one
Tenant:

| Clause | Operation |
|---|---|
| 10.4.2, 10.4.3, batch query | Retrieve and Query Entities |
| 10.4.4 | Retrieve Attribute Value |
| 10.6.3 to 10.6.8 | Entity types and Attributes |
| 11.3.2, 11.3.3 | Temporal Retrieve and Query |
| 13.3 | List @contexts: the core context only; a view that shows every name lists the Tenant's stored @contexts as the root does |
| 10.2.2 to 10.2.11, but 10.2.12 Purge | Create, Update, Append, Partial Update, Set Value, Delete Attribute, Delete, Merge, Replace Entity, Replace Attribute |
| 10.3.2 to 10.3.6 | Batch create, upsert, update, merge and delete, member by member |
| 10.5.2 to 10.5.6 | Create, Update, Retrieve, Query and Delete Subscription |

A view serves an operation only when its `operations` list names it.
Purge and the temporal writes are not served through a view. A view of
another model serves the reads only (section 5a). A Batch
Upsert member that exists out of the view is never overwritten: it takes
the create half and answers 409, as a create of a taken id does.

A view may select every type and show every Attribute with one form:
`"selector": ["*"]` and the single rule `{"types": ["*"], "attributes":
["*"]}` (ADR-0023 rule 12). Its `q`, `scopeQ`, `geoQ`, `operations` and
Tenant still apply. `*` anywhere else is refused when the view loads.

A view may carry `representations`, an object keyed by a format: `ical`
([iCalendar](ical.md)), `feed` (Atom, RSS and JSON Feed), `ogc`
([OGC API - Features](ogc.md)), `geojson` and `csv` ([GeoJSON](geojson.md)),
`sta` ([SensorThings](sta.md)), `odata` ([OData](odata.md)) and `gbfs`
([GBFS](gbfs.md)), whose outputs carry the GBFS system fields that page
lists instead. Each other member is a list of named outputs (docs API/11 EP-159, T-5216): `name`,
`title` and `description` per language, `version`, `sources` (model classes
with their role `mapping`), a `filter` (`q`, `scopeQ`, `georel`, `geometry`,
`coordinates`, `geoproperty`) that only narrows the view, `lang` and `limit`.
Any other key, or a member that is not a list, is refused when the view loads.
A list of more than 20 outputs or with a name twice loads and serves nobody
(the gateway refuses it when the Endpoint is saved); an output whose members
do not hold serves nobody while the other outputs of the list still serve. The
shared contract is `antares_ops::outputs`; the surface reads its own member.

## 2. What a client sees

- An Entity is in the view when a rule names its type and the view's `q`,
  `scopeQ` and areas hold for its current state; the history of an Entity
  out of the view now is not served (rule 8).
- Hidden Attributes are absent from every answer, and a `q`, `orderBy` or
  `pick` on one matches as if it were absent: the answer is the same
  whatever the hidden values are.
- `NGSILD-Tenant` may be omitted or name the view's Tenant; any other value
  is `400`. The answer echoes the header only when the client sent it.
- `Link`, `Location` and `Content-Location` name the view root, so a next
  page stays inside the view.

## 3. Writes

A write through a view runs on the local store only, never forwarded to a
Context Source, and the broker checks it on the stored Entity inside the
same store operation (ADR-0023 rule 7):

- The Entity must be in the view before the write. One out of the view
  answers `404`, byte for byte as an absent one, before any type selector
  or body check.
- Every Attribute the write names must be one a rule holding for the
  Entity shows, or the whole write is `403`. The answer is the same
  whether the Entity holds that Attribute or not. A Replace Entity also
  counts every Attribute the stored Entity holds, so a replace cannot
  delete what the view hides.
- The Entity must still be in the view after the write, or the write is
  `403` ("the write would move the Entity out of this view").
- A create must produce an Entity the view selects. An id taken out of
  the view is `409`, the same body an id taken in the view gets.
- A Relationship must point at an Entity the view shows. A hidden target
  and an absent one both answer `403`.
- A batch applies these checks to each member and answers the
  `BatchOperationResult`, `207` when mixed.

A refused write leaves the store as it was.

### Conditional writes

Retrieve Entity through a view answers an `ETag`: a strong entity tag of
what the view shows of the stored Entity (`id`, `type`, `scope` and the
Attributes a rule holding for it shows). A write to one Entity through the
view (Update, Append, Merge and Replace Entity, Delete Entity, and the
Attribute writes) honours `If-Match` (RFC 9110 13.1.1) inside the same store
operation as the checks above (T-3789):

- `If-Match: *` holds for an Entity the view shows; a list holds when one
  of its strong tags equals the Entity's. A weak tag never holds.
- When it does not hold the write is `412` with no body, and the store
  keeps what it held. An Entity out of the view and an absent one are
  both `412` under `If-Match`, so the answer still tells nothing about an
  Entity the view hides.
- A change to an Attribute the view hides leaves the tag as it was.
- A batch ignores `If-Match`: it names no single Entity.

The tag is a hash of the stored document (std `DefaultHasher`, 64 bits);
a broker upgrade may change every tag, which a client meets as a `412` and
a fresh read.

Every operation a surface runs, through a view or not, writes one `info`
line, `surface operation`, with `surface`, `view` and `tenant` (through a
view), `operation` (its matrix id) and `status`. The line names no subject
and no argument, so you can trace a write to the surface and view that made
it without the broker recording who asked.

## 3a. Subscriptions

A Subscription made through a view is stored as the client sent it, with
the view beside it, and the broker applies the view each time it matches
and notifies (ADR-0023 rule 9). Narrowing or removing the view takes
effect on the Subscriptions already made:

- Its conditions read only what the view shows of the Entity now, and an
  Entity the view does not select does not match.
- A change only to hidden Attributes sends nothing and leaves `timesSent`
  and `lastNotification` as they were.
- A notification carries the shown Attributes only; a hidden Attribute's
  `previousValue` goes with it.
- A removed view delivers nothing.
- `watchedAttributes` keeps only the Attributes the view shows, at create
  and at update; none left is `400`.
- Only the view a Subscription was made through reads, lists, updates or
  deletes it; another view answers `404`. The root sees every
  Subscription of its Tenant.
- A Subscription made through a view is local: the broker makes no
  distributed half for it (TS 104 175 5.8.1.4), so no Context Source
  receives a Subscription the view could not narrow.

## 4. Refusals

| Answer | When |
|---|---|
| `404` | unknown, broken or removed view id (all alike); a resource the view serves nothing on |
| `405` + `Allow` | the view serves other methods on this resource |
| `422` | the view serves this method here, but not this operation of it (`?details=true`, for example) |
| `400` | the request names an entity map (`entityMap`, `NGSILD-EntityMap`): maps are not served through a view |
| `412` | a write's `If-Match` does not hold for what the view shows (section 3) |

The view id is the raw path segment: `CARS`, `ca%72s` or `x/../cars` never
reach the view `cars`. No refusal names a hidden Attribute or an inner
failure; a broker error inside the view is a `500` with a request id only.

## 5. The Tenant's schema

A surface that needs the data model of a view's Tenant (OData's Edm types
and navigation properties, the LinkML surface) reads the Tenant's `LinkML`
schema, written to [`/q/schemas/{tenant}`](admin-api.md#schemas). Every view
of that Tenant sees the same schema and no view sees another Tenant's. The
surface gets the bytes and a revision through `ViewOps::schema`, parses
them under its own limits, and parses again only when the revision changes. A view
document carries no schema of its own: one with a `linkml` member is
refused with `400` like any unknown member, so the Tenant's schema is the
one source (T-3658).

## 5a. A view of another model

A view may serve its Entities in another data model than the one the
Tenant stores, through a `mapping` member: the mapping IR (version 2) that
Model Tools compiles from a Mapping (DM-52). Example: the Tenant stores
`MestskySenzor` with `teplota` in kelvin, and the view serves
`AirQualityObserved` with `temperature` in degrees Celsius. The design,
with every inversion rule, is [the mapping note](../analysis/view-mapping.md).

```json
{
  "id": "air", "tenant": "city", "selector": ["MestskySenzor"],
  "rules": [{"types": ["MestskySenzor"], "attributes": ["teplota", "location"]}],
  "operations": ["query_entities", "retrieve_entity"],
  "mapping": {"version": 2, "sourceClass": "MestskySenzor", "targetClass": "AirQualityObserved",
    "slots": [
      {"target": "temperature", "source": "teplota", "kind": "unitConversion",
       "factor": 1, "offset": -273.15, "filterable": true},
      {"target": "location", "source": "location", "kind": "rename", "filterable": true}]}
}
```

What changes for a client:

- The request is written in the target model: `type`, `attrs`, `q`,
  `orderBy`, `pick`, `omit` and `geoproperty` name target Attributes; the
  broker inverts them into the source model and rebuilds the answer.
- A geo-query without `geoproperty` filters the target model's `location`
  (4.10), read from the slot that serves it. When no filterable renamed slot
  serves `location`, the geo-query is refused.
- Only reads: Retrieve and Query Entities, Retrieve Attribute Value,
  discovery (10.6.3 to 10.6.8) and the two temporal reads. A write or a
  subscription operation in `operations` is refused when the view loads.
- The selector is the mapping's `sourceClass` and nothing else.
- Each attribute kind is translated per element: `value`, `object`,
  `vocab`, and each element of `valueList` and `objectList`. A
  `languageMap` or `json` value has no unit or value table: a slot that
  converts or translates it serves nothing for that Entity, and Retrieve
  Attribute Value answers `404`. A `rename` serves every kind.
- A `unitConversion` of a value that is not a number serves nothing; a
  `valueMappings` table keyed `"1"` also translates a stored `1.0`.
- A slot whose source Attribute no rule of the view shows is never served,
  and the loader says so in the view's `notes` (`/q/health`, `/q/views`):
  this is what a model change that renames or removes a source Attribute
  looks like. A `q` term on such a slot holds for nobody (`!slot` for
  everybody), as on a hidden Attribute of a view without a mapping; an
  `orderBy` or a geo-query on it is refused (400, the slot is not
  filterable), the default `location` of a geo-query included, so no answer
  depends on a hidden value. The mapping never widens what the
  rules show.
- A slot member the IR does not define for its kind (a misspelt `ofset`)
  refuses the view with the slot's index.

A mapped view answers `400` for:

| Detail | When |
|---|---|
| the request names an attribute the model this view serves does not define | a target name the mapping lacks |
| the request filters on an attribute this view computes or translates without an inverse | `q`, `orderBy` or `geoproperty` on such a slot, or a geo-query with no filterable `location` |
| the request uses a parameter a mapped view does not serve | `lang`, `join`, `datasetId`, `aggregatedValues` and every parameter not in the note's table |
| the request names a parameter twice | any parameter twice |
| a mapped view names its attributes by its own @context; send no Link header | a `Link` header |
| a mapped view answers JSON and JSON-LD, not GeoJSON | `Accept: application/geo+json` |
| a mapped view filters and orders on whole attributes of its own class only | a member path on a translated slot, any member path in `orderBy` |

OData, OGC API, the WebSocket, GeoJSON and iCalendar surfaces do not serve a
mapped view: to them its id is unknown (`404`). The cross-view read of
section 6 includes it.

## 6. One read across views

`GET /x/views/_union/ngsi-ld/v1/entities?views=a,b,c&type=Event` runs Query
Entities through each named view, the way `/x/views/{view}/ngsi-ld/v1/entities` would,
and answers the results as one list (T-4494). Each part goes through its own view's
selector, rules, narrowing and Policy. A view the caller may not read, an
unknown view and a view without `query_entities` all contribute nothing,
and the answer gives no sign of which views they were. The design and
its bounds are in [the mapping note](../analysis/view-union.md).

| Parameter | Meaning |
|---|---|
| `views` | the view ids, comma-separated; at most `ANTARES_UNION_MAX_VIEWS` (200) |
| `limit` | Entities per page, default 100, at most 1000 |
| `cursor` | the position the `rel="next"` `Link` hands back; never written by hand |
| `count=true` | `NGSILD-Results-Count` is the sum over the views |
| anything else Query Entities takes | sent to every view as is |

The order is by Entity id, then by the view's position in `views`. An Entity two
views show appears twice, once from each view. `Antares-Entity-Views` names the view of
each Entity, in body order. `orderBy`, `collation`, an `offset` other than 0,
`entityMap`, and `NGSILD-Tenant` are refused with `400`. A write is `405`.

