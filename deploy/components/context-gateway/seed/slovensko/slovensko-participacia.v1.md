# Citizen participation in Slovakia

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What a town publishes on its participation platform (Decidim): the participatory processes and assemblies, the proposals residents filed, the projects of a participatory budget, the results the town reports, the meetings, the debates and the surveys. Each record is copied from the platform's public GraphQL API, or from its public pages where the API hides it, and carries the link back to its page. No personal data: author names, profiles, comments and their authors are never read, and e-mail addresses and phone numbers are cut from every text. An id is `urn:ngsi-ld:{Type}:slovensko-participacia:{instance}-{Decidim id}`, so a record keeps its entity from run to run; one no run has seen for three days leaves the space (expiresAt). ProposalStatistic is the one derived class: the counts the space computes from its own Proposal records.

- Namespace: `https://joinedcontext.com/models/slovensko/slovensko-participacia`
- Rendered by: `linkml-1.12.0`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### ParticipatoryProcess

One participatory process or assembly of the platform: a participatory budget year, a consultation on a strategy, a survey. Its phases and dates as the town set them.

IRI: `jc:ParticipatoryProcess`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `processType` | Property | [`ParticipatorySpaceType`](#participatoryspacetype) |  |  | `jc:processType` | Whether the space is a participatory process or a standing assembly. |
| `status` | Property | [`ProcessStatus`](#processstatus) |  |  | `jc:status` | Whether the process has not started, runs or is over, from its dates on the day of the run. |
| `startDate` | Property | `string` |  |  | `schema:startDate` | When it starts, a date (`2026-05-01`) or an RFC 3339 instant with its offset. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When it ends, a date or an RFC 3339 instant with its offset. |
| `activeStep` | Property | `string` |  |  | `jc:activeStep` | The title of the phase the process is in, `Hlasovanie`. |
| `steps` | Property | [`ProcessStep`](#processstep) (list) |  |  | `jc:steps` | The phases of the process in their order, each with its title and dates. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### ProcessStep

The value of one of `steps`, never an entity of its own.

IRI: `jc:ProcessStep`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `title` | Property | `string` |  |  | `schema:name` | The phase's title. |
| `startDate` | Property | `string` |  |  | `schema:startDate` | When it starts, a date (`2026-05-01`) or an RFC 3339 instant with its offset. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When it ends, a date or an RFC 3339 instant with its offset. |
| `active` | Property | `boolean` |  |  | `jc:active` | Whether this phase is the current one. |

### Proposal

One proposal residents or the town filed in a process, with the town's answer and its support. The author is never kept.

IRI: `jc:Proposal`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `proposalState` | Property | `string` |  |  | `jc:proposalState` | The town's answer as Decidim's state token: `not_answered`, `evaluating`, `accepted`, `rejected`, `withdrawn`, or a token the instance defined itself. Empty when only the page was readable; `stateLabel` then holds what the page shows. |
| `stateLabel` | Property | `string` |  |  | `jc:stateLabel` | The state as the page shows it, `Postupuje do hlasovania`. |
| `proposalAnswer` | Property | `string` |  |  | `jc:proposalAnswer` | The town's published answer to the proposal. Only the platform's own Decidim sends it, because the public API hides it. |
| `voteCount` | Property | `integer` |  |  | `jc:voteCount` | How many votes the proposal or project has, where the platform shows them. |
| `endorsementsCount` | Property | `integer` |  |  | `jc:endorsementsCount` | How many residents support the proposal. |
| `official` | Property | `boolean` |  |  | `jc:official` | Whether the town itself filed the proposal. |
| `category` | Property | `string` |  |  | `jc:category` | The category the platform files the record under, `Životné prostredie`. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where it is, as the platform writes it; the town is the instance's. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateCreated` | Property | `datetime` |  |  | `sdm:dateCreated` | When the platform created or published the record. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### BudgetProject

One project of a participatory budget, what it costs and whether the vote selected it.

IRI: `jc:BudgetProject`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `budgetAmount` | Property | `integer` |  |  | `jc:budgetAmount` | What the project costs, in euro. |
| `selected` | Property | `boolean` |  |  | `jc:selected` | Whether the vote selected the project for funding. |
| `voteCount` | Property | `integer` |  |  | `jc:voteCount` | How many votes the proposal or project has, where the platform shows them. |
| `budgetName` | Property | `string` |  |  | `jc:budgetName` | The budget the project competes in, `Online hlasovanie 2025/2026`. |
| `budgetTotal` | Property | `integer` |  |  | `jc:budgetTotal` | The whole budget the projects share, in euro. |
| `category` | Property | `string` |  |  | `jc:category` | The category the platform files the record under, `Životné prostredie`. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where it is, as the platform writes it; the town is the instance's. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Result

One result the town reports on (Decidim accountability), with its progress and dates.

IRI: `jc:Result`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `progress` | Property | `float` |  |  | `jc:progress` | How far the town reports the result to be, in percent. |
| `resultStatus` | Property | `string` |  |  | `jc:resultStatus` | The status the town gives the result, `Realizuje sa`. |
| `startDate` | Property | `string` |  |  | `schema:startDate` | When it starts, a date (`2026-05-01`) or an RFC 3339 instant with its offset. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When it ends, a date or an RFC 3339 instant with its offset. |
| `category` | Property | `string` |  |  | `jc:category` | The category the platform files the record under, `Životné prostredie`. |
| `refParent` | Relationship | `uriorcurie` |  |  | `jc:refParent` | The Result this one is part of, its URN. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Meeting

One public meeting of a process, where and when; a private one is never read.

IRI: `jc:Meeting`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `startDate` | Property | `string` | yes |  | `schema:startDate` | When it starts, a date (`2026-05-01`) or an RFC 3339 instant with its offset. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When it ends, a date or an RFC 3339 instant with its offset. |
| `meetingType` | Property | [`MeetingType`](#meetingtype) |  |  | `jc:meetingType` | How people attend, `in_person`, `online` or `hybrid`. |
| `address` | Property | [`PostalAddress`](#postaladdress) |  |  | `schema:address` | Where it is, as the platform writes it; the town is the instance's. |
| `onlineUrl` | Property | `uri` |  |  | `jc:onlineUrl` | Where an online meeting is held. |
| `closed` | Property | `boolean` |  |  | `jc:closed` | Whether the meeting is over and its minutes are published. |
| `closingReport` | Property | `string` |  |  | `jc:closingReport` | The minutes of a closed meeting, once they are public. Only the platform's own Decidim sends them. |
| `attendeesCount` | Property | `integer` |  |  | `jc:attendeesCount` | How many people attended a closed meeting, once that is public. Only the platform's own Decidim sends it. |
| `category` | Property | `string` |  |  | `jc:category` | The category the platform files the record under, `Životné prostredie`. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Debate

One debate of a process, its question and dates; the contributions are not kept.

IRI: `jc:Debate`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `description` | Property | `string` |  |  | `schema:description` | The text as plain text without markup, at most 2000 characters, e-mail addresses and phone numbers cut out. |
| `startDate` | Property | `string` |  |  | `schema:startDate` | When it starts, a date (`2026-05-01`) or an RFC 3339 instant with its offset. |
| `endDate` | Property | `string` |  |  | `schema:endDate` | When it ends, a date or an RFC 3339 instant with its offset. |
| `category` | Property | `string` |  |  | `jc:category` | The category the platform files the record under, `Životné prostredie`. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `reference` | Property | `string` |  |  | `jc:reference` | Decidim's reference code of the record, `BB-PROP-2026-05-21616`. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateCreated` | Property | `datetime` |  |  | `sdm:dateCreated` | When the platform created or published the record. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### Survey

One survey of a process, its title and link only; the questions and answers are not kept.

IRI: `jc:Survey`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `url` | Property | `uri` |  |  | `schema:url` | The record's own page on the participation platform. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `sourceId` | Property | `string` |  |  | `jc:sourceId` | The record's id on the platform (Decidim's own numeric id). |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateCreated` | Property | `datetime` |  |  | `sdm:dateCreated` | When the platform created or published the record. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### ProposalStatistic

How many proposals of one process are in one state, counted from the Proposal records of this space. It holds figures only, so no text a resident wrote reaches it.

IRI: `jc:ProposalStatistic`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `name` | Property | `string` | yes |  | `schema:name` | The title as the platform shows it, in the instance's language. |
| `processName` | Property | `string` | yes |  | `jc:processName` | The title of the process the figure counts, as the platform shows it. |
| `proposalState` | Property | `string` |  |  | `jc:proposalState` | The town's answer as Decidim's state token: `not_answered`, `evaluating`, `accepted`, `rejected`, `withdrawn`, or a token the instance defined itself. Empty when only the page was readable; `stateLabel` then holds what the page shows. |
| `stateLabel` | Property | `string` |  |  | `jc:stateLabel` | The state as the page shows it, `Postupuje do hlasovania`. |
| `proposalCount` | Property | `integer` | yes |  | `jc:proposalCount` | How many proposals of the process are in this state. |
| `refParticipatoryProcess` | Relationship | `uriorcurie` | yes |  | `jc:refParticipatoryProcess` | The ParticipatoryProcess of this space the record belongs to, its URN. |
| `source` | Property | `uri` |  |  | `sdm:source` | The participation platform the record was read from, its base URL. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | The town or body that runs the participation platform. |
| `dateModified` | Property | `datetime` |  |  | `sdm:dateModified` | When the platform last changed the record. |
| `id` | Property | `string` | yes |  | `ngsi-ld:hasId` | The entity id, urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}. |
| `type` | Property | `string` | yes |  | `ngsi-ld:hasType` | The entity type, one class of a published data model. |
| `location` | GeoProperty | `string` |  |  | `geojson:geometry` | Where the entity is, as GeoJSON geometry. |
| `observedAt` | Property | `datetime` |  |  | `ngsi-ld:observedAt` | When the observation the entity reports was made. |

### PostalAddress

The value of `address`, never an entity of its own.

IRI: `schema:PostalAddress`

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `streetAddress` | Property | `string` |  |  | `schema:streetAddress` | The street and number, or the place, as the platform writes it. |
| `addressLocality` | Property | `string` |  |  | `schema:addressLocality` | The town. |
| `addressCountry` | Property | `string` |  |  | `schema:addressCountry` | The country code, `SK`. |

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

### ParticipatorySpaceType

| Value | Meaning |
|---|---|
| `process` |  |
| `assembly` |  |

### ProcessStatus

| Value | Meaning |
|---|---|
| `upcoming` |  |
| `active` |  |
| `past` |  |

### MeetingType

| Value | Meaning |
|---|---|
| `in_person` |  |
| `online` |  |
| `hybrid` |  |
