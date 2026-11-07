# Data Prahy / Data of Prague

The data grids of T-2786: one tab per type of the city's public space `praha-mesto` (places,
recycling points, sensor containers, park and ride, shared bikes, air quality, city districts,
budget lines), each the SDK's `EntityGrid` with the city's Czech and English column labels, a filter
on every column that becomes the endpoint's own `q`, the model's enums (category, access, kind of
waste, status) as pick lists in words, the first column pinned as the primary field (the name, or
the code where a type has no name) that opens the row in the SDK's entity panel (SDK-40, linked to the Portal) inside the SDK's shell (SDK-39), and the endpoint's own download in CSV
(human headers). It reads through a public endpoint, never writes and never asks anybody
to sign in.

## Files

- `ui/src/datasets.ts` — the grids, the enums and the download links; `datasets.test.ts`
- `ui/src/App.tsx` — the tabs and the panel, tested in `App.test.tsx`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean;
  a row in the entity panel at 375 and 1440 px, light and dark
