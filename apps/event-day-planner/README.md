# My day of events

For a visitor to Helsinki: which of the day's events to see, in which order, with the walk
between them, in Finnish, Swedish or English (the events' own names as the Portal's language asks):

- the day (today, or a later one) and a search;
- the plan on opening: with nothing picked, a suggested day of events that follow one another on
  foot; with events picked, those events in the order that fits most of them, each with its time,
  the walk from the last one, and a mark when it starts before the visitor can get there or is out
  of reach; the events that take place at the same time are named;
- the plan's stops on the map, with the walk between them;
- the day's events, soonest first, each with a box to add it to the day (a cancelled one is shown
  and cannot be picked), its place, its source and what it is about;
- how many events start each hour; a click on a bar lists that hour's;
- the day as a calendar file (.ics) to save.

An event's name in the plan or the list, or its stop on the map, opens it in the SDK shell's
entity panel, in the page's language, with a link to the event in the Portal; the App writes
never writes the city's data, so the panel offers no Edit. The language is the shell's switch,
Suomi, English or Svenska.

The day, the search, the hour and the picks are kept in the address (`?day=`, `?q=`, `?hour=`,
`?pick=`, `?lang=`), so a reload or a shared link shows the same day.

## How the day is planned

The planner is Rust, in `wasm/`, compiled to WebAssembly and run in the reader's browser in a Web
Worker, so the page never stalls:

- an event of up to three hours is sat through from its start; a longer one (an exhibition, a
  fair) is a place to drop by for an hour while it is open;
- the walk between two places is the great-circle distance at 5 km/h; an event with no place adds
  no walk and says so;
- up to eight picked events every order is tried, more by start time then 2-opt; the best order
  fits the most events, then is the least late, then walks the least; at most 20 are planned;
- with nothing picked, the suggested day takes the events by earliest end, each reachable before
  it starts, at most four;
- the calendar file is RFC 5545: UTC times, escaped text, lines folded at 75 octets.

## Sharing a day

**Share this day** sends the day and the picked events' ids to the App's server, a WebAssembly
component on the platform's shared host (`server/`, `kind: wasm`, ADR-N-044). The server reads those
events again from the gateway, as the visitor (an anonymous visitor reads what the public role may),
plans the day with the same Rust (`../wasm`, without the browser's bindings) and keeps it in the
App's own schema (`migrations/`) under a code of 12 letters and digits; the calendar file goes
under the App's own prefix, `shares/{code}.ics`. The visitor gets the link, `?share={code}`, to copy,
and the calendar file of what was shared, downloaded through a URL valid for two minutes.

Opening a link puts the shared day and picks into the address and drops the code, so the page then
plans that day from the data of the moment and the visitor's own changes stay theirs. A shared day is
cleared a week after its date, a few at each new share.

| Route | What it does |
|---|---|
| `POST /api/itineraries` `{day, ids, lang}` | the day planned from the gateway and kept; its code |
| `GET /api/itineraries/{code}` | the shared day, its picks and its plan |
| `GET /api/itineraries/{code}/ics` | a URL the calendar file downloads from |

## Roles and history

The server decides from the signed-in person the platform names (`identity::caller()`, API/07),
never from anything the page sends:

- anyone, signed in or not, plans and shares a day and sees the **featured events**;
- a **viewer** finds the days they shared while signed in under **My days**, opens one again and
  deletes it with its calendar file; a day of someone else's answers as if it did not exist;
- a **steward** features an event of the day from the list (**Feature**), writes a note for it
  and takes it back; every visitor sees the featured events above the list and picks them into
  their day like any other.

A feature, a note, a removal and a deleted day show at once and go back when the server refuses.
Every change writes a row into the App's `app_audit` table in the same statement, so a refused or
failed change leaves none; the **History** page reads it: a viewer their own rows, a steward
everyone's.

| Route | Who | What it does |
|---|---|---|
| `GET /api/me` | anyone | the caller's name and roles, `null` when signed out |
| `GET /api/days` | signed in | the latest 50 days the caller shared |
| `DELETE /api/days/{id}` | the sharer | the day and its calendar file gone |
| `GET /api/featured` | anyone | the featured events, at most 50 |
| `POST /api/featured` `{eventId, note}` | steward | an event featured |
| `PUT /api/featured/{id}` `{note}` | steward | its note changed |
| `DELETE /api/featured/{id}` | steward | no longer featured |
| `GET /api/history` | signed in | the newest 100 changes (a viewer: their own) |

`server/host-test.json` plays every route on the real host as an anonymous visitor, a viewer,
another viewer and a steward, and counts the rows after each refused write.

## Data

One data need on the Context Space `helsinki`: `Event` with `name`, `description`, `startDate`,
`endDate`, `eventStatus`, `address`, `location` and `source`, the Linked Events registers the
events pipeline writes. Only events that have not ended are read. The app writes nothing to the
Context Space: shared days, featured events and the history are kept in its own schema.

## Run the tests

Needs Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.129, the version
`wasm/Cargo.lock` pins and the build lane binds with.

```sh
pnpm install
pnpm wasm          # cargo test of wasm/, then the module into wasm/pkg (the lane runs builder/build-wasm.sh)
pnpm test          # vitest, with the real WebAssembly module
pnpm build         # the bundle the build lane publishes
pnpm e2e           # the built bundle in Chromium at four widths, light and dark (needs `pnpm build`)
(cd server && cargo test && cargo build --release --target wasm32-wasip2)   # the server component
```

`server/host-test.json` is the server component's scenario on the real host: the portal
repository's `tests/wasm-apps` (`tests/scenarios.rs`, ci-full) builds the component, runs
`migrations/` twice as the reconciler does, and plays the scenario against Postgres, RustFS and a
mock of the App's own Endpoint, with a second App that must see nothing.
