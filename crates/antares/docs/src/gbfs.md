# GBFS of a view

A build with the `gbfs` feature (`cargo build -p antares-broker --features gbfs`)
serves the bike-hire docking stations of a view as a GBFS 3.0 feed (T-4488):

```text
GET /x/views/{view}/gbfs/v1/{name}/gbfs.json
GET /x/views/{view}/gbfs/v1/{name}/system_information.json
GET /x/views/{view}/gbfs/v1/{name}/station_information.json
GET /x/views/{view}/gbfs/v1/{name}/station_status.json
```

Mount it with `ANTARES_API_SURFACES=admin,views,gbfs`. `gbfs.json` is the
discovery file; it lists the other three with absolute URLs (the base is
`ANTARES_PUBLIC_URL`, else the request's `Host`).

## 1. The output

No Entity says what the bike system is called, who answers for the feed or in
which time zone it runs, so a view serves a feed only for an output its
`representations.gbfs` lists (at most 20, each `name` once):

```json
{"representations": {"gbfs": [
  {"name": "citybikes", "systemId": "hsl-citybikes", "language": "fi",
   "title": {"fi": "Kaupunkipyörät", "en": "City bikes"},
   "timezone": "Europe/Helsinki", "feedContactEmail": "opendata@example.fi",
   "openingHours": "Apr 1-Oct 31", "ttl": 60}
]}}
```

| Member | Meaning | Bounds |
|---|---|---|
| `name` | the `{name}` segment | 1 to 64 of `a-z`, `0-9`, `-` |
| `systemId` | `system_id`; default `{view}-{name}` | 1 to 64 of `A-Z a-z 0-9 - _ . :` |
| `language` | the language of a station name written as a plain Property | BCP 47 as GBFS takes it: `fi`, `en-GB` |
| `title` | `system_information.name`, one text per language | 1 to 10 languages, each text 1 to 200 characters |
| `timezone` | `timezone`, an IANA zone | `Area/City` segments |
| `feedContactEmail` | `feed_contact_email` | one `@`, a dotted domain, no space |
| `openingHours` | `opening_hours`, OSM `opening_hours` syntax | 1 to 200 characters |
| `ttl` | every file's `ttl`, seconds; set it to the pipeline's period | 0 to 86400, default 60 |

A member out of bounds or unknown makes the whole list refused with `500`
("the view's gbfs outputs do not hold; its Endpoint must be published
again"), so a broken configuration never serves a half-filled file.

## 2. Mapping

The stations are the view's `BikeHireDockingStation` Entities (SDM
`dataModel.Transportation`), all of them in one NGSI-LD Query Entities
through the view. What an Entity does not carry stays out of the file; a
station without a field GBFS requires is left out of that file, counted in
the `antares-skipped-records` header and in
`antares_surface_records_skipped_total{surface="gbfs",reason="unmapped"}`.

| GBFS field | From | Absent when |
|---|---|---|
| `station_information.station_id`, `station_status.station_id` | the Entity `id` | never |
| `station_information.name` | `name`: a LanguageProperty gives one text per language, a Property one text in the output's `language`; a tag GBFS refuses is dropped | no name: the station is left out |
| `station_information.lat`, `lon` | the `location` Point | no Point: the station is left out |
| `station_information.capacity` | `totalSlotNumber` | no `totalSlotNumber` |
| `station_status.num_vehicles_available` | `availableBikeNumber` | no live status: the station is left out of `station_status` |
| `station_status.num_docks_available` | `freeSlotNumber` | no `freeSlotNumber` |
| `station_status.is_renting`, `is_returning` | `status`: `outOfService` is false, every other SDM state true | no `status`: left out |
| `station_status.is_installed` | true: the pipeline drops an uninstalled station | |
| `station_status.last_reported` | `availableBikeNumber.observedAt`, in UTC | no time: left out |
| `last_updated` (every file) | the newest `last_reported` of the read, else now | |
| `ttl` (every file) | the output's `ttl` | |
| `system_information.languages` | `language`, the `title` languages and the station names' | |

A read is one moment: the four files of one read carry the same
`last_updated` and `ttl`. `Cache-Control` is `private, max-age={ttl}`, since
the Policy decides per caller.

## 3. Refusals

A view that is unknown, does not allow `query_entities`, does not show
`BikeHireDockingStation` or lists no output `{name}`, and a file GBFS 3.0
does not name here, answer `404` alike. A read the Policy refuses answers the
NGSI-LD refusal (`403`). `HEAD` answers like `GET` without the body; every
other method but `OPTIONS` is `405` with `Allow`.

## 4. Conformance

`tests/gbfs/validate.py` validates each file against MobilityData's GBFS 3.0
JSON schemas (`tests/gbfs/schemas`, gbfs-json-schema at commit
`2e974fd5`). `cargo test -p antares-gbfs` compares the served files with
`tests/gbfs/golden`; after a change to the mapping, copy the served file the
failed assertion prints into its golden file and run the validator on it. The pack is
[`tests/gbfs/README.md`](https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker/blob/dev/tests/gbfs/README.md).

## Related

- [Surfaces](surfaces.md)
- [Views](views.md)
- [New surface](new-surface.md)
