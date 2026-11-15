# Open data of Žilina

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the city's public space carries side by side (DM-01, DM-61): the hourly readings of the city's one air-quality station as the European Environment Agency republishes them, the city's immovable national cultural monuments from the Monuments Board's register placed through the national register of addresses, and the railway stations of the city from the national train timetable. One model, one space, one public endpoint. Where Smart Data Models has the class, its IRI is cited rather than minted (DM-04, DM-16). Sources: Research/zilina-open-data-sources.md.

- Namespace: `https://joinedcontext.com/models/zilina/zilina-verejne`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AirQualityObserved

One air-quality station in the city and the latest hourly means it reported.

IRI: `sdm:AirQualityObserved`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the publisher writes it. |
| `stationCode` | Property | `string` | yes |  | `jc:stationCode` | The station's EoI code in the European air-quality network, such as `SK0020A`. |
| `dateObserved` | Property | `datetime` |  |  | `sdm:dateObserved` | The end of the latest hour a reading of the entity covers. |
| `pm10` | Property | `float` |  | ug/m3 | `sdm:pm10` | Particulate matter up to 10 µm, the hour's mean at the station. |
| `pm25` | Property | `float` |  | ug/m3 | `sdm:pm25` | Particulate matter up to 2.5 µm, the hour's mean at the station. |
| `no2` | Property | `float` |  | ug/m3 | `sdm:no2` | Nitrogen dioxide, the hour's mean at the station. |
| `o3` | Property | `float` |  | ug/m3 | `sdm:o3` | Ozone, the hour's mean at the station. |
| `co` | Property | `float` |  | mg/m3 | `sdm:co` | Carbon monoxide, the hour's mean at the station. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One immovable national cultural monument in the city, placed at its address. The register's parcel numbers and authors are left out; ownership is a category only.

IRI: `sdm:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the publisher writes it. |
| `monumentNumber` | Property | `string` | yes |  | `jc:monumentNumber` | The monument's number in the central list of the Monuments Fund (Č. ÚZPF), with the index of the object when the monument has several (`529/1`). |
| `monumentKind` | Property | `string` |  |  | `jc:monumentKind` | What the object is, in the register's unified name (KOSTOL, KÚRIA, MEŠTIANSKY DOM). |
| `architecturalStyle` | Property | `string` |  |  | `jc:architecturalStyle` | The prevailing style, as the register writes it. |
| `constructionPeriod` | Property | `string` |  |  | `jc:constructionPeriod` | When the object was built, as the register writes it (a year, a century, a range). |
| `cadastralArea` | Property | `string` |  |  | `jc:cadastralArea` | The cadastral area the object stands in. |
| `ownershipForm` | Property | `string` |  |  | `jc:ownershipForm` | The form of ownership as the register categorises it (state, church, private); never the owner, whom the register does not name and this model would not carry. |
| `address` | Property | `string` |  |  | `schema:address` | The street address, one line, as the register of addresses writes it. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### GtfsStop

One railway station in the city and how many trains leave it today.

IRI: `sdm:GtfsStop`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The name, per language, as the publisher writes it. |
| `stopCode` | Property | `string` | yes |  | `jc:stopCode` | The station's `stop_id` in the ŽSR timetable. |
| `dailyDepartures` | Property | `integer` |  |  | `jc:dailyDepartures` | How many trains leave the station on the day the pipeline ran, by the timetable's calendar and its exceptions; a train that ends its run at the station does not count. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `sdm:source` | The open-data service the entity was read from. |
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
