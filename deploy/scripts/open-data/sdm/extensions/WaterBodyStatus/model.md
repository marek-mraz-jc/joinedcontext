# WaterBodyStatus, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A water body with its ecological status or potential class. Extends dataModel.PointOfInterest/PointOfInterest: WaterQualityObserved and WaterObserved require dateObserved and hold measured parameters; these rows are assessed classes of a located water body.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/WaterBodyStatus`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### WaterBodyStatus

A water body with its ecological status or potential class.

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `NAZOV_VU`. |
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
