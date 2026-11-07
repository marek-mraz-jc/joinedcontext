# iCalendar (RFC 5545) feed of a view: a field mapping onto `VEVENT` (T-4487, T-4901)

Sources: RFC 5545 (iCalendar), Smart Data Models `dataModel.Event/Event`, the joinedcontext platform's API/02 §8b
(the Endpoint's `spec.representations.ical` mapping, EP-125, T-4900) and `schemas/surface-requirements.md`. The
crate is `crates/antares-ical`; it mounts at `/x/views/{view}/ical/v1/events.ics` (feature and surface name `ical`).
The user page is `docs/src/ical.md`.

## 1. Which Entities

The feed is one NGSI-LD Query Entities through the view, of the mapping's type (`Event` by default): the view's
types whose local name is that type (any @context's IRI ending in `/Event` or `#Event`); a view of every type reads
`local=true` and keeps the Entities whose type's local name is it. A view that shows no such type, or does not allow
`query_entities`, has no feed: `404`, the answer an unknown view gets. Entities are read under the core @context
with `options=sysAttrs`; an Attribute is found by its local name, so `startDate` and
`https://smartdatamodels.org/dataModel.Event/startDate` are the same source.

The caller's `q`, `scopeQ`, `georel`, `geometry`, `coordinates`, `geoproperty`, `id`, `idPattern` and `lang` are
passed to that Query Entities as they are, and `attrs` selects as it does there; `type` may name the mapping's type
alone. The view's narrowing and the Policy apply as on the NGSI-LD surface, so the feed of
`q=address[addressLocality]=="Zvolen"` holds the Entities the NGSI-LD answer to the same `q` holds. Any other
parameter, or one given twice, is `400`; a broken `q` is the NGSI-LD `400` itself, a refused read its `403`.

## 2. The mapping

The view carries the mapping as `representations.ical`, written by the Portal from the Endpoint's
`spec.representations.ical` (same shape: `type`, `preset`, `mapping`). None written is the `sdm-event` preset over
`Event`. A `preset` with a `mapping` is the preset with those fields replaced; a `mapping` without one is the whole
mapping. The broker refuses a mapping that does not hold whatever the model is (an unknown field or remap key, two
sources, a remap the field does not take, `UID` or `DTSTAMP` mapped, no `DTSTART` or `SUMMARY`, `DTEND` beside
`DURATION`, `CLASS` other than `PUBLIC`, a `STATUS` value other than `TENTATIVE`, `CONFIRMED` or `CANCELLED`, a
zone that is not IANA): a stored view with such a mapping serves `500` with a fixed text and no feed. The check
against the data model is the platform's, at propose.

| `VEVENT` property | RFC 5545 | `sdm-event` | Absent value |
|---|---|---|---|
| `UID` | 3.8.4.7 | the Entity `id`, fixed | never absent |
| `DTSTAMP` | 3.8.7.2 | the mapped `LAST-MODIFIED`, else the newest `modifiedAt` of the mapped attributes, else the Entity's (T-5208) | the time of the request |
| `DTSTART` | 3.8.2.4 | `startDate` | no `VEVENT`, counted (section 4) |
| `DTEND` | 3.8.2.2 | `endDate`, the last day (`endInclusive`) | no `DTEND` |
| `DURATION` | 3.8.2.5 | | no `DURATION` |
| `SUMMARY` | 3.8.1.12 | `name` | no `VEVENT`, counted |
| `DESCRIPTION` | 3.8.1.5 | `description` | absent |
| `LOCATION` | 3.8.1.7 | `address`: `streetAddress`, `postalCode`, `addressLocality`, `addressCountry`, comma-joined; or its text | absent |
| `GEO` | 3.8.1.6 | `location`, a `Point`: latitude `;` longitude | absent (another geometry too) |
| `URL` | 3.8.4.6 | `url` | absent |
| `CATEGORIES` | 3.8.1.2 | | absent |
| `STATUS` | 3.8.1.11 | (`schema-org`: `eventStatus`, mapped) | absent |
| `ORGANIZER` | 3.8.4.3 | | absent |
| `CLASS` | 3.8.1.3 | | absent |
| `LAST-MODIFIED` | 3.8.7.3 | as `DTSTAMP` unless mapped | absent |

The `schema-org` preset is `sdm-event` with `STATUS` from `eventStatus`: `EventScheduled`, `EventRescheduled` and
`EventMovedOnline` (bare or as `https://schema.org/…`) `CONFIRMED`, `EventPostponed` `TENTATIVE`, `EventCancelled`
`CANCELLED`, any other `CONFIRMED`.

A value is a Property's `value` (a typed literal's `@value`, a VocabProperty's `vocab`), a LanguageProperty's text
in the `language` asked (a BCP 47 tag, else the output's `lang`; then the same primary language, `@none`, the first tag), a GeoProperty's geometry, the default instance of a multi-instance
Attribute (else its first). A dotted path (`address.addressLocality`) reads a member of the value, else a
sub-attribute. `firstOf` takes the first path with a value, `join` the values of those that have one joined by
`separator` (`, `), `constant` a fixed text, `truncate` the first n characters, `map` a source value's calendar
value (`default` for any other; a value neither names is left out). `CATEGORIES` takes every text of a list.
`STATUS` is upper-cased and kept only as one of its three values. `ORGANIZER` takes a `mailto:` or http(s) URI, or
an e-mail address written as `mailto:`.

## 3. Dates

- `2026-10-20` is a date: `DTSTART;VALUE=DATE:20261020`. An `endInclusive` end names the last day and RFC 5545
  `DTEND` the day after it, so a fair of `2026-09-10` to `2026-09-12` ends `DTEND;VALUE=DATE:20260913`; without it a
  date end is written as it is. The presets read `endDate` as inclusive, as the feed did before mappings.
- An RFC 3339 date-time is written in UTC (`20261020T160000Z`). A date-time without offset is read in the field's
  `timezone`, else the output's (default `Europe/Bratislava`), and written in UTC: `2026-10-20T18:00:00` is
  `20261020T160000Z`, never a floating time (RFC 5545 3.3.5 form 1 would show 18:00 in every zone, T-5208). A start
  and an end of different value types are both read in that zone before the end takes the start's type. The feed
  carries the output zone's `VTIMEZONE` and `X-WR-TIMEZONE`. A wall time the zone skips is unreadable.
- `date: date` writes a day (an instant's day in the zone), `date: datetime` a day's midnight in the zone.
- `DURATION: {constant: PT1H}` is the length of an Event with a date-time start and no end; a mapped `DTEND` wins.
- `DTEND` takes the value type of `DTSTART` (3.8.2.2).
- `from` and `to` take a date or a date-time. An Event is in the feed when it starts before the end of `to` (a date
  `to` is inclusive) and has not ended before `from`. Bad or reversed bounds answer `400`.

## 4. Records left out

An Entity the mapping cannot render is no `VEVENT`: the feed never serves an invalid record. Each one increments
`antares_surface_records_skipped_total{surface="ical",reason}` (EP-126) and the response's
`antares-skipped-records` header counts them. The reasons: `missing-dtstart`, `missing-summary` (no value),
`unparsable-date` (a mapped date field holds no date), `unparsable-duration` (no RFC 5545 `dur-value`),
`dtend-not-after-dtstart` (an end before the start, or a date end not after a date start; an instant end equal to an
instant start is a zero-length event and is written without `DTEND`, RFC 5545 3.6.1), `geo-out-of-range` (a Point outside -90..90 / -180..180), `url-not-http` (not http(s) or
holding white space).

## 5. Output

Text is escaped (3.3.11: backslash, semicolon, comma, newline; other control characters dropped) and every content
line is folded at 75 octets between UTF-8 characters (3.1), each ending in CRLF. One `VEVENT` per Entity, so each
`UID` once. The calendar carries `VERSION:2.0`, `PRODID:-//Antares//antares-ical//EN`, `CALSCALE:GREGORIAN` and
`NAME`, `X-WR-CALNAME` (the output's title, else the view id), `REFRESH-INTERVAL` and the `VTIMEZONE`. A feed reads at most `RESIDUAL_SCAN` (10 000) Entities and answers `413` past that; the
window is applied after the read, so a large view is narrowed with `q`.

## 6. Preview

`POST /x/views/{view}/ical/v1/preview` with `{"mapping": <representations.ical>, "limit": 1..50}` (default 50, body
at most 16 KiB, no other member) renders a candidate mapping over the first `limit` Entities the request's subject
headers read through the stored view, with the feed's query parameters. The answer is
`{"read", "valid", "outside", "skipped": {reason: n}, "events": [the first 3 VEVENTs]}`. Nothing is stored and no
metric counts. Only a request with the broker's admin bearer token (`ANTARES_ADMIN_TOKEN_FILE`, the Portal's) is
served; any other is `403` before the view is looked up, and a broker without an admin token answers `403` to all.

## 7. Security

The feed is read only (`GET`, `HEAD`, `OPTIONS`; any other method `405`). Every read is the view's Query Entities
under the caller's subject, so a hidden type or Attribute is in no `VEVENT` and the Policy decides as for NGSI-LD.
A mapping is attribute names and the remaps above, never an expression or a template. The refusals carry fixed
messages that name nothing the view hides; a mapping that does not hold names no field to a feed caller.
