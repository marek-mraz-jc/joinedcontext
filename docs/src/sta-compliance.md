# SensorThings API compliance

The [SensorThings API endpoint](sta.md) runs against the OGC CITE
executable test suite for SensorThings,
[ets-sta10](https://github.com/opengeospatial/ets-sta10), the suite OGC
certification uses. Our own tests (`crates/antares-sta/tests/sta`) check
each row of `docs/analysis/sta-mapping.md` and that every navigation link
answers. The ETS checks the endpoint from the outside, as a client would.
This page records what the build passes today (T-4878) and why each other
test does not pass.

## 1. The suite

| | |
|---|---|
| ETS | ets-sta10 1.5, the latest release (`opengeospatial/ets-sta10` tags 1.1 to 1.5) |
| Image | `ogccite/ets-sta10:1.5-teamengine-6.0.0-RC2@sha256:e1ec8dc3508ea3355f059abee98737f5d2c00434aeffa298d80ea04361c93db3`, the only tag OGC publishes for it |
| Standard | OGC SensorThings API Part 1: Sensing 1.0 (15-078r6) |
| TEAM Engine suite | `sta10`, run with `ics=3`: the three classes below |
| Fork | [`marek-mraz-jc/ets-sta10`](https://github.com/marek-mraz-jc/ets-sta10) branch `level3-seeded`, commit `5d5d9793c1cbae502a603f4630d1984b97044fd3`: tag 1.5 and two patches, `@iot.id` read as ANY and a Level 3 setup that writes nothing (section 4) |

OGC publishes no ETS for SensorThings 1.1 (18-088), the version the
endpoint serves (`/sta/v1.1`). 1.1 adds members to the 1.0 entities and
keeps its requirement classes, so the 1.0 suite is the one that applies.

| ETS class | 1.0 requirement classes | The endpoint |
|---|---|---|
| Conformance Level 1, Sensing Core (`Capability1Tests`) | datamodel, resource-path, request-data (read) | declares all three in `serverSettings.conformance` |
| Conformance Level 2, Create-Update-Delete (`Capability2Tests`) | create-update-delete | not claimed: read only by the owner's decision (2026-10-28) |
| Conformance Level 3, Filtering Extension (`Capability3Tests`) | request-data query options | 4 of 8 on the fork; the 4 others need fixture counts the derived mapping cannot hold (section 4) |

## 2. The run

Job `sta-compliance-external` in `.github/workflows/surfaces-conformance.yml`
runs on every pull request to `dev`, nightly and on `v*` tags:

1. It takes the `full` broker of the `build` job and starts it on port 9090
   with `ANTARES_API_SURFACES=admin,views,sta`.
2. `dev/views-rig.sh` loads `ci/views/sta`: view `sta` of Tenant `sta`
   (`AirQualityObserved`, `Device`, `PointOfInterest`; reads only), five
   synthetic Entities, and an earlier `pm10` and `location` of
   `AirQualityObserved:station-1` from `history/sta.json`. Every one of the
   eight sets holds a resource.
3. TEAM Engine runs the suite against
   `http://localhost:9090/x/views/sta/sta/v1.1` and answers a TestNG document.
4. `dev/ogc-compliance/baseline.py check` compares it with
   `ci/sta-compliance-baseline.json`, the gate the OGC API - Features lane
   uses ([OGC API - Features compliance](ogc-compliance.md) section 3): red
   when a test the baseline passes fails, is skipped or is gone; a new pass
   is listed so the baseline can be raised in the same change.

5. The same job then builds the fork's jar at its pinned commit with
   `maven:3.9-eclipse-temurin-17@sha256:1a352420…` and copies it over
   `WEB-INF/lib/ets-sta10-1.5.jar` of the pinned image above, so the jar is
   the only difference between the two runs. It runs that image against the
   view `sta-ets` (`ci/views/sta-ets`, T-5918): the fixtures of
   `Capability3Tests` as NGSI-LD Entities, two Things with two measured
   Attributes each, their `location` and value history, and a Device per
   Thing, which `dev/views-rig.sh` writes through NGSI-LD Batch Create and
   Temporal Create before the run, since the fork's Level 3 setup writes
   nothing (section 4). It checks the result twice: against
   `ci/sta-compliance-fork-baseline.json`, and against the official run of
   step 4, red when the fork loses a test the official image passes. The
   fork only adds passes.

The artifact `sta-compliance-external` holds both TestNG documents, the
gates' tables, the broker log and both TEAM Engine logs for 14 days. A
nightly red files an issue.

## 3. Pass rate per class

Each class as the job of section 2 runs it, from a local run of both
images on `agent/worker-t5919/T-5919` (2026-10-28) with the commands of
that job, which wrote both baselines:

| Conformance class | Claimed | Official image, view `sta` | Fork `5d5d979`, view `sta-ets` |
|---|---|---|---|
| Level 1, Sensing Core (6) | yes | 3 passed, 3 failed | 6 passed (100 %) |
| Level 2, Create-Update-Delete (8) | no: read only | 8 skipped | 8 skipped |
| Level 3, Filtering Extension (8) | yes | 8 skipped | 4 passed, 4 failed |
| Total (22) | | 3 passed, 3 failed, 16 skipped | 10 passed, 4 failed, 8 skipped |

The gap between the two columns is the fork's two patches (section 4):
the official image stops at the first `@iot.id` it reads as a number,
so it fails three Level 1 tests and skips every Level 2 and Level 3 test
in their setup. Every test the official image passes, the fork passes too;
the job checks that on each run. Of the 14 tests in the two claimed
classes the fork passes 10. The other 4 need fixture counts only stored
SensorThings resources hold (section 4); Level 2 is not claimed, since
the owner kept the endpoint read only (2026-10-28) and
`serverSettings.conformance` lists no create-update-delete class.

How the counts moved:

| Change | Official image | Fork | What the ETS found and the change fixed |
|---|---|---|---|
| T-4878, first run | 1 passed | | |
| T-4878 | 3 passed, 3 failed, 16 skipped | | Navigation links on every entity, `resultTime` `null` when unknown, `404` for a key that is no URI, an empty `HistoricalLocations` for a Thing without `location` |
| T-5916, fork `0db9383`, view `sta` | unchanged | Level 1 6 passed, 16 skipped | `{member}/$value` (15-078r6 9.2.6), `{path}/$ref` (9.2.7), paths through a related resource (9.2.4), the Sensor `unknown` with `metadata` `[]` and only while a Datastream names it |
| T-5918, fork `5d5d979`, view `sta-ets` | unchanged | 10 passed, 4 failed, 8 skipped | `$filter` and `$orderby` on every property of every set and related collection, `$expand` one and two deep on every navigation property |

The defects in detail:

- `readEntitiesAndCheckResponse` (T-4878): a Location carried no `Things`
  or `HistoricalLocations` navigation link, a Sensor, ObservedProperty and
  FeatureOfInterest none, a Datastream no `Sensor` or `ObservedProperty`,
  an Observation no `FeatureOfInterest` (15-078r6 section 8.2), and only a
  Thing's and a Datastream's links answered. An Observation also lacked
  `resultTime`, mandatory with `null` allowed.
- `readNonexistentEntity` (T-4878): `Sensors(9223372036854775807)` answered
  `400`, the view's refusal of an id that is no URI; it is the `404` of an
  unknown resource now.
- `readPropertyOfEntityAndCheckResponse` (T-5916): `{member}/$value` was
  not served, and the Sensor `unknown` answered a `null` `metadata`, a
  mandatory member (Table 8).
- `checkResourcePaths` (T-5916): `{path}/$ref` and paths such as
  `Things('…')/Datastreams('…')/Thing` were not served, and the Sensor
  `unknown` was listed with no Datastream when every Entity named a Device.
- `readEntitiesWithFilterQO`, `readEntitiesWithOrderbyQO`,
  `readEntitiesWithExpandQO` (T-5918): the endpoint answered `400` to
  `$orderby` on any property but an Observation's `phenomenonTime`, to
  `$filter` on any property but a Thing's, and to `$expand` beyond five
  paths, and dropped `$filter` on a related collection
  ([the endpoint page](sta.md) section 2).

## 4. The tests that do not pass

| Test | Status | Why | Decision |
|---|---|---|---|
| Level 1 `readEntityAndCheckResponse`, `readPropertyOfEntityAndCheckResponse`, `checkResourcePaths` | FAIL on the official image, PASS on the fork | The ETS reads `@iot.id` with `getLong` and stops: `JSONObject["@iot.id"] is not a long`. 15-078r6 table 2 types `@iot.id` as ANY; the endpoint's ids are the Entity URNs and ids derived from them (sta-mapping rule 2). Upstream issue [ets-sta10#40](https://github.com/opengeospatial/ets-sta10/issues/40), "Test suite assumes IDs are long", is open. | ETS defect, fixed in the fork. Numeric ids would make URNs unstable across views and are not planned. |
| Level 2, all 8 | SKIP (setup fails): not claimed | Create-Update-Delete: the setup DELETEs every resource first (`deleteEverythings`), and every test POSTs, PATCHes or DELETEs. The endpoint is read only (`405` with `Allow`); on the official image the setup stops earlier, at the `getLong` read. | Not claimed: the owner kept SensorThings read only (2026-10-28, "do not implement sensorthings writes"); `serverSettings.conformance` lists no create-update-delete class. |
| Level 3 `readEntitiesWithSelectQO`, `readEntitiesWithExpandQO`, `readEntitiesWithOrderbyQO`, `readEntitiesWithFilterQO` | PASS on the fork | `$select`, `$expand` one and two deep on every navigation property, `$orderby` and `$filter` on every property of every set and related collection. | Served (T-5918). |
| Level 3 `readEntitiesWithTopQO`, `readEntitiesWithSkipQO`, `readEntitiesWithCountQO` | FAIL on the fork: not reachable without writes | Each asserts exact sizes per set: 2 Things, 2 Locations, 4 Datastreams, 4 Sensors, 3 ObservedProperties, 4 HistoricalLocations, 12 Observations, 2 FeaturesOfInterest. The derived mapping cannot hold them together: every Entity of the view is a Thing, the Devices that are Sensors included (`Things?$count=true` answers 4); a Sensor is the Device its Entity's `refDevice` names, one per Thing, so 4 Datastreams of 2 Things have 2 Sensors; the `Observations` set lists each Datastream's current instance (4), the earlier ones only under `Datastreams('…')/Observations`. | Not reachable: only stored SensorThings resources (native writes, out of scope by the owner's order of 2026-10-28) hold these counts. |
| Level 3 `checkQueriesPriorityOrdering` | FAIL on the fork: not reachable without writes | `Observations?$count=true&$top=1&$skip=2&$orderby=phenomenonTime asc&$filter=result gt '3'` must count 6 of the 12 fixture Observations; the `Observations` set holds the 4 current ones, so the count is 2. The order of the options is applied as 15-078r6 9.3.3.1 says (filter, count, order, skip, top): `crates/antares-sta/tests/sta/findings.rs`. | Not reachable, as above. |

The fork's patch over tag 1.5, the only one (owner rule 2026-10-28: a
suite defect may be fixed in a fork, each patch minimal, with the clause
that justifies it; never a test patched because the endpoint fails it):

- **`@iot.id` as ANY** (15-078r6 Table 2, ets-sta10#40). Every id stays the
  JSON value the service answered. A path key is a number, or an OData
  string literal with an inner quote doubled (OData 4.0 URL Conventions);
  an id linked in a request body is its JSON value; an id read from a
  `Location` header is a string when quoted. A service with numeric ids
  gets the same requests as before. `VerifyServiceURLBuilder` tests both
  kinds; `mvn test` passes.

- **Level 3 setup writes nothing** (T-5918, owner order 2026-10-28: "do not
  implement sensorthings writes", setup changes allowed with the
  assertions untouched). `createEntities` (POST, PATCH) and
  `deleteEverythings` (DELETE) are removed from `Capability3Tests`; the
  fixtures are the view's NGSI-LD seed (section 2 step 5). No test read
  the ids the setup kept. Branch `level3-seeded` (commit `5d5d979`, on top
  of `iot-id-any`); this one is not proposed upstream.

The `@iot.id` patch goes upstream as a pull request from the fork's branch
`iot-id-any` to `opengeospatial/ets-sta10` `master`
([compare](https://github.com/opengeospatial/ets-sta10/compare/master...marek-mraz-jc:ets-sta10:iot-id-any)).

## 5. The gate goes red

Two deliberate breaks proved the gates locally:

- Official image (T-4878): dropping the Location's
  `HistoricalLocations@iot.navigationLink` again made `baseline.py check`
  exit 1 on
  `Conformance Level 1 :: Capability1Tests.readEntitiesAndCheckResponse`.
- Fork (T-5919): reading `desc` in `$orderby` as ascending
  (`crates/antares-sta/src/residual.rs`, `orderby`) and rerunning both
  images with the job's commands left the official gate at exit 0 and made
  the fork gate exit 1 on
  `Conformance Level 3 :: Capability3Tests.readEntitiesWithOrderbyQO`
  (9 passed, 5 failed, 8 skipped).

## Related

- [SensorThings API endpoint of a view](sta.md)
- [OGC API - Features compliance](ogc-compliance.md)
- [OData compliance](odata-compliance.md)
