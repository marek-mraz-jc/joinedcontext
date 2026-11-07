# iCalendar feed of a view

An `ical` build (`cargo build -p antares-broker --features ical`) serves the
Events of a [view](views.md) as iCalendar (RFC 5545, RFC 7986) feeds a calendar
application subscribes to, one per named output the view lists:

```text
GET /x/views/{view}/ical/v1/{output}.ics
GET /x/views/{view}/ical/v1/kultura.ics?from=2026-10-01&to=2026-10-31
GET /x/views/{view}/ical/v1/vsetky.ics?q=address[addressLocality]=="Zvolen"
GET /x/views/{view}/ical/v1/events.ics   (deprecated: the output `events`, else the first)
```

Mount it with `ANTARES_API_SURFACES=admin,views,ical`. The feed is one NGSI-LD
Query Entities through the view: `q`, `scopeQ`, `georel`, `geometry`,
`coordinates`, `geoproperty`, `id`, `idPattern`, `lang` and `attrs` mean what
they mean there, and `type` may name the mapped type alone. `from` and `to`
take a date or an RFC 3339 date-time; a date `to` is inclusive. Any other
parameter is `400`. The answer is `text/calendar; charset=utf-8`.

Calendars are off by default (owner 2026-10-22T09:25Z): a view serves one only
when its `representations.ical` lists outputs, at most 20, as the platform's
Endpoint writes them (T-5207, owner 2026-10-22T09:30Z and 09:40Z):

```json
{"ical": [{"name": "kultura", "version": 1,
           "title": {"sk": "Kultúra v kraji", "en": "Culture in the region"},
           "description": {"sk": "Koncerty, divadlo, výstavy"},
           "sources": [{"type": "Event", "preset": "sdm-event",
                        "mapping": {"LAST-MODIFIED": "dateModified"}},
                       {"type": "CouncilMeeting"}],
           "filter": {"q": "eventCategory==\"kultura\""},
           "lang": "sk", "timezone": "Europe/Bratislava",
           "sort": "start", "limit": 500}]}
```

`name` is the URL slug (`[a-z0-9-]`, 1 to 40, not `preview`). Each source maps
one type with a preset or its own `mapping`; a preset serves `Event`,
`CouncilMeeting` and `OfficeHours` alone, another type maps `DTSTART` and
`SUMMARY` itself. `filter` (`q`, `scopeQ`, a geo-query) narrows the view and
never widens it: its `q` and `scopeQ` are AND-ed with a caller's, and a caller
cannot replace its geo-query (`400`). `lang` picks a LanguageProperty's text
(BCP 47; then `@none`, then the first tag). `timezone` (default
`Europe/Bratislava`) is the zone a time without offset is read in; the feed
writes every instant in UTC, never a floating time, and carries that zone's
`VTIMEZONE`, so an empty calendar is still a valid iCalendar object. `sort`
(`start`, `-start`) and `limit` (1 to 10 000) keep the first Events by start.
The calendar carries `NAME` and `DESCRIPTION` per title language,
`X-WR-CALNAME`, `REFRESH-INTERVAL;VALUE=DURATION:PT6H` and a strong `ETag`;
`If-None-Match` with it answers `304`. `DTSTAMP` is the mapped `LAST-MODIFIED`,
else the newest `modifiedAt` of the mapped attributes, else the Entity's, so a
re-ingest that rewrites the same values leaves the feed byte for byte when the
source carries its own modification date. An output's `version` is the
contract version, `1` when absent (T-5207): an output of version 1 is read
strictly, and one of a newer version (a Portal released before the broker) keeps
its name, texts, filter and settings and serves each source's type over its
preset with a log warning, never a `500`. A view without the list, an unknown
output, or a view that shows none of the types or does not allow
`query_entities`, answers `404`. An Entity the mapping cannot render (no start or title, an
unreadable date, an end before its start, a bad URL or point) is left out,
never served invalid, and counted in
`antares_surface_records_skipped_total{surface="ical",reason}` and the
`antares-skipped-records` header. `HEAD` answers like `GET` without the body;
every other method but `OPTIONS` is `405` with `Allow`.

`POST /x/views/{view}/ical/v1/preview` renders a candidate mapping over the
first 50 Entities for the Portal's Endpoint form; it answers the broker's
admin bearer token alone.

The field mapping, the remaps and their reasons are
`docs/analysis/ical-mapping.md` (T-4487, T-4901).
