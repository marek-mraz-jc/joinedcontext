# ScopeDefinition

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `ScopeDefinition`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/scopedefinitions`
- Repository path: `projects/{project}/policies/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ScopeDefinitionSpec)

Desired specification of a `ScopeDefinition` resource (ADR 005, R19, R29).

| Field | Type | Required | Description |
|---|---|---|---|
| `isChildOf` | string | no | Parent scope path (absent if and only if `is_root` is true). |
| `isRoot` | boolean | no | Whether this definition represents a taxonomy root (`/geo`, `/domain`, `/admin`). |
| `scopeString` | string | yes | Multidimensional hierarchical scope path (e.g. `/geo/SK/BB/Radvan`). |
