# Slovak events: inventory beyond BBSK

Who publishes events outside Banskobystrický kraj in a standard machine-readable feed, and in
which format. `sources-sk.csv` has the same columns and decision values as `sources.csv`
(T-4894, BBSK) and one row per organisation; together the two files are the national inventory
that T-4896 builds pipelines from and T-4898 publishes. Every row was read on 2026-10-20
(column `read_on`) with the User-Agent
`joinedcontext-platform-events-inventory/1.0 (+https://dev.joinedcontext.com; open data inventory, Slovakia)`,
robots.txt read first per host and at most one request per second per host.

## Totals

| | organisations | `pipeline` | `later` | `skip` |
|---|---|---|---|---|
| BBSK (`sources.csv`, T-4894, T-5081, T-5354) | 45 | 34 | 0 | 11 |
| rest of Slovakia (`sources-sk.csv`) | 109 | 86 | 20 | 3 |
| **Slovakia** | **154** | **120** | **20** | **14** |

102 organisations have a working standard feed with dated events and the decision `pipeline`.
The 20 `later` rows also have a working feed: 10 parse but listed no upcoming event on the read
day, and 10 list posts in which no start date was found in the feed or in 3 sampled event pages.

## By region (`pipeline` rows)

| region | towns | villages | other organisations | total |
|---|---|---|---|---|
| BBSK | | | | 16 |
| Bratislavský (BA) | 2 | 2 | 11 | 15 |
| Trnavský (TT) | 0 | 10 | 10 | 20 |
| Trenčiansky (TN) | 1 | 2 | 2 | 5 |
| Nitriansky (NR) | 1 | 2 | 6 | 9 |
| Žilinský (ZA) | 2 | 3 | 4 | 9 |
| Prešovský (PO) | 0 | 9 | 8 | 17 |
| Košický (KE) | 0 | 2 | 9 | 11 |

"Other organisations" outside BBSK: 13 culture centres, 11 tourism organisations (OOCR/KOCR),
10 museums, 4 libraries, 3 theatres, 3 self-governing regions (Trnavský, Prešovský, Košický),
2 universities, 2 festivals, 1 gallery, 1 exhibition venue.

## By feed kind (`pipeline` rows, both files)

| feed_kind | rows | regions | what it is | dates and place |
|---|---|---|---|---|
| `wp-rest` | 31 | all 7 | WordPress custom event post type, `/wp-json/wp/v2/<type>` | meta fields (Senec, Žilina, Ružomberok: `iq_dateStart`, `iq_coordinatesLat`, …) or the event page |
| `lsvr-rest` | 17 | all 7 | LSVR Townpress theme, `lsvr_event` | event page: JSON-LD Event, plus labelled `Dátum`/`Čas`/place when JSON-LD `startDate` is empty (recurring events) |
| `tribe-rest` | 13 | 6 + BBSK | The Events Calendar, `/wp-json/tribe/events/v1/events` | in the JSON (`start_date`, `end_date`, `venue`) |
| `wp-rss` | 12 | 6 + BBSK | RSS of an events category or post type | event page |
| `weby-feed` | 11 | BBSK only | WEBY PORTÁL `get_rss.php?id=3_atom` | event page (`Dátum`, `Čas`, `Miesto`) |
| `joomla-rss` | 7 | NR, ZA, PO, KE | Joomla category feed `?format=feed&type=rss` | event page |
| `eventon-rest` | 4 | KE | EventON `ajde_events` | event page JSON-LD Event |
| `mec-rest` | 2 | TT + BBSK | Modern Events Calendar `mec-events` | event page JSON-LD Event |
| `ical` | 2 | KE + BBSK | iCalendar (`?ical=1` of The Events Calendar) | DTSTART, DTEND, LOCATION |
| `jsonld` | 2 | BA, TT | schema.org Event JSON-LD on the events page itself | `startDate`, `location` |
| `wp-rest-sc` | 1 | BBSK | Banská Bystrica smart-city theme | meta_box |

Three templates cover 100 of the 102 (all but the two `jsonld` pages): WordPress REST (`wp-rest` and the REST list + event-page
read of `lsvr-rest`, `mec-rest`, `eventon-rest`), The Events Calendar (`tribe-rest`, `ical`) and
RSS/Atom + event page (`wp-rss`, `weby-feed`, `joomla-rss`). The Malacky, Prievidza and Komárno
town sites (and Dom kultúry Komárno) run one vendor theme: an `event` post type without meta,
with the date as text such as `6. nov 2026 od 17:00`, or `2026. október 25.` on Hungarian pages.

## GPS: what T-4896 needs

`geo = y` means the source has a coordinate field or a map point in the event page; it does not
say how many events fill it (Žilina's `iq_coordinatesLat` was empty in the sample). Ten rows
outside BBSK have one: the town sites of Senec, Žilina, Ružomberok, Malacky and Komárno, Dom
kultúry Komárno, Visit Bratislava, Visit Poprad, ÚĽUV and UPJŠ. No Events Calendar site fills
the venue's `geo_lat`. Every other row names the venue as text, so the owner rule (name,
startDate and a GPS point, no town centroids) needs the seeded venue table, as in BBSK.

## How the organisations were found

1. A hand list: the 117 towns outside BBSK, the 7 other self-governing regions, and about 90
   tourism organisations, theatres, galleries, museums, culture centres, libraries and
   universities at their known or likely domains (227 sites).
2. 226 more likely domains of city districts, culture centres, festivals and OOCRs; 113 did not
   answer and were dropped.
3. Links to culture, tourism, museum, library and cinema sites harvested from the homepages of
   1 and 2 (122 new sites).
4. The official website (Wikidata P856) of every Slovak municipality with a LAU code (P782,
   the Statistical Office code that gives the region): 2,230 sites outside BBSK.
5. Wikidata cultural institutions in Slovakia with a website (theatres, museums, galleries,
   libraries, culture centres, festivals, universities): 105 new sites.

Each site was probed in the task's order: `/wp-json/` namespaces and
`/wp-json/tribe/events/v1/events`, `/wp-json/wp/v2/types` for event post types, iCal links,
RSS/Atom links, then schema.org Event JSON-LD on the homepage and the events page. The chosen
feed was read once more (up to 3 event pages for list feeds) to fill the field columns.
The Statistical Office register was not fetched directly; its LAU codes came through Wikidata.

Probe scripts and raw results are not committed: they wrote JSON lines to a scratch directory,
and the CSV plus `check-sources-sk.py` is what a later reader re-runs.

## What the probes ruled out

- Lego CMS: 594 of the 2,230 municipality sites (and about 30 towns, e.g. Skalica, Holíč,
  Galanta, Sereď, Myjava, Snina) run it. Its event calendar is HTML only; `?rss=200` is the news
  feed. One HTML template (the `lego-html` kind of `sources.csv`) would reach the most
  municipalities of any single source, but it is not a standard feed and stays out of this
  file.
- WEBY PORTÁL, the most common feed in BBSK, did not show up in the 2,230 municipality sites
  outside BBSK.
- The probe found no standard events feed on the sites of Bratislava, Košice, Prešov, Trnava,
  Trenčín, Poprad or Martin (Nitra's `event` type is `later`); their own calendars are HTML. Trebišov's Google Calendar iCal is refused by `calendar.google.com`
  robots.txt for our agent (`skip`).
- 10 feeds (9 of them The Events Calendar) answered with 0 upcoming events (`later`): the feed
  works, so a pipeline can start as soon as they post.

## Terms and personal data

Only public pages; no source needs a login. No row states a licence on its feed; pipelines keep
only the facts of an event (name, time, place, category, source link, image URL) and attribute
the source page, as in `README.md`. Organiser e-mail and phone are not taken even where an event
page shows them.

## Re-check

`check-sources-sk.py` runs `check-sources.py` over `sources-sk.csv` and adds parsers for the
kinds found only outside BBSK (`wp-rest`, `eventon-rest`, `lsvr-rest`, `joomla-rss`, `jsonld`).

```sh
python3 seeds/bbsk-events/check-sources-sk.py          # kept rows
python3 seeds/bbsk-events/check-sources-sk.py --all    # later and skip rows too
python3 seeds/bbsk-events/check-sources-sk.py --selftest  # the jsonld parser, offline
```

## Related

- `README.md` and `sources.csv` (T-4894, BBSK); T-4895 events space, T-4896 pipelines, T-4898
  national dataset with the BBSK view.
