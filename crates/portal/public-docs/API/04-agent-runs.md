---
sidebar_position: 5
title: "Agent Runs API"
---

# Agent Runs API

The Agent Runs API controls autonomous application-builder sessions, questionnaire interactions, lifecycle cancellation, merge proposal submission, and real-time Server-Sent Events (SSE).

## 1. Standard Headers

- `Authorization: Bearer <jwt>` (Required for all project routes)
- `Content-Type: application/json`
- `Accept: text/event-stream` (Required for `/events`)

## 2. Error Response Format (RFC 7807)

Errors return `application/problem+json` conforming to the platform standard:

```json
{
  "type": "https://joinedcontext.com/errors/invalid-request",
  "title": "Invalid Request",
  "status": 400,
  "detail": "declared dataNeeds exceed what endpoint 'helsinki-bikes' publishes",
  "errors": [
    "attribute 'maintenanceInternalCode' is not published by endpoint 'helsinki-bikes'"
  ]
}
```

## 3. Project Run Management

### Create an Agent Run

```http
POST /api/v1/projects/{project}/agent-runs HTTP/1.1
Host: portal.example.joinedcontext.com
Authorization: Bearer eyJhbGciOiJSUzI1Ni...
Content-Type: application/json
```

Request payload:

```json
{
  "appName": "city-bikes-overview",
  "endpointNames": ["helsinki-bikes", "helsinki-kpi"],
  "profile": "app-builder",
  "appClass": "fullstack",
  "visibility": "project",
  "kind": "application",
  "unattended": false,
  "prompt": "Create a live bike availability dashboard with station filtering by district",
  "dataNeeds": [
    {
      "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
      "types": ["BikeHireDockingStation"],
      "attrs": ["name", "location", "totalSlotNumber", "freeSlotNumber"],
      "operations": ["queryEntity", "retrieveEntity"]
    }
  ]
}
```

Response: `202 Accepted`

```json
{
  "id": "e3b0c442-98fc-1c14-9afb-4c7b2756a120",
  "project": "helsinki",
  "appName": "city-bikes-overview",
  "endpointName": "helsinki-bikes",
  "endpointSlug": "si6epqkx364lprho5uaigutk274r5grb",
  "endpoints": [
    { "name": "helsinki-bikes", "slug": "si6epqkx364lprho5uaigutk274r5grb", "space": "helsinki" },
    { "name": "helsinki-kpi", "slug": "q3mzkq2v7w5ayxcbn4ltdj6hof2repgu", "space": "helsinki-kpi" }
  ],
  "profile": "app-builder",
  "appClass": "fullstack",
  "visibility": "project",
  "kind": "application",
  "unattended": false,
  "continues": null,
  "prompt": "Create a live bike availability dashboard with station filtering by district",
  "promptDigest": "sha256:d8578edf8458ce06fbc5bb76a58c5ca4",
  "dataNeeds": [
    {
      "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
      "types": ["BikeHireDockingStation"],
      "attrs": ["name", "location", "totalSlotNumber", "freeSlotNumber"],
      "operations": ["queryEntity", "retrieveEntity"]
    }
  ],
  "allowsWrite": false,
  "branch": "agent/app-city-bikes-overview/e3b0c442-98fc-1c14-9afb-4c7b2756a120",
  "pathPrefix": "projects/helsinki/apps/city-bikes-overview/",
  "status": "queued",
  "steps": 0,
  "tokensUsed": 0,
  "createdBy": "demo.steward@hel.fi",
  "createdAt": "2026-09-12T10:15:30Z",
  "expiresAt": "2026-09-12T10:35:30Z"
}
```

- `endpointNames`: The endpoints the application reads, one to five distinct names of the project, the first one the primary (AP-44). `endpointName` (one name) is the same request with one endpoint; a body with both, or with neither, answers `400 Bad Request`. Every endpoint must exist with a slug and be granted by the profile (AG-70).
- Each data need belongs to the first listed endpoint whose `contextSpaceRef` names the need's context space, or to the primary endpoint when none does; its attributes are checked against that endpoint's projection. Its operations are checked against the caller's own grant on that endpoint (AP-44, AP-132): the Portal reads the endpoint's `/access` document from the context gateway with the caller's token, and a need naming an operation the caller does not hold on one of its types answers `400 Bad Request` with each such operation in `errors`, beside every other violation. A Portal cookie session carries no token, so a request from it with a write need answers `400` naming it, and a gateway that does not answer is `503 Service Unavailable`.
- `endpoints` (response): every endpoint of the run with its `slug` and context `space`, the primary first; `endpointName` and `endpointSlug` stay the primary's.
- `kind`: Optional string, one of `"application"`, `"dashboard"`, or `"analysis"`. Defaults to `"application"`.
- `unattended`: Optional boolean. Defaults to `false` for `application`. Always `true` for `dashboard` and `analysis` (a value of `false` for these kinds is rejected with `400 Bad Request`).
- Public visibility (`visibility: "public"`) is rejected with `400 Bad Request` (AP-42).
- Write operations (every operation jc-core's `OperationRef::is_write` names: `createEntity`, `updateAttrs`, `appendAttrs`, `deleteAttrs`, the batch writes, …) trigger `allowsWrite: true` and sandbox provisioning; unauthorized write attempts fail with `403 Forbidden`.
- `dataNeeds[].roles`: Optional list of application role names (`[a-z][a-z0-9-]{0,31}`, at most 16 distinct across the run) that the need is granted to, as in an App manifest (AP-96). A name outside that pattern, or more than 16, answers `400 Bad Request` here, before the run starts. Publishing declares every distinct name in the App's `spec.roles` (`{name}`, no title and no members), so the manifest passes AP-91; the role's title and its members are added on the App page afterwards, each through a Change (AP-91, UI-44). A need without `roles` keeps granting every caller the endpoint admits, and a write on it stays red lane (AP-98).
- If the builder profile or model secrets are missing, returns `503 Service Unavailable`.

### Test Runs of the Live Journeys (AG-93)

A run started by the Portal's own live journeys carries `"origin": "journey"`; every other run carries `"origin": "person"`. The journeys mark theirs with one request header on `POST /api/v1/projects/{project}/agent-runs` and `POST /api/v1/projects/{project}/assistant/conversations`:

```http
X-JC-Run-Origin: journey
```

- Only the journeys set it: a browser session (the Portal's cookie or the edge's `X-Access-Token`) of a user named in `JC_PORTAL_JOURNEY_USERS`, the demo people the journeys sign in as. Anyone else who sends it answers `403 Forbidden`, an administrator included, and so does the header beside `Authorization: Bearer`, so neither a person, a service, a script nor an agent hides a run. Any value other than `journey` answers `400 Bad Request`.
- The marker only hides a run from a list's default. It deletes nothing and changes nothing else: the run and its events stay whole for the audit (AG-45).

### List Runs in Project

```http
GET /api/v1/projects/{project}/agent-runs?kind=application&status=building&mine=true&limit=20 HTTP/1.1
```

Query parameters:

- `kind`: Filter runs by kind (`conversation`, `application`, `dashboard`, `analysis`).
- `status`: Filter runs by lifecycle status (`queued`, `starting`, `interviewing`, `building`, `testing`, `previewing`, `awaitingApproval`, `published`, `failed`, `cancelled`, `expired`).
- `mine`: Boolean (`true` or `false`). When `true`, restricts results to runs initiated by the calling user.
- `app`: Filter runs associated with a specific application name.
- `limit`: Maximum number of records returned.
- `origin`: `person`, `journey` or `all` (AG-93). Without it, a list leaves the journeys' test runs out (`person`). A request that carries `X-JC-Run-Origin: journey` itself lists `all`, so a journey finds the runs it started; from anyone but a journey the header answers `403`. Any other value answers `400 Bad Request`.

Access visibility: A user sees the runs they initiated. Users holding the `portal-approver` role in the project also see all project runs.

Response: `200 OK`

```json
{
  "items": [
    {
      "id": "e3b0c442-98fc-1c14-9afb-4c7b2756a120",
      "project": "helsinki",
      "appName": "city-bikes-overview",
      "endpointName": "helsinki-bikes",
      "profile": "app-builder",
      "appClass": "fullstack",
      "visibility": "project",
      "kind": "application",
      "unattended": false,
      "continues": null,
      "status": "building",
      "steps": 14,
      "tokensUsed": 245000,
      "createdBy": "demo.steward@hel.fi",
      "origin": "person",
      "createdAt": "2026-09-12T10:15:30Z"
    }
  ]
}
```

### Get Single Run Details

```http
GET /api/v1/projects/{project}/agent-runs/{id} HTTP/1.1
```

Response: `200 OK` with full run representation including `kind`, `unattended`, `continues`, `previewUrl` (when available), `firstFrameMs` (the milliseconds from the run's creation to its first preview, set once and never moved by a later pass; the number AP-57 is measured on) and `mergeRequest` number. Two more fields link the application's source and its publication (AP-71):

- `sourceUrl` — the forge's public web address of the run's `pathPrefix` on its branch, present once the run has committed and the Portal knows the forge; for a `static` run the source is the application's own repository `{project}_{app}` and `pathPrefix` is empty (AP-75, AP-78);
- `mirrorUrl` — the GitHub web address of the application's repository, present for a `static` run whose installation keeps application repositories on GitHub (AP-79); the forge's `sourceUrl` stays the repository of record;
- `changeId` — the `chg-…` id of the Change `POST …/publish` opened, present after Publish; its state is read from `GET /api/v1/projects/{project}/changes/{changeId}`.

### Preview Document

```http
GET /api/v1/projects/{project}/agent-runs/{id}/preview HTTP/1.1
Accept: text/html
```

Response: `200 OK`, `Content-Type: text/html`, one document in either of two shapes:

- **A code run** (its files hold `src/**`): the run's interface files transpiled in the Portal and the SDK runtime, each a `data:` module behind one inline import map, the SDK stylesheet and the run's `src/**/*.css` inlined, `src/main.tsx` always the template's, and `#jc-config` naming the run's endpoint slug, `orgDomain`, space, application and `transport: "bridge"` (SDK-16). Its policy allows the import map and the entry script by hash plus `data:` modules, and `connect-src` only the project's basemap route. The response is `400 Bad Request` with `errors` listing every problem as `{file}:{line}:{column}: {message}` when a file does not parse, an import is outside SDK-12 or a relative import names no file; `503 Service Unavailable` when the Portal was built without the SDK runtime.
- **A kit run**: the kit bundle inlined with the run's current `spec.json`, served with the application Content Security Policy (`connect-src` limited to the platform origin and the basemap tiles).

Both carry `frame-ancestors 'self'`. The Portal frames the document with `sandbox="allow-scripts"` and no `allow-same-origin` (AP-50), and it holds no rows, token or cookie. Needs the session of the initiating user or a reviewer; answers `404 Not Found` until a pass has written files. Every `preview` event carries `previewUrl` with `?v={seq}` appended, so a frame reloads once per pass and never serves a cached document.

## 4. Real-Time Event Streaming (Server-Sent Events)

Clients subscribe to live run activity via Server-Sent Events:

```http
GET /api/v1/projects/{project}/agent-runs/{id}/events HTTP/1.1
Accept: text/event-stream
Last-Event-ID: 12
```

Response: `200 OK`, `Content-Type: text/event-stream`

```text
id: 13
event: status
data: {"seq":13,"status":"interviewing","timestamp":"2026-09-12T10:16:02Z"}

id: 14
event: question
data: {"seq":14,"questionId":"q-center","required":true,"timeoutSeconds":300,"schema":{"type":"object","title":"Default Map Center","properties":{"center":{"type":"string","title":"District","enum":["Kamppi","Kallio","Pasila"],"default":"Kamppi"}},"required":["center"]}}

: keep-alive

id: 15
event: thought
data: {"seq":15,"text":"Scaffolding Rust axum backend with embedded React static assets."}

id: 16
event: tool
data: {"seq":16,"tool":"cargo_build","command":"cargo build --release","exitCode":0,"durationMs":14500}

id: 17
event: commit
data: {"seq":17,"sha":"7f1b2c4d","message":"feat: scaffold axum server and bike station table view","author":"agent:app-builder@example.org","url":"https://git.example.fi/hel/org/commit/7f1b2c4d"}

id: 18
event: preview
data: {"seq":18,"previewUrl":"https://portal.example.fi/apps/city-bikes-overview/?preview=7f1b2c4d"}

id: 19
event: usage
data: {"seq":19,"step":22,"tokensThisStep":4120,"inputTokens":3980,"outputTokens":140,"cachedTokens":3200,"latencyMs":1830,"model":"google/gemini-3.8-flash","reasoningEffort":"medium","cumulativeTokens":382100,"timestamp":"2026-09-25T08:14:03.412Z"}

id: 20
event: message
data: {"seq":20,"text":"Sort by free bikes, not by name.","sentBy":"jana.kovacova"}

id: 21
event: navigate
data: {"seq":21,"route":"/projects/helsinki/endpoints","prefill":{"name":"air-quality-public","contextSpaceRef":"helsinki","enabledRepresentations":["ngsi-ld","csv"]}}
```

The kinds a stream carries are `status`, `question`, `answer`, `message`, `thought`, `partial`, `tool`, `commit`, `preview`, `preview_error`, `tests`, `usage`, `navigate`, `path` and `lag`. A `tests` event is one run of the application's tests in the run's sandbox (SDK-38): `{version, outcome, passed, failed, durationMs, failures}`, `outcome` one of `running`, `passed`, `failed`, `error` (the sandbox could not run them: a timeout, no pod, an unreadable result, with `reason`) or `skipped` (no sandbox in this installation, or a version without tests, with `reason`), and `failures` at most 20 of `{file, name, message}`, each message cut at 2,000 characters. A `partial` is the assistant's words while the model still writes them (ADR-N-032, T-2821): `{text, elapsedMs}`, `text` the prose of the answer so far, cut before the first tool fence, so a tool call's JSON never shows; at most two a second, each carrying the whole text so far. The `thought` or the step that follows is the answer, and a client draws a `partial` only until an event of another kind arrives after it. A `thought` carrying `failed: true` is a turn that ended without an answer: its `text` says why in words a person can act on (the model service was busy, did not answer within the turn's time, answered nothing readable), never the provider's body, the key or the prompt; the model call behind it was already asked once more after a 408, 425, 429, 500, 502, 503 or 504 or a connection that failed, so the person reads a failure only when the second try failed too. The dock offers Try again on it, which sends the person's last message again in the same conversation (T-2772). A `path` event names the path a conversation is on and who chose it (section 8, [Paths](#paths)). Two of them are written by a person rather than by the agent: `answer` is one question answered, `message` is a free instruction sent between questions (section 5). A `usage` frame is one model call: `tokensThisStep` what it cost, `inputTokens` and `outputTokens` its two halves when the provider reports them, `cachedTokens` the input tokens the provider read from its prompt cache, `latencyMs` the milliseconds from the call leaving `jc-agent-proxy` to its last byte, `model` the model named in the call, `reasoningEffort` the effort it was sent with (`low`, `medium` or `high`: the profile's `model.reasoningEffort`, or the one the call's body named itself) and absent when it named none (T-2998), and, on a frame the editing agent wrote, `step` and `cumulativeTokens` (the run's total). Every model call is counted once, on the run's `tokensUsed` and as one frame (T-3072): `jc-agent-proxy` writes the frame for every call it relays (AG-72, T-2771), and the editing agent writes one of its own (SDK-20) only for a call whose provider reported no usage, which the proxy counts nothing for, with `inputTokens` estimated from the bytes it sent. A `status` frame that ends a run by a cancel or an expiry carries `reason`: who cancelled it ("cancelled by jana.kovacova"), or what the expiry lost, and the conversation says it under the ended chat (T-2772). A run that reaches `awaiting_approval` waits on the approval's lease, `JC_AGENT_APPROVAL_TTL` seconds (seven days by default), in place of what was left of its wall clock: an approver has days. When that lease runs out too, the run expires with a `reason` saying its change stays open and that approving it still publishes the application. Every frame's data carries `timestamp`, the RFC 3339 instant the Portal recorded the event (a `status` frame's own `timestamp` is that instant too), so a client reads where a run's seconds went from the stream alone (T-2771).

A `ui` frame is a browser action of the Portal assistant (T-5959): `{action: "highlight", kind, field, message}` focuses the control of `field` in the open form and shows `message` beside it as text, `{action: "filter", query}` types `query` into the page's table filter. The server checked both before the frame exists (07-agents-and-mcp, `jc_ui_highlight`, `jc_ui_filter_table`); the dock runs each frame once, after the page of the `navigate` before it has mounted, and no frame submits, proposes or approves anything.

A `tool` frame is one step the assistant took, inspectable in the conversation (AG-56, OPS-50): `tool` names it, `status` is `ok` or `failed`, `durationMs` how long it took, `elapsedMs` the milliseconds since the run started (every `tool` and `navigate` event carries it, so a journey reads where the time went, T-2697), `input` what it was given, `output` what it produced or `error` why it did not, and `diff` the change it made when there is one; `command` and `exitCode` stay for a shell step. A `tool` named `search_catalog` carries the catalog search's body (01-portal-api §18) as its `output`, and the conversation draws it as cards (UI-46); one named `propose_endpoint` carries the endpoint proposal (01-portal-api §19) and is followed by the `navigate` that opens the form. One named `change_resource` (AG-77) carries `{kind, name, patch?, delete?, next?}` as its `input`, `patch` a JSON merge patch (RFC 7386) of the manifest that keeps its kind, name and namespace, and `{kind, name, checked: true, test?}` or `{kind, name, delete: true}` as its `output`: the patched manifest passed the same dry run as `jc_manifest_dry_run`, and a changed Pipeline or DataSource passed its test before anything opens (PL-45, MF-39): `test` is `{source: {dataSource} | {endpoint}, verdict: {ok, findings}, records, sample}` for a pipeline run by `jc_pipeline_test` on one fetch of its `http` data source or on a page of the endpoint it reads, `{source: {url}, probe}` for a data source's one fetch (the dry run's `probe`), or `{untested}` when there is nothing to test on; a test that is not green is a `failed` step carrying the same `test` and goes back to the model with what the test saw, and a green pipeline test is the verdict the draft carries. The manifest is kept as the person's draft and travels as the `prefill` of the `navigate` that follows, to `/projects/{project}/{page}?edit={name}` (the endpoint form opens from the draft at `/projects/{project}/endpoints`), or to `?delete={name}`, which opens the typed-confirm removal. A refused check, an unknown kind or name (answered with the real ones) and a call without `patch` (answered with the manifest as it is) go back to the model for its next attempt; a kind the profile or the person may not change is a `failed` step with the reason; nothing is proposed. A request for several resources (a space with its model, Endpoint, source and pipeline) drafts one per turn: the call's `next` (`[{kind, name}]`, at most 8 kept) lists the rest in order, and the Portal ends the answer with one line per resource and a link to its kind's page, inside the person's open workspace; a kind the person may not propose is named as such, and an unknown kind or a name that is no resource name stays out of the text (T-6531). A change to a `DataModel` carries `operations` instead of `patch`: the model editor's operations (01-portal-api §11, DM-13) that add, rename or remove classes and slots, `addClass {name, class_uri?, description?, is_a?}`, `removeClass {name}`, `renameClass {name, to}`, `addSlot {name, class?, range?, slot_uri?, kind?}`, `removeSlot {name}`, `renameSlot {name, to}`, `attachSlot {class, slot}`, `detachSlot {class, slot}` and `setSlot {name, field, value}` for `range`, `required`, `multivalued` or `description`. The Portal applies them to the model's LinkML source as the repository holds it and runs the source's dry run (`PUT /api/v1/projects/{project}/datamodels/{name}/source?dryRun=All`: Model Tools compiles it and the changes are classified against the published source), and the `output` is `{kind, name, checked: true, severity, changes, version}`, a breaking change included. The `navigate` to `/projects/{project}/models?edit={name}` carries `{operations}` as its `prefill`: the editor loads the source and applies the same operations to its text, so its comments and order stay, and a breaking change is saved only once the person confirms it there. An operation naming a class or slot the model does not have, or a name it already has, goes back to the model with the model's classes and their slots, and so does a call without `operations`. A new `Dashboard` is `change_resource` with `create: true`, `patch` holding its whole `spec` (`{title, visibility, pages}`), and the new `Layer`s its pages draw in `layers` (`[{name, spec}]`, each spec `{sourceEndpointRef, entityType, style, filter?, colorBy?, sizeBy?, popupProperties?}`). The Portal refuses a name the project already has, an endpoint that is not the project's, and an attribute a layer names (`colorBy.property`, `sizeBy.property`, `popupProperties`) that the endpoint's data model does not give its type, answering with the endpoint's attributes; it runs `jc_manifest_dry_run` on every layer and on the dashboard, keeps each as the person's draft, and the `output` is `{kind, name, checked: true, create: true, layers}`. The `navigate` goes to `/projects/{project}/dashboards?edit={name}` with the dashboard as `prefill`: the dashboard editor opens on the new dashboard with its new layers read from the drafts, and its one proposal carries the layers and the dashboard through `POST /api/v1/projects/{project}/import`, one Change. While the dock follows an application's run, a message the person sends goes to that run (`POST /agent-runs/{id}/messages`), and the dock names the application it talks to; closing the dock leaves the run. One named `write_entities` (AG-78) carries `{endpoint, entities: [{id, attrs}]}` as its `input`, `attrs` the attributes that change with their new values, at most 50 entities, and `{endpoint, slug, entities: [{id, type, changes: [{attribute, before, after}]}]}` as its `output`: the Portal read the person's grants on the endpoint and each entity through `jc-agent-proxy`, and nothing is written. The conversation draws it as a card with Apply and Cancel; Apply sends `PATCH /api/endpoint/{slug}/ngsi-ld/v1/entities/{id}/attrs` for each entity from the browser with the person's session, each attribute a `Property` with its new value, and shows the gateway's answer per entity. A grant that does not let the person update the type or an attribute, an entity the person cannot read, an unknown endpoint and more than 50 entities are a `failed` step with the reason, which goes back to the model. One named `grant_role` (AG-77, PF-52) carries a binding's `{subjects, role, scope, validity?}` as its `input`, `subjects` users (username or e-mail) or groups and `scope` one of `organization`, `project` or `contextSpace`, and `{kind: "RoleBinding", name, checked: true}` as its `output`: the Portal names the binding from its subject, role and scope, runs the dry run, which holds it to what the person holds on that scope, keeps it as the person's draft and sends the `navigate` to `/projects/{project}/access?grant={name}` with the manifest as `prefill`, which opens the grant form filled in; an unknown role (answered with the organization's roles) and a grant beyond the person's rights (answered with the verbs they lack) go back to the model, and nothing is proposed. Every field is untrusted text, and the proxy redacts a bearer token, a cookie, a password, an API key or a connection string before the event is stored:

```text
id: 22
event: tool
data: {"seq":22,"tool":"propose_endpoint","status":"failed","durationMs":840,"input":{"name":"air-quality-public","contextSpaceRef":"helsinki"},"error":"422: representations must name at least one of ngsi-ld, csv"}
```

`navigate` is how the agent drives the Portal (UI-45): `route` is a path inside the Portal, `prefill` an optional object the page hands to the form it opens (a form's values, or the changed manifest for a page opened with `?edit={name}`) and `draft` the `{kind, name}` of the draft it came from. The Portal accepts a route only when it starts with one `/`, carries no scheme, no `//`, no `#`, no control character and is at most 512 characters; any other route is refused with `400` and never reaches a browser. The browser switches to the route without reloading the page, opens the form with the prefill, and shows a banner saying the assistant navigated it. Prefill values are untrusted data (AG-46): the form validates them like typed input and nothing is proposed until the person submits.

- Reconnection: Sending `Last-Event-ID` replays all missed events from PostgreSQL before joining the live broadcast channel.
- Keep-Alive: An empty comment `: keep-alive` is transmitted every 15 seconds to prevent gateway timeouts.
- APISIX Buffering: Response carries `X-Accel-Buffering: no` and `Cache-Control: no-cache`.

## 5. Talking to the Run

A run is a conversation, not a form: a person answers the questions the agent asks, and may also
say something at any point while the run is alive.

### Answering a Question

When a run enters `interviewing` status:

```http
POST /api/v1/projects/{project}/agent-runs/{id}/answers HTTP/1.1
Content-Type: application/json

{
  "questionId": "q-center",
  "answers": {
    "center": "Kallio"
  }
}
```

Response: `204 No Content`

Submitting an answer appends an `answer` event to the run log and signals the waiting agent via proxy long-poll.

A conversation run asks with `jc_ask` (AG-80, AG-83):

```json
{ "question": "Which endpoints should the app read?", "pick": "endpoints", "multiple": true, "min": 1 }
```

```json
{ "question": "Which district first?", "options": [{ "value": "kallio", "title": "Kallio", "description": "Most stations" }] }
```

`pick` names a kind (`endpoints`, `spaces`, `datamodels`, `pipelines`, `datasources`, `policies`,
`projects`); the Portal fills the options from what the person may read, and `options` with
`pick` only narrows that list by `value`, so a name the person cannot read is dropped. Without
`pick`, `options` are the model's own, rendered as text. The `question` event carries a `oneOf` of
`{const, title, description}` for one answer, or `{type: array, items: {oneOf: …}, uniqueItems,
minItems, maxItems}` with `multiple`. The answer to a `pick` or `multiple` question is checked
against what the question offered before the run continues: a value that was not offered, one
chosen twice, or a count outside `min`/`max` answers `400` and the question stays open. A question
of the model's own options with one answer keeps "Something else…" (UI-57), so its answer is free
text. A kind the person may not read at all is a refusal the model reads, never an empty question.

**Options a person cannot take, and data to hand over** (T-2694, ADR-N-032, UI-44). Each entry of the
`question` event's `options` carries `value`, `title` and `description`. An option the platform offers
but the person cannot take right now also carries `disabledReason`, one sentence saying why (for
example, the project has no data sources yet). The panel shows that option disabled, with the reason,
and the answer refuses it. A question may also ask for data, in the event's `input`:

```json
{ "input": { "file": { "accept": ["csv", "tsv", "json"], "maxBytes": 262144 }, "url": true } }
```

`file` takes one text file of a format in `accept`, at most `maxBytes` bytes. `url` takes one
absolute `http` or `https` address. The Portal's own first steps set `input`. A model sets it
with `jc_ask`'s `input: ["file"]`, `["url"]` or both, and the Portal fills in `accept` and
`maxBytes`. The answer is exactly one of:

- `{"answer": "<value>"}`, an option;
- `{"file": {"name": "stations.csv", "format": "csv", "text": "…"}}`, the file's text;
- `{"url": "https://…"}`, the address. The run fetches it later, through the same guard as every other fetch (no cluster, private or metadata address).

The answer is refused with `400`, and the question stays open, when it:

- names an option that was not offered or that carries a `disabledReason`;
- hands over a file or an address the question did not ask for;
- gives a format outside `accept`, or a text over `maxBytes` or empty;
- gives a file name with a path separator, JSON text that does not parse, or text with a NUL character;
- gives an address that is not absolute `http(s)`, or that is longer than 2048 characters.

The model reads a file answer as its name, format, size, line count and first lines. It never reads the whole file.

### Steering the Run

Any status short of a terminal one accepts an instruction, so a person who sees the build going
the wrong way says so instead of cancelling and starting again (AG-45):

```http
POST /api/v1/projects/{project}/agent-runs/{id}/messages HTTP/1.1
Content-Type: application/json

{
  "text": "Sort by free bikes, not by name."
}
```

Response: `204 No Content`

The text is at most 4000 characters and is appended as a `message` event. It is an instruction,
never a grant: the run reads through the Endpoint and the confirmed `dataNeeds` it was created
with, and no message widens either. A run that has ended answers `409 Conflict`.

A message on a `conversation` run MAY carry `endpointNames` (zero to five names of the project,
AG-75): the endpoints the assistant may query from this message on, replacing the ones before.
They are resolved and checked against the profile like the conversation's own (`403 Forbidden`
when the profile does not grant reading one), recorded as an `endpoints` event with the names,
and the gateway's policy for the person still decides every row. On any other kind of run the
field answers `400 Bad Request`.

It MAY also carry `access` (AG-92), checked as on the start: the capabilities from this message
on, replacing the ones before; the `message` event carries it. It MAY also carry `pageContext`, the page the person is on as they send it, checked as on the
start (section 8, "Start or Continue") and refused the same way. The `message` event carries the
page read from it as `page`: `{route, page, kind, name, sub, tab}`, names only. The run answers
about the newest page it was told. On any other kind of run `pageContext` answers `400 Bad Request`.

A message on an application run MAY carry `scope`, the `src` a `jc-select` of the preview named (Architecture/20 §5, SDK-46): `{file}:{line}` with `file` an interface file under `src/` ending in `.tsx` and `line` from 1. The `message` event carries it, and the editing agent writes that file only for this instruction. Any other shape, a path outside `src/` and a `scope` on a `conversation` answer `400 Bad Request`.

### Calling a Function

The page that frames a code run's preview forwards the SDK's `/functions/{fn}` requests here (Architecture/20 §3, SDK-23):

```http
POST /api/v1/projects/{project}/agent-runs/{id}/functions/summary?types=BikeHireDockingStation HTTP/1.1
Content-Type: application/json

{ "types": ["BikeHireDockingStation"] }
```

The Portal transpiles the run's current `functions/**` (tests excluded), sends them with the SDK's server module, the request (`method` `POST`, `query` from the query string, `body`, `user` the caller's identity) and the caller's access token to `jc-functions`, and answers with the function's own status and body. A function that threw, ran out of memory or time, or returned more than 1 MiB answers `500` with `{ "error": { "message", "file", "line" } }`. `fn` must match `[a-z][a-z0-9-]{0,39}` and name a file `functions/{fn}.ts` of the run, else `404`; a function file that does not build answers `400` with `errors` as the preview does. The caller's token is the one the request was authenticated with (the edge's `X-Access-Token` or a bearer); a Portal cookie session carries none, and its function calls go to the endpoint anonymously, so a function never reads more than its caller may. Every call is a `tool` event named `function:{fn}` with `status`, `durationMs`, the request body as `input`, and the answer's status and the function's `ctx.log` lines as `output` or the error. `503` when the Portal has no `jc-functions` address (`JC_FUNCTIONS_URL`) or no Keycloak client to authenticate to it with, `429` when the runtime is full, `409` when the run has ended.

The editing agent's own `call_function` has no person's token to send. The Portal asks the agent proxy for a five-minute data credential of the run and calls `jc-functions` with it and `"via": "proxy"`. The function's requests then go through the proxy's `/v1/data/endpoints/{slug}/…` and read exactly what the run reads, as its person (ADR-N-038 decision 6). Without a proxy address (`JC_AGENT_PROXY_URL`), or when the run holds no delegated grant, the function's data calls answer `401`. The tool says so to the agent, and the function itself is not at fault.

### Reporting a Preview Error

The page that frames a code run's preview relays each runtime error the frame posts (`{kind: "jc-error", message, file, line}`, Architecture/20 §5) to the run:

```http
POST /api/v1/projects/{project}/agent-runs/{id}/preview-errors HTTP/1.1
Content-Type: application/json

{
  "message": "Cannot read properties of undefined (reading 'value')",
  "file": "src/pages/Overview.tsx",
  "line": 42
}
```

Response: `204 No Content`

The error is appended as a `preview_error` event with `reportedBy`; the first run's repair and the editing agent's `preview_errors` tool read it (SDK-14, SDK-20). `message` is required, at most 2000 characters and not blank; `file` is optional, at most 256 characters; `line` is optional and at least 1. Every field is untrusted text the frame wrote. A value outside those bounds answers `400 Bad Request`, a body with any other field `422 Unprocessable Entity`, and a run that has ended `409 Conflict`.

The same page relays the first `jc-observation` the frame posts for each preview version (SDK-27):

```http
POST /api/v1/projects/{project}/agent-runs/{id}/preview-observations HTTP/1.1
Content-Type: application/json

{
  "version": 2,
  "pages": [
    { "label": "Overview", "text": "Stations 5 Bikes available 24 …", "rows": [], "h1": 1 },
    { "label": "Stations", "text": "Kaivopuisto 7 Laivasillankatu 2 …", "rows": [5], "h1": 1 }
  ],
  "failedRequests": [
    { "path": "/functions/summary", "status": 500 }
  ]
}
```

Response: `204 No Content`

The observation is appended as a `preview_observation` event with `reportedBy`, and the run's verification reads it (SDK-28). `version` is the `v` of the preview URL the frame loaded, at least 1; `pages` holds 1 to 20 pages, each `label` at most 120 characters, `text` at most 20,000 characters, `rows` at most 50 counts and the optional `h1`, the number of level-1 headings the page shows (`<h1>` or `role="heading"` with `aria-level="1"`), at most 50 (an SDK older than T-3060 sends none); `failedRequests` holds at most 50 entries, each `path` at most 256 characters and `status` between 400 and 599. Every field is untrusted text the frame wrote. A value outside those bounds answers `400 Bad Request`, a body with any other field `422 Unprocessable Entity`, a second observation of the same version `409 Conflict`, and a run that has ended `409 Conflict`.

## 6. Cancel and Publish Actions

### Cancel an In-Flight Run

```http
POST /api/v1/projects/{project}/agent-runs/{id}/cancel HTTP/1.1
```

Response: `200 OK`

Deletes the Kubernetes workspace Job, invalidates the ticket hash, and transitions run status to `cancelled`. Pushed Git commits remain intact in the forge.

### Publish an Application

```http
POST /api/v1/projects/{project}/agent-runs/{id}/publish HTTP/1.1
```

Response: `202 Accepted`

```json
{
  "apiVersion": "joinedcontext.com/v1alpha1",
  "kind": "Change",
  "metadata": { "name": "chg-0000021a", "namespace": "helsinki" },
  "status": {
    "lane": "yellow",
    "mergeRequest": "https://git.example.fi/hel/org/pulls/538",
    "plan": { "create": 1, "update": 0, "delete": 0 },
    "phase": "PendingApproval"
  }
}
```

Transitions the App manifest to `lifecycle: published` via standard Gitea pull request. Approval rules follow AP-10. For a `static` run it also opens (or reuses) the merge request from the run's branch into the default branch of the application's repository, and the manifest's `spec.source.git` names that repository and the branch's head commit; approving the Change merges that merge request (AP-77).

A run whose newest version's tests failed, or are still running in the sandbox, is not published: `409 Conflict` with a `detail` naming the version and up to five failing tests (SDK-38). A version whose tests were skipped because the installation has no sandbox publishes, and the build lane's run is then the check (SDK-24).

## 7. Internal Proxy Endpoints

Served on the Portal's separate internal listener (`JC_INTERNAL_BIND`, `joinedcontext-portal/src/config.rs`), which APISIX does not route and a NetworkPolicy opens to the agent proxy alone, so none of these calls is part of the public URL scheme. Authenticated via `Authorization: Bearer <proxy-token>`.

There are six (`src/api/internal/agent_runs.rs`, `router`):

| Route | What it is for |
|---|---|
| `POST /internal/agent-runs/events` | the run's events, relayed; a body over 64 KiB is refused on this door as on the proxy's |
| `GET /internal/agent-runs/{id}` | the run's parameters, `ticketHash` and profile constraints |
| `GET /internal/agent-runs/{id}/inbox` | what the person said, in `seq` order |
| `POST /internal/agent-runs/{id}/mcp` | the operations registry: the same MCP dispatcher a person's client speaks to, entered as the person who started the run and narrowed by the run's profile (AG-64, AG-70) |
| `GET /internal/agent-runs/{id}/diagnostics/{component}/{name}` | §7.1 |
| `POST /internal/model-key` | §7.2: the model key's state, as the proxy last saw it (AG-96) |

### Receive Run Events from Proxy

```http
POST http://portal-internal:9090/internal/agent-runs/events HTTP/1.1
Authorization: Bearer <proxy-jwt>
Content-Type: application/json

{
  "runId": "e3b0c442-98fc-1c14-9afb-4c7b2756a120",
  "kind": "thought",
  "payload": { "text": "Analyzing schema slots..." }
}
```

Response: `201 Created` with allocated `seq` number.

### Fetch Run Context for Proxy Validation

```http
GET http://portal-internal:9090/internal/agent-runs/{id} HTTP/1.1
Authorization: Bearer <proxy-jwt>
```

Response: `200 OK` containing run parameters, `ticketHash`, and profile constraints.

In layout 2 (CC-85) a workspace run's folder is in its project's own repository: the context
then carries `repository`, that repository's name, and `pathPrefix` as that repository spells
it (`apps/{app}/`, without `projects/{project}/`). The proxy's forge route forwards to that one
repository and to no other, whatever the request names, so a run of one project reads and
writes nothing of another project's repository nor of the organization's (AG-86, CC-87).
Without `repository` the route reaches the configuration repository, as in layout 1.

The context names the run's `kind` (`conversation`, `application`, `dashboard`, `analysis`), from
which the proxy reads whose daily budget a call counts against (AG-97).

### Read What the Person Said

The inbox is the one place a workspace reads from: the answers and the instructions of section 5,
in `seq` order, so an agent needs no second channel to be steered. The call waits up to 25 seconds
for something new before answering with an empty list, which is the long-poll the answer route
signals.

```http
GET http://portal-internal:9090/internal/agent-runs/{id}/inbox?after=19 HTTP/1.1
Authorization: Bearer <proxy-jwt>
```

Response: `200 OK`

```json
{
  "items": [
    { "seq": 20, "kind": "message", "payload": { "text": "Sort by free bikes, not by name.", "sentBy": "jana.kovacova" } }
  ]
}
```

The proxy exposes it to the workspace as `GET /v1/runs/inbox?after={seq}`, with the run taken from
the request's own ticket: a workspace cannot name another run's id (AG-52).

### 7.1 The diagnostics door

The workspace asks the proxy for `/v1/diagnostics/{component}/{id}` with its `X-JC-Run` and `X-JC-Ticket` headers, the proxy asks the Portal's internal listener with its own bearer, and the Portal answers only for a resource of the run's project (AG-57). A `pipeline` name answers `200 OK` with the pipeline's counters (`received`, `sent`, `errors`, `bufferDepth`, as the runner reports them), `application/json`; a `change` id answers with the change's state, lane, plan and merge or apply error. `400` names a component the door does not know or an id that is not a name; `404` a resource outside the run's project. The body has passed the redaction of AG-56.

### 7.2 The model key's state

```http
POST http://portal-internal:9090/internal/model-key HTTP/1.1
Authorization: Bearer <proxy-jwt>
Content-Type: application/json

{ "state": "valid", "source": "probe", "limit": 10.0, "usage": 2.41, "remaining": 7.59 }
```

`state` is one of `valid`, `invalid`, `out_of_credit` and `unreachable`; `source` is `probe` or
`call`; `limit`, `usage` and `remaining` are the provider's credits, `remaining` being what is
left to spend (the smaller of the key's remaining limit and the account's balance), and are `null` or absent when
the provider said nothing about them (a key without a limit has `limit: null`). Any other member is
refused with `400`. Response: `204 No Content`. The body never carries the key (AG-96).

### 7.3 A call past the daily budget

The proxy answers a model call past a daily cap (AG-97) before it reaches the provider:

```http
HTTP/1.1 429 Too Many Requests
Content-Type: application/problem+json

{
  "type": "https://joinedcontext.com/errors/daily-budget",
  "title": "Daily Budget Spent",
  "status": 429,
  "detail": "Today's model budget for the assistant is spent. It starts again at 00:00 UTC; an administrator can raise it."
}
```

## 8. Conversations

The Conversations API starts open-ended assistant sessions that are not bound to an application name or endpoint, providing interactive dialogue and invoking platform operations from the central registry (UI-53, AG-67).

### Start or Continue a Conversation

```http
POST /api/v1/projects/{project}/assistant/conversations HTTP/1.1
Host: portal.example.joinedcontext.com
Authorization: Bearer eyJhbGciOiJSUzI1Ni...
Content-Type: application/json

{
  "message": "Find air quality stations in Pasila with particulate matter readings",
  "profile": "steward",
  "continues": null,
  "endpointNames": ["helsinki-all", "helsinki-kpi"],
  "formContext": { "kind": "Pipeline", "name": "citybikes-gbfs", "field": "spec.source.query.q" }
}
```

Request payload properties:

- `message`: The initial prompt or question; required unless `path` is given, when it may be absent or empty.
- `path`: Optional, the path the person picked in the empty assistant (AG-87, [ADR-N-032](../Decisions/adr-n-032-assistant-paths.md)): one of `integrate-pipeline`, `upload-data`, `find-data`, `share-data`, `build-app`, `build-dashboard`, `create-data-model`, `define-kpi`. Any other value answers `422 Unprocessable Entity`, as a body the route cannot read does, and a path whose kind (the table below) the caller may not propose answers `403 Forbidden` naming the kind, the same reason the dock shows on the disabled option. See [Paths](#paths).
- `profile`: Optional string naming the `AgentProfile` to execute. Defaults to the project default assistant profile.
- `formContext`: Optional, where the person is standing when they ask (UI-61, AG-77): `kind` is the
  manifest kind whose form is open, `name` the draft that form is editing, and `field` the path of the
  field they were last in (`spec.caching.maxAgeSeconds`, `spec.pages[0].layers`), each optional but
  `name` only meaningful with `kind`. The Portal validates `kind` against the kinds it knows, `name` as
  a DNS-1123 label and `field` as a dotted path of at most 200 characters, and answers `400 Bad Request`
  otherwise. It carries **no values**: the form's own content is the person's draft, which the assistant
  reads with `jc_draft_get` and changes with `jc_draft_put` under the caller's own grants, so a secret
  typed into a form never travels in a prompt and a field the caller may not read stays unread. With it
  the run is told which form is open and which field was asked about, so "what goes here?" is answerable
  and "fill this from a sentence" writes the draft the open form is already showing — the form picks the
  new values up from the draft's own event stream. The assistant still never proposes (AG-77).
- `pageContext`: Optional, the page the person is on when they ask (UI-61, AG-77, T-2763): `{ "route": "/projects/helsinki/spaces/helsinki?tab=inside" }`, the address bar's path and query. The Portal reads it against its own page table, never trusting a kind the browser names:
  - the path is `/projects/{project}/{page}[/{name}[/{sub}]]`, with `{project}` the conversation's own;
  - `{page}` is a kind's plural (or `models` for DataModel) or another page of the project (`approvals`, `activity`, `import`, `explore` and the rest);
  - `{name}` and `{sub}` are DNS-1123 labels, and `new`, `edit` or `complete` are actions, never names;
  - a `tab` query parameter, when it is a label, is kept; every other query parameter is dropped.
  The route is at most 300 characters. Anything else answers `400 Bad Request`. It carries names only, never a value. The run is told which page is open and how to read it: `jc_resource_get` on the kind and name of a detail page, `jc_resource_list` on a list. A question about "this page" is answered from that page, never from a catalog search. The same `pageContext` may ride on every later message (`POST …/agent-runs/{id}/messages`), and the newest one is the page the run answers about.
- `endpointNames`: Optional, zero to five distinct endpoint names of the project the conversation may query (AG-75). Each is resolved like an application run's (AP-44) and refused with `403 Forbidden` when the profile does not grant reading it (AG-70); the gateway's policy for the person still decides every row. A continuation without it keeps the endpoints of the run it continues.
- `access`: Optional, the capabilities the person chose for the conversation (AG-92, T-2718): `{ "preset": "read" | "propose" | "build", "endpoints": { "<endpoint name>": "read" | "readWrite" } }`.
  - `read`: the reading tools only (`jc_ask`, `jc_ui_navigate`, `jc_switch_path`, `describe_tool`, `search_catalog`, `jc_catalog_search`, `query_endpoint`, and the registry's read-only operations such as `jc_resource_get`), and of the paths only `find-data`.
  - `propose`: also every tool that drafts something the person proposes (`change_resource`, `propose_endpoint`, `edit_endpoint`, `grant_role`, `write_entities`, `space_complete`, `compute_kpi`, `draft_kpi_pipeline` and the registry's operations that are not read-only), and the paths `share-data`, `upload-data` and `create-data-model`.
  - `build`: also the paths `build-app`, `build-dashboard`, `integrate-pipeline` and `define-kpi`.
  - Per endpoint, `readWrite` lets `write_entities` prepare a change to that endpoint's entities; `read`, the default for an endpoint not named, does not.
  The choice only narrows: the Portal intersects it with the person's permissions and the profile's `spec.access` (AG-70). A tool outside it is a `failed` `tool` step naming the choice, which goes back to the model; a `path` outside it answers `403 Forbidden` naming the preset. Without `access` nothing is narrowed beyond AG-70. Any other shape answers `400 Bad Request`.
- `continues`: Optional string referencing the `id` of an ended conversation run. When supplied, the Portal seeds the new run with the prior conversation transcript (user and assistant messages and tool results, newest retained if token budgets require truncation) and links the thread.

Response: `202 Accepted`

```json
{
  "id": "f4c1b553-18dc-2e25-8afb-5c8b3867b231",
  "project": "helsinki",
  "appName": null,
  "endpointName": null,
  "endpointSlug": null,
  "profile": "steward",
  "appClass": null,
  "visibility": null,
  "kind": "conversation",
  "unattended": false,
  "continues": null,
  "prompt": "Find air quality stations in Pasila with particulate matter readings",
  "promptDigest": "sha256:7c92a188f121d5c2e9a59b9a6712bb1478ec7b3ad295c9604eb8b8c2c0406852",
  "dataNeeds": [],
  "allowsWrite": false,
  "branch": null,
  "pathPrefix": null,
  "status": "queued",
  "steps": 0,
  "tokensUsed": 0,
  "createdBy": "demo.steward@hel.fi",
  "createdAt": "2026-09-14T11:00:00Z",
  "expiresAt": "2026-09-14T11:30:00Z"
}
```

Error responses:

- `400 Bad Request`: Returned if `continues` references a run ID that does not exist or references a run whose `kind` is not `"conversation"`.
- `403 Forbidden`: Returned if the authenticated caller lacks the required assistant permission in the project (the `propose` verb on any resource kind; a person who may only read starts no conversation and spends no model calls, AG-67).
- `409 Conflict`: Returned when posting an instruction to a run that has reached a terminal state (`cancelled`, `expired`, `failed`, or completed). To resume dialogue, clients must invoke this route with `continues: <runId>`.

### Paths

A path is a guided flow (AG-87…AG-91). It is started by `path` on the first request, or chosen for free text, and it narrows what the assistant may do to what the flow needs.

**The first step answers without the model** (AG-91). On a request with `path`, the run publishes, before any model call:

1. a `path` event, `{ "path": "integrate-pipeline", "by": "person", "elapsedMs": 41 }`;
2. the first step's `question` event, built as `jc_ask` builds one (section 5): a `pick` fills its options from what the person may read, and the event carries `elapsedMs`, the milliseconds since the request;
3. for a path whose first step works on a page, the `navigate` event that opens it.

A `pick` with nothing the person may read asks the same question as free text and says why in a `thought`. The answer is an `answer` event like any other, checked against what the question offered, and the next turn is the model's, on the path.

**Integrate a pipeline takes its next steps itself** (T-2695). Its first question carries `step: "integrate-source"`. The Portal answers some sources itself; the model never sees these steps:

1. A file: the Portal writes a `tool` event `profile_sample` with the file's rows and columns, `durationMs` and `elapsedMs`.
2. A feed's address: the Portal keeps it. The runner reads it once, in step 4, never twice.
3. After either, the Portal asks "Which space should it land in?" (`step: "integrate-target"`). The options are the spaces the person may read and `new`, "A new context space".
4. `new` runs `space_complete` on what was handed over, as the model's call would. That is one `tool` event with the drafts, each draft with its verdict (the pipeline's is its test run on the sample), then a `navigate` to `/projects/{project}/spaces/complete?space={name}` carrying the drafts. The person proposes them there, as one Change. The run proposes nothing.
5. From an address, the drafts are the space, its data model, a data source, an endpoint and the pipeline that reads the address.
6. A file is a sample of a feed. Before `new` drafts anything, the Portal asks "What is the address of the feed this file is a sample of?" (`step: "integrate-feed"`). The question takes an address (`input.url`) or the option `none`, "There is no feed".
7. With an address, the drafts are those of step 5: the data source reads the address, and the pipeline's verdict is its test run on the file. Nothing fetches the address while drafting.
8. With `none`, only the space and its model are drafted, because the file is data once. A `thought` says so, and says the rows load through Import once the space is approved. Words instead go to the model, on the path.

An existing space, a data source, a context space as the source, or words: these go to the model, on the path, with where the data lands said plainly.

**Build an app is built in the conversation** (T-2696, T-2721). The Portal asks every question of the path itself; the model never sees them, and no form opens:

1. "Which endpoints should the app read?" (`step: "build-app-endpoints"`, `pick: endpoints`, several). The first is the app's endpoint, the rest are read beside it.
2. "What may the app do with the data?" (`step: "build-app-access"`): `read` "Only read it", `update` "Read and update records" or `full` "Read, add, update and delete records", the presets of AP-132.
3. "Who opens it?" (`step: "build-app-audience"`): `project` "Everyone in the project", `organization` "Everyone in the organization" or `private` "Only me". `public` is listed disabled with its reason: a run never builds a public app (AP-42); the App is made public on its own page, by a red-lane Change a publisher approves.
4. "What should the app do?" (`step: "build-app-describe"`), free text. Words typed in the text box while this question stands answer it.
5. An `app-build` event carries what was answered: `{ "endpoints": ["bikes", "air"], "access": "read", "visibility": "project", "prompt": "…", "elapsedMs": 412 }`, and a `thought` says it is ready to build.

The dock draws `app-build` as a card with **Build**. The person's own click starts the run with `POST /api/v1/projects/{project}/agent-runs` from the browser, under their own session, so the run is checked against the grants they hold as any run is (AP-132, PF-70) and the conversation starts no run itself (AG-11). The card computes `dataNeeds` from the endpoints' published models as the builder does; a write preset the person's grant does not carry falls back to reading, and the card says so. The dock then follows the run: its preview, its tests and each next message as an instruction to it, and Publish on the App's page proposes the Change and says who approves it. Words instead of a choice go to the model, on the path.

| `path` | Proposes (the guard) | First step | Tools beyond every path's |
|---|---|---|---|
| `integrate-pipeline` | `Pipeline` | "Where does the data come from?": a data source or a context space the project has (each disabled with the reason when there is none), a file dropped (`input.file`) or a feed's address (`input.url`) | `change_resource`, `jc_datasource_check`, `jc_pipeline_test`, `jc_pipeline_metrics`, `jc_manifest_dry_run`, `jc_draft_put` |
| `upload-data` | `ContextSpace` | "Which space should the data go into?" (`pick: spaces`), and opens the import page | `space_complete`, `change_resource`, `jc_manifest_dry_run`, `jc_draft_put` |
| `find-data` | none | "What are you looking for?", free text | none |
| `share-data` | `Endpoint` | "Which data do you want to share?" (`pick: endpoints`) | `propose_endpoint`, `edit_endpoint`, `grant_role`, `change_resource`, `jc_manifest_dry_run` |
| `build-app` | `App` | "Which endpoints should the app read?" (`pick: endpoints`, several), then what the app may do with the data, who opens it and what it should do, and the `app-build` card the person starts the run from | `change_resource`, `jc_manifest_dry_run`, `jc_draft_put` |
| `build-dashboard` | `Dashboard` | "Which endpoint should the dashboard draw?" (`pick: endpoints`) | `change_resource`, `jc_manifest_dry_run`, `jc_draft_put` |
| `create-data-model` | `DataModel` | "Where does the model start?": a Smart Data Model, a sample file, or nothing, and opens the models page | `change_resource`, `jc_manifest_dry_run`, `jc_draft_put` |
| `define-kpi` | `Pipeline` | "Which space do you measure?" (`pick: spaces`) | `compute_kpi`, `draft_kpi_pipeline`, `jc_kpi_compute`, `jc_pipeline_test`, `change_resource` |

Every path also has the conversation's own tools and the reading ones: `jc_ask`, `jc_ui_navigate`, `jc_switch_path`, `describe_tool`, `search_catalog`, `jc_catalog_search`, `query_endpoint`, `jc_resource_list`, `jc_resource_get`, `jc_draft_get`, `jc_draft_list`, `jc_change_list` and `jc_activity_list`.

**The tools of a path** (AG-89). The model's pack names the path, its goal and its tools, and the registry section lists only the path's operations. A call of any other tool is a `failed` `tool` step naming the path and the tools it has, and it goes back to the model. A path only narrows: every tool is still checked against the profile and the person at call time (AG-70), and no path writes anything; every write ends as a Change the person sends from the page the flow filled (AG-73).

**Free text chooses a path** (AG-88). The first message of a conversation that has no path is routed by `choose_path`: one short call to the profile's model with the eight paths and the message, which answers one path id and a reason, or none. The run publishes `{ "path": "find-data", "by": "router", "reason": "…", "elapsedMs": 850 }` and answers the message on that path. With none, or a path whose kind the person may not propose, the conversation goes on without a path, with every tool the profile and the person allow, as before paths.

**Switching** (AG-88). On any path the model may call `{ "tool": "jc_switch_path", "arguments": { "path": "share-data", "reason": "…" } }`. The run publishes `{ "path": "share-data", "by": "handover", "reason": "…" }`, and the rest of the turn runs on the new path. The dock shows the path event with an offer to switch back.

A continuation (`continues`) keeps the path of the run it continues, the last `path` event of its transcript.

### Proposing an Endpoint

The assistant's share tool, which renders manifests and writes nothing (EP-72, PF-50).

```http
POST /api/v1/projects/{project}/assistant/propose-endpoint HTTP/1.1
Content-Type: application/json

{
  "contextSpace": "air-quality",
  "name": "public-air",
  "title": "Ilmanlaatu",
  "audience": "project-list",
  "allowedProjects": ["mobility"],
  "representations": ["ngsi-ld", "csv"],
  "hiddenAttributes": ["contactPoint"],
  "entityTypes": ["AirQualityObserved"],
  "rateLimits": { "requestsPerMinute": 600 }
}
```

Response `200 OK`: `lane`, the fresh 26-character `slug` the Portal minted (never the model, EP-02),
the rendered `endpoint` manifest, the `policies` granting `retrieveOps` on the named types, any
placeholder `groups` a consumer project still needs, and `prefill` for the Endpoint form. Nothing
is created: the person submits the prefilled form through the ordinary change flow. A caller who
may not `propose` an `Endpoint` here gets `403` (PF-50); `audience` defaults to `project-list`,
never to `public`.

`access` (`read`, `update` or `full`, AP-132) renders the endpoint Build an app proposes inline.
`audience` must then be `project-list` naming this project alone (both default to that). A `read`
rendering carries no policy and no group: the person reads through the endpoint with the grants
they hold on the space. `update` and `full` need `entityTypes` and carry one Policy
`{name}-{project}` granting the preset's operations on those types to the project's group, and
`lane` is then the lane of the whole Change, red for a Policy. The builder proposes the rendering as one Change of its
own and starts the run once the endpoint is served.

## 9. Agent Access

What an assistant or agent may do is the intersection of its `AgentProfile`'s `spec.access` block and the permissions of the person who starts the run, evaluated at every call (AG-70). This route shows that intersection to the signed-in person (UI-56).

### Read the Effective Access

```http
GET /api/v1/projects/{project}/assistant/access HTTP/1.1
Host: portal.example.joinedcontext.com
Authorization: Bearer eyJhbGciOiJSUzI1Ni...
```

Response: `200 OK`

```json
{
  "items": [
    {
      "name": "app-builder",
      "title": "Application builder",
      "role": "builder",
      "access": {
        "operations": ["jc_catalog_search", "jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }],
        "endpoints": [{ "name": "helsinki-bikes", "verbs": ["read"] }]
      },
      "egressHosts": ["registry.npmjs.org"],
      "operations": [
        { "name": "jc_catalog_search", "readOnly": true, "profile": true, "person": true, "reason": null },
        { "name": "jc_space_complete", "readOnly": false, "profile": false, "person": true, "reason": "the agent profile does not grant jc_space_complete (AG-70)" }
      ]
    }
  ]
}
```

Response properties:

- `items`: Every `AgentProfile` of the organization, ordered by `name`.
- `access`: The profile's `spec.access` block as written, or `null` when the profile has none; a profile without one may call only operations annotated `readOnlyHint: true`.
- `egressHosts`: The hosts the profile's workspace may reach (AG-50).
- `operations`: Every operation of the registry (ADR-N-021). `profile` is the profile's half, `person` is the caller's own permission in the project (PF-50, PF-51), and the operation is available to a run the caller starts only when both are `true`; `reason` names the half that refuses, `null` when neither does.

The page edits a profile through the change flow of the `agentprofiles` resource (`PUT /api/v1/projects/org/agentprofiles/{name}`, CC-03): the edit is a Change reviewed like any manifest, never a direct write (UI-56). Admission refuses an `access.operations` entry that is not a registered operation (MF-40).

Error responses:

- `401 Unauthorized`: Returned when the request carries no session or bearer.

### Endpoint Grants on Run Creation

When a profile's `access` block lists `endpoints`, `POST /api/v1/projects/{project}/agent-runs` answers `403 Forbidden` for a run whose endpoint the list does not name with `read`, or with `write` when the run's data needs carry a write operation (AG-70). A profile whose `access` block lists no endpoints, or that has no `access` block, leaves the endpoint to the person's own grants and the Endpoint's Policy (EP-55).

## Related

- [01-portal-api.md](01-portal-api.md) — primary platform REST API specification.
- [19-agent-runner.md](../Architecture/19-agent-runner.md) — architectural specification for the runner and proxy.
- [16-apps-on-demand.md](../Architecture/16-apps-on-demand.md) — application lifecycle and access controls.
