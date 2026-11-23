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

## Who a Channel acts as

A Channel keeps the identity of who bound it: the person, client or
participant the policy engine names, and a read-only limit. It keeps no
role, group or Organization membership. The bind and each sample are decided
as that identity, on the policies in force then:

- a Policy that grants that person, client or participant allows the write;
- a Policy that grants only a role, a group or the Organization does not,
  because the broker cannot know who still holds one;
- every prohibition by a role, a group or the Organization applies.

A caller the engine names nobody for (roles alone, or anonymous) binds no
Channel and is answered 422. A Channel stored before this rule is reduced to
its identity at start; one without an identity stays stopped until it is
bound again. An engine of your own implements `PolicyEngine::identity`, or
no Channel binds behind it. Disabling a person's account does not stop their
Channels: remove their Policy or unbind the Channel.

## Limits and replicas

A Tenant binds at most `ANTARES_CHANNELS_PER_TENANT` Channels (50), and one
process runs at most `ANTARES_CHANNELS_MAX` (1000). A bind past either
answers 403 and names the setting.

A Channel is stored in its Tenant's own scope. Every replica checks the
Channels it runs against the store every 5 seconds: one unbound, re-bound or
purged through another replica stops within that time, and while the store
cannot be read its samples are refused.

## MQTT

The MQTT bridge speaks MQTT 5 at QoS 1 under the shared subscription
`$share/antares/{topic}`, so with several replicas each message is written
once. The bridge host passes the egress policy (`ANTARES_EGRESS_*`).
