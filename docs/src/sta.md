# SensorThings API endpoint of a view

An `sta` build (`cargo build -p antares-broker --features sta`) serves every
loaded [view](views.md) to OGC SensorThings API v1.1 clients, Sensing
profile, read only:

```text
GET /x/views/{view}/sta/v1.1/
GET /x/views/{view}/sta/v1.1/{Set}
GET /x/views/{view}/sta/v1.1/{Set}('{id}')
GET /x/views/{view}/sta/v1.1/{Set}('{id}')/{navigation}
GET /x/views/{view}/sta/v1.1/{Set}('{id}')/{member}
GET /x/views/{view}/sta/v1.1/{Set}('{id}')/{member}/$value
GET /x/views/{view}/sta/v1.1/{path}/$ref
GET /x/views/{view}/sta/v1.1/{Set}('{id}')/{Set}('{id}')/…
OPTIONS any of them
```

Mount it with `ANTARES_API_SURFACES=admin,views,sta`. The sets are
`Things`, `Locations`, `HistoricalLocations`, `Datastreams`, `Sensors`,
`Observations`, `ObservedProperties` and `FeaturesOfInterest`; the mapping
and its reasons are `docs/analysis/sta-mapping.md` (T-4462). `HEAD` answers
like `GET` without the body; every other method is `405` with `Allow`.

The `{id}` of a link is the id as an OData key literal (a quote written
twice), percent-encoded but for the unreserved characters, `:` and the
quote: `Datastreams('urn:ngsi-ld:Car:1%2FNO2%23urn:ngsi-ld:Dataset:a')`.
Section 3 defines the id itself.
Every `@iot.selfLink`, `@iot.navigationLink` and `@iot.nextLink` is written
so; a request may send the key encoded or not, as the path is decoded once
before the key is read.

Every `@iot.navigationLink` a resource carries answers (SensorThings 1.1
`req/resource-path`, T-4878): a Thing's `Locations`, `Datastreams` and
`HistoricalLocations`; a Location's `Things` and `HistoricalLocations`; a
HistoricalLocation's `Thing` and `Locations`; a Datastream's `Thing`,
`Sensor`, `ObservedProperty` and `Observations`; an Observation's
`Datastream` and `FeatureOfInterest`; a Sensor's and an ObservedProperty's
`Datastreams`; a FeatureOfInterest's `Observations`. Every Datastream has
exactly one Sensor: the Device its Entity's `refDevice` names (`404` when
the view hides that Device), else the Sensor `unknown`, whose Datastreams
are those of every Entity naming no Device (T-5205); `unknown` is listed and
read only while such a Datastream exists in the view (T-5916). A member
path such as `Things('{urn}')/name` answers `{"name": …}`. A key that is no URI, such as
`Sensors(1)`, answers `404`.

`{member}/$value` (15-078r6 9.2.6) answers the member's raw value: a string
as its text and a number or boolean as its literal (`text/plain;
charset=utf-8`), an object or array such as `location` or
`unitOfMeasurement` as its JSON (`application/json`), a `null` member
`204`. A navigation property has no `$value`: `404`. `{path}/$ref`
(9.2.7) answers the resources of `{path}` as `{"@iot.selfLink": …}` alone,
a list's `@iot.count` and `@iot.nextLink` kept, the next link to the next
page of links; `$select` and `$expand` do not apply to it, and a member that
is no resource (`name/$ref`) is `404`. A path may walk through related
resources (9.2.4): `Things('{a}')/Datastreams('{b}')/Observations` reads
`Datastreams('{b}')/Observations` once `{b}` is one of `{a}`'s Datastreams,
and `Datastreams('{b}')/Thing/Locations` the Locations of that Thing; a
resource that is not related, or that the view hides, at any step is the
`404` of an unknown one. The query options apply to the last step only.

## 1. What a set holds

Every set reads the view's Query Entities, naming the types the view shows,
so a hidden type or Attribute is in no set, no count and no id, and a
hidden id answers the `404` an unknown one does.

| Set | From | Id |
|---|---|---|
| `Things` | each Entity | `E` |
| `Locations` | its `location` now; a HistoricalLocation's, its `location` then | `{E}/location`; `{E}/location/{T}` |
| `HistoricalLocations` | the timed instances of its `location` history | `{E}/historicalLocation/{T}` |
| `Datastreams` | each instance of a numeric Property | `{E}/{A}`, `#{D}` after it for a dataset |
| `Observations` | each numeric instance: the current one, and on a Datastream its history | `{datastream id}/{T}` |
| `ObservedProperties` | each measured Attribute | its expanded IRI |
| `Sensors` | the `Device` its `refDevice` points at; `unknown` first | the Device's id; `unknown` |
| `FeaturesOfInterest` | the target of `refPointOfInterest`, else its `location` | `{P}`, else `{E}/location` |

A Datastream's `unitOfMeasurement` is always present: from the instance's
`unitCode` (`name` the UN/CEFACT code, `symbol` its UCUM symbol for the
common codes, else the code, `definition` the code's IRI), else every member
`null`. Its `observationType` is `OM_CountObservation` for a whole number
without a unit, else `OM_Measurement`. A Sensor's `encodingType` is
`application/json`: its `metadata` is the Device's `controlledProperty` as
JSON, not a SensorML or PDF document; `[]` for a Device without one and for
the Sensor `unknown`, since `metadata` is mandatory (15-078r6 Table 8).

## 2. Query options

| Option | Behaviour |
|---|---|
| `$top`, `$skip` | resources per page, at most 100 (default 100); `$top=0` answers `@iot.count` alone (on a set other than `Things`, only when the first 100 resources are the whole set); `$skip` at most 10 000; on `Datastreams`, `Observations` and `ObservedProperties` a `$skip` reaching past the first 1 000 Entities is `400`: follow `@iot.nextLink`. A page of those sets reads past Entities that hold none of its resources, up to 1 000 Entities per request |
| `$count=true` | `@iot.count`: the Query Entities count on `Things`, and on any other set only when the page is the whole answer; on a collection read whole for `$filter` or `$orderby` (below), its exact count |
| `$filter` | see below; at most 4 096 bytes; anything else `400` naming it |
| `$expand` | every navigation property, one or two deep (`Datastreams/Sensor`), on a list and on one resource: a collection inlined as an array, a single resource as an object, `null` when the view hides it. `Locations`, `Datastreams` and `Datastreams/Observations` on `Things` and `Observations` on `Datastreams` come with the page's Query Entities (an inlined `Observations` there is the current Observation); every other path is read through each resource's navigation link, at most 500 reads per request, past which it is `400`. Nested options (`Observations($top=1)`) are not read; a path that is no navigation property is `400` naming it |
| `$select` | the members each resource keeps, on a list and on one resource; `id` is `@iot.id` |
| `$orderby` | `phenomenonTime desc` alone on a Datastream's Observations is their read order; any other, on any collection: properties or member paths (`unitOfMeasurement/name`), each `asc` (default) or `desc`, a missing or `null` member first, read over the collection (below) |

`$filter` takes comparisons (`eq`, `ne`, `gt`, `ge`, `lt`, `le`), `and`,
`or`, parentheses and `substringof('text', property)`, its text matched
literally. A text literal may hold spaces, parentheses and a doubled quote.
An unquoted value is a number, `true`, `false` or an RFC 3339 instant
(`2026-01-01T00:00:00Z`); any other unquoted value is `400`. A property
name holds letters, digits and `_` only.
The properties are SensorThings names, each read as the NGSI-LD Attribute
of the Entity behind the resource:

| Set | Property | NGSI-LD |
|---|---|---|
| `Things` | `name`, `description`, `properties/{a}` | the Attribute `name`, `description`, `{a}` |
| `Datastreams` | `Thing/name`, `Thing/description`, `Thing/properties/{a}` | the same, of the Datastream's Entity |
| `Observations` | `Datastream/Thing/…` | the same, of the Observation's Entity |
| `Things`, `Datastreams`, `Observations` | `properties/type eq '{type}'` (with the prefix above) on the top-level `and` | the view's `type` selector, narrowed to that type |
| every set | `id eq '{id}'`, `id ne '{id}'` on the top-level `and` | read by id |

A Datastream's Observations take `phenomenonTime` predicates on the
top-level `and`, as the temporal window: `ge t` holds `t`, `gt t` starts one
millisecond after it, `lt t` ends before it, `le t` one millisecond after
it, and `eq t` is the millisecond at `t`.

Any other `$filter`, and every `$orderby` but the read order above, is read
over the collection (T-5918): the endpoint reads the collection whole, at
most 1 000 resources (past them `400`: narrow the path to one Thing or
Datastream, or filter a top-level set on Thing properties), and judges
each resource on the members it answers, so a filter reads nothing the
view hides. Such a `$filter` also takes `not`, `startswith(p, 'text')`,
`endswith(p, 'text')`, `null`, and any member path of the resource
(`result`, `unitOfMeasurement/symbol`, `properties/owner`); a path through a
related resource (`Thing/name`) only where the table above lists it. A
number compares with a number by value and with a text as text
(`result gt '3'`), an instant literal with an instant member by time, and
a comparison with a missing member holds only for `ne`. A name no
resource of the collection holds is `400` naming it, since an unquoted
text would otherwise match nothing. `@iot.nextLink` of such a page carries
`$skip`.

A Datastream's Observations are newest first: its Entity's current instance
and the newest `$skip + $top + 1` instances of its history, read with their
system times, so a page never repeats an earlier one. An Entity with no
temporal record lists its current Observation.

An Observation's `phenomenonTime` is the instance's `observedAt`, and so is
its `resultTime`. An instance without one takes the time its value was last
set (`modifiedAt`) as `phenomenonTime`, has `resultTime: null` and
`parameters: {"timeSource": "modifiedAt"}`; an instance with neither time is
no Observation. A text and the deletion marker `urn:ngsi-ld:null` are no
Observation. A page is cut at 1 MiB between resources, and a cut or a full
page carries `@iot.nextLink` with the request's own options; an empty page
carries none. Errors are the OData JSON error body with a fixed message.

## 3. Resource ids

The ids are a contract, version 1, pinned by the test
`the_ids_of_a_fixture_are_pinned` in `crates/antares-sta/src/mapping.rs`: a
change to it is a new version, listed here, since a client may have stored
an id. The parts of an id are:

- `E`: the Entity id, `P`: a point of interest's Entity id, `A`: the
  Attribute name as the view answers it under the core @context (a term the
  core @context defines, else the expanded IRI), `D`: a `datasetId`. Each
  enters an id with `%`, `/` and `#` percent-encoded (`%25`, `%2F`, `%23`),
  so `https://example.org/car/9` is `https:%2F%2Fexample.org%2Fcar%2F9`.
- `T`: an instant in UTC with three fractional digits,
  `2026-01-01T00:00:00.000Z`; a time stored with an offset is kept as
  stored.

A Thing's id is `E` itself. Two vocabularies' `NO2` are two Datastreams,
since `A` keeps the whole name. An ObservedProperty's id is the expanded
IRI of its Attribute and changes when the @context does.

## 4. Not served

The review of 2026-10-22 (T-5205) listed these; each answers as written
here:

- One Location per `location` instance: a multi-instance `location`,
  `observationSpace`, `operationSpace` and other GeoProperties are no
  Location.
- `@iot.count` on a set other than `Things` past the first page, or on a
  first page that is not the whole answer.
- A `$filter` or `$orderby` read over a collection of more than 1 000
  resources (section 2).
- A neutral observation model shared with a SensorThings 2.0 encoding: the
  `/sta/v1.1` path leaves room for `/sta/v2.0` beside it, and the shared
  representation contract of T-5207 decides the model first.
- The selection of which numeric Attributes are measurements: every numeric
  Property is a Datastream; a view hides the ones it does not publish.

## 3. Named outputs

A view's `representations.sta` lists named outputs (docs API/11 EP-159,
T-5225; the shared contract in [views](views.md)). Each output is its own
read-only service at `/x/views/{view}/sta/{output}/v1.1`, beside the view's
own `/sta/v1.1`, which serves what it served before:

- Its Things are the Entities of its `sources` types that the view shows and
  its `filter` selects. The filter's `q` and `scopeQ` are AND-ed with the
  request's, and its geo-query is fixed. The filter applies to every set,
  every count and every id looked up, so a resource of an Entity outside it
  answers 404.
- Its Datastreams are the attributes its sources' `datastreams` role names
  (`mapping: {datastreams: [pm10, NO2]}`), every numeric Property when no
  source maps the role. A Datastream's Observations read only an Entity the
  filter selects now.
- A Sensor or a point of interest of a type the output does not list is none
  of its resources: list `Device` as a source to serve the Sensors.
- An output name the list does not hold, an output that does not hold, or one
  whose types the view shows none of, answers 404.

## 5. MQTT subscriptions

A build with `sta` and `ws` serves the receive side of the SensorThings MQTT
extension (OGC 18-088 section 14, T-5911): MQTT 3.1.1 over a WebSocket.
Mount it with `ANTARES_API_SURFACES=admin,views,sta,sta-mqtt`.

```text
GET /x/views/{view}/sta/mqtt   Upgrade: websocket, Sec-WebSocket-Protocol: mqtt
```

- **Identity.** The upgrade request is the caller, as a GET of the view is:
  its token and policy headers decide every read. CONNECT's username and
  password are read and dropped. A caller who cannot read the view's
  `/sta/v1.1` root gets CONNACK `5` (not authorized). An upgrade without
  the `mqtt` subprotocol answers `400`.
- **Topics.** `v1.1/{Set}('{id}')`, `v1.1/{Set}('{id}')/{navigation}`
  (`v1.1/Datastreams('{id}')/Observations` among them), `v1.1/Things` and
  `v1.1/Datastreams`, each with an optional `?$select=a,b`. The `{id}` is
  written as in a self link, encoded or not. A topic with a wildcard (`+`,
  `#`), another query option, another whole set, an ObservedProperty, or a
  resource the caller cannot read now (unknown, hidden or refused) gets
  SUBACK `0x80` and no message. Every grant is QoS 0.
- **Messages.** Each topic is one NGSI-LD Subscription made through the
  view as the caller, endpoint this connection, so the notifier feeds it
  and nothing polls. On a notification the broker reads the topic's
  resource again through its own router as the caller. A single resource
  is published when its read changed. For a collection, each member of its
  first page (the newest 10 Observations, else the first 100 members) that
  is new or changed is published as the read of that member. Every message
  is the HTTP `GET` answer of the resource, byte for byte, `$select`
  applied. A whole-set topic publishes the Thing of each changed Entity, or
  its Datastreams.
- **Read only.** A client PUBLISH, and a CONNECT carrying a Will, closes
  the connection: the extension takes no SensorThings writes.
- **Bounds.** The WebSocket channel's limits apply (`ANTARES_WS_*` in
  [WebSocket notifications](websocket.md)): connections per process and per
  Tenant, `ANTARES_WS_MAX_SUBSCRIPTIONS` topics per connection, the
  queue's count and bytes (an overflow closes the connection), the client
  frame rate, the lifetime. A client packet is at most 64 KiB; a message is
  at most 1 MiB. A connection with a keep alive closes after 1.5 times it
  without a packet. Its Subscriptions are deleted when it closes: an MQTT
  connection does not resume.

## Related

- [Surfaces](surfaces.md)
- [Endpoint views](views.md)
- [OGC API - Features endpoint of a view](ogc.md)
- [SensorThings API compliance](sta-compliance.md)
