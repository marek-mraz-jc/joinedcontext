# City bike rebalancing planner

For the operator of Helsinki's city bikes: which docking stations are about to run empty or full,
and a route for one service van that takes bikes from the full ones to the empty ones. In Finnish,
English or Swedish, three pages: the plan, the planner's levels and the history of changes. The
plan page shows:

- three numbers on opening: the stations about to run empty, those about to be full, and the
  bikes the van moves, over how many kilometres and stops;
- the controls: the van's capacity and where it starts (the fullest station when none is chosen);
- the map, every station in its state's colour and the van's route over them; a click on a station
  takes it into or out of the route and opens it in the entity panel;
- the route, stop by stop: take or leave how many bikes, what is on board after, the distance from
  the last stop;
- the stations most out of balance, each with its button to add it to the route or leave it out;
- the stations by fill, in tenths.

The page sits in the SDK's `AppShell` (SDK-39), with its Suomi/English/Svenska switch and the
reader's name and role (Viewer, Steward or Not signed in) in the header. A station's name in
the route or the table, and a station on the map, open the station in the shell's entity panel
(SDK-40), read fresh through the App's endpoint. A station's choice button says what its click
does: a station about to run empty or full is routed unless left out, so it reads "Leave out" even
before the plan on screen has reached it.

Every choice is kept in the address (`?van=`, `?start=`, `?add=`, `?skip=`, `?lang=`), so a reload
or a shared link shows the same plan. The counts are read again every minute.

## How the plan is made

The planner is Rust, in `wasm/`, compiled to WebAssembly and run in the reader's browser in a Web
Worker, so the page never stalls:

- a station's state from its bikes and places: empty, low (under 15 %), balanced, nearly full
  (over 85 %), full; a station with no count, or out of service, is never routed;
- what it needs: the bikes above or below half its places;
- the route: nearest neighbour from the start under the van's capacity, then 2-opt while the route
  brings more bikes, serves more stations or drives fewer kilometres (haversine distances). At
  most the 60 most urgent stations are planned over.

The page sends the stations as JSON and gets the states and the route back (`plan` in
`wasm/src/lib.rs`).

## Saved plans, drives and route sheets

The page plans in the browser while the operator clicks; what is kept lives on the App's server, a
WebAssembly component on the platform's shared host (`server/`, `kind: wasm`, ADR-N-044):

- **Save this plan** sends the choices on screen (the van's capacity, the start, the stations added
  and left out) with the name of the van or crew it is for. The server reads the stations again
  from the gateway, as the signed-in caller through the App's own Endpoint, plans with the same Rust (`../wasm`, without the
  browser's bindings) and keeps the stops it made in the App's own schema (`migrations/`). What is
  kept is what the city's data said at that moment, never what a browser sent.
- Each saved plan gets a **route sheet**, a CSV of its stops for the driver (what to take or leave,
  what is on board after, the leg's kilometres), stored under the App's own prefix and downloaded
  through a URL valid for two minutes. A cell that would start a spreadsheet formula is quoted.
- **Mark as driven** records that the van drove the plan's stops; a plan lists how often it was
  driven. **Delete** takes the plan, its drives and its sheet away, after a second click.
- The van or crew is kept in the address (`?op=`), so a reload or a shared link shows that van's
  plans. It is a label the operator types, not an account.

## Roles and history

The server decides from the signed-in person the platform names (`identity::caller()`, API/07),
never from anything the page sends:

- a **viewer** saves plans of their own and sees, drives, downloads and deletes only those; a plan
  of someone else's answers as if it did not exist;
- a **steward** does that for every plan, marked "saved by someone else" where it is not theirs,
  hands a plan to another van or crew, and sets the planner's **levels** (below what share a
  station is about to run empty, above what share it is about to be full, and the share the van
  brings it back to). Every plan saved after uses them, in the browser and on the server;
- signed out, the page shows the stations and the levels and offers no save.

A control the reader's role cannot use stays on screen, disabled, with the reason beside it and
as its tooltip. A hand-over and a change of levels show at once and go back when the server
refuses. Every change writes a row into the App's `app_audit` table in the same statement, so a
refused or failed change leaves none; the **History** page reads it: a viewer their own rows, a
steward everyone's.

| Route | Who | What it does |
|---|---|---|
| `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /api/levels` | anyone | the planner's levels |
| `PUT /api/levels` | steward | the levels changed for every plan after |
| `GET /api/plans?operator=` | signed in | the latest 50 plans (a steward: everyone's), of one operator when named |
| `POST /api/plans` | signed in | a plan made from the gateway's counts and kept, with its route sheet |
| `GET /api/plans/{id}` | owner, steward | the plan, its stops and its drives |
| `DELETE /api/plans/{id}` | owner, steward | the plan, its drives and its sheet gone |
| `PUT /api/plans/{id}/operator` | steward | the plan handed to another van or crew |
| `POST /api/plans/{id}/drives` | owner, steward | a drive recorded; its stops must be the plan's |
| `GET /api/plans/{id}/sheet` | owner, steward | a URL the route sheet downloads from |
| `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

`server/host-test.json` plays every route on the real host as an anonymous visitor, a viewer,
another viewer and a steward, and counts the rows after each refused write.

## Data

One data need on the Context Space `helsinki`: `BikeHireDockingStation` with `name`, `location`,
`availableBikeNumber`, `freeSlotNumber`, `totalSlotNumber`, `status` and `dateModified`, as the
bikes pipeline writes them from HSL's feed. The app reads through its own endpoint, in the
browser and on its server alike, and writes nothing to the Context Space: its plans and drives are
in its own schema.

**What an operator edits here: nothing.** The `dataNeeds` keep `queryEntity` and
`retrieveEntity`. Every station attribute is the feed's: the pipeline rewrites the counts each
minute, so a value changed by hand would be gone at the next run. The panel never offers Edit; it
links the station to the Portal.

## Run the tests

Needs Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version
`wasm/Cargo.lock` pins and the build lane binds with.

```sh
pnpm install
pnpm wasm          # cargo test of wasm/, then the module into wasm/pkg (the lane runs builder/build-wasm.sh)
pnpm test          # vitest, with the real WebAssembly module; every control exercised (T-3373)
pnpm build         # the bundle the build lane publishes
pnpm e2e           # the built bundle in Chromium at four widths, light and dark (needs `pnpm build`)
(cd server && cargo test && cargo build --release --target wasm32-wasip2)   # the server component
```

`server/host-test.json` is the server component's scenario on the real host: the portal
repository's `tests/wasm-apps` (`tests/scenarios.rs`, ci-full) builds the component, runs
`migrations/` twice as the reconciler does, and plays the scenario against Postgres, RustFS and a
mock of the App's own Endpoint, with a second App that must see nothing.

## Where it is built

This tree is the repository `helsinki_bike-rebalancing` on the installation's forge. The build
lane tests and compiles `wasm/` into `wasm/pkg` (builder/build-wasm.sh, AP-142) before `pnpm build`,
tests `server/` and packs its component as `.jc/component.wasm` (builder/build-component.sh, AP-151),
and serves the bundle by its digest; the static host allows `'wasm-unsafe-eval'` and nothing wider.
