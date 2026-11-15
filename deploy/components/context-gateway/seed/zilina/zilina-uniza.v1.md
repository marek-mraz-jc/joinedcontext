# Open research of the University of Žilina

<!-- Generated from the LinkML source by Model Tools. Do not edit: `jcctl model
     generate` overwrites this file and CI fails on any difference (DM-01, DM-02). -->

What the university's space carries (DM-01, DM-61): one entity per item of DREPO, the university's digital library, that carries an open licence of its own, described by its title, kind, year, collection and licence. Authors are people, so the model has no slot for them: an application counts and lists works, it does not list who wrote them. Sources: Research/zilina-open-data-sources.md.

- Namespace: `https://joinedcontext.com/models/zilina/zilina-uniza`
- Rendered by: `linkml-1.11.1`
- License: https://joinedcontext.com/licenses/cc-by-4.0

## Classes

### Article

One openly licensed work of the University of Žilina's digital library, as the Smart Data Model extension Article (CPSV-AP Evidence base).

IRI: `jc:Article`

Specialises `Entity`.

| Attribute | NGSI-LD kind | Range | Required | Unit | IRI | Description |
|---|---|---|---|---|---|---|
| `identifier` | Property | `string` | yes |  | `sdm:dataModel.CPSV-AP/identifier` | The work's handle in the repository (`hdluniza/936`), the identifier the Article extension requires. |
| `name` | LanguageProperty | `string` |  |  | `schema:name` | The work's title, per language, as the repository records it. |
| `workType` | Property | `string` |  |  | `jc:workType` | What the work is, in the repository's words (Article, Conference paper, Thesis, …). |
| `yearPublished` | Property | `integer` |  |  | `jc:yearPublished` | The year the work was issued. |
| `isPartOf` | Property | `uri` |  |  | `schema:isPartOf` | The address of the repository collection the work was deposited in (`https://dspace.uniza.sk/handle/hdluniza/556`). |
| `collectionName` | Property | `string` |  |  | `jc:collectionName` | The repository collection the work was deposited in, as DREPO names it: a journal issue, a proceedings volume or a series (`Krízový manažment - Ročník 24.; Číslo 2/2025`). |
| `publisher` | Property | `string` |  |  | `schema:publisher` | Who issued the work, as the repository records it. |
| `license` | Property | `uri` | yes |  | `schema:license` | The licence the work itself carries (`dc.rights.uri`); only open licences reach the space. |
| `url` | Property | `uri` |  |  | `schema:url` | The work's persistent handle in the repository. |
| `dataProvider` | Property | `string` |  |  | `sdm:dataProvider` | Who publishes the record, with the credit its licence asks for. |
| `source` | Property | `uri` |  |  | `sdm:source` | The repository service the entity was read from. |
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
