# Bratislava, open data

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the space bratislava carries (DM-01, DM-61): the open data of the City of Bratislava (data.bratislava.sk) as Smart Data Models types, their IRIs cited rather than minted (DM-04, DM-16). The first type is AirQualityObserved, one entity per air-quality station in the city with the PM10 concentration the city publishes for it; each batch of the ranked matrix adds its own classes: the city districts, places and services, the hub's district tables as key performance indicators, the city's counter sites (TrafficFlowObserved for cyclists and motor vehicles, CrowdFlowObserved for pedestrians), the Slovnaft BAjk bike stations (BikeHireDockingStation) and the public transport network of Dopravný podnik Bratislava from its GTFS timetable (GtfsStop, GtfsRoute, GtfsAgency).

- Namespace: `https://joinedcontext.com/models/bratislava/bratislava`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AirQualityObserved

One air-quality station of Bratislava and its PM10 mean.

IRI: `sdm:AirQualityObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's reading covers, UTC. |
| `pm10` | Property | `float` |  | ug/m3 | `sdm:pm10` | Particulate matter up to 10 µm, the period's mean at the station. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### AdministrativeArea

One of the 17 city districts (mestské časti) of Bratislava, its outline and its ZÚJ code, as the Smart Data Model extension AdministrativeArea (PointOfInterest base).

IRI: `jc:AdministrativeArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `districtCode` | Property | `string` | yes |  | `jc:districtCode` | The district's ZÚJ code (základná územná jednotka), the key the national registers use. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One place a resident goes to: a barrier-free place, a lifelong-learning provider, a needle drop point or a service for homeless people, with its own coordinates from the hub.

IRI: `sdm:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `category` | Property | `string` (list) |  |  | `sdm:category` | The kind of place (its table's code first, then the table's own word, such as `barrier-free`, `Kultúra, kostoly`), or of indicator (`quantitative`). |
| `description` | Property | `string` |  |  | `schema:description` | What the place offers or how it is reached, or the hub table an indicator comes from. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The Smart Data Models PostalAddress of the place, its street, district and city. |
| `url` | Property | `uri` |  |  | `schema:url` | The place's own web page. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KeyPerformanceIndicator

One figure of one hub table for one city district and one period: a count, a rate or an area, such as the offences the city police recorded in Nové Mesto in 2025.

IRI: `sdm:KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `description` | Property | `string` |  |  | `schema:description` | What the place offers or how it is reached, or the hub table an indicator comes from. |
| `process` | Property | `string` |  |  | `sdm:process` | What the indicator counts or measures, in the hub's words, with its unit. |
| `category` | Property | `string` (list) |  |  | `sdm:category` | The kind of place (its table's code first, then the table's own word, such as `barrier-free`, `Kultúra, kostoly`), or of indicator (`quantitative`). |
| `kpiValue` | Property | `float` |  |  | `sdm:kpiValue` | The figure for the district and period, as the hub's table states it. |
| `calculationPeriod` | Property | [`KpiPeriod`](#kpiperiod) |  |  | `sdm:calculationPeriod` | The period the figure covers, its first and last day. |
| `calculationFrequency` | Property | `string` |  |  | `sdm:calculationFrequency` | How often the hub publishes the figure (yearly, quarterly, every five years). |
| `area` | Property | `string` |  |  | `sdm:area` | The city district the figure describes, by its name. |
| `organization` | Property | `string` |  |  | `sdm:organization` | The body the figure describes, the City of Bratislava. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PostalAddress

The value of `address`: a postal address, never an entity of its own (Smart Data Models' common schema).

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and number as the publisher writes them. |
| `district` | Property | `string` |  |  | `sdm:district` | The city district (mestská časť) as the publisher writes it. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The city, Bratislava. |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country, as an ISO 3166-1 alpha-2 code. |

### KpiPeriod

The value of `calculationPeriod`, never an entity of its own.

IRI: `jc:KpiPeriod`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `from` | Property | `date` | yes |  | `sdm:from` | The first day of the period. |
| `to` | Property | `date` | yes |  | `sdm:to` | The last day of the period. |

### TrafficFlowObserved

One day of one of the city's counter sites in one direction: the cyclists or the motor vehicles it counted, from the hub's hourly tables. The site's name and place ride on the observation; the history of the days is the entity's temporal history.

IRI: `sdm:TrafficFlowObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's reading covers, UTC. |
| `dateObservedFrom` | Property | `datetime` |  |  | `sdm:dateObservedFrom` | The start of the counted day, the Bratislava midnight that opens it, UTC. |
| `dateObservedTo` | Property | `datetime` |  |  | `sdm:dateObservedTo` | The end of the counted day, the Bratislava midnight that closes it, UTC. |
| `intensity` | Property | `integer` |  | /d | `sdm:intensity` | How many bicycles, or motor vehicles, the counter site detected in the day in one direction. |
| `vehicleType` | Property | [`CountedVehicleType`](#countedvehicletype) |  |  | `sdm:vehicleType` | The one kind of vehicle the observation counts; absent when the site sums every motor vehicle. |
| `laneDirection` | Property | [`LaneDirection`](#lanedirection) |  |  | `sdm:laneDirection` | The direction the counted traffic travels: forward towards the place the city's counter names first (SMER_DO), backward towards the second (SMER_Z, or against a one-way street). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CrowdFlowObserved

One day of one of the city's counter sites: the pedestrians it counted, each direction apart.

IRI: `sdm:CrowdFlowObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's reading covers, UTC. |
| `dateObservedFrom` | Property | `datetime` |  |  | `sdm:dateObservedFrom` | The start of the counted day, the Bratislava midnight that opens it, UTC. |
| `dateObservedTo` | Property | `datetime` |  |  | `sdm:dateObservedTo` | The end of the counted day, the Bratislava midnight that closes it, UTC. |
| `peopleCount` | Property | `integer` |  | /d | `sdm:peopleCount` | The pedestrians the counter site detected in the day, both directions. |
| `peopleCountTowards` | Property | `integer` |  | /d | `sdm:peopleCountTowards` | The pedestrians of the day walking towards the place the counter names first (SMER_DO). |
| `peopleCountAway` | Property | `integer` |  | /d | `sdm:peopleCountAway` | The pedestrians of the day walking towards the second place the counter names (SMER_Z). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### BikeHireDockingStation

One Slovnaft BAjk station of nextbike's Bratislava system and what it holds right now.

IRI: `sdm:BikeHireDockingStation`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `totalSlotNumber` | Property | `integer` |  |  | `sdm:totalSlotNumber` | The number of docks the station has, where the feed names one. |
| `availableBikeNumber` | Property | `integer` |  |  | `sdm:availableBikeNumber` | Bikes ready to rent at the station right now. |
| `freeSlotNumber` | Property | `integer` |  |  | `sdm:freeSlotNumber` | Empty docks a bike can be returned to right now. |
| `status` | Property | [`StationStatus`](#stationstatus) |  |  | `sdm:status` | Whether the station rents bikes right now. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the station last reported the values beside it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsStop

One platform of a DPB stop (GTFS stop of location type 0): its name, the stop number and platform letter on its sign, its fare zone and where it is. The lines stopping there come through trips.txt and stop_times.txt, which are not read.

IRI: `sdm:GtfsStop`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `stopCode` | Property | `string` |  |  | `jc:stopCode` | The number DPB gives a stop, shared by its platforms (93 for every platform of Hlavná stanica). |
| `platformCode` | Property | `string` |  |  | `jc:platformCode` | The platform letter of a stop (A, B), as the sign shows it. |
| `zoneCode` | Property | `string` |  |  | `sdm:zoneCode` | The fare zone of the Bratislava integrated transport system the stop lies in (100 the city core, 101 the rest of the city). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsRoute

One DPB line as its GTFS timetable gives it, every variant, with its number, what runs it, its colour and the agency.

IRI: `sdm:GtfsRoute`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `shortName` | Property | `string` |  |  | `sdm:shortName` | The line number riders see, such as 4, 40 or N29. |
| `routeType` | Property | `string` |  |  | `sdm:routeType` | GTFS's basic route type: 0 a tram, 3 a bus. A trolleybus (GTFS 11, which Smart Data Models' list lacks) is written as 3, and transportMode says trolleybus. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What runs the line. |
| `routeColor` | Property | `string` |  |  | `sdm:routeColor` | The line's colour on DPB's maps and signs, six hexadecimal digits without the hash (F56200). |
| `operatedBy` | Relationship | [`GtfsAgency`](#gtfsagency) |  |  | `sdm:operatedBy` | The agency that runs the line. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsAgency

The transit agency DPB's GTFS timetable names, Dopravný podnik Bratislava itself.

IRI: `sdm:GtfsAgency`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `agencyName` | Property | `string` |  |  | `sdm:agencyName` | The transit agency's full name. |
| `page` | Property | `uri` |  |  | `sdm:page` | The publisher's web page about it. |
| `timezone` | Property | `string` |  |  | `sdm:timezone` | The agency's time zone, an IANA name such as Europe/Bratislava. |
| `language` | Property | `string` |  |  | `sdm:language` | The agency's main language, a two-letter code. |
| `phone` | Property | `string` |  |  | `sdm:phone` | The agency's public customer service number. |
| `routes` | Relationship | [`GtfsRoute`](#gtfsroute) (list) |  |  | `jc:routes` | The lines the agency operates, computed from their operatedBy and never stored. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OnStreetParking

One tourist coach stop of the city's list (data.bratislava.sk), placed by its name with OpenStreetMap; the list carries no coordinates.

IRI: `sdm:OnStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name of the station, counter site, stop, line, district, place or indicator as its publisher writes it. |
| `category` | Property | [`CoachStopCategory`](#coachstopcategory) (list) |  |  | `sdm:category` | The kind of place (its table's code first, then the table's own word, such as `barrier-free`, `Kultúra, kostoly`), or of indicator (`quantitative`). |
| `allowedVehicleType` | Property | `string` (list) |  |  | `sdm:allowedVehicleType` | What may stop there; a tourist coach stop of the city's list is for coaches (bus). |
| `totalSpotNumber` | Property | `integer` |  |  | `sdm:totalSpotNumber` | The coach places of a long-term stop as the city's list counts them. |
| `description` | Property | `string` |  |  | `schema:description` | What the place offers or how it is reached, or the hub table an indicator comes from. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The hub page or open-data service of the dataset the entity was read from. |
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

## Enumerations

### CountedVehicleType

The kinds of vehicle a city counter tells apart, a subset of Smart Data Models' vehicleType.

| Value | Meaning |
|---|---|
| `bicycle` | A bicycle, as the city's counter sites count it. |

### LaneDirection

| Value | Meaning |
|---|---|
| `forward` | Towards the first place the counter names. |
| `backward` | Towards the second place the counter names, or against a one-way street. |

### StationStatus

| Value | Meaning |
|---|---|
| `working` | The station rents bikes and takes them back. |
| `outOfService` | The station neither rents nor takes back bikes right now. |

### TransportMode

| Value | Meaning |
|---|---|
| `tram` | Tram (GTFS route type 0). |
| `bus` | Bus (GTFS route type 3). |
| `trolleybus` | Trolleybus (GTFS route type 11). |

### CoachStopCategory

The OnStreetParking `category` values the city's tourist coach stops map to, a subset of the Smart Data Model's enum.

| Value | Meaning |
|---|---|
| `shortTerm` | A short-term stop: passengers get on and off. |
| `mediumTerm` | A long-term stop where a coach waits (the Smart Data Model has no longer term). |
