# RoadStructure, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

An underpass, culvert, expansion joint or level crossing on a regional road. Extends dataModel.PointOfInterest/PointOfInterest: as Bridge: a located structure of a road, not a road.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/RoadStructure`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### RoadStructure

An underpass, culvert, expansion joint or level crossing on a regional road.

IRI: `jc:RoadStructure`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `celkovaDlzkaPodjazdu` | Property | `integer` |  |  | `jc:celkovaDlzkaPodjazdu` | The source column `Celková_dĺžka_podjazdu`. |
| `cisloPK` | Property | `string` |  |  | `jc:cisloPK` | The source column `Číslo_PK`. |
| `druhKonstrukcie` | Property | `string` |  |  | `jc:druhKonstrukcie` | The source column `Druh_konštrukcie`. |
| `druhZabezpecenia` | Property | `string` |  |  | `jc:druhZabezpecenia` | The source column `Druh_zabezpečenia`. |
| `jICZSR` | Property | `string` |  |  | `jc:jICZSR` | The source column `JIČ_ŽSR`. |
| `kraj` | Property | `string` |  |  | `jc:kraj` | The source column `Kraj`. |
| `material` | Property | `string` |  |  | `jc:material` | The source column `Material`. |
| `materialPriepustu` | Property | `string` |  |  | `jc:materialPriepustu` | The source column `Materiál_priepustu`. |
| `minVyskaPodjazdu` | Property | `float` |  |  | `jc:minVyskaPodjazdu` | The source column `Min__výška_podjazdu`. |
| `najblizsiKM` | Property | `float` |  |  | `jc:najblizsiKM` | The source column `Najbližší_KM`, `Najbližši_KM`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `ID_DC`, `Identifikačné_číslo_podjazdu`, `ID_číslo_priepustu`, `Identifikacne_čislo_žel__priec_`. |
| `normanaZatazitelnostT` | Property | `integer` |  |  | `jc:normanaZatazitelnostT` | The source column `Normána_zaťažiteľnost___t`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `pocetKolaji` | Property | `integer` |  |  | `jc:pocetKolaji` | The source column `Počet_kolaji`. |
| `predmetPremostenia` | Property | `string` |  |  | `jc:predmetPremostenia` | The source column `Predmet_premostenia`. |
| `rokStavby` | Property | `integer` |  |  | `jc:rokStavby` | The source column `rok_stavby`, `rok_Stavby`. |
| `spravca` | Property | `string` |  |  | `jc:spravca` | The source column `Spravca`. |
| `spravcaMostu` | Property | `string` |  |  | `jc:spravcaMostu` | The source column `Správca_mostu`. |
| `spravcaUseku` | Property | `string` |  |  | `jc:spravcaUseku` | The source column `Správca_úseku`. |
| `stavebnoTechnickyStav` | Property | `string` |  |  | `jc:stavebnoTechnickyStav` | The source column `Stavebno_technický_stav`. |
| `triedaPK` | Property | `string` |  |  | `jc:triedaPK` | The source column `Trieda_PK`. |
| `typPodjazdu` | Property | `string` |  |  | `jc:typPodjazdu` | The source column `Typ_podjazdu`. |
| `typSpravcu` | Property | `string` |  |  | `jc:typSpravcu` | The source column `Typ_Spravcu`. |
| `vlastnik` | Property | `string` |  |  | `jc:vlastnik` | The source column `Vlastník`. |
| `vlastnikMostu` | Property | `string` |  |  | `jc:vlastnikMostu` | The source column `Vlastník_mostu`. |
| `vlastnikUseku` | Property | `string` |  |  | `jc:vlastnikUseku` | The source column `Vlastník_úseku`. |
| `vyhradnaZatazitelnostT` | Property | `string` |  |  | `jc:vyhradnaZatazitelnostT` | The source column `Výhradná_zaťažiteľnosť___t`. |
| `vynimocnaZatazitelnostT` | Property | `string` |  |  | `jc:vynimocnaZatazitelnostT` | The source column `Výnimočná_zaťažiteľnosť___t`. |
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
