# Role

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Role`
- Lives in the Organization (`org`) or a project
- API: `/api/v1/projects/{project}/roles`
- Repository path: `users/roles/{name}.yaml`
- Repository path in a project: `projects/{project}/roles/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (RoleSpec)

`spec` of a Role (PF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `rules` | list of [Rule](#rule) | yes | The rules; a role with none grants nothing and is refused. |

## Rule

One rule: the verbs allowed on the kinds, under the constraints.

| Field | Type | Required | Description |
|---|---|---|---|
| `constraints` | list of [Constraint](#constraint) | no | Constraints every covered manifest must satisfy for the rule to apply. |
| `kinds` | list of string | yes | Manifest kinds the rule covers, e.g. `Pipeline`. |
| `verbs` | list of [Verb](#verb) | yes | Verbs granted on those kinds. |

## Constraint

A constraint on one spec field or on `metadata.name`; exactly one of `in`, `notIn`, `equals`, `pattern` is given.

The one exception is [`BUILD_FIELD`] with no operator, on a rule of `App` with `propose` alone: it names the rule as the writer of that field, and a rule carrying it authorizes nothing else (AP-73).

| Field | Type | Required | Description |
|---|---|---|---|
| `equals` | string | no | The value must equal this. |
| `field` | string | yes | Dotted path from the manifest root, e.g. `spec.audience` or `metadata.name`. |
| `in` | list of string | no | The value must be one of these. |
| `notIn` | list of string | no | The value must be none of these. |
| `pattern` | string | no | The whole value must match this regular expression, at most [`PATTERN_MAX`] characters (T-2627): `t1[0-9]{3}-.+` confines a rule to the names a journey gives what it makes. |

## Verb

What a rule allows on its kinds (PF-49, PF-59).

Type: one of `"read"`, `"propose"`, `"approve"`, `"delete"`, `"create"`, `"update"`, `"disable"`
