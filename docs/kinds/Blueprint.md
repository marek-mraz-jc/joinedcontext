# Blueprint

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Blueprint`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/blueprints`
- Repository path: `blueprints/{name}/blueprint.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (BlueprintSpec)

`spec` of a Blueprint (CC-23…CC-27, CC-59).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedRoles` | list of string | yes | Roles that may see and run this blueprint (CC-59, CC-30). |
| `category` | string | no | Gallery grouping, e.g. `alerting` or `onboarding` (CC-30). |
| `parameterSchema` | any | yes | JSON Schema draft-07 of the parameters: the complete user-facing surface (CC-24). |
| `riskClass` | [RiskClass](#riskclass) | yes | The merge lane changes from this blueprint take (CC-59, CC-63). |
| `templates` | list of [BlueprintTemplate](#blueprinttemplate) | yes | The templates this blueprint expands into (CC-23). |
| `version` | [SemVer](#semver) | yes | Blueprint version; an upgrade never silently rewrites instances (CC-26). |

## RiskClass

The review lane a blueprint's changes take (CC-59, CC-63).

Type: one of `"green"`, `"yellow"`, `"red"`

## BlueprintTemplate

One template, rendering exactly one manifest (CC-23).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Stable name of this template; it names the rendered file and appears in diagnostics. |
| `template` | string | yes | The minijinja (Jinja2) source, rendered in a sandbox with no filesystem, no environment and strict undefined values (ADR-N-005). |

## SemVer

Semantic version string conforming to major.minor.patch (DM-22)

Type: string
