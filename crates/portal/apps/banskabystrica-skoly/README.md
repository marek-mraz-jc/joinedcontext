# Školy mesta Banská Bystrica / Schools of Banská Bystrica

The staff operations app of T-2782: a school coverage desk. Every school of the national school
map in the city with its pupils, teachers, non-teaching staff and budget, read from the city's
public space `banskabystrica-verejne`, and the two ratios a desk compares: pupils per teacher and
budget per pupil. Staff sign in: the App is opened by its `viewer` role, which its default group
`banskabystrica-skoly-viewer` holds (T-2686, AP-118). It reads and never writes.

It sits in the SDK's `AppShell` (SDK-39); a school's name in the table opens the school in the
shell's entity panel (SDK-40), read fresh through the App's endpoint.

**What staff edit here: nothing.** The `dataNeeds` keep `queryEntity` and `retrieveEntity`. The
schools are the national school map's, written by its pipeline (T-2781) on each run, so a value
changed here would be overwritten and lost. The panel never offers Edit; it links the school to the
Portal, where a person with the right on the space corrects it under their own rights.

## What it claims, and what it does not

- A ratio comes only from counts the school published; a missing count makes the ratio
  "neuvedené", never 0, and the school is counted under "without complete figures".
- The city's ratio is all pupils over all teachers of the schools with both counts, not a mean of
  ratios.
- What stands out is judged against the city's own schools: the highest tenth of pupils per teacher
  and the lowest tenth of budget per pupil, marked in words beside the number. With fewer than ten
  schools with figures nothing is marked, and the page says why. There is no norm in this app.
- The CSV is the table as shown (search and filter applied), made in the browser, UTF-8 with a BOM;
  a cell that a spreadsheet would run as a formula is prefixed so it never executes.

## Files

- `ui/src/coverage.ts` — ratios, totals, the city's tenth, sorting and the CSV; `coverage.test.ts`
- `ui/src/App.tsx` — the shell and the desk, tested in `App.test.tsx`: every control exercised,
  the coverage gate of T-3373 held (`sh scripts/app-coverage-run.sh banskabystrica-skoly`)
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe
  clean; and a school in the entity panel at 375 and 1440 px, light and dark
