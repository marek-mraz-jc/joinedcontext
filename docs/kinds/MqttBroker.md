# MqttBroker

<!-- Generated from the jc-core schema by crates/portal/tests/kind_reference_tests.rs; do not edit. -->

- apiVersion: `joinedcontext.com/v1alpha1`, kind: `MqttBroker`
- Lives in a project (namespace: the project's slug)
- API: `/api/v1/projects/{project}/mqttbrokers`
- Repository path: `projects/{project}/mqttbrokers/{name}.yaml`

Every field below is checked with `deny_unknown_fields`: a field this page does not name is refused at propose, approve and load (`jcctl validate --as-portal`).

## spec (MqttBrokerSpec)

One MQTT edge broker (MF-54).

| Field | Type | Required | Description |
|---|---|---|---|
| `address` | [MqttAddress](#mqttaddress) | no | How the broker is reached from outside; absent is `dedicatedPort` when `port` is set, else `platform` (ADR-N-048 §2.11). |
| `allowedSources` | list of string | no | The networks devices may connect from, in CIDR notation, at most 32; empty admits any address (PF-129). The edge and the gateway both hold a device to it. |
| `capture` | [MqttCapture](#mqttcapture) | no | How much a `capture` broker keeps; read only in that mode. |
| `deviceAuth` | list of [MqttDeviceAuth](#mqttdeviceauth) | no | How devices may authenticate, a non-empty subset of `token` and `clientCertificate`; absent is `[token]` (ADR-N-048 §2.12). |
| `hostname` | string | no | One of the allowance's `customHostnames`, e.g. `mqtt.city.example`; required with `address: customHostname` and read only there. |
| `limits` | [MqttLimits](#mqttlimits) | no | Per-client and per-broker limits, each within the platform ceiling. |
| `mode` | [MqttMode](#mqttmode) | no | What the broker does with a message; absent is `ingest` (ADR-N-048 §2.10). |
| `port` | integer (uint16) | no | A dedicated port from 18830–18849 instead of TLS by SNI on 8883; required with `address: dedicatedPort` and read only there. |
| `roles` | list of [MqttRole](#mqttrole) | yes | Who may connect and to which topics; at least one role. |

## MqttAddress

How a broker is addressed from outside (ADR-N-048 §2.11).

Type: one of `"platform"`, `"dedicatedPort"`, `"customHostname"`

## MqttCapture

`spec.capture` of a `capture` broker.

| Field | Type | Required | Description |
|---|---|---|---|
| `messagesPerTopic` | integer (uint32) | no | Messages kept per topic, newest first. |
| `window` | string | no | How long a message is kept from its arrival, an ISO 8601 duration `PT…`. |

## MqttDeviceAuth

How a device proves who it is when it connects (ADR-N-048 §2.12).

Type: one of `"token"`, `"clientCertificate"`

## MqttLimits

The broker's limits (MF-54).

| Field | Type | Required | Description |
|---|---|---|---|
| `connectionsPerAddress` | integer (uint32) | no | Connections one source address may hold at once at the edge, in `1..=maxConnections`; absent means [`CONNECTIONS_PER_ADDRESS`] or `maxConnections` when lower (T-5888). |
| `maxConnections` | integer (uint32) | no | Connections the broker holds at once. |
| `maxMessageBytes` | integer (uint32) | no | The largest message, in bytes. |
| `messagesPerSecond` | integer (uint32) | no | Messages one client may publish per second. |

## MqttMode

What a broker does with a message (ADR-N-048 §2.10).

Type: one of `"ingest"`, `"capture"`, `"relay"`

## MqttRole

A role: the topic filters it grants. Who holds it is a ServiceAccount RoleBinding `mqtt/<broker>/<role>` (ADR-N-048 §2.13, [`crate::mqtt::roles_held`]).

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | A DNS label, unique among the broker's roles. |
| `publish` | list of string | no | Topic filters the role may publish to; `${clientid}` is the connecting client. |
| `reason` | string | no | Why the role's devices share their topics; required with `sharedTopics`. |
| `serviceAccounts` | list of string | no | Deprecated (T-5872): ServiceAccounts of the broker's own project, by name. Still read for one release beside the bindings; `jcctl migrate` moves them to `mqtt/<broker>/<role>` bindings on each account. |
| `sharedTopics` | boolean | no | The role's devices share their `publish` topics, so each may publish as any other; without it every `publish` filter holds `${clientid}` before any wildcard (T-5561). |
| `subscribe` | list of string | no | Topic filters the role may subscribe to, each under the client's own `${clientid}`. |
