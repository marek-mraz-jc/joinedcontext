# BudgetLine, an extension of dataModel.CPSV-AP/Cost

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One line of a city budget in the national budget classification, with the approved and the adjusted amount. Extends dataModel.CPSV-AP/Cost: CPSV-AP Cost is the official model of an amount a public body sets (identifier, value, currency); no subject holds a budget line.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/BudgetLine`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### BudgetLine

One line of a city budget in the national budget classification, with the approved and the adjusted amount.

IRI: `jc:BudgetLine`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `adjustedAmount` | Property | `float` |  |  | `jc:adjustedAmount` | The amount after the year's budget changes so far, in the same unit as the approved one. |
| `approvedAmount` | Property | `float` |  |  | `jc:approvedAmount` | The amount the council approved, as the city publishes it. The city's documentation names no unit; the 2026 expenditure lines add up to 119 054 085, which is the size of Prague's budget in thousands of CZK. |
| `budgetArea` | Property | `string` |  |  | `jc:budgetArea` | The city's own area of the line (oblast), its code and name joined, such as 09 Školství; absent on a correction line the city files under none. |
| `budgetFunction` | Property | `string` |  |  | `jc:budgetFunction` | The section of the national budget classification (paragraf, ODPA), code and name. |
| `budgetItem` | Property | `string` |  |  | `jc:budgetItem` | The item of the national budget classification (položka), code and name. Items 1xxx to 4xxx are revenue, 5xxx and 6xxx expenditure, 8xxx financing. |
| `budgetPurpose` | Property | `string` |  |  | `jc:budgetPurpose` | The earmark of the line (účelový znak), code and name. |
| `fiscalYear` | Property | `integer` |  |  | `jc:fiscalYear` | The budget year. |
| `identifier` | Property | `string` |  |  | `sdm:dataModel.CPSV-AP/identifier` | The budget line's own key, the year and its codes of area, function, item and purpose (`mhmp-2026-00-612100-5011-000000000`), the identifier the BudgetLine extension requires. |
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
