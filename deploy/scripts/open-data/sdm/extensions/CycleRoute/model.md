# CycleRoute, an extension of dataModel.Transportation/BikeLane

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A signed cycle touring route or one surface section of it. Extends dataModel.Transportation/BikeLane: Transportation BikeLane is the official model of a way for bicycles; OSMCycleRoute requires osmId.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/CycleRoute`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### CycleRoute

A signed cycle touring route or one surface section of it.

IRI: `jc:CycleRoute`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `alps` | Property | `string` |  |  | `jc:alps` | The source column `alps`. |
| `ctt` | Property | `string` |  |  | `jc:ctt` | The source column `ctt`. |
| `dlzka` | Property | `float` |  |  | `jc:dlzka` | The source column `dlzka`. |
| `fName` | Property | `string` |  |  | `jc:fName` | The source column `f_name`. |
| `fNazov` | Property | `string` |  |  | `jc:fNazov` | The source column `f_nazov`. |
| `farba` | Property | `string` |  |  | `jc:farba` | The source column `farba`. |
| `layer` | Property | `string` |  |  | `jc:layer` | The source column `layer`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `name`. |
| `nazov` | Property | `string` |  |  | `jc:nazov` | The source column `nazov`. |
| `obnRok` | Property | `string` |  |  | `jc:obnRok` | The source column `obn_rok`. |
| `povrch` | Property | `string` |  |  | `jc:povrch` | The source column `povrch`. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The source column `id`. |
| `spr` | Property | `string` |  |  | `jc:spr` | The source column `spr`. |
| `spravca` | Property | `string` |  |  | `jc:spravca` | The source column `spravca`. |
| `stav` | Property | `string` |  |  | `jc:stav` | The source column `stav`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `typ`. |
| `zdroj` | Property | `string` |  |  | `jc:zdroj` | The source column `zdroj`. |
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
