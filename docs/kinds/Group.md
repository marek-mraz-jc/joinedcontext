# Group

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Group`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/groups`
- Repository path: `users/groups/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (GroupSpec)

Desired specification of a `Group` resource (PF-62).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | What the group is, for whoever reviews a binding that names it. |
| `members` | list of [Member](#member) | no | Everyone in the group; empty is legal, and means the bindings that name it match nobody until somebody is added. |

## Member

One member of a `Group` (PF-62).

| Field | Type | Required | Description |
|---|---|---|---|
| `user` | string | yes | The person's Keycloak user id, the `sub` of their token: a lower-case UUID such as `3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14` (PF-126). |
