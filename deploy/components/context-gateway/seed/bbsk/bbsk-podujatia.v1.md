# Events in Slovakia

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

One public event as a town, a region or a cultural body announces it on its web site or feed: what it is, when it starts and ends, and where, as a GPS point. The class is schema.org's Event under the names Smart Data Models and schema.org use, so the space's iCalendar surface (RFC 5545, EP-125) reads it as published: `id` the UID, `name` the SUMMARY, `startDate` and `endDate` DTSTART and DTEND, `description`, `address`, `location` and `url` DESCRIPTION, LOCATION, GEO and URL, `organizer` ORGANIZER. An event without a name, a start or a link is refused by the pipeline and never reaches the space (owner 2026-10-20, 2026-10-22); its point and organizer are kept where known. One event announced by two sources is one entity: its id is `urn:ngsi-ld:Event:bbsk-podujatia:` followed by the first 16 hex digits of the SHA-256 of `name \| start \| lower(addressLocality)` (the name in lower case with only its letters and digits, the start to the hour in UTC or the day), so the second source overwrites the first rather than adding a copy.

- Namespace: `https://joinedcontext.com/models/bbsk/bbsk-podujatia`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Event

One public event: its name, its start and a link are required (owner 2026-10-22); its place as a GPS point is expected and filled from the source or the venue table, its organizer kept where the source names one, the rest kept where the source gives it.

IRI: `schema:Event`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title of the event as the source writes it, in Slovak. |
| `startDate` | Property | `string` | yes |  | `schema:startDate` | When the event starts: an RFC 3339 instant with its offset (`2026-11-14T19:00:00+01:00`, Europe/Bratislava as the source publishes it), or a date alone (`2026-11-14`) for an event that lasts whole days. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When the event ends, written as `startDate`; for a whole-day event its last day, inclusive. Absent when the source gives no end: the calendar then shows the start alone. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the event takes place, a GeoJSON Point in WGS 84 inside Slovakia (longitude 16.8 to 22.6, latitude 47.7 to 49.7): the source's own point, the venue's building, else the organizer's or the town's office or the town's centre, as `locationPrecision` says (owner 2026-10-21: at least 90 % of events with a point). |
| `locationPrecision` | Property | [`LocationPrecision`](#locationprecision) |  |  | `jc:locationPrecision` | How close `location` is to the venue: `source` the publisher's own point, `building` the venue's building (the address register or a venue checked against OpenStreetMap), `office` the organizer's or the town's office standing in for a venue nobody placed, `centre` the town's centre. A map draws `office` and `centre` as approximate. |
| `locationSource` | Property | `string` |  |  | `jc:locationSource` | Who placed `location`: `feed` the publisher, `register-adries`, `overpass`, `nominatim` or `hand` the venue table's row (seeds/bbsk-events/venues.csv), `towns` the town table's office or centre. |
| `venue` | Property | `string` |  |  | `jc:venue` | The name of the place, as the source writes it (a hall, a square, a museum). |
| `refPointOfInterest` | Relationship | [`PointOfInterest`](#pointofinterest) |  |  | `sdm:refPointOfInterest` | The venue as a PointOfInterest of this space, when the venue table placed the event; every event held there links the same one (DM-141). |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | The municipality the event or the venue belongs to, by its official name (`Detva`, `Košice`): the key a count or a filter per town reads, on every event (DM-141). |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address of the venue, field by field (Smart Data Models' common PostalAddress); `addressLocality` is the town a calendar filters on. A field the source leaves out is absent, never guessed. |
| `category` | Property | [`EventCategory`](#eventcategory) (list) |  |  | `sdm:category` | The event's main category, one code of the platform's list of 16 (docs API/12-event-categories.md), picked by the rule table seeds/event-categories.json from the source's own word, the name or the kind of body that announced it (`music`, `sport`), else by the decision model's cached answer. A list of one, as Smart Data Models' Event writes `category`; absent while neither has named the event. |
| `eventCategory` | Property | `string` |  |  | `jc:eventCategory` | The kind of event in the source's own words (`koncert`, `výstava`, `šport`), kept so `category` can be traced back to it. |
| `audience` | Property | [`EventAudience`](#eventaudience) (list) |  |  | `sdm:audience` | Who the event is for, in Smart Data Models' Event codes; set with the category `children-family`, `children` when the source or the name says children, else `family`. |
| `categorySource` | Property | `string` |  |  | `jc:categorySource` | How `category` was picked: `rule` the rule table, `model` the decision model's cached answer for an event no rule decides (scripts/events-classify.py). |
| `description` | Property | `string` |  |  | `schema:description` | What the source says about the event, as plain text without markup. |
| `organizer` | Property | [`Organizer`](#organizer) |  |  | `schema:organizer` | Who runs the event: the museum, the school, the town or the named performer, never the feed or the aggregator that announced it (that is `dataProvider`). Its name and its web site only, never a person's e-mail or phone. |
| `url` | Property | `uri` | yes |  | `schema:url` | An http(s) link a person opens: the page of the event at its source, where they read more or buy a ticket, else the organizer's page that lists its events. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who announced the event (the town, the region, the cultural body), as the credit names it. |
| `source` | Property | `uri` |  |  | `sdm:source` | The feed or the page the pipeline read the event from. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The event's own identifier at its source, so a record can be traced back. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the source last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PointOfInterest

One venue of the venue table (seeds/bbsk-events/venues.csv): a hall, a square, a museum where events take place, one entity per real place (table rows of one name within 50 m are one venue), its id the OpenStreetMap object of its building or a hash of its town, name and point (DM-141).

IRI: `sdm:PointOfInterest`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The venue's name (`Dom kultúry A. Sládkoviča`). |
| `location` | GeoProperty | `string` | yes |  | `geojson:geometry` | The venue's building as a GeoJSON Point in WGS 84 inside Slovakia. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | The postal address of the venue, field by field (Smart Data Models' common PostalAddress); `addressLocality` is the town a calendar filters on. A field the source leaves out is absent, never guessed. |
| `areaServed` | Property | `string` |  |  | `sdm:areaServed` | The municipality the event or the venue belongs to, by its official name (`Detva`, `Košice`): the key a count or a filter per town reads, on every event (DM-141). |
| `source` | Property | `uri` |  |  | `sdm:source` | The feed or the page the pipeline read the event from. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who announced the event (the town, the region, the cultural body), as the credit names it. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PostalAddress

The value of `address`, never an entity of its own.

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and the house number. |
| `postalCode` | Property | `string` |  |  | `schema:postalCode` | The postal code. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The town or the municipality, by its Slovak name with diacritics (`Banská Bystrica`). |
| `addressRegion` | Property | `string` |  |  | `schema:addressRegion` | The self-governing region, by its Slovak name (`Banskobystrický kraj`). |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country, as an ISO 3166-1 alpha-2 code. |

### Organizer

The value of `organizer`, never an entity of its own.

IRI: `schema:Organization`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The organizer's name as the source writes it (`Stredoslovenské múzeum`). |
| `url` | Property | `uri` |  |  | `schema:url` | The organizer's own web site, when the source names it. |

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

### LocationPrecision

How close an event's point is to its venue; office and centre are approximate.

| Value | Meaning |
|---|---|
| `source` | The point the publisher gives. |
| `building` | The venue's building. |
| `office` | The organizer's or the town's office, approximate. |
| `centre` | The town's centre, approximate. |

### EventCategory

The main event categories of the platform, with labels in sk, cs, en and de.

| Value | Meaning |
|---|---|
| `sport` |  |
| `outdoor-nature` | Smart Data Models category outdoorActivities, excursion. |
| `film` | Smart Data Models category spectacle. |
| `theatre-dance` | Smart Data Models category spectacle. |
| `music` | Smart Data Models category spectacle. |
| `festival-fair` | Smart Data Models category spectacle. |
| `food-market` | Smart Data Models category gastronomy, shopping. |
| `exhibition-museum` | Smart Data Models category museum. |
| `history-heritage` | Smart Data Models category history. |
| `faith-tradition` | Smart Data Models category religiousWorship. |
| `education-talk` |  |
| `children-family` |  |
| `hobby-workshop` |  |
| `health-wellness` | Smart Data Models category wellness. |
| `community-civic` |  |
| `other` | No other code fits; only the decision model gives it, never the rule table. |

### EventAudience

The Smart Data Models Event audience codes the pipelines write.

| Value | Meaning |
|---|---|
| `children` |  |
| `family` |  |
