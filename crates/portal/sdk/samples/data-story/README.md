# Public data story

**For:** the public, a newsroom, a council report: a page people read from top to bottom.
**Shape:** one readable column: a lede with the main number, sections that each make one claim
with a figure, the numbers behind each figure in a `<details>`, and how the numbers were made.

## What it uses
- `useEntities` for the readings; every number the text says comes from pure functions in
  `src/story.ts` (`readings`, `dailyMeans`, `stationMeans`, `summarise`), tested on their own.
- `Page` and `Header` from the SDK; the template's `ChartCard` and `EntityMap`.
- `format` and `currentTokens` for numbers and chart colours.

## What to copy
- Write the sentence from the data, never beside it: "On 3 of 14 days the city-wide mean was
  above…" is computed, and the wording changes when the count is 0.
- Each chart is a `<figure>` with a `<figcaption>` that says what it shows, and a table of the
  same numbers for a screen reader or a person who wants them.
- Say what was left out and why ("a reading without a station, a date or a value is left out,
  not guessed") and where the reference value comes from.
- Read-only: the app needs `queryEntity` on one type and nothing else, so its endpoint can be
  public.

## Data
`AirQualityObserved` (`model.linkml.yaml`): `stationName`, `pm25`, `dateObserved`, `location`.
The guideline is the WHO 2021 air quality guideline for PM2.5 over 24 hours (15 µg/m³).
