# How far can I get by transit

Click anywhere in Helsinki and see the area you reach in 10, 20 and 30 minutes: walking to a stop,
waiting, riding, changing on foot and walking on.

- one sentence that answers on arrival, from Rautatientori: the area each band reaches and how
  many stops are within 30 minutes;
- a map with the area in three colours, the stops and the starting point; a click anywhere starts
  from there;
- the stops reached, by minutes, each a button that starts from it, and a list to start from any
  stop with the keyboard; an HSL stop also opens in the SDK's entity panel, read only, with a link
  to the Portal (SDK-40, AP-140);
- the starting point and the hours of vehicle history (1, 3 or 6) in the address, so a view is a
  link (`?at=24.95000,60.20000&hours=6&lang=en`).

Finnish, English or Swedish (`?lang=fi|en|sv`, else the browser's language), chosen in the
shell's language menu. Light and dark follow the reader's system. Times are Helsinki's.

## What the answer rests on

HSL's own registers when the space holds them (T-3356): 8389 stops and platforms with their
names and sign codes, and 1121 line variants with their stops in order. They carry no timetable,
so a ride between two stops of a line takes the distance (1.2 times the straight line) at the
mode's usual speed, stops included: metro 40 km/h, commuter train 50, bus 20, tram 15, ferry 18.

Where the space holds no stops and lines, or refuses them, the App derives them from the vehicles'
last hours instead:

- a **stop** is where readings of at least two vehicles stood still (≤ 0.5 m/s, DBSCAN 35 m,
  three readings), so a traffic light two buses waited at counts too;
- a **ride** is a vehicle's move from one such place to the next on one route, timed by its own
  readings (the median over runs, at most 20 minutes a hop).

Either way the **trip** is: walking at 4.5 km/h along streets 1.3 times the straight line, a
5-minute wait at each boarding, changes on foot up to 300 m. Staying aboard costs no second wait.
The page says which of the two it used, under the answer.

## How it is built

A `wasm` App (AP-142, ADR-N-044): React on the joinedcontext App SDK for the page, inside the SDK's
`AppShell`, and the analysis in Rust, compiled to WebAssembly and run in a Web Worker in the
visitor's browser, so a click anywhere answers at once. The static host serves its files, the
endpoint answers the vehicles' history, and the App's server keeps what is worth keeping (below).

- `wasm/` is the crate: `network.rs` (stops and rides from HSL's registers), `stops.rs` (stops
  and rides from the readings), `reach.rs` (Dijkstra over
  "on foot at a stop" and "aboard a route at a stop", then the hexagons each walk covers),
  `dbscan.rs` and `geo.rs` (from alerts-heatmap), `lib.rs` (`analyse`: JSON in, JSON out).
- `src/network.ts` reads the stops and lines, every page (some 8400 stops, past the SDK's `all`).
- `src/vehicles.ts` joins each vehicle's positions, speeds and routes by time; `src/history.ts`
  reads them through the temporal endpoint, at most six hours (`temporalQ`).
- HSL's whole network answers in about 100 ms in the module, 0.6 to 1.1 s to the first answer in
  the page; 32 vehicles of 1000 readings each in about 100 ms.

## From a stop, on the server

The App's server is a WebAssembly component on the platform's shared host (`server/`):

- it reads HSL's stops and lines from the App's own Endpoint, as the page does, and keeps them as
  one file under the App's prefix, `networks/{version}.json`, where the version is the SHA-256 of
  the network as read (the same registers always give the same version); a version read more than
  six hours ago is read again when the page next asks, and a changed network drops the older
  version's areas;
- **Download the areas from the start stop (GeoJSON)** asks it for the area reached from the stop
  the view starts from, on the kept network: computed once per version with the page's own crate
  (`../wasm`) and kept in the App's schema (`migrations/`) with its hexagons as GeoJSON,
  `tiles/{version}/{stop}.geojson`, downloaded through a URL valid for two minutes. Every visitor
  gets the same answer for the same stop and network. When the server has not read the network
  yet, the page asks it to once and then asks for the areas again.

| Route | What it does |
|---|---|
| `GET /api/network` | the kept network's version, sizes, and whether it is due to be read again |
| `POST /api/network` | HSL's registers read again; a new version when they changed |
| `GET /api/reach?stop=` | the areas from a stop in each band; a URL their GeoJSON downloads from |

## Origins, roles and history

The server decides from the signed-in person the platform names (`identity::caller()`, API/07),
never from anything the page sends:

- anyone, signed in or not, sees the **published origins** on the **Origins** page;
- a **viewer** keeps the point the view starts from under a name (**Keep this origin**), with
  the area the browser reached from it in each band. The Origins page sets the kept origins side
  by side: the area in 10, 20 and 30 minutes and the stops within 30. A viewer renames and
  deletes their own; someone else's answers as if it did not exist;
- a **steward** sees and changes every origin and **publishes** one into the shared set every
  visitor sees, or withdraws it.

A control the reader's role cannot use stays on screen, disabled, with the reason as its tooltip.
A rename, a deletion and a publication show at once and go back when the server refuses. Every
change writes a row into the App's `app_audit` table in the same statement, so a refused or failed
change leaves none. The **History** page reads that table: a viewer their own rows, a steward
everyone's.

| Route | Who | What it does |
|---|---|---|
| `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /api/origins` | anyone | the published origins, the caller's own, a steward's every one |
| `POST /api/origins` `{name, lon, lat, source, bands}` | signed in | an origin kept, at most 50 a person |
| `PUT /api/origins/{id}` `{name}` | owner, steward | renamed |
| `DELETE /api/origins/{id}` | owner, steward | gone |
| `PUT /api/origins/{id}/published` `{published}` | steward | in or out of the shared set |
| `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

`server/host-test.json` plays every route on the real host as an anonymous visitor, a viewer,
another viewer and a steward, and counts the rows after each refused write.

## Run the tests

```sh
pnpm install
pnpm wasm          # cargo build --target wasm32-unknown-unknown, then wasm-bindgen --target web
(cd wasm && cargo test)
pnpm test          # vitest, with the compiled module run in-process
pnpm build
pnpm e2e           # the built bundle in Chromium: 4 widths, light and dark, fi and en, axe, the panel
(cd server && cargo test && cargo build --release --target wasm32-wasip2)   # the server component
```

`server/host-test.json` is the server component's scenario on the real host: the portal
repository's `tests/wasm-apps` (`tests/scenarios.rs`, ci-full) builds the component, runs
`migrations/` twice as the reconciler does, and plays the scenario against Postgres, RustFS and a
mock of the App's own Endpoint, with a second App that must see nothing.

The coverage gate (T-3373): `sh ../../scripts/app-coverage-run.sh transit-reach --rust` holds the
page at 95 % of lines and 90 % of branches, every control used by a test, and `wasm/` at 95 %
(`cargo llvm-cov`). `wasm/pkg/` is generated and left out of the page's figure.

`pnpm wasm` comes first: the tests, the type check and the build import `wasm/pkg/`. The build lane
runs the same two steps itself (builder/build-wasm.sh) on the app-build-rust runner. It needs the
`wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version the crate pins.
