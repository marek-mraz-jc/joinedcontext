# BBSK events: source inventory

Who publishes events in Banskobystrický kraj, where, and in what format. Each row of `sources.csv`
was read by hand on 2026-10-20 (column `read_on`) with one polite request per page and the
User-Agent `joinedcontext-platform-events-inventory/1.0`. The pipelines of T-4896 are built from
this file; the rest of Slovakia is T-4899.

## Coverage

- All 24 towns of the kraj (Banská Bystrica, Banská Štiavnica, Brezno, Detva, Dudince, Fiľakovo,
  Hnúšťa, Hriňová, Jelšava, Kremnica, Krupina, Lučenec, Modrý Kameň, Nová Baňa, Poltár, Revúca,
  Rimavská Sobota, Sliač, Tisovec, Tornaľa, Veľký Krtíš, Žarnovica, Žiar nad Hronom, Zvolen).
  Each town's own website was opened and confirmed as the official town site. The Statistical
  Office register itself was not fetched; this list matches the kraj's 24 towns as the task names
  them.
- Three towns have moved off the obvious domain: Tisovec is `tisovec.com`, Tornaľa is
  `mestotornala.sk` and Veľký Krtíš is `velky-krtis.sk`. Dudince's official site is
  `dudince-mesto.sk`; `dudince.sk` is a Wix spa page.
- The region `bbsk.sk`, the regional tourism organisation (KOCR Stredné Slovensko,
  `centralslovakia.eu`), the tourism and culture sites the town pages link to (Visit Banská
  Bystrica, Región Banská Štiavnica, Supervulkán Štiavnica, visitkremnica.com, the Ticketware
  culture centres of Banská Štiavnica and Tornaľa, Dom kultúry Rimavská Sobota, MsKC Žiar), and
  the national catalogue (data.gov.sk now redirects to data.slovensko.sk).

## Decisions

| decision | rows | meaning |
|---|---|---|
| `pipeline` | 34 | a feed (REST JSON, iCal, RSS/Atom) or a CMS's HTML calendar (one template per CMS, T-5047 and T-5354) lists the events |
| `skip` | 11 | no events calendar, robots.txt refuses it, broken TLS, or a catalogue only (reason in the row) |

20 of the 24 towns have at least one source (feed or HTML); Kremnica's is a WEBY feed that was
empty on the read day. Hnúšťa, Modrý Kameň and Nová Baňa publish no dated events today. Žiar nad
Hronom's own calendar is closed by robots.txt and its culture centre's calendar loads by JavaScript.

## Formats that repeat: one Bento template each

| feed_kind | rows | sources | GPS in the source |
|---|---|---|---|
| `weby-feed` | 11 | WEBY PORTÁL (WEBY GROUP, Zvolen): Detva, Fiľakovo, Jelšava, Kremnica, Lučenec, Revúca, Rimavská Sobota, Tornaľa, Veľký Krtíš, Žarnovica, Zvolen | no |
| `lego-html` | 4 | Lego CMS: Dudince, Hriňová, Poltár, Tisovec | no |
| `eventphp-html` | 1 | event.php CMS: Brezno (visitkremnica.com and kino.mskcentrum.sk run the same CMS but fill the month by JavaScript: skip) | no |
| `tribe-rest` / `ical` | 2 | The Events Calendar: visitbanskabystrica.sk (REST), centralslovakia.eu (iCal; its robots.txt disallows /wp-json/) | no (venue has no geo_lat) |
| `ticketware-html` | 2 | Ticketware culture centres: Banská Štiavnica, Tornaľa | no |
| `wp-rest-sc` | 1 | Banská Bystrica (WP REST `/wp/v2/events`, also on data.gov.sk, CC-BY-4.0) | **yes**, 58 of 58 upcoming |
| `sc-rss` | 1 | Sliač, same smart-city theme as Banská Bystrica but REST closed (401): RSS `?post_type=event` + detail page | no: the page's static-map point is the town's default (2026-10-23) |
| `mec-rest` | 1 | Modern Events Calendar: banskastiavnica.travel (REST list + schema.org Event JSON-LD per page) | no (address only) |
| `krupina-html` | 1 | Krupina (ID-SK template) | no |

The WEBY template alone serves 11 towns. A WEBY feed carries only title, link and publication
date; the `/akcia/` detail page has the labelled fields `Dátum`, `Čas`, `Miesto` and
`Usporiadateľ`. Zvolen serves RSS 1.0 at `get_rss.php?id=3`; the other WEBY towns serve Atom at
`id=3_atom`. Six WEBY feeds were empty on 2026-10-20 but well formed, so their pipelines run and
report 0.

## GPS: what T-4896 needs

The owner rule (name, startDate and a GPS point required, no town centroids) means only
Banská Bystrica and Sliač give events a location straight from the source. Every other kept row
names its venue as text (`Miesto`, `Kde`, LOCATION, Place address). Those rows need the seeded
venue table with hand-checked coordinates, or their events are rejected.

## Terms and personal data

- Only public pages; no source needs a login. `robots.txt` was read per host with the
  standard parser, by user-agent group: `ziar.sk` disallows `/event.php` and `centralslovakia.eu`
  disallows `/wp-json/`. Neither path is used.
- Only Banská Bystrica states a licence (CC-BY-4.0 in its data.gov.sk record, which also flags
  personal data). Its feed holds the organiser's e-mail and phone: the pipeline drops both. No
  other site states a licence, so pipelines keep only the facts of an event (name, time, place,
  category, the source link and an image URL), never copy texts or image files, and attribute the
  source page.
- `bbsk.sk` sends its certificate without the GeoTrust intermediate, so curl and Go refuse it.
  The row stays `skip` until the region fixes the chain; TLS verification is never turned off.

## Re-check

`check-sources.py` re-fetches every kept row's `check_url` (robots.txt first, 1 request per
second) and checks that the format still parses: JSON shape, RSS/Atom XML, VCALENDAR, or the
row's `item_pattern` on an HTML list. Exit 1 when a kept row fails; an empty feed is only reported.

```sh
python3 seeds/bbsk-events/check-sources.py          # kept rows
python3 seeds/bbsk-events/check-sources.py --all    # skip rows too
```

## Related

- T-4895 events space and Event model, T-4896 pipelines, T-4897 dashboard, T-4899 Slovakia.
- docs Requirements PL-19, PL-47, PL-50, PL-72, DM-83; iCalendar SurfaceRequirement in
  platform `crates/jc-core/src/kinds/endpoint.rs`.

## Pipelines (T-4896)

`generate.py` writes one DataSource, Pipeline and Bento mapping per `pipeline` row whose feed kind has a template, into
`components/context-gateway/seed/bbsk/bbsk-{datasource,pipeline}-podujatia-<host>.yaml` and their lines in that folder's
`index.yaml`. Change a template, `sources.csv` or `venues.csv`, then run `python3 seeds/bbsk-events/generate.py`;
`tests/test_bbsk_events_pipelines.py` fails while the seed differs from what the script writes.

Every Pipeline writes through `bbsk-podujatia-zapis` (T-5033): audience `organization`, no `policyRef`, so the gateway
binds each pipeline's derived write Policy (`pl-<pipeline>`, PL-20) there. The public Endpoints `bbsk-podujatia` and
`bbsk-podujatia-slovensko` name a read `policyRef` and serve no write; a pipeline aimed at one gets 404 on every upsert.
`tests/test_pipeline_write_path.py` holds every seeded Pipeline to that rule.

| Template (`pipelines/`) | Feed kind | Sources | Where the date and the place come from |
|---|---|---|---|
| `wp-rest-sc.yaml` | wp-rest-sc | Banská Bystrica | Meta Box fields, GPS included |
| `tribe-rest.yaml` | tribe-rest | visitbanskabystrica.sk | The Events Calendar REST, venue GPS when set |
| `ical.yaml` | ical | centralslovakia.eu | DTSTART/DTEND, GEO, else LOCATION in the venue table |
| `weby-feed.yaml` | weby-feed | 11 towns | the feed's /akcia/ links, each page read once, 1 s apart |
| `mec-jsonld.yaml` | mec-rest | banskastiavnica.travel | the event page's schema.org Event JSON-LD, 1 s apart |

`common.yaml` closes every template: it keeps events whose end is now or later, the 100 soonest by start; finds the
point in the feed or in `venues.csv`; mints the id from name, start and town, so one event announced by two sources is
one entity; writes `expiresAt` at the event's end (start + 1 day without an end) and scope `/SK/BB/<town>`.

The runner holds `common.yaml` once, as the processor resource `events_tail` in
`components/pipeline-runner/streams/resources.yaml`, which `generate.py` writes between its BEGIN and END lines (T-5555).
A generated pipeline ends with a `mutation` that sets the source's settings as the structured metadata `events`
(venues, towns, organizer, grace, lease, the space as `env("JC_SPACE")`, which the Portal writes in per stream) and
`- resource: events_tail`. A change to the tail is one change of `resources.yaml` and reaches the runner when the
pipeline-runner release is applied, not with the seed.

- `venues.csv` holds venues checked by hand against OpenStreetMap (the object's id in `osm`), matched by the place
  text in lower case without parentheses and commas, per town; `regional=y` rows also serve the region's iCal feed.
  A venue nobody checked gives no point, never a town's centre; the event is kept without one (owner 2026-10-22).
  Not found as the named building on 2026-10-20: Kultúrny dom Veľký Krtíš, Kultúrny dom v Žarnovici, Dom kultúry
  Podborová (Zvolen), Športová hala Arena (Lučenec).
- A source with no licence stated gives facts only (name, time, place, link back), never its description;
  Banská Bystrica's feed is CC BY 4.0 and keeps the text, without organiser e-mail or phone.
- Each run is daily, staggered 00:00-02:34 UTC (01:00-05:00 in Bratislava, winter and summer); the Portal's
  run-now button starts one source.
- The HTML sources (T-5354) read machine-readable marks where the page has them (Sliač's microdata `<time datetime>`)
  and the printed date otherwise; a Ticketware date has no year and is the next one on or after yesterday.

## The national store (T-5047)

Since 2026-10-21 (owner: "a second store of all events across the whole Slovakia ... at least 100 sources ...
synchronized, up to date") `generate.py` writes two stores from the same inventory:

| store | project / space | sources | cadence | an event leaves |
|---|---|---|---|---|
| region | `bbsk` / `bbsk-podujatia` | the BBSK rows | daily, 00:00-02:59 UTC | at its end, or 52 h after the last run that listed it (T-5081: an event the model now refuses leaves too) |
| national | `slovensko` / `slovensko-podujatia` | every kept row | feeds every 3 h; Lego and WEBY calendars daily, spread over the day | a day after its end, or 8 h (daily: 52 h) after the last run that listed it |

The national store's pipelines write through `slovensko-podujatia-zapis` (audience organization, no policyRef);
people read `slovensko-podujatia` (NGSI-LD, OGC, iCal, CSV, OData, WebSocket, MCP; CKAN dataset "Podujatia na
Slovensku" in the `slovensko` catalogue). The lease ends an event its source dropped or cancelled, because
`spec.expiry` is refused where several pipelines write one space.

One template per source kind, a source is one DataSource + one Pipeline naming its kind:

| feed kind | templates (`pipelines/`) | what it reads |
|---|---|---|
| `lego-html` | `lego-html` | Lego CMS calendar list; each event page's `data-date-start`/`data-date-end` and `class="venue"`, 3 s apart (the CMS's robots.txt asks Crawl-Delay 3) |
| `lsvr-rest`, `eventon-rest`, `mec-rest`, `wp-rest-jsonld` | `wp-list` + `jsonld-pages` | WordPress REST list (50 newest), each event page's schema.org Event JSON-LD, 1 s apart |
| `wp-rss-jsonld` | `rss-list` + `jsonld-pages` | RSS/Atom of event posts, each post's JSON-LD |
| `jsonld` | `page-jsonld` | the events page's own JSON-LD; a page without any JSON-LD fails the run |
| `tribe-rest`, `ical`, `weby-feed`, `wp-rest-sc` | as above (T-4896) | |
| `sc-rss` | `rss-list` + `sc-event-page` | smart-city theme (Sliač): RSS of post_type=event, each page's microdata start and end; its static-map point only beside a place other than the town |
| `ticketware-html` | `ticketware-html` | Ticketware culture centre home page's `/event/<id>/` links, each page's shows (one record per show, date without a year) and hall address |
| `eventphp-html` | `eventphp-html` | event.php calendar's map array of page links (its points are the CMS default, never read), each page's dates with place and time |
| `krupina-html` | `krupina-html` | Krupina's ID-SK list `/podujatia/<id>`, each page's summary list (Kedy as printed, a range included; Kde as "[place, ]town"; Organizátor) |

The 182 rows marked `found by T-5047 from Wikidata P856` come from probing the website of every Slovak
municipality on Wikidata (2,859 sites, `P782` LAU code, region from its NUTS 3 prefix) and 254 museums,
galleries, theatres, libraries and culture centres: robots.txt first, then `/wp-json/` (The Events Calendar, LSVR,
EventON, MEC routes), `get_rss.php?id=3_atom` (WEBY) and the Lego `bundles/kalendar` calendar page. A row was
kept only when its feed parsed and listed at least one event on 2026-10-21. 89 WEBY sites serve an empty
`id=3` feed and stay out.

Decisions (worker-t5047, owner may override):

- GPS stays required with no town centroids (owner 2026-10-20): most new sources give a venue name only, so their
  events reach the space once `venues.csv` holds their venue; until then the run report lists them as refused.
- No conditional GET: a one-shot run keeps no state between runs to remember an ETag, and each run is one list
  request plus at most 50 page reads at 1-3 s.
- No paging: a store keeps the 100 soonest events per source; The Events Calendar answers upcoming events by start
  (`start_date=now`) and a WordPress list its newest posts, which hold the upcoming ones.
- Duplicates collapse by id (name, start, town hash, as T-4895's model states): one entity, the last source's link.
  A sameAs relation across sources within 200 m needs a cross-source pass over the space, which a per-source
  one-shot run cannot do.
- Free-text dates stay out: `wp-rest` (detail HTML), `wp-rss` and `joomla-rss` rows need one parser per site.

## Organizer and link (T-5081)

Both stores require name, start and `url` (an http(s) link); a record without one is refused into the run
report. GPS and `organizer` (schema:organizer: `name`, maybe `url`) are kept where known and never refuse an event
(owner 2026-10-22). The Portal stages each DataModel's JSON Schema, so the runner's stage holds records to the
whole model (T-5081; before that it checked id and type alone). `source` and `dataProvider` stay as
provenance: who published the feed is not who runs the event.

- The organizer comes from the feed where it names one: The Events Calendar's `organizer` and `website`,
  schema.org `organizer` (Organization or Person, the first of a list) on JSON-LD pages, WEBY's `Usporiadateľ`
  field, iCal `ORGANIZER;CN=`. Only the name and the web site are kept; an e-mail, a phone number or a
  `tel.`/`kontakt` label typed into the name is cut, and a `mailto:` is no web site.
- Else the body whose calendar it is (`generate.py organizer()`): the name in `org_type (Name)`, `Mesto {town}`
  for a town, `Obec {town}` for a municipality, with the inventory's site. A tourism organisation or a region
  announces other bodies' events, so its rows name none: their events without a feed organizer keep none.
- `url` is the event's page, else the source's events page (`events_url`). A link that is no absolute http(s)
  URL (a `javascript:` or relative link) counts as none.

Sources (2026-10-21): 143 BBSK candidates beyond T-5047's (museums, galleries, theatres, libraries, osvetové
strediská, culture centres, UMB/AU/TUZVO and their faculties, gymnáziá, OOCR, clubs and venues) were probed
with `/workspace/evidence/T-5081/probe.py` (robots.txt first, 1 s per host, the pipelines' User-Agent): The
Events Calendar REST, `/wp-json/` namespaces, `?ical=1`, WEBY `get_rss.php?id=3_atom`, Lego calendars and
schema.org Event JSON-LD on the home page and on up to four event-looking pages. 81 answered over https; 10
showed a machine-readable event list, 6 of them already kept. One new source is kept (Verejná knižnica
Mikuláša Kováča, WEBY, its address in the venue table); Šahy and Modrý Kameň list only past or 2028 items and
Teatro Tatro's EventON only past summer shows; `muzeum.sk` is the national museum in Bratislava. The other
museums, galleries, theatres, universities and schools publish HTML only (UMB's six faculty sites share one
CMS with `kalendar-udalosti`, TUZVO's five one with `kalendar-podujati`): those need an HTML kind per CMS.

## Universities and geocoded venues (T-5083)

Two HTML kinds read the university calendars that publish no feed, one template per CMS:

| feed kind | template | sources | what it reads |
|---|---|---|---|
| `umb-cms` | `umb-cms` | umb.sk, pdf.umb.sk, ff.umb.sk | the calendar's `<h3 class="title">` links under the list path (the 50 soonest), each event page's `eventTable`: Začiatok, Koniec, Miesto, Usporiadateľ, Mesto; 1 s apart |
| `drupal-kalendar` | `drupal-kalendar` | tuzvo.sk and its faculties lf, df, fee, ft | this month's list and the next (`?mini=YYYY-MM`), 10 s apart as robots.txt asks; start from the `content` attribute, Koniec, Miesto, Usporiadateľ, Zameranie; no event page |

fpvmv.umb.sk serves an iCal feed (`kalendar/iCalFeed`) and runs as an `ical` row. ff.umb.sk and the four TUZVO
faculties listed no event on 2026-10-21 but parse: their runs report 0 until they post one. Neither template reads
a person: UMB's contact rows and TUZVO's "Odborný gestor" stay on the page. UMB's organizer placeholder `Iné`
counts as none, so the university stands in. ef.umb.sk answers 403 and ftvs.umb.sk does not resolve.

Not built: the WordPress `podujatie` post types of stredoslovenskemuzeum.sk, horehronskemuzeum.sk and
muzeumkremnica.sk (no JSON-LD, dates in free text) and the WEBY `kalendar-podujati-0.html` pages; each needs its own
parser and none reaches five sources of one CMS.

`geocode-venues.py` fills `venues.csv` at inventory time, never from a pipeline: give it `town,place` rows (a run
report's refused events, or the venue texts read while probing a source). It asks Register adries
(ra.synchronix.sk, the official address register) first for each part holding a street and a number, then
Nominatim for the whole text, each part holding a house number and the first part, each with the town, one request a
second per host with the inventory User-Agent. A register hit counts in the same municipality or one of its boroughs,
on the same street and with the same orientačné number (the súpisné number where the address has none); its rows say
`register-adries` and name the register's address id in `osm` (`ra/<id>`). A Nominatim hit counts only when it is a
building, an amenity or an address in that town, and is written only when its `town,match,osm` line is in
`reviewed.csv`, the list of name matches a person opened on OpenStreetMap and accepted (T-5240: a third of the
pilot's name matches were another building). Every other hit is printed as a line to review. The rows checked
against OpenStreetMap by a person say `hand`. On 2026-10-21 it placed 21 of the 39 venue texts that UMB, FPVaMV and
TUZVO used in Banská Bystrica and Zvolen. The rest were online events, a room without an address ("TUZVO B8") or a
building OSM does not name; two of those (Aula Beliana, Aula FPVaMV UMB) were added by hand from the address the
same source writes elsewhere.

```sh
python3 seeds/bbsk-events/geocode-venues.py places.csv          # print the rows
python3 seeds/bbsk-events/geocode-venues.py places.csv --write  # append them to venues.csv
python3 seeds/bbsk-events/generate.py                           # then regenerate the seed
```

## GPS share and the organizer's address (T-5135)

`report.py` reads a store through its public NGSI-LD API and prints per source the events, how many carry a
point, and where each point came from: `feed`, a venues.csv row checked by a person (`hand`) or by
geocode-venues.py (`nominatim`), or the source's own address (`home`). Its last line is the store's share with
GPS, which the owner holds at >= 90 % (2026-10-22). `--places` writes the venues of the events without a point
as geocode-venues.py's input:

```sh
python3 seeds/bbsk-events/report.py --url https://api.dev.joinedcontext.com/api/endpoint/<slug>/ngsi-ld/v1 --places places.csv
python3 seeds/bbsk-events/geocode-venues.py places.csv     # review each row against OSM, then --write
python3 seeds/bbsk-events/generate.py
```

`homes.csv` names, per source, the venues.csv row of the body whose calendar it is: the library, the
university, the theatre, the town or village office. An event that names no place, or names only the town or
its organizer ("Obec Strelníky", "Ľubietová"), takes that address as its point; its `venue` stays as the source
wrote it. An event naming a place nobody placed keeps no point, and an aggregator (a tourism organisation or the
region) has no home. Decided (worker-t5135, owner may override): the town or village office counts as the
institution's address for a municipality's calendar, because the owner's rule names "the institution's
address" beside the venue table; options: home only for museums, libraries, theatres and universities.

geocode-venues.py refuses a `place` hit (Nominatim answered "Ľubietová" with the village's cadastral community)
and a map board, as it refuses a town. The rows of 2026-10-22 placed the venues dev's events named that
OpenStreetMap holds; the hit "Dom kultúry ŽSR" for "Dom kultúry Podborová" was dropped as another building.

Not placed on 2026-10-22, nothing in OpenStreetMap to point at: Poniky's parts, Spoločenský dom and Komunitné
centrum Predajná, the Kultúrny dom of Medzibrod, Žarnovica, Tuhár and Halič, MsKS Hriňová's halls, the Obecný
úrad of Hontianske Moravce, Športová hala Arena (Lučenec), Baňa Starovšechsvätých, and online or "see the poster"
places. Each one needs a person to set its point from the building on the map (`checked_by` `hand`).

Kinds not built, refuted on 2026-10-22: WEBY's `kalendar-podujati-0.html` is an editor page holding PDF and
image posters (filakovo.sk, jelsava.sk, revuca.sk), not a list; stredoslovenskemuzeum.sk and muzeumkremnica.sk
have no `podujatie` post type, and horehronskemuzeum.sk writes the date only in the post's prose or poster.
Five municipal waste collection calendars (vhkalinka.eu and four outside the region) listed pickups as events:
they are `skip` now.

## Dates, notices, duplicates and office points (T-5215)

Measured on dev on 2026-10-22 through the national Endpoint (`report.py`, every Event, keyValues): 1,680 events from 175
sources, 200 with GPS (12 %), 711 without a start (42 %). What changed:

- **Dates.** A record without a readable start is refused, never stored without a date (owner 2026-10-22); the model
  check alone had let 711 through. EventON writes its offset with a one-digit hour and no seconds
  (`2026-11-14T10:00+2:00`): `jsonld-pages` now reads it (194 of 197 EventON events had no start). LSVR Events
  (TownPress theme) leaves its JSON-LD dates empty and prints them in the page's `post__info` list ("Dátum" and
  "Čas", or "Začiatok" and "Koniec", as `28. augusta 2026` or `7. 12. 2026`): `jsonld-pages` reads that list when the
  JSON-LD has no start (451 of 492 LSVR events had none). Visit Bratislava's pages carry no Event JSON-LD at all; its
  events stay refused until a template reads its HTML.
- **Notices.** `common.yaml` refuses a record whose name is a notice, for every source kind: waste collection
  (vývoz, odvoz, zvoz, zber of papier, plasty, sklo, odpad ...), a council session (zasadnutie, zastupiteľstvo),
  VZN, zámer, oznámenie, elections and referenda, power cuts (prerušenie dodávky/distribúcie, odstávka), "Nová webová
  stránka", and a hall's private booking (Svadba, Rodinná oslava, Kar). Decided (worker-t5215, owner may override):
  council sessions and hall bookings count as notices, because the analyst's audit of 2026-10-22 counted them so
  and a booked wedding is no public event; options: keep council sessions as public meetings.
- **Duplicates.** The id hashes the name in lower case with only its letters and digits, the start to the hour in UTC
  (or the day), and the town: "Kintsugi – japonské umenie" and "Kintsugi: japonské umenie" at 16:30, and "Čarovný
  kľúčik" at 16:35 and 16:36, are one entity each. Two sessions of one name on one day at different hours stay two.
  Every id changes once with this key: the old entities leave by their lease (8 h for the 3-hourly feeds, 52 h for
  the daily calendars and the region store).
- **Office points.** `offices.py` asks Overpass, in one query, for each municipal source's boundary (by name in
  Slovakia; of several of one name only the one whose OSM `website` is the source's host) and the `amenity=townhall`
  and `amenity=community_centre` objects inside it. The office becomes the source's home (`homes.csv`), the point of
  an event that names no place or only the municipality ("Obec", "Celá obec", "Obecný úrad", "Obec X"); a single
  community centre becomes the row of "Kultúrny dom" and its usual spellings. Two culture houses, no office, or an
  object outside Slovakia's box give no row; never a village centre. Nominatim answered the swarm's address with 429
  on 2026-10-22, so offices.py does not use it.

```sh
python3 seeds/bbsk-events/offices.py           # print the rows and every OSM name it read, per municipality
python3 seeds/bbsk-events/offices.py --write   # append them to venues.csv and homes.csv
python3 seeds/bbsk-events/generate.py
```

`report.py` follows the API's `Link: rel="next"` with a parser that keeps the commas inside its target
(`attrs=source,location,venue`); it stopped after the first page before.

## Every event a point, labelled by precision (T-5240)

The owner holds the share of upcoming events with GPS at >= 90 % (2026-10-21). The common tail takes an event's point
from, in order: the feed (`locationPrecision` `source`), a venues.csv row (`building`), the organizer's address of
homes.csv for an event naming no place (`office`), then the event's own town in `towns.csv` (`office` or `centre`).
An event whose town the table does not know keeps no point. A map draws `office` and `centre` as approximate.
Decided (supervisor, owner may override): office and centre points are written and labelled approximate.

`towns.csv` holds one row per municipality the events name: its `amenity=townhall` (`office`) or, where OpenStreetMap
maps none, its own `place` node (`centre`). `towns.py` builds it with one Overpass query per kraj; a name two
municipalities of a kraj share counts only for the source whose web site names one (`unique` n). Košice and
Bratislava are pinned by hand to Stará radnica (Hlavná 59) and the Primaciálny palác magistrát: the townhall rule
picked a borough office in Košice for every venue of the geocode pilot. A municipality's calendar falls back to its
own town only; a museum, a theatre or a tourism body to every town of its region the table names once.

The common tail also spells out the calendars' abbreviations in the match key (KD, DK, MsKS, OcÚ, OÚ, MsÚ, MÚ:
generate.py `ABBREVIATIONS`), reads a locality naming a district or a kraj ("okres Myjava") as the source's town, and
moves a postcode glued to the house number ("Bečov 96/6976 63") or written after it to `postalCode`.

```sh
python3 seeds/bbsk-events/report.py --url <store>/ngsi-ld/v1 --places places.csv --localities localities.csv
python3 seeds/bbsk-events/geocode-venues.py places.csv      # register rows; review the printed name hits
python3 seeds/bbsk-events/towns.py --localities localities.csv   # read each printed name, then --write
python3 seeds/bbsk-events/generate.py
```
