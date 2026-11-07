# Bikes and the weather (Pyörät ja sää)

"Will there be a bike at my station?" For one city-bike station: its availability by hour of the week, how temperature and rain change it, and an estimate for the next 6 hours with its uncertainty.

Built with React 19, TypeScript, and a WebAssembly analysis engine compiled from Rust (`wasm-bindgen`). Runs completely in the browser as a static `ui` bundle without requiring server pods.

## Data sources

- **BikeHireDockingStation**: Current state and 7-day temporal history (`availableBikeNumber`) for the chosen station.
- **WeatherObserved**: Current road weather observations and 7-day temporal history (`temperature`, `precipitation`) from the nearest station within 10 km (calculated via haversine distance), or the weather station and radius a steward set for the station.

## In the shell

The page sits in the SDK's `AppShell` (SDK-39) with its Suomi/English/Svenska switch, and uses the SDK's
loading, empty and error states. A station picked on the map, and the chosen station and its
weather station by their Details buttons, open in the shell's entity panel (SDK-40), read fresh
through the App's endpoint. The App is public, so the panel never offers Edit and links the entity
to the Portal (AP-140); the stations and the weather are their feeds' pipelines', and the App writes
nothing. Retry on a history that could not be read reads it again.

## Analysis engine

- Computes Europe/Helsinki hours of week (0..=167) accounting for EU daylight saving time transitions.
- Evaluates the baseline hour-of-week profile with fallback to day-hour averages.
- Fits the weather terms with the within-slot estimator: the residual bike count, the temperature and rain are each taken from their slot group's mean before the 3×3 least-squares fit, so the profile does not absorb weather effects that follow the time of day (a plain residual fit came out biased toward zero).
- Projects availability over the next 6 hours with an 80 % prediction interval (±1.28 σ), assuming the weather stays as last observed: the data holds no forecast.
- Reads one station's history only, through the SDK's temporal `id` parameter: a week of every station's minute-by-minute availability would be millions of points.

## Server component

A `wasm` App (AP-148, T-3355): beside the browser's module, `server/` (`wasm32-wasip2`, on `jc-app-sdk`) runs on the shared WASM host and reuses `wasm/` without wasm-bindgen. The broker keeps only a station's last seven days, so once a day per station, on the first request for it, the server reads the station's week and the nearest weather station's (within 10 km, as the page picks it) through the App's own Endpoint with the reader's token, trains the same model and keeps its weather coefficients, spread and hour-of-week profile in its table `station_models` (`migrations/`). The data it trained on goes to `training/<day>/<hash>.json` under the App's storage prefix. The page shows the selected station's models day by day and downloads a day's training data through a presigned URL.

| Route | What it does |
|---|---|
| `GET /apps/bike-weather-demand/api/models?station=<urn>` | the station's kept models, newest first |
| `GET /apps/bike-weather-demand/api/models/{id}/snapshot` | a URL to download the data a model was trained on |

## Roles and history

A steward tunes a station's model inputs (T-3827): a weather station to read instead of the nearest, how far the nearest may be (1 to 25 km, default 10) and how many days the model trains on (1 to 7, default 7), with a note. The server keeps them in `station_inputs`, drops today's model of that station so the next read trains it again on them, and records each change in `app_audit`. Every reader sees the inputs above the kept models, and the page's own estimate reads with them too. A viewer's controls are disabled, each with its reason as a tooltip; a save shows at once and is taken back when the server refuses. History lists a viewer's own changes and a steward's every one. The roles come from the caller's token through `identity::caller()`, never from the page.

| Route | Who | What it does |
|---|---|---|
| `GET /apps/bike-weather-demand/api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /apps/bike-weather-demand/api/inputs?station=<urn>` | anyone | the station's model inputs, the defaults when none are set |
| `PUT /apps/bike-weather-demand/api/inputs?station=<urn>` `{weatherStation, radiusKm, days, note}` | steward | inputs set; today's model trained again |
| `DELETE /apps/bike-weather-demand/api/inputs?station=<urn>` | steward | back to the defaults |
| `GET /apps/bike-weather-demand/api/history` | signed in | the newest 100 changes (a viewer: their own) |

The groups `bike-weather-demand-viewer` and `bike-weather-demand-steward` (`grants/users/groups/`) hold the demo viewer and steward. The App writes no context data, so its Endpoint's Policy stays read-only.

`server/` is tested natively (`cargo test`), built with `cargo build --release --target wasm32-wasip2`, and played on the host against Postgres and RustFS from `server/host-test.json` by the platform's `crates/wasm-host/tests/apps_tests.rs`.

## Development

```bash
# Build the WebAssembly module
pnpm wasm

# Run development server
pnpm dev

# Run unit and integration tests (every control exercised, the coverage gate of T-3373)
pnpm test

# Check types and produce production bundle
pnpm build

# Run Playwright responsive end-to-end tests
pnpm e2e
```
