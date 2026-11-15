# OfficeHours, an extension of dataModel.Organization/Organization

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One office's opening hours on one weekday. Extends dataModel.Organization/Organization: Organization is the office; the rows are its hours per weekday, which no model holds as an entity.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/OfficeHours`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### OfficeHours

One office's opening hours on one weekday.

IRI: `jc:OfficeHours`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `denVTyzdni` | Property | `string` |  |  | `jc:denVTyzdni` | The source column `Deň_v_týždni`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `identifikatorKontaktnehoMiest` | Property | `string` |  |  | `jc:identifikatorKontaktnehoMiest` | The source column `Identifikátor_kontaktného_miest`. |
| `kodDnaVTyzdni` | Property | `string` |  |  | `jc:kodDnaVTyzdni` | The source column `Kód_dňa_v_týždni`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov_kontaktného_miesta`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `otvoreneDo` | Property | `string` |  |  | `jc:otvoreneDo` | The source column `Otvorené_do`. |
| `otvoreneOd` | Property | `string` |  |  | `jc:otvoreneOd` | The source column `Otvorené_od`. |
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
