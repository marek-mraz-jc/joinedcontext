# SharedSpaceReference

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `SharedSpaceReference`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/shared`
- Repository path: `projects/{project}/shared/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (SharedSpaceReferenceSpec)

Desired specification of a `SharedSpaceReference` resource (EP-15).

| Field | Type | Required | Description |
|---|---|---|---|
| `alias` | string | yes | Local alias name for the remote context space. |
| `cachedTokenSecretRef` | [SecretRef](#secretref) | no | Optional reference to cached access token in secret store. |
| `endpointRef` | [EndpointRef](#endpointref) | no | The source Endpoint by project and name, inside one Organization repository; the loader resolves it to this environment's slug (EP-77). |
| `endpointSlug` | [EndpointSlug](#endpointslug) | no | Opaque slug of the target endpoint, for a source in another instance (EP-77). |
| `schedule` | [Schedule](#schedule) | no | How often the peer's schema surface is mirrored; 24 hours when absent (DM-49). |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## EndpointRef

An Endpoint named by where it lives rather than by its slug (EP-77).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The Endpoint's name in that project. |
| `project` | string | yes | The project the Endpoint belongs to. |

## EndpointSlug

Cryptographically random endpoint slug with at least 128 bits of entropy (lowercase RFC 4648 base32 without padding, >=26 chars, EP-02)

Type: string

## Schedule

How often a [`SyncSourceSpec`] runs (MF-28): `{ interval: 30m }` or `{ webhook: true }`.

Exactly one of the two members is set, for the same reason as [`SyncOrigin`].

| Field | Type | Required | Description |
|---|---|---|---|
| `interval` | string | no | Fixed interval such as `30m`, `6h` or `1d` (suffixes `s`, `m`, `h`, `d`). |
| `webhook` | boolean | no | Driven by a webhook from the source instead of a timer; only `true` is a schedule. |
