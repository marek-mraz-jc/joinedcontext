# Helsinki KPIs: trend and forecast

Every indicator of Helsinki's `helsinki-kpi` space on one page:

- one sentence that answers on arrival: how many indicators, how many are rising, falling or flat
  over the period, how many readings look wrong, and how many have too little history;
- the indicators, each with its current value, its direction and change a week, and its odd
  readings;
- the chosen one in full: what was measured, what the model expected one step ahead, the forecast
  with its 95 % band and the readings that look wrong; its formula, source and the model's step
  and season;
- the period (7, 30 or 90 days), "only indicators with odd readings" and the chosen indicator in
  the address, so a view is a link (`?kpi=urn:…&days=7&odd=1&lang=en`).

The page sits in the SDK's shell (SDK-39), and "All details" opens the chosen indicator in the
SDK's entity panel (SDK-40), which links it to the Portal: a public App writes nothing.

Finnish, English or Swedish (`?lang=fi|en|sv`, else the browser's language, switched in the shell); the indicators' names are the
pipelines' own, in English. Light and dark follow the reader's system. Times are Helsinki's.

## How it is built

A `wasm` App (AP-142, ADR-N-044): React on the joinedcontext App SDK for the page, and the analysis
in Rust, compiled to WebAssembly and run in a Web Worker in the visitor's browser, so the period and
the chosen indicator answer at once. The static host serves its files, the endpoint answers the
indicators and their history, and the App's server keeps the forecasts (below).

- `wasm/` is the crate. `stats.rs`: median, quantiles, a robust standard deviation (IQR / 1.349),
  least squares. `forecast.rs`: the history onto a regular clock (the median step, at most 2000
  steps), the season (a day of hourly steps, a week of daily ones), robust Holt-Winters (an error
  over three robust deviations moves the model by three only) with its weights picked from a grid
  by the smallest clipped one-step error. `lib.rs` (`analyse`: JSON in, JSON out): per indicator
  the trend (flat under a 2 % move or an R² under 0.3), the forecast (one season, else a quarter of
  the history, at most 24 steps) and the readings three robust deviations off the model.
- `src/history.ts` reads `currentValue` over the period through the temporal endpoint; a refused
  read keeps the current values on screen and says so.
- The data need is one: `KeyPerformanceIndicator` in the Context Space `helsinki-kpi`, with the
  temporal read limited to 90 days (`temporalQ`).

## Forecasts kept, set against what came

The App's server is a WebAssembly component on the platform's shared host (`server/`):

- its daily job `record-forecasts` (03:20 UTC) reads the last 90 days of every indicator's history
  from the App's own Endpoint once, 100 indicators a page with `timerel=after`, forecasts each for
  every period with the page's own crate (`../wasm`) and keeps each forecast with the day it was
  made in the App's schema (`migrations/`). No request reads the history: at 6,719 indicators it
  takes longer than a request's 5 s (T-6197). The Endpoint is public and its policies are its own
  role, so every visitor reads the same history;
- **Earlier forecasts against what came**, under the chosen indicator: its kept forecasts whose
  time has come, each with the reading at that time (within half the model's step) and whether it
  fell within the 95 % band, the latest ten;
- **Monthly report**: for this month or one of the two before, every kept forecast point of the
  month against the reading, as CSV under the App's own prefix, `reports/{YYYY-MM}.csv`, downloaded
  through a URL valid for two minutes. The daily job writes this month's report and last month's
  until a run after its end wrote it, then keeps it as it is; until the job wrote a month's report
  the page says when it will. A month older than the 90 days of history the App may read is
  refused.

| Route | What it does |
|---|---|
| `POST /api/forecasts` `{days}` | 200 when the job recorded today's forecasts of the period, 202 until it does |
| `GET /api/forecasts?kpi=&days=` | an indicator's kept forecasts, newest first |
| `POST /api/reports` `{month}` | the URL the job's report of the month downloads from; 409 until the job wrote it |

## Roles and history

The server decides from the signed-in person the platform names (`identity::caller()`, API/07),
never from anything the page sends:

- anyone, signed in or not, reads every indicator and the **published forecast** under the chosen
  one: who published it and when, the period and the horizon it was made with, the steward's note
  and its points with their 95 % band, and the earlier publications;
- a **viewer** follows indicators (**Follow** beside "All details") and finds them under
  **Followed**, each opened on the indicators page or let go; a follow of someone else's answers
  as if it did not exist;
- a **steward** publishes the chosen indicator's forecast: the server reads its history over the
  period on screen, forecasts it with the page's own crate over the horizon the steward chose (1 to
  48 steps) and keeps it with the note; any steward withdraws one. An indicator with too little
  history is refused with the reason.

Signed out, Follow and the publish form stay in sight, closed, with the reason as their tooltip. A
follow let go and a forecast withdrawn go at once and come back when the server refuses. Every
change writes a row into the App's `app_audit` table in the same statement, so a refused or failed
change leaves none; the **History** page reads it: a viewer their own rows, a steward everyone's.
Finnish, English and Swedish.

| Route | Who | What it does |
|---|---|---|
| `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /api/published?kpi=` | anyone | the indicator's published forecasts, newest first, at most 10 |
| `POST /api/published` `{kpi, days, horizon, note}` | steward | a forecast made with these parameters and published |
| `DELETE /api/published/{id}` | steward | the forecast withdrawn |
| `GET /api/follows` | signed in | the indicators the caller follows |
| `POST /api/follows` `{kpi}` | signed in | an indicator followed, at most 50 |
| `DELETE /api/follows/{id}` | the follower | no longer followed |
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

`pnpm wasm` comes first: the tests, the type check and the build import `wasm/pkg/`. The build lane
runs the same two steps itself (builder/build-wasm.sh) on the app-build-rust runner. It needs the
`wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version the crate pins.
