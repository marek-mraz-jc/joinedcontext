# hsl-transport

The public reference app (AP-34, AP-38). HSL's vehicles on a map, moving: the buses of four
trunk lines and every tram, metro, train and ferry, at most 90 at a time (the HFP pipeline's caps
per mode, T-3893), with a filter per mode, the departures of a chosen stop and HSL's service
alerts beside the map, the stops they name marked on it (T-3895).

It reads one Context Space through one Endpoint and nothing else: no broker, no database, no
second host, no credential. The Endpoint it is bound to is public, so the app never sees a
user and asks nobody to sign in (AP-28). Its sibling `air-quality` covers the other half of
the login front, with a Keycloak session and a write.

The page sits in the SDK's shell (SDK-39). A bus clicked on the map, or chosen from the list of
those in view, opens in the shell's entity panel (SDK-40), read from this app's own backend
(`ui/src/api.ts` `panelSource`), never from the endpoint. The app writes nothing, so the panel
links the bus to the Portal, at the `portal` the reconciler puts into `JC_APP_CONFIG`.

## How it stays live (T-3581)

One WebSocket for the whole process, to the Endpoint's `{JC_ENDPOINT_URL}ws` (API/07), opened
anonymously like every other read of this public Endpoint. It subscribes to `Vehicle` with
`location` watched and the attributes the map draws, keeps the last known position of each
vehicle in memory, and pushes what moved to every open browser as Server-Sent Events on
`{JC_BASE_PATH}api/stream`. A hundred open maps are still one socket, and the browser never
learns the Endpoint URL.

The fleet is read once over REST (`ngsi-ld/v1/entities` as GeoJSON) after each new
subscription, after an `x-gap` and when a resume fails, because only then can the stream have
missed a move. The server closes the socket every five minutes (`x-closing: lifetime`); the
next socket resumes from the last `x-seq` and nothing in between is lost. While no socket is up,
the stream's `feed` event carries `{"lostSince": time}` and the map says "live feed lost since
11:05" instead of looking calm; it is `null` again once the feed is back.

`{JC_BASE_PATH}metrics` counts notifications, gaps and reconnects, and gives the p50 and p95 of
the update latency over the last ten minutes: from the position's `observedAt` (the HFP event)
to its notification arriving here.

## Departures and alerts (T-3895)

Every 30 s the app reads, in the simplified representation: the GtfsStops that carry
`departures` (`q=departures`, the curated set of T-3891), the headsign of each line and direction
from PublicTransportRoute (read again hourly), HSL's Alerts (the space's Digitraffic road alerts are left
out by their `source`) and the GtfsStops those alerts name, for their place on the map. The
browser reads them from `{JC_BASE_PATH}api/departures` and `{JC_BASE_PATH}api/alerts` (503 before
the first read), every 30 s; a failed read keeps what was shown. Alert text is HSL's and the page
renders it as text, never as HTML.

## Environment

| Variable | Meaning |
|---|---|
| `JC_ENDPOINT_URL` | the Endpoint this app reads, the only host it calls. Required |
| `JC_BASE_PATH` | the path it is served under, `/apps/hsl-transport/`. Defaults to `/` |
| `JC_BIND_ADDRESS` | defaults to `127.0.0.1:8080` on a laptop; the reconciler sets `0.0.0.0:8080` in the pod, where the edge is the only caller |

## Running it

```sh
cd ui && pnpm install && pnpm build && cd ..
JC_ENDPOINT_URL=https://<host>/api/endpoint/<slug>/ cargo run
```

## Tests

```sh
cargo test -p hsl-transport      # the contract with the Endpoint and its socket, against stand-ins
cd ui && pnpm test               # the map source, against a MapLibre double
cd ui && pnpm build && pnpm e2e  # the built page at four widths, light and dark
```

`pnpm e2e` serves the built bundle under `/apps/hsl-transport/` with the snapshot and one stream
frame answered from `ui/e2e/serve.ts`, and holds the map and its lines to 375, 768, 1440 and
2560 px: no sideways scroll, no overlap, no control cut off, axe clean (T-2825). It is also what
found MapLibre's worker missing from the bundle, which `ui/map-worker.ts` now ships.

The Rust tests assert what actually leaves the pod: the representation asked for, the fleet
cap, that no credential is ever attached, that serving a browser costs no query, and that the
socket subscribes, resumes from its last `x-seq` without a second read and reads again on a gap. The
UI tests assert that a bus moving on the Endpoint reaches the one GeoJSON source the map
draws from, without the layer being rebuilt. MapLibre needs WebGL, which jsdom has not, so
the library is replaced by a double; there is no Playwright flow here for the same reason.
