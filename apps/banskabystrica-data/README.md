# Dáta mesta Banská Bystrica / Data of Banská Bystrica

The data grids of T-2782: one tab per dataset of the city's public space `banskabystrica-verejne`
(events, schools, air quality), each the SDK's `EntityGrid` with the city's column labels, a
filter on every column that becomes the endpoint's own `q`, the model's `EventCategory` as a pick
list, the name pinned as the row's primary field, and the endpoint's own download of the dataset
in CSV (with human headers). It reads through a public endpoint, never writes and
never asks anybody to sign in.

The page sits in the SDK's `AppShell` (SDK-39). A row's name opens the entity in the SDK's entity
panel (SDK-40): its attributes as the grant reads them and a link to it in the Portal, where a
person with the rights changes it. The App is public and gains no write grant, so the panel offers
no Edit (AP-140).

- The downloads are the whole dataset under the same grant as the grid; the page says the grid's
  filter does not travel into the file.
- The tabs are a WAI-ARIA tablist: arrow keys, Home and End move between them.

## Files

- `ui/src/datasets.ts` — the grids and the download links; tested in `datasets.test.ts`
- `ui/src/App.tsx` — the tabs and the panel, tested in `App.test.tsx`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe
  clean; and an event in the panel at 375 and 1440 px, light and dark

The App is under the Apps' coverage gate (T-3373): vitest 95 % (branches 90 %), every control the
App draws exercised; the grid's own controls are the SDK suite's.
