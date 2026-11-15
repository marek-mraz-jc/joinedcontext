# FloodZone, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One designated flood area: the ground a flood of a given return period covers, or a zone of it. Extends dataModel.PointOfInterest/PointOfInterest: Environment's FloodMonitoring holds a water level measured at a gauge, not the area a flood covers.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/FloodZone`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### FloodZone

One designated flood area: the ground a flood of a given return period covers, or a zone of it.

IRI: `jc:FloodZone`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `floodReturnPeriod` | Property | `integer` |  |  | `jc:floodReturnPeriod` | The return period of the flood the area is flooded by, in years: 5, 20, 50 or 100 for the designated flood areas Q5, Q20, Q50 and Q100. |
| `floodZoneCategory` | Property | `string` |  |  | `jc:floodZoneCategory` | The kind of zone of the Vltava and Berounka flood area: the active zone, where water flows through, where it stands, or ground the city or its owners protect. |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language, as the publisher writes it. |
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
