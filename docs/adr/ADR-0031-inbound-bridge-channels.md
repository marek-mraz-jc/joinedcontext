# ADR-0031 — Inbound bridge Channels

Date: 2026-10-23. Status: accepted (owner may override). Decided by
worker-t5079 from the Coraine comparison (platform docs
`analysis/coraine-architecture.md` section 2 item a; Coraine
`doc/bridge-channels.md` at 44da1e7). Task T-5079.

## Context

Antares sends over MQTT (clause 7 notifications, `antares-notifier`
`mqtt.rs`) but receives nothing from a message bus: every inbound value
reaches it through a Bento pipeline and the gateway, one HTTP write per
batch. Coraine binds a bridge endpoint (an MQTT topic) to one Attribute of
one Entity with a Channel, and gives the Channel a retention: `mirror`
stores and publishes, `relay` feeds Subscriptions and keeps nothing.

Three questions decide whether such a binding is safe here:

- who may bind a topic to a tenant's Attribute;
- what stops a value written from a topic from coming back on the same
  topic as a notification and being written again;
- how `relay` sits beside the history rules (T-5056: a type records history
  only by its retention rule under `ANTARES_TEMPORAL_RECORD=ruled`).

## Decision

1. **A Channel is a surface resource, `/x/channels/{id}`** (feature
   `channels`, `ANTARES_API_SURFACES=...,channels`), under the request's
   `NGSILD-Tenant`: `PUT` binds or rebinds, `GET` reads one with its sample
   counters, `GET /x/channels` lists the caller's, `DELETE` unbinds. The
   body names `entity`, `attribute`, `broker`, `topic`, `retention`
   (`mirror` | `relay`), and optionally `maxRate` (samples per second,
   1 to 100, default 10) and `maxBytes` (1 to 65536, default 4096). Unknown
   members are refused. The documents live under the broker's reserved
   tenant `antares-channels`, keyed `{tenant}:{id}`.
2. **The bind is the write it stands for.** Binding asks the policy seam
   about a 5.6.3 Append Attributes of that Attribute on that Entity, as the
   binding subject, through `AppState::call` with a task-local bind intent
   that stops the write after the decision: a refusal is the seam's own 403
   and nothing is bound; an absent Entity is 404. The subject's headers are
   stored with the Channel as a Subscription stores its creator's
   (`__subject`, never served), and only that subject reads or unbinds it.
3. **Every sample is decided again.** A sample is a 5.6.3 Append Attributes
   through `AppState::call` with the binder's tenant, `Link` and subject
   headers: negotiation, the bounds wall, the policy seam and views apply as
   for a client. A grant withdrawn after the bind stops the samples; they
   count as `refused`.
4. **Retention.** `mirror` is that write: current state, history by the
   type's rules (T-5056), notifications. `relay` runs the same request with
   a relay intent: the write is decided and applied to a copy of the stored
   Entity, the change goes to the Subscription matcher, and the store is
   not written, so a relay leaves no Entity row and no history row whatever
   the history rules say. A relay needs the Entity to exist, as an Append
   does. A Channel binds no Attribute a Context Source registration serves.
5. **Inbound only.** `direction` is `in`; `out` and `both` are refused,
   naming the clause 7 MQTT notification Subscription as the outbound half.
6. **Loop protection.** A sample whose payload is an NGSI-LD Notification,
   bare or as the `body` of a clause 7 message, is this broker's own
   output coming back, and is dropped as `looped`. A Subscription
   publishing an Attribute's changes on the Channel's own topic therefore
   costs one dropped sample per change, never a second write. Topics are
   concrete: no `+`, `#` or `$` prefix.
7. **Bridges come from the deployment.** `ANTARES_CHANNEL_BROKERS` names
   them, `name=uri` pairs: `loopback`, or an `mqtt[s]://[user[:pass]@]host
   [:port]` base whose credential stays in that variable (set from a secret)
   and never reaches a tenant's document. A tenant names a bridge, never a
   host, so a Channel cannot make the broker dial anywhere new; the MQTT
   bridge also passes the egress policy (`ANTARES_EGRESS_*`) as a
   notification does.
8. **MQTT bridge.** MQTT 5, QoS 1, under the shared subscription
   `$share/antares/{topic}`: every replica binds every stored Channel, and
   the server hands each message to one of them. It resubscribes after a
   reconnect. A bridge that is down when a Channel is bound refuses the bind
   (500, nothing stored); one down at start is retried every 30 s.
9. **Caps.** Per Channel: `maxBytes` per payload (larger is `tooLarge`), a
   one-second window of `maxRate` samples (more is `overRate`), and a queue
   of 256 samples between bridge and writer (a full queue drops the newest).
10. **The loopback bridge** delivers what `Loopback::publish` sends from a
    thread of its own, as a network bridge's event loop would. It is the
    test double and a way for an embedding host to feed Channels without a
    network.

A payload is the Property's value: JSON when it parses, else its UTF-8 text;
`null` and invalid UTF-8 are refused. The sample's `observedAt` is the time
the broker received it.

## Consequences

- One value per topic reaches the store without Bento or the gateway in
  between; the policy seam still decides each one.
- Measured on the memory store, release build, 4 worker threads: 2,000
  Channels at one sample per second each (the Helsinki fleet at peak,
  T-5046) wrote 10,000 of 10,000 samples in 5.09 s (`mirror`) and 5.04 s
  (`relay`), keeping up with the feed; the relay left the Entities' history
  at their creation row, the mirror added one row per sample.
- Counters live in the process: a restart starts them at 0.
- Not decided here: payload mappings (a JSON path into the message, units);
  a Channel today takes the whole payload as the value.

## Related

- ADR-0020 (policy seam), ADR-0026 (WebSocket notification channel),
  ADR-0027 (surfaces).
- `docs/src/channels.md`.
