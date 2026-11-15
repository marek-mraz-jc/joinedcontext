# HeritageArea, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One area of a city's cultural heritage map: its outline and its kind, such as a fortification or a historic park. Extends dataModel.PointOfInterest/PointOfInterest: PointOfInterest's category lists services and amenities, not the kinds of a heritage map, and no Smart Data Model holds a heritage area.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/HeritageArea`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### HeritageArea

One area of a city's cultural heritage map: its outline and its kind, such as a fortification or a historic park.

IRI: `jc:HeritageArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `description` | Property | `string` |  |  | `http://purl.org/dc/terms/description` | A short description, per language. |
| `heritageCategory` | Property | `string` |  |  | `jc:heritageCategory` | What kind of protected or valued cultural landscape the area is: a strongpoint of the First World War land fortifications, or one of the kinds of the city's landscape culture map. |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language. |
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
