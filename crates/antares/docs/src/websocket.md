# WebSocket notifications

A `ws` build (`cargo build -p antares-broker --features ws`) delivers
notifications over a WebSocket the client opened, so a client gets push
notifications without exposing an HTTP endpoint of its own:

```text
GET /x/ws/ngsi-ld/v1             the NGSI-LD root, NGSILD-Tenant as on HTTP
GET /x/views/{view}/ws           one loaded view (needs the views feature too)
```

Mount them with `ANTARES_API_SURFACES=admin,ws` and, for views,
`admin,views,ws,ws-views`. The design is ADR-0026; the messages are
described in `docs/api/asyncapi-ws.yaml` (AsyncAPI 3.0.0). The channel is
an Antares extension: no ETSI test purpose covers it, and a vanilla build
does not link it.

## 1. Messages

Every message is one text frame holding the TS 104 243 envelope: a
`metadata` object and, for most messages, a `body`. `metadata.x-op` names
the message; a client may add `x-id`, which the answer echoes.

| From | `x-op` | Body | What it does |
|---|---|---|---|
| server | `x-welcome` | none | first message: `x-connection` and `x-resume-token` |
| client | `x-subscribe` | a Subscription | Create Subscription (10.5.2) with this connection as its endpoint |
| client | `x-update` | a fragment | Update Subscription (10.5.3) of `x-subscription` |
| client | `x-unsubscribe` | none | Delete Subscription of `x-subscription` |
| client | `x-resume` | none | take over a connection, see section 3 |
| server | `x-result` | the operation's answer | `x-status` is the HTTP status the operation answered |
| server | `x-notification` | the notification | `x-subscription`, `x-seq` from 1, and the same metadata the MQTT binding sends |
| server | `x-gap` | none | `x-from`..`x-to` were lost (overflow, past the resume ring, or dropped by the broker before the queue) |
| server | `x-closing` | none | `x-reason` (`overflow`, `lifetime`, `idle`, `view-removed`, `rate`), then the close frame |

A Subscription made over the socket notifies only that socket: its
endpoint is minted as `urn:antares:ws:<connection>` and a body naming
another endpoint is `400`. An HTTP Create naming a `urn:antares:ws:`
endpoint is refused. Each Subscription is an ordinary one, readable and
deletable over HTTP by the same tenant. Notifications pass the policy
engine's `pre_notify` under the subscriber, `throttling` and `cooldown`
apply as on HTTP, and the body is the HTTP sink's for the same change
(only its own `id`, `notifiedAt` and `subscriptionId` differ).

## 2. Delivery and the queue

Each connection has one queue of `ANTARES_WS_QUEUE` notifications, which
with the resume ring holds at most `ANTARES_WS_QUEUE_BYTES` (16 MiB). A
single-entity notification without `showChanges` replaces the queued one
for the same Subscription and Entity, and then says so: its metadata
carries `x-coalesced`, the number of changes it replaced. A consumer that
keeps up has nothing queued and receives every change, one notification
each; with `showChanges` every change is its own notification. A consumer that falls past the queue gets `x-gap`
and is closed with 1013; a peer that does not take a write within the
pong window is closed too. Nothing is buffered without bound. A notification
the broker drops before it reaches the queue (a Subscription's full delivery
lane, `antares_notification_lane_dropped_total`) takes its `x-seq` and arrives
as an `x-gap` in its place, in order, and the connection stays open: the
consumer re-reads with Query Entities. Losses in a row share one `x-gap`.

In a split fleet (`ANTARES_BUS=nats`) the notifier pod reaches the api pod
that holds the socket over core NATS request/reply on
`antares.ws.<connection>`.

## 3. Resume and lifetime

A connection with Subscriptions outlives its socket for
`ANTARES_WS_RESUME_GRACE_SECS`. A new socket's first message may be
`x-resume` with `x-connection`, `x-resume-token` and `x-seq`, the last
notification processed: the answer is `x-result` 200, then the
notifications after `x-seq` from the ring (with `x-gap` for what the ring
no longer holds), then what was queued meanwhile. Another subject, another
tenant or view, a wrong token or a resume that is not the first message
is `404`. After the grace the connection's Subscriptions are deleted.

Each Subscription made over a socket is leased: its `expiresAt` is at most
`ANTARES_WS_SUBSCRIPTION_LEASE_SECS` ahead (an earlier one you ask for is
kept), and the broker renews it while the socket is attached. A broker that
dies without its grace leaves Subscriptions that expire within the lease and
are deleted by the expiry sweep.

A socket ends at `ANTARES_WS_MAX_LIFETIME_SECS` with `x-closing`
`lifetime`; the client resumes on a new one, so a revoked credential stops
at the next reconnect. A connection without Subscriptions that sends
nothing for 60 s is closed (`idle`), and one that misses a Pong for 60 s
is dropped.

## 4. Bounds

| Bound | Answer |
|---|---|
| `Origin` outside `ANTARES_WS_ALLOWED_ORIGINS` | upgrade `403` |
| past `ANTARES_WS_MAX_CONNECTIONS` or `..._PER_TENANT` | upgrade `503` |
| a client message past 64 KiB, a frame past 16 KiB, or nesting past 64 | the connection closes (1009 for nesting) |
| past `ANTARES_WS_MAX_SUBSCRIPTIONS` on one connection | `x-subscribe` answers `400` |
| more than 100 frames at once, or past 20 a second after that (any frame, Pings and Pongs included) | `x-closing` `rate`, then the connection closes with 1008 |
| a notification past 1 MiB | not sent; the delivery fails as on HTTP |
| a binary frame | the connection closes with 1003 |

Every limit is reported in `/q/health` under the surface's member. An
invalid `ANTARES_WS_*` value stops the broker at startup.

## 5. Logging

Open and close are logged at `info` with the connection id, tenant, view
and close reason, never the subject.

## Related

- [Subscriptions and notifications](subscriptions.md)
- [Endpoint views](views.md)
- [Configuration](configuration.md)
