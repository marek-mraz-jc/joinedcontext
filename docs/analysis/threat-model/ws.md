# Threat model: WebSocket notifications (`ws`)

Section 7 of the [surfaces threat model](../surfaces-threat-model.md), in a file of its own so
that two surfaces never edit the same file (T-3766). Shared boundaries (B), assets (A), bounds
and the cross-surface rows (G) are in the index; `dev/threat-tests.py` checks every row here.

## 7. WebSocket notifications (`ws`)

Entry points: an HTTP upgrade below `/x` (ADR-0003 names the crate `antares-ws` and the feature `ws`, "plugging into the NotificationSink scheme registry and a Router::merge — core crates never change"); client messages that create and delete Subscriptions whose endpoint is the connection; server messages that carry notifications. No WebSocket binding exists in the specifications (matrix section 2.4; ADR-0003: "a WS Notification Binding is an official NGSI-LD 2.1 work item"), so the message format is an Antares one and makes no conformance claim. Library: `tokio-tungstenite` 0.29 and `tungstenite` 0.29 on the connection hyper upgrades; the opening handshake (RFC 6455 section 4.2) is `antares-ws`'s own, never axum's `ws` feature, which cargo would unify into every build of the workspace's axum, the vanilla broker's included (T-3704; `dev/check-vanilla-symbols.sh --metadata` guards it) (F8).

Trust boundaries: B0 at upgrade time only (the gateway authenticates the upgrade, then the connection outlives the request); B1; B3 (delivery is the broker writing to the socket).

Assets: A1 to A3, A6, A8.

| id | S/T/R/I/D/E | threat | mitigation | test |
|---|---|---|---|---|
| W1 | S | cross-site WebSocket hijacking: a page in a browser opens the socket with the user's cookies | `Origin` checked against the deployment's allowlist at upgrade (the same knob as MC8); absent `Origin` is a non-browser client and passes | `crates/antares-ws/tests/ws/ws_bounds.rs::ws_a_foreign_origin_is_refused` |
| W2 | D | WebSocket connection exhaustion | caps per process and per tenant (proposed `MAX_WS_CONNECTIONS`, `MAX_WS_CONNECTIONS_PER_TENANT`), an idle timeout and a maximum lifetime (proposed `ANTARES_WS_MAX_LIFETIME_SECS`), all published in `/q/health`; over a cap the upgrade answers 503 and is counted | `ws_bounds.rs::ws_connections_are_capped_per_tenant_and_per_process` |
| W3 | D | message and frame size: tungstenite defaults to 64 MiB messages and 16 MiB frames (F8), above the 4 MiB body cap | `max_message_size` set to the body cap or less, `max_frame_size` set below it; every client message passes `json_depth` against `MAX_JSON_DEPTH` before its parse | `ws_bounds.rs::ws_an_oversized_or_too_deep_client_message_closes_the_connection` |
| W4 | D | a slow consumer: tungstenite's write buffer is unbounded by default (`max_write_buffer_size: usize::MAX`, F8), so a reader that never reads grows broker memory | a bounded per-connection queue; on overflow the connection is closed and counted, never buffered; a delivery slot (`DELIVERY_WIDTH_PER_TENANT`) is not held while a socket is full | `ws_queue.rs::ws_a_slow_consumer_is_disconnected_not_buffered` |
| W5 | I | notification content and timing leaks | delivery goes through the sink registry and `deliver_as`, so `pre_notify` runs under the subscriber (ADR-0020) and the view rules 6 and 9 apply | `ws_delivery.rs::ws_notifications_pass_pre_notify_under_the_subscriber`; `policy_notify_5_8_6.rs::the_notification_subject_is_the_subscribers` |
| W6 | S, I | subscription takeover: a client names another connection as its endpoint | the endpoint URI of a WebSocket Subscription is minted by the broker for the connection that created it and is not client-settable; a Subscription is delivered only on that connection | `ws_delivery.rs::ws_a_subscription_is_delivered_only_to_the_connection_that_created_it` |
| W7 | S | the subject captured at upgrade outlives the token the gateway checked | the connection ends at its maximum lifetime (W2), which a deployment sets at or below its token lifetime; the subject is the one stored with each Subscription (`__subject`, ADR-0020) | `ws_resume.rs::ws_a_connection_ends_at_its_max_lifetime` |
| W8 | E | a view is narrowed or removed while a connection still holds Subscriptions made through it | ADR-0023 rule 9: the view is applied at match time; a removed view stops delivery and closes its connections | `ws_views.rs::ws_a_removed_view_stops_delivery` |
| W9 | D | a flood of subscribe messages on one connection | a per-connection Subscription cap (proposed `MAX_WS_SUBSCRIPTIONS_PER_CONNECTION`); each create is an ordinary gated 10.5.2 through `Ops` | `ws_bounds.rs::ws_subscriptions_per_connection_are_capped` |
| W10 | R | connections cannot be traced | open and close logged with view, tenant, connection id and close reason; no subject value in the log (ADR-0020) | `ws_log.rs::ws_open_and_close_are_logged_without_the_subject` |
| W11 | T | a client message parser fed crafted input | bounded serde parse of a closed message enum with `deny_unknown_fields` | fuzz target `fuzz/fuzz_targets/ws_client_message.rs` |
