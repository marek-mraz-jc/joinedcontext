# Script

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Script`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/scripts`
- Repository path: `projects/{project}/scripts/{name}/script.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ScriptSpec)

Desired specification of a `Script` resource (PL-118).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | What the script does, for the people who pin it, e.g. `Converts S-JTSK to WGS84`. |
| `language` | [ScriptLanguage](#scriptlanguage) | yes | The language of every version; `javascript` is the only one. |
| `versions` | list of [ScriptVersion](#scriptversion) | yes | Every version, oldest first, e.g. `1.0.0` then `1.1.0`. |

## ScriptLanguage

The language a Script is written in.

Type: `"javascript"`

## ScriptVersion

One version of a Script (PL-118).

| Field | Type | Required | Description |
|---|---|---|---|
| `code` | string | yes | The source, at most 64 KiB, using `bento.v0_msg_as_structured()` and the other v0 calls. |
| `limits` | [ScriptLimits](#scriptlimits) | no | Time and memory per message; absent, the runner's defaults (50 ms, 32 MiB). |
| `sha256` | string | yes | The lower-case hex SHA-256 of `code`; the reconciler renders nothing on a mismatch. |
| `state` | [ScriptState](#scriptstate) | yes | `draft` or `published`. |
| `tests` | list of [ScriptTest](#scripttest) | no | Input records and the records the script must make of them. |
| `version` | string | yes | `MAJOR.MINOR.PATCH`, e.g. `1.1.0`. |

## ScriptLimits

Per-message limits of a version (PL-118).

| Field | Type | Required | Description |
|---|---|---|---|
| `memoryMb` | integer (uint32) | no | MiB per message, 1 to 64, e.g. `32`. |
| `timeoutMs` | integer (uint32) | no | Milliseconds per message, 1 to 200, e.g. `50`. |

## ScriptState

Whether a version may be pinned.

Type: one of `"draft"`, `"published"`

## ScriptTest

One test of a version: what goes in and what must come out.

| Field | Type | Required | Description |
|---|---|---|---|
| `expected` | any | yes | The message the script must produce, as JSON. |
| `input` | any | yes | The input message, as JSON. |
| `name` | string | yes | A short name, e.g. `converts-one-point`. |
