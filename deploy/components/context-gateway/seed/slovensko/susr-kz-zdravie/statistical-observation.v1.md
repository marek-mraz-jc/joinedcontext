# One published statistical cell

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One cell of a published statistical table, as its publisher published it (DM-01, DM-09). A territory code, a period, the publisher's own indicator code, the remaining dimensions of the cube and the number, with the request that produced it. Nothing is renamed and nothing is recomputed on the way in, so a row can always be traced back to the URL it came from. The type is the FIWARE Smart Data Model `KeyPerformanceIndicator` (dataModel.KeyPerformanceIndicator) and the number its `kpiValue`; the indicators computed from these rows are the same type in the project's indicator space (PF-54, PF-55). It is deliberately not a Data Structure Definition in the sense of DM-60: a declared DSD puts its observations in the project's indicator space, and this space would then hold nothing. DM-60 is the upgrade the day an indicator has to be sliced by a dimension rather than read as one number per territory.

- Namespace: `https://joinedcontext.com/models/slovensko/statistical-observation`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### KeyPerformanceIndicator

One cell of one published statistical table.

IRI: `sdm-kpi:KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataSet` | Property | `string` | yes |  | `qb:dataSet` | The publisher's own code for the table this cell comes from, such as `zp3803rs`. |
| `indicator` | Property | `string` | yes |  | `jc:indicator` | The publisher's own code for the measure, such as `ODPAD_TONY_KM2`. |
| `name` | Property | `string` |  |  | `sdm-kpi:name` | What the number is, in the publisher's own words: the label of its indicator, such as `Miera nezamestnanosti (v %)`, or the table's title when the cube names no indicator. Absent when the publisher labels neither. |
| `refArea` | Property | `string` | yes |  | `sdmx-dim:refArea` | The territory the cell describes, as the publisher codes it: a NUTS code for the kraj or an okres (`SK032`, `SK0323`), a LAU code for a municipality (`SK0321508438`). |
| `refPeriod` | Property | `string` | yes |  | `sdmx-dim:refPeriod` | The period the cell covers, as the publisher labels it, such as `2023` or `2025Q2`. |
| `dimensionKey` | Property | `string` |  |  | `jc:dimensionKey` | The cube's remaining dimensions beyond territory and period, as their codes joined by a hyphen and in the cube's own dimension order, such as `1` for the solid-emissions pollutant. It is the tail of the `{localId}`, repeated here so a query can select a slice without matching on an id pattern. Absent when the table has no further dimension. |
| `kpiValue` | Property | `float` | yes |  | `sdm-kpi:kpiValue` | The number, exactly as published. Its unit varies with the indicator and travels on the Property as `unitCode`, a UN/CEFACT common code, beside `unitText`. |
| `unitText` | Property | `string` |  |  | `jc:unitText` | The unit in the publisher's own words, such as `v tis. m3`, kept because a UN/CEFACT code loses the scale factor. |
| `source` | Property | `string` | yes |  | `jc:source` | The exact URL the row was fetched from, with its query, so the cell can be fetched again. |
| `dateObserved` | Property | `datetime` | yes |  | `jc:dateObserved` | When the publisher last updated the table, as the table itself reports it. |
| `isPartOf` | Property | `uri` | yes |  | `jc:isPartOf` | The catalogue dataset the cell belongs to, the page of its ŠÚ SR table, where its licence and its other cells are. Never `dataset`: the NGSI-LD core @context protects that term. |
| `stewardNote` | Property | `string` |  |  | `jc:stewardNote` | A note by the person who keeps this data: what was checked, what looks wrong, whom to ask. The one attribute of this model no pipeline writes and no pipeline overwrites, so an application may offer it for editing without the next run silently taking the words back (AP-62, EP-14). Every figure beside it belongs to the publisher and is read-only. At most 500 characters and no angle brackets, because it is a note and not a document. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Entity

The root every NGSI-LD entity class specialises.

IRI: `ngsi-ld:Entity`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |
