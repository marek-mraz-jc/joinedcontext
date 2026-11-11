# Environment

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Environment`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/environments`
- Repository path: `environments/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (EnvironmentSpec)

Desired specification of an `Environment` (CC-73).

| Field | Type | Required | Description |
|---|---|---|---|
| `features` | map of boolean | no | Feature flags of this environment, e.g. `publicEndpoints: false`. |
| `hosts` | map of string | no | The hosts of this environment by component, e.g. `portal: portal.staging.bb.example`. |
| `images` | map of string | no | The image digest each component runs here, by name; a digest, never a tag. |
| `orgDomain` | string | no | The organization's domain in this environment; every `{orgDomain}` of every manifest is rendered with it before validation (CC-74). |
| `secrets` | [SecretBackend](#secretbackend) | no | Where a `secretRef` is resolved in this environment; a backend, never a value (CC-75). |

## SecretBackend

Where a `secretRef` is resolved in this environment (CC-06, CC-75).

| Field | Type | Required | Description |
|---|---|---|---|
| `backend` | string | yes | The backend that answers: `openbao`, `kubernetes`, `sops`. |
| `mount` | string | no | Where inside it, e.g. `jc/staging`. |
