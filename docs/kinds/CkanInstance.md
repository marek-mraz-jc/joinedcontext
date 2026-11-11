# CkanInstance

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `CkanInstance`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/ckaninstances`
- Repository path: `projects/{project}/ckan/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (CkanInstanceSpec)

Desired specification of a `CkanInstance` resource (EP-62).

| Field | Type | Required | Description |
|---|---|---|---|
| `apiTokenRef` | [SecretRef](#secretref) | yes | Where the API token lives; never the token itself (EP-67, CC-06). |
| `organizationDefault` | string | no | The organization slug a dataset lands in when the Endpoint names none. |
| `url` | string | yes | Base URL of the CKAN instance, `https://` only. |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |
