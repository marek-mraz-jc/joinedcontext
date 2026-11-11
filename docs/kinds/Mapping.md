# Mapping

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Mapping`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/mappings`
- Repository path: `projects/{project}/spaces/{space}/datamodels/mappings/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (MappingSpec)

Desired specification of a `Mapping` resource (DM-33..DM-42).

| Field | Type | Required | Description |
|---|---|---|---|
| `artifacts` | [MappingArtifacts](#mappingartifacts) | no | The compiled artifacts committed beside this specification (DM-52). |
| `contextSpaceRef` | string | yes | Context Space name to which this mapping belongs (PF-09, DM-33). |
| `native` | list of [NativeBlock](#nativeblock) | no | Native Bloblang escape hatch blocks (DM-38). |
| `source` | [DataModelRef](#datamodelref) | yes | Source data model reference (DM-33). |
| `target` | [DataModelRef](#datamodelref) | yes | Target data model reference (DM-33). |
| `tests` | list of [MappingTest](#mappingtest) | yes | Golden test cases asserting input entity produces expected output entity (DM-39). |
| `transformation` | any | yes | Verbatim LinkML-Map TransformationSpecification payload (DM-33). |
| `vocabularyAlignment` | [VocabularyAlignment](#vocabularyalignment) | no | Optional SSSOM alignment set used for editor pre-fill and documentation (DM-42). |

## MappingArtifacts

The two compiled forms of one transformation specification (DM-52).

Both are committed beside the manifest and both are reviewed, named rather than derived for the same reason DM-02 names a DataModel's artifacts: what runs is what a reviewer approved, not what a compiler would produce now. A Mapping used only in replicate mode may carry the Bloblang alone; live translation and a view Endpoint need the IR.

| Field | Type | Required | Description |
|---|---|---|---|
| `bloblang` | string | no | The Bloblang mapping Bento runs, relative to the manifest (DM-35, DM-52). |
| `gatewayIr` | string | no | The gateway mapping IR the Context Gateway interprets, relative to the manifest (DM-51, DM-52, EP-54). |

## NativeBlock

Native code escape hatch block for logic LinkML-Map cannot express (DM-38).

| Field | Type | Required | Description |
|---|---|---|---|
| `language` | [NativeLanguage](#nativelanguage) | yes | Native language of the code block (DM-37, DM-38). |
| `source` | string | yes | Source code of the native processor (non-empty, DM-38). |
| `targetSlot` | string | yes | Target slot identifier to which this native block is attached (`^A-Za-z_*$`, DM-38). |

## DataModelRef

Reference to a DataModel with name and semantic version (DM-33).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | no | Optional resource kind (must be `DataModel` if present) (MF-07). |
| `name` | string | yes | Referenced DataModel name (DNS-1123 label). |
| `version` | string | yes | Served major version of the referenced model, `"1"`, `"2"` … (DM-22, DM-33). |

## MappingTest

Golden test case asserting input entity produces expected output entity (DM-39).

| Field | Type | Required | Description |
|---|---|---|---|
| `expect` | string | yes | Path of the expected output example, relative to the manifest (DM-39). |
| `input` | string | yes | Path of the input example, relative to the manifest (DM-39). |

## VocabularyAlignment

Vocabulary alignment set backing the editor's mapping pre-fill (DM-42).

| Field | Type | Required | Description |
|---|---|---|---|
| `sssomRef` | [TypedRef](#typedref) | yes | SSSOM mapping set carrying the alignment; never executed as a transformation (DM-42). |

## NativeLanguage

Supported native transformation language (DM-37, DM-38).

Type: `"bloblang"`

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |
