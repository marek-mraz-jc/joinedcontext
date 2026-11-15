# Contract, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A contract the publisher signed and published. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body pays or charges (value, currency, identifier); no subject holds a contract, order, invoice, grant or procurement.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/Contract`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Contract

A contract the publisher signed and published.

IRI: `jc:Contract`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cenaCelkom` | Property | `string` |  |  | `jc:cenaCelkom` | The source column `Cena_celkom`. |
| `datumPlatnostiDo` | Property | `date` |  |  | `jc:datumPlatnostiDo` | The source column `Dátum_platnosti_do`. |
| `datumPodpisu` | Property | `string` |  |  | `jc:datumPodpisu` | The source column `Dátum_podpisu`. |
| `datumPrilohy` | Property | `date` |  |  | `jc:datumPrilohy` | The source column `Dátum_prílohy`. |
| `datumUcinnosti` | Property | `date` |  |  | `jc:datumUcinnosti` | The source column `Dátum_účinnosti`. |
| `datumUzavretia` | Property | `date` |  |  | `jc:datumUzavretia` | The source column `Dátum_uzavretia`. |
| `datumZverejnenia` | Property | `string` |  |  | `jc:datumZverejnenia` | The source column `Dátum_zverejnenia`. |
| `druh` | Property | `string` |  |  | `jc:druh` | The source column `Druh`. |
| `iCODodavatela` | Property | `string` |  |  | `jc:iCODodavatela` | The source column `IČO_dodávateľa`. |
| `iCOObjednavatela` | Property | `string` |  |  | `jc:iCOObjednavatela` | The source column `IČO_objednávateľa`. |
| `iCOOrganizacie` | Property | `string` |  |  | `jc:iCOOrganizacie` | The source column `IČO_organizácie`. |
| `iDZmluvy` | Property | `string` |  |  | `jc:iDZmluvy` | The source column `ID_zmluvy`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Centrálne_číslo_zmluvy`, `Číslo_zmluvy`. |
| `index` | Property | `string` |  |  | `jc:index` | The source column `Index_`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov_zmluvy`. |
| `nazovDodavatela` | Property | `string` |  |  | `jc:nazovDodavatela` | The source column `Názov_dodávateľa`. |
| `nazovObjednavatela` | Property | `string` |  |  | `jc:nazovObjednavatela` | The source column `Názov_objednávateľa`. |
| `poznamka` | Property | `string` |  |  | `jc:poznamka` | The source column `Poznámka`. |
| `predmet` | Property | `string` |  |  | `jc:predmet` | The source column `Predmet`. |
| `priloha` | Property | `string` |  |  | `jc:priloha` | The source column `Príloha`. |
| `rezort` | Property | `string` |  |  | `jc:rezort` | The source column `Rezort`. |
| `rok` | Property | `integer` |  |  | `jc:rok` | The source column `Rok`. |
| `sidloObjednavatela` | Property | `string` |  |  | `jc:sidloObjednavatela` | The source column `Sídlo_objednávateľa`. |
| `sumaSpoluVEUR` | Property | `string` |  |  | `jc:sumaSpoluVEUR` | The source column `Suma_spolu_v_EUR`. |
| `sumaVEUR` | Property | `string` |  |  | `jc:sumaVEUR` | The source column `Suma_v_EUR`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `Typ`. |
| `zdrojNazov` | Property | `string` |  |  | `jc:zdrojNazov` | The source column `Zdroj_Názov`. |
| `zmluvneStrany` | Property | `string` |  |  | `jc:zmluvneStrany` | The source column `Zmluvné_strany`. |
| `zmluvneStranySUdajomICO` | Property | `string` |  |  | `jc:zmluvneStranySUdajomICO` | The source column `Zmluvné_strany_s_údajom_IČO`. |
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
