# Bundle

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Bundle`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/bundles`
- Repository path: `bundle.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (BundleSpec)

Desired specification of a `Bundle` download index (MF-17, MF-18).

A Bundle is generated on download and never stored in the repository; it travels inside the archive so the import wizard knows what it received and from where.

| Field | Type | Required | Description |
|---|---|---|---|
| `exportedAt` | string (date-time) | yes | When the bundle was produced. |
| `exportedBy` | string | yes | Who produced it (a user or ServiceAccount name). |
| `files` | list of [BundleFile](#bundlefile) | no | The SHA-256 of every file the bundle holds, over the bytes as exported (MF-42). An import compares what it wrote against these, so a transfer between instances is verified before the source is deleted. A bundle from an older exporter carries none, and an import then reports nothing verified rather than reporting everything equal. |
| `items` | list of [BundleItem](#bundleitem) | yes | The exported resources. |
| `models` | list of [BundleModel](#bundlemodel) | no | The organization models a git-native export carries because the project imports them (MF-49): schema files only, each listed in `files` too. |
| `nativeFiles` | list of string | no | Native files carried next to their envelopes (`bento.yaml`, `*.linkml.yaml`) (MF-17). |
| `omitted` | integer (uint32) | no | How many resources the caller was not allowed to read; names are never disclosed (MF-18). |
| `readme` | string | no | What the bundle holds, in prose: the text of the `README.md` a complete export carries at its root, so a stream export says the same thing as an archive (MF-41). |
| `repositories` | list of [BundleRepository](#bundlerepository) | no | The git bundles of a git-native export, one per repository (MF-45): the project's and one per application of it. An import verifies each by its head commit (MF-46). |
| `schemas` | [BundleSchemas](#bundleschemas) | no | The schemas a complete export carries beside its manifests (MF-41). |
| `sourceInstance` | string | no | Base URL of the instance it came from, so a re-import can be traced (MF-25). |
| `sourceRevision` | string | yes | Git revision the export was taken at, 7 to 40 lowercase hexadecimal characters. |

## BundleFile

One file of a bundle and the checksum a transfer is verified against (MF-42).

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Path inside the bundle, as the archive carries it. |
| `sha256` | string | yes | Lowercase hexadecimal SHA-256 of the file's bytes as exported. |

## BundleItem

One resource listed in a [`BundleSpec`] index.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Manifest kind of the exported resource; must be a kind this crate knows. |
| `name` | string | yes | `metadata.name` of the resource. |
| `namespace` | string | no | Namespace of the resource, absent for organization-scoped kinds. |
| `path` | string | yes | Path of the manifest inside the bundle archive. |

## BundleModel

One organization model a project export carries (MF-49, ADR-N-039 §3.4).

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | Its LinkML source, relative to the index, under `models/`. |
| `manifest` | string | yes | Its `DataModel` manifest, relative to the index, under `models/`. |
| `name` | string | yes | The model's name in the organization it came from. |
| `origin` | [BundleModelOrigin](#bundlemodelorigin) | yes | Where it came from. |
| `sha256` | string | yes | Lowercase hexadecimal SHA-256 of the LinkML source: what an import compares a destination model's source with (MF-50). |
| `version` | [SemVer](#semver) | yes | The version the organization held when the project was exported. |

## BundleRepository

One repository of a git-native export (MF-45).

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | string | yes | The `git bundle` file, relative to the index. |
| `head` | string | yes | The commit the repository's default branch ends at, 40 lowercase hexadecimal characters. |
| `name` | string | yes | The repository's name, and the name it is created under at the target. |
| `role` | [BundleRole](#bundlerole) | yes | What the repository is. |

## BundleSchemas

The schemas of a complete export: one per kind in the bundle, and the LinkML source and JSON Schema of every `DataModel` it holds (MF-41).

| Field | Type | Required | Description |
|---|---|---|---|
| `kinds` | object | no | Kind name to its JSON Schema (draft-07). |
| `models` | object | no | DataModel name to `{linkml, jsonSchema}`; a member is absent when neither the repository nor Model Tools could produce it. |

## BundleModelOrigin

The organization a carried model belongs to, and its name there (MF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The model's name in that organization. |
| `organization` | string | yes | The exporting organization's `metadata.name`. |

## SemVer

Semantic version string conforming to major.minor.patch (DM-22)

Type: string

## BundleRole

What a repository of a git-native export is (MF-45, PF-85).

Type: one of `"organization"`, `"project"`, `"application"`
