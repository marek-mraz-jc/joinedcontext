# CouncilMeeting, an extension of dataModel.TourismDestinations/Event

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A meeting of a council or another body of the self-government. Extends dataModel.TourismDestinations/Event: TourismDestinations Event is the official model of a dated happening; CPSV-AP has none for a council.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/CouncilMeeting`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### CouncilMeeting

A meeting of a council or another body of the self-government.

IRI: `jc:CouncilMeeting`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cZasadnutia` | Property | `integer` |  |  | `jc:cZasadnutia` | The source column `Č._zasadnutia`. |
| `cisloZasadnutia` | Property | `string` |  |  | `jc:cisloZasadnutia` | The source column `Číslo_zasadnutia`. |
| `datumZasadnutia` | Property | `string` |  |  | `jc:datumZasadnutia` | The source column `Dátum_zasadnutia`. |
| `hyperlinkNaDochadzku` | Property | `string` |  |  | `jc:hyperlinkNaDochadzku` | The source column `Hyperlink_na_dochádzku`. |
| `hyperlinkNaPozvanku` | Property | `string` |  |  | `jc:hyperlinkNaPozvanku` | The source column `Hyperlink_na_pozvánku`. |
| `hyperlinkNaPrijateUznesenia` | Property | `string` |  |  | `jc:hyperlinkNaPrijateUznesenia` | The source column `Hyperlink_na_prijaté_uznesenia`. |
| `hyperlinkNaVysledkyHlasovani` | Property | `string` |  |  | `jc:hyperlinkNaVysledkyHlasovani` | The source column `Hyperlink_na_výsledky_hlasovani`. |
| `hyperlinkNaZapisnicu` | Property | `string` |  |  | `jc:hyperlinkNaZapisnicu` | The source column `Hyperlink_na_zápisnicu`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `rok` | Property | `integer` |  |  | `jc:rok` | The source column `Rok`. |
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
