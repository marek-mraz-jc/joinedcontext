# A joinedcontext application (`ui`)

React 19 on the App SDK, served by the Portal's static host: no pod of its own, and it reads and
writes only through the endpoints the App's `dataNeeds` name. Small server-side work (a computed
value, a form submit, a scheduled fetch) goes into `functions/`, which the Portal runs (SDK-21).

## Commands

| Command      | What it does                                                                 |
|--------------|------------------------------------------------------------------------------|
| `pnpm dev`   | Vite's dev server with hot reload                                            |
| `pnpm test`  | vitest with testing-library, for `src/**/*.test.tsx` and `functions/**/*.test.ts` |
| `pnpm build` | typechecks (`tsc -b`) and bundles into `dist/`                               |
| `pnpm exec playwright test` | after `pnpm build`: every page of `dist/` at 375, 768, 1440 and 2560 px, served from `src/fixtures.ts` |

The build lane runs the same tests, build and browser checks on every push to `main`, with the packages its
runner image holds, pinned in `package.json` and `pnpm-lock.yaml` of the Portal release: it
installs nothing, and a `package.json` naming a package the template does not carry fails the
build. On your own machine, `pnpm install` needs the SDK of the same release: run `pnpm pack` in
the Portal repository's `sdk/` and add the tarball (`pnpm add ./joinedcontext-sdk-0.1.0.tgz`).

## Where things are

| Path                      | Holds                                                                  |
|---------------------------|------------------------------------------------------------------------|
| `src/App.tsx`             | the pages: the overview and one page per entity type; add a page here   |
| `src/components/AppShell.tsx` | the header and the routing between pages (`#/{page}`, `navigate(id)`) |
| `src/pages/TypePage.tsx`  | one type: filters, tiles, map, charts, grid, export, detail, edit form |
| `src/components/`         | `shape.ts` (what a page reads from a type: filters, columns, labels), `TypeGrid` (the SDK's `EntityGrid` over one type), `EntityMap` (maplibre), `FeatureMap` (a type's map from the endpoint's OGC Features `/items`, the rows when the endpoint serves none), `charts` (echarts), `EntityForm`, filters, states, `EmbeddedFrame` |
| `src/endpoints.ts`        | which endpoint each type is read through, when the app reads several    |
| `src/i18n.ts`             | every text the app shows, in English, Slovak, German and Czech          |
| `src/jc-types.ts`         | the row types of the endpoint, rendered from its model: never edit      |
| `src/design-tokens.json`  | the app's look, inside the platform's branding                           |
| `functions/`              | server-side functions, called with `useFunction(name, input)`           |
| `src/main.tsx`            | the entry the Portal owns: it stays as it is                             |

## Data

`useEntities(type)`, `useEntity(id)`, `useSave()`, `useSchema()`, `useAccess()` and `useMe()`
from `@joinedcontext/sdk` call the endpoints with the person's own token. An App reading several
endpoints names each in `dataNeeds`; the client reads a type through the endpoint that serves it,
and a type two endpoints serve gets a page per endpoint (`sourcesOf` in `src/endpoints.ts`), read
with `useEntities(type, { endpoint })`, checked with `useAccess(endpoint)` and created with
`useSave().create(type, attrs, localId, { endpoint })`. `localId` becomes
`urn:ngsi-ld:{Type}:{orgDomain}:{space}:{localId}`; a whole `urn:ngsi-ld:{Type}:…` id is kept as
given. An entity is its endpoint's space and its id: with several endpoints serving a type, name
the endpoint on `get`, `update` and `remove` too, because the id never says which space it is in.
A write is offered only where `useAccess().can(...)` says the person's grant allows it.

## Languages

`t("key", { name: value })` from `src/i18n.ts` answers in `?lang=`, else the browser's language,
else English, and sets the document's language for screen readers. A new text is a key in every
catalog; the tests fail when a language misses a key or a `{placeholder}`.

## Imports

Interface code imports its own files, `react`, `react-dom/client`, `@joinedcontext/sdk`,
`echarts`, `recharts`, `maplibre-gl` and `@deck.gl/*`; a function its own files under
`functions/` and `@joinedcontext/sdk/server`; a test also `vitest`, `@testing-library/react` and
`@joinedcontext/sdk/testing`. The Portal refuses any other import before the preview (SDK-12).

## A page of another site

`<EmbeddedFrame src="https://…" title="…" />` shows an https page of another site inside the App
(a map embed, a video). The browser draws it only when the App's manifest names the origin:

```yaml
spec:
  csp:
    frameSrc: ["https://www.openstreetmap.org"]
```

The Portal writes those origins into the App's `frame-src`, beside `'self'` and nothing else; an
origin that is not one https address is left out (AP-12). A page of the App's own origin is never
framed this way (AP-19).
