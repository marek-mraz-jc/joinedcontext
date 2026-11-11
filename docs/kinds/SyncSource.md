# SyncSource

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `SyncSource`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/syncsources`
- Repository path: `projects/{project}/sync/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (SyncSourceSpec)

Desired specification of a `SyncSource` resource (MF-27..MF-32).

| Field | Type | Required | Description |
|---|---|---|---|
| `autoMerge` | boolean | no | Whether a clean sync merge request merges itself (CC-70, MF-29). |
| `conflictPolicy` | [ConflictPolicy](#conflictpolicy) | yes | What to do when a resource already exists locally (MF-23). |
| `mode` | [SyncMode](#syncmode) | yes | Mirror the source continuously or copy it once. |
| `prune` | boolean | no | Whether resources missing from the source are deleted; deletions are never implied (CC-19). |
| `schedule` | [Schedule](#schedule) | yes | How often the run happens (MF-28). |
| `selector` | map of string | no | Label selector narrowing which resources of the source are synced (MF-10). |
| `source` | [SyncOrigin](#syncorigin) | yes | Where the resources come from (MF-27). |
| `webhook` | [WebhookAuth](#webhookauth) | no | What authorises a run through the webhook route (MF-44). Required by, and only meaningful with, `schedule: { webhook: true }`. |

## ConflictPolicy

What an import does when a resource already exists (MF-23).

Type: one of `"fail"`, `"skip"`, `"replace"`, `"rename"`

## SyncMode

What a sync run does with the synced subtree.

Type: one of `"mirror"`, `"oneshot"`

## Schedule

How often a [`SyncSourceSpec`] runs (MF-28): `{ interval: 30m }` or `{ webhook: true }`.

Exactly one of the two members is set, for the same reason as [`SyncOrigin`].

| Field | Type | Required | Description |
|---|---|---|---|
| `interval` | string | no | Fixed interval such as `30m`, `6h` or `1d` (suffixes `s`, `m`, `h`, `d`). |
| `webhook` | boolean | no | Driven by a webhook from the source instead of a timer; only `true` is a schedule. |

## SyncOrigin

What a [`SyncSourceSpec`] follows (MF-27).

Exactly one of the three members is set. This is the shape the docs print (`source: { git: { … } }`) and, unlike an externally tagged Rust enum, it survives the YAML round-trip without a `!Git` tag.

| Field | Type | Required | Description |
|---|---|---|---|
| `bundle` | [BundleOrigin](#bundleorigin) | no | A published bundle URL (MF-17). |
| `git` | [GitOrigin](#gitorigin) | no | A remote Git repository, optionally a subtree of it. |
| `platformApi` | [PlatformApiOrigin](#platformapiorigin) | no | Another instance's public resource API (MF-32). |

## WebhookAuth

The inbound credential of a webhook-driven [`SyncSourceSpec`] (MF-44).

The signature the origin sends covers the request body and not the path it posts to, so one secret shared between sources would let the origin of any one of them force a run of every other, in projects it has no binding in. Each source therefore carries its own, by reference like every other credential (MF-24): what a department is handed opens their door and nothing else.

| Field | Type | Required | Description |
|---|---|---|---|
| `previousSecretRef` | [SecretRef](#secretref) | no | The secret being retired, accepted alongside the current one so that moving the origin's own hook to the new value is a separate step from writing it here — without a window, every rotation is an outage and nobody rotates (T-0982). |
| `secretRef` | [SecretRef](#secretref) | yes | The secret the origin signs the request body with, HMAC-SHA256. |

## BundleOrigin

A published bundle URL as a sync origin (MF-17).

| Field | Type | Required | Description |
|---|---|---|---|
| `secretRef` | [SecretRef](#secretref) | no | Credential for a protected bundle URL (MF-31). |
| `url` | string | yes | URL of the bundle archive or `kind: Bundle` index; `https://` only. |

## GitOrigin

A remote Git repository as a sync origin.

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Subtree to sync; the whole repository when absent. |
| `ref` | string | yes | Branch, tag or commit to follow. |
| `secretRef` | [SecretRef](#secretref) | no | Read-only credential, resolved by the reconciler (MF-31). |
| `url` | string | yes | Clone URL; `https://` or an `ssh://`/`git@` remote, never plain `http://`. |

## PlatformApiOrigin

Another platform instance's public resource API as a sync origin (MF-32).

| Field | Type | Required | Description |
|---|---|---|---|
| `baseUrl` | string | yes | Base URL of the remote instance, `https://` only. |
| `project` | string | yes | Project slug to follow on the remote instance. |
| `secretRef` | [SecretRef](#secretref) | no | Credential for the remote resource API (MF-31). |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |
