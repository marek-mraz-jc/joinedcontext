# Otvorené dáta o Žiline / Open data about Žilina

The grids of T-3140: the public datasets of the Žilina project as tables — the monuments, the
railway stations and the air station of `zilina-verejne` and the university's works of
`zilina-uniza` — each with the model's columns, a filter in each column the endpoint can answer
and the whole dataset as a CSV file. A ui app reads its own space and one more (AP-04), so the
six indicators of `zilina-kpi` are shown by zilina-ukazovatele.

## How it reads

- Each dataset through the app's own endpoint of its space, found by space and never by position.
- Read only: the pipelines write these data, and their next run would replace any edit.
- The page sits in the SDK's `AppShell`; a row's name in the grid opens it in the `EntityPanel`,
  read only: a public App gains no write grant, editing is the Portal's (SDK-39, SDK-40, AP-140).
- The CSV is the whole dataset, read page by page (500 a page, at most 5 000 rows): a name in the
  reader's language, a window as `start/end`, an empty value as an empty field.
- The grid is the SDK's `EntityGrid` (paging, filters as the endpoint's own `q`).

## Files

- `ui/src/datasets.ts` — the datasets, their grids and the export; tested in `datasets.test.ts`
- `ui/src/App.tsx` — the screen, tested over the pipelines' own output in `App.test.tsx`
- `ui/src/fixtures/data.ts` — what the Žilina pipelines wrote from the feeds recorded on 2026-10-06
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px, light and dark, a row in the panel:
  no sideways scroll, no overlap, axe clean

The coverage gate (T-3373): `sh ../../../scripts/app-coverage-run.sh zilina-zaznamy` from `ui/`.
