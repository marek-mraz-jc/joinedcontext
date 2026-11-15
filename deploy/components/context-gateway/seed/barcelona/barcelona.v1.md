# Barcelona, open data

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the space barcelona carries (DM-01, DM-61): the open data of Barcelona City Council (Open Data BCN) as Smart Data Models types, their IRIs cited rather than minted (DM-04, DM-16). The first type is AirQualityObserved, one entity per station of the city's air-quality network with the hourly mean of each pollutant it measures; Event holds the city's activity agendas (cultural, daily and sports); PointOfInterest the city's register of facilities (equipaments); each Open Data BCN batch adds its own classes. Transport: TrafficFlowObserved, the traffic state of each street section and each route of sections the city reports every five minutes, OnStreetParking, each regulated surface parking section (blue, green, loading and bus zones), and EVChargingStation, each public charging point of the Endolla Barcelona network with its live socket states, BikeLane, each segment of the bike network open or under construction, RoadAccident, each accident the Guàrdia Urbana handled, as counts only, and each traffic counter's monthly intensity as TrafficFlowObserved. Urbanism: the districts and barris as AdministrativeArea, the works on the public space as CityWork and the street inventory's crossings as PedestrianCrossing, its speed reducers as StreetFurniture and the drinking and ornamental fountains as PointOfInterest. Tourism: each licensed tourist flat (habitatge d'ús turístic) as TouristRental, at the building's address and point. Statistics: the city's statistics per barri, district and city as KeyPerformanceIndicator.

- Namespace: `https://joinedcontext.com/models/barcelona/barcelona`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AirQualityObserved

One air-quality station of Barcelona and its hourly means.

IRI: `sdm:AirQualityObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's newest readings cover, UTC: the hour of an air-quality mean, the local day of a weather station's values, the day of a well's reading, a traffic state's report, a charging point's newest port change. |
| `pm10` | Property | `float` |  | ug/m3 | `sdm:pm10` | Particulate matter up to 10 µm, the hour's mean at the station. |
| `pm25` | Property | `float` |  | ug/m3 | `sdm:pm25` | Particulate matter up to 2.5 µm, the hour's mean at the station. |
| `no2` | Property | `float` |  | ug/m3 | `sdm:no2` | Nitrogen dioxide, the hour's mean at the station. |
| `o3` | Property | `float` |  | ug/m3 | `sdm:o3` | Ozone, the hour's mean at the station. |
| `so2` | Property | `float` |  | ug/m3 | `sdm:so2` | Sulphur dioxide, the hour's mean at the station. |
| `co` | Property | `float` |  | mg/m3 | `sdm:co` | Carbon monoxide, the hour's mean at the station, in mg/m³ as the city measures it. |
| `no` | Property | `float` |  | ug/m3 | `sdm:no` | Nitrogen monoxide, the hour's mean at the station. |
| `nox` | Property | `float` |  | ug/m3 | `sdm:nox` | Nitrogen oxides (NO + NO2 as NO2), the hour's mean at the station. |
| `blackCarbon` | Property | `float` |  | ug/m3 | `jc:blackCarbon` | Black carbon, the hour's mean at the station. A property of this model: the Smart Data Model AirQualityObserved names no black carbon attribute. The city's file gives it in ng/m³ under a µg/m³ label; the pipeline divides by 1000. |
| `stationCode` | Property | `string` |  |  | `jc:stationCode` | The station's code in the network that runs it: the Catalan air-quality network (XVPCA, the city's codi_eoi, such as `8019043`), the Meteocat weather network (such as `D5`) or the Catalan water agency's well register (ACA, such as `08019-0148`). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### WeatherObserved

One Meteocat weather station in Barcelona and its newest day's values.

IRI: `sdm:WeatherObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's newest readings cover, UTC: the hour of an air-quality mean, the local day of a weather station's values, the day of a well's reading, a traffic state's report, a charging point's newest port change. |
| `temperature` | Property | `float` |  | Cel | `sdm:temperature` | The day's mean air temperature at the station. |
| `maxTemperature` | Property | `float` |  | Cel | `jc:maxTemperature` | The day's highest air temperature at the station. A property of this model: the Smart Data Model WeatherObserved names only the mean. |
| `minTemperature` | Property | `float` |  | Cel | `jc:minTemperature` | The day's lowest air temperature at the station. A property of this model: the Smart Data Model WeatherObserved names only the mean. |
| `relativeHumidity` | Property | `float` |  | 1 | `sdm:relativeHumidity` | The day's mean relative humidity, as a fraction (0.57 is 57 %). |
| `atmosphericPressure` | Property | `float` |  | hPa | `sdm:atmosphericPressure` | The day's mean air pressure at the station's height (not reduced to sea level). |
| `precipitation` | Property | `float` |  | mm | `sdm:precipitation` | The day's precipitation, in millimetres (litres per square metre). |
| `windSpeed` | Property | `float` |  | m/s | `sdm:windSpeed` | The day's mean wind speed at 10 m. |
| `windDirection` | Property | `float` |  | deg | `sdm:windDirection` | The day's mean wind direction at 10 m, in degrees from north. |
| `gustSpeed` | Property | `float` |  | m/s | `sdm:gustSpeed` | The day's strongest gust at 10 m. |
| `solarRadiation` | Property | `float` |  | W/m2 | `sdm:solarRadiation` | The day's mean global solar irradiance: the day's irradiation the city gives in MJ/m², times 10^6 / 86400. |
| `stationCode` | Property | `string` |  |  | `jc:stationCode` | The station's code in the network that runs it: the Catalan air-quality network (XVPCA, the city's codi_eoi, such as `8019043`), the Meteocat weather network (such as `D5`) or the Catalan water agency's well register (ACA, such as `08019-0148`). |
| `altitude` | Property | `float` |  |  | `jc:altitude` | The station's height above sea level in metres, as the weather service lists it. A fixed property of the station, written without a unit code so its rows are never read as a measurement stamped on receipt. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### WeatherForecast

Meteocat's forecast for the comarca of Barcelonès, one entity per day ahead (day-1 tomorrow, day-2 the day after), as Open Data BCN republishes it.

IRI: `sdm:WeatherForecast`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `dateIssued` | Property | `datetime` | yes |  | `sdm:dateIssued` | When the meteorological service issued the forecast, UTC (Meteocat's datacreacio, local time in the file). |
| `validFrom` | Property | `datetime` |  |  | `sdm:validFrom` | The start of the day a forecast covers, local midnight in Europe/Madrid, as UTC. |
| `validTo` | Property | `datetime` |  |  | `sdm:validTo` | The end of the day a forecast covers, the next local midnight in Europe/Madrid, as UTC. |
| `dayMaximum` | Property | [`DayExtremes`](#dayextremes) |  |  | `sdm:dayMaximum` | The highest values forecast for the day (Smart Data Models' structured value); Meteocat gives the air temperature. |
| `dayMinimum` | Property | [`DayExtremes`](#dayextremes) |  |  | `sdm:dayMinimum` | The lowest values forecast for the day (Smart Data Models' structured value); Meteocat gives the air temperature. |
| `weatherType` | Property | `string` |  |  | `sdm:weatherType` | The sky Meteocat forecasts in Catalan, morning then afternoon when they differ ("sol i núvols alts (matí), sol (tarda)"), read from the symbol table of the same file. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PhreaticObserved

One piezometer or well of Barcelona and its newest groundwater reading.

IRI: `sdm:PhreaticObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's newest readings cover, UTC: the hour of an air-quality mean, the local day of a weather station's values, the day of a well's reading, a traffic state's report, a charging point's newest port change. |
| `measurementType` | Property | [`MeasurementType`](#measurementtype) (list) |  |  | `sdm:measurementType` | What the well's readings measure; `depth` for each of the city's wells. |
| `waterTable` | Property | `float` |  | m | `sdm:waterTable` | The depth from the surface to the water in the well at the newest reading. |
| `piezometricLevel` | Property | `float` |  | m | `jc:piezometricLevel` | The height of the water above sea level at the newest reading (below 0 where the sea draws the aquifer down). A property of this model: PhreaticObserved has only the depth. |
| `investigationDepth` | Property | `float` |  |  | `sdm:investigationDepth` | The depth of the well or piezometer in metres. A fixed property of the well, written without a unit code so its rows are never read as a measurement stamped on receipt. |
| `aquifer` | Property | `string` |  |  | `jc:aquifer` | The groundwater body (massa d'aigua) the well reaches, as the water agency names it. |
| `stationCode` | Property | `string` |  |  | `jc:stationCode` | The station's code in the network that runs it: the Catalan air-quality network (XVPCA, the city's codi_eoi, such as `8019043`), the Meteocat weather network (such as `D5`) or the Catalan water agency's well register (ACA, such as `08019-0148`). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### TrafficFlowObserved

One street section of the city's traffic-state network with its line, its state now and the state forecast for 15 minutes, every five minutes (Open Data BCN trams and transit-relacio-trams); one route of sections with its travel time (itineraris); or one traffic or bicycle counter with its mean working-day intensity of the newest month (aforaments-descriptiu and aforaments-detall).

IRI: `sdm:TrafficFlowObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's newest readings cover, UTC: the hour of an air-quality mean, the local day of a weather station's values, the day of a well's reading, a traffic state's report, a charging point's newest port change. |
| `congested` | Property | `boolean` |  |  | `sdm:congested` | Whether the section is congested now: true for the city's states 4 (very dense) and 5 (congestion), false otherwise, as SDM reads an absent flag (no data and closed included; trafficState tells those apart). |
| `trafficState` | Property | [`TrafficState`](#trafficstate) |  |  | `jc:trafficState` | The city's own traffic state of the section (custom: TrafficFlowObserved has only `congested`), its code 1 to 6 named in English; Open Data BCN dataset trams. |
| `trafficStateForecast` | Property | [`TrafficState`](#trafficstate) |  |  | `jc:trafficStateForecast` | The state the city forecasts for the section in 15 minutes, same codes as trafficState (custom). |
| `travelTime` | Property | `integer` |  | s | `jc:travelTime` | How long a vehicle takes now to drive the route, in seconds (custom: TrafficFlowObserved has no travel time); Open Data BCN dataset itineraris, its fourth field. Absent while the city reports no reading for the route. |
| `usualTravelTime` | Property | `integer` |  | s | `jc:usualTravelTime` | The time the city expects for the route now from its history of the same kind of day, in seconds (custom; itineraris, its fifth field, temps previst). travelTime above it means slower traffic than usual. |
| `intensity` | Property | `integer` |  |  | `sdm:intensity` | A traffic counter's mean number of vehicles (or bicycles, at a bicycle counter) a working day of the month dateObservedFrom..dateObservedTo, the city's IMD. |
| `dateObservedFrom` | Property | `datetime` |  |  | `sdm:dateObservedFrom` | The start of the period a traffic counter's intensity covers, UTC. |
| `dateObservedTo` | Property | `datetime` |  |  | `sdm:dateObservedTo` | The end of the period a traffic counter's intensity covers, UTC. |
| `vehicleType` | Property | [`CountedVehicleType`](#countedvehicletype) |  |  | `sdm:vehicleType` | bicycle at a counter of the bicycle network; absent at a counter of motor traffic, which counts every kind. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### OnStreetParking

One regulated surface parking section of B:SM (blue, green, residents, loading or coach zone), its curb lines, its places and its rate and timetable codes (Open Data BCN trams-aparcament-superficie).

IRI: `sdm:OnStreetParking`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `category` | Property | [`StreetParkingCategory`](#streetparkingcategory) (list) |  |  | `sdm:category` | An Event's main category first, one of the platform's fifteen event codes (music, theatre-dance, film, exhibition-museum, festival-fair, sport, outdoor-nature, education-talk, children-family, hobby-workshop, food-market, history-heritage, faith-tradition, health-wellness, community-civic), then the Smart Data Models category it falls under where one fits, read from the first words of the Catalan name and absent when no rule matches. A PointOfInterest's family in the city's facility register first (culture-leisure, sports, education, social-services, public-administration, markets-shopping, information-centres, religious-services, environment, animals-plants, cemeteries, media, utility-companies, restaurants), then the register's own kind in Catalan (Biblioteques, Poliesportius); a fountain is drinking-fountain or ornamental-fountain alone; a dog area of the city's parks carries animals-plants, dog-area, a tree of local interest environment, local-interest-tree. An OnStreetParking's kind of regulated parking, from the section type (AZL blue zone, VM green zone, VR residents, DUM loading, BUS coaches). |
| `allowedVehicleType` | Property | `string` (list) |  |  | `sdm:allowedVehicleType` | What may park there, the section's own kind first; at a charging point, what its sockets serve (car for Type 2, CCS and CHAdeMO and for a car park's household sockets, motorcycle for the 3.6 kW household sockets). |
| `totalSpotNumber` | Property | `integer` |  |  | `sdm:totalSpotNumber` | The parking places of the section as B:SM counts them (PLACES). |
| `zoneCode` | Property | `string` |  |  | `jc:zoneCode` | The section type as B:SM codes it (AZL, VM, VR, DUM, BUS), kept as published (custom). |
| `tariffCode` | Property | `string` |  |  | `jc:tariffCode` | The B:SM rate (ID_TARIFA) and timetable (ID_HORARIO) codes of the section, joined as rate/timetable, kept as published (custom; Open Data BCN tarifes- and horaris-aparcament-superficie). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### EVChargingStation

One public charging point of the Endolla Barcelona network run by B:SM, its ports, socket kinds and how many are free now, every five minutes (Open Data BCN informacio-punts-recarrega-electric, which carries the port states of estat-ports-recarrega-electric too).

IRI: `sdm:EVChargingStation`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `dateObserved` | Property | `datetime` | yes |  | `sdm:dateObserved` | The end of the period the entity's newest readings cover, UTC: the hour of an air-quality mean, the local day of a weather station's values, the day of a well's reading, a traffic state's report, a charging point's newest port change. |
| `capacity` | Property | `integer` |  |  | `sdm:capacity` | How many vehicles the charging point charges at once, one per port the city lists; how many guests a tourist flat's licence admits (its places). |
| `socketNumber` | Property | `integer` |  |  | `sdm:socketNumber` | The ports of the charging point. |
| `availableCapacity` | Property | `integer` |  |  | `sdm:availableCapacity` | The ports free now (port status AVAILABLE); never more than capacity. |
| `socketType` | Property | [`SocketType`](#sockettype) (list) |  |  | `sdm:socketType` | The kinds of socket, in Smart Data Models' words: CCS_TYPE_2 is CCS/SAE, CHADEMO CHAdeMO, MENNEKES and a wall outlet over 3.7 kW Type2, a wall outlet up to 3.7 kW Wall_Euro. |
| `allowedVehicleType` | Property | `string` (list) |  |  | `sdm:allowedVehicleType` | What may park there, the section's own kind first; at a charging point, what its sockets serve (car for Type 2, CCS and CHAdeMO and for a car park's household sockets, motorcycle for the 3.6 kW household sockets). |
| `status` | Property | [`ChargingStationStatus`](#chargingstationstatus) |  |  | `sdm:status` | The state of the charging point from its ports: working while one is free, full when none is free and one charges, outOfService when every port is out of order, unavailable or unknown. |
| `network` | Property | `string` |  |  | `sdm:network` | The brand of the charging network (Endolla Barcelona). |
| `operator` | Property | `string` |  |  | `sdm:operator` | Who runs the charging point (Barcelona de Serveis Municipals S.A.). |
| `openingHours` | Property | `string` |  |  | `schema:openingHours` | When the charging point may be used, in schema.org's syntax (Mo-Su 00:00-24:00). |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### AdministrativeArea

One district (10) or barri (73) of Barcelona, its outline and its code, as the Smart Data Model extension AdministrativeArea (PointOfInterest base).

IRI: `jc:AdministrativeArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `districtCode` | Property | `string` | yes |  | `jc:districtCode` | The city's own two-digit code of the area: the district number 01..10, or for a barri its number 01..73, which runs across the whole city. |
| `divisionLevel` | Property | [`DivisionLevel`](#divisionlevel) | yes |  | `jc:divisionLevel` | Which of the city's two divisions the area belongs to; for a statistics indicator also the whole city. |
| `refParentDistrict` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refParentDistrict` | The district a barri belongs to. |
| `neighbourhoods` | Relationship | [`AdministrativeArea`](#administrativearea) (list) |  |  | `jc:neighbourhoods` | The barris of this district, computed from their refParentDistrict and never stored. |
| `cityWorks` | Relationship | [`CityWork`](#citywork) (list) |  |  | `jc:cityWorks` | The works that lie in this district only, computed from their refDistrict and never stored. |
| `pedestrianCrossings` | Relationship | [`PedestrianCrossing`](#pedestriancrossing) (list) |  |  | `jc:pedestrianCrossings` | The crossings in this barri, computed from their refNeighbourhood and never stored. |
| `indicators` | Relationship | [`KeyPerformanceIndicator`](#keyperformanceindicator) (list) |  |  | `jc:indicators` | The statistics indicators of this district or barri, computed from their refArea and never stored. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### CityWork

One work on Barcelona's public space, in execution, stopped or finished within 30 days, with its outline, its dates and its promoter. The constructor is left out.

IRI: `sdm:CityWork`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `description` | Property | `string` |  |  | `schema:description` | The description as the city publishes it, in Catalan (a work, a statistics dataset). |
| `workNumber` | Property | `string` | yes |  | `sdm:workNumber` | The city's code of the work, such as 05-2025G0074 (district, year, number). |
| `workState` | Property | [`WorkState`](#workstate) | yes |  | `sdm:workState` | Where the work stands, in the Smart Data Model's terms. |
| `workStateLabel` | Property | `string` |  |  | `jc:workStateLabel` | The work's state as the city writes it, in Catalan (En execució, Aturada, Finalitzada). |
| `workCategory` | Property | `string` |  |  | `jc:workCategory` | The city's kind of work, in Catalan (Urbanització, Serveis, Clavegueram...). |
| `startDate` | Property | `datetime` | yes |  | `schema:startDate` | The first day, local midnight in Europe/Madrid: of an event (the agenda publishes the day, its hours sit in a free-text timetable) or of a work on the public space. |
| `endDate` | Property | `datetime` | yes |  | `schema:endDate` | The last day, local midnight in Europe/Madrid: of an event (absent when the agenda gives none) or the planned last day of a work on the public space. |
| `contractingAuthority` | Property | `string` |  |  | `sdm:contractingAuthority` | The work's promoter, a public body, utility or company; left out when it may name a person. |
| `tenderAmount` | Property | `float` |  |  | `jc:tenderAmount` | The budget the work was put out to tender with, in euros. |
| `awardAmount` | Property | `float` |  |  | `jc:awardAmount` | The budget the work was awarded for, in euros. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `territorialArea` | Property | `string` |  |  | `sdm:territorialArea` | Every district the work touches, by name, separated by "; ". |
| `refDistrict` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refDistrict` | The district the work lies in, when it lies in one district only. |
| `seeAlso` | Property | `uri` |  |  | `sdm:seeAlso` | The work's page on the city's works site. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PedestrianCrossing

One pedestrian or cycle crossing painted on Barcelona's streets, from the city's street inventory, with its kind, its painted area and its barri. Smart Data Models has no crossing model; the class is the project's own, with SDM's common names where they exist (location, dataProvider, source).

IRI: `jc:PedestrianCrossing`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `crossingType` | Property | [`CrossingType`](#crossingtype) | yes |  | `jc:crossingType` | How the crossing is marked, from the city's typology (infraestructures-tipologia-pas-vianants). |
| `crossingTypeLabel` | Property | `string` |  |  | `jc:crossingTypeLabel` | The crossing's kind as the city writes it, in Catalan (Pas de pastilles, Pas de tacs, Pas de bicicletes). |
| `markedArea` | Property | `float` |  | m2 | `jc:markedArea` | The painted area of the crossing in square metres (unitCode MTK). |
| `refNeighbourhood` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refNeighbourhood` | The barri the crossing lies in; none for a crossing on the city's edge. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### StreetFurniture

One piece of furniture standing on Barcelona's streets, from the city's street inventory: today its speed reducers, each with its kind, its model and its address. Smart Data Models has no street furniture model; the class is the project's own, with SDM's common names where they exist (location, address, dataProvider, source).

IRI: `jc:StreetFurniture`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `furnitureType` | Property | [`StreetFurnitureType`](#streetfurnituretype) | yes |  | `jc:furnitureType` | What the piece of street furniture is, from the model code of the city's street inventory. |
| `furnitureTypeLabel` | Property | `string` |  |  | `jc:furnitureTypeLabel` | The piece's model as the city writes it, in Catalan (Coixí berlinés, Banda reductora). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### BikeLane

One segment of Barcelona's bike network as the city's map layer draws it, open (carril-bici) or under construction (carrils-bici-en-construccio), with its line and length.

IRI: `sdm:BikeLane`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `localId` | Property | `string` |  |  | `sdm:localId` | The source's own identifier of the record: a bike lane's segment code in the city's map layer (GL4683814), an accident's police file number (2025S000001). |
| `bikeLaneState` | Property | [`BikeLaneState`](#bikelanestate) | yes |  | `jc:bikeLaneState` | Whether the lane is open to cycling or still under construction, from the dataset it is read from. |
| `laneLength` | Property | `float` |  | m | `sdm:laneLength` | The length of the bike lane's line in metres, summed over its points on a local plane (83,550 m a degree of longitude and 111,050 m a degree of latitude at 41.4° N). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### RoadAccident

One road accident the Guàrdia Urbana handled (accidents-gu-bcn), its hour, place and the people injured or dead as counts, with its kinds, causes and the kinds of vehicle involved from the four detail tables joined by file number; nothing about a person.

IRI: `sdm:RoadAccident`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `localId` | Property | `string` |  |  | `sdm:localId` | The source's own identifier of the record: a bike lane's segment code in the city's map layer (GL4683814), an accident's police file number (2025S000001). |
| `status` | Property | [`RoadAccidentStatus`](#roadaccidentstatus) | yes |  | `sdm:status` | The state of the charging point from its ports: working while one is free, full when none is free and one charges, outOfService when every port is out of order, unavailable or unknown. |
| `accidentDate` | Property | `datetime` | yes |  | `sdm:accidentDate` | The hour of the accident, UTC: the police file gives the local day and hour only, so the minutes are 00. |
| `totalInjured` | Property | `integer` |  |  | `sdm:totalInjured` | The people injured and alive, slightly and seriously, as the police file counts them. |
| `seriouslyInjured` | Property | `integer` |  |  | `jc:seriouslyInjured` | The people seriously injured (lesionats greus). |
| `slightlyInjured` | Property | `integer` |  |  | `jc:slightlyInjured` | The people slightly injured (lesionats lleus). |
| `deadCount` | Property | `integer` |  |  | `jc:deadCount` | The people dead, as the police file counts them (morts). |
| `vehicleCount` | Property | `integer` |  |  | `jc:vehicleCount` | The vehicles the accident involved. |
| `pedestriansInvolved` | Property | `boolean` |  |  | `sdm:pedestriansInvolved` | True when the police file names a pedestrian's cause; absent otherwise, since the file does not say whether an accident without one involved a pedestrian. |
| `pedestrianCause` | Property | `string` |  |  | `jc:pedestrianCause` | The pedestrian's cause the police file names, in Catalan (Creuar per fora pas de vianants). |
| `accidentKind` | Property | `string` (list) |  |  | `jc:accidentKind` | The kinds of the accident the police file names, in Catalan as the city writes them (Col.lisió lateral, Abast, Atropellament), from accidents-tipus-gu-bcn. |
| `accidentType` | Property | `string` (list) |  |  | `sdm:accidentType` | The Smart Data Model codes of accidentKind where a code means the same: 1 frontal, 2 frontal-lateral, 3 side collision, 4 collision (Abast, Abast multiple, Encalç), 5 pedestrian hit, 8 impact with an obstacle (Xoc contra element estàtic); a kind with no such code has none. |
| `accidentCause` | Property | `string` (list) |  |  | `jc:accidentCause` | The causes the police file names, in Catalan: the mediate causes (accidents-causes-gu-bcn, Alcoholèmia), then the drivers' causes (accidents_causa_conductor_gu_bcn, Gir indegut o sense precaució). |
| `vehicleKind` | Property | `string` (list) |  |  | `jc:vehicleKind` | The kind of each vehicle involved, once a vehicle, in Catalan as the city writes it (Turisme, Motocicleta), from accidents-vehicles-gu-bcn; no make, model, colour or licence. |
| `vehiclesInvolved` | Property | `string` (list) |  |  | `sdm:vehiclesInvolved` | The Smart Data Model code of each vehicleKind where a code means the same (1 bicycle, 3 bus, 4 minibus, 5 car, 7 tram, 11 lorry, 12 moped, 14 motorcycle, 18 van, 20 construction or maintenance vehicle); a personal mobility vehicle, an ambulance or an unreported kind has none. |
| `numPassengersInjured` | Property | `integer` |  |  | `sdm:numPassengersInjured` | The passengers injured and alive, a count from accidents-persones-gu-bcn. |
| `numPassengersDead` | Property | `integer` |  |  | `sdm:numPassengersDead` | The passengers dead, a count from accidents-persones-gu-bcn. |
| `numPedestrianInjured` | Property | `integer` |  |  | `sdm:numPedestrianInjured` | The pedestrians injured and alive, a count from accidents-persones-gu-bcn. |
| `numPedestrianDead` | Property | `integer` |  |  | `sdm:numPedestrianDead` | The pedestrians dead, a count from accidents-persones-gu-bcn. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Event

One activity of the Ajuntament's agendas (agenda diària, agenda cultural, agenda d'activitats esportives), the day it starts and ends, where it happens and its category.

IRI: `sdm:Event`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `startDate` | Property | `datetime` | yes |  | `schema:startDate` | The first day, local midnight in Europe/Madrid: of an event (the agenda publishes the day, its hours sit in a free-text timetable) or of a work on the public space. |
| `endDate` | Property | `datetime` |  |  | `schema:endDate` | The last day, local midnight in Europe/Madrid: of an event (absent when the agenda gives none) or the planned last day of a work on the public space. |
| `category` | Property | `string` (list) |  |  | `sdm:category` | An Event's main category first, one of the platform's fifteen event codes (music, theatre-dance, film, exhibition-museum, festival-fair, sport, outdoor-nature, education-talk, children-family, hobby-workshop, food-market, history-heritage, faith-tradition, health-wellness, community-civic), then the Smart Data Models category it falls under where one fits, read from the first words of the Catalan name and absent when no rule matches. A PointOfInterest's family in the city's facility register first (culture-leisure, sports, education, social-services, public-administration, markets-shopping, information-centres, religious-services, environment, animals-plants, cemeteries, media, utility-companies, restaurants), then the register's own kind in Catalan (Biblioteques, Poliesportius); a fountain is drinking-fountain or ornamental-fountain alone; a dog area of the city's parks carries animals-plants, dog-area, a tree of local interest environment, local-interest-tree. An OnStreetParking's kind of regulated parking, from the section type (AZL blue zone, VM green zone, VR residents, DUM loading, BUS coaches). |
| `subCategory` | Property | `string` (list) |  |  | `sdm:subCategory` | The Smart Data Models sub-category where the name says it (music, theater). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `contactPoint` | Property | [`ContactPoint`](#contactpoint) |  |  | `schema:contactPoint` | The public telephone line the agenda or the facility register lists. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### TouristRental

One tourist flat (habitatge d'ús turístic) the city has licensed, by its licence file: the guests it admits, its Generalitat register number, and the building it is in (street and number, district, barri and point). Floor, door, staircase and block are never written: they locate a household. Smart Data Models' TouristRental is a facility rented to tourists; the licence register names the facility, so no rental dates or tourist.

IRI: `sdm:TouristRental`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `capacity` | Property | `integer` |  |  | `sdm:capacity` | How many vehicles the charging point charges at once, one per port the city lists; how many guests a tourist flat's licence admits (its places). |
| `registerNumber` | Property | `string` |  |  | `jc:registerNumber` | The flat's number in the Generalitat's tourism register (Registre de Turisme de Catalunya), HUTB- and six digits, which every advert of the flat must show. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | Where the entity is as the city names it: the barri of a station, a well or a tourist flat, the place of a work (a street and number, a square, a park). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One facility of the city's register (Guia BCN equipaments): a library, a sports hall, a school, a market, a social services centre; its family, where it is and its telephone.

IRI: `sdm:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `category` | Property | `string` (list) |  |  | `sdm:category` | An Event's main category first, one of the platform's fifteen event codes (music, theatre-dance, film, exhibition-museum, festival-fair, sport, outdoor-nature, education-talk, children-family, hobby-workshop, food-market, history-heritage, faith-tradition, health-wellness, community-civic), then the Smart Data Models category it falls under where one fits, read from the first words of the Catalan name and absent when no rule matches. A PointOfInterest's family in the city's facility register first (culture-leisure, sports, education, social-services, public-administration, markets-shopping, information-centres, religious-services, environment, animals-plants, cemeteries, media, utility-companies, restaurants), then the register's own kind in Catalan (Biblioteques, Poliesportius); a fountain is drinking-fountain or ornamental-fountain alone; a dog area of the city's parks carries animals-plants, dog-area, a tree of local interest environment, local-interest-tree. An OnStreetParking's kind of regulated parking, from the section type (AZL blue zone, VM green zone, VR residents, DUM loading, BUS coaches). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where the station, the well, the event, the facility, the charging point or the street furniture is, as the city publishes it, field by field (Smart Data Models' common PostalAddress); a field the source leaves out is absent. |
| `contactPoint` | Property | [`ContactPoint`](#contactpoint) |  |  | `schema:contactPoint` | The public telephone line the agenda or the facility register lists. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### KeyPerformanceIndicator

One indicator of the city's statistics (Departament d'Estadística, Open Data BCN) for one barri, district or the whole city: the newest period's value, its history one row per period. Read daily from Open Data BCN.

IRI: `sdm:KeyPerformanceIndicator`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the city publishes it, in Catalan (a station, an event, a facility, a street section and its stretch, a street address, a district or barri, a work, a tourist flat by its register number). |
| `description` | Property | `string` |  |  | `schema:description` | The description as the city publishes it, in Catalan (a work, a statistics dataset). |
| `kpiValue` | Property | `float` | yes |  | `sdm:kpiValue` | The indicator's value for its period, with the unit as `unitCode` (UN/CEFACT: C62 a count, EUR euros, MTK square metres) and the period's last day as `observedAt`, so the history holds one value per period. |
| `calculationPeriod` | Property | [`KpiPeriod`](#kpiperiod) | yes |  | `sdm:calculationPeriod` | The period the value covers, its first and last day. |
| `calculationFrequency` | Property | [`CalculationFrequency`](#calculationfrequency) |  |  | `sdm:calculationFrequency` | How often the source publishes a new period. |
| `area` | Property | `string` |  |  | `sdm:area` | The name of the area the value describes, a barri, a district or the whole city. |
| `districtCode` | Property | `string` |  |  | `jc:districtCode` | The city's own two-digit code of the area: the district number 01..10, or for a barri its number 01..73, which runs across the whole city. |
| `divisionLevel` | Property | [`DivisionLevel`](#divisionlevel) | yes |  | `jc:divisionLevel` | Which of the city's two divisions the area belongs to; for a statistics indicator also the whole city. |
| `refArea` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refArea` | The district or barri (AdministrativeArea) the value describes; none for the whole city. |
| `process` | Property | `string` |  |  | `sdm:process` | The statistics dataset the value comes from, its Open Data BCN title in Catalan. |
| `statisticsTable` | Property | `string` |  |  | `jc:statisticsTable` | The Open Data BCN dataset the value comes from, such as pad_mdbas, which `source` links to. |
| `dimensionKey` | Property | `string` |  |  | `jc:dimensionKey` | Which breakdown of the dataset the value is: each kept column and its value as the source writes it, such as SEXE=2; total when the dataset has no breakdown. |
| `calculatedBy` | Property | `string` |  |  | `sdm:calculatedBy` | The organization that computes the value. |
| `organization` | Property | `string` |  |  | `sdm:organization` | The organization the indicator describes. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data file or the Open Data BCN dataset the entity was read from. |
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

### PostalAddress

The value of `address`: a postal address, never an entity of its own (Smart Data Models' common schema).

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and the house number, or for a station its square, park or street corner. |
| `postalCode` | Property | `string` |  |  | `schema:postalCode` | The postal code. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The town or the municipality. |
| `district` | Property | `string` |  |  | `sdm:district` | The city district (districte) the address lies in. |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country, as an ISO 3166-1 alpha-2 code. |

### ContactPoint

The value of `contactPoint`: how to reach the organizer, never an entity of its own.

IRI: `schema:ContactPoint`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `telephone` | Property | `string` |  |  | `schema:telephone` | A telephone number as the agenda writes it. |

### DayExtremes

The value of `dayMaximum` or `dayMinimum`: the forecast extremes of one day, never an entity of its own.

IRI: `schema:StructuredValue`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `temperature` | Property | `float` |  | Cel | `sdm:temperature` | The air temperature forecast as the day's highest or lowest. |

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

### MeasurementType

What a PhreaticObserved measures (Smart Data Models' enum).

| Value | Meaning |
|---|---|
| `depth` |  |
| `volume` |  |
| `quality` |  |
| `other` |  |

### TrafficState

Open Data BCN's traffic state codes 0 to 6, named in English, the Catalan name in each description.

| Value | Meaning |
|---|---|
| `noData` | 0 sense dades: no reading this period. |
| `veryFluid` | 1 molt fluid. |
| `fluid` | 2 fluid. |
| `dense` | 3 dens. |
| `veryDense` | 4 molt dens. |
| `congestion` | 5 congestió. |
| `closed` | 6 tallat. |

### SocketType

The EVChargingStation `socketType` values the Endolla connector types map to, a subset of the Smart Data Model's enum.

| Value | Meaning |
|---|---|
| `CCS/SAE` | CCS_TYPE_2, the combined DC socket. |
| `CHAdeMO` | CHADEMO. |
| `Type2` | MENNEKES, and a WALL_OUTLET above 3.7 kW (the 22 kW AC outlets). |
| `Wall_Euro` | A WALL_OUTLET up to 3.7 kW, the household socket. |

### ChargingStationStatus

The EVChargingStation `status` values the port states map to, a subset of the Smart Data Model's enum.

| Value | Meaning |
|---|---|
| `working` | At least one port AVAILABLE. |
| `full` | No port free and at least one IN_USE. |
| `outOfService` | Every port OUT_OF_ORDER, UNAVAILABLE or UNKNOWN. |

### StreetParkingCategory

The OnStreetParking `category` values the B:SM section types map to, a subset of the Smart Data Model's enum.

| Value | Meaning |
|---|---|
| `blueZone` | AZL, the blue zone anyone parks in for a fee. |
| `greenZone` | VM and VR, the green zone. |
| `feeCharged` | Parking costs by the time parked (AZL, VM, BUS). |
| `forResidents` | VR, residents of the area only. |
| `forLoadUnload` | DUM, urban goods distribution. |
| `onlyWithPermit` | VR and DUM, a resident permit or the DUM app registration is needed. |

### CalculationFrequency

| Value | Meaning |
|---|---|
| `monthly` | A new value every month. |
| `yearly` | A new value every year. |

### DivisionLevel

| Value | Meaning |
|---|---|
| `city` | The whole city (statistics only). |
| `district` | A district (districte), 10 of them. |
| `neighbourhood` | A barri, 73 of them, each inside one district. |

### WorkState

The Smart Data Model CityWork workState values the city's states map to.

| Value | Meaning |
|---|---|
| `open` | In execution (En execució). |
| `other` | Stopped (Aturada). |
| `completed` | Finished (Finalitzada). |

### StreetFurnitureType

The kinds of street furniture the city's street inventory lists that the space carries.

| Value | Meaning |
|---|---|
| `speedCushion` | A Berlin cushion: a raised block a car straddles (Coixí berlinés). |
| `speedBump` | A speed bump or rumble band across the lane (Banda reductora). |

### CrossingType

The three kinds of crossing marking the city's typology names.

| Value | Meaning |
|---|---|
| `stripes` | Painted bars across the road, the zebra crossing (Pas de pastilles). |
| `blocks` | A crossing marked with painted square blocks (Pas de tacs). |
| `cycle` | A crossing for bicycles (Pas de bicicletes). |

### BikeLaneState

Which of the city's two bike lane layers the segment is read from.

| Value | Meaning |
|---|---|
| `inService` | Open to cycling (carril-bici). |
| `underConstruction` | Being built (carrils-bici-en-construccio). |

### RoadAccidentStatus

The Smart Data Model RoadAccident status; a police file the city publishes is closed.

| Value | Meaning |
|---|---|
| `archived` | A closed police file of a past accident. |

### CountedVehicleType

The Smart Data Model vehicleType a counter counts, where it counts one kind only.

| Value | Meaning |
|---|---|
| `bicycle` | A counter of the bicycle network (Bicis). |
