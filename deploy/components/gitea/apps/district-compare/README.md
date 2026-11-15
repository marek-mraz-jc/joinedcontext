# Compare districts

Helsinki districts compared side by side: events, bike stations, alerts, and air quality:

- a choropleth map of all Helsinki districts coloured by the chosen measure (default: events per km²);
- the two districts with the most events preselected on arrival;
- side-by-side comparison table showing each selected district's values, values per km² (where applicable), and ranking among all districts;
- bar chart for the chosen measure across the selected districts;
- ranked district list allowing selection via checkboxes or directly on the map;
- URL hash keeping the active selection and measure (`#compare?d=101,102&m=events`).

Finnish, English or Swedish (`?lang=fi|en|sv`, else the browser's language), switched in the SDK's `AppShell` (SDK-39), with the SDK's loading, empty and error states. Light and dark themes follow the reader's system.

A district picked on the map joins the comparison and opens in the SDK's entity panel (SDK-40); each compared district has its Details button. The App is public and writes nothing, so the panel links the district to the Portal and offers no Edit (AP-140).

## How it is built

A `wasm` App (AP-142, AP-148): React on the joinedcontext App SDK for the page, spatial point-in-polygon and ranking analysis in Rust, compiled to WebAssembly and run in a Web Worker in the visitor's browser, and a server component on the shared WASM host for what the browser cannot keep (T-3353).

- `wasm/` is the crate: `geo.rs` (point-in-polygon, geometry area in km², bounding boxes, representative coordinate extraction) and `lib.rs` (`compare`: JSON in, JSON out, computing district aggregates, per-km² rates, rankings, and points falling outside districts).
- `src/compare.worker.ts` loads the module once and answers each call; `src/compare.ts` is the page's side of it.
- Reads `AdministrativeArea` (`divisionLevel = "district"`), `Event`, `BikeHireDockingStation`, `Alert`, and `AirQualityObserved` in the Context Space `helsinki`.
- `server/` is the server component (`wasm32-wasip2`, on `jc-app-sdk`), reusing `wasm/` without wasm-bindgen. Each hour until it has today's figures, on Helsinki's calendar, the job `keep-metrics` reads the same five types through the App's own Endpoint as the App's job principal, runs the same comparison and keeps every district's figures for that day in its table `district_metrics` (`migrations/`), so a district has a history; it also writes the boundaries it compared against as `boundaries/districts.geojson`, with `boundaries/LICENCE.txt` (CC BY 4.0, City of Helsinki, HRI package helsingin-piirijako), under the App's storage prefix. The page shows the selected districts' days for the chosen measure and downloads both files through presigned URLs.

| Route | What it does |
|---|---|
| `GET /apps/district-compare/api/metrics?codes=101,102` | the kept days of up to six districts, the newest first; `stale` until the job kept today's |
| `GET /apps/district-compare/api/boundaries` | URLs to download the boundaries and their licence from |

## Roles and history

A signed-in reader keeps comparisons of their own (T-3828): a name, up to six districts and a measure, at most 20. They open, update or delete their own, and share one by its link (`?comparison=<share token>`), which any signed-in reader of the project opens read-only with its owner named. The token is 64 hex characters the database draws for each comparison (`migrations/0003_share_tokens.sql`); by its id, which anyone could count through, a comparison answers its owner only. A steward pins the comparison on screen for everyone: the page opens on the newest pin unless its address names districts or a shared comparison; taking a pin back returns to the one before. Every change shows at once and is taken back when the server refuses; the server records each in `app_audit`, and the Changes page lists a viewer's own and a steward's every one. Roles come from the caller's token through `identity::caller()`, never from the page.

| Route | Who | What it does |
|---|---|---|
| `GET /apps/district-compare/api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /apps/district-compare/api/comparisons` | signed in | the caller's comparisons, newest first |
| `POST /apps/district-compare/api/comparisons` `{name, codes, measure}` | signed in | a comparison kept |
| `GET /apps/district-compare/api/comparisons/{id}` | owner | the caller's own comparison |
| `GET /apps/district-compare/api/shared/{share}` | signed in | the comparison a link's token names, read-only, with its owner's name |
| `PUT /apps/district-compare/api/comparisons/{id}` | owner | changed |
| `DELETE /apps/district-compare/api/comparisons/{id}` | owner | gone |
| `GET /apps/district-compare/api/default` | anyone | the pinned comparison, `null` when none |
| `PUT /apps/district-compare/api/default` `{name, codes, measure}` | steward | pinned for everyone |
| `DELETE /apps/district-compare/api/default` | steward | the newest pin taken back |
| `GET /apps/district-compare/api/history` | signed in | the newest 100 changes (a viewer: their own) |

The groups `district-compare-viewer` and `district-compare-steward` (`grants/users/groups/`) hold the demo viewer and steward. The App writes no context data, so its Endpoint's Policy stays read-only.

## Run the tests

```sh
pnpm install
pnpm wasm          # cargo build --target wasm32-unknown-unknown, then wasm-bindgen --target web
(cd wasm && cargo test)
(cd server && cargo test && cargo build --release --target wasm32-wasip2)
pnpm test          # vitest, with the compiled module run in-process
pnpm build
pnpm e2e           # the built bundle in Chromium: 4 widths, light and dark, fi and en, axe
```

`pnpm test`, `pnpm typecheck` and `pnpm build` import `wasm/pkg/`, so `pnpm wasm` runs first; the build lane runs it as `builder/build-wasm.sh` (T-3327).
The build needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version the
crate pins.
