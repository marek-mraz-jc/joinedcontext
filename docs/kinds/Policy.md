# Policy

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Policy`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/policies`
- Repository path: `projects/{project}/spaces/{space}/policies/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (PolicySpec)

Desired specification of a `Policy` resource (ADR 002, R1..R9).

| Field | Type | Required | Description |
|---|---|---|---|
| `actions` | list of [PlatformAction](#platformaction) | no | Granted platform actions CIM 009 has no operation for (ADR-N-054, T-4666). |
| `assignee` | [Principal](#principal) | no | Grantee principal receiving permissions; unset when the Policy lists `rules`. |
| `assigner` | string | yes | Granting data owner (e.g. `did:web:banskabystrica.sk`). |
| `contextSpaceRef` | [Ref](#ref) | yes | Reference to the target ContextSpace. |
| `effect` | [PolicyEffect](#policyeffect) | no | Whether this policy grants or takes away (GW4, GW8). |
| `geoQ` | string | no | Residual geographic query filter string (R7, GW11, EP-56). |
| `information` | list of [RegistrationInfo](#registrationinfo) | no | Target entity types and attribute whitelist (R6, R9). |
| `operations` | list of [OperationRef](#operationref) | no | Granted CIM 009 operations or operation groups (R8); empty when the Policy grants `actions` alone. |
| `q` | string | no | Residual NGSI-LD query filter string (R7, EP-56). |
| `rules` | list of [PolicyRule](#policyrule) | no | Several grants and prohibitions in one Policy (ADR-N-057 decision 1), instead of the single rule above; [`PolicySpec::expand`] turns either shape into one grant per rule. |
| `scopeQ` | string | no | Residual scope query filter string (R7, R13, EP-56). |
| `temporalQ` | string | no | Residual temporal query filter string (R7, GW11, EP-56). |
| `validity` | [Validity](#validity) | no | Optional time validity window for this policy. |

## PlatformAction

What a Policy grants beyond CIM 009's operations (ADR-N-054 2.4, 2.6; T-4666).

Type: one of `"readVersions"`, `"personalData"`

## Principal

Identity principal receiving a policy grant (ADR 002).

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | no | Identifier of the principal (username, role name, group name, DID). An `organization` assignee has none: it is always the Policy's own Organization (ADR-N-067 2.3). |
| `kind` | [PrincipalKind](#principalkind) | yes | Category of principal. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## PolicyEffect

Whether a policy grants access or takes it away (GW4, GW8).

A prohibition subtracts from a grant that stays in force. It is not how a resource is kept private in the first place: the default verdict is already DENY (GW5), so a type nobody was granted is closed without any policy naming it.

Type: one of `"permission"`, `"prohibition"`

## RegistrationInfo

Specification of target entity types, properties, and relationships (R6).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | map of [AttributeCapabilities](#attributecapabilities) | no | What the grant lets a caller do with each attribute, keyed by attribute name or `*` (ADR-N-066, DM-147). It only narrows what the operations and actions grant. |
| `entities` | list of [EntitySelector](#entityselector) | yes | Target entity selectors. |
| `propertyNames` | list of string | no | Whitelist of readable or writable property names (R6, R9). |
| `relationshipNames` | list of string | no | Whitelist of readable or writable relationship names (R6, R9). |

## OperationRef

Reference to a CIM 009 operation or operation group (R8).

Closed enum ensures no proprietary verbs (`read`, `write`, `upsertEntity`) are permitted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: [Operation](#operation) or [OperationGroup](#operationgroup)

## PolicyRule

One rule of a Policy's `rules[]` (ADR-N-057 decision 1): the single-rule members of [`PolicySpec`], while `assigner`, `contextSpaceRef` and `validity` stay on the Policy.

| Field | Type | Required | Description |
|---|---|---|---|
| `actions` | list of [PlatformAction](#platformaction) | no | Granted platform actions (ADR-N-054). |
| `assignee` | [Principal](#principal) | yes | Grantee principal of this rule. |
| `effect` | [PolicyEffect](#policyeffect) | no | Whether this rule grants or takes away (GW4, GW8). |
| `geoQ` | string | no | Residual geographic query filter string (R7, GW11, EP-56). |
| `information` | list of [RegistrationInfo](#registrationinfo) | no | Target entity types and attribute whitelist (R6, R9). |
| `operations` | list of [OperationRef](#operationref) | no | Granted CIM 009 operations or operation groups (R8). |
| `q` | string | no | Residual NGSI-LD query filter string (R7, EP-56). |
| `scopeQ` | string | no | Residual scope query filter string (R7, R13, EP-56). |
| `temporalQ` | string | no | Residual temporal query filter string (R7, GW11, EP-56). |

## Validity

Temporal validity window for a policy grant.

| Field | Type | Required | Description |
|---|---|---|---|
| `from` | string (date-time) | no | Effective start timestamp (UTC). |
| `to` | string (date-time) | no | Effective expiration timestamp (UTC). |

## PrincipalKind

Principal category for authorization rules.

Type: one of `"user"`, `"group"`, `"role"`, `"serviceAccount"`, `"did"`, `"organization"`

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## AttributeCapabilities

One attribute's capabilities in a grant (ADR-N-066, DM-147). A field left unset takes the `*` entry's field, then the grant's own behaviour without the block.

| Field | Type | Required | Description |
|---|---|---|---|
| `history` | boolean | no | Whether its history is readable; `true` needs `queryTemporal` or `retrieveTemporal`. |
| `metadata` | list of [MetadataMember](#metadatamember) | no | The metadata members passed for this attribute, e.g. `[observedAt, unitCode]`. |
| `read` | boolean | no | Whether the current value is readable, e.g. `false` to keep an attribute for history only. |
| `versions` | list of [VersionsCapability](#versionscapability) | no | Version capabilities: `read` needs the action `readVersions`, `keep` a write operation. |
| `write` | list of [WriteCapability](#writecapability) | no | The write kinds allowed on this attribute, e.g. `[update]`; needs a write operation. |

## EntitySelector

Entity type and id/pattern selector within a policy grant (R6).

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | [Urn](#urn) | no | Explicit target entity URN. |
| `idPattern` | string | no | Anchored regular expression matching target entity URNs (R24, R33). |
| `type` | string | yes | Target entity type short name in PascalCase. |

## Operation

Table 4.20-1: every named distributed API operation in ETSI GS CIM 009 (R8).

Closed enum ensures no proprietary verbs (`upsertEntity`, `read`, `write`) are accepted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: one of `"createEntity"`, `"updateEntity"`, `"appendAttrs"`, `"updateAttrs"`, `"deleteAttrs"`, `"deleteEntity"`, `"createBatch"`, `"upsertBatch"`, `"updateBatch"`, `"deleteBatch"`, `"upsertTemporal"`, `"appendAttrsTemporal"`, `"deleteAttrsTemporal"`, `"updateAttrInstanceTemporal"`, `"deleteAttrInstanceTemporal"`, `"deleteTemporal"`, `"mergeEntity"`, `"replaceEntity"`, `"replaceAttrs"`, `"mergeBatch"`, `"purgeEntity"`, `"retrieveEntity"`, `"queryEntity"`, `"queryBatch"`, `"retrieveTemporal"`, `"queryTemporal"`, `"retrieveEntityTypes"`, `"retrieveEntityTypeDetails"`, `"retrieveEntityTypeInfo"`, `"retrieveAttrTypes"`, `"retrieveAttrTypeDetails"`, `"retrieveAttrTypeInfo"`, `"createSubscription"`, `"updateSubscription"`, `"retrieveSubscription"`, `"querySubscription"`, `"deleteSubscription"`, `"retrieveEntityMap"`, `"updateEntityMap"`, `"deleteEntityMap"`, `"createEntityMapQueryEntity"`, `"createEntityMapQueryTemporal"`, `"retrieveContextSourceIdentity"`

## OperationGroup

Table 4.20-2: the five distributed operation group names (R8).

Type: one of `"federationOps"`, `"associationOps"`, `"updateOps"`, `"retrieveOps"`, `"redirectionOps"`

## MetadataMember

A metadata member a grant may pass for an attribute (ADR-N-066 2.4, DM-139).

Type: one of `"observedAt"`, `"unitCode"`, `"createdAt"`, `"modifiedAt"`, `"modifiedBy.organization"`, `"modifiedBy.actorKind"`, `"modifiedBy.actor"`, `"provenance"`, `"reason"`

## VersionsCapability

What a grant may do with an attribute's versions (ADR-N-066 2.3).

Type: one of `"read"`, `"keep"`

## WriteCapability

A write kind a grant may allow on an attribute (ADR-N-066 2.1).

Type: one of `"update"`, `"append"`, `"delete"`

## Urn

NGSI-LD entity URN (ADR-N-041, PF-43): urn:ngsi-ld:{Type}:{id}; the prefixed shape urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId} is optional

Type: string
