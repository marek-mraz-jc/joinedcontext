# PurchaseOrder, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

An order the publisher issued and published. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body pays or charges (value, currency, identifier); no subject holds a contract, order, invoice, grant or procurement.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/PurchaseOrder`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### PurchaseOrder

An order the publisher issued and published.

IRI: `jc:PurchaseOrder`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the row belongs to, where its licence and its other rows are. |
| `datumVystavenia` | Property | `string` |  |  | `jc:datumVystavenia` | The source column `Dátum_vystavenia`. |
| `datumZverejnenia` | Property | `string` |  |  | `jc:datumZverejnenia` | The source column `Dátum_zverejnenia`. |
| `dodavatel` | Property | `string` |  |  | `jc:dodavatel` | The source column `Dodávateľ`. |
| `iCO` | Property | `string` |  |  | `jc:iCO` | The source column `IČO`. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The source column `Číslo_objednávky`. |
| `mena` | Property | `string` |  |  | `jc:mena` | The source column `Mena`. |
| `objednavkaSpolu` | Property | `string` |  |  | `jc:objednavkaSpolu` | The source column `Objednávka_spolu`. |
| `predmetObjednavky` | Property | `string` |  |  | `jc:predmetObjednavky` | The source column `Predmet_objednávky`. |
| `rok` | Property | `string` |  |  | `jc:rok` | The source column `Rok`. |
| `suma` | Property | `float` |  |  | `jc:suma` | The source column `Suma`. |
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
