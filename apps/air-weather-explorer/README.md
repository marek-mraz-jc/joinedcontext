# Air quality and weather

How the weather moves Helsinki's air quality, station by station. One page, in Finnish or
English:

- the answer on opening, in one sentence: at the first air quality station, compared with the
  nearest weather station over three days, which pollutant moves most with which weather variable,
  which way and how strongly, with the Spearman and Pearson correlations and the number of hours
  they are computed over;
- the choices: the air quality station, the weather station (the nearest named with its distance),
  the period (1, 3 or 7 days), the rolling mean (1 to 12 hours), the pollutant and the variable;
- the chosen pair hour by hour on one chart, the pollutant's outlying readings marked;
- every pollutant against every weather variable as a heat map; a click on a cell shows that pair;
- the stations on the map; a click chooses one and opens it in the entity panel;
- every pair in a table, with Spearman, Pearson and the hours.

The page sits in the SDK's `AppShell` (SDK-39), which carries the language switch. A station on the
map, or the "Details of" button of each chosen station, opens it in the SDK's entity panel
(SDK-40): its attributes as the App's grant reads them, and a link to it in the Portal, where a
person with the rights changes it. The App is public and writes nothing, so the panel offers no
Edit (AP-140).

Every choice is kept in the address (`?station=`, `?weather=`, `?days=`, `?window=`, `?air=`,
`?w=`, `?lang=`).

## How it is computed

The statistics are Rust, in `wasm/`, compiled to WebAssembly and run in the reader's browser in a
Web Worker, so every change of smoothing or pair is answered at once:

- each attribute's readings are averaged per hour, and all of them put on the same hours;
- a rolling mean over the chosen window smooths each series, skipping missing hours;
- Pearson's and Spearman's correlation (ranks, ties shared) over the hours both sides have, at
  least six; a constant side says nothing;
- an outlier is a reading more than 3.5 robust standard deviations (1.4826 × MAD) from the median.

The strength words are: under 0.2 "hardly", under 0.4 "weakly", under 0.7 "moderately", else
"strongly". A correlation is not a cause: the page says how they move together, nothing more.

## The server: kept hours, saved comparisons, exports

The App's server is a WebAssembly component on the platform's shared host (`server/`,
`kind: wasm`, ADR-N-044). It keeps each station's readings as hourly means in the App's own schema
(`migrations/`), and reads the App's own Endpoint, as the visitor, only for the hours it has not
kept yet (from the last kept hour, at most a week back, at most every 15 minutes). The page asks it
for the two chosen stations' hours of the period instead of reading a week of every station's raw
readings; the correlations stay in the browser. The Endpoint is public and its policies are its own
role, so every visitor reads the same readings and what one visitor's request kept, another may be
shown. A station's hours older than eight days are dropped.

- **Save the comparison** keeps the choices on screen (the stations, the period, the smoothing, the
  pair and an optional name) under a code of 12 letters and digits. The link, `?compare={code}`,
  opens them again: the page puts them into the address and drops the code. Comparisons are cleared
  half a year after they were saved, a few at each new one.
- **Export the hours (CSV)** writes the two stations' hourly means of the period, one row per hour
  and a column per attribute, under the App's own prefix, `exports/{code}.csv`, and downloads it
  through a URL valid for two minutes. Exports are cleared a day after they were written.

| Route | What it does |
|---|---|
| `GET /api/series?air=&weather=&days=` | both stations' hourly means over the last 1, 3 or 7 days |
| `POST /api/comparisons` | a comparison kept; its code |
| `GET /api/comparisons/{code}` | the comparison's choices |
| `POST /api/exports` | the hours as CSV; a URL it downloads from |

## Roles and history

The server decides from the signed-in person the platform names (`identity::caller()`, API/07),
never from anything the page sends:

- anyone, signed in or not, compares stations and sees the periods marked faulty;
- a **viewer** keeps up to 20 **places** of their own under **My places**: a station, a pollutant
  and a level. The page shows whether the latest hourly mean is above it and opens the station in
  the comparison. They change and delete their own places; a place of someone else's answers as if
  it did not exist;
- a **steward** marks a period of a station's readings **faulty** with a reason (at most 31 days)
  and takes a mark back. The comparison leaves every hour a mark covers out of the analysis, for
  everyone, and says how many hours went.

A change, a deleted place and a mark taken back show at once and go back when the server refuses.
Every change writes a row into the App's `app_audit` table in the same statement, so a refused or
failed change leaves none; the **History** page reads it: a viewer their own rows, a steward
everyone's.

| Route | Who | What it does |
|---|---|---|
| `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /api/places` | signed in | the caller's places, each with the latest hourly mean |
| `POST /api/places` `{station, name, attr, threshold}` | signed in | a place kept, at most 20 |
| `PUT /api/places/{id}` `{name, threshold}` | the owner | its name and level changed |
| `DELETE /api/places/{id}` | the owner | the place gone |
| `GET /api/faults?station=` | anyone | the station's faulty marks |
| `POST /api/faults` `{station, from, to, note}` | steward | a period marked faulty |
| `DELETE /api/faults/{id}` | steward | the mark taken back |
| `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

`server/host-test.json` plays every route on the real host as an anonymous visitor, a viewer,
another viewer and a steward, and counts the rows after each refused write.

## Data

Two data needs on the Context Space `helsinki`, each with a week of history (`retrieveTemporal`,
window `P7D`): `AirQualityObserved` with `pm10`, `pm25` and `airQualityIndex`, and
`WeatherObserved` with `temperature`, `windSpeed`, `relativeHumidity` (stored as a share, shown in
per cent) and `precipitation`, each with `name`, `location` and `dateObserved`. The app writes
nothing to the Context Space: what it keeps is in its own schema.

## Run the tests

Needs Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version
`wasm/Cargo.lock` pins and the build lane binds with.

```sh
pnpm install
pnpm wasm          # cargo test of wasm/, then the module into wasm/pkg (the lane runs builder/build-wasm.sh)
pnpm test          # vitest, with the real WebAssembly module
pnpm build         # the bundle the build lane publishes
pnpm e2e           # the built bundle in Chromium at four widths, light and dark, and a station in the panel (needs `pnpm build`)
(cd server && cargo test && cargo build --release --target wasm32-wasip2)   # the server component
```

`server/host-test.json` is the server component's scenario on the real host: the portal
repository's `tests/wasm-apps` (`tests/scenarios.rs`, ci-full) builds the component, runs
`migrations/` twice as the reconciler does, and plays the scenario against Postgres, RustFS and a
mock of the App's own Endpoint, with a second App that must see nothing.
The page's tests and `e2e/serve.ts` answer the server's routes from the fixture
(`src/testing/`).

The App is under the Apps' coverage gate (T-3373): `sh ../../scripts/app-coverage-run.sh
air-weather-explorer --rust` in a throwaway copy, vitest 95 % (branches 90 %) with every control
exercised, and `cargo llvm-cov` of `wasm/` at 95 % lines. wasm-bindgen's generated glue in
`wasm/pkg` is not counted.
