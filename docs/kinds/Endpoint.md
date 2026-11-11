# Endpoint

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Endpoint`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/endpoints`
- Repository path: `projects/{project}/spaces/{space}/endpoints/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (EndpointSpec)

Desired specification of an `Endpoint` resource (EP-01..EP-28).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedProjects` | list of string | no | Whitelist of project slugs permitted when audience is `project-list` (EP-14, EP-15). |
| `audience` | [Audience](#audience) | yes | Audience access scope (EP-14). |
| `audit` | [Audit](#audit) | no | `changes`: every write through this Endpoint that changes a value is kept as a version with its provenance, whatever the type's history rule (ADR-N-054 2.8, DM-136). |
| `caching` | [Caching](#caching) | no | Optional response caching configuration. |
| `callerRole` | boolean | no | Every caller this Endpoint admits holds the role [`endpoint_role`] names for it, on requests through this Endpoint alone (AP-96, AP-97). The App reconciler sets it, so an App's grants never reach another surface of the space. |
| `catalog` | [Catalog](#catalog) | no | What a DCAT-AP catalogue needs beyond the distributions: publisher, contact, licence, themes, coverage, frequency and provenance (EP-78, EP-79). |
| `contextSpaceRef` | [Ref](#ref) | yes | Reference to the underlying ContextSpace. |
| `creates` | [Creates](#creates) | no | How the Endpoint takes new entities: minted ids and a daily cap (EP-97). |
| `deprecation` | [Deprecation](#deprecation) | no | A breaking change announced ahead: the gateway answers with `Deprecation` and `Sunset` headers until the deprecated part is removed (EP-146, EP-147). |
| `disabledRepresentations` | list of [Representation](#representation) | no | Representations an Endpoint without `enabledRepresentations` does not serve (EP-104). |
| `display` | map of [Display](#display) | no | Per entity type, the slots this Endpoint's tables, popups and CKAN columns show and its key attributes, reordering and narrowing the model's display order (EP-135, DM-98). |
| `enabledRepresentations` | list of [Representation](#representation) | no | The representations this Endpoint serves, listed (EP-05). Omitted: every one but `sta`, less `disabledRepresentations` (EP-104); see [`EndpointSpec::served_representations`]. |
| `facets` | list of string | no | The attributes a reader may list distinct values of at `/facets` (EP-178), at most [`MAX_FACETS`]: an attribute name or `name.member`, e.g. `address.addressLocality`. |
| `fileLimits` | [FileLimits](#filelimits) | no | Optional ceiling on one `file.*` download (EP-44). |
| `mcp` | boolean | no | `false` turns the Endpoint's MCP instance off. Every Endpoint serves `mcp` without listing it, public or internal, gated by its own Policy (EP-24, T-2901); see [`EndpointSpec::served_representations`]. |
| `policyRef` | [Urn](#urn) | no | Optional NGSI-LD Policy URN attached to this endpoint; means `policyRefs: [it]`. Written `urn:ngsi-ld:Policy:{orgDomain}:{space}:{name}`, where `{space}` is the rendered segment of the space: its `urnSegment`, else `{project}-{name}`, never the bare space name (EP-117). |
| `policyRefs` | list of [Urn](#urn) | no | The Policies this Endpoint evaluates, each a Policy URN of its space (ADR-N-057 decision 2), written `urn:ngsi-ld:Policy:{orgDomain}:{space}:{name}`, where `{space}` is the rendered segment of the space: its `urnSegment`, else `{project}-{name}`, never the bare space name. Never set beside `policyRef` and never empty; read both through [`EndpointSpec::policy_refs`]. |
| `projection` | [Projection](#projection) | no | Optional publication narrowing applied to every representation (EP-61). |
| `projectionRef` | [TypedRef](#typedref) | no | The named subset of the space's model this Endpoint reads (MP-02): a `ModelProjection` of the same space, intersected with the caller's grants by the gateway, so it narrows and never widens. |
| `publish` | [Publication](#publication) | no | Where this Endpoint is published beside its own surface: an open-data catalogue so far (EP-62). |
| `rateLimits` | [RateLimits](#ratelimits) | no | Optional rate limiting configuration (EP-20). |
| `representations` | [Representations](#representations) | no | The mappings of the domain-standard representations, `ical` first (EP-125, API/02 §8b). |
| `roles` | list of [EndpointRole](#endpointrole) | no | Roles a caller this Endpoint admits holds when a subject matches them, on requests through it alone (AP-96, AP-97). |
| `slug` | [EndpointSlug](#endpointslug) | yes | Unguessable opaque slug with >= 128 bits of entropy (EP-02). |
| `viewMappingRef` | [TypedRef](#typedref) | no | A `Mapping` whose target model this Endpoint serves instead of the space's own (EP-54, DM-51). The gateway translates every entity through the Mapping's compiled IR and rewrites `attrs`, `q` and `geoQ` back through its inverse. An Endpoint that carries one is read only: a mapping is invertible per slot, not per entity, so a write in the target model has no source entity to reconstruct. |

## Audience

Target audience access control setting for an endpoint (EP-14).

Type: one of `"project-list"`, `"organization"`, `"public"`

## Audit

Whether an Endpoint keeps an audit trail of the writes through it (ADR-N-054 2.8, DM-136).

Type: one of `"off"`, `"changes"`

## Caching

HTTP caching configuration for endpoint responses.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxAgeSeconds` | integer (uint32) | yes | Maximum cache age in seconds. |

## Catalog

The catalogue description of one Endpoint (EP-78).

| Field | Type | Required | Description |
|---|---|---|---|
| `applicableLegislation` | list of string | no | ELI IRIs of the laws that apply: `dcatap:applicableLegislation`. |
| `attribution` | map of string | no | The words the attribution duty asks for, per language: `dct:rights`. |
| `contactPoint` | [ContactPoint](#contactpoint) | no | Where questions about it go: `dcat:contactPoint`, a role address (EP-80). |
| `datasets` | list of [CatalogDataset](#catalogdataset) | no | The catalogue datasets of this Endpoint, each a slice narrowed by its types and query (ADR-N-068, EP-175). Listed, they replace the Endpoint's single dataset; empty is one dataset of the whole Endpoint. |
| `frequency` | [Frequency](#frequency) | no | How often the data changes, from the EU frequency table: `dct:accrualPeriodicity`. |
| `harvest` | boolean | no | Whether the installation's and the project's feeds list the record (EP-139, EP-84, EP-127). `false` for a door over data other datasets already publish, such as a whole space split into one dataset per type: its own record stays, a harvester never takes the same data twice. Unset is `true`. |
| `keywords` | map of list of string | no | Keywords per language: `dcat:keyword`. |
| `license` | [Licence](#licence) | no | The licence, from the EU licence table: `dct:license`. |
| `personalData` | [PersonalData](#personaldata) | no | Whether the data holds personal data, as the publisher states it; never read from the data. Only the national profiles carry it: `leg:personalDataContainmentType` and `pu:osobní-údaje` (API/08 section 5). |
| `pipelineRef` | [TypedRef](#typedref) | no | The Pipeline of this project that fills the space: `prov:wasGeneratedBy`. |
| `publisher` | [Publisher](#publisher) | no | Who publishes the dataset: `dct:publisher`. |
| `source` | list of [Source](#source) | no | The original open datasets the data comes from: `dct:source`. |
| `spatial` | list of string | no | NUTS codes or location IRIs: `dct:spatial`. |
| `temporal` | [Temporal](#temporal) | no | The period the data covers: `dct:temporal`. |
| `themes` | list of [DataTheme](#datatheme) | no | EU data themes: `dcat:theme`. |
| `visualizations` | list of [Visualization](#visualization) | no | Charts on the dataset, Vega-Lite 5 or Vega 5 over the Endpoint's own sources, published as CKAN `jc_visualization` views (ADR-N-053, EP-130); validated by the Endpoint, which knows what it serves. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## Creates

How an Endpoint takes new entities from callers who do not choose their ids: a public form (EP-97).

| Field | Type | Required | Description |
|---|---|---|---|
| `embedOrigins` | list of string | no | The sites that may frame the form's page `/f/{slug}`, each an `https` origin exactly as a browser names it (`https://www.example.org`, an optional non-default port, no path, no wildcard), at most 20. The Portal sends them in that page's `frame-ancestors` and nowhere else; empty, only the Portal frames the form (EP-101). |
| `mintIds` | boolean | no | The gateway replaces the id a create names with one it mints, `urn:ngsi-ld:{Type}:{uuid}`, so a caller can neither pick nor overwrite one. |
| `perDay` | integer (uint32) | no | At most this many creates through the Endpoint per UTC day, 1..=10000; past it the gateway answers 429 with `Retry-After` until midnight UTC. |

## Deprecation

A breaking change of a public Endpoint, announced ahead (EP-146).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | list of string | no | Only these attributes are deprecated. |
| `link` | string | no | A public `https` page about the change and its replacement. |
| `representations` | list of [Representation](#representation) | no | Only these surfaces are deprecated; empty, the whole Endpoint. |
| `since` | string (date) | yes | The day the deprecation was announced, `YYYY-MM-DD`. |
| `sunset` | string (date) | yes | The earliest day the deprecated part may stop answering, at least 90 days after `since`. |

## Representation

Supported representation formats served under an endpoint (EP-05, Architecture/04).

Note: `schema` (EP-46) and `access` (EP-55) are not variants: they are unconditional surfaces on every endpoint and cannot be toggled off. `file.csv` is the one file an Endpoint publishes; the names of the removed files are refused by name ([`REMOVED_REPRESENTATIONS`]).

Type: one of `"ngsi-ld"`, `"mcp"`, `"csv"`, `"ogc-features"`, `"sta"`, `"odata"`, `"websocket"`, `"ical"`, `"feed"`

## Display

An override of the model's display order for one type: an Endpoint's `spec.display` entry or a visualization's `fields` (EP-135). Either list empty leaves that part to the model.

| Field | Type | Required | Description |
|---|---|---|---|
| `fields` | list of string | no | The slots shown, in this order; the rest are not shown. |
| `summary` | list of string | no | The key attributes, in place of the model's `summary` annotations. |

## FileLimits

How much of a `file.*` representation one download may return (EP-44).

The gateway pages through the broker and stops at the first row that would cross either bound, so a caller gets a refusal rather than a file that is silently short. An absent field leaves the gateway's own ceiling in charge.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxFileBytes` | integer (uint64) | no | Bytes a single download may return. |
| `maxFileRows` | integer (uint32) | no | Rows a single download may return. |

## Urn

NGSI-LD entity URN (ADR-N-041, PF-43): urn:ngsi-ld:{Type}:{id}; the prefixed shape urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId} is optional

Type: string

## Projection

What this Endpoint never serves, whatever the Policy set allows (EP-61).

A publication decision, not an authorization one: the gateway intersects this list with the caller's policy projection, so an Endpoint can subtract from a grant and never add to it. The same space can therefore be published twice with different amounts of detail without writing a second Policy, and because the subtraction happens where every representation reads its entities, a hidden attribute is missing from the CSV, the GeoJSON, the MCP tool result and the schema surface alike (EP-06, EP-07, EP-47).

| Field | Type | Required | Description |
|---|---|---|---|
| `hiddenAttributes` | list of string | no | Attribute names this endpoint removes from every representation. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## Publication

Where an Endpoint is published beside its own surface (EP-62).

| Field | Type | Required | Description |
|---|---|---|---|
| `ckan` | [CkanPublication](#ckanpublication) | no | Publication to an open-data catalogue. |

## RateLimits

Token-bucket rate limiting configuration per endpoint (EP-20).

| Field | Type | Required | Description |
|---|---|---|---|
| `burst` | integer (uint32) | no | Optional maximum burst capacity above steady rate. |
| `requestsPerMinute` | integer (uint32) | yes | Maximum allowed requests per minute. |

## Representations

`spec.representations`: the mappings of the domain-standard representations (EP-125).

| Field | Type | Required | Description |
|---|---|---|---|
| `feed` | list of [FeedOutput](#feedoutput) | no | The feed outputs (Atom, RSS 2.0, JSON Feed); none means no feed (EP-150, EP-151). |
| `ical` | list of [IcalOutput](#icaloutput) | no | The iCalendar outputs (API/11 EP-159); none means no calendar is served. The single mapping object of version 1 loads as one output named `events` (deprecated). |
| `widget` | list of [WidgetOutput](#widgetoutput) | no | The catalogue widget's defaults per class: the map's geometry, colour, popup and clustering (API/11 EP-163). Never read by the broker. |

## EndpointRole

One role an Endpoint gives the callers it admits who match a subject (AP-96, AP-97).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The role's name, `a-z{0,31}`; the caller holds [`endpoint_role`] of it. |
| `subjects` | list of [Subject](#subject) | yes | Who holds it: `user` by e-mail, `group` by name, never a wildcard. |

## EndpointSlug

Cryptographically random endpoint slug with at least 128 bits of entropy (lowercase RFC 4648 base32 without padding, >=26 chars, EP-02)

Type: string

## ContactPoint

The contact point of a dataset, a `vcard:Kind`.

| Field | Type | Required | Description |
|---|---|---|---|
| `email` | string | yes | A role address, never a person's own (EP-80): `vcard:hasEmail`. |
| `name` | string | yes | The desk's name: `vcard:fn`. |

## CatalogDataset

One catalogue dataset of an Endpoint: a name, its texts, and the rows it holds (EP-175).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | map of string | yes | The dataset's description per language: `dct:description`. |
| `keywords` | map of list of string | no | Keywords per language, in place of `spec.catalog.keywords`: `dcat:keyword`. |
| `license` | [Licence](#licence) | no | The licence from the EU licence table, in place of `spec.catalog.license`: `dct:license`. |
| `name` | string | yes | The CKAN dataset name, unique in the catalogue: 2 to 100 characters of `a-z`, `0-9`, `-` and `_`, such as `bbsk-skoly`. |
| `q` | string | no | An NGSI-LD query its rows match (CIM 009 clause 4.9), such as `schoolType=="gymnazium"`. |
| `title` | map of string | yes | The dataset's title per language: `dct:title`. |
| `types` | list of string | yes | The entity types the dataset holds, each one the Endpoint serves, such as `[School]`. |

## Frequency

An update frequency of the EU frequency table.

Type: one of `"CONT"`, `"HOURLY"`, `"DAILY"`, `"WEEKLY"`, `"MONTHLY"`, `"QUARTERLY"`, `"ANNUAL"`, `"IRREG"`, `"NEVER"`, `"UNKNOWN"`

## Licence

A licence of the EU licence table whose duties are known (EP-79).

Type: one of `"CC0"`, `"CC_BY_4_0"`, `"CC_BYSA_4_0"`, `"ODC_BY"`, `"ODC_ODBL"`, `"ODC_PDDL"`

## PersonalData

The publisher's statement about personal data in a dataset (API/08 section 5).

Type: one of `"contains"`, `"does-not-contain"`

## Publisher

The publisher of a dataset, a `foaf:Agent`.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | map of string | yes | The publisher's name per language: `foaf:name`. |
| `uri` | string | no | The publisher's own IRI, its web site or registry entry. |

## Source

One original dataset the data comes from, itself a `dcat:Dataset`.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | map of string | yes | Its description per language; DCAT-AP requires one of every dataset it names. |
| `title` | map of string | yes | Its title per language; DCAT-AP requires one of every dataset it names. |
| `url` | string | yes | Its IRI, the page or API of the original. |

## Temporal

The period a dataset covers; either end may be open.

| Field | Type | Required | Description |
|---|---|---|---|
| `end` | string (date) | no | The last day covered: `dcat:endDate`. |
| `start` | string (date) | no | The first day covered: `dcat:startDate`. |

## DataTheme

A theme of the EU data-theme table.

Type: one of `"AGRI"`, `"ECON"`, `"EDUC"`, `"ENER"`, `"ENVI"`, `"GOVE"`, `"HEAL"`, `"INTR"`, `"JUST"`, `"REGI"`, `"SOCI"`, `"TECH"`, `"TRAN"`

## Visualization

One chart on the dataset (EP-130).

| Field | Type | Required | Description |
|---|---|---|---|
| `data` | [VisualizationData](#visualizationdata) | yes | Which of the Endpoint's own sources fills the spec's dataset `source`. |
| `description` | map of string | yes | The view's description per language. |
| `name` | string | yes | Unique on the Endpoint, a DNS label: the CKAN view is found again by it. |
| `spec` | object | yes | The Vega-Lite 5 or Vega 5 specification; it reads `{"data": {"name": "source"}}`. |
| `title` | map of string | yes | The view's title per language. |

## CkanPublication

Publication of one Endpoint as one CKAN dataset (EP-62…EP-65).

| Field | Type | Required | Description |
|---|---|---|---|
| `instanceRef` | [Ref](#ref) | yes | The `CkanInstance` this Endpoint publishes to. |
| `license` | string | no | The CKAN licence id the organization chose, such as `cc-by` for CC-BY 4.0. Used when the Endpoint's DCAT-AP record names no `dct:license`, which is the one term the record does not carry (EP-62, EP-63). |
| `name` | string | no | The CKAN dataset name; the Endpoint's own name when absent. |
| `organization` | string | no | The CKAN organization slug; the instance's default when absent. |

## FeedOutput

One output of `spec.representations.feed`.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | map of string | no | The feed's description per language. |
| `filter` | [OutputFilter](#outputfilter) | no | What narrows the view for this output; it never widens it. |
| `lang` | string | no | The feed's language when the request names none. |
| `limit` | integer (uint64) | no | The default page size, 1 to 200 (50 when left out). |
| `name` | string | yes | A slug `[a-z0-9-]`, 1 to 63, unique on the Endpoint: the URL segment after `feed/v1/`. |
| `query` | string | no | The preset's query string over the alias allow-list (API/02 §4a); it replaces `filter`. |
| `sort` | [FeedSort](#feedsort) | no | The order: `updated`, newest first, the only one. |
| `sources` | list of [FeedSource](#feedsource) | yes | The model classes the feed lists, each with its roles. |
| `title` | map of string | no | The feed's title per language (`sk`, `en`). |
| `version` | integer (uint64) | no | The contract version, 1 when left out. |

## IcalOutput

One named calendar of `spec.representations.ical` (API/11 EP-159), served at `/ical/v1/{name}.ics`.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | map of string | no | The calendar's description per language tag. |
| `filter` | [OutputFilter](#outputfilter) | no | A query that narrows the view for this calendar, never widens it (deprecated: `query`). |
| `lang` | string | no | The language tag (BCP 47) a LanguageProperty is read in when a field names none. |
| `limit` | integer (uint32) | no | At most this many Events, 1 to 10000. |
| `name` | string | yes | The URL slug: lowercase letters, digits and `-`, 1 to 40, unique on the Endpoint. |
| `query` | string | no | The preset's query string over the alias allow-list (API/02 §4a), such as `q=eventCategory=="koncert"&georel=near;maxDistance==5000&geometry=Point&coordinates=[19.15,48.73]`; it narrows the view like `filter`, which it replaces. |
| `sort` | string | no | `start` or `-start`: the order the first `limit` Events are kept in. |
| `sources` | list of [IcalMapping](#icalmapping) | yes | The model classes the calendar maps, each with its preset or mapping; 1 to 10. |
| `timezone` | string | no | The IANA zone a time without offset is read in; default `Europe/Bratislava`. |
| `title` | map of string | no | The calendar's title per language tag (`NAME`, `X-WR-CALNAME`). |
| `version` | integer (uint64) | no | The contract version (API/11 EP-152); `1` when absent. |

## WidgetOutput

One named output of `spec.representations.widget`.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | map of string | no | The output's description per language tag. |
| `name` | string | yes | A slug: lowercase letters, digits and `-`, 1 to 40, unique on the Endpoint. |
| `sources` | list of [WidgetSource](#widgetsource) | yes | The model classes it sets defaults for; 1 to 10, each class once. |
| `title` | map of string | no | The output's title per language tag. |

## Subject

One human or group a binding names; exactly one of `user`, `group` (PF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `group` | string | no | A group: a `Group` manifest of the organization unless `source` says otherwise (PF-64). |
| `source` | [SubjectSource](#subjectsource) | no | Who owns `group`'s members: absent for a `Group` manifest, `provider` for a group the identity provider owns, which the reconciler never manages. Only a `RoleBinding` subject carries it, and only with `group` (PF-63, PF-64). |
| `user` | string | no | The person's Keycloak user id, the `sub` of their token: a lower-case UUID, never an e-mail or a username (PF-126); or a client's own `service-account-<client>` user. |

## VisualizationData

The Endpoint source a visualization reads, never a URL (ADR-N-053 §2.2).

Type: object or object or object or object

## OutputFilter

An output's NGSI-LD query on top of the view (TS 104 175 6.4.3.2): `q` and `scopeQ` are AND-ed with a caller's, and a geo-query here cannot be replaced by one.

| Field | Type | Required | Description |
|---|---|---|---|
| `coordinates` | string | no | `coordinates`. |
| `geometry` | string | no | `geometry`. |
| `geoproperty` | string | no | `geoproperty`. |
| `georel` | string | no | `georel`; with `geometry` and `coordinates`. |
| `q` | string | no | `q`, such as `eventCategory=="šport"`. |
| `scopeQ` | string | no | `scopeQ`. |

## FeedSort

The order of a feed's entries.

Type: `"updated"`

## FeedSource

One model class of an output.

| Field | Type | Required | Description |
|---|---|---|---|
| `mapping` | map of [FeedPaths](#feedpaths) | no | Roles on top of the preset, or the whole mapping without one. |
| `preset` | [FeedPreset](#feedpreset) | no | The starting roles. |
| `type` | string | yes | The NGSI-LD type. |

## IcalMapping

One source of an [`IcalOutput`] (`spec.representations.ical[].sources[]`), and the deprecated single mapping object.

| Field | Type | Required | Description |
|---|---|---|---|
| `mapping` | map of [FieldSource](#fieldsource) | no | Fields that replace the preset's, or the whole mapping when no preset is named. |
| `preset` | [Preset](#preset) | no | The preset the mapping starts from; with no `mapping` either, `sdm-event`. |
| `type` | string | no | The NGSI-LD type the feed maps. |
| `version` | integer (uint64) | no | The contract version (API/11 EP-152); `1` when absent, the one version the broker reads. |

## WidgetSource

One class of a [`WidgetOutput`] and its map defaults.

| Field | Type | Required | Description |
|---|---|---|---|
| `cluster` | boolean | no | Whether markers cluster; absent: the widget's own rule (above 200 in view, EP-161). |
| `mapping` | [WidgetRoles](#widgetroles) | no | The roles: which attribute the map reads for each. |
| `table` | [WidgetTable](#widgettable) | no | Where the table starts; absent: the model's `view_*` annotations (EP-164). |
| `type` | string | yes | The model class, by its local name (`Event`). |

## SubjectSource

Who owns the members of a subject's group (PF-64).

Type: `"provider"`

## FeedPaths

One attribute path, or a list of them: the first with a value wins.

Type: string or list of string

## FeedPreset

A named starting mapping (API/02 §8b, the presets table).

Type: one of `"event"`, `"article"`, `"generic"`

## FieldSource

A field's source: an attribute name, or an object with one source and its remaps.

Type: string or [FieldRemap](#fieldremap)

## Preset

A named starting mapping.

Type: one of `"sdm-event"`, `"schema-org"`

## WidgetRoles

The map roles of a class; an absent role keeps the widget's default (EP-161).

| Field | Type | Required | Description |
|---|---|---|---|
| `colour` | string | no | The enum or boolean attribute the markers are coloured and the legend is drawn by. |
| `geometry` | string | no | The GeoProperty drawn (`location`, `operationSpace` or a model GeoProperty). |
| `popup` | list of string | no | The attributes the popup shows, in order; 1 to 10. |

## WidgetTable

The table defaults of a class (EP-164): what the reader sees first and can remove.

| Field | Type | Required | Description |
|---|---|---|---|
| `columns` | list of string | no | The columns shown, in order; 1 to 30, each once. |
| `filter` | string | no | The default filter, an NGSI-LD `q` whose dates may be `today`, `today+7d`, `today-1d`. |
| `orderBy` | string | no | The default order: one attribute, `;desc` for descending (`startDate`, `name;desc`). |
| `pageLength` | integer (uint16) | no | Rows per page: 10, 20, 50 or 100. |

## FieldRemap

The object form of a [`FieldSource`] (API/02 §8b): exactly one of `attribute`, `firstOf`, `join` and `constant`, and the remaps the field allows.

| Field | Type | Required | Description |
|---|---|---|---|
| `attribute` | string | no | An attribute, or a dotted path into its value (`address.addressLocality`). |
| `constant` | string | no | A fixed value. |
| `date` | [DateMode](#datemode) | no | How a date field reads its value. |
| `default` | string | no | The calendar value of a source value `map` does not name. |
| `endInclusive` | boolean | no | The source names the last day, so DTEND is the day after. |
| `firstOf` | list of string | no | The first of these attributes that has a value. |
| `join` | list of string | no | The values of these attributes that have one, joined by `separator`. |
| `language` | string | no | Which text of a LanguageProperty: a two-letter code or `first` (the default). |
| `map` | map of string | no | Source value to calendar value. |
| `separator` | string | no | The `join` separator; default `", "`. |
| `timezone` | string | no | The IANA zone of a floating time; default `Europe/Bratislava`. |
| `truncate` | integer (uint32) | no | At most this many characters. |

## DateMode

How a date field reads its value.

Type: one of `"auto"`, `"date"`, `"datetime"`
