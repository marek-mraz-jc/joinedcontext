# Air quality observed (`conformance-air-quality` 1.0.0)

The fixture model of the `conformance` space (F-BNT1-002), generated from `conformance-air-quality.linkml.yaml`. One
class, `AirQualityObserved`, specialising the shared `Entity` of `ngsi-ld-core`.

| Attribute | NGSI-LD kind | Range | Unit | IRI |
|---|---|---|---|---|
| `id` | Property | uri | | `ngsi-ld:hasId` |
| `type` | Property | string | | `ngsi-ld:hasType` |
| `location` | GeoProperty | GeoJSON geometry | | `geojson:geometry` |
| `observedAt` | Property | datetime | | `ngsi-ld:observedAt` |
| `name` | LanguageProperty | text per language | | `schema:name` |
| `dateObserved` | Property | datetime | | `sdm:dateObserved` |
| `pm10` | Property | float, ≥ 0 | µg/m³ (UN/CEFACT `GQ`) | `sdm:pm10` |
| `pm25` | Property | float, ≥ 0 | µg/m³ (UN/CEFACT `GQ`) | `sdm:pm25` |
| `reliability` | Property | float, 0…1 | | `sdm:reliability` |
| `refDevice` | Relationship | entity URN | | `sdm:refDevice` |

`dateObserved`, `id` and `type` are required. Every IRI is Smart Data Models' or ETSI's, cited
through `upstream_source` rather than minted here (DM-04, DM-16).

The public endpoint `public-air` does not grant `reliability` or `refDevice`: both are absent
from the entities it serves, from `schema/v1/json-schema` and from `schema/v1/context.jsonld`,
and both are listed in `redactedSlots` of `schema/index.json` (EP-47).
