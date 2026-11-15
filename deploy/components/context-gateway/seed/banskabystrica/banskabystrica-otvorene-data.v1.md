# Banská Bystrica, the city's open data

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

The open data of the city of Banská Bystrica from the catalogue: registers and tables (addresses, contracts, orders, grants, council meetings, greenery, parking) and the city's news (Aktuality) as Articles.

- Namespace: `https://joinedcontext.com/models/banskabystrica/banskabystrica-otvorene-data`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Address

One row of Adresy; DTM_BB-Register-adries.

IRI: `sdm:dataModel.CPSV-AP/Address`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `addressArea` | Property | `string` (list) |  |  | `sdm:dataModel.CPSV-AP/addressArea` | The source column `Časť_obce`. |
| `cASTOBCE` | Property | `string` |  |  | `jc:cASTOBCE` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `cOID` | Property | `integer` |  |  | `jc:cOID` | The source column `CO_ID`. |
| `castObce` | Property | `string` |  |  | `jc:castObce` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `dPOSTA` | Property | `string` |  |  | `jc:dPOSTA` | The source column `DPOSTA`. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `iD` | Property | `float` |  |  | `jc:iD` | The source column `ID`. |
| `idRegad` | Property | `string` |  |  | `jc:idRegad` | The source column `id_regad`. |
| `kRAJ` | Property | `string` |  |  | `jc:kRAJ` | The source column `KRAJ`. |
| `kRAJID` | Property | `integer` |  |  | `jc:kRAJID` | The source column `KRAJ_ID`. |
| `oBEC` | Property | `string` |  |  | `jc:oBEC` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `oBECID` | Property | `integer` |  |  | `jc:oBECID` | The source column `OBEC_ID`. |
| `oKRES` | Property | `string` |  |  | `jc:oKRES` | The source column `OKRES`. |
| `oKRESID` | Property | `integer` |  |  | `jc:oKRESID` | The source column `OKRES_ID`. |
| `oRIENTC` | Property | `string` |  |  | `jc:oRIENTC` | The source column `ORIENT_C`. |
| `oRIENTCC` | Property | `string` |  |  | `jc:oRIENTCC` | The source column `ORIENT_C_C`. |
| `oRIENTCP` | Property | `string` |  |  | `jc:oRIENTCP` | The source column `ORIENT_C_P`. |
| `obec` | Property | `string` |  |  | `jc:obec` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `orientC` | Property | `string` |  |  | `jc:orientC` | The source column `Orient._č.`. |
| `pSC` | Property | `string` |  |  | `jc:pSC` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `postCode` | Property | `string` (list) |  |  | `sdm:dataModel.CPSV-AP/postCode` | The source column `PSC`. |
| `postName` | Property | `string` (list) |  |  | `sdm:dataModel.CPSV-AP/postName` | The source column `Obec`. |
| `sCHRANKY` | Property | `integer` |  |  | `jc:sCHRANKY` | The source column `SCHRANKY`. |
| `sUPISNEC` | Property | `string` |  |  | `jc:sUPISNEC` | The source column `SUPISNE_C`. |
| `supC` | Property | `string` |  |  | `jc:supC` | The source column `Súp._č.`. |
| `tYPBUDOVY` | Property | `integer` |  |  | `jc:tYPBUDOVY` | The source column `TYP_BUDOVY`. |
| `thoroughfare` | Property | `string` (list) |  |  | `sdm:dataModel.CPSV-AP/thoroughfare` | The source column `Ulica`. |
| `uLICA` | Property | `string` |  |  | `jc:uLICA` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `uLICAID` | Property | `integer` |  |  | `jc:uLICAID` | The source column `ULICA_ID`. |
| `ulica` | Property | `string` |  |  | `jc:ulica` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `zSJ` | Property | `string` |  |  | `jc:zSJ` | The source column `ZSJ`. |
| `zSJID` | Property | `float` |  |  | `jc:zSJID` | The source column `ZSJ_ID`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Article

One row of Aktuality.

IRI: `jc:Article`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `content` | Property | `string` |  |  | `jc:content` | The source column `content`. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `dateGmt` | Property | `datetime` |  |  | `jc:dateGmt` | The source column `date_gmt`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `id`. |
| `link` | Property | `string` |  |  | `jc:link` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `modifiedGmt` | Property | `datetime` |  |  | `jc:modifiedGmt` | The source column `modified_gmt`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `title`. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `title` | Property | `string` |  |  | `jc:title` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `url` | Property | `uri` |  |  | `jc:url` | The source column `link`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Contract

One row of Zmluvy BB.

IRI: `jc:Contract`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cenaCelkom` | Property | `string` |  |  | `jc:cenaCelkom` | The source column `Cena_celkom`. |
| `centralneCisloZmluvy` | Property | `string` |  |  | `jc:centralneCisloZmluvy` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `datumPodpisu` | Property | `string` |  |  | `jc:datumPodpisu` | The source column `Dátum_podpisu`. |
| `datumZverejnenia` | Property | `string` |  |  | `jc:datumZverejnenia` | The source column `Dátum_zverejnenia`. |
| `druh` | Property | `string` |  |  | `jc:druh` | The source column `Druh`. |
| `iCOOrganizacie` | Property | `string` |  |  | `jc:iCOOrganizacie` | The source column `IČO_organizácie`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `id`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `predmet` | Property | `string` |  |  | `jc:predmet` | The source column `Predmet`. |
| `rok` | Property | `integer` |  |  | `jc:rok` | The source column `Rok`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `Typ`. |
| `zmluvneStrany` | Property | `string` |  |  | `jc:zmluvneStrany` | The source column `Zmluvné_strany`. |
| `zmluvneStranySUdajomICO` | Property | `string` |  |  | `jc:zmluvneStranySUdajomICO` | The source column `Zmluvné_strany_s_údajom_IČO`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CouncilMeeting

One row of Zasadnutia orgánov samosprávy.

IRI: `jc:CouncilMeeting`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cZasadnutia` | Property | `integer` |  |  | `jc:cZasadnutia` | The source column `Č._zasadnutia`. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `datumZasadnutia` | Property | `string` |  |  | `jc:datumZasadnutia` | The source column `Dátum_zasadnutia`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `title`. |
| `organSamospravy` | Property | `string` |  |  | `jc:organSamospravy` | The source column `Orgán_samosprávy`. |
| `rok` | Property | `integer` |  |  | `jc:rok` | The source column `Rok`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Garden

One row of DTM_BB-register-verejnej-zelene-1.

IRI: `sdm:dataModel.ParksAndGardens/Garden`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `datumplatnosti` | Property | `datetime` |  |  | `jc:datumplatnosti` | The source column `datumplatnosti`. |
| `datumvkladu` | Property | `datetime` |  |  | `jc:datumvkladu` | The source column `datumvkladu`. |
| `datumzmeny` | Property | `datetime` |  |  | `jc:datumzmeny` | The source column `datumzmeny`. |
| `ics` | Property | `string` |  |  | `jc:ics` | The source column `ics`. |
| `idzmeny` | Property | `string` |  |  | `jc:idzmeny` | The source column `idzmeny`. |
| `popisobjektu` | Property | `string` |  |  | `jc:popisobjektu` | The source column `popisobjektu`. |
| `typudrzovanezelene` | Property | `integer` |  |  | `jc:typudrzovanezelene` | The source column `typudrzovanezelene`. |
| `urovenumisteniobjektuzps` | Property | `integer` |  |  | `jc:urovenumisteniobjektuzps` | The source column `urovenumisteniobjektuzps`. |
| `vrstva` | Property | `string` |  |  | `jc:vrstva` | The source column `vrstva`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Grant

One row of Dotácie.

IRI: `jc:Grant`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `akcia` | Property | `string` |  |  | `jc:akcia` | The source column `Akcia`. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `druh` | Property | `string` |  |  | `jc:druh` | The source column `Druh`. |
| `iCOZiadatela` | Property | `string` |  |  | `jc:iCOZiadatela` | The source column `IČO_žiadateľa`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `id`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `title`. |
| `pozadovaneProstriedky` | Property | `string` |  |  | `jc:pozadovaneProstriedky` | The source column `Požadované_prostriedky`. |
| `rokDotacie` | Property | `integer` |  |  | `jc:rokDotacie` | The source column `Rok_dotácie`. |
| `schvProstr` | Property | `string` |  |  | `jc:schvProstr` | The source column `Schv._prostr.`. |
| `stavVybav` | Property | `string` |  |  | `jc:stavVybav` | The source column `Stav_vybav.`. |
| `typ` | Property | `string` |  |  | `jc:typ` | The source column `Typ`. |
| `ziadatel` | Property | `string` |  |  | `jc:ziadatel` | The source column `Žiadateľ`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KeyPerformanceIndicator

One row of Počet občanov podľa dňa narodenia; Počet občanov podľa veku; Prírastky a úbytky občanov za jednotlivé roky.

IRI: `sdm:dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `den` | Property | `string` |  |  | `jc:den` | The source column `Deň`. |
| `narodenych` | Property | `integer` |  |  | `jc:narodenych` | The source column `Narodených`. |
| `narodenychM` | Property | `integer` |  |  | `jc:narodenychM` | The source column `Narodených_M`. |
| `narodenychZ` | Property | `integer` |  |  | `jc:narodenychZ` | The source column `Narodených_Ž`. |
| `odstahovanych` | Property | `integer` |  |  | `jc:odstahovanych` | The source column `Odsťahovaných`. |
| `odstahovanychM` | Property | `integer` |  |  | `jc:odstahovanychM` | The source column `Odsťahovaných_M`. |
| `odstahovanychZ` | Property | `integer` |  |  | `jc:odstahovanychZ` | The source column `Odsťahovaných_Ž`. |
| `pocet` | Property | `integer` |  |  | `jc:pocet` | The source column `Počet`. |
| `pocetMuzov` | Property | `integer` |  |  | `jc:pocetMuzov` | The source column `Počet_mužov`. |
| `pocetObcanovVDanomVeku` | Property | `string` |  |  | `jc:pocetObcanovVDanomVeku` | The source column `Počet_občanov_v_danom_veku`. |
| `pocetZien` | Property | `integer` |  |  | `jc:pocetZien` | The source column `Počet_žien`. |
| `pohlavie` | Property | `string` |  |  | `jc:pohlavie` | The source column `Pohlavie`. |
| `prirastok` | Property | `string` |  |  | `jc:prirastok` | The source column `Prírastok`. |
| `prirastokMuzi` | Property | `string` |  |  | `jc:prirastokMuzi` | The source column `Prírastok_muži`. |
| `prirastokZeny` | Property | `string` |  |  | `jc:prirastokZeny` | The source column `Prírastok_ženy`. |
| `pristahovanych` | Property | `integer` |  |  | `jc:pristahovanych` | The source column `Prisťahovaných`. |
| `pristahovanychM` | Property | `integer` |  |  | `jc:pristahovanychM` | The source column `Prisťahovaných_M`. |
| `rok` | Property | `integer` |  |  | `jc:rok` | The source column `Rok`. |
| `ubytok` | Property | `string` |  |  | `jc:ubytok` | The source column `Úbytok`. |
| `ubytokMuzi` | Property | `string` |  |  | `jc:ubytokMuzi` | The source column `Úbytok_muži`. |
| `ubytokZeny` | Property | `string` |  |  | `jc:ubytokZeny` | The source column `Úbytok_ženy`. |
| `vek` | Property | `integer` |  |  | `jc:vek` | The source column `Vek`. |
| `zomrelych` | Property | `integer` |  |  | `jc:zomrelych` | The source column `Zomrelých`. |
| `zomrelychM` | Property | `integer` |  |  | `jc:zomrelychM` | The source column `Zomrelých_M`. |
| `zomrelychZ` | Property | `integer` |  |  | `jc:zomrelychZ` | The source column `Zomrelých_Ž`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OffStreetParking

One row of DTM_BB-register-statickej-dopravy.

IRI: `sdm:dataModel.Parking/OffStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `datumplatnosti` | Property | `datetime` |  |  | `jc:datumplatnosti` | The source column `datumplatnosti`. |
| `datumvkladu` | Property | `datetime` |  |  | `jc:datumvkladu` | The source column `datumvkladu`. |
| `datumzmeny` | Property | `datetime` |  |  | `jc:datumzmeny` | The source column `datumzmeny`. |
| `ics` | Property | `string` |  |  | `jc:ics` | The source column `ics`. |
| `idzmeny` | Property | `string` |  |  | `jc:idzmeny` | The source column `idzmeny`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `title`. |
| `popisobjektu` | Property | `string` |  |  | `jc:popisobjektu` | The source column `popisobjektu`. |
| `prevazujicipovrch` | Property | `integer` |  |  | `jc:prevazujicipovrch` | The source column `prevazujicipovrch`. |
| `urovenumisteniobjektuzps` | Property | `integer` |  |  | `jc:urovenumisteniobjektuzps` | The source column `urovenumisteniobjektuzps`. |
| `vrstva` | Property | `string` |  |  | `jc:vrstva` | The source column `vrstva`. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PurchaseOrder

One row of Objednávky BB.

IRI: `jc:PurchaseOrder`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `cisloObjednavky` | Property | `string` |  |  | `jc:cisloObjednavky` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `dataset` | Property | `string` |  |  | `jc:dataset` | An attribute an earlier version of the mapping wrote; the next run removes it. |
| `datumVystavenia` | Property | `string` |  |  | `jc:datumVystavenia` | The source column `Dátum_vystavenia`. |
| `datumZverejnenia` | Property | `string` |  |  | `jc:datumZverejnenia` | The source column `Dátum_zverejnenia`. |
| `dodavatel` | Property | `string` |  |  | `jc:dodavatel` | The source column `Dodávateľ`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `IČO`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `id`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `title`. |
| `objednavkaSpolu` | Property | `string` |  |  | `jc:objednavkaSpolu` | The source column `Objednávka_spolu`. |
| `textObjednavky` | Property | `string` |  |  | `jc:textObjednavky` | The source column `Text_objednávky`. |
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
