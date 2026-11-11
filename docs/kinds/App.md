# App

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `App`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/apps`
- Repository path: `projects/{project}/apps/{name}/app.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (AppSpec)

Desired specification of an `App` resource (AP-01..AP-20).

| Field | Type | Required | Description |
|---|---|---|---|
| `access` | list of [AppAccess](#appaccess) | no | Who holds each role (AP-91). |
| `build` | map of string | yes | Toolchain versions the build lane builds with; `{}` on a static app is no build step (AP-11, AP-83). |
| `csp` | [ContentSecurityPolicy](#contentsecuritypolicy) | no | Content Security Policy of the served app (AP-12). |
| `dataNeeds` | list of [DataNeed](#dataneed) | yes | What the app needs to read or write; the reconciler renders its Endpoint and Policies from this (AP-04, AP-05). |
| `egress` | list of [AppEgress](#appegress) | no | Where a server pod may connect besides its endpoint, by address only; declaring one takes the red lane (AP-134). |
| `embeddable` | boolean | no | Whether the Portal may embed the app in a frame (AP-12). |
| `hostnames` | list of string | no | Names of the Organization's own domain the App also answers on, besides its host under the installation (AP-172); the Portal checks each against the Organization's verified domain and the other Apps. |
| `kind` | [AppClass](#appclass) | yes | How the app is built and served; `spec.kind` in the manifest (AP-01). |
| `lifecycle` | [AppLifecycle](#applifecycle) | no | Lifecycle state; a manifest without one is a draft (AP-18). |
| `limits` | [AppLimits](#applimits) | no | Per-app rate limits (AP-17). |
| `roles` | list of [AppRole](#approle) | no | The roles a person can hold in the app (AP-90). |
| `server` | [AppServer](#appserver) | no | What a `wasm` App's server part runs on a schedule (AP-154). |
| `services` | list of [AppService](#appservice) | no | The platform services the App uses beyond identity and data, each once (AP-160). |
| `source` | [AppSource](#appsource) | yes | Where the source lives (AP-02). |
| `storage` | [AppStorage](#appstorage) | no | The database schema and the storage prefix of a `wasm` App (AP-148). |
| `visibility` | [AppVisibility](#appvisibility) | no | Who may reach the published app (AP-18); `project` when the manifest does not say, so an app nobody opened up asks for a login (AP-120). |

## AppAccess

Who holds one role of an App (AP-91).

| Field | Type | Required | Description |
|---|---|---|---|
| `role` | string | yes | A role of `spec.roles`. |
| `subjects` | list of [Subject](#subject) | yes | `user` by e-mail, `group` by the name of a `Group` manifest (PF-62, PF-64). |

## ContentSecurityPolicy

Content Security Policy of a served app (AP-12).

| Field | Type | Required | Description |
|---|---|---|---|
| `connectSrc` | list of string | no | `connect-src`; only `self` and https origins, never `*` (AP-12). |
| `frameAncestors` | list of string | no | `frame-ancestors`; defaults to `none` (AP-12). |
| `frameSrc` | list of string | no | `frame-src` beyond `self`; only https origins, never `*` (AP-12). |

## DataNeed

One declared data need, the input the reconciler renders a Policy from (AP-04, AP-05).

| Field | Type | Required | Description |
|---|---|---|---|
| `attrs` | list of string | no | Attributes the app needs; empty means every readable attribute of those types. |
| `contextSpaceRef` | [Ref](#ref) | yes | The context space the app reads. |
| `geoQ` | [GeoConstraint](#geoconstraint) | no | Geographic narrowing. |
| `operations` | list of [OperationRef](#operationref) | yes | CIM 009 clause 4.20 operation names the app performs (R8). |
| `q` | string | no | NGSI-LD query narrowing the readable set. |
| `representations` | list of [Representation](#representation) | no | Representations of the rendered endpoint this need contributes (AP-05, EP-08). |
| `roles` | list of string | no | The app roles this need is granted to; empty means every caller the endpoint admits (AP-96). |
| `scopeQ` | string | no | Scope query narrowing the readable set. |
| `temporalQ` | [TemporalConstraint](#temporalconstraint) | no | Temporal narrowing. |
| `types` | list of string | yes | NGSI-LD entity types the app needs. |

## AppEgress

One destination a server pod reaches besides its endpoint (AP-134). A NetworkPolicy matches addresses, never names, so a host name has no place here (ADR-N-037).

| Field | Type | Required | Description |
|---|---|---|---|
| `cidr` | string | yes | An IPv4 or IPv6 network, `203.0.113.0/24`, with no host bits set; never `/0`. |
| `ports` | list of integer (uint16) | yes | The TCP ports the pod may reach there, at least one. |

## AppClass

The App's shape (AP-124, AP-148): `ui`, React on the SDK served statically, `ui-rust`, the same UI with an axum server in a pod, or `wasm`, the same UI with a server component on the shared WASM host. `ui-node` is declared and not built yet.

Type: one of `"ui"`, `"ui-rust"`, `"wasm"`

## AppLifecycle

Lifecycle state of an app (AP-18, AP-19, AP-20).

Type: one of `"draft"`, `"preview"`, `"published"`, `"retired"`

## AppLimits

Per-app runtime limits enforced on its own endpoint (AP-17).

| Field | Type | Required | Description |
|---|---|---|---|
| `aiTokensPerDay` | integer (uint32) | no | Model tokens, in and out, the App may use a day through `ai` (AP-165). |
| `emailsPerDay` | integer (uint32) | no | Messages the App may send a day through `email` (AP-165). |
| `filesMiB` | integer (uint32) | no | MiB the App may store through `files` (AP-165). |
| `maxFileRows` | integer (uint32) | no | Rows a single file representation download may return. |
| `requestsPerMinute` | integer (uint32) | no | Requests per minute allowed on the app endpoint. |

## AppRole

One role a person can hold in an App (AP-90).

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | no | What a person holding it may do, for whoever adds a member. |
| `name` | string | yes | `a-z{0,31}`, unique in the App. |
| `title` | map of string | no | The role's name per locale, as the App page and the `403` page show it. |

## AppServer

The server part of a `wasm` App beyond its request handler (AP-154).

| Field | Type | Required | Description |
|---|---|---|---|
| `jobs` | list of [AppJob](#appjob) | no | What the host calls on a schedule, each in a fresh instance (AP-154). |

## AppService

A platform service an App uses (AP-160, ADR-N-045): the closed set, so a name outside it is refused like any unknown field, with the six it may be.

Type: one of `"identity"`, `"data"`, `"files"`, `"email"`, `"jobs"`, `"ai"`

## AppSource

Where the app's source lives; exactly one member is set (AP-02).

| Field | Type | Required | Description |
|---|---|---|---|
| `git` | [GitSource](#gitsource) | no | A repository of the same forge (AP-02). |
| `path` | string | no | Path beside the manifest in the org repository. |

## AppStorage

What a `wasm` App stores: its own schema in `apps-db` and its own prefix in the object store (AP-148, ADR-N-044).

| Field | Type | Required | Description |
|---|---|---|---|
| `blob` | [BlobStorage](#blobstorage) | no | Its objects, under its own prefix (AP-145). |
| `sql` | [SqlStorage](#sqlstorage) | no | Its schema, migrated by the reconciler at publish (AP-149). |

## AppVisibility

Who may reach a published app (AP-18). A login by default: `project` (AP-120).

Type: one of `"private"`, `"project"`, `"organization"`, `"public"`, `"roles"`

## Subject

One human or group a binding names; exactly one of `user`, `group` (PF-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `group` | string | no | A group: a `Group` manifest of the organization unless `source` says otherwise (PF-64). |
| `source` | [SubjectSource](#subjectsource) | no | Who owns `group`'s members: absent for a `Group` manifest, `provider` for a group the identity provider owns, which the reconciler never manages. Only a `RoleBinding` subject carries it, and only with `group` (PF-63, PF-64). |
| `user` | string | no | The person's Keycloak user id, the `sub` of their token: a lower-case UUID, never an e-mail or a username (PF-126); or a client's own `service-account-<client>` user. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## GeoConstraint

Geographic narrowing of a data need, e.g. `{ within: { scopeRef: /geo/SK/BB } }` (AP-05).

| Field | Type | Required | Description |
|---|---|---|---|
| `within` | [GeoWithin](#geowithin) | yes | Scope tree the app may read inside. |

## OperationRef

Reference to a CIM 009 operation or operation group (R8).

Closed enum ensures no proprietary verbs (`read`, `write`, `upsertEntity`) are permitted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: [Operation](#operation) or [OperationGroup](#operationgroup)

## Representation

Supported representation formats served under an endpoint (EP-05, Architecture/04).

Note: `schema` (EP-46) and `access` (EP-55) are not variants: they are unconditional surfaces on every endpoint and cannot be toggled off. `file.csv` is the one file an Endpoint publishes; the names of the removed files are refused by name ([`REMOVED_REPRESENTATIONS`]).

Type: one of `"ngsi-ld"`, `"mcp"`, `"csv"`, `"ogc-features"`, `"sta"`, `"odata"`, `"websocket"`, `"ical"`, `"feed"`

## TemporalConstraint

Temporal narrowing of a data need, e.g. `{ window: P1D }` (AP-05).

| Field | Type | Required | Description |
|---|---|---|---|
| `window` | string | yes | ISO 8601 duration reaching back from now. |

## AppJob

One scheduled call of the App's component (AP-154).

| Field | Type | Required | Description |
|---|---|---|---|
| `export` | string | yes | The function the component exports, called with no arguments. |
| `name` | string | yes | `a-z{0,39}`, unique in the App, as a function's name (SDK-21). |
| `schedule` | string | yes | Five-field cron, `minute hour day month weekday`, read in UTC. |

## GitSource

A source repository on the organization's own forge (AP-02).

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Subdirectory holding the app. |
| `ref` | string | yes | Branch, tag or commit. |
| `url` | string | yes | Clone URL on the organization's forge; `https://` only. |

## BlobStorage

An App's object prefix (AP-148).

| Field | Type | Required | Description |
|---|---|---|---|
| `quotaMiB` | integer (uint32) | no | How much the prefix may hold, MiB. |

## SqlStorage

An App's schema (AP-148, AP-149).

| Field | Type | Required | Description |
|---|---|---|---|
| `migrations` | string | no | The folder of the repository whose `.sql` files are run in the order of their names. |
| `quotaMiB` | integer (uint32) | no | How much the schema may hold, MiB. |

## SubjectSource

Who owns the members of a subject's group (PF-64).

Type: `"provider"`

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## GeoWithin

The scope a [`GeoConstraint`] confines the app to (ADR 005).

| Field | Type | Required | Description |
|---|---|---|---|
| `scopeRef` | string | yes | Absolute scope string, e.g. `/geo/SK/BB`. |

## Operation

Table 4.20-1: every named distributed API operation in ETSI GS CIM 009 (R8).

Closed enum ensures no proprietary verbs (`upsertEntity`, `read`, `write`) are accepted, satisfying the strict R8 CIM 009 vocabulary guarantee.

Type: one of `"createEntity"`, `"updateEntity"`, `"appendAttrs"`, `"updateAttrs"`, `"deleteAttrs"`, `"deleteEntity"`, `"createBatch"`, `"upsertBatch"`, `"updateBatch"`, `"deleteBatch"`, `"upsertTemporal"`, `"appendAttrsTemporal"`, `"deleteAttrsTemporal"`, `"updateAttrInstanceTemporal"`, `"deleteAttrInstanceTemporal"`, `"deleteTemporal"`, `"mergeEntity"`, `"replaceEntity"`, `"replaceAttrs"`, `"mergeBatch"`, `"purgeEntity"`, `"retrieveEntity"`, `"queryEntity"`, `"queryBatch"`, `"retrieveTemporal"`, `"queryTemporal"`, `"retrieveEntityTypes"`, `"retrieveEntityTypeDetails"`, `"retrieveEntityTypeInfo"`, `"retrieveAttrTypes"`, `"retrieveAttrTypeDetails"`, `"retrieveAttrTypeInfo"`, `"createSubscription"`, `"updateSubscription"`, `"retrieveSubscription"`, `"querySubscription"`, `"deleteSubscription"`, `"retrieveEntityMap"`, `"updateEntityMap"`, `"deleteEntityMap"`, `"createEntityMapQueryEntity"`, `"createEntityMapQueryTemporal"`, `"retrieveContextSourceIdentity"`

## OperationGroup

Table 4.20-2: the five distributed operation group names (R8).

Type: one of `"federationOps"`, `"associationOps"`, `"updateOps"`, `"retrieveOps"`, `"redirectionOps"`
