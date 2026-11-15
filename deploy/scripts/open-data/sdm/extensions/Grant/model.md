# Grant, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A grant or subsidy the publisher awarded or paid out. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body pays or charges (value, currency, identifier); no subject holds a contract, order, invoice, grant or procurement.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/Grant`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Grant

A grant or subsidy the publisher awarded or paid out.

IRI: `jc:Grant`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `akcia` | Property | `string` |  |  | `jc:akcia` | The source column `Akcia`. |
| `cDodavatela` | Property | `string` |  |  | `jc:cDodavatela` | The source column `Č__dodávateľa`. |
| `cZmluvy` | Property | `string` |  |  | `jc:cZmluvy` | The source column `Č__zmluvy`. |
| `datumDoruceniaVyuctovania` | Property | `date` |  |  | `jc:datumDoruceniaVyuctovania` | The source column `Dátum_doručenia_vyúčtovania`. |
| `datumRealizacieProjektu` | Property | `string` |  |  | `jc:datumRealizacieProjektu` | The source column `Dátum_realizácie_projektu`. |
| `datumUhradyDotacie` | Property | `date` |  |  | `jc:datumUhradyDotacie` | The source column `Dátum_úhrady_dotácie`. |
| `datumZverejneniaZmluvy` | Property | `date` |  |  | `jc:datumZverejneniaZmluvy` | The source column `Dátum_zverejnenia_zmluvy`. |
| `denneTabory` | Property | `string` |  |  | `jc:denneTabory` | The source column `Denné_tábory`. |
| `dotacnyTitul` | Property | `string` |  |  | `jc:dotacnyTitul` | The source column `Dotačný_titul`. |
| `druh` | Property | `string` |  |  | `jc:druh` | The source column `Druh`. |
| `ePFRVSucet` | Property | `float` |  |  | `jc:ePFRVSucet` | The source column `EPFRV___súčet`. |
| `ePZFSucet` | Property | `float` |  |  | `jc:ePZFSucet` | The source column `EPZF___súčet`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `IČO`. |
| `iCOZiadatela` | Property | `string` |  |  | `jc:iCOZiadatela` | The source column `IČO_žiadateľa`. |
| `iD` | Property | `integer` |  |  | `jc:iD` | The source column `ID`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `kampan` | Property | `string` |  |  | `jc:kampan` | The source column `Kampaň`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov`. |
| `obecPO` | Property | `string` |  |  | `jc:obecPO` | The source column `Obec__PO_`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `opravneneSpolu` | Property | `integer` |  |  | `jc:opravneneSpolu` | The source column `Oprávnené_spolu`. |
| `pSC` | Property | `string` |  |  | `jc:pSC` | The source column `PSČ`. |
| `pobytoveTabory` | Property | `string` |  |  | `jc:pobytoveTabory` | The source column `Pobytové_tábory`. |
| `pocetDeti` | Property | `string` |  |  | `jc:pocetDeti` | The source column `Počet_detí`. |
| `pocetDni` | Property | `string` |  |  | `jc:pocetDni` | The source column `Počet_dní`. |
| `popisDotacnehoTitulu` | Property | `string` |  |  | `jc:popisDotacnehoTitulu` | The source column `Popis_dotačného_titulu`. |
| `pozadovanaDotacia` | Property | `integer` |  |  | `jc:pozadovanaDotacia` | The source column `Požadovaná_dotácia`. |
| `pozadovaneProstriedky` | Property | `string` |  |  | `jc:pozadovaneProstriedky` | The source column `Požadované_prostriedky`. |
| `poznamky` | Property | `string` |  |  | `jc:poznamky` | The source column `Poznámky`. |
| `pravnaForma` | Property | `string` |  |  | `jc:pravnaForma` | The source column `Právna_forma`. |
| `prveDorucenie` | Property | `date` |  |  | `jc:prveDorucenie` | The source column `Prvé_doručenie`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `rokDotacie` | Property | `integer` |  |  | `jc:rokDotacie` | The source column `Rok_dotácie`. |
| `rokKampan` | Property | `integer` |  |  | `jc:rokKampan` | The source column `Rok___kampaň`. |
| `rozdielNaVratenie` | Property | `float` |  |  | `jc:rozdielNaVratenie` | The source column `Rozdiel_na_vrátenie`. |
| `rozpocet` | Property | `float` |  |  | `jc:rozpocet` | The source column `Rozpočet`. |
| `sRSRSucet` | Property | `float` |  |  | `jc:sRSRSucet` | The source column `ŠR_SR___súčet`. |
| `schvProstr` | Property | `string` |  |  | `jc:schvProstr` | The source column `Schv._prostr.`. |
| `schvalenaVyskaDotacie` | Property | `float` |  |  | `jc:schvalenaVyskaDotacie` | The source column `Schválená_výška_dotácie`. |
| `sidlo` | Property | `string` |  |  | `jc:sidlo` | The source column `Sídlo`. |
| `skutocneCerpanie` | Property | `float` |  |  | `jc:skutocneCerpanie` | The source column `Skutočné_čerpanie`. |
| `stavFinanVyuctovania` | Property | `string` |  |  | `jc:stavFinanVyuctovania` | The source column `Stav_finan__vyúčtovania`. |
| `stavVybav` | Property | `string` |  |  | `jc:stavVybav` | The source column `Stav_vybav.`. |
| `stavZiadosti` | Property | `string` |  |  | `jc:stavZiadosti` | The source column `Stav_žiadosti`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `Typ`. |
| `typDotacie` | Property | `string` |  |  | `jc:typDotacie` | The source column `Typ_dotácie`. |
| `typTabora` | Property | `string` |  |  | `jc:typTabora` | The source column `Typ_tábora`. |
| `vSuladeSPrioritamiPoskytovaniaDotacii` | Property | `string` |  |  | `jc:vSuladeSPrioritamiPoskytovaniaDotacii` | The source column `V_súlade_s_prioritami_poskytovania_dotácií`. |
| `vZN` | Property | `string` |  |  | `jc:vZN` | The source column `VZN`. |
| `vratenaNedocerpanaCastDotacieDna` | Property | `string` |  |  | `jc:vratenaNedocerpanaCastDotacieDna` | The source column `Vrátená_nedočerpaná_časť_dotácie_dňa`. |
| `vyplatenaSumaSpoluSucet` | Property | `float` |  |  | `jc:vyplatenaSumaSpoluSucet` | The source column `Vyplatená_suma_spolu___súčet`. |
| `ziadatel` | Property | `string` |  |  | `jc:ziadatel` | The source column `Žiadateľ`. |
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
