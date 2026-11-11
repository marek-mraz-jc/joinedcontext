# ContextSpace

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ContextSpace`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/spaces`
- Repository path: `projects/{project}/spaces/{name}/space.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ContextSpaceSpec)

Desired specification of a `ContextSpace` resource (PF-06, PF-09, PF-19).

| Field | Type | Required | Description |
|---|---|---|---|
| `brokerRef` | string | no | The `ContextBroker` this space lives on; the organization's default when absent (PF-111, ADR-N-047). Changing it on a space that holds entities is a move job (PF-115). |
| `dataModelRef` | [Ref](#ref) | no | The one LinkML Data Model of this space, whose classes are its types (DM-61, ADR-N-033). Absent on a space created before DM-61, which `jcctl validate` warns about. It names a model of the space's own project: another project's model is reused through the LinkML `imports` of this one, read-only and at a pinned version. |
| `defaultLocale` | string | no | Default locale for entities and metadata within this space (PF-25). |
| `isSandbox` | boolean | no | Whether this context space is an ephemeral sandbox (CC-67, PF-19). |
| `missingUnitCode` | [MissingUnitCode](#missingunitcode) | no | What a write through an Endpoint does with a quantity Property of the model that carries no `unitCode`: `fill` writes the model's code and says so, `refuse` answers `400` (DM-06). A `unitCode` other than the model's is refused either way. |
| `personalData` | [SpacePersonalData](#spacepersonaldata) | no | The space's personal-data settings: the key a `keyedHash` slot of its model is hashed under (DM-121, ADR-N-054 2.6). |
| `quality` | list of [QualityRule](#qualityrule) | no | The space's own quality rules beside its model's constraints, at most 64 (DM-87, ADR-N-046). |
| `retention` | list of [RetentionRule](#retentionrule) | no | How long the broker keeps each type's data; a type without a rule is kept forever (DM-82, ADR-N-046). |
| `ttlDays` | integer (uint32) | no | Enforced time-to-live in days for ephemeral sandboxes (1..=14, PF-19). |
| `urnSegment` | string | no | The `{space}` segment of this space's entity ids, when it is not the rendered `{project}-{name}`; pins the segment of a space that predates PF-84 (PF-84). |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## MissingUnitCode

What a space does with a quantity written without its `unitCode` (DM-06).

Type: one of `"fill"`, `"refuse"`

## SpacePersonalData

A space's personal-data settings (DM-121).

| Field | Type | Required | Description |
|---|---|---|---|
| `keyRef` | [SecretRef](#secretref) | yes | The secret whose value is the HMAC-SHA-256 key of the space's `keyedHash` slots, at least 32 bytes; the reconciler hands it to the gateway, and a model never holds a key. |

## QualityRule

One named quality rule of a space (DM-87, ADR-N-046 2.4).

| Field | Type | Required | Description |
|---|---|---|---|
| `attribute` | string | yes | The slot of `entity_type` the rule reads. |
| `bbox` | list of number | no | `area`: `[minLon, minLat, maxLon, maxLat]` in WGS 84. |
| `entityType` | string | yes | A class of the space's Data Model. |
| `kind` | [QualityKind](#qualitykind) | yes | What the rule checks. |
| `max` | number | no | `range`: the largest value allowed. |
| `maxAge` | string | no | `freshness`: how old `observedAt` may be, checked by the sweep. |
| `maxAhead` | string | no | `freshness`: how far `observedAt` may lie in the future, checked on write. |
| `min` | number | no | `range`: the smallest value allowed. |
| `name` | string | yes | A DNS-1123 label, unique in the space: it names the rule in a refusal and the metric. |
| `outcome` | [QualityOutcome](#qualityoutcome) | no | Overrides the kind's default outcome. |

## RetentionRule

One type's retention in a space (DM-82, ADR-N-046): `current` absent keeps its current state; `history` absent leaves its history to the space's `*` rule, then the platform's (DM-97).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | map of [AttributeRecording](#attributerecording) | no | Per attribute of the type, its own recording over the type's (DM-108). |
| `current` | string | no | How long an entity not modified stays, as `PT5M`, `PT8H`, `P30D` or `P1DT12H` (DM-83). |
| `deadband` | number | no | With `onChange`, the numeric move a row needs, in the attribute's `unitCode` (DM-108). |
| `entityType` | string | yes | A class of the space's Data Model, or `*` for the space's default (DM-97). |
| `heartbeat` | string | no | With `onChange`, the longest gap between two rows of an attribute, PT1M to P3650D (DM-109). |
| `history` | string | no | How long a temporal instance is kept, in the same form (DM-84); `off` keeps the current state only and records no history, `forever` keeps it all and needs a `reason` (T-4275). |
| `minDistance` | number | no | With `onChange`, the move in metres a GeoProperty row needs (DM-108). |
| `reason` | string | no | Why the type keeps its whole history: required with `history: forever` (T-4275). |
| `record` | string | no | How the type records history: `every`, `observed`, `never` or `onChange` (DM-108); unset, the broker's default, `observed` on the platform (DM-132). |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## QualityKind

What a quality rule checks (DM-87).

Type: one of `"range"`, `"area"`, `"freshness"`

## QualityOutcome

What a broken quality rule does (DM-88, DM-89, DM-90).

Type: one of `"reject"`, `"flag"`, `"quarantine"`

## AttributeRecording

One attribute's recording under its type's retention rule (DM-108, ADR-N-054 2.2).

| Field | Type | Required | Description |
|---|---|---|---|
| `deadband` | number | no | As the type's, for this attribute. |
| `heartbeat` | string | no | As the type's, for this attribute. |
| `minDistance` | number | no | As the type's, for this attribute. |
| `record` | string | no | `every`, `observed`, `never` or `onChange`; absent takes the type's. |
