# ModelProjection

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ModelProjection`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/projections`
- Repository path: `projects/{project}/spaces/{space}/projections/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ModelProjectionSpec)

Desired specification of a `ModelProjection` resource (MP-01).

| Field | Type | Required | Description |
|---|---|---|---|
| `classes` | list of [ProjectedClass](#projectedclass) | yes | The exposed classes. A class not listed is not exposed. |
| `contextSpaceRef` | string | yes | DNS-1123 label of the owning Context Space; with `metadata.namespace` it derives the repository path (MF-06). |
| `dataModelRef` | [DataModelRef](#datamodelref) | yes | The model this is a subset of: one DataModel of the space at one served major (DM-22). |
| `filter` | [ProjectionFilter](#projectionfilter) | no | Optional residual filter. |

## ProjectedClass

One exposed class and the slots it keeps.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The class (NGSI-LD entity type short name) as the model names it. |
| `slots` | list of string | no | The slots kept. Empty means identity only: `id` and `type`, nothing else. |

## DataModelRef

Reference to a DataModel with name and semantic version (DM-33).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | no | Optional resource kind (must be `DataModel` if present) (MF-07). |
| `name` | string | yes | Referenced DataModel name (DNS-1123 label). |
| `version` | string | yes | Served major version of the referenced model, `"1"`, `"2"` … (DM-22, DM-33). |

## ProjectionFilter

A residual filter in NGSI-LD query syntax, intersected like a REWRITE constraint (GW10).

| Field | Type | Required | Description |
|---|---|---|---|
| `geoQ` | string | no | `geoQ` as one string (`georel`, `geometry`, `coordinates` joined with `;`). |
| `q` | string | no | `q` (CIM 009 clause 4.9). |
| `scopeQ` | string | no | `scopeQ` (CIM 009 clause 4.19). |
| `temporalQ` | string | no | `temporalQ` as one string (`timerel`, `timeAt`, `endTimeAt`, `timeproperty` joined with `;`). |
