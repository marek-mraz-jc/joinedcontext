# DataModel

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DataModel`
- Lives in the Organization (`org`) or a project
- API: `/api/v1/projects/{project}/datamodels`
- Repository path: `datamodels/{name}/{name}.yaml`
- Repository path in a project: `projects/{project}/spaces/{space}/datamodels/{name}.yaml`
- Repository path without a space: `projects/{project}/datamodels/{name}/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DataModelSpec)

Desired specification of a `DataModel` resource (DM-01..DM-53).

| Field | Type | Required | Description |
|---|---|---|---|
| `artifacts` | [GeneratedArtifacts](#generatedartifacts) | no | Artifacts generated beside the source in the same commit (DM-02). |
| `classes` | list of string | no | NGSI-LD entity types this model defines. |
| `consumers` | list of [TypedRef](#typedref) | no | Informational list of resources consuming this version (DM-25). |
| `contextSpaceRef` | string | no | DNS-1123 label of the owning ContextSpace; with `metadata.namespace` it derives the path (MF-06). Absent on a model no space owns: an organization model, or a project model at `projects/{p}/datamodels/{name}/` (DM-75). |
| `lifecycle` | [DataModelLifecycle](#datamodellifecycle) | yes | Lifecycle state of this version (DM-26, DM-48). |
| `linkml` | string | yes | Path of the authoring LinkML source, relative to this manifest and ending `.linkml.yaml` (DM-01). |
| `openWorld` | boolean | no | Whether entities of this model may carry undeclared attributes (DM-28). |
| `origin` | [DataModelOrigin](#datamodelorigin) | no | Where an organization model was promoted from (DM-77); kept on a model a bundle lands (MF-50). |
| `source` | [DataModelSource](#datamodelsource) | no | Provenance; absent for a hand-authored model (DM-08, DM-48). |
| `version` | [SemVer](#semver) | yes | Semantic version; the major is the served `schema/v{major}` (DM-22). |

## GeneratedArtifacts

Artifacts generated beside the LinkML source and committed in the same change (DM-02).

| Field | Type | Required | Description |
|---|---|---|---|
| `context` | string | no | Generated JSON-LD `@context` (DM-02, DM-05). |
| `docs` | string | no | Generated Markdown documentation (DM-02). |
| `example` | string | no | Generated and validated example entity (DM-02, DM-21). |
| `jsonSchema` | string | no | Generated JSON Schema draft-07 (DM-02, DM-03). |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## DataModelLifecycle

Lifecycle state of a DataModel version (DM-26, DM-48).

Type: one of `"draft"`, `"published"`, `"deprecated"`, `"retired"`, `"mirrored"`

## DataModelOrigin

Where an organization model was promoted from (DM-77), recorded and never linked: the organization owns the copy, and a project bundle keeps it on a model it lands (MF-50).

| Field | Type | Required | Description |
|---|---|---|---|
| `commit` | string | yes | Commit of the project repository the source was copied at, 7 to 40 lowercase hexadecimal characters. |
| `name` | string | yes | Name of the model in that project. |
| `project` | string | yes | Project the model was promoted from. |
| `space` | string | no | Context Space whose model it was; absent for a project model no space owns. |
| `version` | [SemVer](#semver) | yes | Version that was promoted. |

## DataModelSource

Provenance of a data model, absent for a hand-authored one (DM-08, DM-48).

Either the upstream `repository`/`path`/`commit` triple of an imported model or the `remote` block of a mirrored foreign model — never both.

| Field | Type | Required | Description |
|---|---|---|---|
| `commit` | string | no | Pinned upstream commit, 7 to 40 lowercase hexadecimal characters (DM-08). |
| `path` | string | no | Path inside the upstream repository (DM-08). |
| `remote` | [RemoteSource](#remotesource) | no | Remote endpoint a foreign model was mirrored from (DM-48). |
| `repository` | string | no | Upstream repository the model was imported from (DM-08). |

## SemVer

Semantic version string conforming to major.minor.patch (DM-22)

Type: string

## RemoteSource

Where a mirrored foreign data model came from (DM-48, DM-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `fetchedAt` | string (date-time) | yes | When the fetch happened. |
| `sha256` | string | yes | SHA-256 of the fetched source, 64 lowercase hexadecimal characters; a change opens a merge request. |
| `url` | string | yes | Schema surface the model was fetched from; `https://` only. |
| `version` | [SemVer](#semver) | yes | Version the peer published. |
