# DelegationPolicy

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `DelegationPolicy`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/delegationpolicies`
- Repository path: `users/delegation/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (DelegationPolicySpec)

Desired specification of a `DelegationPolicy` (AG-150).

| Field | Type | Required | Description |
|---|---|---|---|
| `aboveCeiling` | [AboveCeiling](#aboveceiling) | no | A request above the ceiling is refused (default) or queued for an administrator. |
| `allowedClients` | list of string | no | The OAuth clients a connector may name (a CIMD URL or a realm client id); absent: any client of the realm. |
| `endpoints` | list of string | no | The Endpoints a connector may name; absent: the projects' Endpoints the person may read. |
| `kinds` | list of [KindVerbs](#kindverbs) | no | Verbs per kind; absent: `read` on the kinds the segments reach, nothing more. |
| `maxExpiry` | string | no | The longest expiry, an ISO 8601 duration in days such as `P90D`, at most `P365D`; absent: `P30D`. |
| `personalData` | boolean | no | Whether a connector of theirs may tick `personalData` on a scope item, keeping the attributes a model marks personal in its answers (AG-158); default false. |
| `projects` | list of string | yes | The projects a connector may name, at least one; `*` is refused, list them. |
| `segments` | list of [Segment](#segment) | no | The segments a connector may carry; absent: `data-read` only. |
| `subjects` | list of [DelegationSubject](#delegationsubject) | yes | Who the policy binds: at least one Group or user. |
| `writes` | boolean | no | Whether a connector of theirs may have writes on. |

## AboveCeiling

What happens to a connector request above the ceiling (AG-150).

Type: one of `"refuse"`, `"queue"`

## KindVerbs

The verbs a connector may carry on one kind.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | A kind of the platform registry, such as `Pipeline`. |
| `verbs` | list of [ConnectorVerb](#connectorverb) | yes | At least one of `read`, `create`, `edit`, `delete`, `run`; `propose` stands for `create`, `edit` and `delete` together. |

## Segment

A segment, the preset a person ticks when creating a connector (Architecture/07 §11).

Type: one of `"data-read"`, `"context"`, `"pipeline"`, `"access"`, `"publish"`, `"apps"`, `"operate"`, `"knowledge"`, `"admin"`

## DelegationSubject

One person or Group a policy binds (AG-150). The `kind` tag, `Group` or `User`, selects which.

Type: object or object

## ConnectorVerb

What a connector may do on a kind: the registry's verb set (API/14 §4). No verb approves, merges or decides: those doors refuse every connector (CC-41).

Type: one of `"read"`, `"propose"`, `"create"`, `"edit"`, `"delete"`, `"run"`
