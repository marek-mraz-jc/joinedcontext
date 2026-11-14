---
sidebar_position: 8
title: Live Updates over WebSocket
description: How to receive an Endpoint's changes as they happen over one WebSocket, with the message format, the limits and examples for websocat, JavaScript and Python.
---

# Live Updates over WebSocket

Every Endpoint that serves the `websocket` standard answers a WebSocket at `{endpointUrl}/ws`. You open one connection, subscribe to the entities you care about, and each change the Endpoint's Policy lets you read arrives as a message on that connection. You need no server of your own to receive notifications, and the connection runs through the same gateway, token and Policy as every REST call (EP-106).

The broker side of this channel is Antares ADR-0026. This page describes what a client sees through an Endpoint. The examples use the transport Endpoint on `dev.joinedcontext.com` (`ajaezssvx3bwqgejgepmq4ig3frgan4e`, public, entity type `Vehicle`); replace the host, the slug and the type for any other Endpoint.

## 1. Address and authentication

| | |
|---|---|
| URL | `wss://api.{domain}/api/endpoint/{endpointSlug}/ws`, on the same host as the Endpoint's REST URL (`api.dev.joinedcontext.com` on dev) |
| Subprotocol | `ngsi-ld-ws`: optional for a client that sends no token or sends it in the header, required for the browser form below |
| Public Endpoint | open without a token; you receive what the anonymous grant reads |
| Any other Endpoint | a bearer access token for the Endpoint's audience, as for REST |
| Header form | `Authorization: Bearer <token>` on the upgrade request |
| Browser form | `Sec-WebSocket-Protocol: ngsi-ld-ws, bearer.<token>`; the gateway answers `Sec-WebSocket-Protocol: ngsi-ld-ws` |

The address is the Endpoint's own URL with `/ws` appended. The Portal's host (`portal.{domain}`) is not the address of an Endpoint.

A browser cannot set an `Authorization` header on a WebSocket, so the token travels as the second subprotocol entry. The gateway removes the `bearer.` entry before it forwards the upgrade, never echoes it, and never logs it.

A token in the query string is refused with `400`. A browser page opens the socket only from an origin the installation allows; any other `Origin` answers `403` (a client without an `Origin` header, such as websocat or Python, is not affected). Without a valid token, an Endpoint that is not public answers the upgrade `401` with the same `WWW-Authenticate` header as its REST surface.

A plain `GET` of the address without an upgrade (a browser tab, `curl`) answers `426 Upgrade Required` with `Upgrade: websocket` (over HTTP/1.1; HTTP/2 forbids the field, RFC 9113 8.2.2) and `Link: <https://{host}/docs/API/07-websocket>; rel="help"` (this page, for a signed-in reader, API/00 §1): an HTML page for a browser (`Accept: text/html`) and a problem+json (`upgrade-required`) otherwise, naming the address, the subprotocol, whether a token is needed, one `x-subscribe` message for a type the Endpoint serves and a websocat line. The Endpoint's DCAT record lists the WebSocket distribution at the same address, with a one-sentence `dct:description` in the Endpoint's languages, `foaf:page` and `dct:conformsTo` both naming this page.

An AI agent finds the socket through the Endpoint's MCP surface (T-5045). Where the Endpoint serves the WebSocket, `initialize.instructions` names the address, and `describe_access` (format `permissions`) and the `access://{endpointSlug}` resource carry `liveUpdates`:

| Member | Content |
|---|---|
| `address` | the `wss://` address above, on the gateway's public host |
| `protocol` | `ngsi-ld-ws` |
| `token` | whether a token is needed and where it goes; never a token |
| `subscribe` | one `x-subscribe` message for a type the Endpoint serves |
| `welcome` | the `x-welcome` first message and its resume members (§4) |
| `limits` | `connectionsPerCaller`, `connectionsPerSpace`, `maxMessageBytes`, `maxFrameBytes`, and `lifetime` with `tokenSeconds` and, on a public Endpoint, `anonymousSeconds`: the values the gateway enforces (§3) |
| `httpSubscription` | the alternative: notifications to your own HTTPS URL through `create_subscription` or `POST {endpointUrl}/ngsi-ld/v1/subscriptions`, where your grants list `createSubscription` |
| `guide` | this page |

An Endpoint that does not serve the WebSocket has no `liveUpdates`, and its `/ws` answers `404`.

The connection's identity is fixed when it opens. A connection opened with a token ends when the token expires (its `exp`), and after 300 seconds at the latest: reconnect with a fresh token and resume (§4). An anonymous connection to a public Endpoint has no token to expire and lasts up to 24 hours.

A connection never outlives a change to what admitted it. When the Endpoint is changed in any way (its audience, roles, Policies, hidden attributes or projection, a Policy granted or withdrawn) or the App client the token was issued to loses the Endpoint, every open connection of the Endpoint gets `x-closing` with `x-reason: endpoint-changed` and closes with code `1000` as soon as the gateway loads the change; nothing queued behind the close is sent. A deleted Endpoint, or one that stops serving the WebSocket, closes its connections the same way with `x-reason: view-removed`. The reason names no detail of the change. Reconnect: the new connection is admitted, or refused, under the Endpoint as it is now; a resume (§4) of the closed connection restores no Subscription, so subscribe again. A change that widens access never reaches an open connection.

## 2. Messages

Every message, in both directions, is one text frame holding one JSON object with two members, `metadata` and `body`. This is the envelope of ETSI TS 104 243 §5.2. `body` is plain NGSI-LD, and `metadata.x-op` names what the message is.

| `x-op` | Sent by | `body` |
|---|---|---|
| `x-welcome` | server, first message | none; `metadata` carries `x-connection` and `x-resume-token` |
| `x-subscribe` | you | an NGSI-LD Subscription (TS 104 175 §5.2.12) without `notification.endpoint`: the server sets it |
| `x-update` | you | the members of the Subscription to change; `metadata.x-subscription` names it |
| `x-unsubscribe` | you | none; `metadata.x-subscription` names it |
| `x-resume` | you, first message after a reconnect | none; see §4 |
| `x-result` | server | the created or updated Subscription, or a ProblemDetails; `metadata.x-status` is the HTTP status the same request would get over REST, `x-id` echoes yours |
| `x-notification` | server | the NGSI-LD Notification (TS 104 175 §5.2.6.9.1), the same body an HTTP subscriber would receive; `metadata.x-seq` counts from 1 per connection |
| `x-gap` | server | none; notifications `x-from` to `x-to` were lost, so read the entities again over REST |
| `x-closing` | server, before it closes | none; `x-reason` is `lifetime`, `overflow`, `view-removed`, `endpoint-changed`, `shutdown`, `idle` or `rate` |

A Subscription made over the socket lives on that connection only. The gateway serves your sockets of an Endpoint from one broker subscription made as you, which the Endpoint's Policy decides as a read, and matches your filters itself ([ADR-N-049](../Decisions/adr-n-049-websocket-fan-out-per-endpoint.md)), so `GET {endpointUrl}/ngsi-ld/v1/subscriptions` does not list it. It ends 30 seconds after the connection closes unless you resume. Keep its id from `x-result`; it is the `x-subscription` of every `x-update` and `x-unsubscribe`.

A notification carries only the attributes your grant reads. A change to an attribute you cannot read sends nothing. No message tells you that an entity left what you may read, because that message would itself reveal a hidden change. On `x-gap`, and on your own schedule, read the subscribed types again over REST: `GET {endpointUrl}/ngsi-ld/v1/entities?type=Vehicle` for the examples below.

## 3. Limits

| Limit | Value | When exceeded |
|---|---|---|
| Open connections per client address, at the edge | 50 | the upgrade answers `429` |
| Connections per caller (token subject, or client address when anonymous) | 10 | the upgrade answers `429` |
| Connections per Endpoint's space | 1 000 (an installation may set fewer) | the upgrade answers `503` |
| Endpoint fan-outs (Endpoint and caller identity, each one broker connection) per gateway replica | 64, at most 32 per space (an installation may set others) | a fan-out without a client gives way; when every one has a client, the upgrade answers `503` [`fan-out-full`](09-errors.md#fan-out-full) |
| Subscriptions per connection | 64 (an installation may set fewer) | `x-result` with `x-status: 400` |
| Message you send | 64 KiB (16 KiB per frame) | the connection closes with code `1009` |
| Frames you send (messages, Pings and Pongs) | 100 at once, then 20 a second | `x-closing: rate`, then the connection closes with code `1008` |
| Message the server sends | one entity per notification, up to 1 MiB | the gateway does not read the change, and every socket of the Endpoint gets `x-gap` |
| Idle time (no message and no Subscription) | 60 s | `x-closing: idle` |
| Unread notifications queued | 256 (an installation may set fewer) | `x-gap`, then `x-closing: overflow` |
| A frame the server sends that your socket does not take | 10 s | the connection is dropped as `overflow`; reconnect and resume |
| Compression | `permessage-deflate` and other extensions are not negotiated | a compressed frame (RSV1 set) closes the connection with code `1002` |
| Connection lifetime | the token's `exp`, at most 300 s, with a token; 24 h anonymous | `x-closing: lifetime`; reconnect and resume |

The 64 KiB bound applies only to what you send. A notification carries one entity and may be up to 1 MiB.

When the gateway ends a connection with a close frame, it then reads and discards what you still send until your own close frame arrives, for at most 5 s, before it closes the TCP connection (RFC 6455 7.1.1). A client that floods and then reads still receives `x-closing` and the close code; a dropped connection would reach it as a reset without them.

The `ws-fanout` rig (EP-115) measures one gateway replica on a 4-vCPU GitHub runner, its clients in the same process, every notification checked against the ground truth. At 20 changes a second, the rate of dev's transit stream: 1 000 sockets p50 7 ms and p99 15 ms from change to socket; 5 000 sockets p50 25 ms and p99 237 ms, with 2.4 GiB resident. At 200 changes a second, 1 000 sockets reach p50 3.4 s and p99 6.5 s: the stream outruns one replica, and coalescing keeps each socket's latest state.

The `ws-abuse` rig (T-4902, platform `crates/context-gateway/tests/ws_abuse_rig/main.rs`, job `abuse` of `ws-fanout-rig.yml`) holds these limits under abuse against one gateway replica, its clients in the same process. A control client's p99 from change to socket is measured alone, then beside each abuser, and must stay within twice that baseline plus 20 ms; the process stays under 512 MiB resident and no task panics. First run (2026-10-21, debug build on the shared 12-core host, 200 changes a second): baseline p99 45 ms; 10 000 upgrades from one address keep 10 sockets and answer 9 990 with `429`, 109 MiB resident; a subscriber that stops reading at 1 000 changes a second is dropped after the 10 s send bound and its address opens 10 sockets again; a 1 MiB message closes with `1009`; the `permessage-deflate` offer is not taken and a compressed frame closes with `1002`; a subscribe flood is closed with `x-closing: rate` and `1008` after 105 frames; 2 000 upgrades and closes 50 at a time from one address answer 644 upgrades and 1 356 `429`, then the address holds exactly 10 again. The control p99 stayed between 21 and 47 ms in every scenario. The conformance suite runs the 1009, 1002 and 1008 cases against a deployed Endpoint (`tests/ws/test_gateway_ws.py`).

The server pings every 30 seconds and closes a connection that sent no pong for 60 seconds. Client libraries answer pings by themselves.

When the context broker refuses that subscription (your Policy reads none of the Endpoint's types, or the broker is down), the gateway serves you no notification: every open connection closes with code `1011`, and every `x-subscribe` or `x-update` answers `x-result` with `x-status: 503`, whose ProblemDetails `detail` names the broker's answer. Try again later; the gateway retries every 30 seconds ([ADR-N-049](../Decisions/adr-n-049-websocket-fan-out-per-endpoint.md) §2 item 10).

## 4. Resume after a reconnect

Keep `x-connection` and `x-resume-token` from the first `x-welcome`, and the last `x-seq` you processed. Open a new connection (with a fresh token, if you use one) and send this first message:

```json
{"metadata": {"x-op": "x-resume", "x-id": "r1", "x-connection": "<from x-welcome>", "x-resume-token": "<from x-welcome>", "x-seq": 41}}
```

Within 30 seconds of the close, under the same identity and on the same gateway replica, you get `x-result` with `x-status: 200`, then every notification after `x-seq` 41, then the live ones. The resumed connection keeps its first `x-connection` and `x-resume-token`: use those for the next resume, not the ones in the new `x-welcome`. A notification that is no longer held arrives as `x-gap` first. A resume that fails for any reason, a reconnect that reached another replica among them, answers `x-result` with `x-status: 404`: subscribe again and read the entities over REST.

## 5. Examples

Each example below runs as it stands against the public transport Endpoint and prints `Vehicle` changes. For an Endpoint that is not public, add the token as each example shows.

With [websocat](https://github.com/vi/websocat), type the subscription as the first line after the connection opens:

```bash
websocat --protocol ngsi-ld-ws wss://api.dev.joinedcontext.com/api/endpoint/ajaezssvx3bwqgejgepmq4ig3frgan4e/ws
{"metadata":{"x-op":"x-subscribe","x-id":"s1"},"body":{"type":"Subscription","entities":[{"type":"Vehicle"}],"notification":{"attributes":["speed","location"]}}}
```

With a token, add `-H "Authorization: Bearer $TOKEN"` before the address. The answers arrive on standard output:

```text
{"metadata":{"x-op":"x-welcome","x-connection":"k3v…","x-resume-token":"…"}}
{"metadata":{"x-op":"x-result","x-id":"s1","x-status":201},"body":{"id":"urn:ngsi-ld:Subscription:…","type":"Subscription"}}
{"metadata":{"x-op":"x-notification","x-seq":1,"x-subscription":"urn:ngsi-ld:Subscription:…","Content-Type":"application/json"},"body":{"id":"urn:ngsi-ld:Notification:…","type":"Notification","subscriptionId":"urn:ngsi-ld:Subscription:…","notifiedAt":"2026-10-12T23:04:39Z","data":[{"id":"urn:ngsi-ld:Vehicle:hel.fi:helsinki:12-2406","type":"Vehicle","speed":{"type":"Property","value":13.48,"unitCode":"MTS"}}]}}
```

In a browser, from a page the installation allows:

```js
const url = "wss://api.dev.joinedcontext.com/api/endpoint/ajaezssvx3bwqgejgepmq4ig3frgan4e/ws";
// With a token: new WebSocket(url, ["ngsi-ld-ws", `bearer.${token}`])
const ws = new WebSocket(url, ["ngsi-ld-ws"]);
ws.onopen = () => ws.send(JSON.stringify({
  metadata: { "x-op": "x-subscribe", "x-id": "s1" },
  body: { type: "Subscription", entities: [{ type: "Vehicle" }], notification: { attributes: ["speed", "location"] } },
}));
ws.onmessage = (event) => {
  const { metadata, body } = JSON.parse(event.data);
  if (metadata["x-op"] === "x-notification") console.log(body.data);
  if (metadata["x-op"] === "x-gap") console.log("missed changes: read /ngsi-ld/v1/entities?type=Vehicle again");
};
```

In Python, with the `websockets` package (version 14 or later), reconnecting and resuming after every close:

```python
import asyncio, json, os, websockets

URL = "wss://api.dev.joinedcontext.com/api/endpoint/ajaezssvx3bwqgejgepmq4ig3frgan4e/ws"
SUBSCRIBE = {"metadata": {"x-op": "x-subscribe", "x-id": "s1"},
             "body": {"type": "Subscription", "entities": [{"type": "Vehicle"}],
                      "notification": {"attributes": ["speed", "location"]}}}

async def main():
    resume = None  # x-connection, x-resume-token and x-seq of the connection to resume
    while True:
        # With a token: export TOKEN; a fresh one for every reconnect.
        headers = {"Authorization": f"Bearer {os.environ['TOKEN']}"} if "TOKEN" in os.environ else {}
        try:
            async with websockets.connect(URL, additional_headers=headers) as ws:
                welcome = json.loads(await ws.recv())["metadata"]
                if resume:
                    await ws.send(json.dumps({"metadata": {"x-op": "x-resume", "x-id": "r1", **resume}}))
                else:
                    await ws.send(json.dumps(SUBSCRIBE))
                    resume = {"x-connection": welcome["x-connection"],
                              "x-resume-token": welcome["x-resume-token"], "x-seq": 0}
                async for frame in ws:
                    message = json.loads(frame)
                    metadata, body = message["metadata"], message.get("body")
                    op = metadata["x-op"]
                    if op == "x-notification":
                        resume["x-seq"] = metadata["x-seq"]
                        for entity in body["data"]:
                            print(entity["id"], entity.get("speed", {}).get("value"))
                    elif op == "x-gap":
                        print("missed changes: read /ngsi-ld/v1/entities?type=Vehicle again")
                    elif op == "x-result" and metadata.get("x-id") == "r1" and metadata["x-status"] != 200:
                        resume = None  # the connection is gone: subscribe again
                        break
                    elif op == "x-closing":
                        print("server closes:", metadata.get("x-reason"))
        except (OSError, websockets.WebSocketException) as error:
            print("reconnecting after", error)
        await asyncio.sleep(1)

asyncio.run(main())
```

## 6. Requirements

- **EP-106** [A][S] — the WebSocket surface of an Endpoint ([Requirements/endpoints](../Requirements/endpoints.md#23-live-access-and-the-open-data-portal)).
- **EP-107** [S] — the token never in a URL; connections capped per caller.
- **EP-114** [A][S] — one broker subscription per Endpoint and replica; your filters matched in the gateway.

## Related

- [API/02-endpoint-representations](02-endpoint-representations.md) — every standard an Endpoint serves, and how the open data portal lists them.
- [Requirements/endpoints](../Requirements/endpoints.md) — EP-104, EP-106 to EP-110, EP-114, EP-115.
- [Architecture/04-context-spaces-and-endpoints](../Architecture/04-context-spaces-and-endpoints.md) — the Endpoint model and its standards table.
- [Architecture/21-open-data-catalogue](../Architecture/21-open-data-catalogue.md) — the catalogue the Endpoint is published in.
