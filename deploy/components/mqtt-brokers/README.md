# mqtt-brokers

The MQTT edge brokers of the projects (`docs/Decisions/adr-n-048-mqtt-edge-brokers.md`,
MF-54, PL-74, PL-76, PL-77): RMQTT 0.24.0 as published, one Deployment per `MqttBroker`,
named `mqtt-{project}-{name}`. That name is also the broker's ServiceAccount, so its mesh
identity, and its Keycloak audience.

A broker keeps nothing. Each CONNECT goes to the gateway's `POST /internal/mqtt/auth` on
port 8082 (rmqtt-auth-http, `deny_if_error`); the answer carries the client's subscriptions and
the end of its session, the token's `exp` or its credential's `expiresAt`, whichever comes
first. Each publish goes to `POST /internal/mqtt/acl`, which decides it against the
repository as it is now and refuses a client above the broker's `messagesPerSecond` (a token
bucket per client); the gateway sends no `X-Cache`, so the broker asks for every publish and
disconnects a client whose publish is refused: a device whose role or account is removed, or
that floods the broker, stops at its next publish (T-5560, T-5562). `mqueue_rate_limit` caps
delivery to each subscriber at the same rate. TLS ends at Traefik on 8883 with the BSI
TR-02102-2 TLSOption `mqtt-bsi-tr-02102` of the brokers' namespace, the HTTP edge's settings.
On a dedicated port TLS ends in RMQTT (rustls): TLS 1.2 and 1.3, ECDHE with AEAD suites only,
the TLSOption's four TLS 1.2 suites plus the two ECDHE ChaCha20-Poly1305 ones, which rustls
offers with no setting to drop them (T-5856). The broker's own last rule, `["deny", "all"]`, refuses whatever the gateway did not
allow. No retainer, no session or message storage, sessions end with the connection.

| Port | Listener | Who reaches it |
|---|---|---|
| 1883 | `internal`, plain MQTT | the pipeline runner only (NetworkPolicy and a Linkerd Server) |
| 1884 | `external`, PROXY protocol v2, only on a broker without `spec.port` | the ingress controller only |
| 8883 | Traefik entrypoint `mqtts`, TLS | devices of a broker without `spec.port`, `HostSNI(<name>.<project>.mqtt.<domain>)`, one HTTP-01 certificate per host |
| 8884 | `tls.external`, TLS in RMQTT after a PROXY v2 header, only on a broker with `spec.port` | the ingress controller only (NetworkPolicy and a Linkerd Server) |
| 18830-18839 | Traefik entrypoint `mqtt-<port>`, TLS passthrough | devices of the one broker whose `spec.port` it is, `HostSNI(*)`, so a device without SNI connects and still gets the broker host's certificate from RMQTT; PROXY v2 to 8884 (T-5856) |
| 6060 | RMQTT HTTP API, under the mesh only | the Portal's identity, `GET /api/v1/metrics/history/sum` and `GET /api/v1/clients` only (Linkerd `Server` + `HTTPRoute`, ADR-N-048 §2.8, T-4043) |

A broker has one outside door (T-5871): 8883 by hostname, or its dedicated port when `spec.port`
is set, never both. A pool port no broker holds keeps its entrypoint with no route, and Traefik
closes the connection.

RMQTT opens TLS, WebSocket and QUIC listeners on its own; the chart turns each of them off, and
binds the cluster gRPC to loopback. Its HTTP API runs only where the instance runs the mesh and
names the Portal; elsewhere the chart turns it off too and the Portal shows "not measured".

`brokers` is empty in every environment: the Portal's reconciler stamps one broker per
`MqttBroker` manifest (T-3994). The chart renders a broker's objects once more with sentinels
(`__RELEASE__`, `__PROJECT__`, `__NAME__`, `__MAX_BYTES__`, `__PER_SECOND__`, `__CONNECTIONS__`,
`__CONFIG_HASH__`) into the ConfigMap `broker-template`, key `objects.yaml`, and once more with a
dedicated port into `objects-port.yaml` (`__PORT__`, and `__TLS_REVISION__`, the broker
Certificate's `status.revision`), which the Portal takes for a broker with a served `spec.port`.
RMQTT reads its certificate only at start, so a renewal changes the pod annotation
`joinedcontext.com/tls-revision` and rolls the broker at the next reconcile. The Portal fills the
sentinels from the manifest,
applies the objects by server-side apply with `app.kubernetes.io/managed-by: joinedcontext-portal`,
and deletes those of a broker whose manifest is gone. The Role `portal-mqtt-brokers` lets it do
that here and nowhere else, and `JC_PORTAL_MQTT_NAMESPACE` tells it where. An entry under
`mqtt-brokers.brokers.brokers` still runs one by hand, from the same template.

## Preconditions on the cluster

The `mqtts` entrypoint is part of the cluster's Traefik, not of this repository: an entrypoint
on 8883, published on the node with `externalTrafficPolicy: Local`, and its container port in
Traefik's `config.linkerd.io/skip-inbound-ports`. Without the last one, Traefik sees every
client as 127.0.0.1, writes that into the PROXY header, and the gateway reads a device as a
client inside the cluster. Each dedicated port is the same: an entrypoint `mqtt-<port>` for
18830-18839, published on the node, and in the same skip list (hetzner-kit
`extra-manifests/40-traefik-linkerd.yaml.tpl`); 18840-18849 stay reserved and are not served.
