# Mapa Banskobystrického kraja / Map of the Banská Bystrica region

The citizen map of T-2784: the hospitals, the public social services and the organizations the
Banskobystrický samosprávny kraj founded, on one map with a search. It sits in the SDK's
`AppShell` (SDK-39); a place picked in the list or on the map opens in the shell's entity panel
(SDK-40), read fresh through the same endpoint, which links it to the Portal for editing and offers
no Edit of its own on this public App (AP-140). It reads the region's public register space `bbsk-registre` through a public endpoint, never writes
and never asks anybody to sign in.

- Each kind loads on its own, page by page up to 1 000; a kind the endpoint refuses is a sentence
  saying why, and the others stay.
- Social services publish no position: they are in the list, said so ("bez polohy"), and never
  placed on the map by a guess.
- The panel shows a published link as text, so a `javascript:` URL in the data never runs.
- The same layout as the city's map (`apps/banskabystrica-mapa`), its own kinds, colours and words.

## Files

- `ui/src/places.ts` — rows as places, search and order; `places.test.ts`
- `ui/src/App.tsx` — the shell and the screen, tested in `App.test.tsx` over
  `ui/src/fixtures/registre.ts`: every control exercised, the coverage gate of T-3373 held
  (`sh scripts/app-coverage-run.sh bbsk-mapa`)
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe
  clean; and a place in the entity panel at 375 and 1440 px, light and dark
