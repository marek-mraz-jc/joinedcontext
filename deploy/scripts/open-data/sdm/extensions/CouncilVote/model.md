# CouncilVote, an extension of dataModel.TourismDestinations/Event

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One deputy's vote on one item of a council meeting. Extends dataModel.TourismDestinations/Event: a vote is a dated happening of a meeting, so the meeting's base.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/CouncilVote`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### CouncilVote

One deputy's vote on one item of a council meeting.

IRI: `jc:CouncilVote`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datum` | Property | `datetime` |  |  | `jc:datum` | The source column `Dátum`. |
| `hlas` | Property | `string` |  |  | `jc:hlas` | The source column `Hlas`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Téma`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `poslanec` | Property | `string` |  |  | `jc:poslanec` | The source column `Poslanec`. |
| `volebneObdobie` | Property | `integer` |  |  | `jc:volebneObdobie` | The source column `Volebné_obdobie`. |
| `zasadnutie` | Property | `string` |  |  | `jc:zasadnutie` | The source column `Zasadnutie`. |
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
