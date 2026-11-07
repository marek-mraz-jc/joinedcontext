# Otvorený výskum Žilinskej univerzity / Open research of the University of Žilina

The university app of T-3140: the works of DREPO that carry an open licence of their own, from
the public space `zilina-uniza`, as totals, works per year, the journals with the most works and
a list a person searches by title or collection, kind and year, each work linked to its handle.

## How it reads

- Every work, page by page (200 a page, at most 3 000): DREPO held 787 open ones on 2026-10-06.
- The journal of a collection is the name before its issue ("Krízový manažment" of
  "Krízový manažment - Ročník 24.; Číslo 2/2025").
- The years are bars for the eye and the same numbers as a table for a screen reader; a year
  between the first and the last with no work is a zero bar, not a gap that hides it.
- No author is shown: the space carries none, and DREPO names them with each work.
- The page sits in the SDK's `AppShell`; a work's title opens it in the `EntityPanel`, read only:
  a public App gains no write grant, editing is the Portal's (SDK-39, SDK-40, AP-140).

## Files

- `ui/src/works.ts` — a work, the journal, the counts and the narrowing; tested in `works.test.ts`
- `ui/src/App.tsx` — the screen, tested over the pipeline's own output in `App.test.tsx`
- `ui/src/fixtures/works.ts` — the 251 works drepo wrote from three pages of DREPO recorded on 2026-10-06
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px, light and dark, one kind and the panel
  open: no sideways scroll, no overlap, axe clean

The coverage gate (T-3373): `sh ../../../scripts/app-coverage-run.sh zilina-vyskum` from `ui/`.
