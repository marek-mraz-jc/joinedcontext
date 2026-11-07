# Citizen map app

**For:** a resident on a phone who wants the nearest place that has what they need now.
**Shape:** a full-screen map with a search on top; tapping a place opens a bottom sheet on a
phone and a panel beside the map on a laptop. Read-only, public-ready.

## What it uses
- `useEntities` for the rows, `useFilters` with a `search` filter over the name.
- The template's `EntityMap` (points coloured by bikes available, the selected one highlighted)
  and `SearchBox`.
- `Header` from the SDK; the sheet is this app's own (`src/App.tsx`, `src/app.css`), because a
  bottom sheet is a phone pattern the SDK has no primitive for.
- `format` for every number, so a station that sends no count reads `—`, never `0`.

## What to copy
- The hero-map layout: `.app-map-screen` fills the window under the header, the map fills it.
- The sheet: a region with a heading, a close button, Escape closes it and focus returns to the
  place in the list the person came from.
- The list of results under the search for screen reader and keyboard users, who cannot pick a
  point on a canvas.

## Data
One type, `BikeHireDockingStation` (`model.linkml.yaml`): `name`, `location`,
`availableBikeNumber`, `freeSlotNumber`, `status`. Swap the type and the attributes for another
place-like type (stops, events, service points) and keep the layout.
