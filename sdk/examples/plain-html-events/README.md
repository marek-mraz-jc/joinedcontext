# Plain-HTML example: Helsinki events

The SDK's teaching example of a `static` App with no build step (AP-83). It is not a published App: the published `helsinki-events` is the React App in `apps/helsinki-events` (AP-138, T-2923). This folder keeps what AP-83 is tested on.

Upcoming events of the City of Helsinki's Linked Events register, with search, a date filter, a detail view and a map of where they are. It is the plain-HTML sample application (T-2597): `spec.build: {}`, so this folder is the bundle as it is, with no build step and no package manager (AP-83).

| File | What it is |
|---|---|
| `app.yaml` | The App manifest: `static`, `build: {}`, public, one data need on `Event` in `helsinki` |
| `index.html` | The page: landmarks, the filter form, the list, the map and the detail |
| `app.js` | Pure functions (endpoint choice, query, entity → view, filter, map projection) and the DOM wiring below them |
| `style.css` | Light and dark colours, a one-column layout below 48 rem, the filters one label and field per row below 60 rem, the map at most 20 rem high |
| `test/app.test.mjs` | The pure functions on `test/events.json` |
| `test/page.test.mjs` | The page wiring in a DOM: the read, the list, the map, the detail, the filters, every failure |
| `test/vitest.config.mjs`, `test/setup.mjs` | The tests' configuration and the coverage gate's controls record (T-3373), kept out of the served root |

## Where the data comes from

The Portal static host writes a `#jc-config` element into `index.html` when it serves the page. The page takes from its `endpoints` the one named `helsinki-events`, else one serving `Event`, else the primary `slug`, and reads `GET /api/endpoint/{slug}/ngsi-ld/v1/entities?type=Event&limit=100&q=endDate>={start of the day}` on its own origin with `Accept: application/ld+json`. It reads nothing else and talks to no other host: the map is an SVG drawn from the entities' `location`, with no tile server.

The attributes it shows are the ones the Event entities carry on dev: `name` and `description` (language maps, shown in the reader's language, else English, Finnish or Swedish), `startDate`, `endDate`, `eventStatus`, `address`, `location` and `source`. A `source` that is not an `https:` link is not shown as a link.

## Run it locally

Tests, with the SDK's vitest and jsdom (the page itself needs no package manager):

```sh
cd ../.. && npx vitest run --root examples/plain-html-events --config test/vitest.config.mjs
```

CI runs them in a copy given a throwaway `package.json`, under the Apps' coverage gate (T-3373):
100 % lines of `app.js`, every control exercised.

The browser flow runs with the Portal UI's Playwright suite (`ui/e2e/app_helsinki_events.spec.ts`, which serves this folder). It serves this folder with a `#jc-config`, the static host's Content Security Policy and a stubbed endpoint, and holds the list and a chosen event to four widths (375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean).

`python3 -m http.server 8000` in this folder serves the page, but without the `#jc-config` the platform writes, it says it has no endpoint to read. Your browser also refuses to read dev's endpoint from `localhost`, because `connect-src` and CORS both stay on one origin. To see it with data, run the Playwright flow, or open it on dev once T-2599 has seeded it.

## How it reaches dev

The repository of record is `joinedcontext/helsinki_helsinki-events` on the dev forge (AP-75). T-2599 pushes this folder there unchanged and proposes `app.yaml`; the build lane uploads the tree with its `integrity.json` (AP-80, AP-83), and the static host serves it under `/apps/helsinki-events/`.
