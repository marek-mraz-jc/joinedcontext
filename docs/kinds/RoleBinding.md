# RoleBinding

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `RoleBinding`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/rolebindings`
- Repository path: `users/assignments/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (RoleBindingSpec)

`spec` of a RoleBinding (PF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `role` | string | yes | The `Role` (its `metadata.name`). |
| `scope` | [RoleScope](#rolescope) | yes | Where the role applies: exactly one of `organization`, `project`, `contextSpace`. |
| `subjects` | list of [Subject](#subject) | yes | Who gets the role. |
| `validity` | [BindingValidity](#bindingvalidity) | no | When the binding applies. |

## RoleScope

Target scope of a role grant: exactly one of `contextSpace`, `project`, or `organization`.

| Field | Type | Required | Description |
|---|---|---|---|
| `contextSpace` | string | no | Context Space scope target. |
| `organization` | string | no | Organization scope target. |
| `project` | string | no | Project scope target. |

## Subject

One human or group a binding names; exactly one of `user`, `group` (PF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `group` | string | no | A group: a `Group` manifest of the organization unless `source` says otherwise (PF-64). |
| `source` | [SubjectSource](#subjectsource) | no | Who owns `group`'s members: absent for a `Group` manifest, `provider` for a group the identity provider owns, which the reconciler never manages. Only a `RoleBinding` subject carries it, and only with `group` (PF-63, PF-64). |
| `user` | string | no | The person's Keycloak user id, the `sub` of their token: a lower-case UUID, never an e-mail or a username (PF-126); or a client's own `service-account-<client>` user. |

## BindingValidity

When a binding applies; absent bounds are open.

| Field | Type | Required | Description |
|---|---|---|---|
| `notAfter` | string (date-time) | no | Not in force after this instant (UTC). |
| `notBefore` | string (date-time) | no | Not in force before this instant (UTC). |

## SubjectSource

Who owns the members of a subject's group (PF-64).

Type: `"provider"`
