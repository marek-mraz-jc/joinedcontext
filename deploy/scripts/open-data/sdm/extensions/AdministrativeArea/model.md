# AdministrativeArea, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A region, district or municipality: its boundary or centroid and its NUTS and LAU codes. Extends dataModel.PointOfInterest/PointOfInterest: OpenStreetMap OSMBoundary requires osmId and osmType, which the region's register has not.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/AdministrativeArea`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AdministrativeArea

A region, district or municipality: its boundary or centroid and its NUTS and LAU codes.

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
| `name` | Property | `string` |  |  | `sdm:name` | The source column `NM2`, `NM4`. |
| `typObce` | Property | `string` |  |  | `jc:typObce` | The source column `typ_obce`. |
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
