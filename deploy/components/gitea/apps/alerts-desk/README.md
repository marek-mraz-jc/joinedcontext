# Alerts desk

The capital region's traffic alerts (`Alert` of the `helsinki` space, fed by Fintraffic) as one
table: sort by any column, filter by category, subcategory, free text and end date, export the
rows shown as CSV or PDF, and open one alert, from its row (click or Enter), in the SDK's entity
panel (SDK-40). It reads and never writes: the owner's decision (a) on T-3016, after the generated
version's edit form asked for a write no person in the project holds. So the App's grant stays
`queryEntity`/`retrieveEntity`, no attribute is edited here, and the panel links to the alert in
the Portal, where a person with the rights changes it.

It is the SDK template (`sdk/template/`) with one page, `src/pages/AlertDesk.tsx`, in the SDK's
`AppShell` (SDK-39); the template's generic pages, map, charts, form and functions are left out
because the desk uses none of them.

Checks: `pnpm test` (the page, the template's components), `pnpm typecheck`, and `e2e/` at 375,
768, 1440 and 2560 px, and an alert in the panel at 375 and 1440 px light and dark, against
`src/fixtures.ts`, which holds invented alerts, never rows copied from the endpoint. The App is
under the Apps' coverage gate (T-3373): vitest 95 % (branches 90 %), every control exercised.

## Related

- `apps/helsinki-alerts` — the same alerts on a map, with a steward's record form
- T-3016 — this application; T-3122 — its validation on dev
