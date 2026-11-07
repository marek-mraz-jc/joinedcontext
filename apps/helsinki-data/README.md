# Helsingin avoin data / Helsinki open data

The data grids of T-2788: one tab per type of the city's public space `helsinki` (services, area
permits, events, traffic notices, parking zones, districts, water temperature, air quality, road
weather), each the SDK's `EntityGrid` with the city's Finnish and English column labels (and a full
Finnish grid vocabulary), a filter on every column that becomes the endpoint's own `q`, the model's
enums (kind of service, permit kind and status, zone kind, level) as pick lists in words, the name
pinned as the primary field that opens the row in the SDK's entity panel (SDK-40, linked to the
Portal), all in the SDK's shell (SDK-39), and the endpoint's own download in CSV (human headers). An event's contact point is not a column: it can name a person. It reads
through a public endpoint, never writes and never asks anybody to sign in.

## Files

- `ui/src/datasets.ts` — the grids, the enums and the download links; `datasets.test.ts`
- `ui/src/App.tsx` — the tabs and the panel, tested in `App.test.tsx`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean;
  a row in the entity panel at 375 and 1440 px, light and dark
