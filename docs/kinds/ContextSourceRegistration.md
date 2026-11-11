# ContextSourceRegistration

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ContextSourceRegistration`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/csrs`
- Repository path: `projects/{project}/spaces/{space}/registrations/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ContextSourceRegistrationSpec)

Desired specification of a `ContextSourceRegistration` resource (MF-36, SP-09).

| Field | Type | Required | Description |
|---|---|---|---|
| `contextSpaceRef` | [Ref](#ref) | yes | The Context Space whose broker learns of the source. The registration is written into this space's tenant and no other (SP-08, SP-09). |
| `endpoint` | string | no | A source elsewhere, as the base URL of its NGSI-LD API. Mutually exclusive with `endpointRef`. |
| `endpointRef` | [Ref](#ref) | no | A source on this platform, as an `Endpoint`. Mutually exclusive with `endpoint`. |
| `expiresAt` | string (date-time) | no | When the registration stops being used, if it is not open-ended. |
| `federation` | [Federation](#federation) | no | Who this platform is when it forwards (PF-48). |
| `information` | list of [RegistrationInfo](#registrationinfo) | yes | What the source is claimed to hold, in CIM 009's own `information` shape: this is what the broker matches a query against, so it is the specification's member and not a shape of ours. |
| `mode` | [RegistrationMode](#registrationmode) | no | How the source's answer relates to the broker's own data. |
| `operations` | list of string | no | Which operations the source is registered for, as CIM 009 clause 5.2.9 names them: an operation (`retrieveEntity`) or one of its groups (`federationOps`). Left out means the specification's own default, which is why this is not an enum here: the vocabulary is the broker's and a list of ours would go stale the first time CIM 009 adds a member. |
| `schedule` | [Schedule](#schedule) | no | How often the source's schema surface is mirrored; 24 hours when absent (DM-49). |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## Federation

Who this platform is when it forwards to the source (PF-48).

| Field | Type | Required | Description |
|---|---|---|---|
| `identity` | [FederationIdentity](#federationidentity) | no | Which identity the forward carries. |
| `serviceAccountRef` | [Ref](#ref) | no | The `ServiceAccount` the hub forwards as, required in `serviceAccount` mode. |

## RegistrationInfo

Specification of target entity types, properties, and relationships (R6).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | map of [AttributeCapabilities](#attributecapabilities) | no | What the grant lets a caller do with each attribute, keyed by attribute name or `*` (ADR-N-066, DM-147). It only narrows what the operations and actions grant. |
| `entities` | list of [EntitySelector](#entityselector) | yes | Target entity selectors. |
| `propertyNames` | list of string | no | Whitelist of readable or writable property names (R6, R9). |
| `relationshipNames` | list of string | no | Whitelist of readable or writable relationship names (R6, R9). |

## RegistrationMode

How the broker treats a source's answer relative to its own data (CIM 009 clause 5.2.9).

Type: one of `"inclusive"`, `"exclusive"`, `"auxiliary"`, `"redirect"`

## Schedule

How often a [`SyncSourceSpec`] runs (MF-28): `{ interval: 30m }` or `{ webhook: true }`.

Exactly one of the two members is set, for the same reason as [`SyncOrigin`].

| Field | Type | Required | Description |
|---|---|---|---|
| `interval` | string | no | Fixed interval such as `30m`, `6h` or `1d` (suffixes `s`, `m`, `h`, `d`). |
| `webhook` | boolean | no | Driven by a webhook from the source instead of a timer; only `true` is a schedule. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## FederationIdentity

Which identity a forwarded request carries (PF-48).

No mode is a grant. `ServiceAccount` reads a source with the grants a `Policy` on that source gave the named account; `Caller` reads it with the caller's own; `Anonymous` reads an address outside this platform with no credential at all, so it sees what any visitor of that address sees. In each, the hub Endpoint's policy set applies as well and the narrower one wins.

Type: one of `"serviceAccount"`, `"caller"`, `"anonymous"`

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
