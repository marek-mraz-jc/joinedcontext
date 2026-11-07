# Mapa mesta Žilina / Map of Žilina

The citizen map of T-3140: the city's national cultural monuments placed at their buildings, its
five railway stations with the trains leaving each today, and the air-quality station SK0020A,
on one map with a search; a place opens in the SDK's entity panel, which links to it in the Portal. It reads the public space `zilina-verejne`
through a public endpoint, never writes and never asks anybody to sign in.

## How it reads

- Each kind (PointOfInterest, GtfsStop, AirQualityObserved) loads on its own, page by page up to
  1 000: a kind the endpoint refuses becomes a sentence saying why, and the others stay.
- The list is the screen and the map its picture: a keyboard and a screen reader use the list,
  which also holds the monuments without an address ("bez adresy").
- One kind or all of them at a time; stations busiest first, the rest by name.
- The air station's line in the list gives PM10 in its own unit; the entity panel shows every
  pollutant as the entity carries it, with its unit and the hour it was observed.
- The page sits in the SDK's `AppShell`; a place picked in the list or on the map opens in the
  `EntityPanel`, read only: a public App gains no write grant, editing is the Portal's (SDK-39,
  SDK-40, AP-140).

## Files

- `ui/src/places.ts` — the rows as places, the search and the order; tested in `places.test.ts`
- `ui/src/App.tsx` — the screen, tested over recorded rows in `App.test.tsx`
- `ui/src/fixtures/verejne.ts` — what the pipelines pamiatky, vlaky and ovzdusie-* wrote from the feeds recorded on 2026-10-06
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px, light and dark, with and without the
  panel open: no sideways scroll, no overlap, axe clean

The coverage gate (T-3373): `sh ../../../scripts/app-coverage-run.sh zilina-mapa` from `ui/`.
