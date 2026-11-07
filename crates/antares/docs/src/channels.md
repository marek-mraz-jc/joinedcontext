# Inbound Channels

A Channel binds one topic of a message bridge to one Attribute of one
Entity: what is published on the topic becomes that Attribute's value
([ADR-0031](../adr/ADR-0031-inbound-bridge-channels.md)). Build with the
`channels` feature, mount it with `ANTARES_API_SURFACES=admin,channels`, and
name the bridges in `ANTARES_CHANNEL_BROKERS`:

```bash
ANTARES_CHANNEL_BROKERS='hsl=mqtts://mqtt.hsl.fi:8883,lo=loopback'
```

## Binding

```bash
curl -si -X PUT "$BROKER/x/channels/bus-12-speed" \
  -H 'Content-Type: application/json' -H 'NGSILD-Tenant: city' \
  -d '{"entity": "urn:ngsi-ld:Vehicle:bus-12", "attribute": "speed",
       "broker": "hsl", "topic": "city/bus/12/speed", "retention": "mirror"}'
```

| Member | Meaning |
|---|---|
| `entity` | The Entity's id. It must exist. |
| `attribute` | The Property the samples write, a term of the request's `Link` `@context`. |
| `broker` | A bridge name from `ANTARES_CHANNEL_BROKERS`. |
| `topic` | One concrete topic, at most 256 bytes: no `+`, `#` or `$` prefix. |
| `retention` | `mirror` stores each sample as a write (history by the type's rules); `relay` notifies the matching Subscriptions and stores nothing. |
| `maxRate` | Samples per second, 1 to 100, default 10. |
| `maxBytes` | Payload bytes, 1 to 65536, default 4096. |
| `direction` | `in`, the default and the only one bound. |

The bind is decided as a 5.6.3 Append Attributes of that Attribute by the
request's subject: a refusal answers 403 and binds nothing. `201` binds,
`204` rebinds a Channel the same subject bound. Every sample is decided
again as that subject.

A payload is the value: JSON when it parses, else its text. `observedAt` is
the time the broker received it. A payload that is an NGSI-LD Notification
(this broker's own MQTT output on the same topic) is dropped as `looped`.

## Reading and unbinding

`GET /x/channels` lists the caller's Channels in the tenant;
`GET /x/channels/{id}` adds `status.samples`: `written`, `refused`,
`tooLarge`, `overRate`, `looped` since the Channel started in this process.
`DELETE /x/channels/{id}` unbinds. A Channel another subject bound answers
404.

## MQTT

The MQTT bridge speaks MQTT 5 at QoS 1 under the shared subscription
`$share/antares/{topic}`, so with several replicas each message is written
once. The bridge host passes the egress policy (`ANTARES_EGRESS_*`).
