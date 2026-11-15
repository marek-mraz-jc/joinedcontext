# NoiseZone, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One area of a computed noise map: the ground where the day or the night sound level falls into one 5 dB band. Extends dataModel.PointOfInterest/PointOfInterest: Environment's NoiseLevelObserved holds a level measured at a sensor at a time, not the area a noise band covers.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/NoiseZone`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### NoiseZone

One area of a computed noise map: the ground where the day or the night sound level falls into one 5 dB band.

IRI: `jc:NoiseZone`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `assessmentPeriod` | Property | `string` |  |  | `jc:assessmentPeriod` | The part of the day the noise level holds for, day (06:00-22:00) or night (22:00-06:00). |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language, as the publisher writes it. |
| `noiseLevelHigh` | Property | `float` |  |  | `jc:noiseLevelHigh` | The upper bound of the area's 5 dB noise band, in decibels. |
| `noiseLevelLow` | Property | `float` |  |  | `jc:noiseLevelLow` | The lower bound of the area's 5 dB noise band, the A-weighted sound level the computed noise map gives 4 m above ground, in decibels. |
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
