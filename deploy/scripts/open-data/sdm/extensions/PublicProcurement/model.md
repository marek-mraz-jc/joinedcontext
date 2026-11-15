# PublicProcurement, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A planned or realised public procurement of the publisher. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body pays or charges (value, currency, identifier); no subject holds a contract, order, invoice, grant or procurement.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/PublicProcurement`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### PublicProcurement

A planned or realised public procurement of the publisher.

IRI: `jc:PublicProcurement`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `adresaNajvyhodnejsiehoUchadza` | Property | `string` |  |  | `jc:adresaNajvyhodnejsiehoUchadza` | The source column `Adresa_najvýhodnejšieho_uchádza`. |
| `cPVCastiNazov` | Property | `string` |  |  | `jc:cPVCastiNazov` | The source column `CPV_časti___názov`. |
| `cPVNazov` | Property | `string` |  |  | `jc:cPVNazov` | The source column `CPV___názov`. |
| `cisloSpisu` | Property | `string` |  |  | `jc:cisloSpisu` | The source column `Číslo_spisu`. |
| `datumSpustenia` | Property | `datetime` |  |  | `jc:datumSpustenia` | The source column `Dátum_spustenia`. |
| `datumUkoncenia` | Property | `string` |  |  | `jc:datumUkoncenia` | The source column `Dátum_ukončenia`. |
| `druhKomoditAleboSluzieb` | Property | `string` |  |  | `jc:druhKomoditAleboSluzieb` | The source column `Druh_komodít_alebo_služieb`. |
| `druhObstaravania` | Property | `string` |  |  | `jc:druhObstaravania` | The source column `Druh_obstarávania`. |
| `druhPostupu` | Property | `string` |  |  | `jc:druhPostupu` | The source column `Druh_postupu`. |
| `elektronickaAukcia` | Property | `string` |  |  | `jc:elektronickaAukcia` | The source column `Elektronická_aukcia`. |
| `financovanie` | Property | `string` |  |  | `jc:financovanie` | The source column `Financovanie`. |
| `hlavnyCPV` | Property | `string` |  |  | `jc:hlavnyCPV` | The source column `Hlavný_CPV`. |
| `hlavnyCPVCasti` | Property | `string` |  |  | `jc:hlavnyCPVCasti` | The source column `Hlavný_CPV_časti`. |
| `iDZakazky` | Property | `integer` |  |  | `jc:iDZakazky` | The source column `ID_zákazky`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `id`, `ID_časti`. |
| `lehotaNaPredkladanie` | Property | `datetime` |  |  | `jc:lehotaNaPredkladanie` | The source column `Lehota_na_predkladanie`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `najvyhodnejsiUchadzac` | Property | `string` |  |  | `jc:najvyhodnejsiUchadzac` | The source column `Najvýhodnejší_uchádzač`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Predmet_zákazky`, `Názov_predmetu`. |
| `nazovCasti` | Property | `string` |  |  | `jc:nazovCasti` | The source column `Názov_časti`. |
| `obstaravatel` | Property | `string` |  |  | `jc:obstaravatel` | The source column `Obstarávateľ`. |
| `pHZ` | Property | `float` |  |  | `jc:pHZ` | The source column `PHZ`. |
| `pocetUchadzacov` | Property | `integer` |  |  | `jc:pocetUchadzacov` | The source column `Počet_uchádzačov`. |
| `pocetZaujemcov` | Property | `integer` |  |  | `jc:pocetZaujemcov` | The source column `Počet_záujemcov`. |
| `ponukaUchadzaca` | Property | `float` |  |  | `jc:ponukaUchadzaca` | The source column `Ponuka_uchádzača`. |
| `predpokladanaHodnota` | Property | `string` |  |  | `jc:predpokladanaHodnota` | The source column `Predpokladaná_hodnota`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `socialneZodpovedneObstaravani` | Property | `string` |  |  | `jc:socialneZodpovedneObstaravani` | The source column `Sociálne_zodpovedné_obstarávani`. |
| `spolocenskyZodpovedneVerejne` | Property | `string` |  |  | `jc:spolocenskyZodpovedneVerejne` | The source column `Spoločensky_zodpovedné_verejné_`. |
| `stav` | Property | `string` |  |  | `jc:stav` | The source column `Stav`. |
| `typKomodity` | Property | `string` |  |  | `jc:typKomodity` | The source column `Typ_komodity`. |
| `typVydavku` | Property | `string` |  |  | `jc:typVydavku` | The source column `Typ_výdavku`. |
| `utvarOvZP` | Property | `string` |  |  | `jc:utvarOvZP` | The source column `Útvar_OvZP`. |
| `zeleneObstaravanie` | Property | `string` |  |  | `jc:zeleneObstaravanie` | The source column `Zelené_obstarávanie`. |
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
