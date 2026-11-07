# Podujatia v Banskobystrickom kraji / Events in the Banská Bystrica Region

The events dashboard of T-4897: every upcoming event the pipelines of T-4896 collect from the web
sites of the region's towns, municipalities and bodies, on a map, in a list and in a month
calendar. It reads the space `bbsk-podujatia` (T-4895) through the App's own public endpoint, whose
Policy keeps the region's scope `/SK/BB/#`; it never writes and never asks anybody to sign in.

- Filters: town, category, a date range (an event is kept on any of its days) and a search over
  name, venue, town, category and organiser, without diacritics. A `From` after `To` is said, not
  silently empty.
- "Add to my calendar": the endpoint's iCalendar feed (EP-125) for the whole region, and, when dates
  are set, the same feed with `from`/`to`. The feed serves no other filter yet (T-4901 brings NGSI-LD
  `q` to it), so a town, category or search never goes into the link; the card says so. Each link
  can be copied or opened as `webcal://`. On `{name}.apps.{domain}` the link names `api.{domain}`.
- The organiser's page of an event is a link only for an `http(s)` address; anything else is left out.
- Every day is a day in Slovakia (Europe/Bratislava). A picked event opens in the SDK's entity panel.

## Files

- `ui/src/events.ts` — rows as events, filters, the month grid, the calendar link; `events.test.ts`
- `ui/src/App.tsx` — the shell, filters, map, list, month and subscription, tested in `App.test.tsx`
  over `ui/src/fixtures/events.ts` (fourteen towns): every control exercised, the coverage gate of
  T-3373 held (`sh scripts/app-coverage-run.sh bbsk-podujatia`)
- `ui/e2e/journey.spec.ts` — open, filter Zvolen, subscribe; every state at 1440 and 400 px without
  overlap or sideways scroll, kept as screenshots in the test output
