# FloodDefence, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One line of flood defences: what it is built of, who keeps it and the flood it is designed for. Extends dataModel.PointOfInterest/PointOfInterest: no Smart Data Model holds a flood defence structure.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/FloodDefence`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### FloodDefence

One line of flood defences: what it is built of, who keeps it and the flood it is designed for.

IRI: `jc:FloodDefence`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `defenceType` | Property | `string` |  |  | `jc:defenceType` | The kind of flood defence line, a fixed barrier, a mobile wall or sandbags. |
| `designFlood` | Property | `string` |  |  | `jc:designFlood` | The flood the line is designed to hold back, the 100-year flood (q100) or the August 2002 flood (flood2002). |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language, as the publisher writes it. |
| `protectedBy` | Property | `string` |  |  | `jc:protectedBy` | Who sets the line up and keeps it, the city or the owner of the ground behind it. |
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
