# Tříděný odpad Prahy / Prague recycling

The staff operations app of T-2786: a waste-collection desk. Every sensor container of the city's
public space `praha-mesto` with its kind of waste, how full it was at its sensor's last reading
(0 empty to 1 full, shown as a percentage), how long ago that reading was, and the sorting isle it
stands at. Staff sign in: the App is opened by its `viewer` role, which its default group
`praha-odpad-viewer` holds (T-2686, AP-118). It reads and never writes.

It runs in the SDK's `AppShell` (SDK-39). A container's code opens it in the SDK's entity panel with
its attributes and a link to it in the Portal; the viewer role grants no write, so there is no Edit
(SDK-40).

- A reading is only as current as its time: the age is shown beside every fill level; a fill outside
  0–1 is no reading, and a time in the future (the sensor's clock) is an age of 0, never negative.
- The fullest and the longest-unread tenth are judged against the city's own containers
  (ceil(n / 10) of them), marked in words beside the number. There is no norm in this app.
- Isle names are read by id, in chunks, for the isles the containers name, never every isle of the
  city; an isle that cannot be named says so.
- The CSV is the table as shown, the kind and the isle in words, UTF-8 with a BOM; a cell a
  spreadsheet would run as a formula is prefixed so it never executes.

## Files

- `ui/src/containers.ts` — readings, totals, the tenth, sorting and the CSV; `containers.test.ts`
- `ui/src/App.tsx` — the desk, tested in `App.test.tsx`
- `ui/e2e/responsive.spec.ts` — 375, 768, 1440 and 2560 px: no sideways scroll, no overlap, axe clean
