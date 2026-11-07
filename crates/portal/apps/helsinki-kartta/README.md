# Helsingin palvelukartta / Helsinki service map

The citizen map of T-2788: the libraries, health stations, swimming halls, beaches and schools of
Helsinki's service register, and the beaches' water-temperature sensors, on one map with a search
and each place opened in the SDK's entity panel (SDK-40, linked to the Portal), in the SDK's shell
(SDK-39). It reads the city's public space `helsinki` through a public endpoint,
never writes and never asks anybody to sign in. Finnish and English; a name is read in the reader's
language where the register has it (fi, sv, en), else in Finnish.

- Services are one type told apart by `serviceCategory`; one of a category the map does not show is
  left out, never drawn under a wrong layer.
- A place picked on the map or in the list opens in the panel, which shows every attribute as text,
  so a link in the data never runs; the list names the water temperature beside each sensor.
- The same layout as the other cities' maps; the bikes, events, alerts and transport are the
  existing Helsinki apps' (`helsinki-bikes`, `helsinki-events`, `helsinki-alerts`, `hsl-transport`).

## Files

- `ui/src/places.ts` — rows as places, kinds, search and order; `places.test.ts`
- `ui/src/App.tsx` — the screen, tested in `App.test.tsx` over `ui/src/fixtures/helsinki.ts`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean,
  a place in the panel, light and dark
