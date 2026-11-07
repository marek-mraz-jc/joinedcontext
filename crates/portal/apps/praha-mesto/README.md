# Praha teď / Prague right now

The one working Prague app of the owner's one-hour deadline (T-2917): nextbike stations with the
bikes and free docks they hold now, the park-and-ride car parks with their capacity and, where
TSK's counters feed the space (T-2907), the free and occupied spaces, and ČHMÚ's hourly air
quality per station. Everything is read from the praha-mesto space through this app's own
endpoint with the reader's session; the bundle holds no credential.

It runs in the SDK's `AppShell` (SDK-39). The name of a station, a car park or an air station in a
table opens it in the SDK's entity panel with its attributes and a link to it in the Portal; a
public App has no Edit (SDK-40, AP-140). `pnpm e2e` (after `pnpm build`) opens the screen and a car
park at four widths, light and dark.

Each section loads on its own: a type the endpoint refuses turns its section into a sentence
saying why, and the other two stay. A row missing the attribute a line is about is left out or
shown as "–", never as a zero. The four-app set of T-2786 (KPI dashboard, grids, map, operations)
builds on this one.

- `ui/src/praha.ts` — the rows as the screen reads them, tested in `praha.test.ts`
- `ui/src/App.tsx` — the three sections, tested over recorded pipeline output in `App.test.tsx`
- `ui/src/fixtures/` — what the praha pipelines wrote from the recorded answers of 2026-09-25
