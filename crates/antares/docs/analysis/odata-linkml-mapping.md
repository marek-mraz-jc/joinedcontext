# OData v4.01 and LinkML surfaces: CSDL from LinkML, discovery and policy, `$filter` to `q`, writes and relationships (T-3441)

Sources: Antares-NGSI-LD-Context-Broker `7185e1a` (branch `agent/worker-2/T-3436`; `cg` graph built at `7185e1a100`), joinedcontext-platform `59a8f70` (`cg` graph at `59a8f705b9`), platform docs `4238cf0`, joinedcontext-portal `bd3a68a8` (grep only). ETSI TS 104 175 V1.1.1 and TS 104 176 V1.1.1 (2026-09), one file per clause under `/workspace/etsi-specs/`. OASIS OData Version 4.01 (OASIS Standard): Part 1 Protocol, Part 2 URL Conventions, JSON Format, CSDL XML and CSDL JSON, fetched on 2026-10-10 from `docs.oasis-open.org/odata/...` (`.../odata/v4.01/os/part1-protocol/odata-v4.01-os-part1-protocol.html`, `.../part2-url-conventions/odata-v4.01-os-part2-url-conventions.html`, `.../odata-json-format/v4.01/os/odata-json-format-v4.01-os.html`, `.../odata-csdl-xml/v4.01/os/odata-csdl-xml-v4.01-os.html`, `.../odata-csdl-json/v4.01/os/odata-csdl-json-v4.01-os.html`). OData vocabularies from `github.com/oasis-tcs/odata-vocabularies` at commit `3e36ad2a4930a47feaf7a7cd0b78ed4f2bafcfb2` (the commit `odata-library.md` section 4 pins): `vocabularies/Org.OData.Core.V1.xml`, `Org.OData.Capabilities.V1.xml`, `Org.OData.Measures.V1.xml`, `Org.OData.Validation.V1.xml`. crates.io API and `static.crates.io` tarballs read on 2026-10-10 (grep only, nothing built). Inputs on this branch: `docs/analysis/operation-matrix.yaml` and `.md` (T-3437, the contract this page refines; its row ids are reused), `docs/analysis/cim009-to-ts104175.md`, `docs/analysis/odata-library.md` (T-3448 survey, read and not edited), `docs/analysis/gateway-to-broker-map.md` (T-3436). Nothing was built or run.

**What is built (T-5206, 2026-10-22).** The rows below record the design; where the build
differs, this list is authoritative and `docs/src/odata.md` describes what is served:

- N1 (b) discovery and (c) sampling are not built: the model comes from (a) alone; a stored
  instance of another kind reads `null` with `@Antares.NgsiLd.Unserved`.
- P2 is `Edm.Untyped` (4.01) or a dynamic property of an open type (4.0), never a `ComplexType`;
  an inlined object written through OData stays a Property.
- P3 is `Edm.Geography` only; the concrete subtypes are not built.
- P4 is served in `ngsild.lang` or as the `@none` text, `$filter` compares the same text; the full
  map is not served.
- R3: DM-05 does list ListRelationship (`KINDS` in `linkml-model`); the section 12 note is outdated.
- R4 is an `Edm.String` URL, not a navigation to `NgsiLd.Entity`.
- S1 and S2 are served as `<prop>@Antares.NgsiLd.observedAt` and `<prop>@Antares.NgsiLd.unitCode`
  under `Prefer: odata.include-annotations`; there is no `Measures` reference.
- S3 serves only the instance without `datasetId` (none: `null`); `_datasets` is not built, and a
  `PUT` of an Entity holding a named instance is `409`.
- 3.3: a name over 128 characters is dropped without a metric.
- 3.4: a class range without `inlined: true` stays a Relationship (every class is an Entity type
  keyed by the NGSI-LD `id`); LinkML's implied `inlined` is not applied.
- E2a keys `urn:ngsi-ld:{Class}:{v}` only, not the platform's `{space}:` form.
- C19 states `Minimal`; the measured level is in `docs/src/odata-compliance.md`. C21
  (`Core.SchemaVersion`) is not built (T-5207).
- F20 and F22 over `scope` or a complex path answer `400`, not `501`.

## 1. Scope, reading guide and how decisions are recorded

This page decides how the OData v4.01 surface (`/x/views/{view}/odata/v4`, operation-matrix column `odata`) and the LinkML schema surface (`/x/views/{view}/linkml`, column `linkml`) map onto TS 104 175 operations and the ADR-0020 policy seam. It has the seven numbered decisions of T-3441 (sections 3 to 9), a Security section (10), the list of operation-matrix rows this page changes (11) and the unknowns (12).

Conventions:

- **Row ids.** A row that maps to an operation names the operation-matrix row id (`create_entity`, `query_entities`, ...). OData features with no NGSI-LD operation reuse the matrix ids of section 7.1 (`odata_search`, `odata_actions`, ...). New ids for forms the matrix did not list start with `odata_` and are marked *new*.
- **Status.** `mapped`: the surface translates the request into the named operation and runs it through the same handler, so the same `gate!` fires (operation-matrix section 2.5). `mapped, gated`: mapped, but refused with 501 until the named precondition (a defect fix or a seam change) has landed. `rejected`: refused before any gate or store access with the stated status and an OData error object (JSON Format 21.1: "The OData error object MUST contain name/value pairs with the names code and message"). The `code` strings are this surface's own, as the matrix proposes (`NotImplemented`); OData leaves the value service-defined.
- **501.** Protocol 9.3.1: "If the client requests functionality not implemented by the OData Service, the service responds with 501 Not Implemented and SHOULD include a response body describing the functionality not implemented." URL Conventions 5.1: "If a data service does not support a system query option, it MUST reject any request that contains the unsupported option." No refused option is ignored.
- **Views (ADR-0023).** Both surfaces are addressed by their view, as `docs/adr/ADR-0023-endpoint-views.md` rule 10 decides: OData service root `/x/views/{view}/odata/v4`, LinkML `/x/views/{view}/linkml`. A view is configuration: `selector` (entity types), `q`, `scope_q`, `geo_q`, an `attributes` allowlist and an `operations` allowlist of operation-matrix ids (ADR-0023, Decision). What a caller may see on either surface is `effective = view ∧ engine`: the view's predicate and allowlists first, then whatever the ADR-0020 engine answers; "both narrowing, neither widening". Every OData and LinkML answer below is computed from that intersection; "hidden" means outside it.
- **Status codes shared with the NGSI-LD view** (ADR-0023 rules 3, 4, 7), translated into OData error objects:

| Situation | NGSI-LD view (ADR-0023) | OData surface | OData error `code` |
|---|---|---|---|
| Entity outside the view, or absent | 404 ResourceNotFound (rule 3) | 404 | `NotFound` |
| create of an id that exists outside the view | 409 AlreadyExists (rule 3, TS 104 175 10.2.2.4) | 409 | `AlreadyExists` |
| operation not in the view's `operations` allowlist, and no allowed operation shares the method and path | 405 with `Allow` (rule 4, TS 104 176 6.3.2) | 405 with `Allow` | `MethodNotAllowed` |
| method and path also serve an allowed operation | 422 OperationNotSupported (rule 4) | 422 | `OperationNotSupported` |
| write naming an Attribute outside the allowlist, a create the view would not select, a post-image leaving the view | 403 AccessDenied (rule 7) | 403 | `AccessDenied` |
| the engine refuses (ADR-0020 `Deny`) | 403 `urn:antares:error:AccessDenied` | 403 | `AccessDenied` |
| an OData feature with no NGSI-LD operation behind it | (no NGSI-LD form) | 501 (Protocol 9.3.1) | `NotImplemented` |
| a name outside the model (hidden or misspelled) in a URL or query option | a hidden Attribute does not exist while `q` is evaluated (rule 2) | 400: OData types are closed (not `OpenType`), so an undeclared name is a request error rather than an absent value; hidden and misspelled names give the identical answer | `BadRequest` |

- **Decisions.** The owner's rule of 2026-10-10 (`tasks/GOAL-antares-surfaces.md`, How to work) is "No questions to the owner". Each decision is written as **Decided (worker-2, owner may override): choice, because reason; options: ...**.

## 2. The owner's design notes, verified

| # | Owner note (T-3441) | Verdict | Evidence |
|---|---|---|---|
| N1 | `$metadata` is built from three sources intersected with the view: (a) authoritative schema, (b) discovery, (c) sampling to `Edm.Untyped` | holds, with the order and limits of section 3.1 | sections 3.1, 7 |
| N2 | (a) LinkML from the platform DataModel (T-2357) gives datatypes, enums, required, relationship targets, subClassOf | holds for the LinkML **source**, not for the generated JSON Schema: the JSON Schema artefact drops the relationship target | DataModel kind `crates/jc-core/src/kinds/data_model.rs:449-478` (`DataModelSpec.linkml`, `classes`, `artifacts`, `open_world`); DM-64 "the source slot on class A with `range: B` and the inverse slot on B with `range: A`" (`docs/Requirements/data-models.md:120`); the generated JSON Schema renders a Relationship as `"type": ["string","null"]` with only `"x-ngsi-ld-kind": "Relationship"` (`examples/datamodels/projects/helsinki/spaces/air-quality/datamodels/json-schema/air-quality-observed.v1.json:72-79`); `is_a: Entity` in the source (`.../air-quality-observed.linkml.yaml:26`) |
| N3 | Kinds come from the LinkML annotation | holds: `ngsi_ld_kind` is the platform's carrier | DM-05 "The NGSI-LD kind of a slot (Property, GeoProperty, Relationship, LanguageProperty, ListProperty, JsonProperty, VocabProperty) MUST be declared with the LinkML annotation `ngsi_ld_kind`" (`docs/Requirements/data-models.md:18`); DM-05 lists seven kinds and omits ListRelationship (5.2.6.4.9), see section 12 |
| N4 | (b) `GET /types` then `GET /types/{type}` gives attributeDetails with attributeTypes | holds | 10.6.5.4 returns EntityTypeInfo "as mandated by clause 5.2.6.10.4"; Table 5.2.6.10.4-1 `attributeDetails`: "Attribute with only the elements "id", "type", "attributeName" and "attributeTypes"" (`ts-104175-ngsi-ld-core-api/5/5.2.6.10.4.md`); broker: `crates/antares-api/src/types_attrs.rs:290-307` |
| N5 | `/types?details=true` gives names only | holds | Table 5.2.6.10.3-1 EntityType: `attributeNames` "List containing the names of attributes that instances of the entity type can have" (`5/5.2.6.10.3.md`); broker `types_attrs.rs:226-235` |
| N6 | `/attributes?details=true` aggregates kinds across types | **does not hold**: it gives `typeNames`, not kinds | 10.6.7.4: "details of available attributes as mandated by clause 5.2.6.10.1 (restricted to the elements id, type, attributeName and typeNames)" (`10/10.6.7.4.md`). The kinds across types come from 10.6.8 (`GET /attributes/{attrId}`), whose Attribute carries `attributeTypes` "List of attribute types (e.g. Property, Relationship, GeoProperty) for which entity instances exist" (Table 5.2.6.10.1-1). The broker follows the spec: `types_attrs.rs:337-350` emits `typeNames` only, `:408-414` emits `attributeTypes` on 10.6.8 |
| N7 | Discovery never gives value datatypes or relationship targets | holds for discovery; instance data can carry both | No member of 5.2.6.10.1 to 5.2.6.10.5 carries a value type or a target. Per instance, a Relationship may carry `objectType` "Node Type of the Relationship's target object" (Table 5.2.6.4.4-1) and a Property `valueType` "The native JSON-LD @type for the Property Value" (Table 5.2.6.4.3-1); sampling (c) may read both |
| N8 | (c) sampling fallback gives `Edm.Untyped` | holds, OData 4.01 only | CSDL XML 3.4: `Edm.Untyped` "cannot be returned in a payload with an OData-Version header of 4.0. Services should treat untyped properties as dynamic properties in 4.0 payloads"; CSDL XML conformance: services "MUST NOT include Edm.Untyped" for `OData-MaxVersion:4.0` |
| N9 | Public endpoints require (a) | holds; adopted as a rule (section 3.1) | sampling reads instance data, so its output depends on what exists; a public schema must not change with the data (security item S4) |
| N10 | All properties Nullable | holds and is the CSDL default | CSDL XML 7.2.1: "If no value is specified for a single-valued property, the Nullable attribute defaults to true. In OData 4.01 responses a collection-valued property MUST specify a value for the Nullable attribute." Protocol 11.4.9.2: "A missing dynamic property is defined to be the same as a dynamic property with value null." This matches NGSI-LD, where an absent Attribute is the only "null" |
| N11 | Sub-attributes as instance annotations, never nested values, so `$filter=speed gt 80` works | holds | JSON Format 20.2: annotations of a primitive "MUST be represented as a single name/value pair and placed immediately prior to the annotated name/value pair. The name is the same as the name of the property ..., followed by the "at" sign (@), followed by the annotation identifier"; 20: "The namespace or alias MUST be defined in the metadata document" |
| N12 | `Measures.Unit` for unitCode | partly: `Measures.UNECEUnit` is the term for an NGSI-LD `unitCode` | `Org.OData.Measures.V1.xml:84` `UNECEUnit`: "according to the UN/CEFACT Recommendation 20"; `:80` `Unit`: "e.g. cm for centimeters or % for percentages". NGSI-LD `unitCode` "As mandated by [10]" (Table 5.2.6.4.3-1); the platform's model carries a UCUM code and UN/CEFACT `exact_mappings` (`air-quality-observed.linkml.yaml` slot `pm25`: `ucum_code: ug/m3`, `unece:GQ`) |
| N13 | `createdAt`/`modifiedAt` `Core.Computed`, ETag via `modifiedAt` | Computed holds; ETag deferred | `Core.Computed` "A value for this property is generated on both insert and update" (`Org.OData.Core.V1.xml:385`); 5.2.6.2 "Those members are read-only, and shall be automatically generated". The ETag needs an atomic conditional write the broker does not have (section 7.4) |
| N14 | Multi-target Relationship as abstract base plus type cast | holds in CSDL 4.01 | CSDL XML 6.2 Abstract Entity Type; 13.4.1: "OData 4.01 services MAY have a type-cast segment as the last path segment, allowing to bind instances of different sub-types to different targets" |
| N15 | NavigationPropertyBinding only when the target type is visible and traversable | holds, and omission is valid CSDL | CSDL XML 13.4: "If omitted, clients MUST assume that the target entity set or singleton can vary per related entity" |
| N16 | Capabilities terms named in T-3441 (Insert/Update/Delete/Filter/Sort/ExpandRestrictions, NonUpdatableProperties) | all exist; `NonUpdatableProperties` is a member of `UpdateRestrictions`, not a term | `Org.OData.Capabilities.V1.xml`: terms `InsertRestrictions` (:646), `UpdateRestrictions` (:738), `DeleteRestrictions` (:837), `FilterRestrictions` (:446), `SortRestrictions` (:523), `ExpandRestrictions` (:549); `UpdateRestrictionsType/NonUpdatableProperties` |
| N17 | `$filter` subset to the broker's own `q` AST | holds: `antares_ql::QNode` exists and is what the seam conjoins on | `crates/antares-ql/src/lib.rs:62` `QNode {And, Or, Cmp, Exists}`; `crates/antares-api/src/policy.rs:241-262` `narrow_params` conjoins "on the AST and rendered back once" |
| N18 | `$expand` to `join`/`joinLevel` capped by ExpandRestrictions | holds, gated on a defect fix (section 5, S1) | TS 104 176 `7/7.2.3.1.md`: `join` "The type of Linked Entity retrieval to apply ... Allowed values: "flat", "inline", "@none"", `joinLevel` "Depth of Linked Entity retrieval"; broker cap `MAX_JOIN_LEVEL = 10` (`crates/antares-api/src/bounds.rs:109`); `ExpandRestrictionsBase/MaxLevels` "A value of -1 indicates there is no restriction" (`Org.OData.Capabilities.V1.xml:565`) |

Two spec defects found while verifying (reported, not acted on): 10.6.2 calls the Attribute structure "clause 5.2.6.10.2" (it is 5.2.6.10.1; 5.2.6.10.2 is AttributeList) and lists "10.6.6, 10.6.6"; Table 5.2.6.4.9-1 gives the ListRelationship `type` as "It shall be equal to "Relationship"". The CIM 009 7.x example in 7.2.3.2 (EXAMPLE 7, `((speed>50|rpm>3000);...)`) nests parentheses that the ABNF `QueryTermAssoc` above it does not produce; the broker's parser accepts nesting up to `MAX_Q_DEPTH = 64` (`crates/antares-ql/src/lib.rs:244`), so the OData translator may emit nested groups.

## 3. Decision 1: NGSI-LD to CSDL mapping

### 3.1 Where each fact comes from, and in which order

| Fact | (a) authoritative schema | (b) discovery (TS 104 175 10.6) | (c) sampling | Used when |
|---|---|---|---|---|
| entity types (EntityType, EntitySet) | LinkML `classes` (`DataModelSpec.classes`, `data_model.rs:463`) | `GET /types` typeList (10.6.3, Table 5.2.6.10.5-1) | no | (a); (b) only for an open-world view without (a) |
| attribute names per type | LinkML class slots, `is_a` resolved | `GET /types?details=true` attributeNames (10.6.4) | no | (a); (b) adds names only where the class is `open_world: true` (DM-28, `data_model.rs:475`) |
| attribute kind | `annotations.ngsi_ld_kind` (DM-05) | `GET /types/{type}` attributeDetails[].attributeTypes (10.6.5) | yes: the member that carries the value (`value`, `object`, `languageMap`, `vocab`, `valueList`, `objectList`, `json`) | (a), else (b); a name with more than one kind in (b) becomes `Edm.Untyped` |
| value datatype | LinkML `range` (section 3.4) | never (N7) | JSON type of sampled values, instance `valueType` | (a); else `Edm.Untyped` (c); sampled JSON types are never promoted to a narrower Edm type, because one later entity breaks it |
| relationship target type | LinkML `range: <class>` and `inverse` (DM-64) | never (N7) | instance `objectType` (Table 5.2.6.4.4-1) | (a); else the abstract base `NgsiLd.Entity` (section 3.2, row R) |
| required | LinkML `required` | no | no | only as `Capabilities.InsertRestrictions/RequiredProperties`, which a create through the surface must name (C5); never `Nullable="false"` (N10) |
| enum | LinkML enum / `permissible_values` | no | no | `Validation.AllowedValues` on an `Edm.String` (section 3.4) |
| unit | LinkML `unit` (`ucum_code`, `exact_mappings` `unece:`) | no | instance `unitCode` | metadata annotation `Measures.UNECEUnit` from the `unece:` mapping, `Measures.Unit` from `ucum_code`; instance annotation for the stored `unitCode` |
| subClassOf | LinkML `is_a` | no | no | `BaseType` only when both classes are visible; otherwise the slots are flattened into the visible class |

**Decided (worker-2, owner may override):** (a) is authoritative and is the only source for public views (N9), because only (a) carries value types and relationship targets (N7) and does not change with the data; (b) is the view's own discovery, which ADR-0023 rule 5 computes from the Entities the view selects with the allowlist applied (clauses 5.7.5 to 5.7.10 join `policy::FILTERABLE`), and is used only to add attribute names to a class marked `open_world: true` and kinds that (a) leaves unset; (c) is used only for non-public views, only through a Query Entities on the view (clause 5.7.2, FILTERABLE) with a fixed sample size, and only to decide the kind, never the value type. Until ADR-0023 rule 5 is implemented (discovery is not FILTERABLE at `7185e1a`, `crates/antares-api/src/policy.rs:68`), (b) is not used at all. Options: (b) always, before rule 5 lands (rejected: unnarrowed discovery lists types and attributes outside the view, S4); (c) for public views (rejected: N9).

The broker cannot read the LinkML source today: nothing in Antares parses LinkML or YAML (no `serde_yaml`, `yaml-rust2`, `serde-saphyr` or `jsonschema` in `Cargo.lock`), and the platform hands the gateway only the JSON Schema and the `@context` (`joinedcontext-platform crates/context-gateway/src/resolver.rs:187-200`, `Model { json_schema, context }`). The JSON Schema is not enough: it loses the relationship target (N2). **Decided:** the schema reaches the broker as the LinkML schema in its JSON serialisation, resolved (imports and `is_a` folded), produced by Model Tools beside the DM-02 artefacts and loaded per tenant as a read-only document; the broker never parses YAML (section 9). Whether Model Tools' `gen-linkml` can write that form is unknown (section 12).

### 3.2 Construct mapping

Namespace: one CSDL schema per view, `Namespace` = the view id mangled (section 3.3), plus one vocabulary schema `Antares.NgsiLd` (alias `NgsiLd`) that defines the surface's own terms (`Iri`, `Kind`, `observedAt`, `datasetId`) and the abstract base `NgsiLd.Entity`. JSON Format 20 requires every instance-annotation namespace to be defined in the metadata document, so the vocabulary is included in every `$metadata`.

| Id | NGSI-LD (TS 104 175) | CSDL 4.01 | Notes and limits |
|---|---|---|---|
| E1 | Entity type (5.2.3) | `EntityType Name=<mangled short name>` with `BaseType="NgsiLd.Entity"`; one `EntitySet` per visible type in the `EntityContainer` | short name = compaction under the view's `@context` (section 7.2) |
| E2 | Entity `id` (Table 5.2.6.4.1-1, "Valid URI") | `Key` inherited from `NgsiLd.Entity`: `PropertyRef id`, `Property id Type="Edm.String" Nullable="false"`, `Core.ComputedDefaultValue` (a create may leave it out, W1; T-3721) | `NgsiLd.Entity` is `Abstract="true"` and defines the key; CSDL XML 6.2: "An abstract entity type MUST NOT inherit from a non-abstract entity type". Key predicate `Set('urn:ngsi-ld:...')`; key-as-segment (URL 4.3.6) not offered |
| E2a | a LinkML `identifier: true` slot of the class, the view showing it (T-3720) | the EntityType has no `BaseType`; it declares the base members of `NgsiLd.Entity` itself and `Key PropertyRef <slot>`, `Nullable="false"`; the key's type is the slot's (`Edm.String`, `Edm.Int64`), `Edm.Guid` when the slot's `edm_type` annotation says so; an `Edm.Guid` or `Edm.String` key is `Core.ComputedDefaultValue` (W1, T-3721), an `Edm.Int64` one is not | key value `v` names the Entity `urn:ngsi-ld:{Class}:{v}`, `{Class}` the LinkML class name, so `Categories(<a product's guid>)` names a Categories URN and is 404; `Set(id='urn:…')` still names any Entity of the set, and is the canonical key of one whose URN does not follow the pattern; the slot stays an Attribute of its own, read and written like any other. One identifier per class and its ancestors, single-valued, `string`/`integer`/`uri`/`uriorcurie`, kind Property, or the schema does not load |
| E3 | Entity `type` (String or String[]) | no structural property; `@odata.type` names the EntityType of the set addressed; `_types` `Collection(Edm.String) Nullable="false"`, `Core.Computed`, lists only the visible types | names starting with `_` cannot collide: NGSI-LD names start with a letter (5.2.2.3 `name = unicodeLetter *nameChar`) |
| E4 | multi-typed Entity | appears in every visible EntitySet of its types; its canonical URL (`@odata.id`) is in the set of its first visible type in IRI order; under a set it is served with that set's EntityType's properties | an Attribute declared on another of its types and not on this one is absent under this set (not dynamic, so `$filter` on it is a 400 like any undeclared name) |
| E5 | `scope` (5.2.7) | `scope Collection(Edm.String) Nullable="false"`, `NgsiLd.Kind='scope'` | read and filtered only as `any(s: s eq '...')` mapped to `scopeQ` (section 5.2, row F28) |
| E6 | `createdAt`, `modifiedAt` (Table 5.2.6.2-1) | `Edm.DateTimeOffset`, `Core.Computed`, and listed in `InsertRestrictions/NonInsertableProperties` and `UpdateRestrictions/NonUpdatableProperties` | the broker emits them only on request (5.2.6.2 "only generate common members ... when the Context Consumer explicitly asks"), so the surface asks with `options=sysAttrs` when they are selected; `deletedAt` and `expiresAt` not exposed |
| P1 | Property (5.2.6.4.3) with a scalar `value` | `Property Type=<Edm type of section 3.4>` | value only; sub-attributes per rows S1 to S4 |
| P2 | Property with a JSON object `value` | LinkML `range: <class>` with `inlined: true`: `ComplexType` (not an EntityType); otherwise `Edm.Untyped` | a complex path `address/city` maps to `q` `address[city]` (section 5.2, row F20) |
| P3 | GeoProperty (5.2.6.4.5) | `Edm.GeographyPoint`, `...LineString`, `...Polygon`, `...MultiPoint`, `...MultiLineString`, `...MultiPolygon` (SRID 4326) when the LinkML range names one geometry; `Edm.Geography` otherwise | 5.2.2.4 excludes GeometryCollection; the JSON Format writes geography as GeoJSON, the NGSI-LD value as stored |
| P4 | LanguageProperty (5.2.6.4.6) | `Edm.String` with `Core.IsLanguageDependent`; the value is the `languageMap` entry the NGSI-LD `lang` parameter (7.2.7) selects from `Accept-Language` | the full map is not exposed in round 1; `Core.IsLanguageDependent` "Properties and terms annotated with this term are language-dependent" (`Org.OData.Core.V1.xml:307`) |
| P5 | VocabProperty (5.2.6.4.7) | `Edm.String`, `NgsiLd.Kind='VocabProperty'`; `Validation.AllowedValues` when the range is an enum | the value is served compacted under the view's `@context` |
| P6 | ListProperty (5.2.6.4.8) | `Collection(<item Edm type>) Nullable="true"` with `Core.Ordered` | Nullable stated explicitly (CSDL XML 7.2.1, 4.01 responses) |
| P7 | JsonProperty (5.2.6.4.10) | `Edm.Untyped` | "Raw unexpandable JSON" (Table 5.2.6.4.10-1); never filterable (`FilterRestrictions/NonFilterableProperties`) |
| R1 | Relationship (5.2.6.4.4), single `object`, LinkML `range: B`, B visible | `NavigationProperty Type="<ns>.B" Nullable="true"`, `Partner` = the LinkML `inverse` when the inverse slot is visible too; `NavigationPropertyBinding Path=<nav> Target=<set of B>` | the NGSI-LD Relationship is one-directional; `Partner` is advisory and every write still writes one side only (section 6) |
| R2 | Relationship, LinkML `multivalued: true` (object String[]) | `NavigationProperty Type="Collection(<ns>.B)"` | `$ref` add and remove on it are refused in round 1 (section 6, rows W18, W19) |
| R3 | ListRelationship (5.2.6.4.9) | as R2 with `Core.Ordered` on the navigation property | DM-05 has no kind name for it (section 12) |
| R4 | Relationship whose target type is unknown or has more than one class | `NavigationProperty Type="NgsiLd.Entity"`; one `NavigationPropertyBinding` per visible target type with a type-cast last segment (`<nav>/<ns>.B`, CSDL XML 13.4.1, 4.01) | path type-cast segments in requests stay refused (`odata_derived_types`); a payload names the concrete type by `@odata.type` |
| R5 | Relationship whose target type is not visible, or which the view makes non-traversable | `Property Type="Edm.String"` with `Core.IsURL` holding the `object` URI, no NavigationProperty and no binding | the object URI is what NGSI-LD itself serves for a visible Relationship; the target's existence and attributes are not revealed |
| R6 | a single-valued Relationship slot (R1) with the annotation `odata_referential_constraint: <slot>`, `<slot>` a single-valued Property of the same class, not `required`, of the Edm type of the target class's key (E2a, or `id`, `Edm.String`, without one; T-3761) | `ReferentialConstraint Property=<slot> ReferencedProperty=<target key>` inside the NavigationProperty (CSDL XML 8.5) | a create, a replace or a merge (W1, W4, W6) and a property write (W10, W11) that gives `<slot>` a value read the Entity that value keys in the target's set under the view first: absent, hidden or of another type is the same 400 `BadRequest`, target `<slot>`, nothing written (Protocol 11.4.2 SHOULD; as a bind, W2). `null` is no reference and is not read. A constraint the view does not show whole (the slot, the target type or its key hidden, a type that differs, a required slot) is left out of `$metadata` and checked nowhere, since the check could not read the target. The broker keeps the Relationship and the Property as two Attributes: a write of one does not write the other |
| S1 | sub-Property `observedAt` (Table 5.2.6.4.3-1) | instance annotation `<prop>@NgsiLd.observedAt` (`Edm.DateTimeOffset`) | returned only when `Prefer: odata.include-annotations` asks for it (Protocol 8.2.8.4) |
| S2 | sub-Property `unitCode` | instance annotation `<prop>@Measures.UNECEUnit`; static metadata annotation `Measures.UNECEUnit`/`Measures.Unit` from (a) | the `Org.OData.Measures.V1` reference is included in `$metadata` |
| S3 | `datasetId`, multi-instance Attributes (Table 5.2.6.4.1-1 NOTE) | the instance without `datasetId` is the property value; every instance is listed in `_datasets Collection(NgsiLd.DatasetInstance)` (`attribute`, `datasetId`, `value Edm.Untyped`, `observedAt`), `Core.Computed` | `_datasets` is read-only and not filterable in round 1 |
| S4 | any other sub-Attribute (Properties of Properties) | not exposed in round 1 | listed as a gap (section 12) |
| S5 | `expiresAt`, `ngsildproof`, output-only members (Tables 5.2.6.4.x-2) | not exposed | `ngsildproof` belongs to TS 104 179; exposing it is a later decision |

**Decided (worker-2, owner may override):** row R5 instead of hiding the Relationship, because the NGSI-LD view of the same caller shows it (the policy narrows by attribute name, `policy.rs:165-179`, not by target), so hiding it in OData only would make the two surfaces disagree; options: hide the attribute (fewer facts, but two surfaces with different answers), expose a NavigationProperty without binding (a client would try to follow it and meet 404s).

**Decided (worker-2, owner may override; T-3761):** row R6 takes an explicit annotation rather than reading every class-ranged slot as a constraint, because a Relationship's `object` already is the reference (W2 checks it) and only a plain Property beside it, such as the reference model's `ProductID`, needs saying which navigation it keys; options: derive the constraint from a slot named `<nav>ID` (a naming rule nothing in LinkML states), or have a write of the Property write the Relationship too (one Attribute changing another, which no NGSI-LD operation does).

### 3.3 Names

CSDL XML 15.2: a simple identifier "consists of at least one and at most 128 Unicode characters"; "The first character MUST be the underscore character (U+005F) or any character in the Unicode category "Letter (L)" or "Letter number (Nl)""; "The remaining characters MUST be the underscore character (U+005F) or any character in the Unicode category "Letter (L)", "Letter number (Nl)", "Decimal number (Nd)", "Non-spacing mark (Mn)", "Combining spacing mark (Mc)", "Connector punctuation (Pc)", and "Other, format (Cf)"". NGSI-LD names (5.2.2.3) are `unicodeLetter *(unicodeNumber / unicodeLetter / "_")` with an optional `prefix:` and `unicodeNumber` is the whole Number category (`\p{N}`, which includes No, for example `²`).

**Decided (worker-2, owner may override):** a name that is already a simple identifier is kept; every other code point (the `:` of `prefix:name`, a No or other non-allowed character) is written `_xHHHH_` (uppercase hex of the code point, at least four digits), and a literal `_x` followed by hex digits and `_` in a source name is itself escaped, so the mapping is injective and reversible; a result longer than 128 characters is left out of the model (it cannot be addressed through OData, and the NGSI-LD surface still serves it) and counted in a metric. The NGSI-LD IRI of every EntityType and Property is the annotation `NgsiLd.Iri` (a term of the surface's vocabulary: Core has no IRI term, `Org.OData.Core.V1.xml` term list read in full). Names beginning with `_` are reserved for the surface (rows E3, S3). Options: a hash-based name (not readable), refusing the whole model (one odd attribute would take the surface down).

### 3.4 Value types

| LinkML range (DM-05 table `docs/Architecture/11-data-models.md:183-192`) | NGSI-LD value (5.2.2.4) | Edm type | Annotation |
|---|---|---|---|
| `string` | JSON string | `Edm.String` | |
| `integer` | JSON number | `Edm.Int64` | |
| `float`, `double` | JSON number | `Edm.Double` | |
| `decimal` | JSON number | `Edm.Decimal` | `Scale="variable"` |
| `boolean` | JSON true/false | `Edm.Boolean` | |
| `datetime` | DateTime `YYYY-MM-DDThh:mm:ss[.ffffff]Z` | `Edm.DateTimeOffset` | written with `Z`; a request literal with an offset is converted to UTC before it becomes a `q` value |
| `date` | Date `YYYY-MM-DD` | `Edm.Date` | |
| `time` | Time `hh:mm:ss[.ffffff]Z` | `Edm.TimeOfDay` | the trailing `Z` is dropped on output and added on input |
| `uri`, `uriorcurie` | URI | `Edm.String` | `Core.IsURL` |
| an enum | string from a closed list | `Edm.String` | `Validation.AllowedValues`; not `EnumType`, because enum member names must be simple identifiers and NGSI-LD values such as `pm2.5` are not |
| `GeoJSONGeometry` | GeoJSON geometry | row P3 | |
| a class, `inlined: true` | JSON object | `ComplexType` | row P2 |
| a class, `inlined: false` | Relationship | rows R1 to R5 | |
| none, or unknown range | any | `Edm.Untyped` | 4.01 only (N8) |

LinkML has one `integer`, one `string` and no binary, duration or GUID type, so a slot may narrow its Edm type with the annotation `edm_type` (T-3720, T-3722): `Edm.Byte`, `Edm.SByte`, `Edm.Int16` or `Edm.Int32` on `integer`; `Edm.Single` on `float` or `double`; `Edm.Guid`, `Edm.Duration` or `Edm.Binary` on `string`. Any other pairing, or another value, fails the load. The NGSI-LD value stays the range's JSON value; a write checks it against the narrower type (an `Edm.Int16` of 40000 is 400), and a `$filter` literal compares as the range's value.

Core annotations from the schema (T-3722): a slot's `description` is `Core.Description` on its property and a class's `description` on its EntityType. The slot annotation `odata_computed: "true"` writes `Core.Computed`, and the property joins `NonInsertableProperties` and `NonUpdatableProperties` (rows C3, C9), so a body naming it is 400 as for the base members; `odata_immutable: "true"` writes `Core.Immutable` and adds it to `NonUpdatableProperties` only. Values other than `"true"` and `"false"` fail the load. The slot annotation `odata_referential_constraint` names the Property a Relationship's target is keyed by (row R6, T-3761); one the view cannot show whole is left out rather than refused, because a view's narrowed schema may hide either half.

For an `OData-MaxVersion: 4.0` request the `$metadata` declares no `Edm.Untyped` property and marks the EntityType `OpenType="true"`; the payloads, `$select` and `$filter` serve each such member as a dynamic property, which is what CSDL XML 3.4 says services should do (T-5206, T-5333); CSDL JSON is 4.01 only (`odata-library.md` section 3.2), so a 4.0 request for `$metadata?$format=json` is answered 406 (unknown: OData names no status for it; HTTP 406 is the general rule).

```cypher
// the policy Filter members and who reads them (row R5 rationale, security S3); Antares clone
MATCH (c:Symbol)-[r:REFERENCES]->(f:Symbol)
WHERE f.path IN ['antares_api::policy::Filter::omit','antares_api::policy::Filter::pick','antares_api::policy::Filter::q'] AND c.is_test = false
RETURN f.name, c.path, c.file, r.line ORDER BY f.name, c.file, r.line;
// the discovery folds behind sources (b), with their tests
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path IN ['antares_api::types_attrs::type_stats','antares_api::types_attrs::attr_stats']
RETURN f.name, c.path, c.file, r.line ORDER BY c.file, r.line;
```

## 4. Decision 2: policy to Capabilities annotations

The view half of `effective = view ∧ engine` is read directly from the view: `selector` gives the EntitySets, `attributes` the properties, `operations` the write capabilities (`create_entity` absent means `Insertable=false` and `POST /{Set}` 405, ADR-0023 rule 4). The engine half has to be asked. The ADR-0020 engine answers one `Operation` at a time (`policy.rs:97-110`, `Decision {Allow, Deny, Filter}` at `:150`); it has no "describe the view" call. The surface therefore learns what to annotate by **probing**: for each visible type T it asks `decide` with a synthetic `Operation` per clause, `types: [T]`, no ids, under the caller's `Subject`. A probe result only shapes the description. **Every real request is still gated on its own** (operation-matrix section 2.5), so an annotation that is wrong in the permissive direction cannot grant anything; one that is wrong in the restrictive direction hides a capability the request would have had. The probes run once per cache key (section 7.3), bounded by the number of visible types times the clauses below.

| Id | CSDL target | Term and member | Value from | Rule |
|---|---|---|---|---|
| C1 | EntityContainer | (presence of an EntitySet) | T in the view `selector`, and probe 5.7.2 (`query_entities`) with `types: [T]` | `Deny` hides T: no EntityType, no EntitySet, no binding to it, and every name of T is unresolvable (S4). `Filter` keeps T and records the Filter for C10 to C12 |
| C2 | EntitySet T | `Capabilities.InsertRestrictions/Insertable` | view `operations` contains `create_entity`, and probe 5.6.1 | `false` on `Deny` or `Filter` (5.6.1 is not FILTERABLE, a Filter answer is a deny, `policy.rs:59-68`) |
| C3 | EntitySet T | `InsertRestrictions/NonInsertableProperties` | (a) plus the Filter of C1 | `createdAt`, `modifiedAt`, `_types`, `_datasets`; a property outside the allowlist or omitted by the C1 Filter is not in the model at all |
| C4 | EntitySet T | `InsertRestrictions/NonInsertableNavigationProperties` | C1 of the target type | every navigation property whose target type is hidden (a bind would write a Relationship to an invisible Entity) |
| C5 | EntitySet T | `InsertRestrictions/RequiredProperties` | (a) `required`, the slots the view shows | enforced on a create through the surface (T-3721): a member it lists, left out or `null`, is 400 `MissingProperty` before any operation (the term: "These properties must be specified in the payload of insert requests"); an update may leave it out. NGSI-LD writes outside the surface are not checked, as before; the platform's write guard did (`gateway-to-broker-map.md` S12) |
| C6 | EntitySet T | `UpdateRestrictions/Updatable` | view `operations` contains `merge_entity` or `replace_entity`, and probe 5.6.17 | `false` on `Deny`/`Filter`. The update handlers fill neither `types` nor `attrs` (operation-matrix 5.1), so an engine sees the same `Operation` for every type at request time: the probe is advisory until that gap is closed |
| C7 | EntitySet T | `UpdateRestrictions/Upsertable` | constant | `false` (section 6, row W7) |
| C8 | EntitySet T | `UpdateRestrictions/DeltaUpdateSupported` | constant | `false` (`odata_delta_update`) |
| C9 | EntitySet T | `UpdateRestrictions/NonUpdatableProperties`, `NonUpdatableNavigationProperties` | as C3, C4 | |
| C10 | EntitySet T | `DeleteRestrictions/Deletable` | view `operations` contains `delete_entity`, and probe 5.6.6 | `false` on `Deny`/`Filter` |
| C10a | EntitySet T | `Insertable`, `Updatable`, `Deletable` of C2, C6, C10 (T-3722) | the class annotations `odata_insertable`, `odata_updatable`, `odata_deletable` with the value `"false"` | the class can only narrow what the view offers, never widen it; a create, an update (of the Entity, a property or a reference) or a delete the set refuses is 405 before its body is read, as for an operation the view lacks |
| C11 | EntitySet T | `FilterRestrictions/Filterable` `true`, `MaxLevels`, `NonFilterableProperties` | (a) and constants | `MaxLevels` = 1 in round 1 (no navigation path in `$filter`, section 5.2 row F23); `NonFilterableProperties`: every JsonProperty (P7) and member of an unknown kind, every collection, GeoProperty, LanguageProperty and VocabProperty, `_datasets`, `_types`, `scope`; an `Edm.Untyped` Property (`range: Any`) is filterable (F34); an inlined class of the view (P2, a complex value) is not; every R5 property is filterable as a string |
| C12 | EntitySet T | `SortRestrictions/Sortable` `true`, `NonSortableProperties` | (a) | every `Edm.Untyped`, collection, LanguageProperty and navigation property, `_types` and `scope`; `createdAt` and `modifiedAt` are sorted by the surface (T-3758) |
| C13 | EntitySet T | `ExpandRestrictions/Expandable`, `MaxLevels`, `NonExpandableProperties` | constants and C1 of targets | `Expandable=false` until the join visibility fix (section 5.1 row Q3); then `MaxLevels = min(MAX_JOIN_LEVEL, view cap)` (`bounds.rs:109`, 10); `NonExpandableProperties`: navigations to hidden or non-traversable targets (none exist as navigations after R5, so the list is empty by construction) |
| C14 | EntitySet T | `CountRestrictions/Countable` | constant | `true` (count is of the narrowed set, S2) |
| C15 | EntitySet T | `Capabilities.SearchRestrictions/Searchable` | constant | `true` (`odata_search`, T-3750) |
| C16 | EntitySet T | `Capabilities.DeepInsertSupport/Supported`, `DeepUpdateSupport/Supported` | constant | `false` (rows W3, W8) |
| C17 | EntitySet T | `Capabilities.NavigationRestrictions/RestrictedProperties` | C1 of targets | empty: C1 already turns every navigation to a hidden target into an R5 property |
| C18 | EntityContainer | `Capabilities.BatchSupported` `true`, `BatchSupport/ContinueOnErrorSupported`, `SupportedFormats` | constants | formats `multipart/mixed`, `application/json`; change sets refused (section 6) |
| C19 | EntityContainer | `Capabilities.ConformanceLevel` | constant | `Minimal` (unknown whether the subset meets Intermediate; Protocol 13.2 not checked row by row) |
| C20 | EntityContainer | `Capabilities.CrossJoinSupported`, `ComputeSupported`, `KeyAsSegmentSupported`, `QuerySegmentSupported`, `AsynchronousRequestsSupported` | constants | `false`, `true` (`odata_compute`, T-3936), `false`, `true` (`/$query`, section 5.1 row Q22), `false`; `Aggregation.ApplySupported` with the `Transformations` row Q19 serves, without `Rollup`: its `RollupType` enum is a member of a vocabulary the clients do not load, and Microsoft.OData.Edm refuses the whole document over it (T-3937, T-3985) |
| C21 | schema | `Core.SchemaVersion` | (a) | the DataModel `version` (`data_model.rs:458`) plus the view fingerprint, so `$schemaversion` can be checked (row Q12) |
| C22 | EntityType property | `Core.Computed`, `Core.IsURL`, `Core.IsLanguageDependent`, `Core.Ordered`, `Validation.AllowedValues`, `Measures.UNECEUnit`, `Measures.Unit`, `NgsiLd.Iri`, `NgsiLd.Kind` | (a) | section 3 |

`NavigationPropertyBinding` (N15): written for navigation property N of set S only when C1 keeps both S and the target type and the view lets the caller follow N (the target's own probe 5.7.1 is not `Deny`). Otherwise the Relationship becomes an R5 string property.

**Decided (worker-2, owner may override):** the view half from the view configuration, the engine half by probing through `decide`, because the view is data the surface can read exactly (ADR-0023) while the engine is opaque by ADR-0020 and probing needs no seam change; options for the engine half: a new optional `PolicyEngine::describe(&Subject) -> View` member (an ADR-0020 change that would also give the cache key its fingerprint, section 7.3; left to T-3443), or annotating the view half only (clients would offer writes the engine then refuses with 403).

## 5. Decision 3: query options and read URL forms

Every request is first parsed by `odata-url` and resolved against the per-view `EdmModel` (section 7). Resolution fails for a name the model does not hold, and the model holds no hidden name (section 3, C1), so a hidden type, attribute or navigation is answered exactly as a misspelled one: `400 Bad Request`, error code `BadRequest`, a message that names the option and never echoes a literal (security S3). Only then is the request translated, as an `antares_ql::QNode` (`crates/antares-ql/src/lib.rs:62`), a `GeoQuery` (`crates/antares-ql/src/geo.rs:73`) and NGSI-LD parameters, into the operation the row names; nothing is concatenated as a string. Where the operation runs as an in-process request (`AppState::call`, `crates/antares-api/src/state.rs:571`), the AST is rendered once by `impl Display for QNode` (`crates/antares-ql/src/render.rs:12`), whose contract is that `parse_q(&node.to_string())` yields the node again; a pattern literal containing `\"` cannot be rendered (`render.rs` module doc) and is refused 501.

Option names are matched as Protocol 11.2.1 and URL 5.1 require: "4.01 Services MUST support case-insensitive system query option names specified with or without the $ prefix", and "The same system query option, irrespective of casing or whether or not it is prefixed with a $, MUST NOT be specified more than once" (a duplicate is 400). Evaluation order is Protocol 11.2.1's ("$schemaversion ... $apply ... $compute $search $filter $count $orderby $skip $top ... After applying any server-driven paging: $expand $select $format", quoted in `odata-library.md` 3.6).

### 5.1 System query options and read forms

The list of system query options is the ABNF `systemQueryOption` rule (`$compute`, `$deltatoken`, `$expand`, `$filter`, `$format`, `$id`, `$count`, `$orderby`, `$schemaversion`, `$search`, `$select`, `$skip`, `$skiptoken`, `$top`, `$index`, and `$levels` inside an expand item; `odata-library.md` 3.6, `odata-abnf` at `a31c8db`), plus `$apply` from the Data Aggregation extension, plus the expression literals `$root`, `$it`, `$this` (URL 5.1.1.14.4 to 5.1.1.14.6).

| Id | Option or form | OData 4.01 | Status | Operation-matrix row | NGSI-LD translation | Refusal (status, OData error code) |
|---|---|---|---|---|---|---|
| Q1 | `$filter` | URL 5.1.2, Protocol 11.2.6.1 | mapped (subset, 5.2) | `query_entities` | `q` (AST), `geoQ`, `id` | outside the subset: 501 `NotImplemented` |
| Q2 | `$select` | URL 5.1.4 | mapped | `query_entities`, `retrieve_entity` | `pick` of the selected attributes (frame `id`, `type` always kept, `policy.rs:184`); `createdAt`/`modifiedAt` add `options=sysAttrs`; `*` is no `pick`; a complex sub-path (`address/city`) 501 | unknown name 400 `BadRequest` |
| Q3 | `$expand=<nav>` | URL 5.1.3 | mapped, gated: 501 until T-3470 (join targets checked against the narrowing, ADR-0023 rule 6) is done (security S1) | `query_entities`, `retrieve_entity` | `join=inline`, `joinLevel` = depth of the expand tree; expansions the request did not name are dropped from the answer (NGSI-LD 7.7.1 follows every retrievable Relationship) | before the fix: 501 `NotImplemented` |
| Q4 | `$levels` (expand item option) | URL 5.1.3 | mapped, gated as Q3 | as Q3 | `joinLevel`; `max` = `ExpandRestrictions/MaxLevels` (C13) | above MaxLevels: 400 `BadRequest` (NGSI-LD answers `joinLevel` over 10 with 400 BadRequestData, `bounds.rs:109`) |
| Q5 | expand options `$filter`, `$orderby`, `$top`, `$skip`, `$count`, `$search`, `$compute` inside `$expand(...)` (*new* `odata_expand_options`) | URL 5.1.3 | rejected | none | NGSI-LD Linked Entity retrieval has no per-relationship filter, order or page (7.7) | 501 `NotImplemented` |
| Q6 | `$select` and `$expand` inside `$expand(...)` | URL 5.1.3 | mapped, gated as Q3 | as Q3 | nested `$select` becomes `pick=<rel>{a,b}` (7.3.2 `LinkedEntityTerm`); nested `$expand` adds one `joinLevel` | as Q3 |
| Q7 | `$expand=<nav>/$ref`, `$expand=*/$ref` | URL 5.1.3 | mapped | `retrieve_entity`, `query_entities` | no join: the Relationship's `object` (or `objectList`) is written as `{"@id": ...}` | none |
| Q8 | `$expand=*` | URL 5.1.3 | mapped, gated as Q3 | as Q3 | every visible navigation | as Q3 |
| Q9 | `$orderby` | URL 5.1.5 | mapped; `createdAt`/`modifiedAt` sorted by the surface (T-3758) | `query_entities` | `orderBy=<attr>[;desc]`, `id`, `type` (TS 104 176 `7/7.2.3.1.md`: "Each String is an Entity member ("id", "type", "scope" or an Attribute name)"), complex sub-path as `attr[a.b]` (7.6.2.3 EXAMPLE 4); under a residual (5.2) the pushed `orderBy` ends in `id` (ADR-0028 rule 4). A key `orderBy` cannot name (`createdAt`, `modifiedAt`: no Entity member of 7.2.3.1) is sorted by the surface over the whole answer, read with `options=sysAttrs` when it holds at most `RESIDUAL_SCAN` (10 000) Entities, nulls first ascending (URL 5.1.5), ties in `id` order (ADR-0028 rule 5). The store orders a missing Attribute after every value, so before a pushed `orderBy` the read counts the Entities lacking a key, `q=(<q>);(!k1\|!k2…)`: none, and the pushed order is URL 5.1.5's; any, and the read is that surface sort (T-3853) | a scalar of the target of a single-valued navigation (`Category/Name`, T-5906): sorted by the surface over the answer read with `join=inline`, the target's value as the join inlined it (a target the view hides reads as absent, null first ascending); a collection-valued navigation path, function, `geo.distance`: 501; a NonSortable property (C12): 400; a surface sort over more than 10 000 Entities: 400 `TooManyResults` |
| Q10 | `$top` | URL 5.1.6 | mapped | `query_entities` | `limit` = min(`$top`, `Prefer: odata.maxpagesize`, `max_limit`); the rest of `$top` travels in the next link | negative or not an integer: 400 |
| Q11 | `$skip` | URL 5.1.6 | mapped | `query_entities` | `offset` (checked add with the page offset) | as Q10 |
| Q12 | `$count=true` | URL 5.1.7 | mapped | `query_entities` | `count=true`; `NGSILD-Results-Count` becomes `@odata.count` (JSON Format 4.5.4); under a residual (5.2) the count of the whole answer, read when the pushdown's count is at most 10 000 (ADR-0028 rule 5, T-3758) | a residual count over more than 10 000 Entities: 400 `TooManyResults` |
| Q13 | `/$count` path segment | URL 4.8, Protocol 11.2.10 | mapped | `query_entities` | `limit=0&count=true`, `text/plain` body; allows `$filter` (URL 5.1); under a residual the whole answer counted as Q12; `<nav>/$count` and `$expand=<nav>/$count` 501 | as Q12 |
| Q14 | `$search` | URL 5.1.7 | mapped | `odata_search`, `query_entities` | words, phrases, `NOT`, `AND` (or whitespace) and `OR` with 5.1.7's precedence; a term matches an Entity when one of the single-valued `Edm.String` properties the view shows contains it, case-insensitively; it joins `$filter` as one more conjunct, which the residual evaluates on the answer the view and the engine admit (ADR-0028), so a hidden property or Entity never matches; `searchExpr-incomplete` (`'...'`) is one phrase; T-3750 | on one Entity or below it: 501; a search that is no `searchExpr`: 400; past the expression bounds: 400 |
| Q15 | `$format` | URL 5.1.9, Protocol 11.2.11 | mapped | all reads | `json`, `application/json` with `metadata=minimal\|full\|none`; `xml` and `json` on `$metadata` | any other media type: 406 (HTTP, see section 12) |
| Q16 | `$compute` | URL 5.1.10 | mapped | `odata_compute`, the read's `query_entities` or `retrieve_entity` | `commonExpr as name`, comma-separated, before every other option (Protocol 11.2.1); each item a residual value (ADR-0028) over the scalar members, the functions and the arithmetic a residual `$filter` takes, evaluated per Entity on the answer the view and the engine admit, so a hidden Attribute reads as absent and the value is `null`; written as a dynamic property, always without `$select` and when `$select` names it or `*`; the name is inlined into `$filter` and sorts as a surface `$orderby` key (Q9); a name the type has is 400; T-3936 | inside `$expand` (Q5) and below one Entity: 501; a list value or `null`: 501; past the expression bounds: 400 |
| Q17 | `$index` | URL 5.1.11 | rejected | `odata_index` | positional insert, no NGSI-LD operation; `Core.PositionalInsert` never annotated | 501 |
| Q18 | `$schemaversion` | URL 5.1.12, Protocol 11.2.12 | mapped for `*` and the current `Core.SchemaVersion` (C21); rejected otherwise | `odata_schemaversion` (refined, section 11) | no-op when it names the current version | any other value: 501 (the broker keeps no older view; Protocol 11.2.12 "the service MUST process the request according to the specified version", which it cannot) |
| Q19 | `$apply` | Data Aggregation 4.0 section 3 | mapped | `odata_apply`, `query_entities` | `identity`, `filter`, `search`, `orderby`, `top`, `skip`, `compute`, `aggregate` (`sum`, `min`, `max`, `average`, `countdistinct`, `$count as`), `groupby((p, ...)[, aggregate(...)])` by single-valued primitive properties, `topcount`, `bottomcount`, `topsum`, `bottomsum`, `toppercent`, `bottompercent` (the instances with a value, ranked highest or lowest first and kept in that order; a sum or percentage the fewest that reach it) and `concat` (the sequences' results one after the other, all keeping the Entities or all aggregating; rows under the union of their names, `null` where a sequence has none), separated by `/` and applied in order to the whole answer of one bounded scan (ADR-0028, `RESIDUAL_SCAN`); each expression a residual value per Entity on the answer the view and the engine admit, so a hidden Entity is never counted and a hidden Attribute reads as absent; `sum`, `average`, `min`, `max` fold the non-null values, `null` without one; after an aggregation the rows hold only the grouping properties and aliases, the context URL is `$metadata#{Set}(names)` and a later transformation, `$top`, `$skip` and `$count` read the rows; a `compute` name is written with the Entities, but not past a `concat`; `Aggregation.ApplySupported` lists the transformations (C20); T-3937, T-3985 | past the scan bound: 400 `TooManyResults`, never a partial aggregate; `expand`, `nest`, `addnested`, a `concat` mixing Entities and rows, `search` after an aggregation, the hierarchy and join transformations, `rollup`, a navigation path, custom aggregates and methods: 501 (T-3985); beside `$filter`, `$orderby`, `$select`, `$expand`, `$search` or `$compute`, on `/$count`, `/$ref` and one Entity: 501; `sum` or `average` of a non-number, `topsum` to `bottompercent` of a non-number, a first parameter of `topcount` to `bottompercent` that is no number or negative, an unknown or hidden name: 400 |
| Q20 | `$skiptoken` | Protocol 11.2.6.7 | mapped | `query_entities` | the surface writes it into `@odata.nextLink`; it encodes a cursor, the `$skip` still to apply, the remaining `$top` and a digest of the query it was issued for; nothing else. The cursor is the NGSI-LD offset; under a residual the upstream offset just past the last Entity served (ADR-0028 rule 4), for a counted or surface-sorted answer the position in it (T-3758) | a token that does not decode, or whose digest does not match the request: 400. Protocol 11.2.6.7: "Its content is opaque, service-specific" |
| Q21 | `$deltatoken` (*new* `odata_deltatoken`) | Protocol 11.3 | rejected | none | no change tracking (`Capabilities.ChangeTracking` not annotated); a `track-changes` preference is not applied | 501 |
| Q22 | `POST <resource>/$query` (*new* `odata_query_segment`) | URL 4.17 | mapped | `query_entities` (GET form) | the `text/plain` body is parsed as the query string; executed as `GET /entities`, never as `entityOperations/query`, whose body members do not reach the engine (operation-matrix 5.1, `query_entities_post`) | body over `max_query_len`: 414-equivalent 400 |
| Q23 | `$id` in `$entity?$id=` | URL 4.3.4, Protocol 11.2.9 | mapped | `retrieve_entity` | the entity-id must be a URL of this service; the key is extracted and the entity read, a foreign URL never fetched | a foreign or unknown id: 404 as a hidden Entity (O5); a missing id, a broken escape or an id with query options of its own: 400 (T-3738) |
| Q24 | `$root` (*new* `odata_root`) | URL 5.1.1.14.5 | rejected | none | a second read inside a filter, outside any gate | 501 |
| Q25 | `$it`, `$this` (*new* `odata_it_this`) | URL 5.1.1.14.4, 5.1.1.14.6 | rejected | none | only meaningful in lambdas and nested filters, which are refused | 501 |
| Q26 | parameter aliases `@p=...` | URL 5.3, Protocol 11.2.6.1.3 | mapped for literal values | as the option using them | substituted before translation | alias bound to an expression: 501 |
| Q27 | custom query options (no `$`, no `@`) (*new* `odata_custom_option`) | URL 5.2 (custom query options) | mapped for `ngsild.q`, `ngsild.scopeQ`, `ngsild.georel`, `ngsild.geometry`, `ngsild.coordinates`, `ngsild.geoproperty`, `ngsild.pick`, `ngsild.omit` (ADR-0028 rule 6, T-3758); rejected otherwise | `query_entities` | the NGSI-LD parameter of the same name in the pushdown, its names expanded under the view's `@context` and written as the request's terms: `ngsild.q` joins `q` with `;`, `ngsild.scopeQ` is `scopeQ`, `ngsild.geo*` takes the one `geoQ` (an F29 conjunct then goes to the residual), `ngsild.pick`/`ngsild.omit` narrow the selected members after selection (TS 104 175 4.21: projection does not change which Entities match). Collection reads only | any other name, a name twice, a `ngsild.` option on another resource, or a `ngsild.q` that does not parse: 400 `BadRequest`; a bad value of the others is the broker's 400 |
| Q28 | `/$filter(...)` path segment | URL 4.12 | rejected (*new* `odata_filter_segment`) | none | | 501 |
| Q29 | `GET /{Set}` | Protocol 11.2.6 | mapped | `query_entities` | `type=<T IRI>` plus Q1 to Q13 | |
| Q30 | `GET /{Set}('{id}')` | Protocol 11.2.2 | mapped | `retrieve_entity` | `type` is checked against the set; an entity of another type is 404 | hidden or absent: 404 (ADR-0020) |
| Q31 | `GET /{Set}('{id}')/{prop}`, `.../{prop}/$value` | URL 4.6, 4.7 | mapped | `retrieve_attribute_value` | `GET /entities/{id}/attrs/{attr}` | hidden or absent attribute: 404, the answer ADR-0023 rule 3 gives the NGSI-LD view and the one OData gives a path that does not resolve |
| Q32 | `GET /{Set}('{id}')/{nav}` (related entities) | Protocol 11.2.7 | mapped | `retrieve_entity` (source, `pick=<nav>`), then `retrieve_entity` or `query_entities` (`id=` list) on the targets | each target read is its own gated operation, so no join is needed and Q3's gate does not apply | a hidden target is absent from the answer (collection) or 404 (single) |
| Q33 | `GET /{Set}('{id}')/{nav}/$ref` | Protocol 11.2.8 | mapped | `retrieve_entity` | `pick=<nav>`; the object URIs as entity references | |
| Q33a | `GET /{Set}/$ref` | URL 4.4, Protocol 11.2.8, JSON Format 14 | mapped (T-3730) | `query_entities` | the page of Q3 with `$filter`, `$orderby`, `$top`, `$skip`, `$count` and `$skiptoken` as on `/{Set}`; each returned Entity written as its reference `{Set}(key)` | `$select` or `$expand`: 400 (URL 4.4: a reference has no properties); a hidden Entity is absent |
| Q33b | `GET /{Set}('{id}')/$ref` | URL 4.4, Protocol 11.2.8, JSON Format 14 | mapped (T-3730) | `retrieve_entity` | the Entity read as Q30, written as its reference | hidden or absent: 404; `$select` or `$expand`: 400; another option: 501 |
| Q34 | `GET /` (service document) | Protocol 11.1.1 | mapped | `retrieve_entity_types` | the visible EntitySets (C1) | |
| Q35 | `GET /$metadata` | Protocol 11.1.2 | mapped | `retrieve_entity_type_details` | section 7 | |
| Q36 | `$crossjoin`, `$all` | URL 4.15, 4.16 | rejected | `odata_crossjoin`, `odata_all` | | 501 |
| Q37 | type-cast path segment | URL 4.11 | rejected | `odata_derived_types` | | 501 |
| Q38 | key-as-segment `/{Set}/{id}` | URL 4.3.6 | not offered | none | the extra segment does not resolve | 404 |
| Q39 | member of an ordered collection `/{prop}/{n}` | URL 4.10, Protocol 11.2.6.8 | rejected (*new* `odata_ordinal`) | none | | 501 |

Request headers: `Prefer: odata.maxpagesize` is applied (capped by `max_limit`); `odata.include-annotations` selects the S1 to S3 instance annotations; `respond-async` is never applied (Protocol 11.6: "the service MAY process the request asynchronously", and "MUST NOT reply ... with 202 Accepted if the request has not included the respond-async preference"); `return=minimal|representation` is applied on writes; `odata.continue-on-error` on `$batch` (row W26).

### 5.2 The `$filter` subset and its `q` translation

Semantics that decide the translation:

- OData `ne` is true on an absent property: URL 5.1.1.1.2 "The null value is not equal to any value but itself". NGSI-LD `!=` needs the attribute: 7.2.3.3 "Unequal operator ... A matching entity shall contain the target element and meet none of the conditions". So `p ne v` becomes `Or[Exists{p, negated}, Cmp{p, Ne, v}]`.
- OData `gt`/`ge`/`lt`/`le`: URL 5.1.1.1.3 "If any operand is null, the operator returns false", which is NGSI-LD's "it shall contain the target element" (7.2.3.3). Direct.
- NGSI-LD `==` on an array value matches membership (7.2.3.4 condition 1, "Is included in the target value, and the latter is an array"); OData `eq` on a scalar property does not. The model types a ListProperty as a collection, so `eq` on it is a type error (400) and membership is spelled `any` (F22). A Property that the schema types as scalar but that holds an array breaks this; such data violates (a) and the answer follows NGSI-LD.
- `and` binds tighter than `or` in both languages (7.2.3.2 "the logical AND operator shall take precedence over the logical OR operator"); the AST keeps the grouping anyway.
- The filter is split by top-level conjunct (ADR-0028 rule 1, T-3758): a conjunct the rows below map is pushed as `q`, `id` or `geoQ`; any other the residual evaluator covers goes, whole, to the residual (`antares-residual`), evaluated by `ViewOps::scan` on what the view answered. The residual keeps OData's null rules on the evaluator's three-valued logic: a comparison with a missing value or one of another type is false, not unknown (URL 5.1.1.1.2, 5.1.1.1.3), so `not (p gt 5)` holds where `p` is absent; a Boolean member or a string function over a missing value stays null, and so does its `not`. Rows that say "residual" are served this way (T-3749 added arithmetic, the functions, two members compared and the lambdas); a type error in a residual row is 400 like a pushed one (URL 5.1.1). A read that needs the scan (a residual, a residual count, a surface sort) beside an `$expand` of entities or an F24 hop stays 501: the scan does not carry the join's truncation warning (S1), T-3823.

| Id | `$filter` construct | URL 5.1.1 | Status | `QNode` / NGSI-LD |
|---|---|---|---|---|
| F1 | `p eq v` | 5.1.1.1.1 | mapped | `Cmp{p, Eq, v}` |
| F2 | `p eq null` | 5.1.1.14.3 | mapped | `Exists{p, negated: true}` (7.2.3.2 EXAMPLE 6 `!isMonitoredBy`) |
| F3 | `p ne v` | 5.1.1.1.2 | mapped | `Or[Exists{p, negated: true}, Cmp{p, Ne, v}]` |
| F4 | `p ne null` | 5.1.1.1.2 | mapped | `Exists{p, negated: false}` |
| F5 | `gt`, `ge`, `lt`, `le` | 5.1.1.1.3 to 5.1.1.1.6 | mapped | `Cmp{p, Gt\|Ge\|Lt\|Le, v}` |
| F6 | `v op p` (literal on the left) | 5.1.1.1 | mapped | operator mirrored, then F1 to F5 |
| F7 | `and`, `or` | 5.1.1.1.7, 5.1.1.1.8 | mapped | `And`, `Or` |
| F8 | `( ... )` grouping | 5.1.1.3 | mapped | AST nesting (depth limit S2) |
| F9 | `not` over `contains`/`startswith`/`endswith` | 5.1.1.1.9 | mapped | `Cmp{p, NotPattern, re}` |
| F10 | `not (p eq null)` | 5.1.1.1.9 | mapped | F4 |
| F11 | any other `not` | 5.1.1.1.9 | residual (T-3758) | `q` has no general negation (only `!Attribute`, 7.2.3.2), and De Morgan does not hold under 7.2.3.3's type-mismatch rule ("shall be considered as not matching" for both `>` and `<=`), so the conjunct is evaluated exactly: `Not` over any row served here, compiled to the residual form; a `not` over a construct still 501 stays 501 |
| F12 | `p in (v1, v2, ...)` | 5.1.1.1.11 | mapped | `Cmp{p, Eq, List[...]}` (7.2.3.4 condition 2) |
| F13 | `has` | 5.1.1.1.10 | rejected 501 | no enum flags (section 3.4) |
| F14 | `add`, `sub`, `mul`, `div`, `divby`, `mod`, negation | 5.1.1.2 | residual (T-3749) | no arithmetic in `q`: `Call(Add\|Sub\|Mul\|Div\|DivBy\|Mod\|Neg)` over numbers, `div` truncating for two integers; an operand of another known type (a string, a Boolean, a date) is 400 (URL 5.1.1.2), a duration (`date sub date`, `dt add duration`) stays 501 (F27) |
| F15 | `contains(p,'s')`, `startswith(p,'s')`, `endswith(p,'s')` | 5.1.1.5.2, 5.1.1.5.3, 5.1.1.5.6 | mapped | `Cmp{p, Pattern, re}` with `re` = the literal regex-escaped, anchored `^...` or `...$` for starts/ends; both are case-sensitive and both need a string value (7.2.3.3 "If the target value data type is different than String then it shall be considered as not matching") |
| F16 | `matchesPattern` | 5.1.1.7.1 | residual (T-3749) | evaluated as the ECMAScript pattern URL 5.1.1.7.1 names, never translated to IEEE 1003.2 (7.2.3.2 `RegExp`): the dialects differ, so a pattern is never pushed; the pattern is a literal, its size bounded by the evaluator (ADR-0028 rule 7) |
| F17 | `length`, `indexof`, `substring`, `concat`, `tolower`, `toupper`, `trim`; `hassubset`, `hassubsequence` | 5.1.1.5 to 5.1.1.7 | residual (T-3749); `hassubset`, `hassubsequence` rejected 501 | `Call(Length\|IndexOf\|...)` in characters on `Edm.String` operands, an operand of another known type 400; a null operand yields null (URL 5.1.1.5); the two collection functions need a collection literal (F27) |
| F18 | date and time functions (`year` ... `totalseconds`, `now`, `maxdatetime`, `mindatetime`) | 5.1.1.8 | residual (T-3749); `totalseconds` rejected 501 | `Call(Year\|...)` on `Edm.Date`, `Edm.TimeOfDay` or `Edm.DateTimeOffset` as URL 5.1.1.8 lists each, in the value's own offset; `now()` is one instant per request; `totalseconds` takes a duration (F27) |
| F19 | `round`, `floor`, `ceiling`, `cast`, `isof`, `case` | 5.1.1.9, 5.1.1.10, 5.1.1.12 | residual (T-3749) for `round`, `floor`, `ceiling`; `cast`, `isof`, `case` rejected 501 | `Call(Round\|Floor\|Ceiling)`, `round` half away from zero; `cast` and `isof` name a type the residual has no conversion for, `case` returns a value of any type |
| F20 | complex path `p/a/b` | 5.1.1.15 | mapped, gated: 501 until the model builds row P2's `ComplexType` (the build types an inlined class `Edm.Untyped`, `edm.rs` P2, so no path below `p` names a member; T-3686) | `QPath{path: [p], bracket: [a, b]}` (7.2.3.2 "trailing path ... enclosed in a single pair of square brackets") |
| F21 | annotation value `p/@NgsiLd.observedAt` | 5.1.1.16 | mapped | `QPath{path: [p, observedAt]}` (7.2.3.2 EXAMPLE 9 `temperature.observedAt>=...`) |
| F22 | `p/any(x: x eq v)` or `x in (...)` over a primitive collection (ListProperty, `scope`) | 5.1.1.13.1 | mapped for a ListProperty; `scope` gated: 501 until the base members are filtered (F33's round; T-3686) | ListProperty: `Cmp{p, Eq, v\|List}` (membership, 7.2.3.4); `scope`: `scopeQ` (7.2.5), intersected like the seam's own (`policy.rs:257-264`) |
| F23 | `p/all(...)`, `any` with any other body | 5.1.1.13 | residual (T-3749) | `Any`/`All` over the ListProperty's `valueList`, the variable a `Var`; an absent list is the empty collection the surface shows (`any` false, `all` true); a variable's member stays 501. Over a collection-valued navigation (T-5906): `nav/any(d: body)` is the body's `q` with each `d/m` the linked-entity term `nav{T:m}` (4.9; nested `any` nests the terms, `join` as deep as the terms), which holds when one target the caller may read matches; `or` distributes over the targets, `nav/any()` is `nav` present; `and` inside the lambda (two terms may match two targets) and `all` stay 501 |
| F24 | single-valued navigation path `nav/attr op v` | 5.1.1.15 | mapped, gated: 501 until the linked-entity evaluation of `q` checks each hop's target against the view and the engine (a new task, section 11; T-3470 covers the join walk only) (security S1) | `QPath{links: [Link{attr: nav, types: [B]}], path: [attr]}` (7.2.3.2 EXAMPLE 15 `sensor{Device:humidity}==40`) |
| F25 | property against property | 5.1.1.1 | residual (T-3749) | `q` compares against literals only: `Cmp` of two values, numbers promoting among the numeric types, other known types of one type or 400, null on either side false except `ne` (as F3) |
| F26 | literals: string, integer, decimal, double, boolean, date, dateTimeOffset, timeOfDay | 5.1.1.14.1 | mapped | `QValue::Str`, `Num`, `Bool`; dateTimeOffset normalised to `...Z`, timeOfDay with `Z` appended (5.2.2.4); a decimal or Int64 outside f64's exact range: 400 (`QValue::Num(f64)`, `lib.rs:218-230`); a literal of another type than the member it is compared with or listed for (URL 5.1.1.1: numbers promote among the numeric types, an integer past its Edm type's range does not), 400 (T-3725) |
| F27 | literals: guid, duration, binary, enum, complex or collection JSON literal, geography outside F29 | 5.1.1.14 | duration and binary: residual (T-5905); the rest rejected 501 | `duration'..'` and `binary'..'` meet only an `Edm.Duration` or `Edm.Binary` member (another member type is 400) and compare by the value they spell; enum, complex, collection and geometry literals have no NGSI-LD value type (5.2.2.4) |
| F28 | `id eq 'u'`, `id in (...)` as a top-level conjunct | | mapped | the `id` parameter of Query Entities; anywhere else 501 |
| F29 | `geo.intersects(<GeographyPoint property>, geography'POLYGON(...)')` as a top-level conjunct | 5.1.1.11.2 | mapped | `geoQ` `georel=intersects`, `geometry=Polygon`, `geoproperty=<p>` (`geo.rs:33-58`); a second one, one below `or`/`not`, or one beside `ngsild.georel`: residual `Spatial(Intersects)` (T-3758) |
| F30 | `geo.distance`, `geo.length` | 5.1.1.11.1, 5.1.1.11.3 | rejected 501 | "the shortest distance ... in the coordinate reference system signified by the two points' SRIDs" (5.1.1.11.1): the unit for SRID 4326 is not stated as metres, NGSI-LD `maxDistance` is metres (`geo.rs` `Rel::Near`, "distances in metres") |
| F31 | LanguageProperty `p eq 's'` | | mapped | `QPath{path: [p], bracket: [<lang>]}` with the response language, `[*]` when none (7.2.3.4 conditions 4 and 5) |
| F32 | VocabProperty `p eq 's'` | | mapped | `Cmp` plus `expandValues=<p>` (7.2.3.2 EXAMPLE 13) |
| F33 | `createdAt`, `modifiedAt`, `_types`, `_datasets` in `$filter` | | rejected 501 | whether the broker's `q` evaluates system members is unknown (section 12) |
| F34 | `Edm.Untyped` Property operand (`range: Any`: a KPI's `currentValue` is a number or `not measured`) | URL 5.1.1.1 | mapped (T-5333) | the `q` term of F1 to F6 with the literal as JSON; a value of another JSON type matches `ne` only, as NGSI-LD 4.9 compares, and the residual the same (a comparison across types is unknown); a date, GUID, duration or geography literal: 400; a JsonProperty (P7), an inlined class of the view (P2, a complex value) or a member of an unknown kind: 400, NonFilterable (C11) |

```cypher
// every production caller of the q parser and of the evaluator (where a translated AST lands); Antares clone
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol) WHERE f.path IN ['antares_ql::parse_q'] AND c.is_test=false
RETURN c.path, c.file, r.line ORDER BY c.file;
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol {path:'antares_ql::eval::eval_q'}) WHERE c.is_test=false
RETURN c.path, c.file, r.line ORDER BY c.file, r.line;
```

## 6. Decision 4: write forms

Every write becomes exactly one TS 104 175 operation per Entity it changes, run through the NGSI-LD handler and its `gate!`; the pre-image, post-image and linked-entity checks are the operation-matrix section 6 rows of that operation (repeated here only where OData adds to them). The body is translated into the **normalized** NGSI-LD form (`{"type":"Property","value":...}`, `{"type":"Relationship","object":...}`), never the concise one, so no kind is inferred from a value's shape. A Relationship written through OData (a bind, a `$ref`, an `@id` reference) is a **relationship write**: the target Entity is read first with `retrieve_entity` under the caller's view (its own gate, clause 5.7.1), and a target that read answers 404 makes the write fail with the same 404 an absent target would give, so a write cannot reveal or create a link to an Entity the caller cannot see.

Pre-image, post-image and allowlist checks are ADR-0023 rule 7's and run in the NGSI-LD handler the surface calls, so OData inherits them unchanged: the stored Entity is loaded and must be in the view (else 404), the proposed result must still be in the view (else 403 `AccessDenied`), a write naming an Attribute outside the allowlist is refused whole (403, never a silent drop), and a create must produce an Entity the view selects (403 before anything is written). An operation outside the view's `operations` allowlist is 405 with `Allow`, or 422 `OperationNotSupported` where the method stays (rule 4); the OData surface computes `Allow` per resource from the same allowlist. Until rule 7 is implemented, the matrix's "[not enforced today ...]" checks stay unenforced on this surface too: it adds no second store read before a gate (ADR-0014 rule 1).

| Id | Form | OData 4.01 | Status | Operation-matrix row | NGSI-LD request | Linked-entity checks (beyond the matrix row) | Refusal |
|---|---|---|---|---|---|---|---|
| W1 | `POST /{Set}` | Protocol 11.4.2 | mapped | `create_entity` | `POST /entities` with `type` = the set's type IRI, `id` from the body | none beyond the row; an id that exists outside the view answers 409 AlreadyExists (ADR-0023 rule 3) | no key: the surface mints `urn:ngsi-ld:{Set}:{uuid v4}` (`{Set}` the set's name, the class name in its CSDL form), or with an E2a `Edm.Guid`/`Edm.String` key the key value `{uuid v4}` and the URN from it, before the operation, so the policy gate sees the minted id; `Location` and, under `Prefer: return=minimal` (204), `OData-EntityId` name it (Protocol 11.4.2, 8.3.3; T-3721). No key for an `Edm.Int64` E2a key: 400 `MissingKey`; `@odata.type` of another type: 400; a body whose R6 Property names an Entity that is absent or hidden: 400 `BadRequest`, target the Property, nothing written (T-3761) |
| W2 | `POST /{Set}` with `<nav>@odata.bind` (4.0) or `<nav>: {"@id": ...}` (4.01) | Protocol 11.4.2.1 | mapped | `create_entity` | the navigation becomes a Relationship attribute | each bound target: `retrieve_entity` under the view first; the bound URL must address this service and the navigation's target set (Protocol 11.4.2.1: a contradiction "MUST fail ... 400 Bad Request") | a hidden or absent target: the same 400 `BadRequest`, target the navigation, nothing written (the bind is the request body's; T-3760); the engine refusing the target read keeps its own answer |
| W3 | deep insert: `POST /{Set}` with nested entities | Protocol 11.4.2.2 | rejected | `create_entity` (cell refined, section 11) | none | the matrix's per-entity Create Entity checks hold if it is ever enabled | 501 `NotImplemented`. Reason: "On success, the service MUST create all entities and relate them. On failure, the service MUST NOT create any of the entities"; NGSI-LD has no transaction (`cim009-to-ts104175.md` row 5.5.14) and every create fires its notifications before a later member can fail, so a compensating delete is still observable |
| W4 | `PATCH /{Set}('{id}')` | Protocol 11.4.3 | mapped | `merge_entity` | `PATCH /entities/{id}`; a property set to `null` becomes the NGSI-LD Null `urn:ngsi-ld:null` (8.4.3), which deletes the Attribute; complex values merge deeply in both protocols (Protocol 11.4.3 "Missing properties of the containing entity or complex property ... MUST NOT be directly altered", 8.4.3 "merges ... up to an arbitrary depth") | a single-valued `<nav>@odata.bind` or `{"@id"}` in the body: relationship write, target read first | NonUpdatable property (C9): 400; an R6 Property as in W1 |
| W5 | `PATCH` with a bind on a **collection-valued** navigation | Protocol 11.4.3.1 ("For collection-valued navigation properties this adds to the relationship") | rejected | `merge_entity` | none | | 501: adding to an `object` array has no NGSI-LD operation; a read-modify-write loses concurrent updates |
| W6 | `PUT /{Set}('{id}')` | Protocol 11.4.3 | mapped | `replace_entity` | `PUT /entities/{id}`; omitted properties are removed (Protocol 11.4.3: "Missing non-key, updatable structural properties ... MUST be set to their default values", and there are no defaults) | as W4 | |
| W7 | `PATCH`/`PUT` on a key that does not exist (upsert) | Protocol 11.4.4 | rejected | `odata_upsert` (refined: 404, not 501) | none | | **404** `NotFound`, the same answer as a hidden Entity. Protocol 11.4.4 lets the service "fail the request altogether"; a 501 here and a 404 for a hidden id would tell the two apart (security S3) |
| W8 | deep update: nested entities or a nested delta in `PATCH`/`PUT` (4.01) | Protocol 11.4.3.1 | rejected (*new* `odata_deep_update`) | none | none | | 501, same reason as W3 |
| W9 | `DELETE /{Set}('{id}')` | Protocol 11.4.5 | mapped | `delete_entity` | `DELETE /entities/{id}`; an Entity of another type than the set's: 404 | none (no referential integrity, matrix row) | |
| W10 | `PUT /{Set}('{id}')/{prop}` (primitive or collection) | Protocol 11.4.9.1, 11.4.9.4 | mapped | `replace_attribute` | `PUT /entities/{id}/attrs/{attr}` | | NonUpdatable: 400; an R6 Property as in W1 |
| W11 | `PATCH /{Set}('{id}')/{complexProp}` | Protocol 11.4.9.3 | mapped | `partial_attribute_update` | `PATCH /entities/{id}/attrs/{attr}` | | |
| W12 | `PUT /{Set}('{id}')/{prop}/$value` | Protocol 11.4.9.1 | rejected | `set_attribute_value` (`not_offered` in the matrix: the broker has no route) | none | | 501 until the broker serves 10.2.6 |
| W13 | `DELETE /{Set}('{id}')/{prop}`, `.../{prop}/$value` | Protocol 11.4.9.2 | mapped | `delete_attribute` | `DELETE /entities/{id}/attrs/{attr}`; OData "sets the property to null", and an absent Attribute is NGSI-LD's null (N10) | relationship delete: no target read (matrix row) | key or Computed property: 400 |
| W14 | `POST` to a collection property (append), positional insert with `$index` | Protocol 11.4.10, 11.4.11 | rejected (*new* `odata_collection_append`) | none | none | | 501 |
| W15 | any write naming `createdAt`, `modifiedAt`, `_types`, `_datasets`, or an R5 property | Protocol 11.4.9 | rejected | none | none | an R5 property is a Relationship to a hidden type: writing it would link to an invisible Entity | 400 (NonInsertable/NonUpdatable, C3, C9) |
| W16 | `PUT /{Set}('{id}')/{nav}/$ref` (single-valued) | Protocol 11.4.6.3 | mapped | `append_attributes` (cell refined: was not offered) | `POST /entities/{id}/attrs` with the Relationship; Append overwrites by default (10.2.4), so it works whether or not the Relationship exists | target read first under the view | hidden target: 404 |
| W17 | `DELETE /{Set}('{id}')/{nav}/$ref` (single-valued) | Protocol 11.4.6.2 | mapped | `delete_attribute` | `DELETE /entities/{id}/attrs/{nav}` | none | |
| W18 | `POST /{Set}('{id}')/{nav}/$ref` (add to collection) | Protocol 11.4.6.1 | rejected | `update_attributes` (cell refined) | none | | 501, as W5 |
| W19 | `DELETE .../{nav}/$ref?$id=...` or `.../{nav}('{k}')/$ref` (remove one from collection) | Protocol 11.4.6.2 | rejected | `update_attributes` (cell refined) | none | | 501, as W5 |
| W20 | `PUT /{Set}('{id}')/{nav}/$ref` with the full collection (replace all) | Protocol 11.4.6.4 | mapped | `replace_attribute` | `PUT /entities/{id}/attrs/{nav}` with `object` = the full list (ListRelationship: `objectList`) | every target read first | any hidden target: 404, nothing written |
| W21 | `PATCH /{Set}` with a delta payload | Protocol 11.4.12 | rejected | `odata_delta_update` | none | | 501 |
| W22 | `PATCH`/`DELETE /{Set}/$each` | URL 4.13, Protocol 11.4.13, 11.4.14 | rejected | `odata_each`, `purge_entities` | none | | 501 |
| W23 | `DELETE /{Set}` without a key (*new* `odata_delete_collection`) | none (not an OData request) | rejected | none | none | | 405 Method Not Allowed |
| W24 | `POST /$batch`, `multipart/mixed` | Protocol 11.7, 11.7.7 | mapped | each member by its own row (W1 to W23, Q29 to Q35), not `batch_create`/`batch_delete` (cells refined) | members run in order, each its own gate and its own status | per member | a change set: 501 (`odata_batch_atomicity`, Protocol 11.7.7.1 change sets are atomic); a third multipart level: 400; a member URL for another host: 400 (`odata-library.md` 3.4) |
| W25 | `POST /$batch`, `application/json` | JSON Format 19.1 | mapped | as W24 | `dependsOn` names only earlier members, so array order is the execution order; a cycle, a forward reference or an unknown id: 400 (`odata-batch` `UnknownDependency`, T-3505) | per member | `atomicityGroup`: 501 |
| W26 | `$batch` options: `Prefer: odata.continue-on-error`, Content-ID references (`$1/...`, Protocol 11.7.7.2), `$batch` inside `$batch` | Protocol 11.7 | `continue-on-error` mapped (multipart stops on the first failure without it, a JSON batch only with `continue-on-error=false`, JSON Format 19.4; a JSON batch under 4.0 is 400; T-3726); references and nesting rejected (*new* `odata_batch_references`) | none | | | references: 501; nesting: 400; more members than min(`odata-batch` `max_requests`, `MAX_BATCH_ITEMS` `bounds.rs:42`): 413-equivalent 400 |
| W27 | bound and unbound actions, action imports | Protocol 11.5.5 | rejected | `odata_actions` | none | | 501 |
| W28 | bound and unbound functions, function imports | Protocol 11.5.4 | rejected | `odata_functions` | none | | 501 |
| W29 | action on members of a collection | Protocol 11.5.2 | rejected | `odata_actions` | none | | 501 |
| W30 | media entities: create, update stream, delete, `GET .../$value` | Protocol 11.4.7, URL 4.14 | rejected | `odata_media_entities` | none | | 501 (`HasStream` never set) |
| W31 | stream properties: update, delete | Protocol 11.4.8 | rejected | `odata_stream_properties` | none | | 501 (no `Edm.Stream` property) |
| W32 | `If-Match` / `If-None-Match` on a write (*new* `odata_conditional_write`) | Protocol 11.4.1.1 | rejected | none | none | | 501: "the operation MUST only be invoked if the If-Match or If-None-Match condition is satisfied", and the broker has no conditional write (no `If-Match`/`ETag` handling in `crates/antares-api/src`, no such header in TS 104 176) |
| W33 | any write method on `/`, `/$metadata`, `/$count`, `/$query` | | rejected (*new* `odata_write_readonly`) | none | none | | 405 |

**Decided (worker-2, owner may override):** deep insert and deep update refused in round 1 (W3, W8), which changes the matrix's safest default for open question 7, because the OData rule is absolute and NGSI-LD notifications make every partial create observable; options: decide every gate first and compensate on failure (the matrix default; still observable), or a broker-level transaction (no such clause in TS 104 175).

**Decided (worker-2, owner may override):** collection-valued `$ref` add and remove refused (W5, W18, W19), because there is no NGSI-LD operation that adds one object to a Relationship and a surface-side read-modify-write loses concurrent writes; options: read-modify-write under a per-entity lock in the surface (a lock no other surface honours), a new broker operation (outside TS 104 175).

```cypher
// the write handlers each row reaches and their gate sites (same query as operation-matrix 2.2, write clauses only); Antares clone
MATCH (c:Symbol)-[r:CALLS]->(g:Symbol {path:'antares_api::policy::gate'})
WHERE c.is_test = false AND c.file IN ['crates/antares-api/src/entities.rs','crates/antares-api/src/attrs.rs','crates/antares-api/src/batch.rs']
RETURN c.path, c.file, r.line ORDER BY c.file, r.line;
```

## 7. Decision 5: one EdmModel, its two serialisations, request validation, cache key and ETag

### 7.1 One model, used four times

The surface builds one `odata_csdl::Model` (`odata-library.md` 3.2) per cache key (7.3) from sections 3 and 4, and uses that same value for:

| Id | Use | Function (`odata-library.md`) | Consequence |
|---|---|---|---|
| M1 | `GET /$metadata` as CSDL XML | `odata_csdl::write_xml` | default for `$metadata` (Protocol 11.1.2) |
| M2 | `GET /$metadata?$format=json` or `Accept: application/json` | `odata_csdl::write_json` | 4.01 only (section 3.4) |
| M3 | resource path and query option resolution | `odata_url::resolve(uri, &model, &limits)` | a name not in the model is unresolvable: the S3/S4 guarantee rests on this being the same `Model` value as M1/M2, not a second description |
| M4 | request body validation | `odata_json::read_entity_body(input, ty, &model, &limits)` | an undeclared or Computed property in a write is 400 before the NGSI-LD handler runs (C3, C9) |

A unit test holds the four together: for a generated model and view, every name M1 and M2 print resolves in M3 and is accepted by M4, and a name taken from the unrestricted model but absent from the view's model is refused by M3 and M4 with the same error as a random name (security S4).

**Decided (worker-2, owner may override):** the model is built from the view's own `@context` (the contexts the view configuration names, ADR-0023 rule 5) for short names, and from the LinkML slot names when a term is not in it; no request `Link` header is consulted, because OData clients do not send one and a per-request context would make the model differ per request. Options: core context only (every user term would be a full IRI, mostly over the 128-character limit of 3.3); the request `Link` header (unbounded cache keys, a client could rename the model).

### 7.2 What the model contains, by construction

Only: EntitySets for types in `selector ∩ engine` (C1); Properties for attributes in `allowlist ∩ engine` that (a) declares, plus (b) names for open-world classes; NavigationProperties and bindings only between visible types (R1 to R4, N15); Capabilities annotations from section 4; the `Antares.NgsiLd`, `Org.OData.Core.V1`, `Org.OData.Capabilities.V1`, `Org.OData.Measures.V1` and `Org.OData.Validation.V1` references (Measures includes Core and Validation, `odata-library.md` 4); no action, function, singleton or import.

### 7.3 Cache key

| Member | Why it is in the key |
|---|---|
| tenant | a view pins one tenant (ADR-0023 rule 10) |
| view id and view revision | a view change is published on the change bus and swapped atomically (ADR-0023 rule 10); the revision makes every pod's cache miss on the next request |
| engine half: the probe results of section 4 (one bit set per type and clause, plus the C1 Filter's `pick`/`omit`) | the engine is opaque (ADR-0020); the probe results are what the model depends on, so they are the key, computed per request from at most `types x 5` `decide` calls |
| schema version | DataModel `version` and source commit (`data_model.rs:458`, `DataModelOrigin.commit` `:316`) |
| `@context` hash | SHA-256 of each context document the view names, in order |
| OData version | 4.0 and 4.01 models differ (section 3.4) |

**Decided (worker-2, owner may override):** the key holds the probe results rather than a policy hash, because the broker cannot hash a policy it does not see (ADR-0020: the engine is a plug-in) and the probe bits are exactly the inputs the model uses; a bounded LRU of 256 models per pod; the probes themselves are not cached, so a policy change takes effect on the next request. Options: an engine-supplied fingerprint (needs the `describe` seam member of section 4); no cache (one model build per `$metadata` and per request validation, measurable cost, decided against).

### 7.4 ETags

- `$metadata`: strong ETag `"<sha256 of the bytes served>"`, one per serialisation (XML and JSON are different bytes), `Cache-Control: no-cache`, `If-None-Match` answered 304. This is the rule the platform gateway applies to its schema documents (`joinedcontext-platform crates/context-gateway/src/handlers/schema_routes.rs:148-201`, `revalidated` `:148`, `matches_etag` `:191`), and it holds because the model is a function of the key.
- `@odata.metadataEtag` is written in every `metadata=minimal|full` response (JSON Format 4.5.2: "the service SHOULD set the metadataEtag control information to the metadata document's ETag in all responses").
- Entities: **Decided (worker-2, owner may override):** no `@odata.etag` and no `ETag` header on entities in round 1, and `If-Match`/`If-None-Match` on writes answered 501 (W32), because Protocol 11.4.1.1 requires the write to run "only ... if the If-Match or If-None-Match condition is satisfied", the broker has no atomic conditional write, and a check-then-write in the surface races; once the store offers a compare-and-set on `modifiedAt`, the ETag becomes `W/"<modifiedAt>"` with `Core.OptimisticConcurrency [modifiedAt]` on each EntitySet. Options: emit the ETag now and check `If-Match` before the write (races), emit it for reads only (a client that echoes it in `If-Match` would then meet 501 on every write).

```cypher
// platform clone: who renders and serves the schema documents today (the code the LinkML surface replaces)
MATCH (c:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path IN ['context_gateway::handlers::schema::visible','context_gateway::handlers::formalisms::linkml','context_gateway::handlers::schema::render'] AND c.is_test=false
RETURN f.name, c.path, c.file, r.line ORDER BY f.name, c.file;
MATCH (t:Symbol)-[r:CALLS]->(f:Symbol)
WHERE f.path IN ['context_gateway::handlers::schema::visible','context_gateway::handlers::formalisms::linkml'] AND t.is_test=true
RETURN f.name, t.path, t.file ORDER BY t.file;
```

## 8. Decision 6: the LinkML surface

The surface serves the view's schema: the authoritative LinkML (section 3.1, (a)) intersected with `effective = view ∧ engine`, read-only, under `/x/views/{view}/linkml`. Its paths keep the platform's `schema/` layout and artefact names (`joinedcontext-platform crates/context-gateway/src/handlers/schema.rs:97-107` `file_name`, `:153-166` `artifact_of`), because the Portal, the SDK and the App generator read them (`gateway-to-broker-map.md` M11: "path and index format must be kept or every App generator run breaks").

Every document is **rendered from the intersected LinkML model**, never served from a committed file, which is the platform's rule ("SHACL, OWL, RDF, LinkML and Markdown are rendered from the projected model rather than served from a committed file, so no formalism can carry a slot the grant forbids", `schema_routes.rs:126-129`). One change: the gateway renders its LinkML from the projected JSON Schema (`formalisms.rs:415` `linkml`, ranges from `linkml_range` `:398-411`), which has already lost relationship ranges, `inverse` and `is_a` (N2); the broker renders from the LinkML model itself, so those survive.

| Id | Path under `/x/views/{view}/linkml` | Content | Operation-matrix row (gate clause) | Status | Refusal |
|---|---|---|---|---|---|
| L1 | `GET schema/index.json` | catalogue: per visible model and major, each artefact with media type, size and SHA-256 of the rendered bytes (the platform's `schema::index`, `schema.rs:406`) | `retrieve_entity_type_details` (5.7.6) | mapped | |
| L2 | `GET schema/v{major}/model.linkml.yaml`, `.../linkml` | LinkML YAML of the view's classes and slots, `is_a` kept only between visible classes (section 3.1), relationship `range` only to visible classes (otherwise `range: uriorcurie`, the DM-69 external-reference form), `inverse` only when both ends are visible, enums only those a visible slot uses | `retrieve_entity_type_details` (5.7.6) | mapped | unknown major: 404 |
| L3 | `GET schema/v{major}/model.schema.json`, `.../json-schema` | JSON Schema draft-07 derived from L2, with `x-ngsi-ld-kind` (DM-05) | 5.7.6 | mapped | |
| L4 | `GET schema/v{major}/context.jsonld`, `.../context` | JSON-LD `@context` holding only the visible terms | 5.7.6 | mapped | |
| L5 | `GET schema/v{major}/model.shacl.ttl`, `.../shacl` | SHACL shapes derived from L2 | 5.7.6 | mapped | |
| L6 | `GET schema/v{major}/model.owl.ttl`, `model.rdf.ttl`, `model.md`, `model.qb.ttl` | OWL, RDFS, Markdown; RDF Data Cube only when a visible class is a DSD (DM-60, `schema_routes.rs:121-125`) | 5.7.6 | mapped | `qb` without a visible DSD: 404 |
| L7 | `GET schema/v{major}/model` | negotiated by `Accept` as `schema.rs:172-190` `negotiated` does | 5.7.6 | mapped | |
| L8 | `GET classes/{type}` | one class with its slots | `retrieve_entity_type_info` (5.7.7) | mapped | out of view: 404 (ADR-0023 rules 3, 5) |
| L9 | `GET slots/{attr}` | one slot with its range and the visible classes using it | `retrieve_attribute_info` (5.7.10) | mapped | out of view: 404 |
| L10 | `HEAD` on L1 to L9 | as GET without body | as GET | mapped | TS 104 176 6.3.2: "a Context broker shall recognize the methods GET and HEAD on all endpoints" |
| L11 | `POST`, `PUT`, `PATCH`, `DELETE` on any path | | none | rejected | 405 with `Allow: GET, HEAD` (ADR-0023 rule 4) |
| L12 | `If-None-Match` | strong ETag of the rendered bytes, 304 on match | as GET | mapped | |
| L13 | any other path or artefact name | | none | rejected | 404 |

**Decided (worker-2, owner may override):** each rendered LinkML document carries `id: <source schema id>/views/<view>` and `version: <source version>`, because a narrowed schema under the source's own `id` would claim to be the full model; the gateway's `urn:joinedcontext:model:{name}:v{major}` (`formalisms.rs:432`) is not kept, since the source `id` is the IRI clients already resolve. Options: keep the source `id` (two different documents under one identity), keep the gateway's URN (loses the source IRI).

**Decided (worker-2, owner may override):** the gate clause for L1 to L7 is 5.7.6 and the rows need ADR-0023 rule 5 (discovery in `FILTERABLE`) to be implemented first; until then the LinkML surface serves the view half only and refuses with 501 when the engine answers `Filter` for 5.7.6, because a narrowing it cannot apply must not be dropped (`policy.rs:59-68`, the seam's rule). Options: wait for rule 5 before shipping the surface at all.

The YAML is written by a deterministic emitter that double-quotes every scalar (the gateway's `yaml_scalar`, `formalisms.rs:485`, does the same job), so no YAML library is linked and no YAML is parsed (section 9).

```cypher
// platform clone: the schema surface symbols that move (gateway-to-broker-map.md 7.4)
MATCH (s:Symbol) WHERE s.path STARTS WITH 'context_gateway::handlers::schema'
   OR s.path STARTS WITH 'context_gateway::handlers::formalisms::'
RETURN s.path, s.file, s.line;
```

## 9. Decision 7: Rust crates

The OData side is surveyed in `docs/analysis/odata-library.md` (T-3448, read, not redone): no Rust crate covers CSDL JSON, the JSON format writer and `$batch` together, so the verdict there is to build `odata-url`, `odata-csdl`, `odata-json`, `odata-batch` (and optional `odata-axum`) with no `antares-*` dependency. This page fixes the subset those crates must implement: exactly the `mapped` rows of sections 5 and 6, everything else `UrlErrorKind::NotImplemented` (`odata-library.md` 3.6). The LinkML side had no survey; it is below. The allow list is `deny.toml` `[licenses] allow` (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, EUPL-1.2, ISC, Unicode-3.0, Zlib, CDLA-Permissive-2.0, as `odata-library.md` 6 lists it); Antares itself is EUPL-1.2 (SPDX header of every source file, e.g. `crates/antares-api/src/types_attrs.rs:1`) with `rust-version = "1.97"` (`Cargo.toml:28`).

| Crate | Version (date) | Licence | On allow list | `unsafe` | Role here | Verdict |
|---|---|---|---|---|---|---|
| quick-xml | 0.42.0 (2026-08-22) | MIT | yes | `#![forbid(unsafe_code)]` (`odata-library.md` 6) | CSDL XML writer | use (as `odata-library.md`) |
| serde_json | 1.0.151, already in `Cargo.lock` | MIT OR Apache-2.0 | yes | 13 lines (`odata-library.md` 6) | CSDL JSON, JSON format, JSON batch, the LinkML JSON input | use |
| nom | 8.0.0, already in `Cargo.lock` | MIT | yes | 4 lines | `odata-url` fallback only; the first choice is a hand-written recursive-descent parser (`odata-library.md` 6) | fallback |
| chumsky | 0.13.0 | MIT | yes | 128 lines | parser option | not chosen (`odata-library.md` 6) |
| panschema-model | 0.4.0 (2026-09-27) | Apache-2.0 | yes | 0 hits in `src/`, no `forbid`; depends on `serde_norway` 0.9 (MIT OR Apache-2.0), which depends on `unsafe-libyaml-norway` 0.2 (MIT, a transpiled C YAML parser); also a runtime dependency on `mutants` 0.0.4 (from its `Cargo.toml`) | the only Rust LinkML metamodel crate found (crates.io `q=linkml`: 4 crates, the others are a SHACL engine, its CLI and an unrelated markdown tool); 18 downloads; its reader takes a file path (`io.rs:50` `fn read(&self, input: &Path)`) | **no**: an unsafe YAML parser on a request-adjacent path, single maintainer, reads from the filesystem |
| serde-saphyr | 1.3.0 (2026-09-16) | MIT OR Apache-2.0 | yes | `#![forbid(unsafe_code)]` (`src/lib.rs`) | YAML reader | not needed (no YAML is parsed, section 8) |
| yaml-rust2 | 0.13.0 (2026-09-11) | MIT OR Apache-2.0 | yes | 0 hits in `src/`, no `forbid` | YAML reader | not needed |
| saphyr-parser | 0.1.0 (2026-09-19) | MIT OR Apache-2.0 | yes | 0 hits in `src/`, no `forbid` | YAML reader | not needed |
| jsonschema | 0.58.6 (2026-10-06) | MIT | yes | not checked | validate L3 output in tests | dev-dependency candidate; `\p{L}` support for `csdl.schema.json` unknown (`odata-library.md` 7) |
| shacl (rudof) | 0.3.25 (2026-10-09) | MIT OR Apache-2.0 | yes | not checked | validate L5 output in tests | dev-dependency candidate; transitive tree not checked |
| json-ld | 0.21.4 (2026-02-19) | MIT/Apache-2.0 | yes | not checked | JSON-LD processing | not needed: the broker has `antares-jsonld` |
| linkml (Python) | 1.12.0 | Apache-2.0 (PyPI `license_expression`) | not a Rust dependency | n/a | the reference toolchain: `linkml-lint`, generators, used by the conformance job (T-3449) on every L2 document | CI only |
| linkml-runtime (Python) | 1.12.0 | CC0-1.0 | not on the list, but not a Rust dependency, so outside `cargo deny` | n/a | pulled by the CI job only | CI only |

**Decided (worker-2, owner may override):** one new crate `linkml-model` (no `antares-*` dependency, `#![forbid(unsafe_code)]`, the lint set of GOAL point 8) holding the LinkML subset this page uses (schema `id`/`name`/`version`/`prefixes`/`default_prefix`, classes with `class_uri`/`is_a`/`slots`/`slot_usage`/`description`, slots with `slot_uri`/`range`/`required`/`multivalued`/`identifier` (T-3720)/`inlined`/`inverse`/`unit`/`annotations`/`description`/`rank`/`title` (T-4672: the display order, read by the SHACL writer as `sh:order`, `sh:group` and language-tagged `sh:name` from the `summary`, `hidden` and `label` annotations), enums with `permissible_values`) read from the resolved LinkML JSON with `serde_json` and `#[serde(deny_unknown_fields)]`, so a metaslot the crate does not understand fails the load (the view is then not served, ADR-0023 rule 10) instead of being ignored; plus writers for L2 to L6. The surface crates are `antares-odata` (feature `odata`) and `antares-linkml` (feature `linkml`); `antares-odata` depends on the `odata-*` crates and `linkml-model`, `antares-linkml` on `linkml-model`; neither reaches the store except through the handlers (GOAL point 6). Options: `panschema-model` (rejected above), parsing YAML with `serde-saphyr` in the broker (an input format the broker does not need, since Model Tools already runs the Python toolchain).

## 10. Security

Each item: the rule, where it is enforced, and the test that proves it (test names are proposals for T-3444; none exists yet).

### S1. `$expand` depth and fan-out

Rule: depth of the expand tree ≤ `ExpandRestrictions/MaxLevels` (C13) ≤ `MAX_JOIN_LEVEL` = 10 (`bounds.rs:109`); `$levels=max` is clamped to it and an explicit larger value is 400; the number of expand items ≤ `odata_url::Limits::max_expand_items`; the number of Entities embedded in one answer ≤ `MAX_JOIN_LOOKUPS` = 1000 (`crates/antares-api/src/repr.rs:728`), the broker's own budget for a join walk. No expansion runs before T-3470: a join target is embedded only if `view ∧ engine` selects it (ADR-0023 rule 6), treated as "not stored locally" otherwise, so hidden and absent targets look the same. Navigation paths in `$filter` (F24) stay 501 until the linked-entity `q` evaluation checks each hop (T-3474): today it reads the target straight from the store (`crates/antares-api/src/entities.rs:1260-1265` calls `notify::linked_eval`, `crates/antares-api/src/notify.rs:548`, whose lookup goes to `st.store` at `:572`).

Defaults, **Decided (worker-2, owner may override):** `max_expand_depth = 3`, `max_expand_items = 8`, because the platform's data products are one or two hops deep and every level multiplies the page; options: the broker's 10 (allowed, but a page of 100 Entities at depth 10 is the 1000-lookup budget at once).

Tests: `odata_expand_levels_above_max_is_400`, `odata_expand_max_is_clamped_to_max_levels`, `odata_expand_stops_at_the_join_lookup_budget`, `odata_expand_hidden_target_is_omitted_like_an_absent_one` (fixture of T-3470: Asset to Secret, Secret hidden by the view `q`; the answer equals the answer for a dangling Relationship byte for byte), `odata_filter_navigation_path_is_501_until_linked_eval_is_narrowed`.

### S2. `$filter` complexity

Rule: before translation, `odata_url::Limits` bound the raw query (`max_query_len` ≤ `MAX_URI_BYTES` = 8 KiB, `bounds.rs:28`), expression depth, node count, `in` list length and literal length; after translation the `QNode` must satisfy the broker's own bounds (`MAX_Q_DEPTH` = 64, `MAX_Q_NODES` = 512, rendered length ≤ `MAX_Q_BYTES` = 4096; `crates/antares-ql/src/lib.rs:237-250`), and a pattern compiles through the bounded regex cache (`crates/antares-ql/src/regex.rs:131`). F3 doubles a term (`Or[Exists, Cmp]`), so the node bound is checked on the translated tree, not on the OData tree. Over any bound: 400, OData error `code` `TooComplexQuery` (the NGSI-LD error name, `crates/antares-model/src/error.rs:94`; NGSI-LD answers it 403, `error.rs:77`, which in OData would read as a permission refusal, so the surface answers 400).

Defaults, **Decided (worker-2, owner may override):** `max_expr_depth = 32`, `max_expr_nodes = 256`, `max_in_list_len = 100`, `max_literal_len = 1024`, because each stays under the broker bound after F3's doubling (256 OData nodes → at most 512 `QNode`s); options: the broker's bounds directly (an OData filter that passes parsing could still fail after translation, with a less precise error).

Tests: `odata_filter_depth_over_limit_is_400_before_translation`, `odata_filter_ne_doubling_is_checked_on_the_translated_tree`, `odata_filter_in_list_over_limit_is_400`, a fuzz target `odata_filter_to_qnode` (arbitrary bytes into `$filter`; property: either an error or a `QNode` that `antares_ql::parse_q(&node.to_string())` accepts unchanged), a proptest `odata_filter_translation_agrees_with_reference_eval` (random entities and filters; the OData reference evaluation of section 5.2 semantics equals `eval_q` of the translation).

### S3. Inference through `$filter` on hidden properties (and `$orderby`, `$select`, counts and errors)

Rule: a hidden attribute is not in the model, so any option naming it fails resolution (M3) with the 400 a misspelled name gets: no row is selected, ordered, counted or projected by it, and the error text names the option, never the value. Below the surface, ADR-0023 rule 2 makes a hidden term evaluate as absent, so a translator bug cannot reopen the channel (defence in depth). `@odata.count` and the next links are computed by the store on the conjoined query (ADR-0023 rule 1), never after the fact. Every existence-revealing status is the same for hidden and absent: W7 (upsert) 404, Q30 404, the create of an out-of-view id 409 as on the NGSI-LD view (ADR-0023 rule 3, which accepts that one channel because 10.2.2.4 fixes the status). Errors never echo literals (`odata-library.md` 3.3 rule).

Gap closed by this rule: at `7185e1a` the ADR-0020 `Filter`'s `pick`/`omit` are read only by the projection (`policy.rs:192-205` `project`, `repr.rs:148`, `:185`, `:210-211`; cg query in section 3), so `q=secret>5` under an `omit: [secret]` narrowing still selects by `secret` on the NGSI-LD surface; ADR-0023 rule 2 fixes that for views. The platform has this defence and its tests: `joinedcontext-platform crates/context-gateway/tests/filter_oracle_tests.rs` (`bisecting_a_hidden_value_learns_nothing` `:227`, `ordering_on_a_foreign_attribute_drops_the_type` `:423`, `negation_and_non_existence_are_references` `:275`).

Tests: `odata_filter_on_hidden_attribute_equals_filter_on_unknown_name` (status, body and timing class equal), `odata_bisecting_a_hidden_value_learns_nothing` (port of the platform test), `odata_orderby_hidden_is_400_like_unknown`, `odata_count_is_the_narrowed_count`, `odata_upsert_on_absent_and_on_hidden_id_answer_the_same_404`, `odata_error_body_never_contains_the_literal` (property over random literals).

### S4. `$metadata` must not reveal hidden types or attributes

Rule: the model is built only from `selector ∩ engine` and `allowlist ∩ engine` (section 7.2); a hidden type leaves no EntityType, EntitySet, BaseType, NavigationProperty `Type`, binding `Target` or annotation `Path`; a hidden attribute leaves no Property, no Capabilities path and no `NgsiLd.Iri`; a Relationship to a hidden type becomes an R5 string property; `_types` lists visible types only; enum members, units and descriptions come from (a) only for visible slots; discovery source (b) is not used before ADR-0023 rule 5; sampling (c) never feeds a public view. The same `Model` serves CSDL XML, CSDL JSON and request validation (M1 to M4), and the LinkML surface renders from the same intersection (section 8), so the five documents cannot disagree.

Tests: `metadata_never_names_a_hidden_type_or_attribute` (for each of XML, JSON, the service document, L1 to L9: the bytes contain none of the hidden names or IRIs; the platform's `an_endpoint_never_names_what_it_hides` in `tests/schema_surface_tests.rs` is the model), `metadata_relationship_to_hidden_type_is_a_url_string`, `metadata_xml_and_json_describe_one_model` (the OASIS converter round trip of `odata-library.md` 4, item 4), `metadata_etag_changes_when_the_view_changes`, `metadata_public_view_without_schema_is_refused_at_load`.

### S5. Further items found while mapping

| Id | Risk | Rule | Test |
|---|---|---|---|
| S5.1 | `$batch` amplification | member count ≤ min(`max_requests`, `MAX_BATCH_ITEMS`); body ≤ `MAX_BODY_BYTES` (`bounds.rs:21`); no nesting; no foreign host; each member gated alone (W24 to W26) | `odata_batch_part_count_is_capped`, `odata_batch_member_for_another_host_is_400`, `odata_batch_members_are_gated_one_by_one` |
| S5.2 | `$skiptoken` tampering | the token holds offset, remaining `$top` and a SHA-256 digest of the normalised query plus view id; a mismatch is 400; with offset only inside, a forged token can do no more than `$skip` | `odata_skiptoken_from_another_query_is_400` |
| S5.3 | `/$query` body bypassing the engine | `/$query` runs as `GET /entities`, never `entityOperations/query` (Q22; operation-matrix 5.1 gap of `query_entities_post`) | `odata_query_segment_reaches_the_engine_with_q` |
| S5.4 | Relationship write to an invisible target | target read under the view first (section 6); R5 properties read-only (W15) | `odata_bind_to_hidden_or_missing_target_is_400_and_writes_nothing`, `odata_bind_keeps_the_engines_refusal_of_its_target`, `odata_a_reference_to_a_hidden_or_missing_entity_is_400_and_writes_nothing` (an R6 Property, T-3761) |
| S5.5 | Capabilities read as a grant | annotations are advisory; every request is gated (section 4) | `odata_capabilities_true_but_engine_denies_is_403` |

## 11. Changes this page makes to operation-matrix rows

To be applied to `operation-matrix.yaml` by the task that owns it (this page does not edit it):

| Matrix row | Column | Was | Now | Section |
|---|---|---|---|---|
| `create_entity` | odata | supported, deep insert 11.4.2.2 | supported for W1, W2; deep insert rejected 501 | W3 |
| `update_attributes` | odata | supported: `$ref` POST/PUT/DELETE | collection `$ref` add/remove rejected 501; single-valued `$ref` moved to `append_attributes`/`delete_attribute`; replace-all to `replace_attribute` | W16 to W20 |
| `append_attributes` | odata | not offered | supported: `PUT .../{nav}/$ref` single-valued | W16 |
| `delete_attribute` | odata | `DELETE .../{property}` | also `.../{prop}/$value` and single-valued `DELETE .../{nav}/$ref` | W13, W17 |
| `replace_attribute` | odata | `PUT .../{property}` | also `PUT .../{nav}/$ref` replace-all | W20 |
| `batch_create`, `batch_delete` | odata | `$batch` members map here | `$batch` members map to the single-Entity rows | W24 |
| `query_entities` | odata | `GET /{EntitySet}` with options | also `POST .../$query` and `{Set}/$ref`; `$expand` gated on T-3470 | Q3, Q22, Q33a |
| `retrieve_entity` | odata | with `$select`, `$expand` | also `$entity?$id=`, related entities and `$ref` reads | Q23, Q32, Q33, Q33b |
| `odata_upsert` | (rejected) | 501 | 404 `NotFound` | W7 |
| `odata_schemaversion` | (rejected) | 501 | mapped for `*` and the current version, 501 otherwise | Q18 |
| new rejected rows | | | `odata_expand_options`, `odata_deltatoken`, `odata_root`, `odata_it_this`, `odata_custom_option` (400), `odata_filter_segment`, `odata_ordinal`, `odata_deep_update`, `odata_collection_append`, `odata_delete_collection` (405), `odata_batch_references`, `odata_conditional_write`, `odata_write_readonly` (405) | 5.1, 6 |
| new mapped row | | | `odata_query_segment` | Q22 |
| `retrieve_entity_types` ... `retrieve_attribute_info` | linkml | `/x/linkml/schema`, `classes/{type}`, `slots/{attr}` | paths of section 8 under `/x/views/{view}/linkml` | L1 to L13 |
| every row | odata, linkml | `/x/odata/v4`, `/x/linkml` | `/x/views/{view}/odata/v4`, `/x/views/{view}/linkml` (ADR-0023 rule 10) | 1 |

Filed as T-3474 (group `antares-surfaces`, batch with T-3470): **the linked-entity evaluation of `q` (`attr{...}` hops) reads each target from the store without applying `view ∧ engine`** (`entities.rs:1260-1265`, `notify.rs:548-572`); T-3470 covers the join walk only. Until it is fixed, F24 stays 501 and the NGSI-LD view must refuse or narrow `q` terms with linked hops the same way (ADR-0023 rule 2 says "at any hop of a `LinkedEntityRelation`" for hidden Attributes, but says nothing about a hidden target Entity).

## 12. Unknowns

- Whether Model Tools' `gen-linkml` (or another LinkML generator) can write the resolved schema as JSON (section 3.1). Not checked against the LinkML CLI.
- DM-05 names seven kinds and no ListRelationship (`docs/Requirements/data-models.md:18`); how the platform models a ListRelationship is unknown.
- The example model's `refStation` has `ngsi_ld_kind: Relationship` and no `range` (`air-quality-observed.linkml.yaml:69-73`), which DM-64 requires; whether it is meant as a DM-69 external reference is unknown. Under section 3.2 it becomes R4.
- The example context maps `location` to `geojson:geometry` (`context/air-quality-observed.v1.jsonld:18-19`) while the NGSI-LD core term is `https://uri.etsi.org/ngsi-ld/location` (`crates/antares-ql/src/geo.rs:25`); which wins when the broker expands under that context was not traced.
- The gateway's LinkML emits `permissible_values: [a, b]` as a list on a slot (`formalisms.rs:471`); whether that is valid against the LinkML metamodel was not checked (the conformance job, T-3449, should run `linkml-lint` on L2).
- Whether the broker's `q` evaluates `createdAt`/`modifiedAt` (F33): no special case in `antares-ql` (`grep` of `eval.rs`, `sql.rs`: no hit), behaviour on stored documents not traced.
- Whether `pick` limits which Relationships a join follows (Q3 drops unrequested expansions after the fact): not traced in `repr.rs`.
- The unit of `geo.distance` for SRID 4326 (F30): URL 5.1.1.11.1 does not say metres.
- The status for an unsupported `$format` or a 4.0 request for CSDL JSON: OData names none; 406 is the HTTP rule chosen here.
- Whether the mapped subset meets OData 4.01 Intermediate conformance (Protocol 13.2.2): not checked row by row; C19 claims Minimal.
- That the .NET OData client sends `If-Match` whenever an entity carries an ETag (reason given in 7.4) is from memory, not verified.
- OData PATCH semantics for `Edm.Untyped` values (deep merge or replace) are not stated in the Protocol text read; NGSI-LD 8.4.3 merges deeply. A PATCH of an untyped property may therefore merge where a client expects a replace.
- `jsonschema` and `shacl` crates: `unsafe` and transitive licences not checked.
- Spec defects recorded in section 2 (10.6.2 clause reference and duplicate, Table 5.2.6.4.9-1 type value, 7.2.3.2 EXAMPLE 7 nesting) are not yet reported to ETSI.

## 13. Related

- `docs/adr/ADR-0023-endpoint-views.md` (views, status codes, discovery, joins, writes, addressing)
- `docs/adr/ADR-0020-policy-seam.md`, `docs/adr/ADR-0014-extension-hooks-model.md`
- `docs/analysis/operation-matrix.md` and `.yaml` (T-3437), `docs/analysis/odata-library.md` (T-3448), `docs/analysis/gateway-to-broker-map.md` (T-3436), `docs/analysis/cim009-to-ts104175.md`
- `crates/antares-api/src/policy.rs`, `crates/antares-api/src/types_attrs.rs`, `crates/antares-api/src/repr.rs`, `crates/antares-ql/src/lib.rs`, `crates/antares-ql/src/render.rs`
- platform: `crates/jc-core/src/kinds/data_model.rs`, `crates/context-gateway/src/handlers/schema.rs`, `schema_routes.rs`, `formalisms.rs`; docs `Decisions/adr-n-010-linkml-data-models.md`, `Requirements/data-models.md` (DM-05, DM-64), `Architecture/11-data-models.md`
- tasks T-3443 (architecture ADR), T-3444 (test plan), T-3445 (files round 1), T-3448 (OData library), T-3449 (conformance suites), T-3470 (join target visibility)
