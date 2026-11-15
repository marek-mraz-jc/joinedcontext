# Helsinki city context

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the Helsinki demonstration space carries side by side (DM-01, DM-09): the events of the city's Linked Events register, the HSL city bike docking stations, the HSL buses of four trunk lines, HSL's stops, stations, lines and their shapes, the city's news, Fintraffic's road weather stations and its traffic alerts, FMI's air quality stations, and the city's open registers: its services (libraries, health stations, swimming halls, beaches, schools), the beach water sensors, parking areas and zones, the permits for digging, traffic arrangements and events on public areas, and its district division. One model, one space, four endpoints that each publish a slice of it. The IRIs are Smart Data Models' and ETSI's, cited rather than minted, so a reader who knows those vocabularies reads the same terms here (DM-04, DM-16).

- Namespace: `https://hel.fi/models/helsinki/helsinki`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Event

One event of the City of Helsinki's Linked Events register.

IRI: `sdm:Event`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `startDate` | Property | `datetime` |  |  | `schema:startDate` | When the event starts, ISO 8601 with a time zone. |
| `endDate` | Property | `datetime` |  |  | `schema:endDate` | When the event ends; absent while the register does not say. |
| `eventStatus` | Property | [`EventStatus`](#eventstatus) |  |  | `schema:eventStatus` | The register's status of the event, one of schema.org's EventStatusType values. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the source publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent, never guessed from a line of text. |
| `thematic` | Property | `string` (list) |  |  | `sdm:thematic` | The event's keywords from the register's YSO terms, in English where the term has an English name and in Finnish otherwise (Smart Data Models Event `thematic`). |
| `isAccessibleForFree` | Property | `boolean` |  |  | `schema:isAccessibleForFree` | True when every offer of the event is free of charge, as the register marks it. |
| `webSite` | Property | `uri` |  |  | `sdm:webSite` | The organizer's page for the event, the register's `info_url` in Finnish, else English or Swedish. |
| `superEvent` | Relationship | [`Event`](#event) |  |  | `sdm:superEvent` | The recurring or umbrella event this event is one date or part of. |
| `subEvent` | Relationship | [`Event`](#event) (list) |  |  | `sdm:subEvent` | The dates and parts of this event, computed from their superEvent and never stored. |
| `contactPoint` | Property | `string` |  |  | `schema:contactPoint` | The organizer's contact line as the register carries it, often a named person with a phone number. The space keeps it for the city's own coordination; no public endpoint serves it. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `refPointOfInterest` | Relationship | `string` |  |  | `sdm:refPointOfInterest` | The venue as the city's service register knows it: a Linked Events `tprek:` place is the Service Map unit of that number, written by the services pipeline. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### BikeHireDockingStation

One HSL city bike station, its position and what it holds right now.

IRI: `sdm:BikeHireDockingStation`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `availableBikeNumber` | Property | `integer` |  |  | `sdm:availableBikeNumber` | Bikes ready to rent at the station right now. |
| `freeSlotNumber` | Property | `integer` |  |  | `sdm:freeSlotNumber` | Empty docks a bike can be returned to right now. |
| `totalSlotNumber` | Property | `integer` |  |  | `sdm:totalSlotNumber` | Bikes plus free docks, the station's working size. |
| `status` | Property | `string` |  |  | `sdm:status` | A bike station's working while it rents and takes returns, otherwise outOfService; a park and ride site's open, closed or closedAbnormal (Fintraffic LIIPI's IN_OPERATION, INACTIVE or TEMPORARILY_CLOSED, EXCEPTIONAL_SITUATION). |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Vehicle

One HSL bus, tram, metro, train or ferry in service, where it is, how fast it moves and how late it runs.

IRI: `sdm:Vehicle`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `speed` | Property | `float` |  | m/s | `sdm:speed` | Ground speed of the vehicle. |
| `heading` | Property | `float` |  | deg | `sdm:heading` | Compass heading of the vehicle, degrees clockwise from north. |
| `route` | Property | `string` |  |  | `sdm:route` | The HSL route id the vehicle is serving, as GTFS names it (2550, 1009, 31M1, 3001K). |
| `vehicleType` | Property | `string` |  |  | `sdm:vehicleType` | The Smart Data Models kind of vehicle - bus, tram, passengerVessel for a ferry, other for metro and train; transportMode names HSL's mode. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `fleetVehicleId` | Property | `string` |  |  | `sdm:fleetVehicleId` | The operator and vehicle number HSL publishes, operator-vehicle. |
| `onRoute` | Relationship | `uriorcurie` |  |  | `jc:onRoute` | The line the vehicle is running, the URN of a GtfsRoute of this space. |
| `nextStop` | Relationship | `uriorcurie` |  |  | `jc:nextStop` | The stop the vehicle stands at or comes to next, the URN of a GtfsStop of this space; HSL moves it on at each departure from or passing of a stop. |
| `delay` | Property | `integer` |  | s | `jc:delay` | Seconds behind the timetable, negative when ahead (HSL's dl with the sign turned). |
| `occupancy` | Property | `integer` |  | % | `jc:occupancy` | How full the vehicle is, in percent; HSL sends 0 (room) or 100 (full). |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Article

One item of the City of Helsinki's news feed, as the Smart Data Model extension Article (CPSV-AP Evidence base).

IRI: `jc:Article`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `identifier` | Property | `string` | yes |  | `sdm:dataModel.CPSV-AP/identifier` | The publisher's own identifier of the record (a news item's RSS guid), the one the Article extension requires. |
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `url` | Property | `uri` |  |  | `schema:url` | Where the item is published in full. |
| `datePublished` | Property | `datetime` |  |  | `schema:datePublished` | When the item was published, ISO 8601 with a time zone. |
| `image` | Property | `uri` |  |  | `schema:image` | The item's picture, when the feed carries one. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### WeatherObserved

One Fintraffic road weather station around Helsinki and its latest readings.

IRI: `sdm:WeatherObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | When the readings were taken. |
| `temperature` | Property | `float` |  | Cel | `sdm:temperature` | The temperature the station measures: the air at a road weather station, the water at a beach sensor. |
| `roadSurfaceTemperature` | Property | `float` |  | Cel | `sdm:roadSurfaceTemperature` | Temperature of the road surface at the station. |
| `relativeHumidity` | Property | `float` |  |  | `sdm:relativeHumidity` | Relative humidity of the air, 0 to 1. |
| `windSpeed` | Property | `float` |  | m/s | `sdm:windSpeed` | Mean wind speed at the station. |
| `windDirection` | Property | `float` |  | deg | `sdm:windDirection` | Where the wind comes from, degrees clockwise from north. |
| `precipitation` | Property | `float` |  | mm/h | `sdm:precipitation` | Precipitation intensity at the station, millimetres per hour. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Alert

One road work or traffic announcement of Fintraffic in the capital region, or one service alert of HSL with the lines and stops it affects.

IRI: `sdm:Alert`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `category` | Property | `string` |  |  | `sdm:category` | The alert's domain; traffic for everything Fintraffic and HSL publish. |
| `subCategory` | Property | `string` |  |  | `sdm:subCategory` | The Smart Data Models code of the alert, roadWorks for a Fintraffic road work. Left out where no code fits: a Fintraffic traffic announcement and an HSL service alert. |
| `alertSource` | Property | `string` |  |  | `sdm:alertSource` | Who raised the alert, Fintraffic or HSL. |
| `effect` | Property | [`AlertEffect`](#alerteffect) |  |  | `jc:effect` | What an HSL service alert does to the service, one of GTFS-RT's Effect values. |
| `cause` | Property | [`AlertCause`](#alertcause) |  |  | `jc:cause` | Why an HSL service alert is in force, one of GTFS-RT's Cause values. |
| `severity` | Property | [`AlertSeverity`](#alertseverity) |  |  | `sdm:severity` | How much the alert matters, informational, medium or high (GTFS-RT INFO, WARNING, SEVERE). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the source publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent, never guessed from a line of text. |
| `dateIssued` | Property | `datetime` |  |  | `sdm:dateIssued` | When the situation was published. |
| `validFrom` | Property | `datetime` |  |  | `sdm:validFrom` | When the situation starts; for a preschool admission area, the first day of its school year. |
| `validTo` | Property | `datetime` |  |  | `sdm:validTo` | When the situation ends; absent while open-ended. For a preschool admission area, the last day of its school year. |
| `refRoute` | Relationship | [`GtfsRoute`](#gtfsroute) (list) |  |  | `jc:refRoute` | The lines an HSL service alert affects, each a GtfsRoute of this space. |
| `refStop` | Relationship | [`GtfsStop`](#gtfsstop) (list) |  |  | `jc:refStop` | The stops an HSL service alert affects, each a GtfsStop of this space. |
| `unknownReference` | Property | `string` (list) |  |  | `jc:unknownReference` | What an HSL service alert names that this space holds no entity for, as `route:<id>` or `stop:<id>`, kept as text rather than as a relationship that would point at nothing. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### AirQualityObserved

One air quality station around Helsinki and its newest hourly readings: FMI's stations, and the stations a steward adds by hand, which carry no source.

IRI: `sdm:AirQualityObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | When the readings were taken. |
| `pm10` | Property | `float` |  | ug/m3 | `sdm:pm10` | Particulate matter up to 10 µm, the hour's mean at the station. |
| `pm25` | Property | `float` |  | ug/m3 | `sdm:pm25` | Particulate matter up to 2.5 µm, the hour's mean at the station. |
| `airQualityIndex` | Property | `float` |  |  | `sdm:airQualityIndex` | FMI's hourly air quality index, 1 (good) to 5 (very poor). |
| `districtCode` | Property | `string` |  |  | `jc:districtCode` | The city's own code of the area: the district number, for a sub-district the full ten-digit code the city's statistics use for it, for a quarter the register's ten-digit quarter code, for a voting district the district number and a letter (049A). An HSL fare zone has none; its letter is its `fareZone`. The districts, sub-districts, quarters and voting-districts mappings write no area without it. |
| `stewardNote` | Property | `string` |  |  | `jc:stewardNote` | A note by the person who keeps this data: what was checked, what looks wrong, whom to ask. No pipeline writes it and no pipeline overwrites it, so an application may offer it for editing without the next run taking the words back (AP-62). At most 500 characters and no angle brackets, because it is a note and not a document. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One place of the city's service register a resident goes to: a library, a health station, a swimming hall, a beach, a school, or any other unit of the register (`other`). The register's contact lines are left out, because they can name a person.

IRI: `sdm:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `serviceCategory` | Property | [`ServiceCategory`](#servicecategory) | yes |  | `jc:serviceCategory` | Which of the city's services the place is, from the service register's own codes. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the source publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent, never guessed from a line of text. |
| `url` | Property | `uri` |  |  | `schema:url` | Where the item is published in full. |
| `openingHours` | LanguageProperty | `string` |  |  | `schema:openingHours` | The opening hours as the city's service register words them, per language. Free text, because the register writes exceptions and self-service hours no fixed shape holds. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `waterQualityObservations` | Relationship | [`WaterQualityObserved`](#waterqualityobserved) (list) |  |  | `jc:waterQualityObservations` | The water-quality readings taken at this place, computed from their refPointOfInterest and never stored. |
| `plotArea` | Property | `float` |  | m2 | `jc:plotArea` | The area of a plot the city offers to a business, in square metres, as the plot register states it. |
| `floorAreaRight` | Property | `float` |  | m2 | `jc:floorAreaRight` | The floor area the detailed plan lets a business build on the plot (rakennusoikeus), in square metres of floor area. |
| `admissionAreas` | Relationship | [`AdministrativeArea`](#administrativearea) (list) |  |  | `jc:admissionAreas` | The admission areas the unit serves, computed from their serviceUnits and never stored. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### WaterQualityObserved

One of Forum Virium Helsinki's beach sensors and the water temperature it last read. A sensor that has not reported this season carries no reading.

IRI: `sdm:WaterQualityObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `temperature` | Property | `float` |  | Cel | `sdm:temperature` | The temperature the station measures: the air at a road weather station, the water at a beach sensor. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | When the readings were taken. |
| `refPointOfInterest` | Relationship | [`PointOfInterest`](#pointofinterest) |  |  | `sdm:refPointOfInterest` | The place in the city's service register the sensor stands at. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OnStreetParking

One street parking area of the city with its outline: a parking area of the city's parking guide with the cars it holds when the city has estimated it (category `public`), a parking fee zone (`feeCharged`) or a resident parking zone (`forResidents`), a zone with its code and rules; or one parking place of the city's street parking map with its rules as the Smart Data Model's categories, or a tourist coach stop or parking place (`allowedVehicleType` bus).

IRI: `sdm:OnStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `category` | Property | [`StreetParkingCategory`](#streetparkingcategory) (list) | yes |  | `sdm:category` | Which of the city's street parking layers the area belongs to, in the Smart Data Model's own words. |
| `allowedVehicleType` | Property | `string` (list) |  |  | `sdm:allowedVehicleType` | What may park when the city's layer names it: bus for a tourist coach stop or parking place, motorcycle or bicycle, other for e-scooters. Absent means any vehicle the signs allow. |
| `totalSpotNumber` | Property | `integer` |  |  | `sdm:totalSpotNumber` | The number of vehicles the site holds, as the publisher states or estimates it; for a park and ride site, of its own kind (car or bicycle). |
| `zoneCode` | Property | `string` |  |  | `jc:zoneCode` | The zone's own code, a fee zone number or a resident permit letter, as the signs show it. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `url` | Property | `uri` |  |  | `schema:url` | Where the item is published in full. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OffStreetParking

One park and ride site of the HSL area from Fintraffic's LIIPI register, where it is, what it holds, whether it is open, the free spaces when it reports them, and its station.

IRI: `sdm:OffStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `status` | Property | `string` |  |  | `sdm:status` | A bike station's working while it rents and takes returns, otherwise outOfService; a park and ride site's open, closed or closedAbnormal (Fintraffic LIIPI's IN_OPERATION, INACTIVE or TEMPORARILY_CLOSED, EXCEPTIONAL_SITUATION). |
| `allowedVehicleType` | Property | `string` (list) |  |  | `sdm:allowedVehicleType` | What may park, the site's own kind first (car or bicycle), then motorcycle where it has spaces for them. |
| `totalSpotNumber` | Property | `integer` |  |  | `sdm:totalSpotNumber` | The number of vehicles the site holds, as the publisher states or estimates it; for a park and ride site, of its own kind (car or bicycle). |
| `capacityByType` | JsonProperty | `string` |  |  | `jc:capacityByType` | The built capacity per kind of space, as LIIPI names them in lower case (car, disabled, bicycle, electricCar, motorcycle, bicycleSecureSpace), each a count. |
| `availableSpotNumber` | Property | `integer` |  |  | `sdm:availableSpotNumber` | Free spaces at the park and ride site when it last reported, LIIPI's spacesAvailable. |
| `refNearestStation` | Relationship | [`GtfsStation`](#gtfsstation) |  |  | `jc:refNearestStation` | The HSL station within 500 m of the park and ride site, the nearest one, a GtfsStation of this space. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CityWork

One permit of the city's public-area register: an excavation, a temporary traffic arrangement or an event, with every area it covers as one outline. The applicant and the contractor are left out, because they can name a person. The Smart Data Model of works that occupy the street; an event's permit covers its build and teardown on it.

IRI: `sdm:CityWork`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `permitKind` | Property | [`PermitKind`](#permitkind) | yes |  | `jc:permitKind` | What the permit lets its holder do on the public area. |
| `permitNumber` | Property | `string` | yes |  | `jc:permitNumber` | The application number the city's permit register gives the permit, such as KP1900206-23. |
| `permitStatus` | Property | [`PermitStatus`](#permitstatus) | yes |  | `jc:permitStatus` | Where the permitted work or event stands today. |
| `permitStart` | Property | `date` | yes |  | `jc:permitStart` | The first day the permit covers. |
| `permitEnd` | Property | `date` | yes |  | `jc:permitEnd` | The last day the permit covers. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the source publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent, never guessed from a line of text. |
| `refDistrict` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refDistrict` | The district the record lies in. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### AdministrativeArea

One district or sub-district of Helsinki, its outline and its code, as the Smart Data Model extension AdministrativeArea (PointOfInterest base).

IRI: `jc:AdministrativeArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `districtCode` | Property | `string` | no |  | `jc:districtCode` | The city's own code of the area: the district number, for a sub-district the full ten-digit code the city's statistics use for it, for a quarter the register's ten-digit quarter code, for a voting district the district number and a letter (049A). An HSL fare zone has none; its letter is its `fareZone`. |
| `divisionLevel` | Property | [`DivisionLevel`](#divisionlevel) | yes |  | `jc:divisionLevel` | Which of the city's divisions the area belongs to: a district, sub-district, quarter or voting district for an AdministrativeArea; for a statistics indicator also the city, a major or basic district, or the inner city or the suburbs. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `publicAreaPermits` | Relationship | [`CityWork`](#citywork) (list) |  |  | `jc:publicAreaPermits` | The public-area permits that lie in this district, computed from their refDistrict and never stored. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `validFrom` | Property | `datetime` |  |  | `sdm:validFrom` | When the situation starts; for a preschool admission area, the first day of its school year. |
| `validTo` | Property | `datetime` |  |  | `sdm:validTo` | When the situation ends; absent while open-ended. For a preschool admission area, the last day of its school year. |
| `serviceUnits` | Relationship | [`PointOfInterest`](#pointofinterest) (list) |  |  | `jc:serviceUnits` | The units of the city's service register that serve the area: for a preschool admission area, the schools and day care centres that give its preschool education, each a PointOfInterest the services pipeline writes. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### HeritageArea

One area of the city's cultural heritage maps, its outline and its kind: a strongpoint of the First World War land fortifications, or an area of the landscape culture map (a park, a boulevard, a manor or villa environment, a residential ensemble...), as the Smart Data Model extension HeritageArea (PointOfInterest base).

IRI: `jc:HeritageArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `heritageCategory` | Property | [`HeritageCategory`](#heritagecategory) | yes |  | `jc:heritageCategory` | What kind of protected or valued cultural landscape the area is: a strongpoint of the First World War land fortifications, or one of the kinds of the city's landscape culture map. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### TrafficSignalJunction

One junction or crossing of the city with traffic lights or warning lights, where it is, the streets that meet there, its installation number and whether it is in service or still being built, as the Smart Data Model extension TrafficSignalJunction (PointOfInterest base).

IRI: `jc:TrafficSignalJunction`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `signalKind` | Property | [`TrafficSignalKind`](#trafficsignalkind) | yes |  | `jc:signalKind` | What the junction's signals are: full traffic lights, or warning lights that only flash to warn of a crossing, a tram or a fire station exit. |
| `junctionNumber` | Property | `string` |  |  | `jc:junctionNumber` | The city's own number of the signal installation, as its controller cabinet shows it. |
| `status` | Property | [`TrafficSignalStatus`](#trafficsignalstatus) | yes |  | `sdm:status` | inService, or underConstruction while the city still builds the signals (the layer's Rakenteilla). |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the publisher last changed the record, or the station last reported. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### NoiseZone

One area of the city's 2022 noise survey: the ground where the day or the night sound level of one traffic source falls into one 5 dB band, as the Smart Data Model extension NoiseZone (PointOfInterest base).

IRI: `jc:NoiseZone`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `noiseLevelLow` | Property | `float` |  | dB | `jc:noiseLevelLow` | The lower bound of the area's 5 dB noise band, the A-weighted equivalent sound level (LAeq) the city's 2022 noise survey computes 2 m above ground, in decibels. |
| `noiseLevelHigh` | Property | `float` |  | dB | `jc:noiseLevelHigh` | The upper bound of the area's 5 dB noise band, in decibels; the loudest band has none. |
| `assessmentPeriod` | Property | [`NoiseAssessmentPeriod`](#noiseassessmentperiod) | yes |  | `jc:assessmentPeriod` | The part of the day the noise level holds for, as the Finnish guideline values count it: the day, 07:00 to 22:00, or the night, 22:00 to 07:00. |
| `noiseSource` | Property | [`NoiseSource`](#noisesource) | yes |  | `jc:noiseSource` | The traffic whose noise the area's band computes: streets and roads, the main railway, the metro or the tram. A custom property: the NoiseZone extension holds one source per map. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Tree

One tree of the city's street and park tree register (puurekisteri): where it stands, its species and Finnish name, its trunk class and planting year. Smart Data Models has no tree class; the closest, GreenspaceRecord, records soil and weather readings of a green area.

IRI: `jc:Tree`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `species` | Property | `string` |  |  | `jc:species` | The tree's scientific name, genus and species as the register writes them (Tilia x vulgaris, Betula pendula); `sp.` stands for a species the register leaves open. |
| `trunkDiameterClass` | Property | [`TrunkDiameterClass`](#trunkdiameterclass) |  |  | `jc:trunkDiameterClass` | The class of the trunk's diameter at breast height the register gives the tree. |
| `plantingYear` | Property | `integer` |  |  | `jc:plantingYear` | The year the tree was planted, when the register knows it. |
| `treeSetting` | Property | [`TreeSetting`](#treesetting) |  |  | `jc:treeSetting` | Whether the tree stands along a street or in a park, as the register files it. |
| `treeSite` | Property | `string` |  |  | `jc:treeSite` | The street or the park the tree stands in, as the register names it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### NatureSite

One site of the city's nature information system (luontotietojärjestelmä): a protected or proposed nature site, or an area the city values for its flying squirrels, birds, bats, plants or geology, with its outline, name, value class and the system's description. Smart Data Models has no protected-area class; the closest, GreenspaceRecord, records soil and weather readings of a green area.

IRI: `jc:NatureSite`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `natureSiteCategory` | Property | [`NatureSiteCategory`](#naturesitecategory) | yes |  | `jc:natureSiteCategory` | Which layer of the city's nature information system the site comes from: a protected site (nature reserve, Natura 2000 area, natural monument, protected habitat type), a site of the national nature conservation programme still to be protected, or a site the city values for its species or its geology. |
| `valueClass` | Property | `string` |  |  | `jc:valueClass` | The value class the nature information system gives the site, in its own Finnish words (I hyvin arvokas, Geologinen arvo: suuri, Arvokas lepakkoalue); the protected sites carry none. |
| `areaHectares` | Property | `float` |  | har | `jc:areaHectares` | The area of the site's outline in hectares, as the nature information system computes it. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsStop

One HSL stop or station platform, where it is, its sign's code and its fare zone.

IRI: `sdm:GtfsStop`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `stopCode` | Property | `string` |  |  | `jc:stopCode` | The code on the stop's sign, such as H2014; empty for a stop HSL gives none. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `fareZone` | Property | `string` |  |  | `jc:fareZone` | The HSL fare zone the stop lies in, A to D; none for a stop beyond HSL's zones. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `wheelChairAccessible` | Property | `string` |  |  | `sdm:wheelChairAccessible` | Whether a wheelchair can board here, GTFS's wheelchair_boarding; 0 unknown, 1 possible, 2 not possible. |
| `hasParentStation` | Relationship | [`GtfsStation`](#gtfsstation) |  |  | `sdm:hasParentStation` | The station the stop or platform belongs to; absent for a stop on its own. |
| `departures` | JsonProperty | `string` |  |  | `jc:departures` | The next departures from the stop, at most five, soonest first, each an object with the line (`route`, the GtfsRoute's id, and `routeId`), its `directionId`, the `expected` time and its `status` (scheduled, skipped or canceled), and `delay` in seconds with the `scheduled` time it implies when HSL sends one. Set every 30 s for a curated set of stops from HSL's GTFS-RT trip updates; empty after five minutes without a feed. |
| `stopAlerts` | Relationship | [`Alert`](#alert) (list) |  |  | `jc:stopAlerts` | The HSL service alerts on the stop, computed from their refStop and never stored. |
| `page` | Property | `uri` |  |  | `sdm:page` | The publisher's web page about it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PublicTransportRoute

One HSL line in one direction: its number, where it runs to, its stops in order and the line through them. A line with several variants has one entity per variant. The Smart Data Model's generic public transport route; GtfsRoute is the line without a direction.

IRI: `sdm:PublicTransportRoute`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `route` | Property | `string` |  |  | `sdm:route` | The HSL route id the vehicle is serving, as GTFS names it (2550, 1009, 31M1, 3001K). |
| `routeShortName` | Property | `string` |  |  | `jc:routeShortName` | The line number riders see, such as 4 or 550. |
| `headsign` | Property | `string` |  |  | `jc:headsign` | Where the line runs to in this direction, as its sign shows it. |
| `directionId` | Property | `integer` |  |  | `jc:directionId` | HSL's direction of the line, 1 or 2. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `transportationType` | Property | `integer` | yes |  | `sdm:transportationType` | What runs the line as its GTFS route type (0 tram, 1 metro, 2 train, 3 bus, 4 ferry), the number the Smart Data Model requires beside the word `transportMode` gives. |
| `stopSequence` | Property | `string` (list) |  |  | `jc:stopSequence` | The stops the line serves in this direction, in order, each the GTFS stop id that ends the id of a GtfsStop of this space. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsStation

One HSL station (GTFS location type 1), where it is and the platforms it gathers.

IRI: `sdm:GtfsStation`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `fareZone` | Property | `string` |  |  | `jc:fareZone` | The HSL fare zone the stop lies in, A to D; none for a stop beyond HSL's zones. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `wheelChairAccessible` | Property | `string` |  |  | `sdm:wheelChairAccessible` | Whether a wheelchair can board here, GTFS's wheelchair_boarding; 0 unknown, 1 possible, 2 not possible. |
| `hasStop` | Relationship | [`GtfsStop`](#gtfsstop) (list) |  |  | `sdm:hasStop` | The platforms of the station, computed from their hasParentStation and never stored. |
| `nearbyParking` | Relationship | [`OffStreetParking`](#offstreetparking) (list) |  |  | `jc:nearbyParking` | The park and ride sites whose nearest station this is, computed from their refNearestStation and never stored. |
| `page` | Property | `uri` |  |  | `sdm:page` | The publisher's web page about it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsAgency

The transit agency HSL's GTFS feed names, HSL itself.

IRI: `sdm:GtfsAgency`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `agencyName` | Property | `string` |  |  | `sdm:agencyName` | The transit agency's full name. |
| `page` | Property | `uri` |  |  | `sdm:page` | The publisher's web page about it. |
| `timezone` | Property | `string` |  |  | `sdm:timezone` | The agency's time zone, an IANA name such as Europe/Helsinki. |
| `language` | Property | `string` |  |  | `sdm:language` | The agency's main language, a two-letter code. |
| `phone` | Property | `string` |  |  | `sdm:phone` | The agency's public customer service number. |
| `routes` | Relationship | [`GtfsRoute`](#gtfsroute) (list) |  |  | `jc:routes` | The lines the agency operates, computed from their operatedBy and never stored. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsDailyDepartures

One transport mode's scheduled departures for today from HSL's GTFS timetable. Its history holds the earlier days.

IRI: `jc:GtfsDailyDepartures`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `scheduledDepartures` | Property | `integer` |  |  | `jc:scheduledDepartures` | The trips HSL's timetable starts on the day of this value's observedAt, each counted once at its first stop. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsRoute

One HSL line as GTFS gives it, both directions and every variant: its number, its name, what runs it and who operates it. Its stops in order are PublicTransportRoute's.

IRI: `sdm:GtfsRoute`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `shortName` | Property | `string` |  |  | `sdm:shortName` | The line number riders see, such as 4 or 550. |
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `routeType` | Property | `string` |  |  | `sdm:routeType` | GTFS's basic route type: 0 a tram, 1 the metro, 2 a train, 3 a bus, 4 a ferry. HSL's extended codes (109, 700s, 900) are written as the basic one they extend. |
| `transportMode` | Property | [`TransportMode`](#transportmode) |  |  | `jc:transportMode` | What serves the stop or runs the line. |
| `operatedBy` | Relationship | [`GtfsAgency`](#gtfsagency) |  |  | `sdm:operatedBy` | The agency that runs the line. |
| `routeAlerts` | Relationship | [`Alert`](#alert) (list) |  |  | `jc:routeAlerts` | The HSL service alerts on the line, computed from their refRoute and never stored. |
| `page` | Property | `uri` |  |  | `sdm:page` | The publisher's web page about it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsShape

The line one HSL route variant draws on the map, about 200 of its points, and the distance travelled at each.

IRI: `sdm:GtfsShape`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `distanceTravelled` | Property | `float` (list) |  |  | `sdm:distanceTravelled` | The distance along the shape at each point of its line, in kilometres. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### ServiceRequest

One report of the City of Helsinki's feedback system (Open311 GeoReport v2, the Issue Reporting API) that the city published. Smart Data Models names the type service_requests; the space names it ServiceRequest like its other types, with the model's attribute names. The reporter's name, e-mail, phone and photos are never read.

IRI: `sdm:service_requests`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `service_request_id` | Property | `string` | yes |  | `sdm:service_request_id` | The number the city's feedback system gives the report (Open311 `service_request_id`). |
| `status` | Property | [`ServiceRequestStatus`](#servicerequeststatus) | yes |  | `sdm:status` | open while the city handles the report (PROCESSING), closed once it has answered (READY). |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | The report as the reporter wrote it, in the language they wrote it in (`@none`). |
| `service_code` | Property | `string` |  |  | `sdm:service_code` | The feedback system's code of the sector the report is filed under, for example 2809. |
| `service_name` | Property | `string` |  |  | `sdm:service_name` | The sector's name in English, read from the feedback system's services list, for example City environment. |
| `status_notes` | Property | `string` |  |  | `sdm:status_notes` | The city's answer or note on the report, as published. |
| `agency_responsible` | Property | `string` |  |  | `sdm:agency_responsible` | The city unit handling the report, when the feedback system names one. |
| `requested_datetime` | Property | `datetime` | yes |  | `sdm:requested_datetime` | When the report was filed, ISO 8601 in UTC. |
| `updated_datetime` | Property | `datetime` |  |  | `sdm:updated_datetime` | When the report last changed, ISO 8601 in UTC. |
| `expected_datetime` | Property | `datetime` |  |  | `sdm:expected_datetime` | When the city expects to have handled the report, if it says. |
| `address_string` | Property | `string` |  |  | `sdm:address_string` | The street or place the reporter named, as written, for example Malminkatu. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KeyPerformanceIndicator

One indicator of the city's area statistics (Aluesarjat) for one area: the newest period's value, its history one row per period. Read daily from stat.hel.fi.

IRI: `sdm:KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `description` | LanguageProperty | `string` |  |  | `schema:description` | A short description, per language. |
| `kpiValue` | Property | `float` | yes |  | `sdm:kpiValue` | The indicator's value for its period, with the unit as `unitCode` (UN/CEFACT: C62 a count, P1 a percentage, EUR euros, MTK square metres) and the period's last day as `observedAt`, so the history holds one value per period. |
| `calculationPeriod` | Property | [`KpiPeriod`](#kpiperiod) | yes |  | `sdm:calculationPeriod` | The period the value covers, its first and last day. |
| `calculationFrequency` | Property | [`CalculationFrequency`](#calculationfrequency) |  |  | `sdm:calculationFrequency` | How often the source publishes a new period. |
| `area` | Property | `string` |  |  | `sdm:area` | The name of the area the value describes, a sub-district, district or the whole city. |
| `districtCode` | Property | `string` |  |  | `jc:districtCode` | The city's own code of the area: the district number, or for a sub-district the full ten-digit code the city's statistics use for it. |
| `divisionLevel` | Property | [`DivisionLevel`](#divisionlevel) |  |  | `jc:divisionLevel` | Which of the city's divisions the area belongs to: a district, sub-district, quarter or voting district for an AdministrativeArea; for a statistics indicator also the city, a major or basic district, or the inner city or the suburbs. |
| `refArea` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refArea` | The sub-district (AdministrativeArea) the value describes. Set only for a sub-district the city's register holds; a district or the whole city is named by `area` and `districtCode`. |
| `process` | Property | `string` |  |  | `sdm:process` | The statistics table the value comes from, in the source's words. |
| `statisticsTable` | Property | `string` |  |  | `jc:statisticsTable` | The source's own table code, such as alu_vamuu_010h, or the series of a usage counter, such as web-visits; `source` links it. |
| `dimensionKey` | Property | `string` |  |  | `jc:dimensionKey` | Which cell of the table the value is: each kept dimension and its code, such as Sukupuoli=ALL;Tiedot=tyottomyysaste. |
| `calculatedBy` | Property | `string` |  |  | `sdm:calculatedBy` | The organization that computes the value. |
| `organization` | Property | `string` |  |  | `sdm:organization` | The organization the indicator describes. |
| `updatedAt` | Property | `datetime` |  |  | `sdm:updatedAt` | When the source last updated the table. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KpiPeriod

The value of `calculationPeriod`, never an entity of its own.

IRI: `jc:KpiPeriod`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `from` | Property | `date` | yes |  | `sdm:from` | The first day of the period. |
| `to` | Property | `date` | yes |  | `sdm:to` | The last day of the period. |

### TrafficFlowObserved

One day of one of the city's traffic counters in one direction: the cyclists an Eco-Counter station counted, or the motor vehicles and their mean speed at one of the city's induction loop (LAM) stations, from the city's traffic measurement API. The counter's name and place ride on the observation; the history of the days is the entity's temporal history.

IRI: `sdm:TrafficFlowObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language. |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the counted day, the same instant as dateObservedTo. |
| `dateObservedFrom` | Property | `datetime` |  |  | `sdm:dateObservedFrom` | The start of the period the observation counts, the local midnight that opens the day. |
| `dateObservedTo` | Property | `datetime` |  |  | `sdm:dateObservedTo` | The end of the period the observation counts, the local midnight that closes the day. |
| `intensity` | Property | `integer` |  | /d | `sdm:intensity` | How many vehicles, or bicycles, the counter detected in the day, in one direction. |
| `averageVehicleSpeed` | Property | `float` |  | km/h | `sdm:averageVehicleSpeed` | The mean speed of the vehicles the counter measured in the period, in km/h. |
| `vehicleType` | Property | [`CountedVehicleType`](#countedvehicletype) |  |  | `sdm:vehicleType` | The one kind of vehicle the observation counts, in the Smart Data Model's words; absent when the counter sums every class of motor vehicle. |
| `trafficDirection` | Property | [`TrafficDirection`](#trafficdirection) |  |  | `jc:trafficDirection` | The compass direction, or into and out of the centre, the counted traffic travels, as the city's counter names it (Pohjoiseen is north). Smart Data Models' laneDirection needs a road segment to mean anything, which the counters do not name. Absent when the counter sums both directions. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, as an application credits it beside the licence (CC BY 4.0 for every feed of this space). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PostalAddress

The value of `address`: a postal address, never an entity of its own (Smart Data Models' common schema).

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and the house number. |
| `postalCode` | Property | `string` |  |  | `schema:postalCode` | The postal code. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The town or the municipality. |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country, as an ISO 3166-1 alpha-2 code. |

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

### CalculationFrequency

| Value | Meaning |
|---|---|
| `daily` | A new value every day. |
| `monthly` | A new value every month. |
| `quarterly` | A new value every quarter. |
| `yearly` | A new value every year. |

### ServiceCategory

| Value | Meaning |
|---|---|
| `library` | A public library. |
| `healthStation` | A health station. |
| `swimmingHall` | An indoor swimming hall. |
| `beach` | A public beach. |
| `school` | A comprehensive or general upper secondary school. |
| `other` | Any other unit of the service register: a private one, or a public one with no point inside the city or of no kind above. |
| `kindergarten` | A day-care centre, a family day-care area, a pre-primary group or a playpark (leikkipuisto). |
| `hospital` | A hospital or one of its clinics. |
| `dentalCare` | A public dental clinic. |
| `maternityClinic` | A maternity and child health clinic (neuvola). |
| `healthCare` | Any other public health care unit, such as a mental health, substance abuse or rehabilitation service. |
| `vocationalSchool` | A vocational school or an apprenticeship training office. |
| `university` | A university or a university of applied sciences. |
| `adultEducation` | An adult education centre. |
| `elderlyCare` | A service centre, home care unit or care home for the elderly. |
| `socialServices` | A social services unit: child welfare, families, disability services or social assistance. |
| `youth` | A youth centre or a youth workshop. |
| `rescueStation` | A fire and rescue station. |
| `police` | A police station. |
| `publicShelter` | A public civil defence shelter. |
| `publicAuthority` | An office of the city or the state: administration, justice, taxation, planning or customs. |
| `sports` | A public sports facility or outdoor exercise place. |
| `culture` | A public cultural or leisure place, such as a cultural centre, a public artwork, a playground or an allotment garden. |
| `waterfrontSite` | A site on the city's shore route offered for a new service, such as a sauna, a cafe or a pier (rantareitti 2025). |
| `businessPlot` | A plot the city offers to a business, with its area and floor area right (vapaat yritystontit). |
| `environmentalPermitSite` | A site that holds an environmental permit or is registered under the Environmental Protection Act (ympäristölupakohde), such as a filling station or a plant. |

### ServiceRequestStatus

Open311's two report states, as Smart Data Models' service_requests names them.

| Value | Meaning |
|---|---|
| `open` | The city is handling the report (the feedback system's PROCESSING). |
| `closed` | The city has answered the report (READY). |

### StreetParkingCategory

The OnStreetParking `category` values the city's layers use, a subset of the Smart Data Model's enum.

| Value | Meaning |
|---|---|
| `public` | Open to anyone with no fee or time rule the layer states, such as a parking area of the parking guide. |
| `feeCharged` | A parking fee zone; parking costs by the hour. |
| `forResidents` | A resident parking zone; residents and businesses of the zone park on its permit. |
| `free` | Parking costs nothing. |
| `shortTerm` | Parking is limited in time, by a single fee or a parking disc. |
| `onlyWithPermit` | Only a permit holder parks, such as a car-sharing pick-up place. |
| `forDisabled` | A place for a person with a disabled parking permit. |
| `forElectricalCharging` | A place for an electric car while it charges. |
| `forLoadUnload` | A loading place. |
| `taxiStop` | A taxi stand. |

### PermitKind

| Value | Meaning |
|---|---|
| `excavation` | Digging on a street or a park. |
| `trafficArrangement` | A temporary change to traffic, such as a closed lane. |
| `event` | An event held on a public area. |

### PermitStatus

| Value | Meaning |
|---|---|
| `upcoming` | Granted and not started. |
| `ongoing` | The work or the event is going on. |
| `setup` | The event's structures are being built. |
| `teardown` | The event's structures are being taken down. |

### DivisionLevel

| Value | Meaning |
|---|---|
| `city` | The whole city (statistics only). |
| `majorDistrict` | A major district (suurpiiri), 8 of them (statistics only). |
| `basicDistrict` | A basic district (peruspiiri), 34 of them (statistics only). |
| `cityPart` | The inner city (kantakaupunki) or the suburbs (esikaupungit) (statistics only). |
| `district` | A district (kaupunginosa), 60 of them. |
| `subDistrict` | A sub-district (osa-alue), the unit the city's statistics use. |
| `fareZone` | An HSL fare zone (A, B, C or D), the area a ticket is valid in. |
| `quarter` | A quarter (kortteli), the block a detailed plan names, about 6,000 of them. |
| `votingDistrict` | A voting district (äänestysalue), about 165 of them. |
| `preschoolAdmissionArea` | A preschool admission area (esiopetusalue) of one school year, Finnish- or Swedish-language, about 70 a year. |

### TransportMode

| Value | Meaning |
|---|---|
| `bus` | A bus. |
| `tram` | A tram. |
| `metro` | The metro. |
| `train` | A commuter train. |
| `ferry` | A ferry. |

### CountedVehicleType

The kinds of vehicle a city counter tells apart, a subset of Smart Data Models' vehicleType.

| Value | Meaning |
|---|---|
| `bicycle` | A bicycle, as an Eco-Counter station or the machine vision pilot counts it. |
| `car` | A car, as the machine vision pilot counts it. |
| `bus` | A bus, as the machine vision pilot counts it. |
| `tram` | A tram, as the machine vision pilot counts it. |
| `motorcycle` | A motorcycle, as the machine vision pilot counts it. |
| `lorry` | A truck or lorry, as the machine vision pilot counts it. |

### TrafficDirection

The direction a city counter names, in English.

| Value | Meaning |
|---|---|
| `north` | Pohjoiseen. |
| `south` | Etelään. |
| `east` | Itään. |
| `west` | Länteen. |
| `inbound` | IN, towards the centre. |
| `outbound` | OUT, away from the centre. |

### EventStatus

Where an event stands, schema.org's EventStatusType values as Linked Events publishes them.

| Value | Meaning |
|---|---|
| `EventScheduled` | Takes place as planned. |
| `EventCancelled` | Called off; it does not take place. |
| `EventPostponed` | Moved to a date not yet known. |
| `EventRescheduled` | Moved to the new date the event now gives. |
| `EventMovedOnline` | Held online instead of at its place. |

### HeritageCategory

The kinds of the fortification and landscape culture maps a HeritageArea is one of.

| Value | Meaning |
|---|---|
| `fortification` | A strongpoint (tukikohta) of the First World War land fortifications. |
| `park` | A park or a small park (puisto, puistikko). |
| `boulevard` | A tree-lined park street (puistokatu). |
| `square` | A square (aukio). |
| `cemetery` | A cemetery (hautausmaa). |
| `peoplesPark` | A people's park (kansanpuisto). |
| `sportsPark` | A sports or exercise park. |
| `allotmentGarden` | An allotment garden (siirtolapuutarha). |
| `holidayArea` | A holiday or recreation area (loma-alue). |
| `otherGreenArea` | Any other green area of the map. |
| `villaPark` | A villa park or villa area (huvilapuisto). |
| `manorPark` | A manor park (kartanopuisto). |
| `manorEnvironment` | The cultural environment of a manor or an estate centre. |
| `residentialEnsemble` | A residential area kept as an ensemble. |
| `publicBuildingGrounds` | The grounds of a public building. |
| `courtyard` | A courtyard (piha). |

### TrafficSignalKind

What signals a TrafficSignalJunction has.

| Value | Meaning |
|---|---|
| `trafficLights` | Traffic lights that control the junction (liikennevalot). |
| `warningLights` | Warning lights that flash before a crossing, a tram line or a fire station exit (varoitusvalot). |

### TrafficSignalStatus

Whether a TrafficSignalJunction's signals work yet.

| Value | Meaning |
|---|---|
| `inService` | The signals are in service. |
| `underConstruction` | The city is still building the signals (rakenteilla). |
### NoiseAssessmentPeriod

The parts of the day a noise band of the 2022 survey holds for.

| Value | Meaning |
|---|---|
| `day` | The day, 07:00 to 22:00 (LAeq 7-22). |
| `night` | The night, 22:00 to 07:00 (LAeq 22-7). |

### NoiseSource

The traffic sources the 2022 noise survey computes one map each for.

| Value | Meaning |
|---|---|
| `roadTraffic` | Streets and roads (kadut ja maantiet). |
| `railway` | The main railway (juna). |
| `metro` | The metro. |
| `tram` | The tram (raitiovaunu). |

### TrunkDiameterClass

The trunk diameter classes of the tree register, at breast height.

| Value | Meaning |
|---|---|
| `cm0to10` | Up to 10 cm. |
| `cm10to20` | 10 to 20 cm. |
| `cm20to30` | 20 to 30 cm. |
| `cm30to50` | 30 to 50 cm. |
| `cm50to70` | 50 to 70 cm. |
| `cm70plus` | 70 cm or more. |

### TreeSetting

Where a tree of the register stands.

| Value | Meaning |
|---|---|
| `street` | Along a street (katu). |
| `park` | In a park (puisto). |

### NatureSiteCategory

The layers of the nature information system a NatureSite comes from.

| Value | Meaning |
|---|---|
| `natureReserve` | A nature reserve protected under the Nature Conservation Act (luonnonsuojelualue). |
| `natura2000` | An area of the EU Natura 2000 network (Natura-alue). |
| `naturalMonument` | A protected natural monument, mostly an old tree or a boulder (luonnonmuistomerkki). |
| `protectedHabitat` | A habitat type protected under the Nature Conservation Act (suojeltu luontotyyppi). |
| `conservationProgramme` | A site of the city's nature conservation programme, to be protected (luonnonsuojeluohjelman kohde). |
| `flyingSquirrelCore` | A core area of the Siberian flying squirrel (liito-oravan ydinalue). |
| `birdArea` | An important bird area (tärkeä lintualue). |
| `batArea` | An area bats use, by value class (lepakkoalue). |
| `valuablePlantSite` | A valuable vegetation or flora site (arvokas kasvillisuus- ja kasvistokohde). |
| `geologicalSite` | A geologically or geomorphologically valuable site (geologisesti arvokas kohde). |

### AlertEffect

What a public transport alert does to the service, GTFS-RT's Effect values as HSL publishes them.

| Value | Meaning |
|---|---|
| `NO_SERVICE` | The service does not run. |
| `REDUCED_SERVICE` | The service runs with fewer departures. |
| `SIGNIFICANT_DELAYS` | The service runs with long delays. |
| `DETOUR` | The service runs on another route. |
| `ADDITIONAL_SERVICE` | Extra departures run. |
| `MODIFIED_SERVICE` | The timetable or the stops differ from the usual. |
| `OTHER_EFFECT` | An effect GTFS-RT has no value for. |
| `UNKNOWN_EFFECT` | The publisher does not know the effect. |
| `STOP_MOVED` | A stop has moved. |
| `NO_EFFECT` | The alert informs and the service does not change. |
| `ACCESSIBILITY_ISSUE` | A step-free route or a lift is out of use. |

### AlertCause

Why a public transport alert is in force, GTFS-RT's Cause values as HSL publishes them.

| Value | Meaning |
|---|---|
| `UNKNOWN_CAUSE` | The publisher does not know the cause. |
| `OTHER_CAUSE` | A cause GTFS-RT has no value for. |
| `TECHNICAL_PROBLEM` | A vehicle or the infrastructure failed. |
| `STRIKE` | Staff are on strike. |
| `DEMONSTRATION` | A demonstration blocks the way. |
| `ACCIDENT` | An accident happened on the way. |
| `HOLIDAY` | A public holiday changes the service. |
| `WEATHER` | The weather affects the service. |
| `MAINTENANCE` | Planned maintenance of the track or the vehicles. |
| `CONSTRUCTION` | Construction work on the way. |
| `POLICE_ACTIVITY` | A police operation on the way. |
| `MEDICAL_EMERGENCY` | A medical emergency on the way. |

### AlertSeverity

How much a public transport alert matters, the Smart Data Models values HSL's three GTFS-RT levels map to.

| Value | Meaning |
|---|---|
| `informational` | For information (GTFS-RT INFO). |
| `medium` | A warning (GTFS-RT WARNING). |
| `high` | A severe disruption (GTFS-RT SEVERE). |
