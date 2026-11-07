# Temporal API

The temporal API (CIM 009 clauses 5.6.11–5.6.16, 5.7.3, 5.7.4) serves the
history of attribute values. Every example below runs against the
quickstart broker seeded by `examples/quickstart/seed.sh`; `$U` is
`http://localhost:9090/ngsi-ld/v1` and `$L` the `Link` header carrying the
core `@context`.

## How history is recorded

Every write through the entity endpoints produces temporal events for the
attribute instances it changed. The events are buffered for the request
and drained to the temporal driver after the handler returns and before
the response leaves, so a temporal read that follows a write sees it. An
update that changes no value produces no event; `ANTARES_TEMPORAL_RECORD`
narrows what enters history to observed instances or to nothing. A driver
failure in the drain never changes the response already produced; it is
counted as `temporalDrainErrors` on `/q/health`. The gates, the driver
choice `ANTARES_TEMPORAL` and retention are described in
[Storage drivers](storage.md#what-enters-history).

Three patches of `temperature` with an `observedAt` each:

```bash
curl -X POST $U/entities/urn:ngsi-ld:TemperatureSensor:qs:1/attrs \
  -H 'Content-Type: application/json' -H "$L" \
  -d '{"temperature":{"type":"Property","value":24.1,"unitCode":"CEL","observedAt":"2026-08-26T09:00:00Z"}}'
```

The seed value `21.5` carried no `observedAt`, so it appears only under
`timeproperty=modifiedAt` (below), never in the default `observedAt`
history.

### Versions, provenance and late values

A history row is one measurement: an Attribute, its `datasetId` and its
`observedAt` (platform ADR-N-054 2.1). Sending the same value again for the
same instant stores nothing new. A different value for the same instant is
a correction: on postgres and timescale the row becomes version 2 and
version 1 is kept, with when it was superseded, in `attr_versions`. A
revert to the old value is version 3, never a rewrite. The memory and file
drivers keep the current version only.

Each row carries its provenance: `channel` (`api`, `pipeline`, `ui`,
`import`, `backfill`, `mqtt`), `actor`, `operation` (`create`, `update`,
`correct`, `backfill`, `delete`), `client` (the application the write came
through), `reason` and `requestId`. The broker believes the
`JC-Provenance-Channel`, `JC-Provenance-Actor`, `JC-Provenance-Client`,
`JC-Reason` (percent-encoded) and `X-Request-Id` headers only from the mesh identity
`ANTARES_PROVENANCE_FROM` names in `l5d-client-id`; from anyone else a
write is `channel: api` with no actor. A read never returns provenance as
data, except through `versions=all` (below).

The current state keeps who made each attribute instance's last change
beside its `modifiedAt`: `modifiedBy` with the write's `organization`,
`actorKind` and `actor`, from the same believed headers (platform ADR-N-054
2.8, DM-138). A write the gateway did not send names nobody and clears the
member, so no earlier writer stays on a value it did not write. A read
answers `modifiedBy` only with `options=sysAttrs` and only when the gateway
also sent `JC-Read-Versions: true`; a plain read, a notification and
keyValues never carry it, and history rows say who in their provenance
instead. A `jc` build only: vanilla Antares stamps none.

With `ANTARES_HISTORY_BACKFILL=on`, a value whose `observedAt` is older
than the current one's is a backfill: it goes into history with
`operation: backfill` and leaves the current state untouched. Off (the
default), it replaces the current value as NGSI-LD says.

A type's retention rule may record history `onChange` instead of every
write ([Retention](admin-api.md#retention)): a row only when the value
moves past its `deadband` (numbers) or `minDistance` (metres, GeoProperty),
when a value of the same instant is corrected, and once per `heartbeat`.
A value older than the last row (a retried batch, a replay) is judged the
same way against the nearest row below it, so it records one row per
heartbeat too (T-5157);
or `observed`: a row only for a newer `observedAt`; or `never`. With
`ANTARES_TEMPORAL_RECORD=ruled` a type records nothing until a retention
rule gives it a history duration, and then `observed` unless the rule names
another mode (T-5056).

## Querying

`timerel` is `before`, `after` or `between` around `timeAt` (and
`endTimeAt` for `between`), applied to `timeproperty` (`observedAt` by
default; `createdAt`, `modifiedAt`, `deletedAt`, `expiresAt`, the 4.8 set of
TS 104 175 5.2.4 NOTE 2). A window over `expiresAt` keeps the instances whose
expiry falls in it; an instance already past its `expiresAt` is invalid
(5.2.4) and absent from every read, before the maintenance reap deletes it.
Clause 4.11 fixes the
bounds: `after` includes the `timeAt` instant, `before` excludes it, and
`between` includes `timeAt` and excludes `endTimeAt`. `attrs` restricts the
attributes.

```bash
curl -si "$U/temporal/entities/urn:ngsi-ld:TemperatureSensor:qs:1?timerel=after&timeAt=2026-08-26T08:30:00Z&attrs=temperature"
```

```json
{"id": "urn:ngsi-ld:TemperatureSensor:qs:1", "type": "TemperatureSensor",
 "temperature": [
   {"type": "Property", "instanceId": "urn:ngsi-ld:Instance:6e0d857e-8845-57ee-9e5b-bbcc34d89a0b",
    "observedAt": "2026-08-26T09:00:00Z", "unitCode": "CEL", "value": 24.1},
   {"type": "Property", "instanceId": "urn:ngsi-ld:Instance:19536e56-b044-51cd-8658-2600fb344d98",
    "observedAt": "2026-08-26T10:00:00Z", "unitCode": "CEL", "value": 26.8}]}
```

Each instance carries an `instanceId`; it is the handle for the
instance-level operations below. `GET $U/temporal/entities?type=…`
queries several entities with the same parameters plus `q`, `geoQ`,
`scopeQ`, `id`, `idPattern` and paging; `POST
$U/temporal/entityOperations/query` takes the same query as a `Query`
body with a `temporalQ` member:

```bash
curl -X POST $U/temporal/entityOperations/query -H 'Content-Type: application/json' -H "$L" -d '{
  "type": "Query", "entities": [{"type": "TemperatureSensor"}],
  "temporalQ": {"timerel": "after", "timeAt": "2026-08-25T10:00:00Z"},
  "attrs": ["temperature"], "q": "temperature>25"}'
```

Errors are `BadRequestData` 400 with the reason in `detail`:

```text
{"detail":"invalid timerel \"since\"", ...}
{"detail":"timeAt must be a valid ISO 8601 DateTime (4.11)", ...}
```

### Representation

`format=temporalValues` (or `options=temporalValues`) collapses each
attribute to `[value, time]` pairs:

```json
{"id": "urn:ngsi-ld:TemperatureSensor:qs:1", "type": "TemperatureSensor",
 "temperature": {"type": "Property",
   "values": [[22.4, "2026-08-26T08:00:00Z"], [24.1, "2026-08-26T09:00:00Z"], [26.8, "2026-08-26T10:00:00Z"]]}}
```

`timeproperty=modifiedAt` keys the history on the write time instead, so
the seed value shows up:

```json
"values": [[21.5, "2026-08-26T15:59:33.332Z"], [22.4, "2026-08-26T15:59:33.409Z"],
           [24.1, "2026-08-26T15:59:33.416Z"], [26.8, "2026-08-26T15:59:33.423Z"]]
```

`lastN=2` keeps the two newest instances of each attribute, newest first:

```json
"temperature": [
  {"type": "Property", "observedAt": "2026-08-26T10:00:00Z", "value": 26.8, ...},
  {"type": "Property", "observedAt": "2026-08-26T09:00:00Z", "value": 24.1, ...}]
```

`firstN=2` keeps the two oldest, oldest first. `offsetN` skips that many
instances in the chosen order, so `lastN=2&offsetN=2` is the third and
fourth newest; an `offsetN` past the end returns the last instance in that
order (TS 104 176 Table 6.4.7.3-2). `firstN` and `lastN` must be positive
integers, `offsetN` zero or more, and `firstN` with `lastN` together is a
400 because one page cannot run both ways. A `temporalQ` object in a
POST query carries the same three members. `pick`/`omit` and `sysAttrs`
apply as on the entity endpoints.

### Aggregation

`aggrMethods` (`totalCount`, `distinctCount`, `sum`, `avg`, `min`, `max`,
`stddev`, `sumsq`) with `aggrPeriodDuration` returns one row per bucket
as `[value, bucketStart, bucketEnd]`, buckets anchored at `timeAt`:

```bash
curl "$U/temporal/entities/urn:ngsi-ld:TemperatureSensor:qs:1?timerel=between&timeAt=2026-08-26T08:00:00Z&endTimeAt=2026-08-26T11:00:00Z&attrs=temperature&aggrMethods=avg,max&aggrPeriodDuration=PT1H"
```

```json
"temperature": {"type": "Property",
  "avg": [[22.4, "2026-08-26T08:00:00Z", "2026-08-26T09:00:00Z"],
          [24.1, "2026-08-26T09:00:00Z", "2026-08-26T10:00:00Z"],
          [26.8, "2026-08-26T10:00:00Z", "2026-08-26T11:00:00Z"]],
  "max": [[22.4, "2026-08-26T08:00:00Z", "2026-08-26T09:00:00Z"], ...]}
```

Without `aggrPeriodDuration` (or with `PT0S`) the whole range is one
bucket, ending one second after the last instance:

```json
"avg": [[24.433333333333334, "2026-08-26T08:00:00Z", "2026-08-26T10:00:01Z"]],
"sum": [[73.3, "2026-08-26T08:00:00Z", "2026-08-26T10:00:01Z"]],
"totalCount": [[3, "2026-08-26T08:00:00Z", "2026-08-26T10:00:01Z"]]
```

Which methods apply to which value type follows Tables 4.5.19.1-1 to -3;
a method that does not apply to the attribute's values is a 400. A value
carrying a `Date`, `DateTime` or `Time` datatype — written as a JSON-LD
typed value, `{"@type": "DateTime", "@value": "..."}`, or as a string with
`valueType` — is ordered by that datatype, so `min` and `max` apply to it
and a `Time` also has an `avg`, returned as a `Time`. A plain JSON string
is ordered lexicographically and has no average.

An Attribute its type records `onChange` holds each value until the next
row. A window that opens after its last row carries that row in, so a
chart starts at the level the Attribute held. `avg` weighs each value by
the time it held inside the bucket, and `min` and `max` see the carried
value. `sum` and `totalCount` answer `400` with a detail starting
`aggregation-not-meaningful`: the rows left out repeat a value, they are
not zeros.

On postgres and timescale the aggregation runs in SQL when the query is
exact there: no `q`, `geoQ` or `scopeQ`, the page pushed down, no `omit`,
second-granular period, and only numeric or boolean values in the
window. Any other shape, and every query on the memory and file drivers,
reconstructs the instances and aggregates in the broker. The result is
the same; the SQL path was measured at 5.7 s → 0.59 s for 50 entities
with 150k instances.

A Tenant that records any type `onChange` aggregates in the broker.

### Every version of a measurement

A platform extension, in `jc` builds only (the platform image, T-4854):
vanilla Antares answers `versions` with 400 `unknown query parameter` and
records every write's provenance as `channel: api`.

`versions=all` on `GET /temporal/entities/{id}` replaces each instance with
its chain, oldest first: every version carries `version` and `provenance`,
and a superseded one `supersededAt`. `lastN` and the other page limits
count measurements, then each brings all its versions. The broker answers
it only when the request carries the gateway's mesh identity
(`ANTARES_PROVENANCE_FROM`) and `JC-Read-Versions: true`, which the gateway
sets once the space's Policy granted the caller `readVersions`; otherwise
`403`. Any value other than `all` is a `400`.

### Pagination

One response carries at most nine instances per attribute, and fewer
when `firstN` or `lastN` asks for fewer. Above nine the page ends at one
instant for the whole entity: the ninth instance, in the page order
(oldest first, newest first with `lastN`), of the attribute that reaches
it first. A sparser attribute serves fewer instances on that page, or
none. The response links to its neighbours with `offsetN` moved by one
page (TS 104 176 6.4.7.3). After such a cut the next link moves `offsetN`
by the fewest instances a cut attribute served, so it repeats instances
and never skips one. If an attribute served none, no `offsetN` continues
it, and the next link moves the time window past the page's last instant
instead (`timerel=after` ascending, which serves the instances at that
instant once more; `timerel=before` with `lastN`). Twelve hourly
instances of a sensor:

```text
HTTP/1.1 206 Partial Content
Link: </ngsi-ld/v1/temporal/entities/urn:ngsi-ld:TemperatureSensor:qs:2?attrs=temperature&format=temporalValues&offsetN=9>; rel="intervalafter";type="application/json"
Content-Range: date-time 2026-08-25T00:00:00Z-2026-08-25T08:00:00Z/*
{"id": "urn:ngsi-ld:TemperatureSensor:qs:2", "type": "TemperatureSensor",
 "temperature": {"type": "Property", "values": [[20.0, "2026-08-25T00:00:00Z"], ..., [28.0, "2026-08-25T08:00:00Z"]]}}
```

Following `intervalafter` serves the rest and links back with
`intervalbefore`; the last page has no forward link:

```text
GET …?attrs=temperature&format=temporalValues&offsetN=9
HTTP/1.1 200 OK
Link: <…&offsetN=0>; rel="intervalbefore";type="application/json"
"values": [[29.0, "2026-08-25T09:00:00Z"], [30.0, "2026-08-25T10:00:00Z"], [31.0, "2026-08-25T11:00:00Z"]]
```

A `lastN` page runs backwards in time, so its next (older) page is
`intervalbefore` and the one before it `intervalafter`. The `206` and
`Content-Range` of a page cut by the nine-instance ceiling stay while the
ETSI suite (020_13, 021_27) still asserts them; the range bounds the
instants the body holds. Aggregated representations are
computed over the whole evolution and are never cut. Entity-level paging
of `GET /temporal/entities` uses `limit`/`offset` and `Link rel="next"`
as the entity endpoints do.

### Temporal entity maps

`entityMap=true` on a multi-entity query pins the matched id set for the
following pages, so a client walking `rel="next"` links sees a stable set
even while entities change:

```text
HTTP/1.1 201 Created
Link: </ngsi-ld/v1/temporal/entities?entityMap=true&limit=1&offset=1&…>; rel="next";type="application/json"
```

The map lives under `/temporal/entityMaps/{id}` for one hour by default;
a client may set `expiresAt` on the map, capped at 24 hours (6.4.3.2-1).
It is stored in the current-state driver, so it survives a restart on
file and postgres.

## Writing and deleting history

| operation | clause | request |
|---|---|---|
| create a temporal entity with its instances | 5.6.11 | `POST $U/temporal/entities` |
| add instances to an attribute | 5.6.12 | `POST $U/temporal/entities/{id}/attrs` with `{"temperature": [instance, …]}` |
| delete an attribute's history | 5.6.13 | `DELETE $U/temporal/entities/{id}/attrs/{attr}`, `?datasetId=` for one instance set, `?deleteAll=true` for every set |
| modify one instance | 5.6.14 | `PATCH $U/temporal/entities/{id}/attrs/{attr}/{instanceId}` |
| delete one instance | 5.6.15 | `DELETE $U/temporal/entities/{id}/attrs/{attr}/{instanceId}` |
| purge an entity's history | 5.6.16 | `DELETE $U/temporal/entities/{id}` |

All answer 204; a missing instance or entity is `ResourceNotFound` 404:

```text
{"detail":"instance urn:ngsi-ld:Instance:cbcc1cff-46bb-4b7f-b82e-f42a60d75542 not found","status":404,"title":"ResourceNotFound", ...}
```

Purging a temporal entity removes its history only; `GET
$U/entities/{id}` still answers 200 with the current state. Deleting the
current-state entity mirrors a deletion instance into history, so the
entity's last state remains queryable under `timeproperty=deletedAt`.

Instances added through the temporal endpoints are stored as sent.
Instances the entity endpoints record with an `observedAt` are keyed on
(entity, attribute, `datasetId`, `observedAt`), so a sensor re-sending
the same measurement replaces the instance instead of duplicating it.

## When the history store is down

History may live in a second store (`ANTARES_TEMPORAL` naming another
backend than `ANTARES_STORE`, [storage](storage.md)). The broker treats
that store as a part it can lose for a while without losing the current
state. Decided (T-5210, owner may override): it serves the current state
and fails history closed, and it does not exit, because an exit would take
the current state down with the history.

| When | What happens |
|---|---|
| at start | the broker retries the connection 30 times, then exits with the reason; it never starts without the history store it was told to use |
| an entity write | the write commits and answers as usual (201, 204); the history events of that request are lost, logged as `temporal drain failed` and counted in `/q/health` `temporalDrainErrors` and the metric `antares_temporal_drain_errors_total` |
| a temporal read (11.3.2, 11.3.3) | `500` `InternalError` with a request id; never an empty or partial history |
| a temporal write (5.6.11 to 5.6.16) | `500` `InternalError`; nothing is written |
| the store comes back | the pool reconnects; history resumes with the next write. The lost window is not replayed |

An alert on `antares_temporal_drain_errors_total` rising is the signal
that history has a gap. The test is `a_failing_drain_keeps_the_2xx_and_is_counted`
in `crates/antares-api/tests/temporal_drain.rs`.

## Retention and `none`

`ANTARES_TEMPORAL_RETENTION_DAYS` starts a sweep on the postgres or
timescale half that drops instances older than the horizon; unset keeps
everything. With `ANTARES_TEMPORAL=none` every temporal endpoint answers
`OperationNotSupported` 422 (Table 6.3.2-1) and the entity endpoints
record nothing:

```text
HTTP/1.1 422 Unprocessable Entity
{"detail":"no temporal store is configured","status":422,"title":"OperationNotSupported","type":"https://uri.etsi.org/ngsi-ld/errors/OperationNotSupported"}
```

`/q/health` names both halves: `"store": "memory", "temporal": "none"`.
