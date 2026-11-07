# ADR-0026 — WebSocket notification channel: one more sink, the TS 104 243 envelope, bounded and resumable

Date: 2026-10-10. Status: accepted, rule 1 amended by ADR-0027 (section 9). Decided by worker-2 under the owner
order of 2026-10-10 ("a design decision is yours: pick the recommended
option"); the owner may override any rule below. Supersedes ADR-0003.
Builds on ADR-0016 (bindings behind the sink registry), ADR-0020 (the
policy seam, `pre_notify`) and ADR-0023 (endpoint views). Task T-3440.

## Context

ADR-0003 kept WebSocket out of v1 for two reasons: structure ("the whole
binding is scoped to a future `antares-ws` crate behind cargo feature
`ws`, plugging into the NotificationSink scheme registry and a
Router::merge — core crates never change") and standards ("a WS
Notification Binding is an official NGSI-LD 2.1 work item (ETSI TC DATA
Issue #8); implementing ahead of it risks divergence").

The structural half is now true: ADR-0016 made `NotificationSink`
(`crates/antares-notifier/src/lib.rs:301`) "the whole binding contract"
and `SinkRegistry` "the only way a binding is chosen". Implementors
today (cg, graph at `4c3610cd5b`):

```cypher
MATCH (t:Symbol {name:'NotificationSink'})<-[:IMPLEMENTS]-(i:Symbol) RETURN i.path, i.file;
// -> antares_notifier::http::HttpSink, antares_notifier::mqtt::MqttSink,
//    antares_plugin_example::sink::MemorySink, two test sinks
```

The standards half is unchanged. `grep -rli websocket /workspace/etsi-specs`
finds one passing mention (`ts-104178-ngsi-ld-information-model/4/4.1.md:45`);
TS 104 175, TS 104 176 and TS 104 243 V1.1.1 define no WebSocket binding
(`docs/analysis/operation-matrix.md` section 2.4). The owner wants the
channel now (`tasks/GOAL-antares-surfaces.md`, point 2), so the
divergence risk is taken on knowingly and kept small: everything that
has a standard form uses it, and everything else is marked `x-`.

The closest standard is the MQTT binding. TS 104 243 5.1: "The
subscriptions are handled using the HTTP binding as specified in ETSI
TS 104 176 [4], but instead of an HTTP endpoint, an MQTT endpoint is
provided." 5.2: "The MQTT notification message shall be provided as a
JSON Object with the two elements "metadata" and "body". The actual
notification, as specified in clause 5.2.6.9.1 of ETSI TS 104 175 [1] is
the value of "body", whereas any additional information is provided as
key-value pairs in "metadata"." Table 5.2-2 puts `Content-Type` and
`Link` into `metadata`, and 5.2 adds "a new entry for each member named
"key"" of `notification.endpoint.receiverInfo`. Antares already renders
that object for MQTT from the transport-neutral `Outbound` (ADR-0016
rule 4).

The threat model fixed the channel's threats W1 to W11
(`docs/analysis/surfaces-threat-model.md` section 7); this ADR answers
each.

## Decision

### 1. One more sink, one more surface, no new delivery path

Crate `antares-ws`, behind the `antares-broker` cargo feature
`surface-ws` (the name the surfaces architecture ADR uses for every
surface; ADR-0003's `ws` is not used), off by default. It contributes
two things through existing seams and nothing else:

- an `ApiSurface` (`crates/antares-api/src/surface.rs`) that serves the
  upgrade below `/x`, and
- a `NotificationSink` registered with `with_sink` (ADR-0016 rule 6).

Matching, `throttling`, `cooldown`, `timesSent`/`lastNotification`, the
5.8.6 delivery policy (ADR-0015), `deliver_as` and the ADR-0020
`pre_notify` call (`crates/antares-api/src/notify.rs:1983`, `:2084`) are
the ones every Subscription goes through. The sink receives an
`Outbound` that has already passed them. No core crate changes.

### 2. Addresses

- `GET /x/ws/ngsi-ld/v1` upgrades a connection for the tenant the upgrade
  request names (`NGSILD-Tenant`, default tenant when absent, 6.3.14
  rules), with the feature alone.
- `GET /x/views/{view}/ws` upgrades a connection bound to one view
  (ADR-0023 rule 10's prefix) when `views` is also compiled in. The view
  pins the tenant; an `NGSILD-Tenant` naming another tenant is 400, as on
  the view's NGSI-LD root.

### 3. The envelope is TS 104 243's, the body is unmodified NGSI-LD

Every message in both directions is one WebSocket text frame holding one
JSON object `{ "metadata": {...}, "body": ... }`.

- Server to client, a notification: `body` is the Notification of
  TS 104 175 5.2.6.9.1, byte for byte what the HTTP binding would POST.
  `metadata` holds `Content-Type` and `Link` exactly as Table 5.2-2, one
  entry per `receiverInfo` key exactly as 5.2, plus the `x-` members of
  rule 4. The sink renders it with the MQTT binding's own `metadata`
  builder, so the two bindings cannot drift apart.
- Client to server: `body` is a Subscription (5.2.12) for `x-subscribe`,
  the Subscription fragment (5.2.12 members) for `x-update`, absent for
  `x-unsubscribe`, `x-resume`.
- Server answers to a client message: `body` is the created or updated
  Subscription, or a ProblemDetails (TS 104 175 5.5.3) for a refusal,
  with `metadata.x-status` holding the HTTP status the same request over
  HTTP would have answered.

A message whose `metadata.x-op` is unknown, or that is not one JSON
object with exactly these two members, is answered with a ProblemDetails
of type `BadRequestData` and does not close the connection; a frame
above the size bound or a binary frame closes it (rule 6).

### 4. Subscriptions stay valid NGSI-LD; the endpoint is minted

A Subscription delivered over a connection is an ordinary Subscription:
created by Create Subscription (10.5.2), readable by Retrieve and Query
Subscription, deletable over HTTP, with
`notification.endpoint.uri = "urn:antares:ws:<connection-id>"`.
`<connection-id>` is 128 random bits, base32, minted by the broker at
upgrade and sent to the client in the first message (`x-op: x-welcome`).

The namespace follows the broker's existing internal endpoint
`urn:antares:distsub:` (`crates/antares-api/src/distsub.rs:232`), not
`urn:ngsi-ld:`, whose namespace ETSI owns. The sink serves the `urn`
scheme and accepts only the `urn:antares:ws:` prefix in
`parse_endpoint`; `urn:antares:distsub:` is handled in `deliver_as`
before the registry is asked (`notify.rs:2013`), so the two never meet.

The client creates, updates and deletes its Subscriptions **over the
socket** (`x-subscribe`, `x-update`, `x-unsubscribe`). The surface turns
each into the ordinary 10.5.2 / 10.5.3 / 10.5.6 request through
`AppState::call` (the operation layer of the module rules), with the
endpoint set to this connection's URI (a body naming any other endpoint
URI is 400), so the request is gated, stored
and validated exactly as over HTTP. The endpoint is not client-settable
(W6): `parse_endpoint` accepts a `urn:antares:ws:` URI only while a
task-local marker set by the surface for that one call names the same
connection, so the same URI in an HTTP Create or Update Subscription,
or another connection's URI over a socket, is 400 `BadRequestData`.

Decided (worker-2, owner may override): subscriptions over the socket
with a minted endpoint, because it closes W6 without a capability token
that `GET /subscriptions` would then hand to every reader of the tenant.
Options: over the socket (chosen); over HTTP naming the connection's URI
as a bearer capability; both.

The delivery rules a view adds (ADR-0023 rule 9: hidden Attributes
removed, hidden-only changes not sent, `watchedAttributes` intersected)
hold because the Subscription is a view Subscription when it is made on
`/x/views/{view}/ws`; nothing in the sink knows views.

**Watching is a read (T-4755).** A Subscription whose endpoint is the
connection making the request is decided by the engine as Query Entities
(5.7.2) with the Subscription's types, Attributes, `q` and `geoQ`, for
create, retrieve, update and delete alike; every other Subscription keeps
its 5.8.x clause. The socket's own Subscription carries nothing a reader
does not already get by polling: `deliver_as` runs `pre_notify` under the
stored subject for every notification and drops the rows and Attributes
the caller may not read (rule 8), so a read grant may watch what it may
query and nothing more. The test is the sink's own (`parse_endpoint`
accepts the URI only inside that connection's call), so an HTTP request,
or another connection, naming the URI keeps the 5.8.x clause and is then
400 as above. An update decided as a read leaves the endpoint a socket:
a fragment naming any other endpoint keeps 5.8.2, and the stored
Subscription is checked again inside the write.

Decided (worker-4, owner may override): a read decides the socket's own
Subscriptions, because a WebSocket Endpoint is how a reader watches, the
gateway relays every socket as its caller (ADR-N-049 amended), and a
5.8.1 grant would also let that caller create HTTP Subscriptions to any
host the egress guard admits. Options: decide as 5.7.2 (chosen); require
5.8.1 of every WebSocket reader; let the gateway's own account subscribe
and filter in the gateway (the duplicate filter T-4755 removes).

### 5. Extensions, all `x-`

| member | where | meaning |
|---|---|---|
| `x-op` | every message | `x-welcome`, `x-subscribe`, `x-update`, `x-unsubscribe`, `x-resume` (client); `x-notification`, `x-result`, `x-gap`, `x-closing` (server) |
| `x-id` | client message, its `x-result` | correlation id chosen by the client, echoed |
| `x-status` | `x-result` | the HTTP status of the same request over HTTP |
| `x-connection` | `x-welcome` | the connection id, so the client can recognise its endpoint URI |
| `x-resume-token` | `x-welcome` | 256 random bits; with `x-connection` it is the only way to resume |
| `x-seq` | `x-notification` | per-connection sequence, `u64`, from 1, no gaps on one connection |
| `x-coalesced` | `x-notification` | present only on a coalesced notification: how many queued, unsent notifications of the same Entity it replaced (rule 6); those changes are not delivered |
| `x-subscription` | `x-notification` | the Subscription id (also in the body's `subscriptionId`; repeated so a client can route without parsing the body) |
| `x-gap` | server message | notifications from `x-from` to `x-to` are lost; re-read with Query Entities |
| `x-closing` | server message | sent before the server closes; `x-reason` is `lifetime`, `overflow`, `view-removed`, `shutdown`, `idle` or `rate` |

Heartbeat is the WebSocket protocol's own Ping/Pong (RFC 6455 5.5.2,
5.5.3), every 30 s; a connection with no Pong for 60 s is closed. No
application heartbeat message exists.

Not adopted:

- **Acknowledgement messages.** Delivery stays what 5.8.6 makes it for
  HTTP (one attempt, with the ADR-0015 retry policy where configured);
  `x-seq` plus the resume ring (rule 7) gives a client gap detection
  without a per-message round trip.
- **A view-exit message** when an Entity stops matching the view. When the
  view's predicate keys on a member outside the allowlist, the exit
  itself tells the client a hidden value changed (the timing leak
  ADR-0023 rule 9 closes). A client that keeps a mirror re-reads on
  `x-gap` and on its own schedule.
- **Durable replay from a change log.** `bus = local` has no durable log
  (ADR-0002: a first-class single-node mode), and replaying the JetStream
  stream through the matcher for one connection would make a reconnect
  cost a re-match of every change since the cursor. The resume ring of
  rule 7 covers a reconnect within its grace period; everything older is
  `x-gap`.

### 6. Backpressure: bounded, never blocking the matcher

- Each connection has one bounded queue (`ANTARES_WS_QUEUE`, default 256
  messages). `NotificationSink::deliver` does a `try_send` into it and
  returns at once; it never awaits the socket. A delivery slot
  (`DELIVERY_WIDTH_PER_TENANT`) is therefore never held while a socket is
  full (W4).
- Coalescing: when a queued, not yet written notification of the same
  Subscription carries exactly one Entity with the same id as the new
  one, and the Subscription has no `showChanges`, the queued one is
  replaced by the new one, which carries `x-coalesced` with the count of
  notifications it replaced (amended 2026-10-21, T-5050: a subscriber of
  per-second changes must see that it missed some). A consumer that keeps
  up finds nothing queued and gets every change; coalescing happens only
  under back-pressure. Nothing else is coalesced: `showChanges`
  makes each `previousValue` meaningful, and a multi-Entity
  notification is not split.
- Overflow: the queue full after coalescing closes the connection with
  close code 1013 after an `x-closing: overflow` and an `x-gap` naming the
  first lost `x-seq`; the delivery answers failure, so the Subscription
  records `lastFailure` as for an unreachable HTTP endpoint. The
  connection is counted in `/q/metrics`.
- `throttling` and `cooldown` (Table 5.2.15-1, 5.2.12) are applied by
  notify before the sink is asked, as for every binding.
- The writer task sets tungstenite's `max_write_buffer_size` to one
  message cap, so the library's own buffer is bounded too (threat-model
  F8).

### 7. Resume within a grace period

A connection keeps the last `ANTARES_WS_QUEUE` written notifications in a
ring. When the socket closes for any reason but `x-unsubscribe` of every
Subscription, the connection's Subscriptions and ring are kept for
`ANTARES_WS_RESUME_GRACE_SECS` (default 30 s). A new upgrade whose first
message is `x-resume` with `x-connection`, `x-resume-token` and `x-seq`
(the last one the client processed) takes the connection over if:

- the token matches (constant-time compare),
- the tenant and, for a view connection, the view are the same, and
- the subject headers of the new upgrade equal the stored ones.

It then receives the ring after `x-seq`, or `x-gap` first when the ring
no longer reaches back that far, and delivery continues with the next
`x-seq`. A failed resume is `x-result` 404 `ResourceNotFound` for every
reason (an existence oracle on connection ids would otherwise appear).
After the grace period the connection's Subscriptions are deleted
through 10.5.6 (Delete Subscription, gated as 5.8.5) like any client delete.

A connection lives in one process, its Subscriptions in the store: a
process that ends without its grace (a crash, an OOM kill) would leave them
behind, matching every change for a socket nobody holds (T-4543). So every
Subscription made over a socket carries a lease: its `expiresAt` is at most
`ANTARES_WS_SUBSCRIPTION_LEASE_SECS` (default 300 s) ahead, an earlier
`expiresAt` the client asks is kept, and the heartbeat of an attached socket
renews it every third of the lease (a resume renews it at once). An expired
one notifies nothing (5.8.1.4), and the expiry sweep deletes a socket
Subscription whose lease has passed. The lease must exceed twice the grace
plus the Ping interval, so a connection within its grace never lapses.

### 8. Identity: bound at upgrade, limited by lifetime

The gateway authenticates the upgrade request, as it authenticates every
request (ADR-0020: the broker parses no credential). The subject headers
of the upgrade request are the connection's subject; every Subscription
the connection creates stores it in `__subject` (ADR-0020), and
`pre_notify` runs under it for every notification (W5).

The broker cannot re-check a token it never parsed, so a connection ends
at `ANTARES_WS_MAX_LIFETIME_SECS` (default 300 s, the Keycloak default
access-token lifespan) with `x-closing: lifetime` (W7). The client
reconnects through the gateway with a fresh token and resumes (rule 7);
the resume requires the same subject. A deployment sets the lifetime at
or below its token lifetime.

### 9. Cross-connection delivery is impossible by construction

The sink resolves `urn:antares:ws:<id>` in a per-process table of live
connections, and only that connection's queue is written. A connection id
appears in a stored Subscription only if that connection created it
(rule 4), and every write to the queue also compares the Subscription's
tenant with the connection's. Under `bus = nats` the notification may be
matched on another pod than the one holding the socket: the sink then
publishes the rendered message as a NATS core request on
`antares.ws.<connection-id>` (the subject is the random id, so no
wildcard of another tenant names it); the holding pod answers `queued`,
`overflow` or nothing, and nothing counts as a failed delivery.

### 10. Bounds and admission

| bound | default | threat |
|---|---|---|
| `Origin` allowlist at upgrade (`ANTARES_WS_ALLOWED_ORIGINS`); absent `Origin` passes (non-browser client) | empty list: every browser `Origin` refused | W1 |
| connections per process / per tenant (`ANTARES_WS_MAX_CONNECTIONS`, `ANTARES_WS_MAX_CONNECTIONS_PER_TENANT`); over a cap the upgrade answers 503 | 10 000 / 1 000 | W2 |
| idle timeout (no client message and no Subscription) | 60 s | W2 |
| maximum message and frame size | 64 KiB message, 16 KiB frame (a Subscription is small; the 4 MiB body cap is for entity writes) | W3 |
| JSON depth of a client message | `MAX_JSON_DEPTH` (64) checked before the parse | W3 |
| Subscriptions per connection (`ANTARES_WS_MAX_SUBSCRIPTIONS`) | 64 | W9 |
| queue per connection (`ANTARES_WS_QUEUE`) | 256 | W4 |

Every limit is reported in `/q/health` beside the existing bounds
(`crates/antares-api/src/bounds.rs`). Open and close are logged with
connection id, tenant, view and close reason, never a subject value
(W10). The client message is a closed `serde` enum over `x-op` with
`deny_unknown_fields` on the envelope (W11).

### 11. The AsyncAPI document

`docs/api/asyncapi-ws.yaml` (AsyncAPI 3.0.0) describes both addresses,
every message of rules 3 to 7, and references the NGSI-LD Subscription,
Notification and ProblemDetails schemas by name rather than copying them.
CI validates it with `@asyncapi/cli validate` (job filed by T-3445); the
message schemas are the ones the crate's own tests load.

## Consequences

- WebSocket is a cargo feature that a vanilla build does not link; ETSI
  results of the vanilla build are unchanged, and no ETSI test purpose
  covers this channel. The conformance claim is "TS 104 243 envelope,
  Antares extensions", stated in the AsyncAPI document.
- A client gets push notifications without exposing an HTTP endpoint,
  through the same gateway as its API calls.
- When ETSI publishes a WebSocket binding (TC DATA Issue #8), it replaces
  the `x-` members; the body and the TS 104 243 `metadata` keys are what
  such a binding is most likely to keep.
- ADR-0016's sink contract says "a release binary registers no sink that
  declares otherwise" (`network() == false`). The WebSocket sink opens no
  outbound socket (it writes to an inbound connection) and declares
  `network() == false`; that sentence is amended to "no sink that opens
  an outbound connection without the egress guard". ADR-0016's own text
  stays; this ADR records the amendment.

## Confirmation

Named tests, filed by T-3445 under `crates/antares-ws/tests/`:

| rule | test |
|---|---|
| 1 | `ws_notifications_pass_pre_notify_under_the_subscriber` (W5), `ws_throttling_and_cooldown_are_honoured` |
| 3 | `ws_notification_body_equals_the_http_body`, `ws_metadata_equals_the_mqtt_metadata` |
| 4 | `ws_a_subscription_is_delivered_only_to_the_connection_that_created_it` (W6), `ws_an_http_create_naming_a_ws_endpoint_is_refused`, `ws_a_subscription_is_readable_over_http` |
| 6 | `ws_a_slow_consumer_is_disconnected_not_buffered` (W4), `ws_coalescing_keeps_the_latest_single_entity_notification`, `ws_show_changes_is_never_coalesced` |
| 7 | `ws_resume_replays_after_the_last_seq`, `ws_resume_past_the_ring_sends_a_gap`, `ws_resume_with_another_subject_is_not_found` |
| 8 | `ws_a_connection_ends_at_its_max_lifetime` (W7) |
| 9 | `ws_cross_pod_delivery_reaches_the_holding_pod` (bus=nats cell), `ws_a_tenant_mismatch_is_never_written` |
| 10 | `ws_a_foreign_origin_is_refused` (W1), `ws_connections_are_capped_per_tenant_and_per_process` (W2), `ws_an_oversized_or_too_deep_client_message_closes_the_connection` (W3), `ws_subscriptions_per_connection_are_capped` (W9), `ws_open_and_close_are_logged_without_the_subject` (W10) |
| views | `ws_a_removed_view_stops_delivery` (W8) |

Plus the fuzz target `fuzz/fuzz_targets/ws_client_message.rs` (W11), the
AsyncAPI validation job, and the vanilla symbol gate (no `antares_ws::`
or `tungstenite::` symbol in the default binary).
