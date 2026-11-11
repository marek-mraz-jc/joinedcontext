# Project

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Project`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/projects`
- Repository path: `projects/{name}/project.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (ProjectSpec)

Desired specification of a `Project` resource (PF-05, PF-17).

| Field | Type | Required | Description |
|---|---|---|---|
| `apps` | [ProjectApps](#projectapps) | no | What the project's Apps may use: a narrower service list and lower quotas than the organization's (AP-163, AP-165). |
| `catalog` | [ProjectCatalog](#projectcatalog) | no | The body that publishes the project's data in a national catalogue: the project's own catalogue names it, and so does every record whose Endpoint names none (EP-127). |
| `domain` | string | no | The internet domain of the body that publishes the project's data, the `did:web` assigner of the Policies compiled for its Apps; absent, the Organization's (PF-108). It describes who publishes and grants, verifies and routes nothing; pipelines keep minting over `JC_ORG_DOMAIN` (PF-44). |
| `organizationRef` | [Ref](#ref) | yes | Reference to the parent Organization. |
| `parameters` | map of [Parameter](#parameter) | no | Declarations in the project file, this deployment's values in a registry entry (CC-88). |
| `quotas` | [Quotas](#quotas) | no | Optional resource quotas for this Project (PF-17); the project file's, never a registry entry's. |
| `ref` | string | no | The tag, branch or commit of `repository` this deployment runs; a registry entry only. |
| `repository` | [ProjectRepository](#projectrepository) | no | Where the project's configuration lives. Its presence makes this manifest a registry entry, `projects/{slug}.yaml` of the organization repository (PF-86). |
| `version` | [SemVer](#semver) | no | The project's version, released as the tag `v{version}`; the project file only (CC-88). |
| `visibility` | [ProjectVisibility](#projectvisibility) | no | Who reads the project through an organization-scope binding that only reads: absent is the Organization's `projects.visibility`; `members` keeps it to the people a binding of the project or of one of its context spaces names, so departments stay apart (PF-61). |

## ProjectApps

`spec.apps` of a Project (AP-163, AP-165, ADR-N-045).

| Field | Type | Required | Description |
|---|---|---|---|
| `limits` | [AppServiceLimits](#appservicelimits) | no | The per-App quotas; an absent one is the organization's. |
| `services` | list of [AppService](#appservice) | no | The platform services the project's Apps may use; absent is the organization's list. |

## ProjectCatalog

A Project's `spec.catalog`: the body that publishes its data, which the project's own catalogue names and every record whose Endpoint names no publisher carries (EP-127, docs API/08 section 6).

| Field | Type | Required | Description |
|---|---|---|---|
| `contactPoint` | [ContactPoint](#contactpoint) | no | The desk that answers for the project's datasets. |
| `publisher` | [Publisher](#publisher) | yes | The publishing body, with its registry IRI: a catalogue's publisher is an IRI. |

## Ref

Reference to another resource, either bare name or typed reference (MF-07).

Type: string or [TypedRef](#typedref)

## Parameter

A parameter entry of `spec.parameters`: a declaration in the project file, a value in a registry entry (CC-88).

Type: [ParameterDeclaration](#parameterdeclaration) or [ParameterValue](#parametervalue)

## Quotas

Structural resource quotas per project (PF-17).

An absent field leaves the dimension to the organization's default, and both stay under the deployment's ceiling ([`crate::kinds::OrganizationBounds::quota_ceiling`]): no project is unlimited (LIM-01, T-6716).

| Field | Type | Required | Description |
|---|---|---|---|
| `agentRunsPerDay` | integer (uint32) | no | Maximum number of agent runs the project starts in one day (PF-73, PF-74). |
| `apps` | integer (uint32) | no | Maximum number of Apps the project deploys (PF-73). |
| `contextSpaces` | integer (uint32) | no | Maximum number of active Context Spaces. |
| `entitiesPerSpace` | integer (uint32) | no | Maximum number of entities one Context Space of the project holds (PF-73). |
| `ingestEventsPerSecond` | integer (uint32) | no | Maximum ingestion rate in events per second. |
| `minPipelinePeriodSeconds` | integer (uint32) | no | The shortest clock a Pipeline of the project may run on, in seconds: its `spec.period` and the shortest gap of its `spec.schedule` (PL-116). A floor, not a count, so it is no PF-73 dimension; the stricter of the project's and the organization's value holds. |
| `mqtt` | [MqttAllowance](#mqttallowance) | no | The project's own MQTT allowance, written only by an org admin; it replaces the organization's `spec.limits.mqtt` as a whole (PF-117, ADR-N-048 §2.9). |
| `publicEndpoints` | integer (uint32) | no | Maximum number of public endpoints. |
| `requestsPerMinute` | integer (uint32) | no | Maximum requests per minute one Endpoint of the project serves (PF-73, EP-17). |
| `residentPipelines` | integer (uint32) | no | Maximum number of resident streaming pipelines. |

## ProjectRepository

Where a registry entry's project comes from (PF-86, CC-89).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | no | A repository of the organization in the local forge. |
| `secretRef` | [SecretRef](#secretref) | no | The credential that reads `url`; a local repository needs none. |
| `url` | string | no | A git repository outside the forge, mirrored read-only at the pinned ref (CC-89). |

## SemVer

Semantic version string conforming to major.minor.patch (DM-22)

Type: string

## ProjectVisibility

Who holds the seeded `viewer` role at organization scope (PF-61).

Type: one of `"organization"`, `"members"`

## AppServiceLimits

The per-App quotas of the platform services an Organization (`spec.limits.apps`) or a Project (`spec.apps.limits`) sets; an absent one takes the layer above (AP-165, ADR-N-045).

| Field | Type | Required | Description |
|---|---|---|---|
| `aiTokensPerDay` | integer (uint32) | no | Model tokens, in and out, an App may use a day through `ai`. |
| `emailsPerDay` | integer (uint32) | no | Messages an App may send a day through `email`. |
| `filesMiB` | integer (uint32) | no | MiB an App may store through `files`. |

## AppService

A platform service an App uses (AP-160, ADR-N-045): the closed set, so a name outside it is refused like any unknown field, with the six it may be.

Type: one of `"identity"`, `"data"`, `"files"`, `"email"`, `"jobs"`, `"ai"`

## ContactPoint

The contact point of a dataset, a `vcard:Kind`.

| Field | Type | Required | Description |
|---|---|---|---|
| `email` | string | yes | A role address, never a person's own (EP-80): `vcard:hasEmail`. |
| `name` | string | yes | The desk's name: `vcard:fn`. |

## Publisher

The publisher of a dataset, a `foaf:Agent`.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | map of string | yes | The publisher's name per language: `foaf:name`. |
| `uri` | string | no | The publisher's own IRI, its web site or registry entry. |

## TypedRef

Structured typed resource reference (MF-07).

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | Target resource kind. |
| `name` | string | yes | Target resource name. |
| `namespace` | string | no | Target resource namespace (defaults to caller namespace if omitted). |

## ParameterDeclaration

The declaration of one deployment knob (CC-88), a subset of JSON Schema draft-07.

| Field | Type | Required | Description |
|---|---|---|---|
| `default` | [ParameterValue](#parametervalue) | no | The value when the registry entry sets none. |
| `description` | string | no | What the knob does, for the parameter form. |
| `enum` | list of [ParameterValue](#parametervalue) | no | The only values allowed; empty allows every value of the type. |
| `type` | [ParameterType](#parametertype) | yes | What the value is; `secret` takes the name of a secret, never its value. |

## ParameterValue

One scalar a parameter takes.

Type: boolean or number or string

## MqttAllowance

`spec.limits.mqtt` of an Organization or `spec.quotas.mqtt` of a Project. Every number is optional; an absent one is its catalog default once `enabled` is `true`.

| Field | Type | Required | Description |
|---|---|---|---|
| `addresses` | list of [MqttAddress](#mqttaddress) | no | The address kinds the project may use; absent is `[platform]`. |
| `brokers` | integer (uint32) | no | How many `MqttBroker` manifests the project may hold. |
| `clientCaRef` | [SecretRef](#secretref) | no | The secret holding the project's device client CA certificate (never its key), required when `deviceAuth` holds `clientCertificate`; only a project's own `spec.quotas.mqtt` sets it. |
| `connections` | integer (uint32) | no | The sum of `spec.limits.maxConnections` over the project's brokers. |
| `customHostnames` | list of [MqttHostname](#mqtthostname) | no | Hostnames the city owns that an org admin assigned to this project, at most 10; only a project's own `spec.quotas.mqtt` lists them. |
| `deviceAuth` | list of [MqttDeviceAuth](#mqttdeviceauth) | no | How devices may authenticate; absent is `[token]`. |
| `enabled` | boolean | no | Whether the project may declare brokers at all; off unless an org admin turns it on. |
| `maxMessageBytes` | integer (uint32) | no | The ceiling of each broker's `spec.limits.maxMessageBytes`. |
| `messagesPerSecond` | integer (uint32) | no | The sum of `maxConnections` × `messagesPerSecond` over the project's brokers. |
| `modes` | list of [MqttMode](#mqttmode) | no | The broker modes the project may use; absent is `[ingest]`. |
| `ports` | list of integer (uint16) | no | Dedicated ports from 18830–18849 an org admin assigned to this project. |

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

## ParameterType

The type of a parameter (CC-88).

Type: one of `"string"`, `"integer"`, `"number"`, `"boolean"`, `"secret"`

## MqttAddress

How a broker is addressed from outside (ADR-N-048 §2.11).

Type: one of `"platform"`, `"dedicatedPort"`, `"customHostname"`

## MqttHostname

One hostname an org admin assigned to a project (ADR-N-048 §2.11).

| Field | Type | Required | Description |
|---|---|---|---|
| `certificateRef` | [SecretRef](#secretref) | no | The secret holding the certificate the edge serves for `name`; absent, cert-manager issues one by HTTP-01 once the name resolves to the edge. |
| `name` | string | yes | A lowercase DNS name the city owns and points at the edge by CNAME, e.g. `mqtt.city.example`. |

## MqttDeviceAuth

How a device proves who it is when it connects (ADR-N-048 §2.12).

Type: one of `"token"`, `"clientCertificate"`

## MqttMode

What a broker does with a message (ADR-N-048 §2.10).

Type: one of `"ingest"`, `"capture"`, `"relay"`
