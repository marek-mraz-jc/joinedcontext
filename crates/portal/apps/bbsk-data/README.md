# Registre Banskobystrického kraja / Registers of the Banská Bystrica region

The data grids of T-2784: one tab per register of the region's public space `bbsk-registre`
(hospitals, social services, bridges, organizations, districts and municipalities), each the SDK's
`EntityGrid` with the region's column labels, a filter on every column that becomes the endpoint's
own `q`, the model's enums (hospital kind, form of service, provider, road class, heritage status,
area, level) as pick lists with their words, the name pinned as the row's primary field, and the
endpoint's own download of the register in CSV (human headers).
It sits in the SDK's `AppShell` (SDK-39), and a row's name opens the row in the shell's entity
panel (SDK-40), read fresh through the same endpoint. The App is public, so the panel links the row
to the Portal for editing and offers no Edit of its own (AP-140). It reads through a public
endpoint, never writes and never asks anybody to sign in. The same layout
as the city's grids (`apps/banskabystrica-data`), the region's own registers and words.

## Files

- `ui/src/datasets.ts` — the grids, the enums and the download links; `datasets.test.ts`
- `ui/src/App.tsx` — the shell, the tabs and the grid, tested in `App.test.tsx`: every control of
  every register (sort, column details, filter, query, downloads, each row's name) is exercised, and
  the App holds the coverage gate of T-3373 (`sh scripts/app-coverage-run.sh bbsk-data`)
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe
  clean; and a row in the entity panel at 375 and 1440 px, light and dark
