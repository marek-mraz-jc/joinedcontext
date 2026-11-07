# Mapa mesta Banská Bystrica / Map of Banská Bystrica

The citizen map of T-2782: the events the city announces, every school of the national school map
in the city and the air-quality station, on one map with a search. It reads the city's public space
`banskabystrica-verejne` through a public endpoint, never writes and never asks anybody to sign in.

The page sits in the SDK's `AppShell` (SDK-39). A place picked in the list or on the map opens in
the SDK's entity panel (SDK-40) and is marked on the map: its attributes as the grant reads them,
and a link to it in the Portal, where a person with the rights changes it. The App is public and
gains no write grant, so the panel offers no Edit (AP-140).

## How it reads

- Each kind (Event, Organization for the schools, AirQualityObserved) loads on its own, page by page up to 1 000: a kind
  the endpoint refuses becomes a sentence saying why, and the others stay; a kind cut at 1 000 says
  so.
- The map is a picture of the list beside it. The list carries every place in text, is what a
  screen reader and a keyboard use, and holds the places without a position too ("bez polohy").
- Events are shown from today on by default; past ones are one checkbox away. An event with no
  date at all is kept, since nothing says it is over.
- Search folds diacritics and case and matches every word in the name or the address.
- The panel shows a value the entity does not carry as empty, never as 0, and shows a URL as
  text, so a `javascript:` URL in the data never becomes a link.

## Files

- `ui/src/places.ts` — the rows as places, the filters and the order; tested in `places.test.ts`
- `ui/src/App.tsx` — the screen, tested over recorded rows in `App.test.tsx`
- `ui/src/fixtures/verejne.ts` — rows as the pipelines podujatia, skoly and ovzdusie-pm10 write them
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean
