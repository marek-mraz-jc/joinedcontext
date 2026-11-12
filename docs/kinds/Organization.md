# Organization

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `Organization`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/organizations`
- Repository path: `organizations/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (OrganizationSpec)

Desired specification of an `Organization` resource (PF-01, PF-02, PF-25, PF-41).

| Field | Type | Required | Description |
|---|---|---|---|
| `appearance` | [OrganizationAppearance](#organizationappearance) | no | What the Organization's Portal, open data portal and login page show of it, each member over the installation's branding (UIX-16). |
| `catalog` | [ProjectCatalog](#projectcatalog) | no | The catalogue publisher a project of this Organization takes when it names none of its own (ADR-N-058 section 2.3, EP-127). |
| `contacts` | list of [Contact](#contact) | no | Administrative, technical, data protection, or security contacts. |
| `dcatProfiles` | list of [DcatProfile](#dcatprofile) | no | The national DCAT-AP profiles the records of its Endpoints are also served in, besides DCAT-AP 3.0, which is always served (EP-27, docs API/08 section 5). |
| `defaultLocale` | string | yes | Organization fallback default locale, MUST be one of [`Self::locales`] (PF-25, PF-26). |
| `domain` | string | yes | Verified internet FQDN, the `{orgDomain}` of every entity URN (PF-41). |
| `gitRepositoryUrl` | string | no | Git repository URL of the one Org Repository in Gitea (PF-02). |
| `installation` | boolean | no | The installation's Organization, which owns the repository, the gateway's `{orgDomain}` and the installation-wide kinds; with several Organizations exactly one sets it, and the legacy `org.yaml` counts as setting it (PF-121, ADR-N-058). |
| `limits` | [OrganizationLimits](#organizationlimits) | no | The organization's numbers: edge rates, body sizes, sessions, invitations, model spend, pipeline outcomes and uploads (ADR-N-035, PF-96). |
| `locales` | list of string | yes | Ordered list of supported locales, most preferred first (PF-25). |
| `policies` | [OrganizationPolicies](#organizationpolicies) | no | The organization's choices: public apps, the password policy, the models agents may use (ADR-N-035, PF-96). |
| `projects` | [ProjectsPolicy](#projectspolicy) | no | How projects are opened and who sees them (PF-61, PF-65). |

## OrganizationAppearance

`spec.appearance` (UIX-16).

| Field | Type | Required | Description |
|---|---|---|---|
| `city` | string | no | The city or region the Organization serves, e.g. `Banská Bystrica`. |
| `colours` | [AppearanceColours](#appearancecolours) | no | The colour tokens, each a hex colour such as `#0b5394`. |
| `contactEmail` | string | no | The contact address in the footer and the open data portal, e.g. `opendata@banskabystrica.sk`. |
| `faviconSvg` | string | no | The favicon as an SVG document of at most 64 KiB, under the same rules as the logo. |
| `homeWidgets` | list of string | no | Dashboards the Organization's Portal home page shows, each `project/name` of one of its projects, e.g. `doprava/prehlad`; a viewer sees only those they may read (T-6779). |
| `instanceName` | string | no | Full name on page titles and the login page, e.g. `Mesto Banská Bystrica`. |
| `licenseDefault` | string | no | The licence a new dataset starts with, e.g. `CC-BY-4.0`. |
| `logoSvg` | string | no | The logo as an SVG document of at most 64 KiB, without script, event handler or external reference. |
| `opendataWidgets` | list of string | no | Dashboards the Organization's open data portal shows on its catalogue home, each `project/name` of one of its projects with `visibility: public`, e.g. `doprava/prehlad`; anonymous readers see them (T-6780). |
| `publisher` | string | no | The legal publisher named in the footer and the open data portal, e.g. `Mesto Banská Bystrica`. |
| `shortName` | string | no | Short name on sidebars and tabs, e.g. `BB`. |

## ProjectCatalog

A Project's `spec.catalog`: the body that publishes its data, which the project's own catalogue names and every record whose Endpoint names no publisher carries (EP-127, docs API/08 section 6).

| Field | Type | Required | Description |
|---|---|---|---|
| `contactPoint` | [ContactPoint](#contactpoint) | no | The desk that answers for the project's datasets. |
| `publisher` | [Publisher](#publisher) | yes | The publishing body, with its registry IRI: a catalogue's publisher is an IRI. |

## Contact

Contact information for an organization role.

| Field | Type | Required | Description |
|---|---|---|---|
| `email` | string | yes | Contact email address. |
| `name` | string | yes | Full name of the contact person or organizational unit. |
| `phone` | string | no | Optional contact telephone number. |
| `role` | [ContactRole](#contactrole) | yes | Role of this contact. |

## DcatProfile

A national DCAT-AP profile an Organization serves its records in (docs API/08).

Type: one of `"dcat-ap-sk"`, `"dcat-ap-cz"`

## OrganizationLimits

`spec.limits`: the organization's numbers (ADR-N-035 §3). Every field is optional; an absent one is its [`CATALOG`] default.

| Field | Type | Required | Description |
|---|---|---|---|
| `agents` | [AgentLimits](#agentlimits) | no | Model spend of the assistant and agent runs. |
| `apps` | [AppServiceLimits](#appservicelimits) | no | The per-App quotas of the platform services (AP-165). |
| `data` | [DataLimits](#datalimits) | no | Uploads and imports. |
| `edge` | [EdgeLimits](#edgelimits) | no | What the edge admits. |
| `gateway` | [GatewayLimits](#gatewaylimits) | no | What the context gateway admits. |
| `mqtt` | [MqttAllowance](#mqttallowance) | no | The MQTT allowance every project gets, off by default (PF-117, ADR-N-048 §2.9). |
| `people` | [PeopleLimits](#peoplelimits) | no | Invitations. |
| `pipelines` | [PipelineLimits](#pipelinelimits) | no | Pipeline outcomes and tests. |
| `signIn` | [SignInLimits](#signinlimits) | no | How long a sign-in lasts. |

## OrganizationPolicies

`spec.policies`: the organization's choices (ADR-N-035 §3).

| Field | Type | Required | Description |
|---|---|---|---|
| `agents` | [AgentsPolicy](#agentspolicy) | no | What the assistant and agent runs may use. |
| `apps` | [AppsPolicy](#appspolicy) | no | What the organization's Apps may be. |
| `signIn` | [SignInPolicy](#signinpolicy) | no | How people sign in. |

## ProjectsPolicy

The organization's rules for its projects (PF-61, PF-65).

| Field | Type | Required | Description |
|---|---|---|---|
| `creation` | [ProjectCreation](#projectcreation) | no | Who may open a project: `anyone`, `group:<name>` or `org-admin` (PF-65). |
| `nameCooldownDays` | integer (uint32) | no | How long a deleted project's name stays reserved, in days (PF-78). Absent is [`DEFAULT_NAME_COOLDOWN_DAYS`]; `0` frees the name the moment the project is gone. |
| `quota` | [Quotas](#quotas) | no | The quota every project the organization opens starts from (PF-73). A project may lower a value in the yellow lane; raising one above this is the red lane and an `org-admin`. |
| `visibility` | [ProjectVisibility](#projectvisibility) | no | Who holds the seeded `viewer` role at organization scope: every signed-in person of the organization (`organization`, the default) or only whoever a binding names (`members`) (PF-61). |

## AppearanceColours

`spec.appearance.colours`: each one a hex colour, `#rgb` or `#rrggbb`.

| Field | Type | Required | Description |
|---|---|---|---|
| `accent` | string | no | Highlight colour, e.g. `#f59e0b`. |
| `background` | string | no | Page background, e.g. `#ffffff`. |
| `primary` | string | no | Primary action colour, e.g. `#0b5394`. |
| `secondary` | string | no | Secondary colour for links and focus, e.g. `#0f766e`. |
| `text` | string | no | Body text, e.g. `#0f172a`. |

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

## ContactRole

Functional responsibility of an organization contact.

Type: one of `"administrative"`, `"technical"`, `"data-protection"`, `"security"`, `"open-data"`

## AgentLimits

`spec.limits.agents`: model spend in whole units of the currency the agent proxy's provider bills in, the day and the month in UTC.

| Field | Type | Required | Description |
|---|---|---|---|
| `spendPerDay` | integer (uint32) | no | Spend a day; absent is no limit. |
| `spendPerMonth` | integer (uint32) | no | Spend a month; absent is no limit. |

## AppServiceLimits

The per-App quotas of the platform services an Organization (`spec.limits.apps`) or a Project (`spec.apps.limits`) sets; an absent one takes the layer above (AP-165, ADR-N-045).

| Field | Type | Required | Description |
|---|---|---|---|
| `aiTokensPerDay` | integer (uint32) | no | Model tokens, in and out, an App may use a day through `ai`. |
| `emailsPerDay` | integer (uint32) | no | Messages an App may send a day through `email`. |
| `filesMiB` | integer (uint32) | no | MiB an App may store through `files`. |

## DataLimits

`spec.limits.data`.

| Field | Type | Required | Description |
|---|---|---|---|
| `uploadMegabytes` | integer (uint32) | no | The largest upload or import, in MiB; at most the edge's body. |

## EdgeLimits

`spec.limits.edge`.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxRequestBodyMegabytes` | integer (uint32) | no | The largest request body the edge accepts, in MiB. |
| `requestsPerMinute` | [EdgeRates](#edgerates) | no | Requests a minute per route class. |

## GatewayLimits

`spec.limits.gateway`.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxRequestBodyMegabytes` | integer (uint32) | no | The largest request body the gateway accepts, in MiB; at most the edge's. |

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

## PeopleLimits

`spec.limits.people`.

| Field | Type | Required | Description |
|---|---|---|---|
| `invitationHours` | integer (uint32) | no | Hours an invitation link stays usable. |

## PipelineLimits

`spec.limits.pipelines`.

| Field | Type | Required | Description |
|---|---|---|---|
| `rejectedKept` | integer (uint32) | no | Rejected records a pipeline keeps, newest first. |
| `sampleMegabytes` | integer (uint32) | no | The largest sample a pipeline test takes, in MiB; at most the edge's body. |

## SignInLimits

`spec.limits.signIn`: written to the edge session and the realm alike (AP-29).

| Field | Type | Required | Description |
|---|---|---|---|
| `sessionIdleMinutes` | integer (uint32) | no | Minutes without a request before a session ends. |
| `sessionMaxHours` | integer (uint32) | no | Hours after sign-in when a session ends whatever happens. |

## AgentsPolicy

`spec.policies.agents`.

| Field | Type | Required | Description |
|---|---|---|---|
| `models` | list of string | no | The models the agent proxy lets a run call. Absent is every model the installation's agent profiles name; a model outside those is never reachable whatever this lists. |

## AppsPolicy

`spec.policies.apps` (PF-103).

| Field | Type | Required | Description |
|---|---|---|---|
| `public` | [PublicApps](#publicapps) | no | Whether an App may be `public`; `refused` makes every door refuse a Change that sets an App public, and leaves the Apps already public as they are (PF-100, PF-103). |
| `services` | list of [AppService](#appservice) | no | The platform services the organization's Apps may use (AP-163); absent is `DEFAULT_ORGANIZATION_SERVICES`. |

## SignInPolicy

`spec.policies.signIn`.

| Field | Type | Required | Description |
|---|---|---|---|
| `password` | [PasswordPolicy](#passwordpolicy) | no | The realm's password policy. |

## ProjectCreation

Who may open a project (PF-65). `group:<name>` names a `Group` of the organization.

Type: `"Anyone"` or object or `"OrgAdmin"`

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

## ProjectVisibility

Who holds the seeded `viewer` role at organization scope (PF-61).

Type: one of `"organization"`, `"members"`

## EdgeRates

`spec.limits.edge.requestsPerMinute`.

| Field | Type | Required | Description |
|---|---|---|---|
| `api` | integer (uint32) | no | The Portal API, per token. |
| `dataRead` | integer (uint32) | no | Context-space reads (`GET`, `HEAD`), per token. |
| `dataWrite` | integer (uint32) | no | Context-space writes (every other method), per token. |
| `publicEndpoint` | integer (uint32) | no | A public endpoint, per address. |
| `web` | integer (uint32) | no | The Portal's pages, per address. |

## MqttAddress

How a broker is addressed from outside (ADR-N-048 §2.11).

Type: one of `"platform"`, `"dedicatedPort"`, `"customHostname"`

## SecretRef

Reference to a credential stored in SOPS or OpenBao (PF-36).

`deny_unknown_fields` is a security control: it rejects inline `value:` or `password:` fields at parse time, enforcing that secrets never appear in Git or manifest files.

| Field | Type | Required | Description |
|---|---|---|---|
| `envVar` | string | no | Optional environment variable name for injection into runners. |
| `key` | string | no | Optional key within secret. |
| `name` | string | yes | Secret name in secret store. |

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

## PublicApps

Whether the organization's Apps may be public (PF-103).

Type: one of `"allowed"`, `"refused"`

## AppService

A platform service an App uses (AP-160, ADR-N-045): the closed set, so a name outside it is refused like any unknown field, with the six it may be.

Type: one of `"identity"`, `"data"`, `"files"`, `"email"`, `"jobs"`, `"ai"`

## PasswordPolicy

`spec.policies.signIn.password`: written to the realm's password policy.

| Field | Type | Required | Description |
|---|---|---|---|
| `history` | integer (uint32) | no | How many earlier passwords a new one may not repeat; 0 to 24. |
| `minLength` | integer (uint32) | no | The shortest password the realm accepts; at least 12. |
