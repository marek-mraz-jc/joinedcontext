# Banská Bystrica Region, catalogue tables

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

Every dataset of the Banská Bystrica Self-Governing Region in the catalogue (opendata.bbsk.sk).

- Namespace: `https://joinedcontext.com/models/bbsk/bbsk-otvorene-data`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AdministrativeArea

One row of Kraj BBK - hranica; Obce BBK - centroidy.

IRI: `jc:AdministrativeArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `dOW` | Property | `datetime` |  |  | `jc:dOW` | The source column `DOW`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fLAT` | Property | `string` |  |  | `jc:fLAT` | The source column `F_LAT`. |
| `fLON` | Property | `string` |  |  | `jc:fLON` | The source column `F_LON`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `iDN3` | Property | `integer` |  |  | `jc:iDN3` | The source column `IDN3`. |
| `iDN4` | Property | `integer` |  |  | `jc:iDN4` | The source column `IDN4`. |
| `lAU1` | Property | `string` |  |  | `jc:lAU1` | The source column `LAU1`. |
| `lAU1CODE` | Property | `string` |  |  | `jc:lAU1CODE` | The source column `LAU1_CODE`. |
| `lAU2` | Property | `string` |  |  | `jc:lAU2` | The source column `LAU2`. |
| `lAU2CODE` | Property | `string` |  |  | `jc:lAU2CODE` | The source column `LAU2_CODE`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `nM3` | Property | `string` |  |  | `jc:nM3` | The source column `NM3`. |
| `nUTS1` | Property | `string` |  |  | `jc:nUTS1` | The source column `NUTS1`. |
| `nUTS1CODE` | Property | `string` |  |  | `jc:nUTS1CODE` | The source column `NUTS1_CODE`. |
| `nUTS2` | Property | `string` |  |  | `jc:nUTS2` | The source column `NUTS2`. |
| `nUTS2CODE` | Property | `string` |  |  | `jc:nUTS2CODE` | The source column `NUTS2_CODE`. |
| `nUTS3` | Property | `string` |  |  | `jc:nUTS3` | The source column `NUTS3`. |
| `nUTS3CODE` | Property | `string` |  |  | `jc:nUTS3CODE` | The source column `NUTS3_CODE`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `typObce` | Property | `string` |  |  | `jc:typObce` | The source column `typ_obce`. |
| `vYMERA` | Property | `float` |  |  | `jc:vYMERA` | The source column `VYMERA`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Bridge

One row of Mosty v správe BBRSC (public view).

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `normalnaZatazitelnostT` | Property | `string` |  |  | `jc:normalnaZatazitelnostT` | The source column `Normálna_zaťažiteľnosť__t_`. |
| `obec` | Property | `string` |  |  | `jc:obec` | The source column `Obec`. |
| `ojedinelaZatazitelnostT` | Property | `string` |  |  | `jc:ojedinelaZatazitelnostT` | The source column `Ojedinelá_zaťažiteľnosť__t_`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `pocetDilCelkov` | Property | `integer` |  |  | `jc:pocetDilCelkov` | The source column `Počet_dil__celkov`. |
| `predmetPremostenia` | Property | `string` |  |  | `jc:predmetPremostenia` | The source column `Predmet_premostenia`. |
| `rokKonstrukcie` | Property | `integer` |  |  | `jc:rokKonstrukcie` | The source column `Rok_konštrukcie`. |
| `rokZasadnejRekonstrukcie` | Property | `integer` |  |  | `jc:rokZasadnejRekonstrukcie` | The source column `Rok_zásadnej_rekonštrukcie`. |
| `spravca` | Property | `string` |  |  | `jc:spravca` | The source column `spravca`. |
| `triedaCesty` | Property | `string` |  |  | `jc:triedaCesty` | The source column `Trieda_cesty`. |
| `typKomunikacie` | Property | `string` |  |  | `jc:typKomunikacie` | The source column `Typ_komunikácie`. |
| `typPamiatky` | Property | `string` |  |  | `jc:typPamiatky` | The source column `Typ_pamiatky`. |
| `ulica` | Property | `string` |  |  | `jc:ulica` | The source column `Ulica`. |
| `vlastnik` | Property | `string` |  |  | `jc:vlastnik` | The source column `Vlastník`. |
| `vyhradnaZatazitelnostT` | Property | `string` |  |  | `jc:vyhradnaZatazitelnostT` | The source column `Výhradná_zaťažiteľnosť___t`. |
| `vynimocnaZatazitelnostT` | Property | `string` |  |  | `jc:vynimocnaZatazitelnostT` | The source column `Výnimočná_zaťažiteľnosť___t`. |
| `zatazitelnostNaJednuNapravu` | Property | `string` |  |  | `jc:zatazitelnostNaJednuNapravu` | The source column `Zaťažiteľnosť_na_jednu_nápravu_`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CommitteeSeat

One row of Komisie zastupiteľstva Banskobystrického samosprávneho kraja.

IRI: `jc:CommitteeSeat`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `pozicia` | Property | `string` |  |  | `jc:pozicia` | The source column `Pozícia`. |
| `poznamka` | Property | `string` |  |  | `jc:poznamka` | The source column `Poznámka`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Contract

One row of Zmluvy.

IRI: `jc:Contract`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datumPlatnostiDo` | Property | `date` |  |  | `jc:datumPlatnostiDo` | The source column `Dátum_platnosti_do`. |
| `datumPrilohy` | Property | `date` |  |  | `jc:datumPrilohy` | The source column `Dátum_prílohy`. |
| `datumUcinnosti` | Property | `date` |  |  | `jc:datumUcinnosti` | The source column `Dátum_účinnosti`. |
| `datumUzavretia` | Property | `date` |  |  | `jc:datumUzavretia` | The source column `Dátum_uzavretia`. |
| `datumZverejnenia` | Property | `date` |  |  | `jc:datumZverejnenia` | The source column `Dátum_zverejnenia`. |
| `iCODodavatela` | Property | `string` |  |  | `jc:iCODodavatela` | The source column `IČO_dodávateľa`. |
| `iCOObjednavatela` | Property | `string` |  |  | `jc:iCOObjednavatela` | The source column `IČO_objednávateľa`. |
| `iDZmluvy` | Property | `string` |  |  | `jc:iDZmluvy` | The source column `ID_zmluvy`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `index` | Property | `string` |  |  | `jc:index` | The source column `Index_`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovDodavatela` | Property | `string` |  |  | `jc:nazovDodavatela` | The source column `Názov_dodávateľa`. |
| `nazovObjednavatela` | Property | `string` |  |  | `jc:nazovObjednavatela` | The source column `Názov_objednávateľa`. |
| `poznamka` | Property | `string` |  |  | `jc:poznamka` | The source column `Poznámka`. |
| `priloha` | Property | `string` |  |  | `jc:priloha` | The source column `Príloha`. |
| `rezort` | Property | `string` |  |  | `jc:rezort` | The source column `Rezort`. |
| `sidloObjednavatela` | Property | `string` |  |  | `jc:sidloObjednavatela` | The source column `Sídlo_objednávateľa`. |
| `sumaSpoluVEUR` | Property | `string` |  |  | `jc:sumaSpoluVEUR` | The source column `Suma_spolu_v_EUR`. |
| `sumaVEUR` | Property | `string` |  |  | `jc:sumaVEUR` | The source column `Suma_v_EUR`. |
| `zdrojNazov` | Property | `string` |  |  | `jc:zdrojNazov` | The source column `Zdroj_Názov`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CouncilMeeting

One row of Zasadnutia samosprávy.

IRI: `jc:CouncilMeeting`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cisloZasadnutia` | Property | `string` |  |  | `jc:cisloZasadnutia` | The source column `Číslo_zasadnutia`. |
| `datumZasadnutia` | Property | `datetime` |  |  | `jc:datumZasadnutia` | The source column `Dátum_zasadnutia`. |
| `hyperlinkNaDochadzku` | Property | `string` |  |  | `jc:hyperlinkNaDochadzku` | The source column `Hyperlink_na_dochádzku`. |
| `hyperlinkNaPozvanku` | Property | `string` |  |  | `jc:hyperlinkNaPozvanku` | The source column `Hyperlink_na_pozvánku`. |
| `hyperlinkNaPrijateUznesenia` | Property | `string` |  |  | `jc:hyperlinkNaPrijateUznesenia` | The source column `Hyperlink_na_prijaté_uznesenia`. |
| `hyperlinkNaVysledkyHlasovani` | Property | `string` |  |  | `jc:hyperlinkNaVysledkyHlasovani` | The source column `Hyperlink_na_výsledky_hlasovani`. |
| `hyperlinkNaZapisnicu` | Property | `string` |  |  | `jc:hyperlinkNaZapisnicu` | The source column `Hyperlink_na_zápisnicu`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CouncilVote

One row of Hlasovania poslancov.

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `poslanec` | Property | `string` |  |  | `jc:poslanec` | The source column `Poslanec`. |
| `volebneObdobie` | Property | `integer` |  |  | `jc:volebneObdobie` | The source column `Volebné_obdobie`. |
| `zasadnutie` | Property | `string` |  |  | `jc:zasadnutie` | The source column `Zasadnutie`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CycleRoute

One row of Cykloturistické trasy - línie; Cykloturistické trasy - línie - povrchy.

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
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

### Grant

One row of Dotácie; PPA platby.

IRI: `jc:Grant`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cDodavatela` | Property | `string` |  |  | `jc:cDodavatela` | The source column `Č__dodávateľa`. |
| `cZmluvy` | Property | `string` |  |  | `jc:cZmluvy` | The source column `Č__zmluvy`. |
| `datumDoruceniaVyuctovania` | Property | `date` |  |  | `jc:datumDoruceniaVyuctovania` | The source column `Dátum_doručenia_vyúčtovania`. |
| `datumRealizacieProjektu` | Property | `string` |  |  | `jc:datumRealizacieProjektu` | The source column `Dátum_realizácie_projektu`. |
| `datumUhradyDotacie` | Property | `date` |  |  | `jc:datumUhradyDotacie` | The source column `Dátum_úhrady_dotácie`. |
| `datumZverejneniaZmluvy` | Property | `date` |  |  | `jc:datumZverejneniaZmluvy` | The source column `Dátum_zverejnenia_zmluvy`. |
| `denneTabory` | Property | `string` |  |  | `jc:denneTabory` | The source column `Denné_tábory`. |
| `dotacnyTitul` | Property | `string` |  |  | `jc:dotacnyTitul` | The source column `Dotačný_titul`. |
| `ePFRVSucet` | Property | `float` |  |  | `jc:ePFRVSucet` | The source column `EPFRV___súčet`. |
| `ePZFSucet` | Property | `float` |  |  | `jc:ePZFSucet` | The source column `EPZF___súčet`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `iD` | Property | `integer` |  |  | `jc:iD` | The source column `ID`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `kampan` | Property | `string` |  |  | `jc:kampan` | The source column `Kampaň`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `obecPO` | Property | `string` |  |  | `jc:obecPO` | The source column `Obec__PO_`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `opravneneSpolu` | Property | `integer` |  |  | `jc:opravneneSpolu` | The source column `Oprávnené_spolu`. |
| `pSC` | Property | `string` |  |  | `jc:pSC` | The source column `PSC`. |
| `pobytoveTabory` | Property | `string` |  |  | `jc:pobytoveTabory` | The source column `Pobytové_tábory`. |
| `pocetDeti` | Property | `string` |  |  | `jc:pocetDeti` | The source column `Počet_detí`. |
| `pocetDni` | Property | `string` |  |  | `jc:pocetDni` | The source column `Počet_dní`. |
| `popisDotacnehoTitulu` | Property | `string` |  |  | `jc:popisDotacnehoTitulu` | The source column `Popis_dotačného_titulu`. |
| `pozadovanaDotacia` | Property | `integer` |  |  | `jc:pozadovanaDotacia` | The source column `Požadovaná_dotácia`. |
| `poznamky` | Property | `string` |  |  | `jc:poznamky` | The source column `Poznámky`. |
| `pravnaForma` | Property | `string` |  |  | `jc:pravnaForma` | The source column `Právna_forma`. |
| `prveDorucenie` | Property | `date` |  |  | `jc:prveDorucenie` | The source column `Prvé_doručenie`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `rokKampan` | Property | `integer` |  |  | `jc:rokKampan` | The source column `Rok___kampaň`. |
| `rozdielNaVratenie` | Property | `float` |  |  | `jc:rozdielNaVratenie` | The source column `Rozdiel_na_vrátenie`. |
| `rozpocet` | Property | `float` |  |  | `jc:rozpocet` | The source column `Rozpočet`. |
| `sRSRSucet` | Property | `float` |  |  | `jc:sRSRSucet` | The source column `ŠR_SR___súčet`. |
| `schvalenaVyskaDotacie` | Property | `float` |  |  | `jc:schvalenaVyskaDotacie` | The source column `Schválená_výška_dotácie`. |
| `sidlo` | Property | `string` |  |  | `jc:sidlo` | The source column `Sídlo`. |
| `skutocneCerpanie` | Property | `float` |  |  | `jc:skutocneCerpanie` | The source column `Skutočné_čerpanie`. |
| `stavFinanVyuctovania` | Property | `string` |  |  | `jc:stavFinanVyuctovania` | The source column `Stav_finan__vyúčtovania`. |
| `stavZiadosti` | Property | `string` |  |  | `jc:stavZiadosti` | The source column `Stav_žiadosti`. |
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

### Invoice

One row of Faktúry.

IRI: `jc:Invoice`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cisloObjednavky` | Property | `string` |  |  | `jc:cisloObjednavky` | The source column `Číslo_objednávky`. |
| `cisloZmluvy` | Property | `string` |  |  | `jc:cisloZmluvy` | The source column `Číslo_zmluvy`. |
| `dIC` | Property | `string` |  |  | `jc:dIC` | The source column `DIČ`. |
| `datumDorucenia` | Property | `date` |  |  | `jc:datumDorucenia` | The source column `Dátum_doručenia`. |
| `datumSplatnosti` | Property | `date` |  |  | `jc:datumSplatnosti` | The source column `Dátum_splatnosti`. |
| `dodavatel` | Property | `string` |  |  | `jc:dodavatel` | The source column `Dodávateľ`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `predmetFaktury` | Property | `string` |  |  | `jc:predmetFaktury` | The source column `Predmet_faktúry`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `suma` | Property | `float` |  |  | `jc:suma` | The source column `Suma`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KeyPerformanceIndicator

One row of Hustota obyvateľstva (2021); Hustota obyvateľstva v obciach BBK; Obyvateľstvo podľa ekonomickej aktivity (2021); Obyvateľstvo podľa materinského jazyka (2011), obce; Obyvateľstvo podľa materinského jazyka (2011), okresy; Obyvateľstvo podľa materinského jazyka (2021), obce; Obyvateľstvo podľa materinského jazyka (2021), okresy; Obyvateľstvo podľa náboženského vyznania (2011), obce; Obyvateľstvo pod.

IRI: `sdm:dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `abs20182040` | Property | `integer` |  |  | `jc:abs20182040` | The source column `abs___2018_2040_`. |
| `adHocHnutia` | Property | `float` |  |  | `jc:adHocHnutia` | The source column `ad_hoc_hnutia____`. |
| `adHocHnutia2` | Property | `float` |  |  | `jc:adHocHnutia2` | The source column `ad_hoc_hnutia__`. |
| `adHocHnutiaAbs` | Property | `integer` |  |  | `jc:adHocHnutiaAbs` | The source column `ad_hoc_hnutia__abs__`. |
| `albanska` | Property | `float` |  |  | `jc:albanska` | The source column `albánska____`. |
| `albanskaAbs` | Property | `integer` |  |  | `jc:albanskaAbs` | The source column `albánska__abs__`. |
| `albansky` | Property | `float` |  |  | `jc:albansky` | The source column `albánsky____`. |
| `albanskyAbs` | Property | `integer` |  |  | `jc:albanskyAbs` | The source column `albánsky__abs__`. |
| `anglicka` | Property | `float` |  |  | `jc:anglicka` | The source column `anglická____`. |
| `anglickaAbs` | Property | `integer` |  |  | `jc:anglickaAbs` | The source column `anglická__abs__`. |
| `anglicky` | Property | `float` |  |  | `jc:anglicky` | The source column `anglický____`. |
| `anglickyAbs` | Property | `integer` |  |  | `jc:anglickyAbs` | The source column `anglický__abs__`. |
| `apostolskaCirkev` | Property | `float` |  |  | `jc:apostolskaCirkev` | The source column `Apoštolská_cirkev`. |
| `apostolskaCirkev2` | Property | `float` |  |  | `jc:apostolskaCirkev2` | The source column `Apoštolská_cirkev__`. |
| `apostolskaCirkevNaSlovensku` | Property | `integer` |  |  | `jc:apostolskaCirkevNaSlovensku` | The source column `Apoštolská_cirkev_na_Slovensku_`. |
| `apostolskaCirkevNaSlovensku1` | Property | `float` |  |  | `jc:apostolskaCirkevNaSlovensku1` | The source column `Apoštolská_cirkev_na_Slovensku1`. |
| `apostolskaCirkevNaSlovensku2` | Property | `float` |  |  | `jc:apostolskaCirkevNaSlovensku2` | The source column `Apoštolská_cirkev_na_Slovensku_`. |
| `arabsky` | Property | `float` |  |  | `jc:arabsky` | The source column `arabský____`. |
| `arabskyAbs` | Property | `integer` |  |  | `jc:arabskyAbs` | The source column `arabský__abs__`. |
| `atribut` | Property | `integer` |  |  | `jc:atribut` | The source column `Atribút`. |
| `bahajskeSpolocenstvo` | Property | `float` |  |  | `jc:bahajskeSpolocenstvo` | The source column `Bahájske_spoločenstvo`. |
| `bahajskeSpolocenstvo2` | Property | `float` |  |  | `jc:bahajskeSpolocenstvo2` | The source column `Bahájske_spoločenstvo__`. |
| `bahajskeSpolocenstvoVSlove1` | Property | `float` |  |  | `jc:bahajskeSpolocenstvoVSlove1` | The source column `Bahájske_spoločenstvo_v_Slove_1`. |
| `bahajskeSpolocenstvoVSlovens` | Property | `integer` |  |  | `jc:bahajskeSpolocenstvoVSlovens` | The source column `Bahájske_spoločenstvo_v_Slovens`. |
| `bezNabozenskehoVyznania` | Property | `float` |  |  | `jc:bezNabozenskehoVyznania` | The source column `bez_náboženského_vyznania____`. |
| `bezNabozenskehoVyznania2` | Property | `float` |  |  | `jc:bezNabozenskehoVyznania2` | The source column `bez_náboženského_vyznania__`. |
| `bezNabozenskehoVyznaniaAbs` | Property | `integer` |  |  | `jc:bezNabozenskehoVyznaniaAbs` | The source column `bez_náboženského_vyznania__abs_`. |
| `bezSkolskehoVzdelaniaOso1` | Property | `float` |  |  | `jc:bezSkolskehoVzdelaniaOso1` | The source column `bez_školského_vzdelania___oso_1`. |
| `bezSkolskehoVzdelaniaOsoby` | Property | `integer` |  |  | `jc:bezSkolskehoVzdelaniaOsoby` | The source column `bez_školského_vzdelania___osoby`. |
| `bezUkoncenehoVzdelaniaOs1` | Property | `float` |  |  | `jc:bezUkoncenehoVzdelaniaOs1` | The source column `bez_ukončeného_vzdelania___os_1`. |
| `bezUkoncenehoVzdelaniaOsob` | Property | `integer` |  |  | `jc:bezUkoncenehoVzdelaniaOsob` | The source column `bez_ukončeného_vzdelania___osob`. |
| `bezVyznania` | Property | `float` |  |  | `jc:bezVyznania` | The source column `bez_vyznania`. |
| `bezVyznania2` | Property | `float` |  |  | `jc:bezVyznania2` | The source column `bez_vyznania__`. |
| `billeterovIndex` | Property | `float` |  |  | `jc:billeterovIndex` | The source column `Billeterov_index____`. |
| `bratskaJednotaBaptistov` | Property | `float` |  |  | `jc:bratskaJednotaBaptistov` | The source column `Bratská_jednota_baptistov`. |
| `bratskaJednotaBaptistov2` | Property | `float` |  |  | `jc:bratskaJednotaBaptistov2` | The source column `Bratská_jednota_baptistov__`. |
| `bratskaJednotaBaptistovVS1` | Property | `float` |  |  | `jc:bratskaJednotaBaptistovVS1` | The source column `Bratská_jednota_baptistov_v_S_1`. |
| `bratskaJednotaBaptistovVSlo` | Property | `integer` |  |  | `jc:bratskaJednotaBaptistovVSlo` | The source column `Bratská_jednota_baptistov_v_Slo`. |
| `budhizmus` | Property | `float` |  |  | `jc:budhizmus` | The source column `budhizmus____`. |
| `budhizmus2` | Property | `float` |  |  | `jc:budhizmus2` | The source column `budhizmus__`. |
| `budhizmusAbs` | Property | `integer` |  |  | `jc:budhizmusAbs` | The source column `budhizmus__abs__`. |
| `bulharska` | Property | `float` |  |  | `jc:bulharska` | The source column `bulharská____`. |
| `bulharska2` | Property | `float` |  |  | `jc:bulharska2` | The source column `bulharská__`. |
| `bulharskaAbs` | Property | `integer` |  |  | `jc:bulharskaAbs` | The source column `bulharská__abs__`. |
| `bulharsky` | Property | `float` |  |  | `jc:bulharsky` | The source column `bulharský`. |
| `bulharsky12` | Property | `float` |  |  | `jc:bulharsky12` | The source column `bulharský12`. |
| `bulharskyAbs` | Property | `integer` |  |  | `jc:bulharskyAbs` | The source column `bulharský__abs__`. |
| `celkovyPrirastokObyvatelstva` | Property | `float` |  |  | `jc:celkovyPrirastokObyvatelstva` | The source column `Celkový_prírastok_obyvateľstva_`. |
| `ceska` | Property | `float` |  |  | `jc:ceska` | The source column `česká____`. |
| `ceska2` | Property | `float` |  |  | `jc:ceska2` | The source column `česká__`. |
| `ceskaAbs` | Property | `integer` |  |  | `jc:ceskaAbs` | The source column `česká__abs__`. |
| `cesky` | Property | `float` |  |  | `jc:cesky` | The source column `český`. |
| `cesky7` | Property | `float` |  |  | `jc:cesky7` | The source column `český7`. |
| `ceskyAbs` | Property | `integer` |  |  | `jc:ceskyAbs` | The source column `český__abs__`. |
| `chemickaZlucenina` | Property | `string` |  |  | `jc:chemickaZlucenina` | The source column `chemická_zlúčenina`. |
| `chorvatska` | Property | `float` |  |  | `jc:chorvatska` | The source column `chorvátska____`. |
| `chorvatska2` | Property | `float` |  |  | `jc:chorvatska2` | The source column `chorvátska__`. |
| `chorvatskaAbs` | Property | `integer` |  |  | `jc:chorvatskaAbs` | The source column `chorvátska__abs__`. |
| `chorvatsky` | Property | `float` |  |  | `jc:chorvatsky` | The source column `chorvátsky`. |
| `chorvatsky10` | Property | `float` |  |  | `jc:chorvatsky10` | The source column `chorvátsky10`. |
| `chorvatskyAbs` | Property | `integer` |  |  | `jc:chorvatskyAbs` | The source column `chorvátsky__abs__`. |
| `cinska` | Property | `float` |  |  | `jc:cinska` | The source column `čínska____`. |
| `cinskaAbs` | Property | `integer` |  |  | `jc:cinskaAbs` | The source column `čínska__abs__`. |
| `cinsky` | Property | `float` |  |  | `jc:cinsky` | The source column `čínsky____`. |
| `cinskyAbs` | Property | `integer` |  |  | `jc:cinskyAbs` | The source column `čínsky__abs__`. |
| `cirkevAdventistovSiedmehoD1` | Property | `float` |  |  | `jc:cirkevAdventistovSiedmehoD1` | The source column `Cirkev_adventistov_siedmeho_d_1`. |
| `cirkevAdventistovSiedmehoDna` | Property | `float` |  |  | `jc:cirkevAdventistovSiedmehoDna` | The source column `Cirkev_adventistov_siedmeho_dňa`. |
| `cirkevBratska` | Property | `float` |  |  | `jc:cirkevBratska` | The source column `Cirkev_bratská`. |
| `cirkevBratska2` | Property | `float` |  |  | `jc:cirkevBratska2` | The source column `Cirkev_bratská__`. |
| `cirkevBratskaVSlovenskejR1` | Property | `float` |  |  | `jc:cirkevBratskaVSlovenskejR1` | The source column `Cirkev_bratská_v_Slovenskej_r_1`. |
| `cirkevBratskaVSlovenskejRep` | Property | `integer` |  |  | `jc:cirkevBratskaVSlovenskejRep` | The source column `Cirkev_bratská_v_Slovenskej_rep`. |
| `cirkevCeskoslovenskaHusitska` | Property | `float` |  |  | `jc:cirkevCeskoslovenskaHusitska` | The source column `Cirkev_československá_husitská`. |
| `cirkevCeskoslovenskaHusitska1` | Property | `float` |  |  | `jc:cirkevCeskoslovenskaHusitska1` | The source column `Cirkev_československá_husitská1`. |
| `cirkevCeskoslovenskaHusitska2` | Property | `float` |  |  | `jc:cirkevCeskoslovenskaHusitska2` | The source column `Cirkev_československá_husitská_`. |
| `cirkevJezisaKristaSvatych1` | Property | `float` |  |  | `jc:cirkevJezisaKristaSvatych1` | The source column `Cirkev_Ježiša_Krista_svätých__1`. |
| `cirkevJezisaKristaSvatychNe` | Property | `float` |  |  | `jc:cirkevJezisaKristaSvatychNe` | The source column `Cirkev_Ježiša_Krista_svätých_ne`. |
| `cisloOkresu` | Property | `string` |  |  | `jc:cisloOkresu` | The source column `Číslo_okresu`. |
| `dOW` | Property | `datetime` |  |  | `jc:dOW` | The source column `DOW`. |
| `dietaDoZacatiaPovinnejSko1` | Property | `float` |  |  | `jc:dietaDoZacatiaPovinnejSko1` | The source column `dieťa_do_začatia_povinnej_ško_1`. |
| `dietaDoZacatiaPovinnejSkols` | Property | `integer` |  |  | `jc:dietaDoZacatiaPovinnejSkols` | The source column `dieťa_do_začatia_povinnej_škols`. |
| `dochodca` | Property | `float` |  |  | `jc:dochodca` | The source column `dôchodca____`. |
| `dochodcaAbs` | Property | `integer` |  |  | `jc:dochodcaAbs` | The source column `dôchodca__abs__`. |
| `dojcenskaUmrtnostOsoba` | Property | `float` |  |  | `jc:dojcenskaUmrtnostOsoba` | The source column `Dojčenská_úmrtnosť__Osoba_`. |
| `ePFRV` | Property | `float` |  |  | `jc:ePFRV` | The source column `EPFRV`. |
| `ePZF` | Property | `float` |  |  | `jc:ePZF` | The source column `EPZF`. |
| `evanjelickaCirkevAugsburske1` | Property | `float` |  |  | `jc:evanjelickaCirkevAugsburske1` | The source column `Evanjelická_cirkev_augsburské_1`. |
| `evanjelickaCirkevAugsburskeho` | Property | `float` |  |  | `jc:evanjelickaCirkevAugsburskeho` | The source column `Evanjelická_cirkev_augsburského`. |
| `evanjelickaCirkevMetodistic1` | Property | `float` |  |  | `jc:evanjelickaCirkevMetodistic1` | The source column `Evanjelická_cirkev_metodistic_1`. |
| `evanjelickaCirkevMetodisticka` | Property | `float` |  |  | `jc:evanjelickaCirkevMetodisticka` | The source column `Evanjelická_cirkev_metodistická`. |
| `extremneHorucavyStupenOhroz` | Property | `integer` |  |  | `jc:extremneHorucavyStupenOhroz` | The source column `Extrémne_horúčavy__stupeň_ohroz`. |
| `extremneZrazkyStupenOhrozen` | Property | `integer` |  |  | `jc:extremneZrazkyStupenOhrozen` | The source column `Extrémne_zrážky__stupeň_ohrozen`. |
| `f1993` | Property | `integer` |  |  | `jc:f1993` | The source column `F1993`. |
| `f1994` | Property | `integer` |  |  | `jc:f1994` | The source column `F1994`. |
| `f1995` | Property | `integer` |  |  | `jc:f1995` | The source column `F1995`. |
| `f1996` | Property | `integer` |  |  | `jc:f1996` | The source column `F1996`. |
| `f1997` | Property | `integer` |  |  | `jc:f1997` | The source column `F1997`. |
| `f1998` | Property | `integer` |  |  | `jc:f1998` | The source column `F1998`. |
| `f1999` | Property | `integer` |  |  | `jc:f1999` | The source column `F1999`. |
| `f2000` | Property | `float` |  |  | `jc:f2000` | The source column `F2000`. |
| `f2001` | Property | `float` |  |  | `jc:f2001` | The source column `F2001`. |
| `f2002` | Property | `float` |  |  | `jc:f2002` | The source column `F2002`. |
| `f2003` | Property | `float` |  |  | `jc:f2003` | The source column `F2003`. |
| `f2004` | Property | `float` |  |  | `jc:f2004` | The source column `F2004`. |
| `f2005` | Property | `float` |  |  | `jc:f2005` | The source column `F2005`. |
| `f2006` | Property | `float` |  |  | `jc:f2006` | The source column `F2006`. |
| `f2007` | Property | `float` |  |  | `jc:f2007` | The source column `F2007`. |
| `f2008` | Property | `float` |  |  | `jc:f2008` | The source column `F2008`. |
| `f2009` | Property | `float` |  |  | `jc:f2009` | The source column `F2009`. |
| `f2010` | Property | `float` |  |  | `jc:f2010` | The source column `F2010`. |
| `f2011` | Property | `float` |  |  | `jc:f2011` | The source column `F2011`. |
| `f2012` | Property | `float` |  |  | `jc:f2012` | The source column `F2012`. |
| `f2013` | Property | `float` |  |  | `jc:f2013` | The source column `F2013`. |
| `f2014` | Property | `float` |  |  | `jc:f2014` | The source column `F2014`. |
| `f2015` | Property | `float` |  |  | `jc:f2015` | The source column `F2015`. |
| `f2016` | Property | `float` |  |  | `jc:f2016` | The source column `F2016`. |
| `f2017` | Property | `float` |  |  | `jc:f2017` | The source column `F2017`. |
| `f2018` | Property | `float` |  |  | `jc:f2018` | The source column `F2018`. |
| `f20182040` | Property | `float` |  |  | `jc:f20182040` | The source column `F___2018_2040_`. |
| `f2019` | Property | `float` |  |  | `jc:f2019` | The source column `F2019`. |
| `f2020` | Property | `float` |  |  | `jc:f2020` | The source column `F2020`. |
| `f2021` | Property | `float` |  |  | `jc:f2021` | The source column `F2021`. |
| `f2022` | Property | `float` |  |  | `jc:f2022` | The source column `F2022`. |
| `f2023` | Property | `float` |  |  | `jc:f2023` | The source column `F2023`. |
| `f2024` | Property | `float` |  |  | `jc:f2024` | The source column `F2024`. |
| `f2025` | Property | `float` |  |  | `jc:f2025` | The source column `F2025`. |
| `f2030` | Property | `integer` |  |  | `jc:f2030` | The source column `F2030`. |
| `f2035` | Property | `integer` |  |  | `jc:f2035` | The source column `F2035`. |
| `f2040` | Property | `integer` |  |  | `jc:f2040` | The source column `F2040`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fPoproduktivny` | Property | `float` |  |  | `jc:fPoproduktivny` | The source column `F__poproduktívny`. |
| `fPredproduktivny` | Property | `float` |  |  | `jc:fPredproduktivny` | The source column `F__predproduktívny`. |
| `fProduktivny` | Property | `float` |  |  | `jc:fProduktivny` | The source column `F__produktívny`. |
| `fRozdielZaporny` | Property | `float` |  |  | `jc:fRozdielZaporny` | The source column `F__rozdiel___zaporny`. |
| `francuzska` | Property | `float` |  |  | `jc:francuzska` | The source column `francúzska____`. |
| `francuzskaAbs` | Property | `integer` |  |  | `jc:francuzskaAbs` | The source column `francúzska__abs__`. |
| `francuzsky` | Property | `float` |  |  | `jc:francuzsky` | The source column `francúzsky____`. |
| `francuzskyAbs` | Property | `integer` |  |  | `jc:francuzskyAbs` | The source column `francúzsky__abs__`. |
| `grecka` | Property | `float` |  |  | `jc:grecka` | The source column `grécka____`. |
| `greckaAbs` | Property | `integer` |  |  | `jc:greckaAbs` | The source column `grécka__abs__`. |
| `greckokatolickaCirkev` | Property | `float` |  |  | `jc:greckokatolickaCirkev` | The source column `Gréckokatolícka_cirkev`. |
| `greckokatolickaCirkev2` | Property | `float` |  |  | `jc:greckokatolickaCirkev2` | The source column `Gréckokatolícka_cirkev__`. |
| `greckokatolickaCirkevNaSlo1` | Property | `float` |  |  | `jc:greckokatolickaCirkevNaSlo1` | The source column `Gréckokatolícka_cirkev_na_Slo_1`. |
| `greckokatolickaCirkevNaSlove` | Property | `integer` |  |  | `jc:greckokatolickaCirkevNaSlove` | The source column `Gréckokatolícka_cirkev_na_Slove`. |
| `hinduizmus` | Property | `float` |  |  | `jc:hinduizmus` | The source column `hinduizmus____`. |
| `hinduizmus2` | Property | `float` |  |  | `jc:hinduizmus2` | The source column `hinduizmus__`. |
| `hinduizmusAbs` | Property | `integer` |  |  | `jc:hinduizmusAbs` | The source column `hinduizmus__abs__`. |
| `hodnota` | Property | `float` |  |  | `jc:hodnota` | The source column `Hodnota`. |
| `hrubaMieraCelkovehoPrirastku` | Property | `float` |  |  | `jc:hrubaMieraCelkovehoPrirastku` | The source column `Hrubá_miera_celkového_prírastku`. |
| `hrubaMieraMigracnehoSaldaP` | Property | `float` |  |  | `jc:hrubaMieraMigracnehoSaldaP` | The source column `Hrubá_miera_migračného_salda__P`. |
| `hrubaMieraPotratovostiPromi` | Property | `float` |  |  | `jc:hrubaMieraPotratovostiPromi` | The source column `Hrubá_miera_potratovosti__Promi`. |
| `hrubaMieraPrirodzenehoPriras` | Property | `float` |  |  | `jc:hrubaMieraPrirodzenehoPriras` | The source column `Hrubá_miera_prirodzeného_príras`. |
| `hrubaMieraRozvodovostiPromi` | Property | `float` |  |  | `jc:hrubaMieraRozvodovostiPromi` | The source column `Hrubá_miera_rozvodovosti__Promi`. |
| `hrubaMieraSobasnostiPromile` | Property | `float` |  |  | `jc:hrubaMieraSobasnostiPromile` | The source column `Hrubá_miera_sobášnosti__Promile`. |
| `hrubaMieraUkoncenychTehotens` | Property | `float` |  |  | `jc:hrubaMieraUkoncenychTehotens` | The source column `Hrubá_miera_ukončených_tehotens`. |
| `hrubaMieraUmrtnostiPromile` | Property | `float` |  |  | `jc:hrubaMieraUmrtnostiPromile` | The source column `Hrubá_miera_úmrtnosti__Promile_`. |
| `hrubaMieraZivorodenostiProm` | Property | `float` |  |  | `jc:hrubaMieraZivorodenostiProm` | The source column `Hrubá_miera_živorodenosti__Prom`. |
| `hustotaOsobyKm` | Property | `float` |  |  | `jc:hustotaOsobyKm` | The source column `Hustota__osoby_km__`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `iDN3` | Property | `integer` |  |  | `jc:iDN3` | The source column `IDN3`. |
| `iDN4` | Property | `integer` |  |  | `jc:iDN4` | The source column `IDN4`. |
| `ina` | Property | `float` |  |  | `jc:ina` | The source column `iná____`. |
| `inaAbs` | Property | `integer` |  |  | `jc:inaAbs` | The source column `iná__abs__`. |
| `indexEkonomickehoZatazenia` | Property | `float` |  |  | `jc:indexEkonomickehoZatazenia` | The source column `Index_ekonomického_zaťaženia___`. |
| `indexEkonomickehoZatazeniaOs` | Property | `float` |  |  | `jc:indexEkonomickehoZatazeniaOs` | The source column `Index_ekonomického_zaťaženia_os`. |
| `indexEkonomickejZavislostiMl` | Property | `float` |  |  | `jc:indexEkonomickejZavislostiMl` | The source column `Index_ekonomickej_závislosti_ml`. |
| `indexEkonomickejZavislostiSt` | Property | `float` |  |  | `jc:indexEkonomickejZavislostiSt` | The source column `Index_ekonomickej_závislosti_st`. |
| `indexPotratovostiPercento` | Property | `float` |  |  | `jc:indexPotratovostiPercento` | The source column `Index_potratovosti__Percento_`. |
| `indexRozvodovostiPercento` | Property | `float` |  |  | `jc:indexRozvodovostiPercento` | The source column `Index_rozvodovosti__Percento_`. |
| `indexStarnutia` | Property | `string` |  |  | `jc:indexStarnutia` | The source column `Index_starnutia____`. |
| `indexStarnutiaPercento` | Property | `float` |  |  | `jc:indexStarnutiaPercento` | The source column `Index_starnutia__Percento_`. |
| `ine` | Property | `float` |  |  | `jc:ine` | The source column `iné`. |
| `ine2` | Property | `float` |  |  | `jc:ine2` | The source column `iné__`. |
| `ineAbs` | Property | `integer` |  |  | `jc:ineAbs` | The source column `iné__abs__`. |
| `iny` | Property | `float` |  |  | `jc:iny` | The source column `iný____`. |
| `inyAbs` | Property | `integer` |  |  | `jc:inyAbs` | The source column `iný__abs__`. |
| `iranska` | Property | `float` |  |  | `jc:iranska` | The source column `iránska____`. |
| `iranskaAbs` | Property | `integer` |  |  | `jc:iranskaAbs` | The source column `iránska__abs__`. |
| `irska` | Property | `float` |  |  | `jc:irska` | The source column `írska____`. |
| `irskaAbs` | Property | `integer` |  |  | `jc:irskaAbs` | The source column `írska__abs__`. |
| `islam` | Property | `float` |  |  | `jc:islam` | The source column `islam____`. |
| `islam2` | Property | `float` |  |  | `jc:islam2` | The source column `islam__`. |
| `islamAbs` | Property | `integer` |  |  | `jc:islamAbs` | The source column `islam__abs__`. |
| `jidis` | Property | `float` |  |  | `jc:jidis` | The source column `jidiš`. |
| `jidis11` | Property | `float` |  |  | `jc:jidis11` | The source column `jidiš11`. |
| `jidisAleboHebrejsky` | Property | `float` |  |  | `jc:jidisAleboHebrejsky` | The source column `jidiš_alebo_hebrejský____`. |
| `jidisAleboHebrejskyAbs` | Property | `integer` |  |  | `jc:jidisAleboHebrejskyAbs` | The source column `jidiš_alebo_hebrejský__abs__`. |
| `kODNUTSObec` | Property | `string` |  |  | `jc:kODNUTSObec` | The source column `KOD_NUTS_obec`. |
| `kODNUTSOkres` | Property | `string` |  |  | `jc:kODNUTSOkres` | The source column `KOD_NUTS_okres`. |
| `kODSTATOkres` | Property | `string` |  |  | `jc:kODSTATOkres` | The source column `KOD_STAT_okres`. |
| `kanadska` | Property | `float` |  |  | `jc:kanadska` | The source column `kanadská____`. |
| `kanadskaAbs` | Property | `integer` |  |  | `jc:kanadskaAbs` | The source column `kanadská__abs__`. |
| `kod` | Property | `string` |  |  | `jc:kod` | The source column `kod`. |
| `kodElementu` | Property | `string` |  |  | `jc:kodElementu` | The source column `Kód_elementu`. |
| `kodElementu3` | Property | `integer` |  |  | `jc:kodElementu3` | The source column `Kód_elementu_3`. |
| `kodKraja` | Property | `string` |  |  | `jc:kodKraja` | The source column `Kód_kraja`. |
| `kodObce` | Property | `string` |  |  | `jc:kodObce` | The source column `Kód_obce`. |
| `kodOblasti` | Property | `string` |  |  | `jc:kodOblasti` | The source column `Kód_oblasti`. |
| `kodOkresu` | Property | `string` |  |  | `jc:kodOkresu` | The source column `Kód_okresu`. |
| `kodStatu` | Property | `string` |  |  | `jc:kodStatu` | The source column `Kód_štátu`. |
| `korejska` | Property | `float` |  |  | `jc:korejska` | The source column `kórejská____`. |
| `korejskaAbs` | Property | `integer` |  |  | `jc:korejskaAbs` | The source column `kórejská__abs__`. |
| `korejsky` | Property | `float` |  |  | `jc:korejsky` | The source column `kórejský____`. |
| `korejskyAbs` | Property | `integer` |  |  | `jc:korejskyAbs` | The source column `kórejský__abs__`. |
| `kraj` | Property | `string` |  |  | `jc:kraj` | The source column `Kraj`. |
| `krajDlhyNazov` | Property | `string` |  |  | `jc:krajDlhyNazov` | The source column `Kraj_dlhý_názov`. |
| `krajID` | Property | `string` |  |  | `jc:krajID` | The source column `Kraj_ID`. |
| `krestanskeZbory` | Property | `float` |  |  | `jc:krestanskeZbory` | The source column `Kresťanské_zbory`. |
| `krestanskeZbory2` | Property | `float` |  |  | `jc:krestanskeZbory2` | The source column `Kresťanské_zbory__`. |
| `krestanskeZboryNaSlovensku` | Property | `integer` |  |  | `jc:krestanskeZboryNaSlovensku` | The source column `Kresťanské_zbory_na_Slovensku__`. |
| `krestanskeZboryNaSlovensku1` | Property | `float` |  |  | `jc:krestanskeZboryNaSlovensku1` | The source column `Kresťanské_zbory_na_Slovensku_1`. |
| `krestanskeZboryNaSlovensku2` | Property | `float` |  |  | `jc:krestanskeZboryNaSlovensku2` | The source column `Kresťanské_zbory_na_Slovensku__`. |
| `lAU1` | Property | `string` |  |  | `jc:lAU1` | The source column `LAU1`. |
| `lAU1CODE` | Property | `string` |  |  | `jc:lAU1CODE` | The source column `LAU1_CODE`. |
| `lAU2` | Property | `string` |  |  | `jc:lAU2` | The source column `LAU2`. |
| `lAU2CODE` | Property | `string` |  |  | `jc:lAU2CODE` | The source column `LAU2_CODE`. |
| `linkPdf` | Property | `string` |  |  | `jc:linkPdf` | The source column `Link_pdf`. |
| `madarska` | Property | `float` |  |  | `jc:madarska` | The source column `maďarská____`. |
| `madarska2` | Property | `float` |  |  | `jc:madarska2` | The source column `maďarská__`. |
| `madarskaAbs` | Property | `integer` |  |  | `jc:madarskaAbs` | The source column `maďarská__abs__`. |
| `madarsky` | Property | `float` |  |  | `jc:madarsky` | The source column `maďarský`. |
| `madarsky3` | Property | `float` |  |  | `jc:madarsky3` | The source column `maďarský3`. |
| `madarskyAbs` | Property | `integer` |  |  | `jc:madarskyAbs` | The source column `maďarský__abs__`. |
| `medianovyVek` | Property | `float` |  |  | `jc:medianovyVek` | The source column `Mediánový_vek`. |
| `medianovyVekRok` | Property | `float` |  |  | `jc:medianovyVekRok` | The source column `Mediánový_vek__Rok_`. |
| `meraciaStanica` | Property | `string` |  |  | `jc:meraciaStanica` | The source column `Meracia_stanica`. |
| `mieraDojcenskejUmrtnostiPro` | Property | `float` |  |  | `jc:mieraDojcenskejUmrtnostiPro` | The source column `Miera_dojčenskej_úmrtnosti__Pro`. |
| `mieraNovorodeneckejUmrtnosti` | Property | `float` |  |  | `jc:mieraNovorodeneckejUmrtnosti` | The source column `Miera_novorodeneckej_úmrtnosti_`. |
| `mieraPerinatalnejUmrtnostiP` | Property | `float` |  |  | `jc:mieraPerinatalnejUmrtnostiP` | The source column `Miera_perinatálnej_úmrtnosti__P`. |
| `migracneSaldoOsoba` | Property | `float` |  |  | `jc:migracneSaldoOsoba` | The source column `Migračné_saldo__Osoba_`. |
| `moravska` | Property | `float` |  |  | `jc:moravska` | The source column `moravská____`. |
| `moravska2` | Property | `float` |  |  | `jc:moravska2` | The source column `moravská__`. |
| `moravskaAbs` | Property | `integer` |  |  | `jc:moravskaAbs` | The source column `moravská__abs__`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `nM3` | Property | `string` |  |  | `jc:nM3` | The source column `NM3`. |
| `nM4` | Property | `string` |  |  | `jc:nM4` | The source column `NM4`. |
| `nUTS1` | Property | `string` |  |  | `jc:nUTS1` | The source column `NUTS1`. |
| `nUTS1CODE` | Property | `string` |  |  | `jc:nUTS1CODE` | The source column `NUTS1_CODE`. |
| `nUTS2` | Property | `string` |  |  | `jc:nUTS2` | The source column `NUTS2`. |
| `nUTS2CODE` | Property | `string` |  |  | `jc:nUTS2CODE` | The source column `NUTS2_CODE`. |
| `nUTS3` | Property | `string` |  |  | `jc:nUTS3` | The source column `NUTS3`. |
| `nUTS3CODE` | Property | `string` |  |  | `jc:nUTS3CODE` | The source column `NUTS3_CODE`. |
| `nabozenskaSpolocnostJehovovi` | Property | `float` |  |  | `jc:nabozenskaSpolocnostJehovovi` | The source column `Náboženská_spoločnosť_Jehovovi_`. |
| `nabozenskaSpolocnostJehovovi1` | Property | `float` |  |  | `jc:nabozenskaSpolocnostJehovovi1` | The source column `Náboženská_spoločnosť_Jehovovi1`. |
| `narodeniOsoba` | Property | `float` |  |  | `jc:narodeniOsoba` | The source column `Narodení__Osoba_`. |
| `nazovKraja` | Property | `string` |  |  | `jc:nazovKraja` | The source column `Názov_kraja`. |
| `nazovObce` | Property | `string` |  |  | `jc:nazovObce` | The source column `Názov_obce`. |
| `nazovOblasti` | Property | `string` |  |  | `jc:nazovOblasti` | The source column `Názov_oblasti`. |
| `nazovOkresu` | Property | `string` |  |  | `jc:nazovOkresu` | The source column `Názov_okresu`. |
| `nazovStatu` | Property | `string` |  |  | `jc:nazovStatu` | The source column `Názov_štátu`. |
| `nemecka` | Property | `float` |  |  | `jc:nemecka` | The source column `nemecká____`. |
| `nemecka2` | Property | `float` |  |  | `jc:nemecka2` | The source column `nemecká__`. |
| `nemeckaAbs` | Property | `integer` |  |  | `jc:nemeckaAbs` | The source column `nemecká__abs__`. |
| `nemecky` | Property | `float` |  |  | `jc:nemecky` | The source column `nemecký`. |
| `nemecky8` | Property | `float` |  |  | `jc:nemecky8` | The source column `nemecký8`. |
| `nemeckyAbs` | Property | `integer` |  |  | `jc:nemeckyAbs` | The source column `nemecký__abs__`. |
| `nezamestnany` | Property | `float` |  |  | `jc:nezamestnany` | The source column `nezamestnaný____`. |
| `nezamestnanyAbs` | Property | `integer` |  |  | `jc:nezamestnanyAbs` | The source column `nezamestnaný__abs__`. |
| `nezistena` | Property | `float` |  |  | `jc:nezistena` | The source column `nezistená____`. |
| `nezistena2` | Property | `float` |  |  | `jc:nezistena2` | The source column `nezistená__`. |
| `nezistenaAbs` | Property | `integer` |  |  | `jc:nezistenaAbs` | The source column `nezistená__abs__`. |
| `nezistene` | Property | `float` |  |  | `jc:nezistene` | The source column `nezistené____`. |
| `nezistene2` | Property | `float` |  |  | `jc:nezistene2` | The source column `nezistené__`. |
| `nezisteneAbs` | Property | `integer` |  |  | `jc:nezisteneAbs` | The source column `nezistené__abs__`. |
| `nezisteny` | Property | `float` |  |  | `jc:nezisteny` | The source column `nezistený`. |
| `nezisteny14` | Property | `float` |  |  | `jc:nezisteny14` | The source column `nezistený14`. |
| `nezistenyAbs` | Property | `integer` |  |  | `jc:nezistenyAbs` | The source column `nezistený__abs__`. |
| `novoapostolskaCirkev` | Property | `float` |  |  | `jc:novoapostolskaCirkev` | The source column `Novoapoštolská_cirkev`. |
| `novoapostolskaCirkev2` | Property | `float` |  |  | `jc:novoapostolskaCirkev2` | The source column `Novoapoštolská_cirkev__`. |
| `novoapostolskaCirkevVSlove1` | Property | `float` |  |  | `jc:novoapostolskaCirkevVSlove1` | The source column `Novoapoštolská_cirkev_v_Slove_1`. |
| `novoapostolskaCirkevVSlovens` | Property | `integer` |  |  | `jc:novoapostolskaCirkevVSlovens` | The source column `Novoapoštolská_cirkev_v_Slovens`. |
| `novorodeneckaUmrtnostOsoba` | Property | `float` |  |  | `jc:novorodeneckaUmrtnostOsoba` | The source column `Novorodenecká_úmrtnosť__Osoba_`. |
| `nuts` | Property | `string` |  |  | `jc:nuts` | The source column `nuts`. |
| `nutsKraj` | Property | `string` |  |  | `jc:nutsKraj` | The source column `nuts_kraj`. |
| `nutsObec` | Property | `string` |  |  | `jc:nutsObec` | The source column `nuts_obec`. |
| `nutsOkres` | Property | `string` |  |  | `jc:nutsOkres` | The source column `nuts_okres`. |
| `obec` | Property | `string` |  |  | `jc:obec` | The source column `Obec`. |
| `obecDlhyNazov` | Property | `string` |  |  | `jc:obecDlhyNazov` | The source column `Obec_dlhy_nazov`. |
| `obecID` | Property | `string` |  |  | `jc:obecID` | The source column `Obec_ID`. |
| `obecKratkyNazov` | Property | `string` |  |  | `jc:obecKratkyNazov` | The source column `obec_kratkyNazov`. |
| `obecNazovDlhy` | Property | `string` |  |  | `jc:obecNazovDlhy` | The source column `Obec_názov_dlhý`. |
| `obecNazovKratky` | Property | `string` |  |  | `jc:obecNazovKratky` | The source column `Obec_názov_krátky`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `okresDlhyNazov` | Property | `string` |  |  | `jc:okresDlhyNazov` | The source column `Okres_dlhy_nazov`. |
| `okresID` | Property | `string` |  |  | `jc:okresID` | The source column `Okres_ID`. |
| `osobaNaMaterskejDovolenke` | Property | `float` |  |  | `jc:osobaNaMaterskejDovolenke` | The source column `osoba_na_materskej_dovolenke___`. |
| `osobaNaMaterskejDovolenkeA` | Property | `integer` |  |  | `jc:osobaNaMaterskejDovolenkeA` | The source column `osoba_na_materskej_dovolenke__a`. |
| `osobaNaRodicovskejDovolenke` | Property | `integer` |  |  | `jc:osobaNaRodicovskejDovolenke` | The source column `osoba_na_rodičovskej_dovolenke_`. |
| `osobaNaRodicovskejDovolenke1` | Property | `float` |  |  | `jc:osobaNaRodicovskejDovolenke1` | The source column `osoba_na_rodičovskej_dovolenke1`. |
| `osobaVDomacnosti` | Property | `float` |  |  | `jc:osobaVDomacnosti` | The source column `osoba_v_domácnosti____`. |
| `osobaVDomacnostiAbs` | Property | `integer` |  |  | `jc:osobaVDomacnostiAbs` | The source column `osoba_v_domácnosti__abs__`. |
| `ostatne` | Property | `float` |  |  | `jc:ostatne` | The source column `ostatné`. |
| `ostatne13` | Property | `float` |  |  | `jc:ostatne13` | The source column `ostatné13`. |
| `ostatne2` | Property | `float` |  |  | `jc:ostatne2` | The source column `ostatné__`. |
| `ostatneANepresneUrceneKre1` | Property | `float` |  |  | `jc:ostatneANepresneUrceneKre1` | The source column `ostatné_a_nepresne_určené_kre_1`. |
| `ostatneANepresneUrceneKrest` | Property | `integer` |  |  | `jc:ostatneANepresneUrceneKrest` | The source column `ostatné_a_nepresne_určené_kresť`. |
| `perinatalnaUmrtnostOsoba` | Property | `float` |  |  | `jc:perinatalnaUmrtnostOsoba` | The source column `Perinatálna_úmrtnosť__Osoba_`. |
| `perzsky` | Property | `float` |  |  | `jc:perzsky` | The source column `perzský____`. |
| `perzskyAbs` | Property | `integer` |  |  | `jc:perzskyAbs` | The source column `perzský__abs__`. |
| `photo` | Property | `string` |  |  | `jc:photo` | The source column `Photo`. |
| `pocetObyvatelovSpolu` | Property | `integer` |  |  | `jc:pocetObyvatelovSpolu` | The source column `Počet_obyvateľov_spolu`. |
| `pocetObyvatelstva` | Property | `integer` |  |  | `jc:pocetObyvatelstva` | The source column `Počet_obyvateľstva`. |
| `podielOsobVPoproduktivnomVe` | Property | `float` |  |  | `jc:podielOsobVPoproduktivnomVe` | The source column `Podiel_osôb_v_poproduktívnom_ve`. |
| `podielOsobVPredproduktivnom` | Property | `float` |  |  | `jc:podielOsobVPredproduktivnom` | The source column `Podiel_osôb_v_predproduktívnom_`. |
| `podielOsobVProduktivnomVeku` | Property | `float` |  |  | `jc:podielOsobVProduktivnomVeku` | The source column `Podiel_osôb_v_produktívnom_veku`. |
| `pohanstvoAPrirodneDuchovno` | Property | `integer` |  |  | `jc:pohanstvoAPrirodneDuchovno` | The source column `pohanstvo_a_prírodné_duchovno__`. |
| `pohanstvoAPrirodneDuchovno1` | Property | `float` |  |  | `jc:pohanstvoAPrirodneDuchovno1` | The source column `pohanstvo_a_prírodné_duchovno_1`. |
| `pohanstvoAPrirodneDuchovno2` | Property | `float` |  |  | `jc:pohanstvoAPrirodneDuchovno2` | The source column `pohanstvo_a_prírodné_duchovno__`. |
| `pohlavie` | Property | `string` |  |  | `jc:pohlavie` | The source column `Pohlavie`. |
| `polska` | Property | `float` |  |  | `jc:polska` | The source column `poľská____`. |
| `polska2` | Property | `float` |  |  | `jc:polska2` | The source column `poľská__`. |
| `polskaAbs` | Property | `integer` |  |  | `jc:polskaAbs` | The source column `poľská__abs__`. |
| `polsky` | Property | `float` |  |  | `jc:polsky` | The source column `poľský`. |
| `polsky9` | Property | `float` |  |  | `jc:polsky9` | The source column `poľský9`. |
| `polskyAbs` | Property | `integer` |  |  | `jc:polskyAbs` | The source column `poľský__abs__`. |
| `poproduktivny` | Property | `integer` |  |  | `jc:poproduktivny` | The source column `poproduktívny`. |
| `poproduktivnyVek` | Property | `float` |  |  | `jc:poproduktivnyVek` | The source column `Poproduktívny_vek____`. |
| `poproduktivnyVekPocetObyvat` | Property | `integer` |  |  | `jc:poproduktivnyVekPocetObyvat` | The source column `Poproduktívny_vek__počet_obyvat`. |
| `potratyPocetVJednotkach` | Property | `float` |  |  | `jc:potratyPocetVJednotkach` | The source column `Potraty__Počet_v_jednotkách_`. |
| `pracujuciDochodca` | Property | `float` |  |  | `jc:pracujuciDochodca` | The source column `pracujúci_dôchodca____`. |
| `pracujuciDochodcaAbs` | Property | `integer` |  |  | `jc:pracujuciDochodcaAbs` | The source column `pracujúci_dôchodca__abs__`. |
| `pracujuciOkremDochodcov` | Property | `float` |  |  | `jc:pracujuciOkremDochodcov` | The source column `pracujúci__okrem_dôchodcov_____`. |
| `pracujuciOkremDochodcovAb` | Property | `integer` |  |  | `jc:pracujuciOkremDochodcovAb` | The source column `pracujúci__okrem_dôchodcov___ab`. |
| `pravoslavnaCirkev` | Property | `float` |  |  | `jc:pravoslavnaCirkev` | The source column `Pravoslávna_cirkev`. |
| `pravoslavnaCirkev2` | Property | `float` |  |  | `jc:pravoslavnaCirkev2` | The source column `Pravoslávna_cirkev__`. |
| `pravoslavnaCirkevNaSlovens1` | Property | `float` |  |  | `jc:pravoslavnaCirkevNaSlovens1` | The source column `Pravoslávna_cirkev_na_Slovens_1`. |
| `pravoslavnaCirkevNaSlovensku` | Property | `integer` |  |  | `jc:pravoslavnaCirkevNaSlovensku` | The source column `Pravoslávna_cirkev_na_Slovensku`. |
| `predproduktivny` | Property | `integer` |  |  | `jc:predproduktivny` | The source column `predproduktívny`. |
| `predproduktivnyVek` | Property | `float` |  |  | `jc:predproduktivnyVek` | The source column `Predproduktívny_vek____`. |
| `predproduktivnyVekPocetObyv` | Property | `integer` |  |  | `jc:predproduktivnyVekPocetObyv` | The source column `Predproduktívny_vek__počet_obyv`. |
| `priemernyVek` | Property | `float` |  |  | `jc:priemernyVek` | The source column `Priemerný_vek`. |
| `priemernyVekObyvatelaRok` | Property | `float` |  |  | `jc:priemernyVekObyvatelaRok` | The source column `Priemerný_vek_obyvateľa__Rok_`. |
| `prijemcaKapitalovychPrijmov` | Property | `integer` |  |  | `jc:prijemcaKapitalovychPrijmov` | The source column `príjemca_kapitálových_príjmov__`. |
| `prijemcaKapitalovychPrijmov1` | Property | `float` |  |  | `jc:prijemcaKapitalovychPrijmov1` | The source column `príjemca_kapitálových_príjmov_1`. |
| `prirodzenyPrirastokObyvatelst` | Property | `float` |  |  | `jc:prirodzenyPrirastokObyvatelst` | The source column `Prirodzený_prírastok_obyvateľst`. |
| `pristahovaniNaTrvalyPobytO` | Property | `float` |  |  | `jc:pristahovaniNaTrvalyPobytO` | The source column `Prisťahovaní_na_trvalý_pobyt__O`. |
| `produktivny` | Property | `integer` |  |  | `jc:produktivny` | The source column `produktívny`. |
| `produktivnyVek` | Property | `float` |  |  | `jc:produktivnyVek` | The source column `Produktívny_vek____`. |
| `produktivnyVekPocetObyvatel` | Property | `integer` |  |  | `jc:produktivnyVekPocetObyvatel` | The source column `Produktívny_vek__počet_obyvateľ`. |
| `rakuska` | Property | `float` |  |  | `jc:rakuska` | The source column `rakúska____`. |
| `rakuskaAbs` | Property | `integer` |  |  | `jc:rakuskaAbs` | The source column `rakúska__abs__`. |
| `reformovanaKrestanskaCirkev` | Property | `float` |  |  | `jc:reformovanaKrestanskaCirkev` | The source column `Reformovaná_kresťanská_cirkev`. |
| `reformovanaKrestanskaCirkev1` | Property | `float` |  |  | `jc:reformovanaKrestanskaCirkev1` | The source column `Reformovaná_kresťanská_cirkev_1`. |
| `reformovanaKrestanskaCirkev2` | Property | `float` |  |  | `jc:reformovanaKrestanskaCirkev2` | The source column `Reformovaná_kresťanská_cirkev__`. |
| `reformovanaKrestanskaCirkevN` | Property | `integer` |  |  | `jc:reformovanaKrestanskaCirkevN` | The source column `Reformovaná_kresťanská_cirkev_n`. |
| `rimskokatolickaCirkev` | Property | `float` |  |  | `jc:rimskokatolickaCirkev` | The source column `Rímskokatolícka_cirkev`. |
| `rimskokatolickaCirkev2` | Property | `float` |  |  | `jc:rimskokatolickaCirkev2` | The source column `Rímskokatolícka_cirkev__`. |
| `rimskokatolickaCirkevVSlov1` | Property | `float` |  |  | `jc:rimskokatolickaCirkevVSlov1` | The source column `Rímskokatolícka_cirkev_v_Slov_1`. |
| `rimskokatolickaCirkevVSloven` | Property | `integer` |  |  | `jc:rimskokatolickaCirkevVSloven` | The source column `Rímskokatolícka_cirkev_v_Sloven`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `rokKampan` | Property | `integer` |  |  | `jc:rokKampan` | The source column `Rok___kampaň`. |
| `romska` | Property | `float` |  |  | `jc:romska` | The source column `rómska____`. |
| `romska2` | Property | `float` |  |  | `jc:romska2` | The source column `rómska__`. |
| `romskaAbs` | Property | `integer` |  |  | `jc:romskaAbs` | The source column `rómska__abs__`. |
| `romsky` | Property | `float` |  |  | `jc:romsky` | The source column `rómsky`. |
| `romsky4` | Property | `float` |  |  | `jc:romsky4` | The source column `rómsky4`. |
| `romskyAbs` | Property | `integer` |  |  | `jc:romskyAbs` | The source column `rómsky__abs__`. |
| `rozvedenyRozvedena` | Property | `float` |  |  | `jc:rozvedenyRozvedena` | The source column `rozvedený__rozvedená____`. |
| `rozvedenyRozvedenaAbs` | Property | `integer` |  |  | `jc:rozvedenyRozvedenaAbs` | The source column `rozvedený__rozvedená__abs__`. |
| `rozvodyPocetVJednotkach` | Property | `float` |  |  | `jc:rozvodyPocetVJednotkach` | The source column `Rozvody__Počet_v_jednotkách_`. |
| `rumunska` | Property | `float` |  |  | `jc:rumunska` | The source column `rumunská____`. |
| `rumunskaAbs` | Property | `integer` |  |  | `jc:rumunskaAbs` | The source column `rumunská__abs__`. |
| `rumunsky` | Property | `float` |  |  | `jc:rumunsky` | The source column `rumunský____`. |
| `rumunskyAbs` | Property | `integer` |  |  | `jc:rumunskyAbs` | The source column `rumunský__abs__`. |
| `rusinska` | Property | `float` |  |  | `jc:rusinska` | The source column `rusínska____`. |
| `rusinska2` | Property | `float` |  |  | `jc:rusinska2` | The source column `rusínska__`. |
| `rusinskaAbs` | Property | `integer` |  |  | `jc:rusinskaAbs` | The source column `rusínska__abs__`. |
| `rusinsky` | Property | `float` |  |  | `jc:rusinsky` | The source column `rusínsky`. |
| `rusinsky5` | Property | `float` |  |  | `jc:rusinsky5` | The source column `rusínsky5`. |
| `rusinskyAbs` | Property | `integer` |  |  | `jc:rusinskyAbs` | The source column `rusínsky__abs__`. |
| `ruska` | Property | `float` |  |  | `jc:ruska` | The source column `ruská____`. |
| `ruska2` | Property | `float` |  |  | `jc:ruska2` | The source column `ruská__`. |
| `ruskaAbs` | Property | `integer` |  |  | `jc:ruskaAbs` | The source column `ruská__abs__`. |
| `rusky` | Property | `float` |  |  | `jc:rusky` | The source column `ruský____`. |
| `ruskyAbs` | Property | `integer` |  |  | `jc:ruskyAbs` | The source column `ruský__abs__`. |
| `sRSR` | Property | `float` |  |  | `jc:sRSR` | The source column `ŠR_SR_`. |
| `sTATKODObec` | Property | `string` |  |  | `jc:sTATKODObec` | The source column `STAT_KOD_obec`. |
| `sliezska` | Property | `float` |  |  | `jc:sliezska` | The source column `sliezska____`. |
| `sliezskaAbs` | Property | `integer` |  |  | `jc:sliezskaAbs` | The source column `sliezska__abs__`. |
| `slobodnySlobodna` | Property | `float` |  |  | `jc:slobodnySlobodna` | The source column `slobodný__slobodná____`. |
| `slobodnySlobodnaAbs` | Property | `integer` |  |  | `jc:slobodnySlobodnaAbs` | The source column `slobodný__slobodná__abs__`. |
| `slovenska` | Property | `float` |  |  | `jc:slovenska` | The source column `slovenská____`. |
| `slovenska2` | Property | `float` |  |  | `jc:slovenska2` | The source column `slovenská__`. |
| `slovenskaAbs` | Property | `integer` |  |  | `jc:slovenskaAbs` | The source column `slovenská__abs__`. |
| `slovensky` | Property | `float` |  |  | `jc:slovensky` | The source column `slovenský`. |
| `slovensky2` | Property | `float` |  |  | `jc:slovensky2` | The source column `slovenský2`. |
| `slovenskyAbs` | Property | `integer` |  |  | `jc:slovenskyAbs` | The source column `slovenský__abs__`. |
| `slovenskyPosunkovyJazyk` | Property | `float` |  |  | `jc:slovenskyPosunkovyJazyk` | The source column `slovenský_posunkový_jazyk____`. |
| `slovenskyPosunkovyJazykAbs` | Property | `integer` |  |  | `jc:slovenskyPosunkovyJazykAbs` | The source column `slovenský_posunkový_jazyk__abs_`. |
| `sobasePocetVJednotkach` | Property | `float` |  |  | `jc:sobasePocetVJednotkach` | The source column `Sobáše__Počet_v_jednotkách_`. |
| `spanielsky` | Property | `float` |  |  | `jc:spanielsky` | The source column `španielsky____`. |
| `spanielskyAbs` | Property | `integer` |  |  | `jc:spanielskyAbs` | The source column `španielsky__abs__`. |
| `spolu` | Property | `integer` |  |  | `jc:spolu` | The source column `Spolu`. |
| `srbska` | Property | `float` |  |  | `jc:srbska` | The source column `srbská____`. |
| `srbska2` | Property | `float` |  |  | `jc:srbska2` | The source column `srbská__`. |
| `srbskaAbs` | Property | `integer` |  |  | `jc:srbskaAbs` | The source column `srbská__abs__`. |
| `srbsky` | Property | `float` |  |  | `jc:srbsky` | The source column `srbský____`. |
| `srbskyAbs` | Property | `integer` |  |  | `jc:srbskyAbs` | The source column `srbský__abs__`. |
| `standardnaDojcenskaUmrtnost` | Property | `float` |  |  | `jc:standardnaDojcenskaUmrtnost` | The source column `Štandardná_dojčenská_úmrtnosť__`. |
| `standardnaNovorodeneckaUmrtno` | Property | `float` |  |  | `jc:standardnaNovorodeneckaUmrtno` | The source column `Štandardná_novorodenecká_úmrtno`. |
| `starokatolickaCirkev` | Property | `float` |  |  | `jc:starokatolickaCirkev` | The source column `Starokatolícka_cirkev`. |
| `starokatolickaCirkev2` | Property | `float` |  |  | `jc:starokatolickaCirkev2` | The source column `Starokatolícka_cirkev__`. |
| `starokatolickaCirkevNaSlov1` | Property | `float` |  |  | `jc:starokatolickaCirkevNaSlov1` | The source column `Starokatolícka_cirkev_na_Slov_1`. |
| `starokatolickaCirkevNaSloven` | Property | `integer` |  |  | `jc:starokatolickaCirkevNaSloven` | The source column `Starokatolícka_cirkev_na_Sloven`. |
| `stavTrvaleByvajucehoObyvat1` | Property | `float` |  |  | `jc:stavTrvaleByvajucehoObyvat1` | The source column `Stav_trvale_bývajúceho_obyvat_1`. |
| `stavTrvaleByvajucehoObyvat2` | Property | `float` |  |  | `jc:stavTrvaleByvajucehoObyvat2` | The source column `Stav_trvale_bývajúceho_obyvat_2`. |
| `stavTrvaleByvajucehoObyvatel` | Property | `float` |  |  | `jc:stavTrvaleByvajucehoObyvatel` | The source column `Stav_trvale_bývajúceho_obyvateľ`. |
| `stredneOdborneUcnovskeVz1` | Property | `float` |  |  | `jc:stredneOdborneUcnovskeVz1` | The source column `stredné_odborné__učňovské__vz_1`. |
| `stredneOdborneUcnovskeVzde` | Property | `integer` |  |  | `jc:stredneOdborneUcnovskeVzde` | The source column `stredné_odborné__učňovské__vzde`. |
| `studentVysokejSkoly` | Property | `float` |  |  | `jc:studentVysokejSkoly` | The source column `študent_vysokej_školy____`. |
| `studentVysokejSkolyAbs` | Property | `integer` |  |  | `jc:studentVysokejSkolyAbs` | The source column `študent_vysokej_školy__abs__`. |
| `sucetZAlbanskyAbs` | Property | `integer` |  |  | `jc:sucetZAlbanskyAbs` | The source column `Súčet_z_albánsky__abs__`. |
| `sucetZAnglickyAbs` | Property | `integer` |  |  | `jc:sucetZAnglickyAbs` | The source column `Súčet_z_anglický__abs__`. |
| `sucetZArabskyAbs` | Property | `integer` |  |  | `jc:sucetZArabskyAbs` | The source column `Súčet_z_arabský__abs__`. |
| `sucetZBulharskyAbs` | Property | `integer` |  |  | `jc:sucetZBulharskyAbs` | The source column `Súčet_z_bulharský__abs__`. |
| `sucetZCeskyAbs` | Property | `integer` |  |  | `jc:sucetZCeskyAbs` | The source column `Súčet_z_český__abs__`. |
| `sucetZChorvatskyAbs` | Property | `integer` |  |  | `jc:sucetZChorvatskyAbs` | The source column `Súčet_z_chorvátsky__abs__`. |
| `sucetZCinskyAbs` | Property | `integer` |  |  | `jc:sucetZCinskyAbs` | The source column `Súčet_z_čínsky__abs__`. |
| `sucetZFrancuzskyAbs` | Property | `integer` |  |  | `jc:sucetZFrancuzskyAbs` | The source column `Súčet_z_francúzsky__abs__`. |
| `sucetZInyAbs` | Property | `integer` |  |  | `jc:sucetZInyAbs` | The source column `Súčet_z_iný__abs__`. |
| `sucetZJidisAleboHebrejsky` | Property | `integer` |  |  | `jc:sucetZJidisAleboHebrejsky` | The source column `Súčet_z_jidiš_alebo_hebrejský__`. |
| `sucetZKorejskyAbs` | Property | `integer` |  |  | `jc:sucetZKorejskyAbs` | The source column `Súčet_z_kórejský__abs__`. |
| `sucetZMadarskyAbs` | Property | `integer` |  |  | `jc:sucetZMadarskyAbs` | The source column `Súčet_z_maďarský__abs__`. |
| `sucetZNemeckyAbs` | Property | `integer` |  |  | `jc:sucetZNemeckyAbs` | The source column `Súčet_z_nemecký__abs__`. |
| `sucetZNezistenyAbs` | Property | `integer` |  |  | `jc:sucetZNezistenyAbs` | The source column `Súčet_z_nezistený__abs__`. |
| `sucetZPerzskyAbs` | Property | `integer` |  |  | `jc:sucetZPerzskyAbs` | The source column `Súčet_z_perzský__abs__`. |
| `sucetZPolskyAbs` | Property | `integer` |  |  | `jc:sucetZPolskyAbs` | The source column `Súčet_z_poľský__abs__`. |
| `sucetZRomskyAbs` | Property | `integer` |  |  | `jc:sucetZRomskyAbs` | The source column `Súčet_z_rómsky__abs__`. |
| `sucetZRumunskyAbs` | Property | `integer` |  |  | `jc:sucetZRumunskyAbs` | The source column `Súčet_z_rumunský__abs__`. |
| `sucetZRusinskyAbs` | Property | `integer` |  |  | `jc:sucetZRusinskyAbs` | The source column `Súčet_z_rusínsky__abs__`. |
| `sucetZRuskyAbs` | Property | `integer` |  |  | `jc:sucetZRuskyAbs` | The source column `Súčet_z_ruský__abs__`. |
| `sucetZSlovenskyAbs` | Property | `integer` |  |  | `jc:sucetZSlovenskyAbs` | The source column `Súčet_z_slovenský__abs__`. |
| `sucetZSlovenskyPosunkovyJaz` | Property | `integer` |  |  | `jc:sucetZSlovenskyPosunkovyJaz` | The source column `Súčet_z_slovenský_posunkový_jaz`. |
| `sucetZSpanielskyAbs` | Property | `integer` |  |  | `jc:sucetZSpanielskyAbs` | The source column `Súčet_z_španielsky__abs__`. |
| `sucetZSrbskyAbs` | Property | `integer` |  |  | `jc:sucetZSrbskyAbs` | The source column `Súčet_z_srbský__abs__`. |
| `sucetZTalianskyAbs` | Property | `integer` |  |  | `jc:sucetZTalianskyAbs` | The source column `Súčet_z_taliansky__abs__`. |
| `sucetZTureckyAbs` | Property | `integer` |  |  | `jc:sucetZTureckyAbs` | The source column `Súčet_z_turecký__abs__`. |
| `sucetZUkrajinskyAbs` | Property | `integer` |  |  | `jc:sucetZUkrajinskyAbs` | The source column `Súčet_z_ukrajinský__abs__`. |
| `sucetZVietnamskyAbs` | Property | `integer` |  |  | `jc:sucetZVietnamskyAbs` | The source column `Súčet_z_vietnamský__abs__`. |
| `suchoStupenOhrozenia` | Property | `integer` |  |  | `jc:suchoStupenOhrozenia` | The source column `Sucho__stupeň_ohrozenia`. |
| `talianska` | Property | `float` |  |  | `jc:talianska` | The source column `talianska____`. |
| `talianskaAbs` | Property | `integer` |  |  | `jc:talianskaAbs` | The source column `talianska__abs__`. |
| `taliansky` | Property | `float` |  |  | `jc:taliansky` | The source column `taliansky____`. |
| `talianskyAbs` | Property | `integer` |  |  | `jc:talianskyAbs` | The source column `taliansky__abs__`. |
| `trvaloByvajuceObyvatelstvoSp` | Property | `integer` |  |  | `jc:trvaloByvajuceObyvatelstvoSp` | The source column `Trvalo_bývajúce_obyvateľstvo_sp`. |
| `turecka` | Property | `float` |  |  | `jc:turecka` | The source column `turecká____`. |
| `tureckaAbs` | Property | `integer` |  |  | `jc:tureckaAbs` | The source column `turecká__abs__`. |
| `turecky` | Property | `float` |  |  | `jc:turecky` | The source column `turecký____`. |
| `tureckyAbs` | Property | `integer` |  |  | `jc:tureckyAbs` | The source column `turecký__abs__`. |
| `ukazovatel` | Property | `string` |  |  | `jc:ukazovatel` | The source column `ukazovatel`. |
| `ukonceneTehotenstvaPocetVJ` | Property | `float` |  |  | `jc:ukonceneTehotenstvaPocetVJ` | The source column `Ukončené_tehotenstvá__Počet_v_j`. |
| `ukrajinska` | Property | `float` |  |  | `jc:ukrajinska` | The source column `ukrajinská____`. |
| `ukrajinska2` | Property | `float` |  |  | `jc:ukrajinska2` | The source column `ukrajinská__`. |
| `ukrajinskaAbs` | Property | `integer` |  |  | `jc:ukrajinskaAbs` | The source column `ukrajinská__abs__`. |
| `ukrajinsky` | Property | `float` |  |  | `jc:ukrajinsky` | The source column `ukrajinský`. |
| `ukrajinsky6` | Property | `float` |  |  | `jc:ukrajinsky6` | The source column `ukrajinský6`. |
| `ukrajinskyAbs` | Property | `integer` |  |  | `jc:ukrajinskyAbs` | The source column `ukrajinský__abs__`. |
| `uplneStredneVzdelanieSMa1` | Property | `float` |  |  | `jc:uplneStredneVzdelanieSMa1` | The source column `úplné_stredné_vzdelanie__s_ma_1`. |
| `uplneStredneVzdelanieSMatu` | Property | `integer` |  |  | `jc:uplneStredneVzdelanieSMatu` | The source column `úplné_stredné_vzdelanie__s_matu`. |
| `ustrednyZvazZidovskychNabo1` | Property | `float` |  |  | `jc:ustrednyZvazZidovskychNabo1` | The source column `Ústredný_zväz_židovských_nábo_1`. |
| `ustrednyZvazZidovskychNaboze` | Property | `float` |  |  | `jc:ustrednyZvazZidovskychNaboze` | The source column `Ústredný_zväz_židovských_nábože`. |
| `uzemnaJednotka` | Property | `string` |  |  | `jc:uzemnaJednotka` | The source column `Územná_jednotka`. |
| `vYMERA` | Property | `float` |  |  | `jc:vYMERA` | The source column `VYMERA`. |
| `vdovecVdova` | Property | `float` |  |  | `jc:vdovecVdova` | The source column `vdovec__vdova____`. |
| `vdovecVdovaAbs` | Property | `integer` |  |  | `jc:vdovecVdovaAbs` | The source column `vdovec__vdova__abs__`. |
| `vietnamska` | Property | `float` |  |  | `jc:vietnamska` | The source column `vietnamská____`. |
| `vietnamskaAbs` | Property | `integer` |  |  | `jc:vietnamskaAbs` | The source column `vietnamská__abs__`. |
| `vietnamsky` | Property | `float` |  |  | `jc:vietnamsky` | The source column `vietnamský____`. |
| `vietnamskyAbs` | Property | `integer` |  |  | `jc:vietnamskyAbs` | The source column `vietnamský__abs__`. |
| `vyplatenaSumaNa1Obyvatela` | Property | `float` |  |  | `jc:vyplatenaSumaNa1Obyvatela` | The source column `Vyplatená_suma_na_1_obyvateľa`. |
| `vyplatenaSumaSpolu` | Property | `float` |  |  | `jc:vyplatenaSumaSpolu` | The source column `Vyplatená_suma_spolu`. |
| `vysokoskolskeVzdelanie` | Property | `float` |  |  | `jc:vysokoskolskeVzdelanie` | The source column `vysokoškolské_vzdelanie____`. |
| `vysokoskolskeVzdelanieAbs` | Property | `integer` |  |  | `jc:vysokoskolskeVzdelanieAbs` | The source column `vysokoškolské_vzdelanie__abs__`. |
| `vyssieOdborneVzdelanie` | Property | `float` |  |  | `jc:vyssieOdborneVzdelanie` | The source column `vyššie_odborné_vzdelanie____`. |
| `vyssieOdborneVzdelanieAbs` | Property | `integer` |  |  | `jc:vyssieOdborneVzdelanieAbs` | The source column `vyššie_odborné_vzdelanie__abs__`. |
| `vystahovaniZTrvalehoPobytu` | Property | `float` |  |  | `jc:vystahovaniZTrvalehoPobytu` | The source column `Vysťahovaní_z_trvalého_pobytu__`. |
| `x` | Property | `float` |  |  | `jc:x` | The source column `x`. |
| `y` | Property | `float` |  |  | `jc:y` | The source column `y`. |
| `zakladneVzdelanie` | Property | `float` |  |  | `jc:zakladneVzdelanie` | The source column `základné_vzdelanie____`. |
| `zakladneVzdelanieAbs` | Property | `integer` |  |  | `jc:zakladneVzdelanieAbs` | The source column `základné_vzdelanie__abs__`. |
| `zenatyVydata` | Property | `float` |  |  | `jc:zenatyVydata` | The source column `ženatý__vydatá____`. |
| `zenatyVydataAbs` | Property | `integer` |  |  | `jc:zenatyVydataAbs` | The source column `ženatý__vydatá__abs__`. |
| `ziakStrednejSkoly` | Property | `float` |  |  | `jc:ziakStrednejSkoly` | The source column `žiak_strednej_školy____`. |
| `ziakStrednejSkolyAbs` | Property | `integer` |  |  | `jc:ziakStrednejSkolyAbs` | The source column `žiak_strednej_školy__abs__`. |
| `ziakZakladnejSkoly` | Property | `float` |  |  | `jc:ziakZakladnejSkoly` | The source column `žiak_základnej_školy____`. |
| `ziakZakladnejSkolyAbs` | Property | `integer` |  |  | `jc:ziakZakladnejSkolyAbs` | The source column `žiak_základnej_školy__abs__`. |
| `zidovska` | Property | `float` |  |  | `jc:zidovska` | The source column `židovská____`. |
| `zidovska2` | Property | `float` |  |  | `jc:zidovska2` | The source column `židovská__`. |
| `zidovskaAbs` | Property | `integer` |  |  | `jc:zidovskaAbs` | The source column `židovská__abs__`. |
| `zidovskaObecNaSlovenskuZ1` | Property | `float` |  |  | `jc:zidovskaObecNaSlovenskuZ1` | The source column `Židovská_obec_na_Slovensku__ž_1`. |
| `zidovskaObecNaSlovenskuZid` | Property | `integer` |  |  | `jc:zidovskaObecNaSlovenskuZid` | The source column `Židovská_obec_na_Slovensku__žid`. |
| `zivonarodeniOsoba` | Property | `float` |  |  | `jc:zivonarodeniOsoba` | The source column `Živonarodení__Osoba_`. |
| `znecistujucaLatka` | Property | `string` |  |  | `jc:znecistujucaLatka` | The source column `Znečisťujúca_látka`. |
| `zomretiOsoba` | Property | `float` |  |  | `jc:zomretiOsoba` | The source column `Zomretí__Osoba_`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OfficeHours

One row of Úradné hodiny.

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `otvoreneDo` | Property | `string` |  |  | `jc:otvoreneDo` | The source column `Otvorené_do`. |
| `otvoreneOd` | Property | `string` |  |  | `jc:otvoreneOd` | The source column `Otvorené_od`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Organization

One row of Dodávatelia; Kontakty na miestne samosprávy v BBK; Materské školy v Banskobystrickom kraji; Organizačná štruktúra; Stredné školy v BBK, odbory a počet študentov (2023); Stredné školy v Banskobystrickom kraji; Vysoké školy v Banskobystrickom kraji; Základné školy v Banskobystrickom kraji; Školy a školské zariadenia v zriaďovateľskej pôsobnosti BBSK.

IRI: `sdm:dataModel.Organization/Organization`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `absolventiZaMinulyRokSpol` | Property | `integer` |  |  | `jc:absolventiZaMinulyRokSpol` | The source column `Absolventi_za_minulý_rok___spol`. |
| `absolventiZaMinulyRokZeny` | Property | `integer` |  |  | `jc:absolventiZaMinulyRokZeny` | The source column `Absolventi_za_minulý_rok___ženy`. |
| `adresa` | Property | `string` |  |  | `jc:adresa` | The source column `Adresa`. |
| `adresaObec` | Property | `string` |  |  | `jc:adresaObec` | The source column `Adresa___Obec`. |
| `adresaPSC` | Property | `string` |  |  | `jc:adresaPSC` | The source column `Adresa___PSČ`. |
| `adresaUlica` | Property | `string` |  |  | `jc:adresaUlica` | The source column `Adresa___ulica`. |
| `akademickyRok` | Property | `string` |  |  | `jc:akademickyRok` | The source column `Akademický_rok`. |
| `datumZaciatkuVyucovacejCinnosti` | Property | `datetime` |  |  | `jc:datumZaciatkuVyucovacejCinnosti` | The source column `DatumZaciatkuVyucovacejCinnosti`. |
| `datumZaniku` | Property | `string` |  |  | `jc:datumZaniku` | The source column `DatumZaniku`. |
| `datumZriadenia` | Property | `datetime` |  |  | `jc:datumZriadenia` | The source column `DatumZriadenia`. |
| `datumZrusenia` | Property | `string` |  |  | `jc:datumZrusenia` | The source column `DatumZrusenia`. |
| `dlzkaStudia` | Property | `integer` |  |  | `jc:dlzkaStudia` | The source column `Dĺžka_štúdia`. |
| `druhSaSZ` | Property | `string` |  |  | `jc:druhSaSZ` | The source column `DruhSaSZ`. |
| `druhSaSZKod` | Property | `string` |  |  | `jc:druhSaSZKod` | The source column `DruhSaSZKod`. |
| `druhSamospravy` | Property | `string` |  |  | `jc:druhSamospravy` | The source column `Druh_samosprávy`. |
| `druhSkoly` | Property | `string` |  |  | `jc:druhSkoly` | The source column `Druh_školy`. |
| `druhStudia` | Property | `string` |  |  | `jc:druhStudia` | The source column `Druh_štúdia`. |
| `eDUID` | Property | `string` |  |  | `jc:eDUID` | The source column `EDUID`. |
| `eMailVseobecny1` | Property | `string` |  |  | `jc:eMailVseobecny1` | The source column `E_mail_všeobecný_1`. |
| `eMailVseobecny2` | Property | `string` |  |  | `jc:eMailVseobecny2` | The source column `E_mail_všeobecný_2`. |
| `email` | Property | `string` |  |  | `sdm:email` | The source column `Email`. |
| `exp` | Property | `string` |  |  | `jc:exp` | The source column `exp_`. |
| `f1Rocnik` | Property | `integer` |  |  | `jc:f1Rocnik` | The source column `F1__ročník`. |
| `f2Rocnik` | Property | `integer` |  |  | `jc:f2Rocnik` | The source column `F2__ročník`. |
| `f3Rocnik` | Property | `integer` |  |  | `jc:f3Rocnik` | The source column `F3__ročník`. |
| `f4Rocnik` | Property | `integer` |  |  | `jc:f4Rocnik` | The source column `F4__ročník`. |
| `f5Rocnik` | Property | `integer` |  |  | `jc:f5Rocnik` | The source column `F5__ročník`. |
| `f6Rocnik` | Property | `integer` |  |  | `jc:f6Rocnik` | The source column `F6__ročník`. |
| `f7Rocnik` | Property | `integer` |  |  | `jc:f7Rocnik` | The source column `F7__ročník`. |
| `f8Rocnik` | Property | `integer` |  |  | `jc:f8Rocnik` | The source column `F8__ročník`. |
| `fakultaStatistickaRocenka` | Property | `string` |  |  | `jc:fakultaStatistickaRocenka` | The source column `Fakulta___štatistická_ročenka`. |
| `fax` | Property | `string` |  |  | `jc:fax` | The source column `Fax`. |
| `formaHospodarenia` | Property | `string` |  |  | `jc:formaHospodarenia` | The source column `FormaHospodarenia`. |
| `formaStudia` | Property | `string` |  |  | `jc:formaStudia` | The source column `Forma_štúdia`. |
| `formaVlastnictva` | Property | `string` |  |  | `jc:formaVlastnictva` | The source column `FormaVlastnictva`. |
| `funkcia` | Property | `string` |  |  | `jc:funkcia` | The source column `Funkcia`. |
| `gPSX` | Property | `float` |  |  | `jc:gPSX` | The source column `GPS_x`. |
| `gPSY` | Property | `float` |  |  | `jc:gPSY` | The source column `GPS_y`. |
| `hierarchia` | Property | `string` |  |  | `jc:hierarchia` | The source column `Hierarchia`. |
| `hlavnaSkupinaOdborov` | Property | `string` |  |  | `jc:hlavnaSkupinaOdborov` | The source column `Hlavná_skupina_odborov`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `iCO2` | Property | `string` |  |  | `jc:iCO2` | The source column `IČO`. |
| `iDBBSK` | Property | `integer` |  |  | `jc:iDBBSK` | The source column `ID_BBSK`. |
| `iDSkoly` | Property | `string` |  |  | `jc:iDSkoly` | The source column `ID_školy`. |
| `jazyk` | Property | `string` |  |  | `jc:jazyk` | The source column `Jazyk`. |
| `jazykStudiaSkratka` | Property | `string` |  |  | `jc:jazykStudiaSkratka` | The source column `Jazyk_štúdia___skratka`. |
| `jazykovaSkola` | Property | `string` |  |  | `jc:jazykovaSkola` | The source column `Jazyková_škola`. |
| `kODSKO` | Property | `string` |  |  | `jc:kODSKO` | The source column `KODSKO`. |
| `kancelaria` | Property | `string` |  |  | `jc:kancelaria` | The source column `Kancelaria`. |
| `kmenovaSaSZEDUID` | Property | `integer` |  |  | `jc:kmenovaSaSZEDUID` | The source column `KmenovaSaSZ_EDUID`. |
| `kmenovaSaSZKODSKO` | Property | `integer` |  |  | `jc:kmenovaSaSZKODSKO` | The source column `KmenovaSaSZ_KODSKO`. |
| `kodJazyka` | Property | `string` |  |  | `jc:kodJazyka` | The source column `Kód_jazyka`. |
| `kodOdboru` | Property | `string` |  |  | `jc:kodOdboru` | The source column `Kód_odboru`. |
| `kodOdboruOdbor` | Property | `string` |  |  | `jc:kodOdboruOdbor` | The source column `Kód_odboru___odbor`. |
| `kodTypuZriadovatela` | Property | `string` |  |  | `jc:kodTypuZriadovatela` | The source column `Kód_typu_zriaďovateľa`. |
| `kraj` | Property | `string` |  |  | `jc:kraj` | The source column `Kraj`. |
| `lAT` | Property | `float` |  |  | `jc:lAT` | The source column `LAT`. |
| `lON` | Property | `float` |  |  | `jc:lON` | The source column `LON`. |
| `lat` | Property | `float` |  |  | `jc:lat` | The source column `lat`. |
| `latitude` | Property | `float` |  |  | `jc:latitude` | The source column `Latitude`. |
| `legalName` | Property | `string` |  |  | `sdm:dataModel.Organization/legalName` | The source column `Nazov`. |
| `lon` | Property | `float` |  |  | `jc:lon` | The source column `lon`. |
| `longitude` | Property | `float` |  |  | `jc:longitude` | The source column `Longitude`. |
| `mesto` | Property | `string` |  |  | `jc:mesto` | The source column `Mesto`. |
| `nadradenaSaSZEDUID` | Property | `string` |  |  | `jc:nadradenaSaSZEDUID` | The source column `NadradenaSaSZ_EDUID`. |
| `nadradenaSaSZKODSKO` | Property | `string` |  |  | `jc:nadradenaSaSZKODSKO` | The source column `NadradenaSaSZ_KODSKO`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovSkolyOda` | Property | `string` |  |  | `jc:nazovSkolyOda` | The source column `nazov_skoly_oda`. |
| `nazovSkrateny` | Property | `string` |  |  | `jc:nazovSkrateny` | The source column `NazovSkrateny`. |
| `novoprijatiDo1RocnikaSpo` | Property | `integer` |  |  | `jc:novoprijatiDo1RocnikaSpo` | The source column `Novoprijatí_do_1__ročníka___spo`. |
| `novoprijatiDo1RocnikaZen` | Property | `integer` |  |  | `jc:novoprijatiDo1RocnikaZen` | The source column `Novoprijatí_do_1__ročníka___žen`. |
| `obec` | Property | `string` |  |  | `jc:obec` | The source column `Obec`. |
| `obecICO` | Property | `string` |  |  | `jc:obecICO` | The source column `Obec_IČO`. |
| `obecKratkyNazov` | Property | `string` |  |  | `jc:obecKratkyNazov` | The source column `obec_kratkyNazov`. |
| `obecLAU2ID` | Property | `string` |  |  | `jc:obecLAU2ID` | The source column `Obec_LAU2_ID`. |
| `obecSTATKOD` | Property | `integer` |  |  | `jc:obecSTATKOD` | The source column `Obec_STAT_KOD`. |
| `obrazokOdkaz` | Property | `string` |  |  | `jc:obrazokOdkaz` | The source column `Obrazok_odkaz`. |
| `odbor` | Property | `string` |  |  | `jc:odbor` | The source column `Odbor`. |
| `odborVycvikAPrax` | Property | `integer` |  |  | `jc:odborVycvikAPrax` | The source column `Odbor__výcvik_a_prax_`. |
| `odborVzdelania` | Property | `string` |  |  | `jc:odborVzdelania` | The source column `Odbor_vzdelania`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `okresDlhyNazov` | Property | `string` |  |  | `jc:okresDlhyNazov` | The source column `Okres_dlhy_nazov`. |
| `okresLAU1ID` | Property | `string` |  |  | `jc:okresLAU1ID` | The source column `Okres_LAU1_ID`. |
| `okresSTATKOD` | Property | `integer` |  |  | `jc:okresSTATKOD` | The source column `Okres_STAT_KOD`. |
| `organizacnaZlozka` | Property | `string` |  |  | `jc:organizacnaZlozka` | The source column `OrganizacnaZlozka`. |
| `orientacneCislo` | Property | `string` |  |  | `jc:orientacneCislo` | The source column `OrientacneCislo`. |
| `pSC` | Property | `string` |  |  | `jc:pSC` | The source column `PSC`. |
| `plnoorganizovana` | Property | `integer` |  |  | `jc:plnoorganizovana` | The source column `Plnoorganizovana`. |
| `pocetAbsolventovDenniIA` | Property | `integer` |  |  | `jc:pocetAbsolventovDenniIA` | The source column `Počet_absolventov___denní__I__a`. |
| `pocetAbsolventovExterniI` | Property | `integer` |  |  | `jc:pocetAbsolventovExterniI` | The source column `Počet_absolventov___externí__I_`. |
| `pocetObyvatelov` | Property | `integer` |  |  | `jc:pocetObyvatelov` | The source column `Pocet_obyvatelov`. |
| `pocetStudentovDenniIAI` | Property | `integer` |  |  | `jc:pocetStudentovDenniIAI` | The source column `Počet_študentov___denní__I__a_I`. |
| `pocetStudentovExterniIA` | Property | `integer` |  |  | `jc:pocetStudentovExterniIA` | The source column `Počet_študentov___externí__I__a`. |
| `pravnaForma` | Property | `string` |  |  | `jc:pravnaForma` | The source column `Právna_forma`. |
| `pravnaFormaKod` | Property | `string` |  |  | `jc:pravnaFormaKod` | The source column `PravnaFormaKod`. |
| `predchodcaSaSZEDUID` | Property | `string` |  |  | `jc:predchodcaSaSZEDUID` | The source column `PredchodcaSaSZ_EDUID`. |
| `predchodcaSaSZNazov` | Property | `string` |  |  | `jc:predchodcaSaSZNazov` | The source column `PredchodcaSaSZ_Nazov`. |
| `priprNaOU` | Property | `integer` |  |  | `jc:priprNaOU` | The source column `prípr__na_OU`. |
| `pripravny8Spolu` | Property | `integer` |  |  | `jc:pripravny8Spolu` | The source column `Prípravný___8____spolu`. |
| `pripravny8Zeny` | Property | `integer` |  |  | `jc:pripravny8Zeny` | The source column `Prípravný___8____ženy`. |
| `sTAV` | Property | `string` |  |  | `jc:sTAV` | The source column `STAV`. |
| `skola` | Property | `string` |  |  | `jc:skola` | The source column `Škola`. |
| `skolskaJedalen` | Property | `string` |  |  | `jc:skolskaJedalen` | The source column `Školská_jedáleň`. |
| `skolskyInternat` | Property | `string` |  |  | `jc:skolskyInternat` | The source column `Školský_internát`. |
| `skupinaOdborovVzdelania` | Property | `string` |  |  | `jc:skupinaOdborovVzdelania` | The source column `Skupina_odborov_vzdelania`. |
| `starostaMeno` | Property | `string` |  |  | `jc:starostaMeno` | The source column `Starosta_meno`. |
| `starostaPriezvisko` | Property | `string` |  |  | `jc:starostaPriezvisko` | The source column `Starosta_priezvisko`. |
| `stupenVzdelania` | Property | `string` |  |  | `jc:stupenVzdelania` | The source column `Stupeň_vzdelania`. |
| `supisneCislo` | Property | `string` |  |  | `jc:supisneCislo` | The source column `SupisneCislo`. |
| `sustavnaPripravaNaPovolanie` | Property | `integer` |  |  | `jc:sustavnaPripravaNaPovolanie` | The source column `SustavnaPripravaNaPovolanie`. |
| `telefon` | Property | `string` |  |  | `jc:telefon` | The source column `Telefon`. |
| `telefonneCisloVsobecne1` | Property | `string` |  |  | `jc:telefonneCisloVsobecne1` | The source column `Telefónne_číslo___všobecné_1`. |
| `telefonneCisloVsobecne2` | Property | `string` |  |  | `jc:telefonneCisloVsobecne2` | The source column `Telefónne_číslo___všobecné_2`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `typ`. |
| `typElokovanehoPracoviska` | Property | `string` |  |  | `jc:typElokovanehoPracoviska` | The source column `TypElokovanehoPracoviska`. |
| `typSaSZ` | Property | `string` |  |  | `jc:typSaSZ` | The source column `TypSaSZ`. |
| `typSaSZKod` | Property | `string` |  |  | `jc:typSaSZKod` | The source column `TypSaSZKod`. |
| `typSaSZSkratka` | Property | `string` |  |  | `jc:typSaSZSkratka` | The source column `TypSaSZSkratka`. |
| `typUniverzity` | Property | `string` |  |  | `jc:typUniverzity` | The source column `Typ_univerzity`. |
| `ulica` | Property | `string` |  |  | `jc:ulica` | The source column `Ulica`. |
| `univerzita` | Property | `string` |  |  | `jc:univerzita` | The source column `Univerzita`. |
| `upravene` | Property | `datetime` |  |  | `jc:upravene` | The source column `Upravené`. |
| `vydajnaSkolskaJedalen` | Property | `string` |  |  | `jc:vydajnaSkolskaJedalen` | The source column `Výdajná_školská_jedáleň`. |
| `vytvorene` | Property | `datetime` |  |  | `jc:vytvorene` | The source column `Vytvorené`. |
| `vyucovaciJazyk` | Property | `string` |  |  | `jc:vyucovaciJazyk` | The source column `VyucovaciJazyk`. |
| `web` | Property | `string` |  |  | `jc:web` | The source column `Web`. |
| `webstranka` | Property | `string` |  |  | `jc:webstranka` | The source column `Webstránka`. |
| `zriadovatel` | Property | `string` |  |  | `jc:zriadovatel` | The source column `Zriaďovateľ`. |
| `zriadovatelEDUID` | Property | `integer` |  |  | `jc:zriadovatelEDUID` | The source column `Zriadovatel_EDUID`. |
| `zriadovatelNazov` | Property | `string` |  |  | `jc:zriadovatelNazov` | The source column `Zriadovatel_Nazov`. |
| `zriadovatelTyp` | Property | `string` |  |  | `jc:zriadovatelTyp` | The source column `Zriadovatel_Typ`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Person

One row of Členovia samosprávy.

IRI: `sdm:dataModel.Organization/Person`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `funkcia` | Property | `string` |  |  | `jc:funkcia` | The source column `Funkcia`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `meno` | Property | `string` |  |  | `jc:meno` | The source column `Meno`. |
| `nazovKlubu` | Property | `string` |  |  | `jc:nazovKlubu` | The source column `Názov_klubu`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `obvod` | Property | `string` |  |  | `jc:obvod` | The source column `Obvod`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `priezvisko` | Property | `string` |  |  | `jc:priezvisko` | The source column `Priezvisko`. |
| `titulPredMenom` | Property | `string` |  |  | `jc:titulPredMenom` | The source column `Titul_pred_menom`. |
| `titulZaMenom` | Property | `string` |  |  | `jc:titulZaMenom` | The source column `Titul_za_menom`. |
| `voFunkciiDo` | Property | `string` |  |  | `jc:voFunkciiDo` | The source column `Vo_funkcii_do`. |
| `voFunkciiOd` | Property | `datetime` |  |  | `jc:voFunkciiOd` | The source column `Vo_funkcii_od`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Petition

One row of Petície.

IRI: `jc:Petition`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datumDorucenia` | Property | `date` |  |  | `jc:datumDorucenia` | The source column `Dátum_doručenia`. |
| `datumVybavenia` | Property | `date` |  |  | `jc:datumVybavenia` | The source column `Dátum_vybavenia`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `jeKvorova` | Property | `string` |  |  | `jc:jeKvorova` | The source column `Je_kvórová`. |
| `link` | Property | `string` |  |  | `jc:link` | The source column `Link`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `pocetOsobPodporujucichPetici` | Property | `integer` |  |  | `jc:pocetOsobPodporujucichPetici` | The source column `Počet_osôb_podporujúcich_petíci`. |
| `poznamka` | Property | `string` |  |  | `jc:poznamka` | The source column `Poznámka`. |
| `sposobVybavenia` | Property | `string` |  |  | `jc:sposobVybavenia` | The source column `Spôsob_vybavenia`. |
| `text` | Property | `string` |  |  | `jc:text` | The source column `Text`. |
| `vybavovatelPeticie` | Property | `string` |  |  | `jc:vybavovatelPeticie` | The source column `Vybavovateľ_petície`. |
| `zverejnenieVysledkuVybavenia` | Property | `date` |  |  | `jc:zverejnenieVysledkuVybavenia` | The source column `Zverejnenie_výsledku_vybavenia`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One row of Cyklosčítače - miesta; Cykloturistické trasy - nosič/infotabuľa/odpočívadlo/značenie; Kontaktné miesta; Kultúrne inštitúcie BBSK; Kultúrne inštitúcie BBSK a ich pobočky; Nabíjačky a Zastávky; Zoznam lekární v BBK; Zoznam zdravotníckych zariadení v BBK.

IRI: `sdm:dataModel.PointOfInterest/PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `addressReg` | Property | `string` |  |  | `jc:addressReg` | The source column `addressReg`. |
| `adresa` | Property | `string` |  |  | `jc:adresa` | The source column `Adresa`. |
| `altitude` | Property | `string` |  |  | `jc:altitude` | The source column `altitude`. |
| `author` | Property | `integer` |  |  | `jc:author` | The source column `author`. |
| `belongRegi` | Property | `integer` |  |  | `jc:belongRegi` | The source column `belongRegi`. |
| `cMSID` | Property | `integer` |  |  | `jc:cMSID` | The source column `CMS_ID`. |
| `categ` | Property | `string` |  |  | `jc:categ` | The source column `Categ`. |
| `category` | Property | `string` (list) |  |  | `sdm:dataModel.PointOfInterest/category` | The source column `category`. |
| `cellPhone` | Property | `string` |  |  | `jc:cellPhone` | The source column `cellPhone`. |
| `changedByC` | Property | `datetime` |  |  | `jc:changedByC` | The source column `changedByC`. |
| `city` | Property | `string` |  |  | `jc:city` | The source column `city`. |
| `contentSta` | Property | `string` |  |  | `jc:contentSta` | The source column `contentSta`. |
| `createdBy` | Property | `integer` |  |  | `jc:createdBy` | The source column `createdBy`. |
| `createdIn` | Property | `integer` |  |  | `jc:createdIn` | The source column `createdIn`. |
| `dIC` | Property | `string` |  |  | `jc:dIC` | The source column `DIČ`. |
| `datumSchvaleniaOH` | Property | `string` |  |  | `jc:datumSchvaleniaOH` | The source column `Dátum_schválenia_OH`. |
| `defLanguag` | Property | `string` |  |  | `jc:defLanguag` | The source column `defLanguag`. |
| `descriptio` | Property | `string` |  |  | `jc:descriptio` | The source column `descriptio`. |
| `domena` | Property | `string` |  |  | `jc:domena` | The source column `Doména`. |
| `doplnStp` | Property | `integer` |  |  | `jc:doplnStp` | The source column `dopln_stp`. |
| `druhZariadenia` | Property | `string` |  |  | `jc:druhZariadenia` | The source column `Druh_zariadenia`. |
| `eMailZariadenia` | Property | `string` |  |  | `jc:eMailZariadenia` | The source column `E_mail_zariadenia`. |
| `email` | Property | `string` |  |  | `sdm:email` | The source column `Email`. |
| `fID2` | Property | `integer` |  |  | `jc:fID2` | The source column `FID2`. |
| `facebook` | Property | `string` |  |  | `jc:facebook` | The source column `Facebook`. |
| `fax` | Property | `string` |  |  | `jc:fax` | The source column `Fax`. |
| `geometry` | Property | `string` |  |  | `jc:geometry` | The source column `geometry`. |
| `hlavnaCat` | Property | `string` |  |  | `jc:hlavnaCat` | The source column `Hlavna_cat`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `iD` | Property | `integer` |  |  | `jc:iD` | The source column `ID`. |
| `idZZ` | Property | `string` |  |  | `jc:idZZ` | The source column `IdZZ`. |
| `images` | Property | `integer` |  |  | `jc:images` | The source column `images`. |
| `importKey` | Property | `string` |  |  | `jc:importKey` | The source column `importKey`. |
| `instagram` | Property | `string` |  |  | `jc:instagram` | The source column `Instagram`. |
| `kategoria` | Property | `string` |  |  | `jc:kategoria` | The source column `Kategória`. |
| `kategoriaObrazok` | Property | `string` |  |  | `jc:kategoriaObrazok` | The source column `Kategória_obrázok`. |
| `komentar` | Property | `string` |  |  | `jc:komentar` | The source column `Komentár`. |
| `kontakt` | Property | `string` |  |  | `jc:kontakt` | The source column `Kontakt`. |
| `lAT` | Property | `float` |  |  | `jc:lAT` | The source column `LAT`. |
| `lON` | Property | `float` |  |  | `jc:lON` | The source column `LON`. |
| `languages` | Property | `string` |  |  | `jc:languages` | The source column `languages`. |
| `lastModi1` | Property | `datetime` |  |  | `jc:lastModi1` | The source column `lastModi_1`. |
| `lastModifi` | Property | `integer` |  |  | `jc:lastModifi` | The source column `lastModifi`. |
| `latitude` | Property | `float` |  |  | `jc:latitude` | The source column `Latitude`. |
| `licenseKey` | Property | `string` |  |  | `jc:licenseKey` | The source column `licenseKey`. |
| `logo` | Property | `string` |  |  | `sdm:dataModel.PointOfInterest/logo` | The source column `Logo`. |
| `longitude` | Property | `float` |  |  | `jc:longitude` | The source column `Longitude`. |
| `malZnac` | Property | `integer` |  |  | `jc:malZnac` | The source column `mal_znac`. |
| `mesto` | Property | `string` |  |  | `jc:mesto` | The source column `Mesto`. |
| `mestoPRACOVISKO` | Property | `string` |  |  | `jc:mestoPRACOVISKO` | The source column `Mesto_PRACOVISKO`. |
| `mestoPoskytovania` | Property | `string` |  |  | `jc:mestoPoskytovania` | The source column `Mesto_poskytovania`. |
| `mestoSIDLO` | Property | `string` |  |  | `jc:mestoSIDLO` | The source column `Mesto_SÍDLO`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovPoskytovatela` | Property | `string` |  |  | `jc:nazovPoskytovatela` | The source column `Názov_poskytovateľa`. |
| `nedela` | Property | `string` |  |  | `jc:nedela` | The source column `Nedeľa`. |
| `nedost` | Property | `integer` |  |  | `jc:nedost` | The source column `nedost`. |
| `nedostTyp` | Property | `string` |  |  | `jc:nedostTyp` | The source column `nedost_typ`. |
| `nedostatok` | Property | `string` |  |  | `jc:nedostatok` | The source column `nedostatok`. |
| `obrazokOdkaz` | Property | `string` |  |  | `jc:obrazokOdkaz` | The source column `Obrazok_odkaz`. |
| `obtiazmost` | Property | `string` |  |  | `jc:obtiazmost` | The source column `obtiazmost`. |
| `odborneZameranie` | Property | `string` |  |  | `jc:odborneZameranie` | The source column `Odborné_zameranie`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `okresPRACOVISKO` | Property | `string` |  |  | `jc:okresPRACOVISKO` | The source column `Okres_PRACOVISKO`. |
| `okresPoskytovania` | Property | `string` |  |  | `jc:okresPoskytovania` | The source column `Okres_poskytovania`. |
| `onlineKatalog` | Property | `string` |  |  | `jc:onlineKatalog` | The source column `Online_katalóg`. |
| `organisati` | Property | `integer` |  |  | `jc:organisati` | The source column `organisati`. |
| `pC` | Property | `integer` |  |  | `jc:pC` | The source column `P_č_`. |
| `pSC` | Property | `string` |  |  | `jc:pSC` | The source column `PSC`. |
| `pSCPRACOVISKO` | Property | `string` |  |  | `jc:pSCPRACOVISKO` | The source column `PSČ_PRACOVISKO`. |
| `pSCPoskytovania` | Property | `string` |  |  | `jc:pSCPoskytovania` | The source column `PSČ_poskytovania`. |
| `pSCSIDLO` | Property | `string` |  |  | `jc:pSCSIDLO` | The source column `PSČ_SÍDLO`. |
| `piatok` | Property | `string` |  |  | `jc:piatok` | The source column `Piatok`. |
| `pondelok` | Property | `string` |  |  | `jc:pondelok` | The source column `Pondelok`. |
| `pozn` | Property | `string` |  |  | `jc:pozn` | The source column `Pozn`. |
| `prevadzkovatel` | Property | `string` |  |  | `jc:prevadzkovatel` | The source column `Prevádzkovateľ`. |
| `primaryIma` | Property | `string` |  |  | `jc:primaryIma` | The source column `primaryIma`. |
| `priznak` | Property | `string` |  |  | `jc:priznak` | The source column `Priznak`. |
| `publishedA` | Property | `datetime` |  |  | `jc:publishedA` | The source column `publishedA`. |
| `publishedB` | Property | `integer` |  |  | `jc:publishedB` | The source column `publishedB`. |
| `rankingPer` | Property | `integer` |  |  | `jc:rankingPer` | The source column `rankingPer`. |
| `scitac` | Property | `string` |  |  | `jc:scitac` | The source column `Sčítač`. |
| `shortDescr` | Property | `string` |  |  | `jc:shortDescr` | The source column `shortDescr`. |
| `sidloPoskytovatela` | Property | `string` |  |  | `jc:sidloPoskytovatela` | The source column `Sídlo_poskytovateľa`. |
| `sidloPrevadzkovatela` | Property | `string` |  |  | `jc:sidloPrevadzkovatela` | The source column `Sídlo_prevádzkovateľa`. |
| `sobota` | Property | `string` |  |  | `jc:sobota` | The source column `Sobota`. |
| `sourceCreatedAt` | Property | `datetime` |  |  | `jc:sourceCreatedAt` | The source column `createdAt`. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The source column `id`. |
| `sourceName` | Property | `string` |  |  | `jc:sourceName` | The source column `name`. |
| `stlpEx` | Property | `integer` |  |  | `jc:stlpEx` | The source column `stlp_ex`. |
| `stlpMat` | Property | `string` |  |  | `jc:stlpMat` | The source column `stlp_mat`. |
| `stlpStav` | Property | `string` |  |  | `jc:stlpStav` | The source column `stlp_stav`. |
| `stn` | Property | `integer` |  |  | `jc:stn` | The source column `stn`. |
| `streda` | Property | `string` |  |  | `jc:streda` | The source column `Streda`. |
| `streetAddr` | Property | `string` |  |  | `jc:streetAddr` | The source column `streetAddr`. |
| `stvrtok` | Property | `string` |  |  | `jc:stvrtok` | The source column `Štvrtok`. |
| `telefonZariadeniaVerejny` | Property | `string` |  |  | `jc:telefonZariadeniaVerejny` | The source column `Telefón_zariadenia_verejný`. |
| `telephone` | Property | `string` |  |  | `sdm:telephone` | The source column `telephone`. |
| `title` | Property | `string` |  |  | `http://purl.org/dc/terms/title` | The source column `title`. |
| `trasaFarb` | Property | `string` |  |  | `jc:trasaFarb` | The source column `trasa_farb`. |
| `trasaNo` | Property | `string` |  |  | `jc:trasaNo` | The source column `trasa_no`. |
| `trasaSpr` | Property | `string` |  |  | `jc:trasaSpr` | The source column `trasa_spr`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `typ`. |
| `typ1` | Property | `string` |  |  | `jc:typ1` | The source column `Typ_1`. |
| `uRL` | Property | `string` |  |  | `jc:uRL` | The source column `URL`. |
| `ulica` | Property | `string` |  |  | `jc:ulica` | The source column `Ulica`. |
| `ulicaCisloPRACOVISKO` | Property | `string` |  |  | `jc:ulicaCisloPRACOVISKO` | The source column `Ulica_číslo_PRACOVISKO`. |
| `ulicaCisloSIDLO` | Property | `string` |  |  | `jc:ulicaCisloSIDLO` | The source column `Ulica_číslo_SÍDLO`. |
| `ulicaPoskytovania` | Property | `string` |  |  | `jc:ulicaPoskytovania` | The source column `Ulica_poskytovania`. |
| `utorok` | Property | `string` |  |  | `jc:utorok` | The source column `Utorok`. |
| `verejnyEmail` | Property | `string` |  |  | `jc:verejnyEmail` | The source column `Verejný_email`. |
| `verejnyTelefon` | Property | `string` |  |  | `jc:verejnyTelefon` | The source column `Verejný_telefón`. |
| `vytvoreny` | Property | `date` |  |  | `jc:vytvoreny` | The source column `Vytvorený`. |
| `web` | Property | `string` |  |  | `jc:web` | The source column `Web`. |
| `webAddress` | Property | `string` |  |  | `jc:webAddress` | The source column `webAddress`. |
| `webStranka` | Property | `string` |  |  | `jc:webStranka` | The source column `web_stránka`. |
| `x` | Property | `float` |  |  | `jc:x` | The source column `x`. |
| `y` | Property | `float` |  |  | `jc:y` | The source column `y`. |
| `youTube` | Property | `string` |  |  | `jc:youTube` | The source column `YouTube`. |
| `zdroj` | Property | `string` |  |  | `jc:zdroj` | The source column `zdroj`. |
| `zipcode` | Property | `integer` |  |  | `jc:zipcode` | The source column `zipcode`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PublicInvestment

One row of Projekty Banskobystrického kraja ITMS21; Register investícií - bodová vrstva; Register investícií - polygónová vrstva.

IRI: `jc:PublicInvestment`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `adresa` | Property | `string` |  |  | `jc:adresa` | The source column `Adresa`. |
| `aktivne` | Property | `string` |  |  | `jc:aktivne` | The source column `Aktívne`. |
| `aktualnyStav` | Property | `string` |  |  | `jc:aktualnyStav` | The source column `Aktuálny_stav`. |
| `aktualnyStavUpraveny` | Property | `string` |  |  | `jc:aktualnyStavUpraveny` | The source column `Aktuálny_stav_upravený`. |
| `celkovaZazmluvnenaSuma` | Property | `float` |  |  | `jc:celkovaZazmluvnenaSuma` | The source column `Celková_zazmluvnená_suma`. |
| `cerpanieSpolu` | Property | `float` |  |  | `jc:cerpanieSpolu` | The source column `Čerpanie_spolu`. |
| `dOW` | Property | `datetime` |  |  | `jc:dOW` | The source column `DOW`. |
| `datumZaniku` | Property | `string` |  |  | `jc:datumZaniku` | The source column `DatumZaniku`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fLAT` | Property | `string` |  |  | `jc:fLAT` | The source column `F_LAT`. |
| `fLON` | Property | `string` |  |  | `jc:fLON` | The source column `F_LON`. |
| `gEOMLATBACKUP` | Property | `float` |  |  | `jc:gEOMLATBACKUP` | The source column `GEOM_LAT_BACKUP`. |
| `gEOMLONBACKUP` | Property | `float` |  |  | `jc:gEOMLONBACKUP` | The source column `GEOM_LON_BACKUP`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `iCOOrganizacia` | Property | `string` |  |  | `jc:iCOOrganizacia` | The source column `IČO_organizácia`. |
| `iCOPrijimatela` | Property | `string` |  |  | `jc:iCOPrijimatela` | The source column `IČO_prijímateľa`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `iDN3` | Property | `integer` |  |  | `jc:iDN3` | The source column `IDN3`. |
| `iDN4` | Property | `integer` |  |  | `jc:iDN4` | The source column `IDN4`. |
| `iDProjektu` | Property | `string` |  |  | `jc:iDProjektu` | The source column `ID_projektu`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `index` | Property | `string` |  |  | `jc:index` | The source column `Index_`. |
| `kategoria` | Property | `string` |  |  | `jc:kategoria` | The source column `Kategória`. |
| `kodKraja` | Property | `string` |  |  | `jc:kodKraja` | The source column `Kód_kraja`. |
| `kodObce` | Property | `string` |  |  | `jc:kodObce` | The source column `Kód_obce`. |
| `kodOkresuKraja` | Property | `integer` |  |  | `jc:kodOkresuKraja` | The source column `Kód_okresu_kraja`. |
| `lATJitter` | Property | `float` |  |  | `jc:lATJitter` | The source column `LAT_jitter`. |
| `lAU1` | Property | `string` |  |  | `jc:lAU1` | The source column `LAU1`. |
| `lAU1CODE` | Property | `string` |  |  | `jc:lAU1CODE` | The source column `LAU1_CODE`. |
| `lAU2` | Property | `string` |  |  | `jc:lAU2` | The source column `LAU2`. |
| `lAU2CODE` | Property | `string` |  |  | `jc:lAU2CODE` | The source column `LAU2_CODE`. |
| `lONJitter` | Property | `float` |  |  | `jc:lONJitter` | The source column `LON_jitter`. |
| `miestoRealizacieProjektu` | Property | `string` |  |  | `jc:miestoRealizacieProjektu` | The source column `Miesto_realizácie_projektu`. |
| `miestoRealizacieProjektuKraj` | Property | `string` |  |  | `jc:miestoRealizacieProjektuKraj` | The source column `Miesto_realizácie_projektu_kraj`. |
| `miestoRealizacieProjektuObec` | Property | `string` |  |  | `jc:miestoRealizacieProjektuObec` | The source column `Miesto_realizácie_projektu_obec`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `nM3` | Property | `string` |  |  | `jc:nM3` | The source column `NM3`. |
| `nM4` | Property | `string` |  |  | `jc:nM4` | The source column `NM4`. |
| `nUTS1` | Property | `string` |  |  | `jc:nUTS1` | The source column `NUTS1`. |
| `nUTS1CODE` | Property | `string` |  |  | `jc:nUTS1CODE` | The source column `NUTS1_CODE`. |
| `nUTS2` | Property | `string` |  |  | `jc:nUTS2` | The source column `NUTS2`. |
| `nUTS2CODE` | Property | `string` |  |  | `jc:nUTS2CODE` | The source column `NUTS2_CODE`. |
| `nUTS3` | Property | `string` |  |  | `jc:nUTS3` | The source column `NUTS3`. |
| `nUTS3CODE` | Property | `string` |  |  | `jc:nUTS3CODE` | The source column `NUTS3_CODE`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovKraja` | Property | `string` |  |  | `jc:nazovKraja` | The source column `Názov_kraja`. |
| `nazovPrijimatela` | Property | `string` |  |  | `jc:nazovPrijimatela` | The source column `Názov_prijímateľa`. |
| `oBJECTID1` | Property | `integer` |  |  | `jc:oBJECTID1` | The source column `OBJECTID_1`. |
| `obecNUTS` | Property | `string` |  |  | `jc:obecNUTS` | The source column `Obec_NUTS`. |
| `obecNazov` | Property | `string` |  |  | `jc:obecNazov` | The source column `Obec_názov`. |
| `obstaravaciaCena` | Property | `float` |  |  | `jc:obstaravaciaCena` | The source column `Obstarávacia_cena`. |
| `oficialnyNazov` | Property | `string` |  |  | `jc:oficialnyNazov` | The source column `Oficiálny_názov`. |
| `okresKraj` | Property | `string` |  |  | `jc:okresKraj` | The source column `Okres_Kraj`. |
| `okresNazov` | Property | `string` |  |  | `jc:okresNazov` | The source column `Okres_názov`. |
| `platnostProjektuOd` | Property | `datetime` |  |  | `jc:platnostProjektuOd` | The source column `Platnosť_projektu_od`. |
| `poznamka` | Property | `string` |  |  | `jc:poznamka` | The source column `Poznámka`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `rokText` | Property | `string` |  |  | `jc:rokText` | The source column `Rok_text`. |
| `rokZaradenia` | Property | `string` |  |  | `jc:rokZaradenia` | The source column `Rok_zaradenia`. |
| `rozpocetAktualny` | Property | `float` |  |  | `jc:rozpocetAktualny` | The source column `Rozpočet_aktuálny`. |
| `rozsahUzemiaProjektu` | Property | `string` |  |  | `jc:rozsahUzemiaProjektu` | The source column `Rozsah_územia_projektu`. |
| `sUMCerpanieSpolu` | Property | `float` |  |  | `jc:sUMCerpanieSpolu` | The source column `SUM_Čerpanie_spolu`. |
| `sektorOvZPUpraveny` | Property | `string` |  |  | `jc:sektorOvZPUpraveny` | The source column `Sektor_OvZP_upraveny`. |
| `typObce` | Property | `string` |  |  | `jc:typObce` | The source column `typ_obce`. |
| `uradOvZPICO` | Property | `string` |  |  | `jc:uradOvZPICO` | The source column `Úrad_OvZP_IČO`. |
| `uradOvZPOvZP` | Property | `string` |  |  | `jc:uradOvZPOvZP` | The source column `Úrad_OvZP_OvZP`. |
| `uroven` | Property | `string` |  |  | `jc:uroven` | The source column `Úroveň`. |
| `vYMERA` | Property | `float` |  |  | `jc:vYMERA` | The source column `VYMERA`. |
| `vecnaOblast` | Property | `string` |  |  | `jc:vecnaOblast` | The source column `Vecná_oblasť`. |
| `zauzivanyNazov` | Property | `string` |  |  | `jc:zauzivanyNazov` | The source column `Zaužívaný_názov`. |
| `zauzivanyNazovKratky` | Property | `string` |  |  | `jc:zauzivanyNazovKratky` | The source column `Zaužívaný_názov_krátky`. |
| `zauzivanyNazovUpraveny` | Property | `string` |  |  | `jc:zauzivanyNazovUpraveny` | The source column `Zaužívaný_názov_upravený`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PublicOrganization

One row of Príspevkové rozpočtové a iné organizácie samosprávy.

IRI: `sdm:dataModel.CPSV-AP/PublicOrganization`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `adresaOrganizacie` | Property | `string` |  |  | `jc:adresaOrganizacie` | The source column `Adresa_organizácie`. |
| `formaPodnikania` | Property | `string` |  |  | `jc:formaPodnikania` | The source column `Forma_podnikania`. |
| `iCOOrganizacie` | Property | `string` |  |  | `jc:iCOOrganizacie` | The source column `IČO_organizácie`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `iDAdresy` | Property | `string` |  |  | `jc:iDAdresy` | The source column `ID_adresy`. |
| `kodFormyPodnikania` | Property | `string` |  |  | `jc:kodFormyPodnikania` | The source column `Kód_formy_podnikania`. |
| `lAT` | Property | `float` |  |  | `jc:lAT` | The source column `LAT`. |
| `lON` | Property | `float` |  |  | `jc:lON` | The source column `LON`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `preferredLabel` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/preferredLabel` | The source column `Názov_organizácie`. |
| `spatial` | Property | `string` (list) |  |  | `sdm:dataModel.CPSV-AP/spatial` | The source column `Obec`. |
| `ulicaACisloDomu` | Property | `string` |  |  | `jc:ulicaACisloDomu` | The source column `Ulica_a_číslo_domu`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PublicProcurement

One row of Plán verejného obstarávania; Realizované verejné obstarávania.

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
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `lehotaNaPredkladanie` | Property | `datetime` |  |  | `jc:lehotaNaPredkladanie` | The source column `Lehota_na_predkladanie`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `najvyhodnejsiUchadzac` | Property | `string` |  |  | `jc:najvyhodnejsiUchadzac` | The source column `Najvýhodnejší_uchádzač`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
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
| `stav` | Property | `string` |  |  | `jc:stav` | The source column `stav`. |
| `typKomodity` | Property | `string` |  |  | `jc:typKomodity` | The source column `Typ_komodity`. |
| `typVydavku` | Property | `string` |  |  | `jc:typVydavku` | The source column `Typ_výdavku`. |
| `utvarOvZP` | Property | `string` |  |  | `jc:utvarOvZP` | The source column `Útvar_OvZP`. |
| `zeleneObstaravanie` | Property | `string` |  |  | `jc:zeleneObstaravanie` | The source column `Zelené_obstarávanie`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PurchaseOrder

One row of Objednávky.

IRI: `jc:PurchaseOrder`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datumVystavenia` | Property | `date` |  |  | `jc:datumVystavenia` | The source column `Dátum_vystavenia`. |
| `dodavatel` | Property | `string` |  |  | `jc:dodavatel` | The source column `Dodávateľ`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `ICO`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `predmetObjednavky` | Property | `string` |  |  | `jc:predmetObjednavky` | The source column `Predmet_objednávky`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `suma` | Property | `float` |  |  | `jc:suma` | The source column `Suma`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### RiverBasin

One row of Povodia Banskobystrického kraja.

IRI: `jc:RiverBasin`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `aCH` | Property | `integer` |  |  | `jc:aCH` | The source column `ACH`. |
| `aUT` | Property | `integer` |  |  | `jc:aUT` | The source column `AUT`. |
| `dOW` | Property | `datetime` |  |  | `jc:dOW` | The source column `DOW`. |
| `designatio` | Property | `string` |  |  | `jc:designatio` | The source column `designatio`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fIDKraj0` | Property | `integer` |  |  | `jc:fIDKraj0` | The source column `FID_kraj_0`. |
| `fIDSubpovodiaBbsk` | Property | `integer` |  |  | `jc:fIDSubpovodiaBbsk` | The source column `FID_subpovodia_bbsk`. |
| `gmlId` | Property | `string` |  |  | `jc:gmlId` | The source column `gml_id`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `inspireIdL` | Property | `integer` |  |  | `jc:inspireIdL` | The source column `inspireIdL`. |
| `inspireIdN` | Property | `string` |  |  | `jc:inspireIdN` | The source column `inspireIdN`. |
| `link` | Property | `string` |  |  | `jc:link` | The source column `Link`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nameLangua` | Property | `string` |  |  | `jc:nameLangua` | The source column `nameLangua`. |
| `relatedZ1` | Property | `string` |  |  | `jc:relatedZ1` | The source column `relatedZ_1`. |
| `relatedZon` | Property | `string` |  |  | `jc:relatedZon` | The source column `relatedZon`. |
| `sOI` | Property | `integer` |  |  | `jc:sOI` | The source column `SOI`. |
| `specialise` | Property | `string` |  |  | `jc:specialise` | The source column `specialise`. |
| `thematic1` | Property | `string` |  |  | `jc:thematic1` | The source column `thematic_1`. |
| `thematicId` | Property | `string` |  |  | `jc:thematicId` | The source column `thematicId`. |
| `vYMERA` | Property | `float` |  |  | `jc:vYMERA` | The source column `VYMERA`. |
| `wiseEvolut` | Property | `string` |  |  | `jc:wiseEvolut` | The source column `wiseEvolut`. |
| `zoneType` | Property | `string` |  |  | `jc:zoneType` | The source column `zoneType`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### RoadAccident

One row of Cyklisti_nehodovosť; Nehodovosť v BBK.

IRI: `sdm:dataModel.Transportation/RoadAccident`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cisloNehody` | Property | `string` |  |  | `jc:cisloNehody` | The source column `Číslo_nehody`. |
| `datumACas` | Property | `datetime` |  |  | `jc:datumACas` | The source column `Dátum_a_čas`. |
| `druhKomunikacie` | Property | `string` |  |  | `jc:druhKomunikacie` | The source column `Druh_komunikácie`. |
| `gPSDlzka` | Property | `float` |  |  | `jc:gPSDlzka` | The source column `GPS_Dĺžka`. |
| `gPSSirka` | Property | `float` |  |  | `jc:gPSSirka` | The source column `GPS_Šírka`. |
| `hlavnyVinnik` | Property | `string` |  |  | `jc:hlavnyVinnik` | The source column `Hlavný_vinník`. |
| `hmotnaSkoda` | Property | `integer` |  |  | `jc:hmotnaSkoda` | The source column `hmotná_škoda`. |
| `lahkoZraneni` | Property | `integer` |  |  | `jc:lahkoZraneni` | The source column `Ľahko_zranení`. |
| `lahkoZraneniUcastnici` | Property | `integer` |  |  | `jc:lahkoZraneniUcastnici` | The source column `ľahko_zranení_účastníci`. |
| `obdobie` | Property | `string` |  |  | `jc:obdobie` | The source column `obdobie`. |
| `parameter` | Property | `string` |  |  | `jc:parameter` | The source column `parameter`. |
| `pocetNehod` | Property | `integer` |  |  | `jc:pocetNehod` | The source column `počet_nehôd`. |
| `pocetNehodPodVplyvomAlkoholu` | Property | `integer` |  |  | `jc:pocetNehodPodVplyvomAlkoholu` | The source column `počet_nehôd_pod_vplyvom_alkoholu`. |
| `pocetNehodVObci` | Property | `integer` |  |  | `jc:pocetNehodVObci` | The source column `počet_nehôd_v_obci`. |
| `pocetUsmrtenychCyklistovKolobezkarov` | Property | `integer` |  |  | `jc:pocetUsmrtenychCyklistovKolobezkarov` | The source column `počet_usmrtených_cyklistov_kolobežkárov`. |
| `povrchVozovky` | Property | `string` |  |  | `jc:povrchVozovky` | The source column `Povrch_vozovky`. |
| `priciny` | Property | `string` |  |  | `jc:priciny` | The source column `Príčiny`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `skodaVEUR` | Property | `integer` |  |  | `jc:skodaVEUR` | The source column `Škoda_v_EUR`. |
| `smerovePomery` | Property | `string` |  |  | `jc:smerovePomery` | The source column `Smerové_pomery`. |
| `status` | Property | `string` |  |  | `ngsi-ld:status` | `archived` on every entity: the model requires it and no column holds it. |
| `tazkoZraneni` | Property | `integer` |  |  | `jc:tazkoZraneni` | The source column `Ťažko_zranení`. |
| `tazkoZraneniUcastnici` | Property | `integer` |  |  | `jc:tazkoZraneniUcastnici` | The source column `ťažko_zranení_účastníci`. |
| `usmrteni` | Property | `integer` |  |  | `jc:usmrteni` | The source column `Usmrtení`. |
| `usmrteniUcastnici` | Property | `integer` |  |  | `jc:usmrteniUcastnici` | The source column `usmrtení_účastníci`. |
| `uzemie` | Property | `string` |  |  | `jc:uzemie` | The source column `Územie`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### RoadStructure

One row of Dilatačné celky v Banskobystrickom kraji (public view); Podjazdy v Banskobystrickom kraji (public view); Priepusty v Banskobystrickom kraji (public view); Železničné priecestia v Banskobystrickom kraji (public view).

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
| `najblizsiKM` | Property | `float` |  |  | `jc:najblizsiKM` | The source column `Najbližší_KM`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `normanaZatazitelnostT` | Property | `integer` |  |  | `jc:normanaZatazitelnostT` | The source column `Normána_zaťažiteľnost___t`. |
| `okres` | Property | `string` |  |  | `jc:okres` | The source column `Okres`. |
| `pocetKolaji` | Property | `integer` |  |  | `jc:pocetKolaji` | The source column `Počet_kolaji`. |
| `predmetPremostenia` | Property | `string` |  |  | `jc:predmetPremostenia` | The source column `Predmet_premostenia`. |
| `rokStavby` | Property | `integer` |  |  | `jc:rokStavby` | The source column `rok_stavby`. |
| `spravca` | Property | `string` |  |  | `jc:spravca` | The source column `spravca`. |
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

### Rule

One row of Zoznam účinných všeobecne záväzných nariadení (VZN).

IRI: `sdm:dataModel.CPSV-AP/Rule`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cisloZasadnutia` | Property | `string` |  |  | `jc:cisloZasadnutia` | The source column `Číslo_zasadnutia`. |
| `description` | Property | `string` |  |  | `http://purl.org/dc/terms/description` | The source column `Názov_VZN`. |
| `iCOSamospravy` | Property | `string` |  |  | `jc:iCOSamospravy` | The source column `IČO_samosprávy`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovSamospravy` | Property | `string` |  |  | `jc:nazovSamospravy` | The source column `Názov_samosprávy`. |
| `platnostOd` | Property | `datetime` |  |  | `jc:platnostOd` | The source column `Platnosť_od`. |
| `uRLLinkaNaVZN` | Property | `string` |  |  | `jc:uRLLinkaNaVZN` | The source column `URL_linka_na_VZN`. |
| `ucinnostDo` | Property | `string` |  |  | `jc:ucinnostDo` | The source column `Účinnosť_do`. |
| `ucinnostOd` | Property | `datetime` |  |  | `jc:ucinnostOd` | The source column `Účinnosť_od`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### WaterBodyStatus

One row of Ekologický potenciál vodných útvarov; Ekologický stav vodných útvarov.

IRI: `jc:WaterBodyStatus`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `aCH` | Property | `integer` |  |  | `jc:aCH` | The source column `ACH`. |
| `aUT` | Property | `integer` |  |  | `jc:aUT` | The source column `AUT`. |
| `celkChemStav` | Property | `string` |  |  | `jc:celkChemStav` | The source column `celk_chem_stav`. |
| `charakter` | Property | `string` |  |  | `jc:charakter` | The source column `charakter`. |
| `charakter1` | Property | `string` |  |  | `jc:charakter1` | The source column `charakter_1`. |
| `dOW` | Property | `datetime` |  |  | `jc:dOW` | The source column `DOW`. |
| `dlzkaVU1` | Property | `float` |  |  | `jc:dlzkaVU1` | The source column `dlzka_VU_1`. |
| `ekologickyStav` | Property | `float` |  |  | `jc:ekologickyStav` | The source column `ekologicky_stav`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fIDKraj0` | Property | `integer` |  |  | `jc:fIDKraj0` | The source column `FID_kraj_0`. |
| `fIDRiekyBbskJoin` | Property | `integer` |  |  | `jc:fIDRiekyBbskJoin` | The source column `FID_rieky_bbsk_join`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `kODVU202` | Property | `string` |  |  | `jc:kODVU202` | The source column `KOD_VU_202`. |
| `kod` | Property | `string` |  |  | `jc:kod` | The source column `kod`. |
| `nAZOVVUD` | Property | `string` |  |  | `jc:nAZOVVUD` | The source column `NAZOV_VU_D`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Miesto`. |
| `nazovVU1` | Property | `string` |  |  | `jc:nazovVU1` | The source column `nazov_VU_1`. |
| `pOVODIE` | Property | `string` |  |  | `jc:pOVODIE` | The source column `POVODIE`. |
| `pOZN1` | Property | `string` |  |  | `jc:pOZN1` | The source column `POZN_1`. |
| `pOZNAMKA` | Property | `string` |  |  | `jc:pOZNAMKA` | The source column `POZNAMKA`. |
| `potencial` | Property | `string` |  |  | `jc:potencial` | The source column `potencial`. |
| `rkmDo1` | Property | `float` |  |  | `jc:rkmDo1` | The source column `rkm_do_1`. |
| `rkmOd1` | Property | `float` |  |  | `jc:rkmOd1` | The source column `rkm_od_1`. |
| `sOI` | Property | `integer` |  |  | `jc:sOI` | The source column `SOI`. |
| `sUBUNIT` | Property | `string` |  |  | `jc:sUBUNIT` | The source column `SUB_UNIT`. |
| `skupina` | Property | `float` |  |  | `jc:skupina` | The source column `skupina`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `typ`. |
| `vYMERA` | Property | `float` |  |  | `jc:vYMERA` | The source column `VYMERA`. |
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
