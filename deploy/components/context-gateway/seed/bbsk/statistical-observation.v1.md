# One published statistical cell (`statistical-observation` 1.1.0)

The model the raw spaces of `bbsk` and `banskabystrica` publish, generated from
`statistical-observation.linkml.yaml`. One class, `KeyPerformanceIndicator`, specialising the
shared `Entity` of `ngsi-ld-core`.

| Attribute | NGSI-LD kind | Range | Unit | IRI |
|---|---|---|---|---|
| `id` | Property | uri | | `ngsi-ld:hasId` |
| `type` | Property | string | | `ngsi-ld:hasType` |
| `location` | GeoProperty | GeoJSON geometry | | `geojson:geometry` |
| `observedAt` | Property | datetime | | `ngsi-ld:observedAt` |
| `dataSet` | Property | string | | `qb:dataSet` |
| `indicator` | Property | string | | `jc:indicator` |
| `refArea` | Property | string | | `sdmx-dim:refArea` |
| `refPeriod` | Property | string | | `sdmx-dim:refPeriod` |
| `dimensionKey` | Property | string | | `jc:dimensionKey` |
| `kpiValue` | Property | float | varies, on the Property as `unitCode` | `sdm-kpi:kpiValue` |
| `unitText` | Property | string | | `jc:unitText` |
| `source` | Property | string | | `jc:source` |
| `dateObserved` | Property | datetime | | `jc:dateObserved` |
| `stewardNote` | Property | string | | `jc:stewardNote` |

`dataSet`, `indicator`, `refArea`, `refPeriod`, `kpiValue`, `source`, `dateObserved`, `id` and
`type` are required.

`stewardNote` is the one attribute no pipeline writes (1.1.0). Every other attribute of a row
belongs to the publisher and is replaced on the next run, so an application that offered one of
them for editing would lose what a person typed without telling them. The note is where the
person who keeps the data writes what they checked and what looks wrong; a Policy grants
`updateAttrs` on this name alone, and a write to any other attribute from the same person is the
gateway's own `403` (EP-14, AP-62). It holds at most 500 characters and no angle brackets.

The `{localId}` of an entity is the publisher's own key, in the publisher's own spelling:
`{dataSet}-{refArea}-{refPeriod}-{indicator}` and then one segment per remaining dimension, so
`zp3803rs-SK032-2023-PROD_TONY-1`. Nothing is renamed on the way in, which is what makes a row
traceable back to the request in `source`.

The unit is on the Property and not on the slot, because one cube serves several indicators with
different units. `unitCode` carries the UN/CEFACT common code and `unitText` carries the
publisher's own words, which is where a scale factor like "v tis. m3" survives.

The class IRI is the Smart Data Model's `KeyPerformanceIndicator`, and the model deliberately declares no Data
Structure Definition: DM-60 puts the observations of a declared DSD in the project's indicator
space, which would leave the raw space holding nothing. DM-60 is the upgrade the day an
indicator has to be sliced by a dimension.
