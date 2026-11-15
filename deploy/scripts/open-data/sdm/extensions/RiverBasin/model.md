# RiverBasin, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A river basin of the region. Extends dataModel.PointOfInterest/PointOfInterest: OpenStreetMap OSMWater requires osmId and osmType, which the region's register has not.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/RiverBasin`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### RiverBasin

A river basin of the region.

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
| `link` | Property | `string` |  |  | `jc:link` | The source column `link`. |
| `nM2` | Property | `string` |  |  | `jc:nM2` | The source column `NM2`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `nameText`. |
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

### Entity

The root every NGSI-LD entity class specialises.

IRI: `ngsi-ld:Entity`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |
