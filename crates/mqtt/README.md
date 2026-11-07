# `mqtt` (`jc-mqtt`) — Multi-Tenant MQTT 3.1.1 & 5.0 Broker Listener

## Scope
`mqtt` provides a single shared multi-tenant MQTT 3.1.1 and MQTT 5.0 listener serving all organizations:
- Authenticates devices on `CONNECT` using service account tokens or mTLS client certificates (`jc-auth` / `identity`).
- Automatically prefixes and isolates every published and subscribed topic under `{OrgId}/{space}/{client_topic}` so no device or subscription can ever cross an organization boundary.
-Enforces per-organization connection counts, message rate limits, payload size caps, and retained message quotas.
- Bridges inbound device telemetry into `ekiben` pipelines (via NATS JetStream or in-process channels) and outbound NGSI-LD notifications back to MQTT subscribers.

## Public API
- MQTT over TLS: `mqtts://...:8883`, MQTT over WebSocket: `wss://.../mqtt`
- gRPC / In-Process: `jc_mqtt_api::MqttBroker`

## Standalone Binary
`jc-mqtt`

## Configuration
- `DATABASE_URL` (role `jc_mqtt`, schema `mqtt`)
- `MQTT_TLS_ADDR` (default `0.0.0.0:8883`), `MQTT_WS_ADDR` (default `127.0.0.1:8086`)
- `NATS_URL` (for forwarding telemetry into `ekiben` streams)

## Standards Implemented
- OASIS MQTT 3.1.1
- OASIS MQTT 5.0 (shared subscriptions, user properties, session expiry, topic aliases, flow control)

## Origin Today
Replaces per-broker **RMQTT** pods.
