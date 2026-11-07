# GTFS Realtime of a view

A build with the `gtfsrt` feature (`cargo build -p antares-broker --features gtfsrt`)
serves the live `Vehicle` Entities of a view as a GTFS Realtime 2.0
`VehiclePositions` feed (T-4489):

```text
GET /x/views/{view}/gtfsrt/v1/vehicle-positions.pb
GET /x/views/{view}/gtfsrt/v1/vehicle-positions.json
GET /x/views/{view}/gtfsrt/v1/vehicle-positions.pb?maxAge=60
```

Mount it with `ANTARES_API_SURFACES=admin,views,gtfsrt`. `.pb` is the
protobuf `FeedMessage` (`application/x-protobuf`); `.json` is the same
message in the proto3 JSON form the GTFS Realtime tools read: the proto's
field names, a `uint64` as a string, an enum by its name.

## 1. Mapping

The vehicles are the view's `Vehicle` Entities (SDM `dataModel.Transportation`,
as the HSL HFP pipeline writes them), all of them in one NGSI-LD Query
Entities through the view. What an Entity does not carry stays absent; the
surface invents no trip, stop or label.

| GTFS Realtime field | From | Absent when |
|---|---|---|
| `header.gtfs_realtime_version` | `2.0` | |
| `header.incrementality` | `FULL_DATASET` | |
| `header.timestamp` | the newest kept position's time, else now | |
| `entity.id` | the Entity `id` | |
| `vehicle.position.latitude`, `longitude` | the `location` Point | no Point: the vehicle is dropped |
| `vehicle.timestamp` | `location.observedAt` | no time: the vehicle is dropped |
| `vehicle.position.bearing` | `heading`, degrees | no `heading` |
| `vehicle.position.speed` | `speed`: metres per second with `unitCode` `MTS`, else km/h (the SDM unit) divided by 3.6 | no `speed` |
| `vehicle.vehicle.id` | `fleetVehicleId`, else the Entity `id` | |
| `vehicle.vehicle.license_plate` | `vehiclePlateIdentifier` | no plate |
| `vehicle.trip.route_id` | `route` | no `route`: no `trip` at all |
| `vehicle.occupancy_percentage` | `occupancy` with `unitCode` `P1`, 0 to 100 | no such `occupancy` |

A position older than the freshness window is dropped: `maxAge` seconds, 1
to 86400, default 300 (the broker holds no per-view window). Every vehicle
dropped for its age, a missing Point or a missing time is counted in the
`antares-skipped-records` header and in
`antares_surface_records_skipped_total{surface="gtfsrt"}`.

The message types are a hand-written mirror of `gtfs-realtime.proto`
(google/transit at `3c9e7b90`, copied to `tests/gtfsrt/gtfs-realtime.proto`)
encoded with `prost`; `the_mirror_carries_the_reference_field_numbers`
checks every field number against that file and walks the encoded bytes.

## 2. Refusals

A view that is unknown, does not allow `query_entities` or does not show
`Vehicle`, and any other file, answer `404` alike. Any parameter but one
`maxAge` is `400`. A read the Policy refuses answers the NGSI-LD refusal
(`403`). `HEAD` answers like `GET` without the body; every other method but
`OPTIONS` is `405` with `Allow`. `Cache-Control` is `private, no-cache`.

The conformance pack is
[`tests/gtfsrt/README.md`](https://github.com/marek-mraz-jc/Antares-NGSI-LD-Context-Broker/blob/dev/tests/gtfsrt/README.md).

## Related

- [Surfaces](surfaces.md)
- [Views](views.md)
- [New surface](new-surface.md)
