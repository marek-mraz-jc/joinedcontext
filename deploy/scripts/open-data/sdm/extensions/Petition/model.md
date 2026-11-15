# Petition, an extension of dataModel.CPSV-AP/Evidence

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

A petition filed with the publisher and how it was settled. Extends dataModel.CPSV-AP/Evidence: CPSV-AP Evidence is the official model of a document filed with a public body (identifier, name).

- Namespace: `https://joinedcontext.com/models/sdm/extensions/Petition`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Petition

A petition filed with the publisher and how it was settled.

IRI: `jc:Petition`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datumDorucenia` | Property | `date` |  |  | `jc:datumDorucenia` | The source column `Dátum_doručenia`. |
| `datumVybavenia` | Property | `date` |  |  | `jc:datumVybavenia` | The source column `Dátum_vybavenia`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `P_č_`. |
| `jeKvorova` | Property | `string` |  |  | `jc:jeKvorova` | The source column `Je_kvórová`. |
| `link` | Property | `string` |  |  | `jc:link` | The source column `Link`. |
| `name` | Property | `string` |  |  | `sdm:name` | The source column `Názov_petície`. |
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

### Entity

The root every NGSI-LD entity class specialises.

IRI: `ngsi-ld:Entity`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |
