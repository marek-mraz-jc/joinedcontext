# EmissionSource, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One stationary source of air pollution of a city's emission register: where it is, its fuel and its yearly emissions. Extends dataModel.PointOfInterest/PointOfInterest: Environment's AirQualityObserved and EnvironmentObserved hold concentrations measured at a station; no model holds a register entry of yearly emissions.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/EmissionSource`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### EmissionSource

One stationary source of air pollution of a city's emission register: where it is, its fuel and its yearly emissions.

IRI: `jc:EmissionSource`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `coEmission` | Property | `float` |  |  | `jc:coEmission` | The source's yearly emission of carbon monoxide, in tonnes a year. |
| `fuelType` | Property | `string` |  |  | `jc:fuelType` | The fuel group that dominates the source as the register writes it, such as zemní plyn, nafta or technologie (a process, not a fuel). |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language, as the publisher writes it. |
| `noxEmission` | Property | `float` |  |  | `jc:noxEmission` | The source's yearly emission of nitrogen oxides (NOx), in tonnes a year. |
| `pm10Emission` | Property | `float` |  |  | `jc:pm10Emission` | The source's yearly emission of PM10, in tonnes a year, as the register states it. |
| `pm25Emission` | Property | `float` |  |  | `jc:pm25Emission` | The source's yearly emission of PM2.5, in tonnes a year (REZZO 2 only). |
| `registerCategory` | Property | `string` |  |  | `jc:registerCategory` | Which part of the Prague emission register lists the source: REZZO 1 (large stationary sources, rezzo1) or REZZO 2 (medium sources, rezzo2), as IPR Praha publishes them. |
| `so2Emission` | Property | `float` |  |  | `jc:so2Emission` | The source's yearly emission of sulphur dioxide, in tonnes a year. |
| `vocEmission` | Property | `float` |  |  | `jc:vocEmission` | The source's yearly emission of volatile organic compounds, in tonnes a year. |
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
