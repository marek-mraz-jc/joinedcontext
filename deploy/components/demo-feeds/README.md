# demo-feeds

A demo NATS broker and live feed publisher for the `dev` cluster (T-0477, PL-50).
This is a demonstration broker for development only — it is not a platform component
and must never be deployed to production.

It provides a lightweight NATS server and a generator stream so that a `nats`
`DataSource` on dev has data to read end to end without external broker infrastructure.

## Parts

- **nats**: A stateless NATS broker (`demo-nats`) running in-memory without persistence or JetStream.
- **feed**: A Bento publisher (`demo-feed`) that emits a counter reading to the broker on a configured interval.
- **hsl-bridge**: A Bento bridge (`hsl-bridge`, T-5883) that reads HSL's live tram positions
  (`wss://mqtt.hsl.fi:443`, `/hfp/v2/journey/ongoing/vp/tram/#`) and publishes each frame, its
  topic under `hsl`, to the helsinki MQTT broker `hsl-bridge` over mqtts (8883), signed in as the
  helsinki ServiceAccount `hsl-bridge` with a Keycloak token. The pipeline `mqtt-hsl-bridge`
  writes the trams into the organization-only space `hsl-mqtt-test`. Its seed is in
  `components/context-gateway/seed/helsinki-demo-feeds`.

## Configuration

| Setting | Default | Description |
|---|---|---|
| `demo-feeds.nats.enabled` | `true` | Enable the demo NATS broker. |
| `demo-feeds.nats.namespace` | `<demo-feeds namespace>` | Namespace for the NATS broker deployment. |
| `demo-feeds.feed.enabled` | `true` | Enable the demo feed publisher. |
| `demo-feeds.feed.namespace` | `<demo-feeds namespace>` | Namespace for the feed publisher deployment. |
| `demo-feeds.feed.subject` | `helsinki.demo.counters` | NATS subject to which readings are published. |
| `demo-feeds.feed.interval` | `10s` | Interval between published messages. |
| `demo-feeds.hsl-bridge.enabled` | `true` | Enable the HSL bridge. |
| `demo-feeds.hsl-bridge.clientId` | `helsinki-hsl-bridge` | Keycloak client of the bridge's ServiceAccount. |
| `demo-feeds.hsl-bridge.clientSecret` | `{secret: hsl-bridge-client, key: client-secret}` | Secret holding the client secret. |
| `demo-feeds.hsl-bridge.topic` | `/hfp/v2/journey/ongoing/vp/tram/#` | HSL topic filter the bridge reads. |
| `demo-feeds.hsl-bridge.messagesPerSecond` | `900` | Bridge send rate cap, under the broker's per-client limit (1,000). |

## Starting the HSL bridge

The bridge waits, logging what it needs, until its secret exists:

1. An approver issues a secret for the helsinki ServiceAccount `hsl-bridge` in the Portal
   (Access, Service accounts, Issue secret). The Portal shows it once.
2. Store it in the demo-feeds namespace, from a file so it stays out of the shell history:
   `kubectl -n <demo-feeds namespace> create secret generic hsl-bridge-client --from-file=client-secret=<file>`.
3. Restart the pod (`kubectl -n <namespace> rollout restart deployment/hsl-bridge`).

The broker drops a connection when its token expires, so the bridge takes a new token and
reconnects 30 s before each expiry. A publish outside its role's filter ends its session.

## How a DataSource reads it

A pipeline's `DataSource` manifest reads the published readings using the runner's native `nats` input:

```yaml
spec:
  type: nats
  input:
    urls: [ "nats://demo-nats.dev.svc.cluster.local:4222" ]
    subject: helsinki.demo.counters
```
