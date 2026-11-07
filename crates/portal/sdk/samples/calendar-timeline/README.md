# Calendar and timeline

**For:** residents looking for what is on, and staff planning around it.
**Shape:** three tabs over the same events (a month, a week, a timeline), the chosen event read
beside them (under them on a phone).

## What it uses
- `useEntities("Event", …, { all: true })` for every event; placing them in time is pure functions
  in `src/calendar.ts` (weeks from Monday, whole-week months, multi-day events on each of their
  days, coming and past), tested on their own.
- `Page`, `Header`, `Split` and `Tabs` from the SDK; the template's `EntityDetail` for the chosen
  event.

## What to copy
- A month is a real `<table>`: each day is a button named by its date and its count
  ("Friday, 25 September, 2 events"), today carries `aria-current="date"`, the chosen day
  `aria-pressed`. On a narrow screen the cells show a count and the chosen day's events are
  listed under the month; titles move into the cells only where a cell has room (a container
  query at 44rem).
- The week is a list of seven days that becomes seven columns at 60rem.
- The timeline puts what is running now and what is coming first, folds the past into a
  `<details>`, and says how many events have no date instead of dropping them silently.
- Previous / Today / Next, with the period's name in a live heading.
- Days are local calendar days: an evening event stays on its evening.

## Data
`Event` (`model.linkml.yaml`): `name`, `category`, `venue`, `description`, `startDate`,
`endDate`. An end before its start is read as no end; an event without a readable start is
counted, not placed.
