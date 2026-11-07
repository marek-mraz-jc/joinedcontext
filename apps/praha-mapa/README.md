# Mapa Prahy / Map of Prague

The citizen map of T-2786: schools, cultural venues, public toilets, PID ticket points and the
sorted-waste points of Prague's open data, on one map with a search. It reads the city's public
space `praha-mesto` through a public endpoint, never writes and never asks anybody to sign in.

It runs in the SDK's `AppShell` (SDK-39). A place picked in the list or on the map opens in the
SDK's entity panel with its attributes and a link to it in the Portal; a public App has no Edit
(SDK-40, AP-140).

- Points of interest are one type told apart by `serviceCategory`; one of a category the map does
  not show is left out, never drawn under a wrong layer.
- The sorting isles are several thousand: their layer starts off and is one checkbox away, so the
  map is readable on a phone; each type loads page by page up to 4 000 and says when it was cut.
- A value a place does not carry stays missing, never a 0 or an empty string; only http(s) links
  count.
- The same layout as the Slovak cities' maps, Prague's kinds, colours and Czech words.

## Files

- `ui/src/places.ts` — rows as places, kinds, search and order; `places.test.ts`
- `ui/src/App.tsx` — the screen, tested in `App.test.tsx` over `ui/src/fixtures/praha.ts`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean
