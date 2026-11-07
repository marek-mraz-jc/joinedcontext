# OData endpoint of a view

An `odata` build (`cargo build -p antares-broker --features odata`) serves
every loaded [view](views.md) to OData 4.0 and 4.01 clients:

```text
GET  /x/views/{view}/odata/v4/
GET  /x/views/{view}/odata/v4/$metadata
GET  /x/views/{view}/odata/v4/{EntitySet}
GET  /x/views/{view}/odata/v4/{EntitySet}('{id}')
GET  /x/views/{view}/odata/v4/{EntitySet}/$count
GET  /x/views/{view}/odata/v4/{EntitySet}('{id}')/{property or navigation}
POST /x/views/{view}/odata/v4/{EntitySet}/$query
POST   /x/views/{view}/odata/v4/{EntitySet}
PATCH  /x/views/{view}/odata/v4/{EntitySet}('{id}')
PUT    /x/views/{view}/odata/v4/{EntitySet}('{id}')
DELETE /x/views/{view}/odata/v4/{EntitySet}('{id}')
PUT|PATCH|DELETE /x/views/{view}/odata/v4/{EntitySet}('{id}')/{property}
PUT|DELETE       /x/views/{view}/odata/v4/{EntitySet}('{id}')/{navigation}/$ref
POST   /x/views/{view}/odata/v4/$batch
```

Mount it with `ANTARES_API_SURFACES=admin,views,odata`. The design is
`docs/analysis/odata-linkml-mapping.md` (T-3441); this page is what the
build serves today (T-3508, T-3509).

## 1. The model

The EDM model is the view's Tenant's LinkML schema (`/q/schemas/{tenant}`)
intersected with what the view shows:

- one EntitySet per class the view's `selector` holds, named after the
  class; one property per slot the view's rules show;
- a Relationship to a type the view shows is a navigation property; to
  any other type, the object URI as a string (R5);
- every type derives from `NgsiLd.Entity`: `id` (the key), `_types`,
  `scope`, `createdAt`, `modifiedAt`;
- a class whose LinkML schema marks a slot `identifier: true`, and whose
  view shows it, is keyed by that slot instead (T-3720): the type declares
  the members of `NgsiLd.Entity` itself, and key value `v` names the Entity
  `urn:ngsi-ld:{Class}:{v}`. The key is `Edm.String`, `Edm.Int64` for an
  `integer` slot, or `Edm.Guid` when the slot carries the annotation
  `edm_type: Edm.Guid`, so `Products(2dcd1e3c-e38a-5391-ba63-17ecfeaea3c2)`
  reads `urn:ngsi-ld:Products:2dcd1e3c-e38a-5391-ba63-17ecfeaea3c2`.
  `(ID=v)` names the same Entity. `(id='urn:…')` still names any Entity of
  the set, and it is the canonical key of an Entity whose URN does not
  follow the pattern. A key value never names an Entity of another set.
- the schema narrows and annotates (T-3722): a slot's `edm_type` annotation
  makes an `integer` an `Edm.Byte`, `Edm.SByte`, `Edm.Int16` or `Edm.Int32`,
  a `float` or `double` an `Edm.Single`, and a `string` an `Edm.Guid`,
  `Edm.Duration` or `Edm.Binary`, and a write is checked against that type;
  `description` is `Core.Description`; `odata_computed: "true"` is
  `Core.Computed` and `odata_immutable: "true"` is `Core.Immutable`, and a
  body that writes such a member where the term forbids it is `400`; a
  class's `odata_insertable`, `odata_updatable` or `odata_deletable` set to
  `"false"` takes that write from its set (`405`), never adding one the
  view's `operations` lack.

Each slot maps by its NGSI-LD kind (T-5206). A slot whose range is a class
without `inlined: true` is a Relationship: every class of a DM-05 model is an
Entity type whose identifier is the NGSI-LD `id`, so LinkML's rule (a class
without an `identifier` slot is inlined) is not applied; mark an embedded
object `inlined: true`. A Property holding such an object, a JsonProperty and
an untyped value are `Edm.Untyped`. `$filter` compares such a Property
(`range: Any`) with a number, string or boolean literal by its
JSON value, as `q` does: a value of another type matches only `ne`, so
`currentValue ge 8372` skips a KPI that reads `not measured` (T-5333). A
JsonProperty and an inlined class of the view (a complex value) are never
filtered. A GeoProperty is `Edm.Geography`; the
concrete `Edm.GeographyPoint` and the other subtypes are not built. A slot
named like a base member (`_types`, `scope`, `createdAt`, `modifiedAt`, or
`id` without `identifier: true`) is served with its first character escaped
(`_x0073_cope`); an `identifier: true` slot named `id` is the NGSI-LD `id`
itself. A slot name over 128 characters has no member and is dropped without
a metric. An identifier key `v` names `urn:ngsi-ld:{Class}:{v}` only: the
platform mints `urn:ngsi-ld:{Class}:{space}:…`, so on its Endpoints use
`(id='urn:…')`.

Under OData 4.0 (no `OData-MaxVersion: 4.01`, as Power BI and Excel send) a
type with an `Edm.Untyped` member is `OpenType="true"` and declares no such
property: the member is read, selected and filtered as a dynamic property
(CSDL XML 3.4, URL 5.1.3). The open type names only those members: URLs
resolve against the 4.01 model, so a name the model does not hold stays `400`
in a query option and `404` in a path.

A view without a schema, or with one the build cannot read, has no OData
endpoint (`404`). Nothing the view hides appears in a document, a name or
an error: a hidden type or Attribute and an unknown one answer alike.

The model is built once per view, shown names, schema revision and OData
version, and cached (at most 256 models). `$metadata` carries a strong
`ETag`, answers `If-None-Match` with `304` and is
`Cache-Control: private, no-cache`. Every JSON answer at `minimal` or
`full` metadata names the tag of the XML document its version reads as
`@odata.metadataEtag` (`@metadataEtag` in 4.01), right after the context
URL, so a client knows when to fetch the model again.

Mapping version (T-5207): the view's schema carries `Core.SchemaVersion`,
the LinkML schema's `version` (`0` without one), `.m` and the mapping version
`MAPPING_VERSION` of `antares-odata` (now `1`), such as `2.1.0.m1`; a character
outside the URL's unreserved set becomes `_`. `$schemaversion` takes `*` or that
value; another value stays `501` as matrix 7.1 lists it. Any change to how a slot
shapes, types or reads bumps the mapping version in the same change. The
`Antares.NgsiLd` vocabulary states its own `Core.SchemaVersion` (`1`); a term
removed or retyped goes to a new namespace `Antares.NgsiLd.V2` beside it. A slot
of a kind the mapping does not know is `Edm.Untyped`, `Core.Computed` and not
filterable, with `NgsiLd.Kind` naming the kind.

## 2. Reads

Each read is one NGSI-LD operation of the view, run through `antares-ops`,
so the policy gate fires and the view narrows the answer as on the view
root.

| OData | NGSI-LD |
|---|---|
| `{Set}` | Query Entities (`type`), `count=true` |
| `{Set}('{id}')`, `{Set}(id='{id}')`, the identifier's `{Set}(v)` | Retrieve Entity |
| `$entity?$id={url}` | the `{Set}(key)` the percent-decoded URL names, absolute under the service root or relative to it; any other well-formed id, foreign or naming nothing, is `404` like a hidden Entity, and a missing id, a broken escape or an id with its own query options is `400` (T-3738) |
| `{Set}/$count` | Query Entities, `limit=0&count=true`, `text/plain` |
| `{Set}('{id}')/{prop}`, `.../{prop}/$value` | Retrieve Attribute Value (the view's `operations` name `retrieve_attribute_value`); `$value` is `text/plain` |
| `{Set}('{id}')/{nav}` | Retrieve Entity of the source, then Retrieve Entity of a single target or Query Entities `id=…` of up to 1000 |
| `{Set}('{id}')/{nav}/$ref` | the Relationship's objects as entity references |
| `{Set}/$ref` | Query Entities as for `{Set}`, with `$filter`, `$orderby`, `$top`, `$skip` and `$count`; each Entity as its reference `{Set}(key)`; `$select` or `$expand` is `400` |
| `{Set}('{id}')/$ref` | Retrieve Entity; the Entity's reference, `404` when hidden or absent; `$select` or `$expand` is `400` |
| `POST {Set}/$query` | the same `GET`: the body's options (`text/plain` or a form) joined to the URL's |
| `$filter` | `q` and `id` (section 4) |
| `$search` | one more `$filter` conjunct: a word or `"phrase"` matches when a single-valued String property the view shows contains it, case-insensitively; `NOT` before `AND` (or a space) before `OR`; evaluated by the residual on what the view and the engine admit, so a hidden property never matches (T-3750) |
| `$compute` | `expr as name`, evaluated per Entity by the residual on what the view and the engine admit and written as a dynamic property; the name works in `$filter`, `$orderby` (sorted by the surface) and `$select`; a hidden Attribute reads as absent, so its computed value is `null` (T-3936) |
| `$apply` | `identity`, `filter`, `search`, `orderby`, `top`, `skip`, `compute`, `aggregate` (`sum`, `min`, `max`, `average`, `countdistinct`, `$count`), `groupby` by single properties, `topcount` to `bottompercent` (only the instances with a value, in rank order) and `concat`, in order, over the whole answer the view and the engine admit, read by one scan of at most 10,000 Entities (past it `400 TooManyResults`, never a partial aggregate); after an aggregation the rows carry only their names and no id; `sum`, `average`, `min` and `max` fold the non-null values and are `null` without one; `$top`, `$skip` and `$count` page and count the rows; `expand`, `nest`, the join and hierarchy transformations, rollup, custom aggregates, a `concat` mixing Entities and rows, `search` after an aggregation, the other options beside it, `/$count`, `/$ref` and one Entity are `501` (T-3937, T-3985) |
| `$orderby` | `orderBy` on `id` and single-valued values, `;desc`; `createdAt`, `modifiedAt` sorted by the surface (section 4); a missing value first ascending, last descending: when any Entity lacks a key, the surface sorts |
| `$select` | `pick` of `id`, `type`, `scope` and the selected Attributes and those a `$compute` reads (TS 104 175 4.21: a projection, so an Entity lacking every selected Attribute is still in the answer, its members `null`); `createdAt`/`modifiedAt` add `options=sysAttrs` and are picked. A name the 4.21 grammar would read as something else (a character outside `_:#/%-+@`, a dot in a short name) pushes no projection: the whole Entity is read and the surface projects it (T-3822) |
| `$expand` | `join=inline&joinLevel=1` (section 5) |
| `$top`, `$skip`, `Prefer: odata.maxpagesize` | `limit`, `offset`; a page is at most 1000, 100 by default |
| `$count=true` | `@odata.count` from `NGSILD-Results-Count`, or counted by the surface (section 4) |
| `ngsild.lang`, `ngsild.q`, `ngsild.scopeQ`, `ngsild.georel`, `ngsild.geometry`, `ngsild.coordinates`, `ngsild.geoproperty`, `ngsild.pick`, `ngsild.omit` | the NGSI-LD parameter of that name, ANDed into the query of a `{Set}` or `{Set}/$count` (section 4) |

Only the default instance of an Attribute, the one without a `datasetId`
(TS 104 175 5.2.6.4.1), is the member's value; an Attribute that holds named
instances only reads `null`, and the named instances are not served. A
LanguageProperty reads as its `@none` text, or in the language of
`ngsild.lang` (one BCP 47 tag; the broker's `lang`), and `$filter` compares
the same text. A stored value the member cannot hold reads `null`, never cut
or converted: an array under a single-valued member, an instance of another
kind than the model names, a LanguageProperty without the text asked for.
The member then carries the instance annotation
`<member>@Antares.NgsiLd.Unserved` with the reason. Under
`Prefer: odata.include-annotations="Antares.NgsiLd.*"` (or `*`) each member
carries its instance's `observedAt` and `unitCode` as
`<member>@Antares.NgsiLd.observedAt` and `<member>@Antares.NgsiLd.unitCode`.

Below one Entity only `$select` applies; another option there is `501`. A
hidden or absent property or single target is `404`, a hidden target of a
collection navigation is left out, and an operation the view does not offer
is `405` `MethodNotAllowed`.

Context URLs, next links, entity references, `Location` and
`OData-EntityId` are absolute under `ANTARES_PUBLIC_URL`, or `http://{Host}`
of the request when it is unset, as for the JSON-LD context URL.

A page that leaves rows carries `@odata.nextLink` with a `$skiptoken`. The
token is bound to the view and the query: a token beside `$skip`/`$top`,
from another query or another view, or one issued before T-3758, is `400`.
A page may hold fewer rows than asked and still carry a next link, when the
surface read its budget before it filled (section 4). A query in a Tenant that holds
nothing yet answers an empty set.

### 2.1 Named outputs

A view's `representations.odata` lists named outputs (docs API/11 EP-159,
T-5225). Each output is one more EntitySet, named after the output with `-`
written `_` (`in-a` is `in_a`), over the EntityType of its one source class,
with the class's members and navigation bindings. It is read only: every
write to it is refused, as on a set without the view's write operations. Its
`filter` takes `q` and `scopeQ`; a geo-query stays the caller's, so an output
naming one serves nobody. The filter's `q` is AND-ed with the `$filter`,
`$search` and `ngsild.q` of every collection, `$count` and `$apply` read, and
its `scopeQ` with the request's `ngsild.scopeQ`. A read by key, of a member or
of a `$ref` first asks Query Entities for the id under the filter, so an
Entity outside it answers the 404 of an absent one. The class's own set and an
Entity's `_types` stay as they were. An output whose class the model does not
show, or whose name a class's set already takes, serves nobody.

### 2.2 History sets

A view that offers `temporal_query` serves, beside each class's EntitySet
`{Set}`, a read-only EntitySet `{Set}_history` (T-5908), the OData form of
the OGC `{type}-history` collections (`ogc.md` section 7). `-` is no
SimpleIdentifier character (CSDL XML 17.2), so the name takes `_`.

- One row per Entity and stored `observedAt`, keyed by both:
  `Car_history(id='urn:ngsi-ld:Car:7',observedAt=2026-10-01T10:05:00Z)`.
- Its members are the class's, nullable, a navigation as its object URIs:
  each holds the Attribute's latest default instance (no `datasetId`) at or
  before the row's `observedAt`, so a row of a label change still holds the
  speed observed earlier. An Attribute the view or the Policy hides makes no
  row and holds no value; an instance without `observedAt` is in no row.
- Every read is the view's Query Temporal Evolution of Entities (`attrs` the
  members, `lastN=1000`, the broker's default instance ceiling), so the view's
  rules and the Policy narrow the history as they narrow the current state.
- `$filter` is evaluated over each row; its top-level bounds on `observedAt`
  narrow the read: `ge`, `gt` or `eq` give `timeAt` (`timerel=after`), `lt`
  gives `endTimeAt` (`between`) or `timeAt` (`before`). A lower bound adds one
  `timerel=before&lastN=1` read, so the first rows of the window hold the
  values valid then. An `id eq` conjunct is the read's `id`.
- Rows come in Entity order, then `observedAt` ascending; `$top`, `$skip`
  and `odata.maxpagesize` page them. `$count`, `/$count` and an `$orderby`
  other than `id, observedAt` read the whole answer, at most 10 000 rows
  kept and 1 000 Entities read, so a `$filter` keeping no row still stops
  there (`400 TooManyResults` past either: narrow by `id` or `observedAt`).
- `$select` takes members; `$expand`, `$search`, `$apply` and `$compute` are
  `501`, an `ngsild.` option `400`, every write `405`.
- Unlike the OGC history, a GeoProperty written without `observedAt` is in
  no row, and a row has no `validTo`: a window's upper bound would cut it.

## 3. Writes

Each write is one NGSI-LD operation of the view, gated like a read, and
the view's rule 7 (pre-image, post-image, allowlist) holds in the handler
it runs. The body is written in the normalized form, so the kind of each
Attribute comes from the model, never from the shape of a value.

| OData | NGSI-LD |
|---|---|
| `POST {Set}` | Create Entity of the set's type, `id` from the body; `201` with the Entity as the view reads it and `Location`, or `204` and `OData-EntityId` under `Prefer: return=minimal` |
| `nav@odata.bind` (4.0), `nav: {"@id": …}` (4.01) | a Relationship; the target is read first with Retrieve Entity under the view |
| `PATCH {Set}('{id}')` | Merge Entity `?type=`: a `null` is the NGSI-LD Null and deletes the Attribute; `204`, or `200` with the Entity as the view reads it under `Prefer: return=representation`; a `return` preference is echoed in `Preference-Applied`. A bind of a navigation with a `ReferentialConstraint` also sets the constrained Property to the target's key |
| `PUT {Set}('{id}')` | Replace Entity `?type=`: what the body omits is removed; `204` (`Prefer: return=` as for `PATCH`). The Entity is read under the view first: one holding an Attribute instance with a `datasetId` is `409` (the surface does not serve those instances, so a replace would delete them unseen; `PATCH` keeps them), and one holding an Attribute the view hides is refused by the view (threat-model D6) (T-5206) |
| `DELETE {Set}('{id}')` | Delete Entity `?type=`; `204` |
| `PUT {Set}('{id}')/{prop}` | Replace Attribute `?type=` in the member's kind; `404` when the Entity does not hold it (a `PATCH` of the Entity adds it); `204` |
| `PATCH {Set}('{id}')/{prop}` | Partial Attribute Update `?type=`; `null` is the NGSI-LD Null; `204` |
| `DELETE {Set}('{id}')/{prop}`, `…/{prop}/$value` | Delete Attribute `?type=`; `204` |
| `PUT {Set}('{id}')/{nav}/$ref` | Append Attributes `?type=`: a single-valued navigation as one Relationship, a collection as its full list in its kind (`ListRelationship` keeps the order); every target is read first under the view; `204` |
| `DELETE {Set}('{id}')/{nav}/$ref` | Delete Attribute `?type=`; `204` |

A write the view's `operations` do not offer is `405` before its body is
read. An absent Entity, a hidden one and one of another type are `404`,
and a `PATCH` or `PUT` never creates one (no upsert). A bind to a hidden
target answers the `404` of an absent one and writes nothing. A body key
that differs from the URL's, a computed member (`_types`, `createdAt`,
`modifiedAt`), a Relationship to a type the view hides and a
`LanguageProperty` are `400`; `$metadata` lists the last two as
`NonInsertableProperties` and `NonUpdatableProperties`. Deep insert, deep
update, a bind on a collection navigation, `POST` or `DELETE` of one
`$ref` in a collection (no operation removes one object of a
Relationship) and `PUT …/$value` are `501`, and so are `If-Match`,
`If-None-Match` and a query option on a write. A property write that
names `id`, `scope`, a computed member, a `LanguageProperty` or a URL
member is `400` before any operation.

### 3.1 `$batch`

`POST $batch` takes a `multipart/mixed` (Protocol 11.7.7) or a JSON
(JSON Format 19) batch and answers in the same format. Each member is
served as a request of its own: parsed, refused and gated as it would be
alone, under the outer request's tenant, subject and credentials. Members
run in order. In a multipart batch the first failed member ends the batch
unless `Prefer: odata.continue-on-error` (or `continue-on-error`) is sent
(Protocol 11.7.7.4). A JSON batch runs every member unless
`continue-on-error=false` is sent (JSON Format 19.4). A member whose
`dependsOn` failed answers `424` (JSON Format 19.1). The response carries
`Preference-Applied` when the preference asked to go on.

A JSON batch is a 4.01 format: with `OData-MaxVersion: 4.0` it is `400`;
without the header it reads.
The multipart framing reads CRLF or a bare LF as a line end; a lone CR is
`400`, and a part's body keeps its bytes.

The whole batch is `400` before any member runs when a member URL leaves
this view's service root (another host, view or root, a `.` or `..`
segment, an encoded `/` or `\`), when a member sets a header other than
`Accept`, `Content-Type`, `Content-Length`, `If-Match`, `If-None-Match`,
`OData-Version`, `OData-MaxVersion` and a `Prefer` of `return` and
`odata.*` preferences, past 100 members (or `ANTARES_MAX_BATCH_ITEMS`
when lower), or when a member's JSON body nests deeper than 16. Change
sets, `atomicityGroup` and Content-ID references (`$1/...`) are `501`; a
batch inside a batch is `400`.

## 4. `$filter`

A filter is split at its top-level `and`s. A part `q` can say is
translated to `q`; any other the residual evaluator covers is evaluated on
the Entities the view answered, never on more (ADR-0028). Each
Attribute is named by a term `t{n}` the request's own `@context` defines,
so a client `Link` header cannot change what a filter means.

- `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `in`, `and`, `or`, `not` around the
  string functions, `eq null` and `ne null`;
- `contains`, `startswith`, `endswith` on `Edm.String` members, escaped
  and anchored (`~=`); a `"` in the pattern is matched as `\x22`;
- `id eq '…'` or `id in (…)` as a top-level conjunct (`id=`);
- one single-valued navigation hop, `owner/name eq 'Ann'`, as
  `t0{t2:t1}=="Ann"` under a `join`: the broker resolves the target
  through the view and the engine, a hidden one like a dangling link.

`ne` holds on an absent member, as in OData. A literal compared with a
member, or listed in `in`, is of the member's type or the filter is `400`
(URL 5.1.1.1, T-3725): `Name eq 1` and `Price eq 'abc'` are refused, a
number promotes among the numeric types (`Quantity lt 7.5`), an integer
past its type's range is refused (`Size eq 9223372036854775808`), and
`null` compares with any member. A number past 2^53 is `400` (NGSI-LD
compares `f64`). A filter longer than 4096 bytes as `q` is `400`
`TooComplexQuery`. Paths past one hop, durations, `cast`, `isof`,
`case`, `hassubset`, `hassubsequence`, `totalseconds`, `geo.distance` and
`geo.length` are `501`.

Evaluated by the surface (T-3758, T-3749): `not` over any filter here
(`not (Price gt 5)` holds where `Price` is absent: OData compares a
missing value as false), a `geo.intersects` that is not the one top-level
conjunct, arithmetic (`Price mul Quantity gt 100`, `-Price lt 0`), two
members compared (`Price gt Cost`), the string functions (`length`,
`indexof`, `substring`, `concat`, `tolower`, `toupper`, `trim`,
`matchesPattern` with an ECMAScript pattern), the date and time functions
(`year` to `fractionalseconds`, `totaloffsetminutes`, `date`, `time`,
`now()`, `mindatetime()`, `maxdatetime()`), `round`, `floor`, `ceiling`,
and `any`/`all` over a list (`Tags/all(t: t ne 'x')`; an absent list is
empty). An operand of the wrong type is `400`: `length(Price)`, `Name add
1`, `Price eq Name`.
Such a filter reads the answer in pages of up to 1000 Entities ordered by
the request's `$orderby` and then `id`, at most 10 000 per request: a page
that runs out of budget is served short with its next link.
`$count=true`, `/$count` and a `$orderby` on `createdAt` or `modifiedAt`
read the whole answer, which is `400` `TooManyResults` when the broker
counts more than 10 000 Entities. Such a read may `$expand` entities and
follow a navigation hop in the filter: the scan reads the broker's
`NGSILD-Warning`, and a join walk any upstream page truncated at the
lookup budget is the same `400` as a pushed read's (T-3823); a partial
expansion is never served.

The custom options `ngsild.q`, `ngsild.scopeQ`, `ngsild.georel`,
`ngsild.geometry`, `ngsild.coordinates`, `ngsild.geoproperty`,
`ngsild.pick` and `ngsild.omit` take NGSI-LD values (TS 104 176 7.2.3.1)
whose names are read under the view's `@context`. They only narrow:
`ngsild.q` is ANDed with the filter, `ngsild.geo*` takes the one `geoQ`,
and `ngsild.pick`/`ngsild.omit` drop members from the answer without
changing which Entities match. Each is accepted once, on a collection or
its `$count`; a `ngsild.q` that does not parse is `400`, a bad value of the
others the broker's own `400`.

## 5. `$expand`

`$expand=owner`, `*`, `owner/$ref` and a nested `$select` are served one
level deep. The join names only the expanded Relationships, so a
navigation that is not expanded costs nothing. A target the view or the
engine hides, or one of another type, reads like an absent one: `null` for
a single navigation, left out of a collection. `/$ref` writes the
Relationship's object as an entity reference. A walk past the broker's
budget of 1000 lookups is `400`, never a partial answer. A nested
`$expand`, `$levels` and the other nested options are `501`.

## 6. Capabilities

`$metadata` annotates each EntitySet with what the surface serves:
`Insertable`, `Updatable` and `Deletable` as the view's `operations` offer
Create, Merge or Replace, and Delete Entity, with the members no write
names; `Countable`, `Filterable` and `Sortable` with the members they
refuse (`NonFilterableProperties`, `NonSortableProperties`), `Expandable`
one level, no upsert, deep insert or deep update, and `Searchable`. The
container declares `QuerySegmentSupported`, `ComputeSupported`,
`BatchSupported` and `BatchSupport` (`multipart/mixed` and `application/json`, continue on
error, no Content-ID references; T-3723). Since T-5907 each EntitySet also
declares `IndexableByKey`, `TopSupported` and `SkipSupported`, and
`NavigationRestrictions` gives each navigation property `$select` and no
other option (`Filterable`, `Searchable`, `Sortable`,
`TopSupported`, `SkipSupported` and `IndexableByKey` false), as a path below
one Entity and an `$expand(...)` serve it; the record carries no
`CountRestrictions`, which the Capabilities vocabulary of the .NET client
(Microsoft.OData.Edm) lacks and refuses as an extra property; `ExpandRestrictions` `MaxLevels`
is 1. The container's `FilterFunctions` lists exactly the operators and
functions a `$filter` translates (`eq` to `mod`, `in`, `any`, `all` and the
functions of section 4); a name outside it is `501`. `ConformanceLevel` is
`Minimal`, the lowest level of the enumeration: the pinned suite measures
Minimal at 85.0 %, Intermediate at 79.8 % and Advanced at 61.1 % (T-5906),
so no higher level is proven (`odata-compliance.md` section 5). The annotations are advisory:
every request is still gated, and a read the engine denies is `403`
whatever they say.

Every `GET` answers `HEAD` too: the same status and headers, with the
`Content-Length` of the `GET` and no body, through the same gate (T-3723).

## 7. Refusals and errors

The OData features no NGSI-LD operation stands behind (matrix 7.1 of
`docs/analysis/operation-matrix.yaml`:
actions, functions, `$crossjoin`, `$all`, delta, media, and the rest) are
refused with the status the matrix names, before any gate or store access.
An upsert is the exception: it is the Merge or Replace Entity it names,
gated, and answers that operation's `404`. A custom query option other
than the `ngsild.` ones of section 4 is `400`. A query a form encoder wrote reads as its plain form (T-3719):
`%24filter` is `$filter`, `%2F` and `%3D` are the query's `/` and `=`, and
`+` is a space where the grammar requires whitespace (`Price+gt+5`), never
inside a literal; a `$format` media type is read percent-decoded
(`application%2Fjson%3Bodata.metadata%3Dnone`, T-3742); a write method where it has no meaning (`/`, `$metadata`,
`$count`, a `POST` to one Entity, a `DELETE` of a set) is `405`, and the
write forms of later rounds are `501`.

These NGSI-LD features have no OData mapping and are not served (T-5206 finding 16):

- temporal representations: history is served only as the `{Set}_history` sets (section 2.2),
  never as `temporalValues` or aggregated values;
- `deletedAt`, `expiresAt` and `ngsildproof`: system and proof members outside the EDM base type;
- the full LanguageMap: one text per read (`ngsild.lang` or `@none`); writing a LanguageProperty is `400`;
- named instances (`datasetId`): only the default instance; no `_datasets` member;
- adding or removing one ListRelationship object through `$ref`: `501`;
- `$deltatoken` and change tracking: `501`;
- `scope` and complex paths (`address/addressLocality`) in `$filter`: `400`;
- sub-properties of a Property (Property of Property): dropped;
- `objectType` of a Relationship: not read; a Relationship to a type the view hides is an URL string.

A change of the view's `@context` renames no member (names come from the slot names); a change
of the schema can, and `@odata.metadataEtag` is then the only signal.

A `501 NotImplemented` names the construct and the reason in a fixed
message, `<construct>: no NGSI-LD equivalent (ADR-0024 decision 5)`; the
construct is one of these names, never a request value
(`tests/odata_refused.rs` sends one request per row):

| Construct | Where it comes from |
|---|---|
| `$index`, `$deltatoken`, `$root`, `$it and $this`, `$each`, `$crossjoin and $all` | URL options and resources ADR-0024 decision 5 leaves out |
| `type cast or bound operation` | a qualified segment: a cast, a derived type, an action or a function |
| `$filter path segment`, `ordinal index`, `media resource`, `non-ASCII identifier` | path forms with no NGSI-LD resource |
| `nested query option`, `nested $count`, `a nested $expand, $levels or a path in $expand`, `annotation path`, `* after a path in $expand` | options inside `$expand(...)` past `$select` (section 5) |
| `$select of a navigation, a path or an annotation` | a `$select` item that is not a plain member |
| `method call`, `has operator`, `cast and isof`, `in over a non-literal`, `literal type`, `JSON literal`, `key or filter in an expression path`, `parameter alias bound to an expression` | `$filter` forms the parser does not take (section 4) |
| `this $filter expression`, `this $orderby expression`, `this $compute expression`, `this $apply expression` | a parsed expression the translation to `q` and the residual cannot hold |
| `$apply transformation`, `search after aggregate in $apply`, `$apply beside another query option, under /$ref or /$count`, `$apply under /$count` | `$apply` outside section 2's list |
| `$filter, $search or $apply on one Entity`, `a query option other than $select below one Entity`, `a path to a system member`, `/$value of a non-primitive value`, `this resource path`, `$id outside $entity`, `a $schemaversion other than the served one` | reads below one Entity and resource options |
| `a write to a whole entity set (delta update)`, `a write to /$value`, `a write appending to a collection property`, `a write to this resource path`, `removing one object of a Relationship`, `deep insert`, `deep update`, `binding a collection-valued navigation property`, `binding a navigation property to null`, `a conditional write (If-Match, If-None-Match)`, `a query option on a write` | write forms of section 3 |
| `change sets`, `atomicity groups`, `references to another member's result`, `this method in a batch`, `binary member bodies` | `$batch` forms of section 3.1 |

An error is `{"error": {"code", "message"}}` with a fixed message: it
names no hidden type or Attribute, echoes no literal of the request and
carries no NGSI-LD `ProblemDetails`.

## Related

- [Endpoint views](views.md)
- [MCP endpoint of a view](mcp.md)
- [OData compliance](odata-compliance.md): the independent suite, its baseline and pass rate per level
- `docs/analysis/odata-linkml-mapping.md`, `docs/analysis/surfaces-threat-model.md` (O1 to O27)
- ADR-0024 (the OData crates), ADR-0028 (pushdown and residual)
