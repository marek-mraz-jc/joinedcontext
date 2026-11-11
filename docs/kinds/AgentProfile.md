# AgentProfile

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `AgentProfile`
- Lives in the Organization (namespace `org`)
- API: `/api/v1/projects/{project}/agentprofiles`
- Repository path: `agentprofiles/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (AgentProfileSpec)

Specification for a `kind: AgentProfile` resource (AG-47…AG-50).

| Field | Type | Required | Description |
|---|---|---|---|
| `access` | [AgentAccess](#agentaccess) | no | What an agent of this profile may reach. Absent: read-only operations and nothing else (AG-70). |
| `egress` | [AgentEgress](#agentegress) | yes | Where the package route may go. |
| `limits` | [AgentLimits](#agentlimits) | yes | The bounds every run of this profile is held to. |
| `model` | [AgentModel](#agentmodel) | yes | The model the proxy calls on the run's behalf. |
| `role` | [AgentProfileRole](#agentprofilerole) | yes | What the profile is allowed to be used for (AG-26). |
| `runtime` | [AgentRuntime](#agentruntime) | yes | The runtime image a run of this profile executes. |
| `tools` | list of [AgentTool](#agenttool) | no | Tools the workspace carries; a steward profile carries none. |
| `workspace` | [AgentWorkspace](#agentworkspace) | yes | Resources the workspace container gets. |

## AgentAccess

The most an agent of this profile may reach; the starting person's own permissions narrow it further at every call, and nothing here widens them (AG-70, MF-40).

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoints` | list of [EndpointGrant](#endpointgrant) | no | Endpoints the agent may read or write through. |
| `kinds` | list of [KindGrant](#kindgrant) | no | Kinds the agent may read or propose changes to. |
| `operations` | list of string | no | Registry operation names (`jc_…`) the agent may call. |

## AgentEgress

What one run may read from the internet, and how much of it (AG-50, AG-65).

The default is nothing: a profile that names no host reaches no host, and the workspace has no route of its own to fall back on (AG-34).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedHosts` | list of string | no | Bare hostnames the package and fetch routes may reach; everything else is refused (AG-50). |
| `maxBytesPerRun` | integer (uint64) | no | Bytes one run may read from those hosts in total, across every fetch (AG-65). Absent means [`DEFAULT_EGRESS_BYTES_PER_RUN`]: reading documentation is what the allow-list is for, and a profile that had to name a number to get it would have every author guessing one. The cap is what stops a run from mirroring a site, not what stops it from reading a page. |

## AgentLimits

Computational, execution, and rate bounds per run (AG-41).

| Field | Type | Required | Description |
|---|---|---|---|
| `concurrentRunsPerOrganization` | integer (uint32) | yes | Runs one organization may have in flight at once. |
| `maxResponseBytes` | integer (uint64) | yes | Bytes one proxied response may carry. |
| `requestsPerMinute` | integer (uint32) | yes | Proxied requests per minute per run. |
| `stepsPerRun` | integer (uint32) | yes | Agent steps one run may take before it stops and hands over the last log (AG-51). |
| `wallClock` | string | yes | ISO 8601 duration (e.g. `PT20M`), maximum 1 hour (`PT1H`). |

## AgentModel

Model selection and token budgeting.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxTokensPerRun` | integer (uint64) | yes | Tokens one run may consume before the proxy ends it (AG-41). |
| `name` | string | yes | Model identifier the provider knows. |
| `provider` | [ModelProvider](#modelprovider) | yes | Which protocol the proxy speaks to the model provider. |
| `reasoningEffort` | [ReasoningEffort](#reasoningeffort) | no | How hard the model thinks on every call of a run; absent, no reasoning setting is sent (AG-72). |
| `routing` | [ProviderRouting](#providerrouting) | no | Which of the model's OpenRouter routes every call of a run asks for, in OpenRouter's terms (AG-72). `jc-agent-proxy` passes it to OpenRouter alone. |

## AgentProfileRole

Profile execution role (AG-26, AG-47).

Type: one of `"builder"`, `"steward"`

## AgentRuntime

Container runtime image specification (AG-49).

| Field | Type | Required | Description |
|---|---|---|---|
| `digest` | string | yes | Cryptographic container digest (`sha256:...`). Required for admission (AG-49). |
| `image` | string | yes | Container image repository and tag. |

## AgentTool

Available coding and debugging tools in the builder workspace.

Type: one of `"shell"`, `"cargo"`, `"pnpm"`, `"git"`, `"playwright"`

## AgentWorkspace

Workspace hardware resource allocations.

| Field | Type | Required | Description |
|---|---|---|---|
| `cpu` | string | yes | CPU request and limit of the workspace container. |
| `ephemeralStorage` | string | yes | Size of the `emptyDir` the workspace works in. |
| `memory` | string | yes | Memory request and limit of the workspace container. |

## EndpointGrant

One endpoint an agent may reach, and how.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | The endpoint's manifest name, a DNS-1123 label. |
| `verbs` | list of [EndpointVerb](#endpointverb) | yes | At least one verb. |

## KindGrant

One kind an agent may reach, and how.

| Field | Type | Required | Description |
|---|---|---|---|
| `kind` | string | yes | A manifest kind of the catalogue, e.g. `Endpoint`. |
| `verbs` | list of [KindVerb](#kindverb) | yes | At least one verb. |

## ModelProvider

Model provider choice.

Type: one of `"anthropic"`, `"openai-compatible"`

## ReasoningEffort

The provider's reasoning setting a run's model calls carry (AG-72).

Type: one of `"low"`, `"medium"`, `"high"`

## ProviderRouting

OpenRouter provider routing, sent as its `provider` request field (T-4292, MF-52).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowFallbacks` | boolean | no | Whether another provider may answer when these fail; default `true`. |
| `order` | list of string | no | Provider slugs, tried in this order, e.g. `cerebras`, `groq`; 1 to 8. |
| `sort` | [ProviderSort](#providersort) | no | How OpenRouter ranks the providers `order` does not pin (T-4360). |

## EndpointVerb

What an agent may do through an endpoint (AG-70).

Type: one of `"read"`, `"write"`

## KindVerb

What an agent may do to a kind of manifest (AG-70).

Type: one of `"read"`, `"propose"`

## ProviderSort

OpenRouter's `provider.sort` (T-4360).

Type: one of `"latency"`, `"throughput"`, `"price"`
