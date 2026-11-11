# Subscription

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Subscription`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/subscriptions`
- Repository path: `projects/{project}/spaces/{space}/subscriptions/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (SubscriptionSpec)

Desired specification of a `Subscription` resource (CC-72, DS-16).

| Field | Type | Required | Description |
|---|---|---|---|
| `contextSpaceRef` | [Ref](#ref) | yes | The Context Space whose broker holds the subscription. It is written into this space's tenant and no other (SP-08, SP-09). |
| `description` | string | no | Why it exists, for whoever reads the repository later. |
| `entities` | list of [WatchedEntity](#watchedentity) | no | What is watched. Empty means "every type", which is only allowed together with `watchedAttributes`. |
| `expiresAt` | string (date-time) | no | When the subscription stops, if it is not open-ended. |
| `geoQ` | string | no | The area an entity must be in, as a `geoQ` (`georel`, `geometry`, `coordinates`). |
| `isActive` | boolean | no | Whether the broker delivers; a subscription written with `isActive: false` is declared and silent, which is how one is parked without deleting it. |
| `notification` | [Notification](#notification) | yes | Where the notifications go. |
| `q` | string | no | The condition an entity must satisfy, in the query language of CIM 009 clause 4.9. |
| `subscriptionName` | string | no | What a person calls it in the Portal. |
| `throttling` | integer (uint32) | no | The shortest interval between two notifications, in seconds. |
| `watchedAttributes` | list of string | no | The attributes whose change fires the subscription; any attribute when empty. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## WatchedEntity

One selector of the entities a subscription watches (CIM 009 clause 5.2.12).

Not `policy::EntitySelector`, which requires a type: a subscription may watch one entity by its URN, and a grant may not be written that loosely.

Either a type, an id, or an id pattern; the broker's own shape, so a query written against the API and a subscription written in a manifest select with the same words.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | no | One entity, by its URN. |
| `idPattern` | string | no | Every entity whose id matches this anchored regular expression. |
| `type` | string | no | The entity type, as the model names it. |

## Notification

What is sent when the subscription fires (CIM 009 clause 5.2.14).

| Field | Type | Required | Description |
|---|---|---|---|
| `attributes` | list of string | no | The attributes the notification carries; every granted one when absent. |
| `endpoint` | [NotificationEndpoint](#notificationendpoint) | yes | Where it goes. |
| `format` | string | no | `normalized`, `concise` or `keyValues`, as the specification names them; the broker's default when absent, which is why this is a string and not an enum of ours. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## NotificationEndpoint

Where a notification goes and how it is addressed (CIM 009 clause 5.2.15).

| Field | Type | Required | Description |
|---|---|---|---|
| `accept` | string | no | The media type the receiver wants; the broker's default when absent. |
| `receiverInfo` | list of [KeyValue](#keyvalue) | no | Headers the receiver needs, values only — a credential belongs in `secretRef`. |
| `secretRef` | [SecretRef](#secretref) | no | The credential the notification carries, resolved by the reconciler at apply time and never written into the manifest (MF-31, PF-36). |
| `uri` | string | yes | The address the platform posts to. `https` outside the cluster, `http` only for a Service name inside it. |

## KeyValue

One `key`/`value` pair, as CIM 009 writes `receiverInfo` and `notifierInfo`.

| Field | Type | Required | Description |
|---|---|---|---|
| `key` | string | yes | The header name. |
| `value` | string | yes | The header value. |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |
