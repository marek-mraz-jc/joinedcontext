---
sidebar_position: 6
title: "Knowledge Assistant API"
description: The chat route of jc-assistant, its Server-Sent Events, and how the assistant calls jc-agent-proxy for a model.
---

# Knowledge Assistant API

`jc-assistant` ([Architecture/22](../Architecture/22-knowledge-assistant.md), [ADR-N-040](../Decisions/adr-n-040-knowledge-assistant.md)) answers one question per request on the channel an `AssistantDeployment` names. This page is the contract between a channel and the service, and between the service and `jc-agent-proxy`.

## 1. Chat

`POST https://assistant.{domain}/api/v1/d/{publicId}/chat`

`publicId` is the deployment's `spec.publicId`, unique across the organization. An unknown one, one of a channel this route does not serve, or one two deployments declare, answers `404`: no load order decides which project answers on an address (MF-52, T-3283).

### 1.1 Request

```json
{
  "conversation": "6f1c0e9e-3b1a-4d7e-9b51-2c4f8f2a7c11",
  "message": "Kde sú meracie stanice kvality ovzdušia?",
  "history": [
    { "role": "user", "text": "Aké sú dnešné podujatia?" },
    { "role": "assistant", "text": "Dnes sú dve podujatia: … [1]" }
  ],
  "connectors": ["ovzdusie-verejne"]
}
```

- `message` is 1 to 4,000 characters after trimming; `conversation`, when present, is an id this service issued; `history` holds at most 6 turns of `user` or `assistant`, each at most 4,000 characters; `connectors`, when present, is a subset of the deployment's connector Endpoints, and an empty list switches every connector off. Anything else answers `400` naming the field. The body is at most 64 KiB. (AG-98)
- The service keeps each turn, with its secrets masked, for the deployment's `transcriptRetention` (default 90 days) so its administrators can see what an answer did (§3.1); the visitor's chat still sends `history` back with each question, and a turn of `history` is the caller's own text, so the rules in §1.4 treat it as untrusted the same as the question. (AG-99)

### 1.2 Who may ask

| Channel | Caller | Served |
|---|---|---|
| `public`, `ckan`, `iframe` | anyone; a browser's `Origin` must be one of `allowedOrigins` | by this route |
| `internal` | a signed-in person, in the Portal | through the Portal (§1.7); `404` here |

- A request whose `Origin` is present and not in the deployment's `allowedOrigins` answers `403`. The route answers CORS preflight for the allowed origins only, with `Access-Control-Allow-Origin` naming the origin, never `*`. (AG-100)
- On these channels every question carries a visitor token (§1.2.1) in `X-JC-Visitor`. A request without one, or with one that is forged, expired or another deployment's, answers `401` with a `detail` the chat shows, in Slovak; the widget then gets a new token and asks again once. (T-4653)
- A request that names no `Origin` is no browser on an allowed site: it also counts in a pool of its own, a sixth of `rateLimit.requestsPerMinute` (one at least), and all such requests share one visitor's share of the day's cost. (T-4653)
- `rateLimit.requestsPerMinute` counts every request of the deployment, `rateLimit.perClientPerMinute` the requests of one client address: `X-Forwarded-For` read from the right, past the cluster's own private, loopback and link-local proxy hops, to the address the edge appended, so an address a caller writes in front changes nothing (T-3059). Past any of them, `429` with `Retry-After` and a `detail` the chat shows, in Slovak. (AG-101)
- One client also has `rateLimit.perClientPerHour`, `rateLimit.perClientPerDay` and `rateLimit.perClientPerMonth` questions (the month is the UTC calendar month; default 300 when the manifest names none), and `rateLimit.streamsPerClient` answers in progress at once. A request that one window refuses counts in none. The service knows a client by a keyed hash of the address and the UTC day (the month, for the monthly window), with a key that is random per process; it stores and logs no address (T-4149, T-4286). Past the monthly window that client's stream is one `error` event (`429`, `Monthly Limit Reached`, "Dosiahli ste mesačný limit otázok. Obnoví sa 1. 11. 2026.", the first of the next month) and `done`, with no model call and no search; the minute, hour and day windows still count, and other clients are not affected. (AG-101, T-4671)
- The visitor token's id counts a minute of `rateLimit.perClientPerMinute` and `rateLimit.streamsPerClient` of its own, so people behind one address (an office, a school, a mobile carrier) each have their own minute. The hour, day and month windows and the cost share below stay the address's: a new token never buys a fresh month or cost share. With a token, the address's minute is five times `rateLimit.perClientPerMinute`. (T-4653)
- One visitor, or the no-`Origin` pool together, spends at most `budget.visitorSharePercent` percent (default 10) of `budget.costPerDayUsd` a day; a deployment many people reach from one address (an office behind one NAT) raises it and keeps `rateLimit.perClientPerMinute` and `perClientPerHour` as the pace one address cannot pass (T-5928); past that, their questions are refused `429` (`Visitor Limit Reached`, "Dnes ste položili veľa otázok. Skúste to znova neskôr.") with `Retry-After` up to the next UTC midnight, while other visitors still get answers. `history` turns on these channels are cut to 1,000 characters each. (T-4653, T-5928)
- `budget.questionChars` caps the question (`400` past it) and `budget.messagesPerConversation` the questions of one conversation (`429`, start a new one). One answer takes at most 30 seconds; past that, an `error` event with status `504`. `enabled: false` answers every request `503`. (T-4149)
- `budget.costPerDayUsd` caps the day's cost: the sum of each model response's `usage.cost`, per deployment and UTC day. From the request that finds the cap reached, the stream is one `error` event (`429`, `Daily Limit Reached`, "Asistent dnes dosiahol denný limit. Skúste to znova zajtra po 2:00.", the next UTC midnight in the deployment's zone) and `done` with nothing spent: no model call, no search, no answer, citation, card or follow-up. A question that already started finishes its answer call; the next one is refused. Today's spend that cannot be read refuses the same way (`503`, `Assistant Unavailable`, "Asistent je dočasne nedostupný. Skúste to o chvíľu.") and logs an error. The service warns once a day per deployment at the first cap and logs each refusal with its `cap` (`day`, `month`, `share`, `unknown`). (T-4149, AG-107, T-4671)
- `budget.tokensPerDay` is the model proxy's cap (§2). A question whose model call the proxy refuses for the day gets the same `Daily Limit Reached` event, whatever it streamed before, and never the proxy's own words. `costPerDayUsd` is the guard a deployment is sized by; a public deployment's `tokensPerDay` is set high enough that the cost cap binds first (T-4286).
- The service answers at most 32 questions at once across its deployments; a question past that is refused, never queued: `503` with `Retry-After: 5` and nothing spent (T-3059).
- One model reply may carry at most 4 tool calls and each call's arguments at most 4,096 characters as JSON; a call past either is answered to the model with a refusal instead of being run (T-3059).

#### 1.2.1 Visitor token

A visitor token proves that the browser spent a moment of work, and gives each person behind one address a minute of their own (T-4653). It carries a random id and its expiry, nothing about the person, and the service stores nothing per token: the challenge and the token are signed with a key that is random per process, so a restart only makes the widget fetch a new one.

1. `GET https://assistant.{domain}/api/v1/d/{publicId}/challenge` answers `200`, `Cache-Control: no-store`:

   ```json
   { "challenge": "3q2-7w.1760792400.14.9f2c…", "difficulty": 14, "expiresAt": "2026-10-18T13:00:00Z" }
   ```

   `challenge` is opaque and valid for 2 minutes. `difficulty` is the number of leading zero bits the proof needs: 14, and 18 while the deployment's minute is above 80 % of `requestsPerMinute`, so a script pays more work per token when the assistant is busy.
2. The widget finds a decimal `nonce` such that SHA-256 of the UTF-8 text `{challenge}:{nonce}` starts with `difficulty` zero bits, in a Web Worker (`crypto.subtle`), while the panel opens.
3. `POST https://assistant.{domain}/api/v1/d/{publicId}/token` with `{"challenge": "…", "nonce": "48211"}` answers `200`:

   ```json
   { "token": "a41f….1760795940.0c7d…", "expiresAt": "2026-10-18T14:00:00Z" }
   ```

   valid for 1 hour. A challenge that is malformed, expired, not signed by this service for this `publicId`, or a nonce whose hash does not meet the difficulty answers `400`. The same solved challenge always gives the same visitor id, so replaying one proof is one visitor. One address redeems at most 10 tokens a minute per deployment (`429` past it); a browser needs one an hour. The challenge route keeps no state and is left to the edge's per-address limit.

Both routes follow the `Origin` rules of §1.2, and the preflight allows `GET`, `POST` and the `content-type` and `x-jc-visitor` headers. The MCP and API channels keep their own authentication and need no visitor token.

### 1.3 Answer: Server-Sent Events

`200`, `Content-Type: text/event-stream`. Events, in this order:

| Event | Data | When |
|---|---|---|
| `conversation` | `{"id": "<uuid>", "at": "2026-10-16T12:17:05Z", "turn": "<uuid>"}`; `at` is the server's moment of the question in UTC, the one every model call of the answer is told (T-4339); `turn` names this answer for `resume` (§1.6.2) and is present on a visitor's stream alone, one that carried a visitor token (T-4685) | first, always |
| `stage` | `{"stage": "understanding" \| "searching" \| "reading" \| "history" \| "schema" \| "answering", "topic": "otváracie hodiny knižnice"}`; `topic` is the search query, the entity type read (`reading`, `history`) or the Endpoint whose types or schema are read (`schema`), absent otherwise | before each step of the answer; the chat shows the latest as one line until the first `delta` or the `answer` (T-4155, T-4294). A public chat never shows `topic` as written except for `searching`: a type read is named in words, an Endpoint slug never; the "Postup" list gives each step its words and its time (T-4682) |
| `tool` | `{"name": "search", "status": "started" \| "done" \| "failed"}`; a connector's tool adds `"endpoint"` | around each tool call |
| `step` | `{"source": "Registre kraja", "type": "Bridge", "q": "yearBuilt<1900", "count": 32, "url": "https://…/dataset/mosty"}`; each member absent when the read has none: `source` the Endpoint's title (its publisher when it has none, never its slug), `titles` the same title per language when the manifest writes it as a language map (`{"sk": "Podujatia — Banská Bystrica", "en": "Events — Banská Bystrica"}`; T-5343), `type` and `q` as the read asked them (`q` at most 200 characters), `count` the result's `total`, how many match, absent when the source gave none (a page is not a count), `url` the dataset page its citation links | after each read of live data that answered; a sentence of the answer that says the data does not hold something cites no page, only the dataset it read (T-4801); the chat lists them under the answer as "Postup" (sk, cs) or "Steps" (en), one line per read with its count and the dataset link, so a person can check each figure; a cached answer replays them (T-4801). A line names the source by its title in the chat's language, the type in words only when the title does not already say it, and the `q` in words: a date bound as "od 20. 10. 2026" / "from 10/20/2026", any other clause as its attribute's label, an operator sign and the value; a `q` with `\|` or parentheses stays as written. A read without a count of a source and type another read counted, and a line equal to an earlier one, are left out; of two `data` events of one Endpoint and type the chat shows the first table only (T-5343) |
| `script` | `{"code": "…", "output": "…"}`, or `"error"` in place of `"output"` | after a `run_script` call (§1.5) |
| `data` | `{"endpoint": "ovzdusie-verejne", "type": "AirQualityObserved", "columns": ["name", "pm10", "observedAt"], "units": [null, "µg/m³", null], "rows": [["Námestie SNP", 21, "2026-10-15T18:00:00Z"]], "dataset": "https://…/dataset/public-air", "download": "https://…/api/endpoint/…/ngsi-ld/v1/entities?type=…"}` | after a data round, before `delta`; at most 50 rows. The columns come by importance (T-4650): `name` (else `id`), the attributes the query named, measured values (a number with a `unitCode` or an `observedAt`), other numbers, categories, the record's metadata, and last one `observedAt`, the row's newest measurement. `units` holds, per column, the symbol (else the name) of the `unitCode` it carries, or `null`. The chat labels each column in the visitor's language with its unit, aligns numbers right and formats numbers, yes/no and times for that language. When rows carry a GeoJSON `location` and the deployment has a `viz.{domain}`, `map` holds the map spec of ADR-N-053 §2.9 built from them (the label, up to four of the values the table leads with, and `observedAt`), and the chat offers "Table \| Map", the map first when every row has a location (T-4657). Per column, `labels` holds the model's label as a language map (`{"sk": "Rok výstavby", "en": "Year built"}`) or `null`, `summary` whether the model marks it as a summary attribute, and `enums` the words of each enum value as `{"notListed": {"sk": "nie je chránená", "en": "not listed"}}` or `null`, all from the type's LinkML at no model cost (T-4672, T-4682). The label column's `enums` entry also holds each row's `title` (else a `name` that is a LanguageProperty) keyed by its cell, `{"index-starnutia-mesto": {"sk": "Index starnutia", "en": "Ageing index"}}`, so a `KeyPerformanceIndicator`, whose `name` is its slug, reads in words (T-5330). The chat shows at most 20 rows, the entity's label first and never the URN (the URN tail stands in where a row has no `name`), no Relationship column, years and codes without grouping, an enum in words, a URL as a short link, and an empty cell empty; `dateModified` is dropped when the row has `observedAt` (T-4682). Rows whose `location` is a Polygon or MultiPolygon also carry two columns `area` and `perimeter`, measured on the server on the WGS84 ellipsoid (geodesic, holes subtracted), each in the unit of its largest value (`m²`, `ha` or `km²`; `m` or `km`) to three significant figures; their outlines reach the map simplified (Douglas-Peucker to a thousandth of the extent, never finer than a metre, never into an invalid polygon). A table of one such row carries `geojson`, the entity's OGC API Features item `https://…/api/endpoint/{slug}/ogc/features/collections/{type}/items/{id}`, which the chat offers as "Stiahnuť GeoJSON" (T-4697) |
| `visualization` | `{"spec": {…}, "rows": [{"name": "…", "pm10": 21}]}`: a Vega-Lite 5 spec over `rows`, or a map spec (ADR-N-053 §2.9) with no `rows` | after a `show_visualization` call the service accepted, at most once per answer; the chat draws it in a frame of `https://viz.{domain}/#show` (T-4657). `labels` maps a field to its label in the visitor's language, which the chat writes as the title and axis names; a chart of fewer than 3 values is refused, and the categories are the entities' names, never their URNs (T-4682). The visuals follow the answer's text, folded when there is more than one |
| `delta` | `{"text": "…"}` | zero or more, the answer's next characters as the answering model streams them |
| `answer` | `{"text": "… [1] … [2]"}` | once, the whole answer, after the last `delta` |
| `citations` | `[{"n": 1, "url": "https://…/page#page=4", "title": "Ovzdušie"}, {"n": 2, "tool": "query_entities", "endpoint": "ovzdusie-verejne", "url": "https://…/dataset/ovzdusie", "title": "Kvalita ovzdušia"}]` | after `answer` |
| `followups` | `{"questions": ["Kde je zberný dvor?", "…"]}` | after `citations`, at most once: up to 3 questions of at most 120 characters each that the router call proposed (T-4139), no extra model call; the router offers up to 5 and names for each the attributes that answer it; one naming an attribute the read Endpoints do not list (every Endpoint shown when none is read) is dropped, as is, where live data is shown, one that names none, and every one that asks about failures, damage, danger, accidents, crime, complaints or blame; the first 3 left are sent (T-4801); a cached answer replays the ones it was sent with (T-4731) |
| `error` | `{"status": 429, "title": "Budget Spent", "detail": "…"}` | instead of `answer` |
| `done` | `{"tokens": 1834, "tokensIn": 1600, "tokensOut": 234, "modelCalls": 2, "firstTokenMs": 900, "totalMs": 3100, "cached": false, "costUsd": 0.000412, "stagesMs": {"route": 420, "search": 40, "judge": 380, "answer": 2100}, "judgeSkipped": false, "providers": {"route": "Groq", "judge": "Groq", "answer": "Google Vertex"}, "reasoningTokens": {"route": 0, "judge": 0, "answer": 64}}` | last, always; the token counts are the sums of each model response's `usage` (`prompt_tokens`, `completion_tokens`), `tokens` their total; `costUsd` is the sum of each response's `usage.cost`, present only when every call reported one, and 0 for a cached answer or one that made no call (T-4178); `stagesMs` is the wall time of each stage that ran, in milliseconds: `route` the router call, `search` and `judge` summed over the rounds, `answer` the streamed answer call from request to last token. A stage that did not run is absent. The question's own search and its judgement run beside the router call, so the stages may add up to more than `totalMs` (T-4293); `judgeSkipped` is true when the first round's passages covered the question and were not judged; `providers` names, per stage, the OpenRouter provider that served its last call, and `reasoningTokens` the reasoning tokens its calls spent (`usage.completion_tokens_details.reasoning_tokens`), each stage only when a call reported it (T-4360) |

- Every model call of the answer reads the moment of the question beside it, local first: the deployment's weekday, date, time to the minute and zone abbreviation (CET or CEST, EET or EEST, from the IANA zone on that date), then UTC. The zone is that of the deployment's first city language (`sk` Europe/Bratislava, `cs` Europe/Prague, `fi` Europe/Helsinki), UTC alone without one. "Today", "now" and "this week" resolve against it, and a question about the date or time is answered from it, like "Je piatok 16. októbra 2026, 14:17 (CEST)." The chat shows the time of each message, the question's from `at`, in the visitor's time zone, with the UTC minute beside it. (T-4339)
- Every numbered marker `[n]` in `answer` has its entry in `citations`: a passage cites its page URL (a PDF passage with `#page=N`), a tool result cites the tool and its Endpoint. (AG-102)
- A passage's citation also carries `published`, `modified` and `crawled` of its page or document, RFC 3339 in UTC, each left out when unknown ([Architecture/22 §3](../Architecture/22-knowledge-assistant.md#3-database-sketch)); the answer call sees the same three dates beside each passage. (T-4749)
- `title` is the first heading of the cited page or document, when it opens with one. A tool result's citation also carries the `url` and `title` of the page the deployment's sources hold about its Endpoint (the catalogue's or its dataset's, found by the Endpoint's address), when one exists. Both come from the deployment's own sources, public ones alone on a public channel. A channel shows the title and never the tool's name. (AG-117)
- A passage citation may carry `cards`, what the cited passage itself publishes, read from its text and links without a model call: `{"kind": "contact", "name": "Klientske centrum", "phones": ["+421 48 4330 111"], "emails": ["podatelna@example.org"]}`, `{"kind": "place", "address": "Československej armády 26, 974 01 Banská Bystrica"}` or `{"kind": "procedure", "steps": ["Vyplňte žiadosť.", "…"]}`. `name` is the passage's last heading; empty lists and absent fields are left out; at most 3 cards per citation, 5 phones or e-mails per contact, 10 steps. The chat links a phone as `tel:`, an e-mail as `mailto:` and an address to an OpenStreetMap search only when each is well formed, and shows it as text otherwise. (T-4155)
- The answer is in the language of the question; the deployment's `languages` only choose the greeting and the search stemmers. (AG-103)
- An error the person can act on is an `error` event with a sentence: the conversation is long (`tokensPerConversation` reached; a question asked past `costPerConversationUsd` or `messagesPerConversation` is answered `429` before any event, §1.4; either way the chat offers a new conversation that asks the question again, T-4737), the model is unreachable, or a connector failed (the answer then goes on without it, and the `tool` event says `failed`). So is each cap of §1.2: a spent day (`tokensPerDay`, `costPerDayUsd`), a spent month, a spent share and unreadable spend; the chat shows a cap's sentence with no retry, since nothing changes before it resets. (AG-104, T-4286, T-4671)

### 1.4 The agent loop

- Crawled text, tool results and `history` are data, never instructions: they reach the model inside quoted blocks after rules that say so. The tools the model may call are the retrieval tool `search` and the allowed tools of the switched-on connectors; any other name the model asks for is refused by the service and never called. (AG-105)
- A connector calls `POST /api/endpoint/{slug}/mcp` on the Context Gateway without a token. On these channels a connector whose Endpoint is not `audience: public` is never offered, and `search` reads `visibility: public` passages of the deployment's `sources` only. (AG-106)
- A connector of another project, `{project}/{endpoint}` with `{project}` in the deployment's `scope.projects`, is offered on every channel only while that Endpoint is `audience: public`, is called without a token, and keeps that name in its `tool` and `data` events. An Endpoint of a project outside `scope.projects` is never reached. (MF-52, T-4177)
- A source of another project, `{project}/{source}` with `{project}` in the deployment's `scope.projects`, is searched in that project's own scope, with the same visibility rule as the deployment's own sources: `public` passages alone on every channel but `internal`. Each project's passages are ranked in their project and the rankings merged by fused score; a citation of another project's page carries that page's own address. A source of a project outside `scope.projects` is never read, and a project the deployment names no source of is never searched. (MF-52, T-4281)
- A data question runs a bounded loop on the server (T-4294, AG-107): the reads the router named are its first steps, then the answering model is offered the connectors' read-only tools and calls them as it needs (a filter, a count, history, a type's attributes), at most `answer.maxDataSteps` tool calls per question in all (default 10, up to 25) and within `answer.maxDataSeconds` of the question (default 20, up to 60); the whole answer may take that time plus 25 seconds, at least 30; a reply without a tool call is the answer, which arrives as one `answer` event without `delta`s. At the last call or the time limit the service asks for the answer from what was read, and the answer says what it could not read. A name not offered, arguments past 4,096 characters or against the tool's schema, and a call past the last are refused to the model and never sent; a call repeated with the same arguments is answered from the question's own results. Each `query_entities` result is also a `data` event. When the deployment has a `viz.{domain}`, the loop also offers `show_visualization`: `{"kind": "map" | "line" | "bar", "data": n, "label": "<column>", "value": "<column>", "time": "<column>", "title": "<at most 80 characters>"}`, `n` the number of a table the answer read, `label`, `value` and `time` columns of it (`value` numeric; `time` required for `line`, optional otherwise), and `map` only for a table whose rows carry a location. The service builds the spec from that table's rows and sends it as a `visualization` event; an unknown table, a column the table lacks, a map without a location or a second call in one answer is refused to the model, which answers without it. The call counts as one of the question's tool calls. (T-4657) A question the router sends to the sources alone keeps the routed answer of one streamed call.
- `answer.routes` decides what one answer may use: `both` (default) lets the router search the sources and call the connectors; `rag` offers no connector and calls no tool, and its `data` and `tool` events never occur; `data` runs no `search` round and answers from the connectors' results alone. (MF-52, T-4281)
- With `feedback: false` the chat shows no verdict buttons and `POST /api/v1/d/{publicId}/feedback` answers `404`. (MF-52, T-4281)
- A question takes one routing call of the router model (route `rag`, `data` or both, and up to 3 search queries), at most 3 retrieval rounds the service runs itself (a follow-up round asks the router once more, with the passages' titles only), and one streamed call of the answer model. (AG-107)
- Before each model call, the conversation's tokens so far plus the call's input estimate are checked against `tokensPerConversation`, and, as each question starts, the conversation's model cost so far against `costPerConversationUsd` when the deployment sets it: past it, or past `messagesPerConversation`, the question is answered `429` `Conversation Budget Spent` with no model call; a question under way finishes. The answering call's output is capped at `outputTokensPerAnswer`. (AG-107, T-4737)
- The first call sends the stable part (rules, the deployment's prompt, the tool list) as one cached prefix, marked `cache_control: {"type": "ephemeral"}`; every later call sends that prefix byte for byte, then the question and the tool results (T-3069). (AG-108)

### 1.5 Scripts over a large tool result

A tool result longer than 20,000 characters reaches the model cut. On a deployment with `sandbox: true` the model is offered `run_script` as well: `{"result": n, "code": "…"}`, JavaScript whose body receives the whole result `n` (at most 256 KiB) as `data` and returns what the model reads. `jc-assistant` runs it in `jc-functions` with no network, no file system, no clock and no environment, under its time, memory and output caps, and streams the code and its output to the person as a `script` event.

- An answer built on a script's output cites the tool result the script read (AG-102). The script itself is the model's text and runs only in the sandbox (AG-112).

### 1.6 The widget

`GET https://assistant.{domain}/d/{publicId}/widget` is the chat a site places in an iframe:

```html
<iframe src="https://assistant.{domain}/d/{publicId}/widget" title="Assistant" width="400" height="600"></iframe>
```

The page loads `/d/widget.js` and `/d/widget.css` from the same host and nothing else; it sets no
cookie. It keeps the conversation's id and its visible turns in its own `sessionStorage` (the
browser partitions it per host site and drops it with the tab), so the open conversation survives a
page change on the host site; "Nová konverzácia" clears it. The footer under the input reads "Dôležité informácie si prosím overte. · Súkromie" ("Please verify important information. · Privacy"); "Súkromie" opens the dialog "O tomto asistentovi" inside the widget (focus held inside, Escape closes): that answers come from AI and may be wrong, no sensitive personal data belongs in a question; a switch, off by default, "Zapamätať konverzáciu v tomto prehliadači", which keeps the conversation in `localStorage` for at most 90 days after its last change and restores it on the next visit, switching off deletes that copy at once (with the warning not to switch it on on a shared device); that the service keeps conversations for the deployment's `transcriptRetention` days for running the service, review and fixing errors; and, when the deployment names `privacy.contact`, whom to write for access, correction or erasure, with `privacy.documentsUrl` when set. Nothing else is stored in the browser (T-4174, T-4670). It shows the deployment's
greeting and colour, a switch per connector, each answer formatted from a safe part of Markdown with
its markers as superscript links to its sources, listed once per address by title (AG-117), a script and
its output when there was one, and every `error` event as the sentence it carries. It is keyboard
operable and announces answers to a screen reader.

- The widget page MUST answer only for a deployment of the `public`, `ckan` or
  `iframe` channel, with `Content-Security-Policy: frame-ancestors` naming exactly the deployment's
  `allowedOrigins` and allowing scripts, styles and requests from its own host alone, and frames
  from `https://viz.{domain}` alone when the deployment has one (ADR-N-053 §2.9); it MUST set
  no cookie. The chat route MUST accept the widget's own origin (`JC_ASSISTANT_PUBLIC_ORIGIN`)
  beside the deployment's, and every limit stays the server's (AG-100, AG-101, AG-110, AG-114).

#### 1.6.1 Docked beside the catalogue (T-4685)

The catalogue's theme frames the widget with `?embed=1&dock=1`; `dock=1` marks a host that can dock it.

- **The panel.** On a screen at least 1100 px wide the header offers a dock button beside full screen. The widget posts `{"type": "jc-assistant-size", "size": "dock" | "panel" | "full"}` to its host. Docked, the host pins the frame to the right edge at full height, 34 % of the window by default. The visitor drags its left edge between 360 px and 50 % of the window, or moves it with the arrow keys; the host keeps the width in `localStorage` (`jc-dock-width`) and the open and docked state in `sessionStorage`. The page gets a right margin as wide as the panel, so the panel never covers it, and search, filters, maps and tables stay usable. Narrower screens keep the floating panel and full screen.
- **A link clicked on the page.** Every catalogue link stays a normal page load: the address bar, Back and bookmarks keep working. The theme declares `@view-transition { navigation: auto; }` and names the frame `view-transition-name: jc-assistant`, so the browser holds the panel in place while the page beside it cross-fades in about 150 ms (no animation under `prefers-reduced-motion`). The host opens the panel again before its first paint. The widget writes its draft, caret and scroll to `sessionStorage` (`jc-chat-view:{publicId}`) on `pagehide` and restores them, with the conversation, when it loads. An answer still streaming at the page change continues through `resume` (§1.6.2), neither restarted nor repeated. A browser without cross-document view transitions gets the same restore without the cross-fade.
- **Opening the dataset an answer is about.** When an answer is `done`, the widget checks its citations. If they link exactly one catalogue dataset page (a path `/dataset/{name}`, every such citation the same page, the first citation among them), the input is empty and the visitor's switch "Otvárať dataset automaticky" is on, the widget posts `{"type": "jc-assistant-dataset", "url": "…", "title": "…"}`. The switch lives in the "Súkromie" dialog, is shown on a docking host alone, is on by default and is kept in `localStorage` (`jc-chat-follow:{publicId}`). The host navigates only when the URL's origin is its own and its path is `/dataset/{name}`, when it is not the page already open, no text is selected and no field of the page has focus. It first answers `{"type": "jc-assistant-opened", "url": "…"}`. The widget then shows "Otvoril som dataset {title} · Späť" under the answer, and a screen reader hears "Stránka zmenená na dataset {title}" once. "Späť" posts `{"type": "jc-assistant-back"}` and the host goes back one page. A broad answer that links several datasets, or none, never navigates. A host that frames the widget on another site ignores the message, and the citation stays a link.
- The host trusts these messages only from its own frame and the assistant's origin. The widget sends them to the framing origin alone. They carry a URL and a title, never a token.

#### 1.6.2 Resuming an answer (T-4685)

`GET https://assistant.{domain}/api/v1/d/{publicId}/turns/{turn}/resume?after={n}` with the header `X-JC-Visitor`.

- A visitor's answer runs to its end on the server even when its reader leaves (a link on the host page, a closed tab, Stop), and its turn is stored as usual (§3.1). The service keeps the turn's events in memory until 2 minutes after `done`.
- `after` is the number of events after `conversation` the widget already handled, 0 by default. The answer is `200`, `text/event-stream`, `Cache-Control: no-store`: the events from index `after` on, in the order and shape of §1.3, the ones still to come as they come, ending with `done`. Each event carries its index as the SSE `id`.
- The turn belongs to the address that asked (its salted daily hash, as the limits of §1.2 key it): a resume carries any current visitor token, since the token lives in the page's memory and the next page earns a new one. Another address, another deployment or an unknown id answers `404`. The turn id is a random UUID and never the authorization. A missing or expired token answers `401`, an origin the deployment does not allow `403`, a turn past its 2 minutes `410` (for 10 minutes more, `404` after that), a 21st resume in a minute from one address `429`.
- At most 1,000 turns are held at once; past that, the oldest finished turn is dropped first.

### 1.7 In the Portal

A signed-in person asks through the Portal, `POST /api/v1/projects/{project}/knowledge/deployments/{deployment}/chat`
(API/01 §34), with the body of §1.1 and the events of §1.3. The Portal checks the person's
permission and asks `jc-assistant` on its internal path:

```http
POST {assistant}/internal/v1/projects/{project}/knowledge/deployments/{deployment}/chat
Authorization: Bearer <the Portal's service-account token>
X-JC-Person: <the person's username>
X-JC-Person-Token: <the person's access token, when the request carried one>
```

- The internal path answers the Portal's service account alone, as §3 does, and serves a
  deployment of any channel by its name. An `internal` deployment searches its sources' `public`
  and `internal` passages; a `public`, `ckan` or `iframe` one answers exactly as it answers a
  visitor (§1.4), so its administrators try what the public will get. (AG-115)
- A connector of an `internal` deployment whose Endpoint is not `audience: public` is called
  with the person's token, so the gateway decides as it decides for the person; without one it is
  not offered. A public Endpoint is called without a token on every channel. The person's token
  lives for the one question: it is never stored, logged or sent anywhere but the gateway. (AG-115)
- The deployment's `budget` caps the Portal's questions as the public ones (AG-110); an internal
  deployment without a `budget` answers an `error` event saying an administrator sets one. Its
  `rateLimit`, when it has one, counts per person. (AG-101, AG-115)

### 1.8 Feedback on an answer

`POST https://assistant.{domain}/api/v1/d/{publicId}/feedback` takes a visitor's verdict on one
answer of a current conversation; the widget sends it from a thumbs up or down, a thumbs down
asking one optional "Čo chýbalo?" (T-4174):

```json
{"conversation": "6f1c0e9e-3b1a-4d7e-9b51-2c4f8f2a7c11", "turn": 2, "verdict": "down", "note": "Chýbali otváracie hodiny."}
```

- `turn` is the answer's place in the conversation, 1 for the first; `verdict` is `up` or `down`;
  `note`, only with `down`, is at most 500 characters. A second verdict on the same turn replaces
  the first. `204` when stored.
- The route checks `Origin` as the chat route does (§1.2) and counts against the same client's
  `rateLimit.perClientPerMinute`, in a window of its own; past it `429` with `Retry-After`. A
  conversation that is not a current one of this deployment, started from this origin, or a turn
  past the questions it asked answers `400`. A note that carries a secret's shape (a key, a token,
  a password in a URL) is refused with `400` and never stored.
- A verdict is stored with its deployment, day, turn and note alone: no address, cookie or other
  personal data. The note is data, never an instruction: no model reads it. Verdicts are deleted
  after 90 days. The usage route of §3 counts them per day.

### 1.9 What the assistant knows: the context

`GET https://assistant.{domain}/api/v1/d/{publicId}/context` answers, without a token, what the
deployment reads, and `https://assistant.{domain}/d/{publicId}/context` shows the same as a page
in the deployment's languages (`?lang=`), linked from the widget's privacy dialog
(AG-123, [ADR-N-055 §2.6](../Decisions/adr-n-055-assistant-profiles.md#26-the-context-page)).
The deployment reads its Endpoints by the selector of MF-58:

```yaml excerpt
spec:
  publicId: bb-doprava
  channel: public
  categories: [TRAN]
  sources: []                     # a data-only assistant
  select:
    projects: [banskabystrica]
    themes: [TRAN]
    accessLevel: [public]
  exclude: [hfp-vehicles-raw]
```

```json
{"publicId": "bb-doprava", "title": {"sk": "Doprava v Banskej Bystrici", "en": "Transport in Banská Bystrica"},
 "languages": ["sk", "en"], "categories": [{"code": "TRAN", "label": "Doprava"}],
 "limits": {"perClientPerHour": 60, "perClientPerDay": 200, "messagesPerConversation": 30, "questionChars": 1000},
 "endpoints": [{"slug": "banskabystrica-parking", "title": "Parkoviská", "publisher": "banskabystrica",
   "themes": ["TRAN"], "types": ["OffStreetParking"], "entities": 41, "modified": "2026-10-18T21:40:00Z",
   "catalogue": "https://opendata.{domain}/dataset/banskabystrica-parking"}],
 "sources": [], "omitted": 0}
```

- `title` is the deployment's `theme.title` per language; `categories` carry the labels of
  AG-124 in the asked language. `endpoints` is the resolution of AG-121 sorted by slug, each
  with the theme codes of its catalogue record, `entities` and `modified` as the Endpoint's catalogue record counts them; `sources` lists
  each `visibility: public` source with `kind`, `sites` (its start hosts), `pages`, `documents`
  and `crawled`. `omitted` counts the Endpoints past the limit of AG-122.
- It never carries the system prompt, the model, the budget, a `secretRef`, a source that is not
  public or an Endpoint outside the resolution. An unknown, disabled or doubly declared
  `publicId`, and one of the `internal` channel, answers `404`, as the chat does.
- An `internal` deployment's context is the Portal's
  `GET /api/v1/projects/{project}/knowledge/deployments/{deployment}/context`, for a caller who may
  read `AssistantDeployment` in the project. The Portal's editor previews an unsaved manifest with
  `POST /api/v1/projects/{project}/knowledge/deployments/context?dryRun=All` and the manifest as
  the body, same rule, same document, computed by the same resolver; it changes nothing.

### 1.10 A chat page outside the platform

A city may build its own chat page, in its own design, on its own host (AG-125,
[ADR-N-055 §2.7](../Decisions/adr-n-055-assistant-profiles.md#27-public-clients-outside-the-platform)).
It is a browser client of one `publicId` and nothing else:

1. Its origin, `https://host[:port]`, is in the deployment's `allowedOrigins`.
2. It reads the look and the texts from `GET …/context` and the widget settings of MF-52.
3. It gets a visitor token by `challenge` and `token` (§1.2), keeps it in memory, and sends
   questions to `chat` and verdicts to `feedback` with it.

It holds no model key, no ServiceAccount, no secret and no copy of the data; every limit, the
budget and the Policy stay the server's. A change of the look, the languages or the reach is a
Change to the manifest, never a new build of the page.

## 2. Calling the model through `jc-agent-proxy`

The assistant holds no model key. It calls `POST {proxy}/v1/llm/chat/completions` with its own token and names the deployment the call is for.

```http
POST {proxy}/v1/llm/chat/completions
Authorization: Bearer <jc-assistant service-account token, audience agent-proxy>
X-JC-Assistant-Deployment: banskabystrica/bb-public
X-JC-Assistant-Tokens-Per-Day: 2000000
```

- The proxy introspects the token: it must be active, issued to the `jc-assistant` client's service account and name the proxy's client in its audience. Anything else answers `401` with the proxy's one refusal sentence. (AG-109)
- The proxy counts every call's tokens against `{project}/{deployment}` for the UTC day and refuses the call once the day's spend has reached `X-JC-Assistant-Tokens-Per-Day` (`429`, `daily-budget`). The header carries the deployment's `budget.tokensPerDay`; a missing, zero or unparsable value is refused with `400`, so no deployment runs without a cap. (AG-110)
- Only `chat/completions` and `messages` are forwarded, with the installation's model key, as for a run. The proxy keeps no text of the call; its audit line names the deployment instead of a run. (AG-111)

## 3. Administration for the Portal

`jc-assistant` answers the Portal's administration routes (API/01 §34) on its internal paths,
`/internal/v1/projects/{project}/…` with the same shapes, on the cluster network only: the edge
routes `/api/v1/d/*` alone. Each call carries the Portal's service-account token, which the
service introspects with its own client: active, issued to the Portal's client, its audience
naming `jc-assistant`. Anything else is `401`.

| Field of a page | Meaning |
|---|---|
| `id`, `url`, `depth`, `parentId` | its place in the tree |
| `status` | `pending`, `fetched`, `failed`, `skipped` |
| `included` | whether its passages are indexed; `excludedBy` says `pattern` or `administrator` when not |
| `language`, `fetchedAt`, `children`, `documents`, `passages` | what it holds |
| `title`, `description`, `published`, `modified` | what the page states of itself (AG-120); `null` when it does not |

Files of a `source: upload` go to `POST …/knowledge/sources/{source}/files?name={file name}` with the file itself as the request body, one file per request, at most `upload.maxBytes`; are listed by `GET …/files`; and are removed by `DELETE …/files/{id}`. The Portal fronts the same paths under `/api/v1` with `propose` on the `KnowledgeSource`. A file's type is checked by its content, never by its name or `Content-Type`: a PDF, a DOCX, an XLSX, or UTF-8 text, read as CSV or Markdown when the name ends in `.csv` or `.md`. A DOCX or XLSX whose archive unpacks past 200 MiB or 5,000 entries is refused. The name is 1 to 200 characters with no `\` or control character, and may sit in up to 8 folders separated by `/` (`reports/2026/budget.pdf`; no empty, `.` or `..` part): the folders and the name are the path its passages are searched and cited by (T-6183). Uploading a name again replaces that file. Its passages are data, never instructions (AG-105). (T-4184)

| Answer | When |
|---|---|
| `201` `{"id", "name", "mime", "bytes", "passages", "status"}` | stored and indexed; `GET …/files` lists the same fields as `items` |
| `204` | a `DELETE` removed the file and its passages |
| `400` | the name is missing or not a file name |
| `404` | no such project, source or file |
| `409` | the source is not `source: upload`, or it already holds `upload.maxFiles` files |
| `413` | the body is past `upload.maxBytes`; it is not read further |
| `415` | the content is none of the types above |

A source's row of `GET …/knowledge/sources` carries its index status, every number a count over
the service's own tables (T-4197):

| Field of a source | Meaning |
|---|---|
| `pages`, `pagesIncluded`, `documents` | what the crawls stored; a `source: upload`'s files are its documents |
| `passages`, `embedded`, `pending` | its passages, those with an embedding, and those still waiting for one |
| `failed` | pages and documents whose last read failed |
| `languages` | its passages per language, `{"sk": 23140, "en": 2012}`; passages of no known language are not counted (UI-99) |
| `lastCrawl` | the end of the last crawl that stored anything |
| `job` | the last crawl job: `state` (`queued`, `running`, `done`, `failed`), `attempts`, `error`, `startedAt`, `finishedAt` |

`GET …/knowledge/sources/{source}/errors` answers `{"items": [{"url", "kind", "reason"}]}`: at most
50 pages (`kind: page`) and documents (`kind: document`) whose last read failed, newest first.
`reason` is `HTTP {status}` or the network error, at most 300 characters; no content of a page is
in it. A source not crawled yet answers `404`.

`DELETE …/knowledge/deployments/{deployment}/answers` forgets the answers the deployment keeps
(Architecture/23 §4) and answers `200` `{"forgotten": n}`, `n` the answers forgotten, 0 when it
kept none; a project or deployment it does not know forgets nothing and answers the same (T-4731).

`GET …/knowledge/deployments/{deployment}/usage` answers `{"items": [{"day", "requests", "tokensIn",
"tokensOut", "thumbsUp", "thumbsDown", "notes"}]}` for the last 30 days; `notes` counts the thumbs
down that carry a note (§1.8).

A page carries `id`, `url`, `depth`, `parentId`, `status`, `included`, `excludedBy`, `language`,
`fetchedAt`, `children`, `documents`, `passages`, and what it states of itself: `title`,
`description`, `published` and `modified` (AG-120), each `null` where the page states none.
A document carries `id`, `url`, `pageId`, `mime`, `bytes`, `pages`, `offDomain`, `status`,
`included`, `excludedBy`, `passages`, and from its document information `title`, `description`
(its `Subject`), `published` and `modified` (AG-120). A passage carries `ordinal`, `text`, `lang`
and `url`.

`GET …/knowledge/sources/{source}/pages?q=` with a `q` of 2 to 100 characters answers, instead of
one level of the tree, at most 100 pages of the whole source whose title or address holds `q`
(case and diacritics ignored), the same fields. `GET …/knowledge/sources/{source}/search?q=&limit=`
answers the hits §4 answers for that one source, with its visibility whatever it is: the
administration sees what an internal deployment would read (UI-99, T-4659).

### 3.1 Conversations (T-4670)

Every turn of a deployment's conversations is stored at the end of the turn, a turn cut by the
answer timeout too, with what it reached (AG-99). The Portal reads them for its administrators:

```text
GET    …/knowledge/conversations?deployment=&from=&to=&status=&feedback=&slow=&minCostUsd=&q=&sort=&cursor=&limit=
GET    …/knowledge/conversations/usage?deployment=&from=&to=&route=
GET    …/knowledge/conversations/export?deployment=&from=&to=&route=
GET    …/knowledge/conversations/{conversation}
DELETE …/knowledge/conversations/{conversation}
```

| Field of a list row | Meaning |
|---|---|
| `id`, `deployment`, `origin`, `startedAt`, `lastAt` | the conversation; `origin` is the site it came from, `null` without one |
| `firstQuestion`, `turns`, `routes` | the first question (redacted), the number of turns, the routes they took |
| `totalMs`, `errors`, `timeouts` | the longest turn's time and the turns that ended in an error or the timeout |
| `thumbsUp`, `thumbsDown` | the visitors' verdicts (§1.8) |
| `costUsd`, `uncosted` | the sum of the priced model calls, and how many calls the provider sent no price for; with `uncosted > 0` the sum is a floor and the Portal says the price is unknown, never $0 |
| `tokensIn`, `tokensOut`, `cachedTokens` | the turns' prompt and completion tokens, and the prompt tokens the provider read from its cache (`null` when no call reported any, T-4728) |
| `firstTokenMs`, `models` | the first turn's time to its first token (`null` when none came), and the models its calls used |
| `lastError` | what the newest failed or timed-out turn told the visitor (its error's `detail`, else its `title`), `null` when every turn answered (T-4994) |

- `status` is `error`, `timeout` or `ok`; `feedback` is `up`, `down` or `none`; `slow=true` keeps
  conversations with a turn over 15 seconds; `minCostUsd` keeps those whose sum is at least that;
  `q` (2 to 100 characters) matches the questions and answers, case and diacritics ignored;
  `sort` is `lastAt` (default, newest first), `costUsd`, `tokens` (in plus out) or `totalMs`
  (the longest turn), each largest first. `limit` is 1 to 100, default 50, and
  `nextCursor` continues.
- The detail answers `{"id", "deployment", "origin", "client", "turns": [...]}`. A turn carries
  `turn`, `at`, `question`, `answer`, `blocks` (the tables, chart and map specs it showed),
  `language`, `route`, `history` (as the client sent it), `firstTokenMs`, `totalMs`, `error`
  (`{status, title, detail}`) or `timedOut: true`, `verdict`, `costUsd`, `uncosted` and `steps`.
  A step carries `kind` (`route`, `search`, `tool`, `model`, `answer`), `name`, `endpoint`,
  `input`, `output` (at most 8 kB; `outputBytes` is the whole size), `status`, `startMs`,
  `durationMs`, `error`, and for `model` the `stage` (`router`, `judge`, `data`, `answer`),
  `model`, `provider`, `effort`, `promptVersion`, `tokensIn`, `tokensOut`, `cachedTokens`,
  `reasoningTokens`, `reasoning` (the text the provider returned, else `null`), `finishReason` and
  `costUsd` (`null` when the provider sent none). `kind` is also `visual` (a table, chart or map
  the visitor was shown: `name` is `table`, `chart` or `map`, `input` its spec, `rows` its row
  count) and `untracked` (the time between steps no step accounts for), so the steps' durations
  add up to the turn's time from the question's arrival, the connectors' setup included. `output` is whole up to 64 kB, beyond that its first 20 rows, with
  `outputBytes` the whole size (T-4681). A turn's `costUsd` is the sum of its steps', a
  conversation's the sum of its turns'.
- `GET …/knowledge/conversations/usage` sums every turn in the filter (`deployment`, `from`
  inclusive, `to` exclusive, both RFC 3339, `route`; another parameter is `400`) in one aggregate
  (T-4728): `questions`, `timeouts`,
  `errors` (an error that is not the timeout), `uncostedQuestions` (turns with a call the provider
  sent no price for: they never count as $0), and over the other turns `costUsd` with `total`,
  `mean`, `p50` and `p90`. `tokens` holds the means per question of `in`, `out` and `cached`, and
  `cachedShare` (cached of all prompt tokens, `null` with none). `answerCache` holds `hits` (the
  turns answered from the kept answers, route `cached`, T-4731), `hitShare` (hits of all
  questions, `null` with none), and two estimates, `null` when the hits or the other turns are
  none: `savedMs`, hits × (the other turns' mean `totalMs` − the hits' mean), and `savedUsd`,
  hits × the other priced turns' mean cost. `firstTokenMs`, `totalMs` and
  `phases` (`route`: the router call; `work`: search, tools and the data, judge and agent calls;
  `answer`: the answer call; each the sum of its steps' durations in a turn) hold `p50`, `p90` and
  `max`, `null` with no turn. `costByStage` splits the cost of every model call of those turns by
  its `stage` (a timed-out turn's calls included): `stage`, `calls`, `costUsd` (`null` when no
  call of it was priced) and `uncostedCalls`; a stage the assistant starts recording (follow-ups,
  embeddings) appears there by itself. `days` lists up to the 14 newest UTC days of the filter with
  `day`, `questions`, `costUsd`, `uncostedQuestions`, `p50TotalMs`, `timeouts` and `errors`. A
  value nothing was measured for is `null`, never 0.
- `GET …/knowledge/conversations/export` answers the same turns as `text/csv` (UTF-8, CRLF), one
  row per turn, newest first: `at`, `deployment`, `conversation`, `turn`, `route`, `question`,
  `firstTokenMs`, `totalMs`, `tokensIn`, `tokensOut`, `cachedTokens`, `models`, `costUsd` (empty
  when a call was unpriced), `uncosted`, `timedOut`, `error`. The question is the stored,
  redacted text; a cell holding a quote, comma or line break is quoted, and one starting with
  `=`, `+`, `-`, `@`, a tab or a CR gets a leading `'` so a spreadsheet never runs it. More than 20,000 turns
  is `400`: narrow the range.
- `GET …/knowledge/conversations/{conversation}/turns/{turn}/same` lists the stored turns of the
  project whose question reads the same (case, diacritics and spacing ignored, matched by a hash),
  newest first, at most 50: `conversation`, `turn`, `at`, `deployment`, `totalMs`, `costUsd`,
  `steps` (their number), so the Portal compares two answers to one question side by side.
- Before a turn is stored, an access token or API key reads `[token]`, a password `[heslo]`, a
  private key `[kľúč]` and a card number `[karta]`; under `transcriptRedaction: personal` an
  e-mail address also reads `[e-mail]`, a phone number `[telefón]`, an IBAN `[IBAN]` and a birth
  number `[rodné číslo]`.
  Inputs and outputs are untrusted text: the Portal shows them escaped and never sends them to a
  model.
- `DELETE` removes the conversation's turns and steps (erasure, T-4668) and answers `204`; an
  unknown or another project's conversation is `404`.

## 4. Search for the MCP servers

`POST /internal/v1/search` answers the Context Gateway's `search_documents` (EP-133, AG-119,
T-4659). It is on the cluster network only: a request that carries `X-Forwarded-For` or
`Forwarded` came through a proxy and is answered `404`, and a NetworkPolicy admits the gateway
alone. It takes no token: it answers only what is public.

```json
{"query": "úradné hodiny klientskeho centra", "sources": [{"project": "banskabystrica", "source": "web"}],
 "publisher": "banskabystrica", "lang": "sk", "limit": 5, "pathPrefix": ["/sluzby/"]}
```

- `query` 1 to 300 characters; `sources` 1 to 20; `limit` 1 to 50 (default 5); `publisher`,
  `lang` and `pathPrefix` (up to five paths, each starting with `/`, at most 200 characters, no
  control character) optional. Anything else in the body is `400`.
- `pathPrefix` keeps the passages whose path starts with one of them, compared as text (`%` and
  `_` are plain characters), inside the search's SQL before ranking (T-6183). The path is a
  website page's URL path without query or fragment (`/sluzby/kupalisko`), and an uploaded
  file's folders and name (`/reports/2026/budget.pdf`).
- It reads only the `visibility: public` passages of the named sources, ranks them by the chat's
  hybrid search (full text, and the local `multilingual-e5-small` embedding of the query, fused by
  reciprocal rank) and answers the best `limit`, with no model call. `publisher` keeps the
  sources of that project, the body that publishes them; `lang` keeps the passages in that
  language and those whose language is unknown.
- A source that does not exist, or is not public, is skipped as if it held nothing: the answer
  never says which sources exist.

```json
{"hits": [{"title": "Klientske centrum", "url": "https://www.banskabystrica.sk/…", "publisher": "banskabystrica",
  "description": "…", "published": null, "modified": "2026-09-30T08:12:00Z", "crawled": "2026-10-19T03:12:40Z", "language": "sk",
  "text": "…", "score": 0.0325, "chunkId": 4211, "ordinal": 3, "path": "/sluzby/kupalisko",
  "breadcrumb": ["Služby mesta"], "page": null}]}
```

`text` is at most 1,200 characters. The metadata is the page's or document's (AG-120), `null`
where the source states none; `modified` falls back to the sitemap's `<lastmod>`, then the
server's `Last-Modified`, and `crawled` is when the crawl last read it (T-4749); a hit's `title` is the page's own, else the first heading of its
first passage. `breadcrumb` holds the titles of the pages the crawl reached the page from
(`parent_id`, up to 8, within its site), the start page first; a page without a title stands by
its path; a document's is empty. `page` is the PDF page a passage starts on, else `null`.

`POST /internal/v1/context` reads the passages around one hit for `read_knowledge_context`
(EP-167, T-6183), under the same network rule: `{"sources": [{"project", "source"}],
"chunkId": 4211, "before": 2, "after": 2}`, `before` and `after` 0 to 10. It answers
`{"publisher": "hronov", "passages": [{"chunkId", "ordinal", "url", "path", "page", "language",
"text", "hit"}]}`, the passages of the hit's page or document from `ordinal - before` to
`ordinal + after`, in order, `hit` true for the one asked. The hit and every neighbour must each
be a `visibility: public` passage of the named sources; one that is not is left out, and a
`chunkId` that is not answers `404`.

`POST /internal/v1/sources` with `{"sources": [{"project", "source"}]}` (1 to 20) answers what the
named sources hold in public, for the tool's description, under the same rules:
`{"passages": 23140, "publishers": ["banskabystrica", "bbsk"], "languages": {"en": 0.08, "sk": 0.92}}`,
the share of each language among the passages whose language is known.

## 5. The assistant's MCP server

The gateway serves every enabled deployment as an MCP server at
`https://api.{domain}/api/mcp/d/{publicId}` (EP-136, EP-137,
[ADR-N-055 §2.5](../Decisions/adr-n-055-assistant-profiles.md#25-an-mcp-server-per-assistant)),
so a person adds the reach of an assistant to Claude, ChatGPT or Gemini and asks with their own
model:

| Offered | What it is |
|---|---|
| the read tools of the hub (`list_endpoints`, `get_endpoint`, `query_entities`, `get_entity`, `query_temporal`, …) | narrowed to the Endpoints the resolver returns for the deployment (AG-121) |
| `search_documents`, `read_knowledge_context` | over the deployment's own `visibility: public` knowledge sources, when it has one (EP-133, EP-167) |
| resource `context` | the document of §1.9 |

- A `public`, `ckan` or `iframe` deployment answers without a token and with the anonymous tokens
  of ADR-N-052, with its `audience: public` Endpoints alone; an `internal` one answers a Keycloak
  token of a member of the organization, each call read with that token.
- The server runs no model, so it never touches the deployment's `budget`; each call counts
  against the deployment's `rateLimit` per caller beside the gateway's buckets.
- An unknown `publicId`, one two deployments declare, and one with `enabled: false` answer as an
  unknown server does.

## Related

- [Architecture/22](../Architecture/22-knowledge-assistant.md): the service, its manifests and its channels.
- [ADR-N-040](../Decisions/adr-n-040-knowledge-assistant.md): the decision, §3.4 the proxy, §3.7 the threats.
- [04-agent-runs.md](04-agent-runs.md): `jc-agent-proxy` for agent runs.
- [02-endpoint-representations.md](02-endpoint-representations.md): the MCP surface of an Endpoint the connectors call.
- [ADR-N-055](../Decisions/adr-n-055-assistant-profiles.md): assistant profiles, §1.9 and §5 here.
