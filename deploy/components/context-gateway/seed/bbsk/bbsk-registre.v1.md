# Registers of the Banská Bystrica region

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the region's register space carries side by side (DM-01, DM-61): the outlines of its 13 districts and 516 municipalities, the organisations the region founded, the hospitals, the social services run by public bodies, and the bridges of the road network, each read from the region's own open-data portal (opendata.bbsk.sk). One model, one space, one public endpoint. Where Smart Data Models has the class, its IRI is cited rather than minted (DM-04, DM-16).

- Namespace: `https://joinedcontext.com/models/bbsk/bbsk-registre`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### AdministrativeArea

One district (okres) or municipality (obec) of the region, its outline and its code, as the Smart Data Model extension AdministrativeArea (PointOfInterest base).

IRI: `jc:AdministrativeArea`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the publisher writes it, in Slovak; a string, as every Smart Data Model's `name` is. |
| `areaCode` | Property | `string` | yes |  | `jc:areaCode` | The area's statistical code: LAU 1 (`SK0321`) for a district, LAU 2 (`SK0321508438`) for a municipality, the same codes the Statistical Office's cubes carry in `refArea`. |
| `divisionLevel` | Property | [`DivisionLevel`](#divisionlevel) | yes |  | `jc:divisionLevel` | Which tier of the territorial division the area is. |
| `refParentArea` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refParentArea` | The district a municipality lies in; absent on a district. |
| `surfaceArea` | Property | `float` |  | m2 | `jc:surfaceArea` | The area's surface as the cadastre states it. |
| `isPartOf` | Property | `uri` |  |  | `jc:isPartOf` | The catalogue dataset the record belongs to, which tells the region's districts from its municipalities in one type. |
| `dataProvider` | Property | `string` |  |  | `smartdatamodels:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `smartdatamodels:source` | The open-data service the entity was read from. |
| `childAreas` | Relationship | [`AdministrativeArea`](#administrativearea) (list) |  |  | `jc:childAreas` | The municipalities of a district, computed from their refParentArea and never stored. |
| `publicOrganizations` | Relationship | [`PublicOrganization`](#publicorganization) (list) |  |  | `jc:publicOrganizations` | The organisations the region founded in this municipality, computed from their refMunicipality and never stored. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PublicOrganization

One organisation the region founded: a secondary school, a social-care home, a theatre, a museum or an office. The FIWARE Smart Data Model CPSV-AP PublicOrganization.

IRI: `sdm-cpsv:PublicOrganization`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the publisher writes it, in Slovak; a string, as every Smart Data Model's `name` is. |
| `preferredLabel` | Property | `string` |  |  | `sdm-cpsv:preferredLabel` | The organisation's official name, as CPSV-AP's PublicOrganization requires it. |
| `spatial` | Property | `string` (list) |  |  | `sdm-cpsv:spatial` | The municipality the organisation is in, by name, as CPSV-AP's PublicOrganization requires it. |
| `alternateName` | Property | `string` |  |  | `schema:alternateName` | The shorter name the organisation goes by, when it differs from the official one. |
| `legalId` | Property | `string` |  |  | `jc:legalId` | The body's identification number (IČO) in the Slovak register of legal persons. |
| `organizationCategory` | Property | [`OrganizationCategory`](#organizationcategory) | yes |  | `jc:organizationCategory` | The field the region founded the organisation for. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the register publishes it, field by field (Smart Data Models' common PostalAddress); a field the register leaves out is absent, never guessed. |
| `refMunicipality` | Relationship | [`AdministrativeArea`](#administrativearea) |  |  | `jc:refMunicipality` | The municipality the record lies in. |
| `dataProvider` | Property | `string` |  |  | `smartdatamodels:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `smartdatamodels:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One hospital (by its facility number in the health-care register) or one registered social service a municipality, a town or the region provides; `category` says which. The FIWARE Smart Data Model PointOfInterest: no published model is a hospital or a social service. Private providers of social services are not in the space.

IRI: `sdm-poi:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the publisher writes it, in Slovak; a string, as every Smart Data Model's `name` is. |
| `category` | Property | [`PlaceCategory`](#placecategory) (list) | yes |  | `sdm-poi:category` | What kind of place it is: `hospital` or `socialService`. The two registers share the Smart Data Model PointOfInterest, and an application asks for one with `q=category=="hospital"`. |
| `hospitalKind` | Property | [`HospitalKind`](#hospitalkind) |  |  | `jc:hospitalKind` | Whether the hospital is a general one or specialises in some fields. |
| `operatorName` | Property | `string` |  |  | `jc:operatorName` | The legal person that runs it, as the register names it. |
| `medicalSpecialties` | Property | `string` |  |  | `jc:medicalSpecialties` | The fields of medicine the hospital is licensed for, with their codes, as the register lists them. |
| `serviceKind` | Property | `string` |  |  | `jc:serviceKind` | The kind of social service in the words of Act 448/2008 Z. z., such as `zariadenie pre seniorov` or `opatrovateľská služba`. |
| `serviceForm` | Property | [`ServiceForm`](#serviceform) |  |  | `jc:serviceForm` | How the service reaches the person it serves. |
| `targetGroup` | Property | `string` |  |  | `jc:targetGroup` | Whom the service is for, as the provisions of the Act the register cites. |
| `capacity` | Property | `integer` |  |  | `jc:capacity` | How many people the service may take at once, as registered; absent for a service registered without one. |
| `providerKind` | Property | [`ProviderKind`](#providerkind) |  |  | `jc:providerKind` | Which public body provides the service. |
| `legalId` | Property | `string` |  |  | `jc:legalId` | The body's identification number (IČO) in the Slovak register of legal persons. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address as the register publishes it, field by field (Smart Data Models' common PostalAddress); a field the register leaves out is absent, never guessed. |
| `districtName` | Property | `string` |  |  | `jc:districtName` | The district the record lies in, by name, as the publisher writes it. |
| `url` | Property | `string` |  |  | `schema:url` | The web page the publisher gives for it. |
| `dataProvider` | Property | `string` |  |  | `smartdatamodels:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `smartdatamodels:source` | The open-data service the entity was read from. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PostalAddress

The value of `address`: a postal address, never an entity of its own (Smart Data Models' common schema).

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and the house number, or the venue. |
| `postalCode` | Property | `string` |  |  | `schema:postalCode` | The postal code. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The town or the municipality. |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country, as an ISO 3166-1 alpha-2 code. |

### Bridge

One bridge of the road network in the region, its road, its age, its size and who manages it, as the Smart Data Model extension Bridge (PointOfInterest base).

IRI: `jc:Bridge`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` |  |  | `schema:name` | The name as the publisher writes it, in Slovak; a string, as every Smart Data Model's `name` is. |
| `bridgeCode` | Property | `string` | yes |  | `jc:bridgeCode` | The bridge's number in the national road databank (Cestná databanka), such as `M9673`. |
| `roadClass` | Property | [`RoadClass`](#roadclass) |  |  | `jc:roadClass` | The class of the road the bridge carries. |
| `roadNumber` | Property | `string` |  |  | `jc:roadNumber` | The number of the road the bridge carries, such as `66b`. |
| `yearBuilt` | Property | `integer` |  |  | `jc:yearBuilt` | The year the bridge was built. |
| `spanCount` | Property | `integer` |  |  | `jc:spanCount` | How many openings the bridge has. |
| `bridgedLength` | Property | `float` |  | m | `jc:bridgedLength` | The length of the bridged gap. |
| `structureMaterial` | Property | `string` |  |  | `jc:structureMaterial` | The material of the load-bearing structure, in the databank's words. |
| `structureKind` | Property | `string` |  |  | `jc:structureKind` | The kind of load-bearing structure, in the databank's words, such as `dosková`. |
| `heritageStatus` | Property | [`HeritageStatus`](#heritagestatus) |  |  | `jc:heritageStatus` | Whether the bridge is a listed monument; absent where the databank does not say. |
| `managerName` | Property | `string` |  |  | `jc:managerName` | The road administration that manages the bridge. |
| `districtName` | Property | `string` |  |  | `jc:districtName` | The district the record lies in, by name, as the publisher writes it. |
| `dataProvider` | Property | `string` |  |  | `smartdatamodels:dataProvider` | Who publishes the data the entity was read from, with the credit its licence asks for, as an application shows it beside the data. |
| `source` | Property | `uri` |  |  | `smartdatamodels:source` | The open-data service the entity was read from. |
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

### PlaceCategory

| Value | Meaning |
|---|---|
| `hospital` | A general or specialised hospital. |
| `socialService` | A registered social service. |

### DivisionLevel

| Value | Meaning |
|---|---|
| `district` | A district (okres), LAU 1. |
| `municipality` | A municipality or town (obec, mesto), LAU 2. |

### OrganizationCategory

| Value | Meaning |
|---|---|
| `school` | A secondary school or a school facility (školy). |
| `socialCare` | A social-care facility (sociálne). |
| `culture` | A theatre, a museum, a gallery, a library or an education centre (kultúra). |
| `office` | An office or an agency of the region (úrad). |

### HospitalKind

| Value | Meaning |
|---|---|
| `general` | A general hospital (všeobecná nemocnica). |
| `specialised` | A specialised hospital (špecializovaná nemocnica). |

### ServiceForm

| Value | Meaning |
|---|---|
| `field` | Provided where the person lives (terénna). |
| `outpatient` | The person comes to it (ambulantná). |
| `residentialYearRound` | The person lives there all year (pobytová - ročná). |
| `residentialWeekly` | The person lives there during the week (pobytová - týždenná). |
| `remote` | By telephone or another telecommunication means (iná forma). |

### ProviderKind

| Value | Meaning |
|---|---|
| `municipality` | A municipality or a town itself (obec/mesto). |
| `municipalityFounded` | A legal person a municipality or a town founded. |
| `regionFounded` | A legal person the self-governing region founded. |

### RoadClass

| Value | Meaning |
|---|---|
| `motorway` | A motorway or an expressway (diaľnica). |
| `firstClass` | A first-class road (cesta I. triedy). |
| `secondClass` | A second-class road (cesta II. triedy). |
| `thirdClass` | A third-class road (cesta III. triedy). |
| `local` | A local road of no stated class (miestna neurčená). |
| `service` | A service road (účelová cesta). |

### HeritageStatus

| Value | Meaning |
|---|---|
| `notListed` | The bridge is not a listed monument. |
| `cultural` | The bridge is a listed cultural monument. |
| `culturalAndTechnical` | The bridge is listed as a cultural and a technical monument. |
