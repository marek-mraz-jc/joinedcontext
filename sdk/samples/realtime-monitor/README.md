# Real-time monitor

**For:** operators watching sensors: air quality, water levels, traffic counters, anything that
reports every few minutes and matters when it crosses a level.
**Shape:** a tile per station (latest value, state, sparkline of the last hour) and a log of
crossings beside the tiles (under them on a phone), with the newest crossing announced.

## What it uses
- `useEntities(type, undefined, { refreshMs })` polls the current readings (10 s here; the SDK
  never polls faster than 2 s). Pausing turns `refreshMs` off; "Refresh now" is `reload()`.
- `useClient().temporal.list` once for the last hour, so each sparkline has a shape before the
  first poll; each poll then adds its newer points (`record`, `merge` in `src/monitor.ts`).
- `Page`, `Header`, `Split`, `Grid`, `Card` from the SDK; the sparkline is a plain SVG.

## What to copy
- Tiles keep their order (by name). A live page that re-sorts on every poll moves the tile a
  person is reading.
- A crossing is announced once, when a station goes over the level, not on every poll while it
  stays there (`crossings`), and nothing is announced for what was already over on arrival.
- A station without a reading for 15 minutes shows "No reading for 40 min", never its last value
  as if it were current.
- Every state is a word and a mark (▲ over, ● normal, ◌ silent); the sparkline carries its range
  in words for a screen reader.
- Updates can be paused (WCAG 2.2.2).

## Data
`AirQualityObserved` (`model.linkml.yaml`): `stationName`, `pm25`, `no2`, `dateObserved`, one
entity per station updated in place; its history through the endpoint's temporal read
(`queryTemporal`). The alert level (25 µg/m³) is the operator's own setting in `src/monitor.ts`,
not a health limit.
