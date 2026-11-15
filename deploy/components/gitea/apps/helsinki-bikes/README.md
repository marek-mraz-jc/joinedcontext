# Helsinki city bikes

Every city bike station in Helsinki. The overview shows three numbers: stations, bikes available
now, and stations with no bike. The Stations page has a map coloured by bikes available, a table,
a search by name, "Only stations with bikes", and a station chosen on a map or in the table opened in the SDK's entity panel (SDK-40, linked to the Portal), all in the SDK's shell (SDK-39).

It is a `static` application on the joinedcontext App SDK: React, read-only, one data need on the
Context Space `helsinki` (`BikeHireDockingStation`). The Portal serves it under
`/apps/helsinki-bikes/` and fills `#jc-config` with the application's own endpoint; the bundle
talks to that endpoint and to nothing else.

## Run the tests

```sh
pnpm install
pnpm test          # vitest, against the SDK's stub transport and five sampled stations
pnpm build         # the bundle the build lane publishes
pnpm e2e           # the built bundle in Chromium, answered by the same stub (needs `pnpm build`)
```

## Run it locally against dev

`pnpm dev` serves the application on `http://localhost:5173/`. Fill the `#jc-config` element in
`index.html` with the endpoint of the application on dev before you start, and sign in to the
Portal in the same browser so the endpoint accepts your session:

```json
{ "slug": "<the endpoint slug the App page shows>", "orgDomain": "hel.fi", "space": "helsinki", "transport": "origin", "appName": "helsinki-bikes" }
```

Keep that edit out of the commit: the Portal writes the element when it serves the application.

## Where it is built

This tree is the repository `helsinki_helsinki-bikes` on the installation's forge. Publishing a
commit there is what the build lane builds: `pnpm install --offline`, `pnpm test`, `pnpm build`,
and the bundle is served by its digest.
