# Ovzdušie v Banskej Bystrici / Air quality in Banská Bystrica

One screen over the city's public space `banskabystrica-verejne`, where the EEA's hourly readings
of station SK0263A land (T-2781, T-2949): the measuring stations on a map, coloured by their
latest PM10 against the daily limit, and one day of history for the station a reader picks. It
reads and never writes, and it never asks anybody to sign in.


It sits in the SDK's `AppShell` (SDK-39) with the SDK's states. A station picked on the map is shown
with its day and opens in the shell's entity panel (SDK-40); each card has its Details button too.
The App is public and writes nothing, so the panel links the station to the Portal and offers no
Edit (AP-140). The tests exercise every control and hold the coverage gate of T-3373
(`sh scripts/app-coverage-run.sh banskabystrica-ovzdusie`); the e2e adds a station in the panel at
375 and 1440 px, light and dark.

## Why staleness is a band and not a footnote

A station that stops reporting keeps its last number. Drawn by that number alone, a station that
fell silent this morning at 9 µg/m³ is the greenest dot on the map, and a reader takes it for the
cleanest air in the city. So `bandOf` decides staleness **before** the thresholds: a reading older
than three hours is `stale`, in its own grey, with the words "staré meranie" on the card. The
number is still shown — it is real, it is just not about now.

The thresholds are Directive 2008/50/EC as `docs/Development/11` §2 quotes it: PM10 elevated from
35 µg/m³ and above the limit from 50 µg/m³. What is judged is the station's latest reading and not
a daily mean, and the screen says so.

## What it reads

One endpoint, one space, one type:

| what | where |
|---|---|
| space | `banskabystrica-verejne` of the project `banskabystrica` |
| endpoint | its own `app-banskabystrica-ovzdusie`, audience `public`, compiled into `grants/` |
| type | `AirQualityObserved` |
| attributes | `name`, `dateObserved`, `location`, `pm10`, `pm25`, `dataProvider` |
| operations | `queryEntity`, `retrieveEntity`, `retrieveTemporal` |

The pipelines `ovzdusie-pm10` and `ovzdusie-pm25` write one entity per station,
`…:eea-SK0263A`, each stream its own attribute, with the station's name and place from the EEA
station metadata. The space's public grant (`policy-verejne-read.yaml`) serves every attribute of
the type. The space `ovzdusie` is the conformance suite's: its seeded stations are test data and
this screen does not read them. The data is the EEA's (SHMÚ's readings, CC BY 4.0), and the
screen says so under the list.

**No login and no token.** The endpoint is public, so the bundle holds no credential and the
browser sends none; `App.test.tsx` asserts that no request carries an `Authorization` header
(AP-28). The endpoint is found in the served configuration by **space**, never by position.

## The map is a picture of the list

A canvas is not readable by a screen reader and cannot be reached by the keyboard, so the map is
`role="img"` with a name, and the same stations are a list beside it: name, band in words and in a
shape, both numbers, and when the reading was taken. Picking a station from the list or from the
map is the same action. A station that publishes no location is on the list with a line saying it
is not on the map, because leaving it off both would hide a station that exists.

The basemap is the keyless OpenStreetMap raster tile service with its attribution, so the screen
runs on a cluster with no map account and no key in any manifest.

## What is the application's own, and what is the publisher's

The published model (`bb-air-quality.linkml.yaml`) has no `name` slot, so a station has no name to
show. The screen calls it after the `{localId}` of its own URN — `station-1` reads "Stanica 1" —
rather than inventing a name no publisher stands behind. The bands and the staleness rule are the
application's own and are stated on the page.

## Building it

```sh
cd ui
pnpm install
pnpm typecheck && pnpm test && pnpm build
pnpm e2e
```

`pnpm e2e` serves the built bundle under `/apps/banskabystrica-ovzdusie/` with the static host's
Content Security Policy and the fixture endpoint, and holds the stations and a picked station to a
phone, a tablet, a laptop and a wall (375, 768, 1440, 2560 px): no sideways scroll, no overlapping
blocks, axe clean at WCAG 2.1 AA (T-2825).

`@joinedcontext/sdk` is linked from this repository, which is why `vite.config.ts` dedupes React.

## Related

- `docs/Development/10-banska-bystrica-contract.md` — the frozen contract: spaces, endpoints, ids
- `docs/Development/11-banska-bystrica-kpis.md` — the limits the bands come from
- T-2435 — this application; T-2432 — the tabular representation of the same endpoint
