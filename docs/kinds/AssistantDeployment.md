# AssistantDeployment

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `AssistantDeployment`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/assistantdeployments`
- Repository path: `projects/{project}/assistant/deployments/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (AssistantDeploymentSpec)

Desired specification of an `AssistantDeployment` (MF-52).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowedOrigins` | list of string | no | The pages it may be placed on, each `https://host[:port]`; no wildcard, no path. |
| `answer` | [AnswerSpec](#answerspec) | no | How it answers: what one answer may use. |
| `budget` | [Budget](#budget) | no | How much it may spend; required on every channel but `internal`. |
| `categories` | list of [DataTheme](#datatheme) | no | EU data-theme codes it is about, for display (MF-60); `theme` is the chat's look. |
| `channel` | [Channel](#channel) | yes | `public`, `internal`, `ckan` or `iframe`. |
| `connectors` | list of [Connector](#connector) | no | The Endpoints of this project whose MCP tools it may call. |
| `disclaimer` | map of string | no | The line under the chat that says answers are machine-written, per language. |
| `enabled` | boolean | no | Whether it answers at all; `false` makes every chat request answer `503`. Default true. |
| `exclude` | list of string | no | Endpoints it never reads, named as a connector names one (MF-59). |
| `feedback` | boolean | no | Whether the chat shows the helpful/unhelpful buttons under an answer; default true. |
| `languages` | list of string | no | The languages it answers in, as ISO 639-1 codes. |
| `mcpServer` | string | no | The named MCP server whose reach it reads, `{project}/{name}` or `org/{name}` (MF-56, ADR-N-043 §7.6); on every channel but `internal` a `public` one. |
| `model` | [ModelChoice](#modelchoice) | no | The router and answer models; the service's default when absent. |
| `privacy` | [Privacy](#privacy) | no | Whom a visitor asks about their data (T-4670). |
| `publicId` | string | yes | The id in the widget's address, `/w/{publicId}`; a DNS label. |
| `rateLimit` | [RateLimit](#ratelimit) | no | How often it answers; required on every channel but `internal`. |
| `sandbox` | boolean | no | Whether it may filter a large tool result with a script in `jc-functions`; default false. |
| `scope` | [AssistantScope](#assistantscope) | no | What it reaches beyond its own project. |
| `select` | [Select](#select) | no | The Endpoints it reads beside its connectors, picked by the McpServer grammar (MF-58, ADR-N-055 §2.2); resolved inside the project's reach by [`crate::reach::resolve_connectors`]. |
| `sources` | list of string | no | The `KnowledgeSource`s it answers from: one of this project by its name, one of a project in `scope.projects` as `{project}/{source}` (T-4281). |
| `suggestions` | map of list of string | no | Questions the chat offers before the first one, per language of `languages`. |
| `systemPrompt` | string | no | What the assistant is told before every conversation; at most 8,000 characters. |
| `theme` | [Theme](#theme) | no | How the chat looks. |
| `transcriptRedaction` | [TranscriptRedaction](#transcriptredaction) | no | What is masked in a turn before it is stored; `secrets` when absent (AG-99, T-4681). |
| `transcriptRetention` | string | no | How long each turn of a conversation is kept, `P{n}D` with `n` 0 to 365; `P90D` when absent, `P0D` keeps none (AG-99, T-4670, T-4681). |
| `widget` | [ChatWidget](#chatwidget) | no | How the chat picks its language and whether it shows the toggle (T-4343). |

## AnswerSpec

How the deployment answers.

| Field | Type | Required | Description |
|---|---|---|---|
| `maxDataSeconds` | integer (uint32) | no | The seconds after the question in which the loop may still call a tool; 1 to 60, default 20 (T-4294). |
| `maxDataSteps` | integer (uint32) | no | The most tool calls a data question's loop makes, the router's reads included; 1 to 25, default 10 (T-4294). |
| `routes` | [Routes](#routes) | no | What one answer may use; default `both`. |

## Budget

How many model tokens the deployment may spend; `jc-agent-proxy` holds it to this.

| Field | Type | Required | Description |
|---|---|---|---|
| `costPerConversationUsd` | number (double) | no | What all model calls of one conversation may cost, in US dollars; 0.01 to `costPerDayUsd`, no cap when absent. No call starts once the conversation has spent it (AG-107, T-4737). |
| `costPerDayUsd` | number (double) | no | What the deployment's model calls may cost per day, in US dollars; 0.01 to 1,000, default 3. Past it an answer lists the matching sources' links only (AG-107). |
| `messagesPerConversation` | integer (uint32) | no | The most questions one conversation may ask; 1 to 200, default 30. |
| `outputTokensPerAnswer` | integer (uint32) | no | The most tokens one answer may have; 100 to 4,000, default 700. |
| `questionChars` | integer (uint32) | no | The longest question, in characters; 100 to 4,000, default 1,000. |
| `tokensPerConversation` | integer (uint64) | yes | Tokens one conversation may spend; at least 1 and no more than a day's. |
| `tokensPerDay` | integer (uint64) | yes | Tokens per day across all conversations; 1 to 100,000,000. |
| `visitorSharePercent` | integer (uint32) | no | The percent of `costPerDayUsd` one visitor, or all callers without an `Origin` together, may spend a day; 1 to 100, default 10. Past it their questions are refused until UTC midnight while others still get answers (T-4653, T-5928). |

## DataTheme

A theme of the EU data-theme table.

Type: one of `"AGRI"`, `"ECON"`, `"EDUC"`, `"ENER"`, `"ENVI"`, `"GOVE"`, `"HEAL"`, `"INTR"`, `"JUST"`, `"REGI"`, `"SOCI"`, `"TECH"`, `"TRAN"`

## Channel

Where a deployment answers.

Type: one of `"public"`, `"internal"`, `"ckan"`, `"iframe"`

## Connector

One Endpoint of the project whose MCP tools the assistant may call.

| Field | Type | Required | Description |
|---|---|---|---|
| `endpoint` | string | yes | The Endpoint's `metadata.name` in this project, or `{project}/{name}` for a public Endpoint of a project in `scope.projects`. |
| `timeoutSeconds` | integer (uint16) | no | How long one tool call may take, in seconds; 1 to 120, default 20. |
| `tools` | list of string | yes | The tools of its MCP surface the assistant may call; at least one. |

## ModelChoice

The models a deployment names for `jc-agent-proxy` (MF-52): the router and answer calls.

| Field | Type | Required | Description |
|---|---|---|---|
| `answer` | string | no | The model that writes the answer, `vendor/name`. |
| `provider` | [ProviderRouting](#providerrouting) | no | The OpenRouter providers both calls are asked on (T-4292); OpenRouter's fastest when absent (T-4360). |
| `reasoning` | [StageReasoning](#stagereasoning) | no | The reasoning effort of each call (T-4360); `low` when absent. |
| `router` | string | no | The model that picks the tools and sources of a question, `vendor/name`. |

## Privacy

Whom a visitor of an assistant asks about their data, shown in the widget's privacy panel (API/05 §1.6, T-4670).

| Field | Type | Required | Description |
|---|---|---|---|
| `contact` | string | no | An e-mail address, at most 254 characters. |
| `documentsUrl` | string | no | An `https` page of the operator's privacy documents, at most 2,000 characters. |

## RateLimit

How often the deployment answers.

| Field | Type | Required | Description |
|---|---|---|---|
| `perClientPerDay` | integer (uint32) | no | Questions per day from one client; 1 to 100,000, default 200. |
| `perClientPerHour` | integer (uint32) | no | Questions per hour from one client; 1 to 7,200, default 60. |
| `perClientPerMinute` | integer (uint32) | yes | Questions per minute from one client; 1 to 120. |
| `perClientPerMonth` | integer (uint32) | no | Questions per UTC calendar month from one client; 1 to 100,000, default 300 (T-4286). |
| `requestsPerMinute` | integer (uint32) | yes | Questions per minute across everyone; 1 to 600. |
| `streamsPerClient` | integer (uint32) | no | Answers one client may have streaming at once; 1 to 10, default 2. |

## AssistantScope

What a deployment reaches beyond its own project.

| Field | Type | Required | Description |
|---|---|---|---|
| `projects` | list of string | no | Other projects whose `audience: public` Endpoints its connectors, and whose `KnowledgeSource`s its sources, may name; at most 10, never its own and never `*`. |

## Select

Which Endpoints a server selects beside its members (MF-55). An Endpoint is selected when it matches every field given, one value of a field being enough; `select: {}` selects every Endpoint the caller's token and the server's audience admit. The gateway resolves it per call from the caller's own reach, so it only narrows.

| Field | Type | Required | Description |
|---|---|---|---|
| `accessLevel` | list of [AccessLevel](#accesslevel) | no | How the caller reaches the Endpoint; no wider than the server's audience admits. |
| `domains` | list of string | no | Smart Data Models domains of the Endpoint's classification, e.g. `Environment` (T-4498). |
| `projects` | list of string | no | Project names. |
| `spaces` | list of string | no | Context Spaces as `{project}/{space}`. |
| `themes` | list of [DataTheme](#datatheme) | no | EU data themes of that classification, e.g. `ENVI`. |
| `types` | list of string | no | NGSI-LD entity types the Endpoint serves, e.g. `OffStreetParking` (MF-61). |

## Theme

How the chat looks where it is placed.

| Field | Type | Required | Description |
|---|---|---|---|
| `font` | string | no | A CSS `font-family` list of family names the chat's text uses, at most 200 characters; the system font when absent. No font file is loaded (T-4740). |
| `greeting` | [Localised](#localised) | no | The first line the chat shows, at most 500 characters. |
| `primaryColor` | string | no | The accent colour, `#rrggbb`. |
| `subtitle` | [Localised](#localised) | no | The line under the title, at most 80 characters; the word Assistant when absent (T-4740). |
| `title` | [Localised](#localised) | no | The body's name in the chat's header, beside its monogram; at most 60 characters. |

## TranscriptRedaction

What a stored turn has masked (AG-99, T-4681).

Type: one of `"secrets"`, `"personal"`

## ChatWidget

How the chat behaves where it is placed (T-4343).

| Field | Type | Required | Description |
|---|---|---|---|
| `language` | [WidgetLanguage](#widgetlanguage) | no | How it picks its language; `browser` by default. |
| `languageToggle` | boolean | no | Whether the header shows the language toggle; default true. |

## Routes

What one answer may use (T-4281).

Type: one of `"both"`, `"rag"`, `"data"`

## ProviderRouting

OpenRouter provider routing, sent as its `provider` request field (T-4292, MF-52).

| Field | Type | Required | Description |
|---|---|---|---|
| `allowFallbacks` | boolean | no | Whether another provider may answer when these fail; default `true`. |
| `order` | list of string | no | Provider slugs, tried in this order, e.g. `cerebras`, `groq`; 1 to 8. |
| `sort` | [ProviderSort](#providersort) | no | How OpenRouter ranks the providers `order` does not pin (T-4360). |

## StageReasoning

The reasoning effort of the router, judge and answer calls apart; each `low` when absent.

| Field | Type | Required | Description |
|---|---|---|---|
| `answer` | [ReasoningEffort](#reasoningeffort) | no | The answer call, and each step of the data loop. |
| `judge` | [ReasoningEffort](#reasoningeffort) | no | The judge call, which reads the first passages and says whether they answer. |
| `router` | [ReasoningEffort](#reasoningeffort) | no | The router call, which picks the route and the queries. |

## AccessLevel

How a caller reaches an Endpoint, the levels of the per-caller catalogue (T-4503).

Type: one of `"public"`, `"organization"`, `"project"`, `"grant"`

## Localised

A text the chat shows: one string in the deployment's first language, or one per language of `languages` (T-4365).

Type: string or map of string

## WidgetLanguage

How the chat picks the language it opens in (T-4343).

Type: one of `"browser"`, `"ask"`, `"fixed"`

## ProviderSort

OpenRouter's `provider.sort` (T-4360).

Type: one of `"latency"`, `"throughput"`, `"price"`

## ReasoningEffort

A reasoning effort, from none to high (T-4360).

Type: one of `"none"`, `"minimal"`, `"low"`, `"medium"`, `"high"`
