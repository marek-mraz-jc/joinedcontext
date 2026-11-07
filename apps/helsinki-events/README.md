# Helsinki events

Upcoming events in Helsinki and Espoo from the Linked Events registers. One page, Events:

- a search by name, place or words, and a From and To date;
- a chart of the events per day for the next 30 days, and a chart of the events per register;
  clicking a bar shows only that day or that register;
- every event with a location on the map, coloured by its register, with a legend;
- the list, soonest first: date, time, place, register, a Cancelled mark, the source link and
  the description.

An event's name in the list, or its marker on the map, opens it in the SDK shell's entity panel,
with a link to the event in the Portal; the application writes nothing, so the panel offers no Edit.

The Event entities carry no category. The one grouping they do carry is the register that
publishes them, which the prefix of the local id names (`helsinki-agf…` is the City of Helsinki,
`espoo_le-…` the City of Espoo, `kulke-…` the culture centres). The map colours and the second
chart group by that register.

It is a `static` application on the joinedcontext App SDK: React, read-only, one data need on the
Context Space `helsinki` (`Event`). The Portal serves it under `/apps/helsinki-events/` and fills
`#jc-config` with the application's own endpoint; the bundle talks to that endpoint and to nothing
else. The query asks only for events that have not ended (`endDate>=` the start of today).

The plain-HTML version this application replaces is the SDK's teaching example of a static App
with no build step: `sdk/examples/plain-html-events` in the portal repository.

## Run the tests

```sh
pnpm install
pnpm test          # vitest, against the SDK's stub transport and six sampled events
pnpm build         # the bundle the build lane publishes
pnpm e2e           # the built bundle in Chromium, answered by the same stub (needs `pnpm build`)
```

## Run it locally against dev

`pnpm dev` serves the application on `http://localhost:5173/`. Fill the `#jc-config` element in
`index.html` with the endpoint of the application on dev before you start:

```json
{ "slug": "<the endpoint slug the App page shows>", "orgDomain": "hel.fi", "space": "helsinki", "transport": "origin", "appName": "helsinki-events" }
```

Keep that edit out of the commit: the Portal writes the element when it serves the application.

## Where it is built

This tree is the repository `helsinki_helsinki-events` on the installation's forge. Publishing a
commit there is what the build lane builds: `pnpm install --offline`, `pnpm test`, `pnpm build`,
and the bundle is served by its digest.
