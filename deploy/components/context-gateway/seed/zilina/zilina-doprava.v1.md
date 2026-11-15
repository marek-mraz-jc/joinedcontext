# Mobility of Žilina, live

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the city's own open data portal (opendata.zilina.sk, Invipo) publishes about moving and parking in Žilina: the occupancy of its car parks, its buses and trolleybuses with their delay, the hourly counts of its traffic counters and its street lights. One model, one space; Smart Data Models classes where they exist (DM-04, DM-16). Licence of the data: ODbL, credited as "Mesto Žilina". Sources: seeds/zilina-opendata/inventory.csv.

- Namespace: `https://joinedcontext.com/models/zilina/zilina-doprava`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### OffStreetParking

One car park of the city with its spaces and how many are free now.

IRI: `sdm:OffStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the city's open data portal writes it. |
| `availableSpotNumber` | Property | `integer` |  |  | `sdm:availableSpotNumber` | Free spaces at the car park when the portal last counted them. |
| `totalSpotNumber` | Property | `integer` |  |  | `sdm:totalSpotNumber` | Spaces the car park counts in total. |
| `occupiedSpotNumber` | Property | `integer` |  |  | `sdm:occupiedSpotNumber` | Taken spaces at the car park, total minus free. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | When the portal last reported the vehicle or the car park. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data and the credit its licence asks for: "Mesto Žilina, ODbL". |
| `source` | Property | `uri` |  |  | `sdm:source` | The portal API the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Vehicle

One bus or trolleybus of DPMŽ in service: where it is, its line and how late it runs.

IRI: `sdm:Vehicle`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the city's open data portal writes it. |
| `vehicleType` | Property | `string` |  |  | `sdm:vehicleType` | The kind of vehicle, as the portal names it (Bus, Trolleybus). |
| `lineName` | Property | `string` |  |  | `jc:lineName` | The line the vehicle runs, its short name (3, 24). |
| `direction` | Property | `string` |  |  | `jc:direction` | The last stop the vehicle heads for. |
| `nextStop` | Property | `string` |  |  | `jc:nextStop` | The name of the stop the vehicle reaches next. |
| `delay` | Property | `integer` |  | s | `jc:delay` | Seconds behind the timetable, negative when ahead (the portal's deviation in minutes times 60, rounded). |
| `heading` | Property | `integer` |  | deg | `sdm:heading` | Where the vehicle points, degrees clockwise from north. |
| `occupancy` | Property | `integer` |  | % | `jc:occupancy` | How full the vehicle is, in percent, as the portal estimates it. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | When the portal last reported the vehicle or the car park. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data and the credit its licence asks for: "Mesto Žilina, ODbL". |
| `source` | Property | `uri` |  |  | `sdm:source` | The portal API the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### TrafficFlowObserved

One traffic counter of the city and what it counted in the last full hour.

IRI: `sdm:TrafficFlowObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the city's open data portal writes it. |
| `intensity` | Property | `integer` |  | /h | `sdm:intensity` | Vehicles the counter saw in the hour. |
| `averageVehicleSpeed` | Property | `float` |  | km/h | `sdm:averageVehicleSpeed` | The mean speed of the vehicles counted in the hour. |
| `dateObservedFrom` | Property | `datetime` |  |  | `sdm:dateObservedFrom` | The start of the hour the count covers. |
| `dateObservedTo` | Property | `datetime` |  |  | `sdm:dateObservedTo` | The end of the hour the count covers. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data and the credit its licence asks for: "Mesto Žilina, ODbL". |
| `source` | Property | `uri` |  |  | `sdm:source` | The portal API the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Streetlight

One street light of the city, its name and place.

IRI: `sdm:Streetlight`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the city's open data portal writes it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data and the credit its licence asks for: "Mesto Žilina, ODbL". |
| `source` | Property | `uri` |  |  | `sdm:source` | The portal API the entity was read from. |
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
