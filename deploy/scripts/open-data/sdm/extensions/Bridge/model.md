# Bridge, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A bridge on a road the region owns or manages. Extends dataModel.PointOfInterest/PointOfInterest: Transportation Road and RoadSegment require roadClass, refRoad, startPoint and endPoint, which a structure register has not; PointOfInterest is a located named thing.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/Bridge`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Bridge

A bridge on a road the region owns or manages.

IRI: `jc:Bridge`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `aktualnyStavebnotechnickyStav` | Property | `string` |  |  | `jc:aktualnyStavebnotechnickyStav` | The source column `Aktuálny_stavebnotechnický_stav`. |
| `dlzkaPremosteniaM` | Property | `float` |  |  | `jc:dlzkaPremosteniaM` | The source column `Dĺžka_premostenia__m_`. |
| `docasnostMosta` | Property | `string` |  |  | `jc:docasnostMosta` | The source column `Dočasnosť_mosta`. |
| `druhNosnejKonstrukcie` | Property | `string` |  |  | `jc:druhNosnejKonstrukcie` | The source column `Druh_nosnej_konštrukcie`. |
| `hodnotenie` | Property | `string` |  |  | `jc:hodnotenie` | The source column `Hodnotenie`. |
| `identifikacneCislo` | Property | `string` |  |  | `jc:identifikacneCislo` | The source column `Identifikačné_číslo`. |
| `komunikacia` | Property | `string` |  |  | `jc:komunikacia` | The source column `Komunikácia`. |
| `legislativnyStav` | Property | `string` |  |  | `jc:legislativnyStav` | The source column `Legislatívny_stav`. |
| `materialNosnejKonstrukcie` | Property | `string` |  |  | `jc:materialNosnejKonstrukcie` | The source column `Materiál_nosnej_konštrukcie`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov`. |
| `normalnaZatazitelnostT` | Property | `string` |  |  | `jc:normalnaZatazitelnostT` | The source column `Normálna_zaťažiteľnosť__t_`. |
| `obec` | Property | `string` |  |  | `jc:obec` | The source column `Obec`. |
| `ojedinelaZatazitelnostT` | Property | `string` |  |  | `jc:ojedinelaZatazitelnostT` | The source column `Ojedinelá_zaťažiteľnosť__t_`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `pocetDilCelkov` | Property | `integer` |  |  | `jc:pocetDilCelkov` | The source column `Počet_dil__celkov`. |
| `predmetPremostenia` | Property | `string` |  |  | `jc:predmetPremostenia` | The source column `Predmet_premostenia`. |
| `rokKonstrukcie` | Property | `integer` |  |  | `jc:rokKonstrukcie` | The source column `Rok_konštrukcie`. |
| `rokZasadnejRekonstrukcie` | Property | `integer` |  |  | `jc:rokZasadnejRekonstrukcie` | The source column `Rok_zásadnej_rekonštrukcie`. |
| `spravca` | Property | `string` |  |  | `jc:spravca` | The source column `Správca`. |
| `triedaCesty` | Property | `string` |  |  | `jc:triedaCesty` | The source column `Trieda_cesty`. |
| `typKomunikacie` | Property | `string` |  |  | `jc:typKomunikacie` | The source column `Typ_komunikácie`. |
| `typPamiatky` | Property | `string` |  |  | `jc:typPamiatky` | The source column `Typ_pamiatky`. |
| `ulica` | Property | `string` |  |  | `jc:ulica` | The source column `Ulica`. |
| `vlastnik` | Property | `string` |  |  | `jc:vlastnik` | The source column `Vlastník`. |
| `vyhradnaZatazitelnostT` | Property | `string` |  |  | `jc:vyhradnaZatazitelnostT` | The source column `Výhradná_zaťažiteľnosť__t_`. |
| `vynimocnaZatazitelnostT` | Property | `string` |  |  | `jc:vynimocnaZatazitelnostT` | The source column `Výnimočná_zaťažiteľnosť__t_`. |
| `zatazitelnostNaJednuNapravu` | Property | `string` |  |  | `jc:zatazitelnostNaJednuNapravu` | The source column `Zaťažiteľnosť_na_jednu_nápravu_`. |
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
