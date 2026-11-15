# TrafficSignalJunction, an extension of dataModel.PointOfInterest/PointOfInterest

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One junction or crossing with traffic lights or warning lights: where it is, its installation number, its signal kind and whether it is in service. Extends dataModel.PointOfInterest/PointOfInterest: the pinned Smart Data Models hold no traffic signal or signalised junction, and PointOfInterest's category lists services and amenities.

- Namespace: `https://joinedcontext.com/models/sdm/extensions/TrafficSignalJunction`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### TrafficSignalJunction

One junction or crossing with traffic lights or warning lights: where it is, its installation number, its signal kind and whether it is in service.

IRI: `jc:TrafficSignalJunction`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who published the row, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The service the row was read from. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `description` | Property | `string` |  |  | `http://purl.org/dc/terms/description` | A short description, per language. |
| `junctionNumber` | Property | `string` |  |  | `jc:junctionNumber` | The city's own number of the signal installation, as its controller cabinet shows it. |
| `name` | Property | `string` |  |  | `sdm:name` | The name, per language. |
| `signalKind` | Property | `string` |  |  | `jc:signalKind` | What the junction's signals are: full traffic lights, or warning lights that only flash to warn of a crossing, a tram or a fire station exit. |
| `status` | Property | `string` |  |  | `jc:status` | A bike station's working while it rents and takes returns, otherwise outOfService; a park and ride site's open, closed or closedAbnormal (Fintraffic LIIPI's IN_OPERATION, INACTIVE or TEMPORARILY_CLOSED, EXCEPTIONAL_SITUATION). |
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
