# PublicInvestment, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A public investment project, with its place where the source has one. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body pays or charges (value, currency, identifier); no subject holds a contract, order, invoice, grant or procurement.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/PublicInvestment`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### PublicInvestment

A public investment project, with its place where the source has one.

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
| `datumZaniku` | Property | `string` |  |  | `jc:datumZaniku` | The source column `Dátum_zániku`. |
| `fACC` | Property | `string` |  |  | `jc:fACC` | The source column `FACC`. |
| `fLAT` | Property | `string` |  |  | `jc:fLAT` | The source column `F_LAT`. |
| `fLON` | Property | `string` |  |  | `jc:fLON` | The source column `F_LON`. |
| `gEOMLATBACKUP` | Property | `float` |  |  | `jc:gEOMLATBACKUP` | The source column `GEOM_LAT_BACKUP`. |
| `gEOMLONBACKUP` | Property | `float` |  |  | `jc:gEOMLONBACKUP` | The source column `GEOM_LON_BACKUP`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `IČO`. |
| `iCOOrganizacia` | Property | `string` |  |  | `jc:iCOOrganizacia` | The source column `IČO_organizácia`. |
| `iCOPrijimatela` | Property | `string` |  |  | `jc:iCOPrijimatela` | The source column `IČO_prijímateľa`. |
| `iDN2` | Property | `integer` |  |  | `jc:iDN2` | The source column `IDN2`. |
| `iDN3` | Property | `integer` |  |  | `jc:iDN3` | The source column `IDN3`. |
| `iDN4` | Property | `integer` |  |  | `jc:iDN4` | The source column `IDN4`. |
| `iDProjektu` | Property | `string` |  |  | `jc:iDProjektu` | The source column `ID_projektu`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Kód_projektu`, `ID_IA`. |
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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov_projektu`, `Názov`. |
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

### Entity

The root every NGSI-LD entity class specialises.

IRI: `ngsi-ld:Entity`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |
