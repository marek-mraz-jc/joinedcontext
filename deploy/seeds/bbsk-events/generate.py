#!/usr/bin/env python3
"""Write one DataSource, Pipeline and mapping per kept events source and store (T-4896, T-5047).

Reads sources.csv (T-4894's inventory of the Banská Bystrica Region), sources-sk.csv (T-4899's and
T-5047's inventory of the rest of Slovakia) and venues.csv (venues checked by hand), takes the rows whose
decision is `pipeline`, whose feed is https and whose feed kind has a template under pipelines/, and writes,
per store of STORES, components/context-gateway/seed/<ns>/<ns>-{datasource,pipeline}-podujatia-<name>.yaml and
its -bento.yaml plus their lines in that folder's index.yaml, and the list of sources with their licence into
the catalogue block of the store's Endpoint (T-4898). The Banská Bystrica Region's store reads its own sources
once a night; the national store (slovensko-podujatia) reads every source every 3 hours. `--check` writes
nothing and fails when a file differs.

Run: python3 seeds/bbsk-events/generate.py [--check]
"""

import csv
import hashlib
import json
import math
import re
import sys
import unicodedata
from collections import Counter
from pathlib import Path
from urllib.parse import urlsplit

import yaml

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SEED = ROOT / "components/context-gateway/seed/bbsk"
TARGET = "urn:ngsi-ld:Endpoint:bbsk.sk:bbsk-podujatia:bbsk-podujatia-zapis"
UA = "joinedcontext-platform/1.0 (+https://dev.joinedcontext.com; public events)"
REGION = "Banskobystrický kraj"
#: The inventories, each with the region code of its rows' scope /SK/{code}/{town}.
INVENTORIES = ("sources.csv", "sources-sk.csv", "sources-cz.csv")
#: Inventory region -> (scope segment, addressRegion). BBSK rows carry the region's own name.
REGIONS = {
    "BBSK": ("BB", "Banskobystrický kraj"),
    "BA": ("BA", "Bratislavský kraj"),
    "TT": ("TT", "Trnavský kraj"),
    "TN": ("TN", "Trenčiansky kraj"),
    "NR": ("NR", "Nitriansky kraj"),
    "ZA": ("ZA", "Žilinský kraj"),
    "PO": ("PO", "Prešovský kraj"),
    "KE": ("KE", "Košický kraj"),
    "PHA": ("PHA", "Hlavní město Praha"),
}
#: The country of an inventory region outside Slovakia: the scope's first level and the address's country (T-5666).
COUNTRIES = {"PHA": "CZ"}
SLOVAK = tuple(r for r in REGIONS if r not in COUNTRIES)
#: The two event stores (owner 2026-10-21, T-5047). Each lists the inventory regions it reads (None: all),
#: its write Endpoint, its cadences and its grace. A cadence names the feed kinds it takes (None: the rest),
#: how often a source runs, the window its runs are staggered over (minutes after 00:00 UTC) and its lease. An
#: event leaves at its end + `grace` seconds, or `lease` seconds after the run that last saw it, whichever comes
#: first (None: no lease). The lease is what ends an event its source dropped or cancelled: spec.expiry is
#: refused where several pipelines write one space (pipeline_expiry::check).
STORES = (
    {
        "ns": "bbsk",
        "domain": "bbsk.sk",
        "space": "bbsk-podujatia",
        "regions": ("BBSK",),
        "target": TARGET,
        "endpoints": ("bbsk-endpoint-podujatia.yaml",),
        # Daily, and the same 52 h lease as the national store's daily sources (T-5081): an event its source
        # dropped, or whose new record the model now refuses (no GPS, start or organizer), leaves within two
        # missed runs instead of staying half until its end.
        "cadences": ({"kinds": None, "period": "24h", "every": 24, "window": 180, "lease": 52 * 3600},),
        "grace": 0,
    },
    {
        "ns": "slovensko",
        "domain": "joinedcontext.com",
        "space": "slovensko-podujatia",
        "regions": SLOVAK,
        "target": "urn:ngsi-ld:Endpoint:joinedcontext.com:slovensko-podujatia:slovensko-podujatia-zapis",
        "endpoints": ("slovensko-endpoint-podujatia.yaml",),
        # A village's calendar page (Lego, WEBY) changes rarely and every run reads each event page 1-3 s
        # apart: once a day, spread over the day. The feeds run every 3 hours. Each lease outlasts two missed
        # runs, so a source down for one run keeps its events.
        "cadences": (
            {"kinds": ("lego-html", "weby-feed", "umb-cms", "drupal-kalendar", "sc-rss", "ticketware-html", "eventphp-html",
                       "krupina-html"), "period": "24h", "every": 24, "window": 1440, "lease": 52 * 3600},
            {"kinds": None, "period": "3h", "every": 3, "window": 180, "lease": 8 * 3600},
        ),
        "grace": 86400,
    },
    # Prague's events (T-5666): sources-cz.csv, every 6 hours, the space praha-akce of the praha project.
    {
        "ns": "praha",
        "domain": "praha.eu",
        "space": "praha-akce",
        "regions": ("PHA",),
        "target": "urn:ngsi-ld:Endpoint:praha.eu:praha-akce:praha-akce-zapis",
        "endpoints": ("praha-endpoint-akce.yaml",),
        "cadences": ({"kinds": None, "period": "6h", "every": 6, "window": 360, "lease": 14 * 3600},),
        "grace": 86400,
    },
)
#: feed kind of the inventory -> template files under pipelines/, run in this order.
TEMPLATES = {
    "wp-rest-sc": ("wp-rest-sc",),
    "wp-rest-fields": ("wp-rest-fields",),
    "tribe-rest": ("tribe-rest",),
    "ical": ("ical",),
    "weby-feed": ("weby-feed",),
    "mec-rest": ("wp-list", "jsonld-pages"),
    "lsvr-rest": ("wp-list", "jsonld-pages"),
    "eventon-rest": ("wp-list", "jsonld-pages"),
    "wp-rest-jsonld": ("wp-list", "jsonld-pages"),
    "wp-rss-jsonld": ("rss-list", "jsonld-pages"),
    "jsonld": ("page-jsonld",),
    "lego-html": ("lego-html",),
    "umb-cms": ("umb-cms",),
    "drupal-kalendar": ("drupal-kalendar",),
    "sc-rss": ("rss-list", "sc-event-page"),
    "ticketware-html": ("ticketware-html",),
    "eventphp-html": ("eventphp-html",),
    "krupina-html": ("krupina-html",),
    "html-cards": ("html-cards",),
}
#: Fewer bytes per run: the fields the template reads (WordPress `_fields`). The kinds that read each
#: event's page list the 50 newest posts: every page read is one more request to a small town's site.
_PAGES = "_fields=id,link,title,modified_gmt&per_page=50"
TRIM = {
    "wp-rest-sc": "_fields=id,link,title,content,modified_gmt,meta_box",
    "wp-rest-fields": "_fields=id,link,title,excerpt,modified_gmt,meta,acf,more_info,event_date",
    "mec-rest": _PAGES,
    "lsvr-rest": _PAGES,
    "eventon-rest": _PAGES,
    "wp-rest-jsonld": _PAGES,
}
#: wp-rest-fields: per feed host, the dotted path of each field the template reads in a REST item (T-5365,
#: read on each site 2026-10-23). A path absent here is no field of that site.
WP_FIELDS = {
    "zilina.sk": {"start": "meta.iq_dateStart", "end": "meta.iq_dateEnd", "time_start": "meta.iq_timeStart",
                  "time_end": "meta.iq_timeEnd", "venue": "meta.iq_addressLocality", "lat": "meta.iq_coordinatesLat",
                  "lon": "meta.iq_coordinatesLng"},
    "regiontatry.sk": {"start": "more_info.datum_od", "end": "more_info.datum_do", "time_start": "more_info.cas_od",
                       "time_end": "more_info.cas_do", "venue": "more_info.miesto_konania"},
    "www.podujatiaksk.sk": {"start": "acf.podujatie_datum.podujatie_datum_od", "end": "acf.podujatie_datum.podujatie_datum_do"},
    # The event's days as Unix seconds at 00:00 UTC; its event page holds no JSON-LD Event since 2026-10 (T-5435).
    "www.visitbratislava.com": {"start": "event_date.start_date", "end": "event_date.end_date"},
    "www.upjs.sk": {"start": "acf.cpt_udalost_datum_konania", "end": "acf.cpt_udalost_datum_koniec",
                    "time_start": "acf.cpt_udalost_cas_konania", "venue": "acf.cpt_udalost_miesto_konania"},
}
#: html-cards: per feed host, where an events list page starts a card and the pattern of each field inside it,
#: the value in the groups v, v2 and v3 (T-5393, read on each site 2026-10-23). A field absent here is no field of that site.
CARDS = {
    "prievidza.sk": {
        "card": '<a href="[^"]{1,300}" class="event-card">',
        "url": '<a href="(?P<v>[^"]{1,300})"',
        "name": 'event-card__heading">(?P<v>[^<]{1,300})<',
        "venue": 'event-card__location">(?P<v>[^<]{1,300})<',
        "when": 'event-card__date">(?P<v>[^<]{1,100})<',
    },
    "uluv.sk": {
        "card": '<a href="https://uluv\\.sk/podujatia/[^"]{1,300}">\\s*<div class="event-single',
        "url": '<a href="(?P<v>[^"]{1,300})"',
        "name": 'event-single__title[^>]*>(?P<v>[^<]{1,300})<',
        "venue": 'event-single__place">(?P<v>[^<]{1,300})<',
        "locality": 'event-single__place">[^<]{0,100}ÚĽUV (?P<v>\\p{Lu}[^<]{1,60}?)\\s*<',
        "when": 'event-single__place">[^<]{0,300}</div>\\s*<div class="ps-md-3">(?P<v>[^<]{1,100})<',
    },
    "www.vsmu.sk": {
        "card": '<a href="[^"]{1,300}" class="cell card card--horizontal card--event"',
        "url": '<a href="(?P<v>[^"]{1,300})"',
        "name": '<h2 class="headline">(?P<v>[^<]{1,300})<',
        "venue": '<div class="category">(?P<v>[^<]{1,300})<',
        "when": '<ul class="meta">\\s*<li><div[^>]*></div></li>\\s*<li>(?P<v>[^<]{1,60})</li>',
        "time": '<ul class="meta">\\s*<li><div[^>]*></div></li>\\s*<li>[^<]{1,60}</li>\\s*<li>(?P<v>[^<]{1,20})</li>',
    },
    "agrokomplex.sk": {
        "card": '<div class="exhib__box[^"]{0,100}" data-href=',
        "url": 'data-href="(?P<v>[^"]{1,300})"',
        "name": '<h2>(?P<v>[^<]{1,300})</h2>',
        "when": '<span class="date"><img[^>]*>(?P<v>[^<]{1,60})<',
    },
    "novacvernovka.eu": {
        "card": '<article class="infinite-scroll-item',
        "url": 'data-href="(?P<v>[^"]{1,300})"',
        "name": '<h3 class="entry-title[^"]{0,100}">(?P<v>[^<]{1,300})<',
        "when": 'data-start="(?P<v>[0-9]{8})"',
        "end": 'data-end="(?P<v>[0-9]{8})"',
        "time": 'class="Dayname">[^<]{0,30}</div>\\s*<div>\\s*(?P<v>[0-9]{1,2}:[0-9]{2})',
    },
    "www.bkis.sk": {
        "card": '<a class="event" href=',
        "url": 'href="(?P<v>[^"]{1,300})"',
        "name": 'event-name">(?P<v>[^<]{1,300})<',
        "when": '<p class="font-variation">(?P<v>[0-9][^<]{1,60})</p>',
        "venue": '<p class="font-variation">[0-9][^<]{1,60}</p>\\s*<div[^>]*>\\s*<p class="font-variation">(?P<v>[^<]{1,300})<',
    },
    "www.kosnr.sk": {
        "card": '<div class="ic-box"',
        "url": '<h3>\\s*<a href="(?P<v>[^"]{1,300})"',
        "name": '<h3>\\s*<a href="[^"]{1,300}">(?P<v>[^<]{1,300})<',
        "when": 'ic-day">\\s*(?P<v>[0-9]{1,2})\\s*</div>\\s*<div class="ic-month">\\s*(?P<v2>[^<\\s]{1,12})\\s*</div>\\s*<div class="ic-year">\\s*(?P<v3>[0-9]{4})',
        "time": 'ic-time">\\s*(?P<v>[0-9]{1,2}:[0-9]{2})',
        "venue": 'ic-place">\\s*(?P<v>[^<]{1,300})<',
    },
    "kultura.mestocadca.sk": {
        "card": '<div class="page-header">\\s*<h2>',
        "url": '<h2>\\s*<a href="(?P<v>[^"]{1,300})"',
        "name": '<h2>\\s*<a href="[^"]{1,300}">(?P<v>[^<]{1,300})<',
        "when": 'Kedy:\\s*</span>\\s*<span class="field-value\\s*">(?P<v>[^<]{1,100})<',
        "venue": 'Kde:\\s*</span>\\s*<span class="field-value\\s*">(?P<v>[^<]{1,300})<',
    },
}
#: Upcoming events first where the feed sorts and filters itself (owner 2026-10-20).
UPCOMING = {"tribe-rest": "start_date=now"}
#: The main event categories and their rule table (T-5363, docs API/12-event-categories.md).
CATEGORIES = json.loads((ROOT / "seeds/event-categories.json").read_text(encoding="utf-8"))["categories"]
#: The decision model's answers for the events no rule decides (scripts/events-classify.py, T-5490): per source,
#: the hash of an event's name and venue -> its code.
with (HERE / "categories.csv").open(encoding="utf-8", newline="") as _f:
    DECIDED: dict[str, dict[str, str]] = {}
    for _r in csv.DictReader(_f):
        DECIDED.setdefault(_r["source"], {})[_r["key"]] = _r["category"]
INDEX_BEGIN = (
    "# BEGIN generated by seeds/bbsk-events/generate.py (T-4896): do not edit by hand"
)
INDEX_END = "# END generated by seeds/bbsk-events/generate.py"
SOURCES_BEGIN = "    # BEGIN sources generated by seeds/bbsk-events/generate.py (T-4898): do not edit by hand"
SOURCES_END = "    # END sources generated by seeds/bbsk-events/generate.py"


class Dumper(yaml.SafeDumper):
    """Writes a multi-line string (a mapping) as a literal block, so it reads as written."""


Dumper.add_representer(
    str,
    lambda d, v: d.represent_scalar(
        "tag:yaml.org,2002:str", v, style="|" if "\n" in v else None
    ),
)


def slug(text: str) -> str:
    ascii_ = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode()
    return re.sub(r"[^a-z0-9]+", "-", ascii_.lower()).strip("-")


def kept_sources() -> list[dict]:
    rows = [
        r
        for f in INVENTORIES
        for r in csv.DictReader((HERE / f).open(encoding="utf-8"))
    ]
    # A plain-http feed stays out: the runner fetches over TLS only (T-4898 names the rows).
    kept = [
        r
        for r in rows
        if r["decision"] == "pipeline"
        and r["feed_kind"] in TEMPLATES
        and r["check_url"].startswith("https://")
    ]
    labels_of = lambda r: urlsplit(r["check_url"]).hostname.removeprefix("www.").split(".")
    # A first label two hosts share (kultura.trnava.sk, kultura.tornala.eu) names neither: both take two labels.
    shared = {k for k, n in Counter(labels_of(r)[0] for r in kept).items() if n > 1}
    for row in kept:
        parts = urlsplit(row["check_url"])
        assert parts.scheme == "https" and re.fullmatch(
            r"[a-z0-9.-]+", parts.hostname or ""
        ), row["check_url"]
        labels = labels_of(row)
        row["name"] = "podujatia-" + "-".join(labels[:2] if labels[0] in shared else labels[:1])
        row["host"] = f"https://{parts.hostname}"
        row["town_slug"] = None if row["town"] == REGION else slug(row["town"])
        row["scope"], row["region_name"] = REGIONS[row["region"]]
        row["country"] = COUNTRIES.get(row["region"], "SK")
    names = [r["name"] for r in kept]
    assert len(names) == len(set(names)), names
    return kept


#: Two venues.csv rows of one name closer than this are one venue (DM-141).
SAME_VENUE_M = 50


def metres(a: dict, b: dict) -> float:
    """The distance of two rows' points, equirectangular: exact to a centimetre over 50 m."""
    la1, lo1, la2, lo2 = map(math.radians, (float(a["lat"]), float(a["lon"]), float(b["lat"]), float(b["lon"])))
    return 6371000 * math.hypot((lo2 - lo1) * math.cos((la1 + la2) / 2), la2 - la1)


def venue_groups(rows: list[dict]) -> list[list[dict]]:
    """The rows grouped by real venue (DM-141): one OpenStreetMap object, or one name in one town within
    SAME_VENUE_M, is one venue, in table order."""
    parent = list(range(len(rows)))

    def root(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i, a in enumerate(rows):
        for j in range(i):
            b = rows[j]
            same_osm = a["osm"] and a["osm"] == b["osm"]
            same_name = (a["town"], key(a["venue"])) == (b["town"], key(b["venue"])) and metres(a, b) < SAME_VENUE_M
            if same_osm or same_name:
                parent[root(i)] = root(j)
    groups: dict[int, list[dict]] = {}
    for i, row in enumerate(rows):
        groups.setdefault(root(i), []).append(row)
    return list(groups.values())


def poi_id(group: list[dict]) -> str:
    """A venue's PointOfInterest id after urn:ngsi-ld:PointOfInterest:{space}: its first row's OpenStreetMap
    object (`osm-way-292648504`), else a hash of its town, name and point."""
    first = group[0]
    if first["osm"]:
        return "osm-" + first["osm"].replace("/", "-")
    text = f"{first['town']}|{key(first['venue'])}|{float(first['lat']):.5f}|{float(first['lon']):.5f}"
    return "h-" + hashlib.sha256(text.encode()).hexdigest()[:16]


_POI_IDS: dict[tuple[str, str], str] = {}


def poi_ids() -> dict[tuple[str, str], str]:
    """(town, match) of every venues.csv row -> its venue's PointOfInterest id."""
    if not _POI_IDS:
        rows = list(csv.DictReader((HERE / "venues.csv").open(encoding="utf-8")))
        for group in venue_groups(rows):
            for row in group:
                _POI_IDS[(row["town"], row["match"])] = poi_id(group)
    return _POI_IDS


def venue(v: dict) -> dict:
    return {
        "poi": poi_ids()[(v["town"], v["match"])],
        "match": v["match"],
        "town": v["town"],
        "venue": v["venue"],
        "streetAddress": v["streetAddress"] or None,
        "addressLocality": v["addressLocality"],
        "lat": float(v["lat"]),
        "lon": float(v["lon"]),
        # Who placed it (register-adries, overpass, nominatim, hand): the event's locationSource (T-5490).
        "checked_by": v["checked_by"],
    }


def venues_for(town_slug: str | None) -> list[dict]:
    rows = csv.DictReader((HERE / "venues.csv").open(encoding="utf-8"))
    return [
        venue(v)
        for v in rows
        if v["town"] == town_slug or (town_slug is None and v["regional"] == "y")
    ]


def home(row: dict) -> dict | None:
    """The source's own address (homes.csv -> a venues.csv row): the point of an event that names no place (T-5135)."""
    for h in csv.DictReader((HERE / "homes.csv").open(encoding="utf-8")):
        if h["events_url"] == row["events_url"]:
            rows = csv.DictReader((HERE / "venues.csv").open(encoding="utf-8"))
            found = [venue(v) for v in rows if (v["town"], v["match"]) == (h["town"], h["match"])]
            assert len(found) == 1, f"homes.csv names no venues.csv row: {h}"
            return found[0]
    return None


def towns_for(row: dict) -> list[list]:
    """The towns.csv points a source's events may fall back to (T-5240): a municipality's calendar its own town; a
    museum's, a theatre's or a tourism body's every town of its region named by one municipality alone."""
    municipal = row["org_type"] in ("town", "municipality", "village")
    return [
        # [name, slug, lat, lon, precision], 5 decimals (about a metre): the table rides in every aggregator's
        # pipeline and the seed releases have a size ceiling (test_sample_apps_seed).
        [t["name"], t["town"], round(float(t["lat"]), 5), round(float(t["lon"]), 5), t["precision"]]
        for t in csv.DictReader((HERE / "towns.csv").open(encoding="utf-8"))
        if t["town"] == row["town_slug"] or (not municipal and t["region"] == row["region"] and t["unique"] == "y")
    ]


#: How a municipality's calendar names the municipality or its office instead of a place (T-5215, dev 2026-10-22:
#: "Obec" 28 events of Oravská Polhora, "Celá obec" Ďurčiná, "Obecný úrad").
MUNICIPAL_SELF = ("obec", "mesto", "celá obec", "obecný úrad", "mestský úrad", "obecný úrad {town}",
                  "mestský úrad {town}", "mesto {town}", "obec {town}", "v obci", "{town} - celá obec")


def self_names(row: dict) -> set[str]:
    """The place texts that name the town or the source's own body rather than a place of their own."""
    named = {key(row["town"]), *([key(organizer(row)["name"])] if organizer(row) else [])}
    if row["org_type"] in ("town", "municipality", "village"):
        named |= {key(t.format(town=row["town"])) for t in MUNICIPAL_SELF}
    return named


#: The abbreviations calendars write for a hall or an office, spelled out in the match key (T-5240), in this order.
ABBREVIATIONS = (("kd", "kultúrny dom"), ("dk", "dom kultúry"), ("msks", "mestské kultúrne stredisko"),
                 ("ocú", "obecný úrad"), ("oú", "obecný úrad"), ("msú", "mestský úrad"), ("mú", "mestský úrad"))


def key(text: str) -> str:
    """A place text's match key, as pipelines/common.yaml computes it."""
    text = re.sub(r"\([^)]*\)", " ", text.lower())
    text = " " + re.sub(r"\s+", " ", re.sub(r"[,;]", " ", text)).strip() + " "
    for short, long in ABBREVIATIONS:
        text = text.replace(f" {short} ", f" {long} ")
    return text.strip()


def cadence(row: dict, store: dict) -> dict:
    return next(c for c in store["cadences"] if c["kinds"] is None or row["feed_kind"] in c["kinds"])


def org_category(row: dict) -> str | None:
    """The category of every event a body of this kind announces (a theatre's calendar), else None."""
    kind = re.sub(r" \(.*", "", row["org_type"])
    return next((c["code"] for c in CATEGORIES if kind in c["orgTypes"]), None)


def processors(row: dict, store: dict) -> list:
    """The template's steps then the common tail, each __NAME__ a JSON literal."""
    facts_only = not row["licence_terms"].startswith("CC-BY")
    values = {
        "__VENUES__": json.dumps(
            venues_for(row["town_slug"]), ensure_ascii=False, separators=(",", ":")
        ),
        "__TOWN__": json.dumps(
            None if row["town_slug"] is None else row["town"], ensure_ascii=False
        ),
        # The feed's town as one whole scope literal: a bare slug can equal a project name ("zilina").
        "__TOWN_SCOPE__": json.dumps(
            f"/{row['country']}/" + row["scope"]
            + ("" if row["town_slug"] is None else "/" + row["town_slug"].replace("-", "_"))
        ),
        "__HOME__": json.dumps(home(row), ensure_ascii=False, separators=(",", ":")),
        "__TOWNS__": json.dumps(towns_for(row), ensure_ascii=False, separators=(",", ":")),
        # A place text that is only the town's or the organizer's own name names no place of its own.
        "__SELF__": json.dumps(sorted(self_names(row)), ensure_ascii=False),
        "__SCOPE__": json.dumps(f"/{row['country']}/" + row["scope"]),
        "__REGION__": json.dumps(row["region_name"], ensure_ascii=False),
        "__SOURCE__": json.dumps(row["events_url"], ensure_ascii=False),
        "__PROVIDER__": json.dumps(provider(row), ensure_ascii=False),
        "__ORGANIZER__": json.dumps(organizer(row), ensure_ascii=False),
        "__LISTING__": json.dumps(row["events_url"], ensure_ascii=False),
        "__FACTS_ONLY__": "true" if facts_only else "false",
        # A link to the source's own host over http is still its page (Kysucké Nové Mesto's feed links every
        # event over http): the regex takes either scheme, the request goes to __HOST__ over https.
        "__HOST_RE__": re.escape(row["host"]).replace("\\", "\\\\").replace("https:", "https?:", 1),
        "__HOST__": row["host"],
        # The events list's own path: a Lego source reads only the event pages under it, never a sidebar's news.
        "__LIST_PATH_RE__": re.escape(urlsplit(row["check_url"]).path).replace("\\", "\\\\"),
        "__LIST_PATH__": json.dumps(urlsplit(row["check_url"]).path),
        "__GRACE__": json.dumps(store["grace"]),
        "__FIELDS__": json.dumps(WP_FIELDS.get(urlsplit(row["check_url"]).hostname)),
        "__CARDS__": json.dumps(CARDS.get(urlsplit(row["check_url"]).hostname), ensure_ascii=False),
        "__LEASE__": json.dumps(cadence(row, store)["lease"]),
        "__CATEGORY_CACHE__": json.dumps(DECIDED.get(row["events_url"], {}), sort_keys=True),
        "__CATEGORY_DEFAULT__": json.dumps(org_category(row)),
    }
    text = "".join(
        (HERE / "pipelines" / f"{name}.yaml").read_text(encoding="utf-8")
        for name in TEMPLATES[row["feed_kind"]]
    )
    for name, value in values.items():
        text = text.replace(name, value)
    assert not re.search(r"__[A-Z_]+__", text), row["name"]
    # The tail's settings as one structured metadata value it reads as @events.<name> (T-5555); the space stays
    # env("JC_SPACE"), which the Portal writes in per stream.
    tail = re.findall(r"@events\.([a-z_]+)\b(?!\()",(HERE / "pipelines/common.yaml").read_text(encoding="utf-8"))
    settings = ", ".join(
        f'"{name}": ' + ('env("JC_SPACE")' if name == "space" else values[f"__{name.upper()}__"])
        for name in dict.fromkeys(sorted(tail))
    )
    return [
        *(yaml.safe_load(text) or []),
        {"mutation": "meta events = {" + settings + "}\n"},
        {"resource": TAIL},
    ]


#: The processor resource every events pipeline ends with, pipelines/common.yaml once in the runner (T-5555).
TAIL = "events_tail"
RESOURCES = ROOT / "components/pipeline-runner/streams/resources.yaml"
TAIL_BEGIN = "# BEGIN events_tail: generated by seeds/bbsk-events/generate.py from pipelines/common.yaml, do not edit"
TAIL_END = "# END events_tail"


def tail_block() -> str:
    """resources.yaml's processor_resources block: common.yaml as the resource events_tail."""
    rules = json.dumps([[c["code"], c["rule"]] for c in CATEGORIES if c["rule"]], ensure_ascii=False)
    text = (HERE / "pipelines/common.yaml").read_text(encoding="utf-8").replace("__CATEGORY_RULES__", rules)
    assert not re.search(r"__[A-Z_]+__", text)
    body = "".join(f"      {line}" if line.strip() else "\n" for line in text.splitlines(keepends=True))
    return f"{TAIL_BEGIN}\nprocessor_resources:\n  - label: {TAIL}\n    processors:\n{body}{TAIL_END}\n"


def provider(row: dict) -> str:
    host = urlsplit(row["events_url"]).hostname.removeprefix("www.")
    licence = (
        row["licence_terms"].split(" ")[0]
        if row["licence_terms"].startswith("CC-")
        else "licencia neuvedená"
    )
    return f"{row['town']}, {host} ({licence})"


#: Inventory bodies that announce other bodies' events: their calendar names no organizer of its own (T-5081).
AGGREGATORS = re.compile(r"^(regional )?tourism organisation|^region\b")


def organizer(row: dict) -> dict | None:
    """Who runs an event its feed names no organizer for: the body whose calendar it is, never an aggregator.

    The museum, theatre or school of `org_type (Name)`, else the town (`Mesto`) or the municipality (`Obec`,
    a town is one too by law), with its web site.
    """
    kind = row["org_type"]
    if AGGREGATORS.match(kind):
        return None
    named = re.fullmatch(r"[^(]+\((.+)\)", kind)
    if named:
        name = named.group(1).strip()
    elif kind == "town":
        name = f"Mesto {row['town']}"
    elif kind in ("municipality", "village"):
        name = f"Obec {row['town']}"
    else:
        return None
    return {"name": name, "url": row["site"]}


def feed_url(row: dict) -> str:
    """The row's feed with the template's trims; a parameter the trim sets replaces the row's own."""
    url = row["check_url"]
    for extra in (TRIM, UPCOMING):
        if row["feed_kind"] in extra:
            for pair in extra[row["feed_kind"]].split("&"):
                url = re.sub(r"([?&])" + re.escape(pair.split("=")[0]) + r"=[^&]*&?", r"\1", url).rstrip("&?")
                url += ("&" if "?" in url else "?") + pair
    return url


def schedule(minute: int, every: int) -> str:
    """Cron for a run at `minute` after 00:00 UTC, then every `every` hours that day."""
    hour = str(minute // 60) if every == 24 else f"{minute // 60}-23/{every}"
    return f"{minute % 60} {hour} * * *"


def documents(minute: int, row: dict, store: dict) -> dict[str, str]:
    name, town, ns = row["name"], row["town"], store["ns"]
    url = feed_url(row)
    plan = cadence(row, store)
    datasource = {
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "DataSource",
        "metadata": {
            "name": name,
            "namespace": ns,
            "title": {
                "sk": f"Podujatia: {town} ({row['feed_kind']})",
                "en": f"Events: {town} ({row['feed_kind']})",
            },
        },
        "spec": {
            "type": "http",
            "http": {
                "url": url,
                "verb": "GET",
                "headers": {"User-Agent": UA, "Accept": accept(row["feed_kind"])},
                "timeout": "30s",
            },
        },
    }
    pipeline = {
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": {
            "name": name,
            "namespace": ns,
            "title": {"sk": f"Podujatia: {town}", "en": f"Events: {town}"},
        },
        "spec": {
            "class": "auto",
            "period": plan["period"],
            "schedule": schedule(minute, plan["every"]),
            "source": {"dataSourceRef": {"kind": "DataSource", "name": name}},
            "targetEndpoint": store["target"],
            "output": {"type": "Event", "mode": "upsert"},
            "quotas": {"maxMemoryMb": 128, "cpuMillicores": 100},
        },
    }
    head = (
        f"# Generated by seeds/bbsk-events/generate.py from the inventory row {town} ({row['feed_kind']}),\n"
        f"# read {row['read_on']}: {row['events_url']} (T-4896). Do not edit: change the template or the CSV.\n"
    )
    bento = {"pipeline": {"processors": processors(row, store)}}
    dump = lambda d: yaml.dump(
        d, Dumper=Dumper, allow_unicode=True, sort_keys=False, width=120
    )
    return {
        f"{ns}-datasource-{name}.yaml": head + dump(datasource),
        f"{ns}-pipeline-{name}.yaml": head + dump(pipeline),
        f"{ns}-pipeline-{name}-bento.yaml": head + dump(bento),
    }


VENUES = "miesta-podujati"


def store_venues(rows: list[dict]) -> list[list]:
    """The PointOfInterest of each venue a store's events can link (DM-141), one per venue_groups() group:
    [id, name, streetAddress, addressLocality, lat, lon, scope, areaServed]."""
    linked = {(v["town"], v["match"]) for r in rows for v in venues_for(r["town_slug"])}
    towns = {t["town"]: t for t in csv.DictReader((HERE / "towns.csv").open(encoding="utf-8"))}
    out = []
    for group in venue_groups(list(csv.DictReader((HERE / "venues.csv").open(encoding="utf-8")))):
        if not any((v["town"], v["match"]) in linked for v in group):
            continue
        first = group[0]
        town = towns[first["town"]]
        out.append([
            poi_id(group),
            min((v["venue"] for v in group), key=len),
            next((v["streetAddress"] for v in group if v["streetAddress"]), None),
            first["addressLocality"],
            float(first["lat"]),
            float(first["lon"]),
            f"/{COUNTRIES.get(town['region'], 'SK')}/" + REGIONS[town["region"]][0] + "/" + first["town"].replace("-", "_"),
            town["name"],
        ])
    return out


#: The mapping of a store's venue pipeline: each table row one PointOfInterest (DM-141).
VENUE_MAPPING = """root = {
  "id": "urn:ngsi-ld:PointOfInterest:%s:%s".format(env("JC_SPACE"), this.index(0)),
  "type": "PointOfInterest",
  "scope": this.index(6),
  "name": { "type": "Property", "value": this.index(1) },
  "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": [this.index(5), this.index(4)] } },
  "address": { "type": "Property", "value": { "streetAddress": this.index(2), "addressLocality": this.index(3), "addressCountry": this.index(6).split("/").index(1) }.filter(kv -> kv.value != null) },
  "areaServed": { "type": "Property", "value": this.index(7) },
  "source": { "type": "Property", "value": if this.index(0).has_prefix("osm-") { "https://www.openstreetmap.org/" + this.index(0).trim_prefix("osm-").replace("-", "/") } else { "https://github.com/marek-mraz-jc/joinedcontext-deployment/blob/main/seeds/bbsk-events/venues.csv" } },
  "dataProvider": { "type": "Property", "value": "© OpenStreetMap contributors, ODbL 1.0" }
}
"""


def venue_documents(store: dict, rows: list[dict]) -> dict[str, str]:
    """The DataSource, Pipeline and mapping that write a store's venues as PointOfInterest entities, once a day
    at 03:30 UTC, after the night's event runs (DM-141). The table rides in the mapping: no outbound call."""
    ns = store["ns"]
    title = {"sk": "Miesta konania podujatí", "en": "Event venues"}
    datasource = {
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "DataSource",
        "metadata": {"name": VENUES, "namespace": ns, "title": title},
        "spec": {"type": "generate", "input": {"mapping": 'root = ""', "count": 1, "interval": ""}},
    }
    pipeline = {
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": {"name": VENUES, "namespace": ns, "title": title},
        "spec": {
            "class": "auto",
            "period": "24h",
            "schedule": "30 3 * * *",
            "source": {"dataSourceRef": {"kind": "DataSource", "name": VENUES}},
            "targetEndpoint": store["target"],
            "output": {"type": "PointOfInterest", "mode": "upsert"},
            "quotas": {"maxMemoryMb": 128, "cpuMillicores": 100},
        },
    }
    table = json.dumps(store_venues(rows), ensure_ascii=False, separators=(",", ":"))
    bento = {"pipeline": {"processors": [
        {"mapping": f"root = {table}\n"},
        {"unarchive": {"format": "json_array"}},
        {"mapping": VENUE_MAPPING},
        {"archive": {"format": "json_array"}},
    ]}}
    head = (
        "# Generated by seeds/bbsk-events/generate.py from venues.csv and towns.csv (T-5364, DM-141): one\n"
        "# PointOfInterest per venue the events link. Do not edit: change the CSV.\n"
    )
    dump = lambda d: yaml.dump(d, Dumper=Dumper, allow_unicode=True, sort_keys=False, width=120)
    return {
        f"{ns}-datasource-{VENUES}.yaml": head + dump(datasource),
        f"{ns}-pipeline-{VENUES}.yaml": head + dump(pipeline),
        f"{ns}-pipeline-{VENUES}-bento.yaml": head + dump(bento),
    }


def accept(kind: str) -> str:
    return {
        "ical": "text/calendar",
        "weby-feed": "application/rss+xml, application/atom+xml, application/xml",
        "jsonld": "text/html",
        "lego-html": "text/html",
        "umb-cms": "text/html",
        "drupal-kalendar": "text/html",
        "ticketware-html": "text/html",
        "eventphp-html": "text/html",
        "krupina-html": "text/html",
        "html-cards": "text/html",
        "sc-rss": "application/rss+xml, application/atom+xml, application/xml",
        "wp-rss-jsonld": "application/rss+xml, application/atom+xml, application/xml",
    }.get(kind, "application/json")


def index_lines(rows: list[dict], ns: str) -> list[str]:
    lines = [INDEX_BEGIN]
    for row in [*rows, {"name": VENUES}]:
        lines += [
            f"{ns}-datasource-{row['name']}.yaml: projects/{ns}/datasources/{row['name']}.yaml",
            f"{ns}-pipeline-{row['name']}.yaml: projects/{ns}/pipelines/{row['name']}/pipeline.yaml",
            f"{ns}-pipeline-{row['name']}-bento.yaml: projects/{ns}/pipelines/{row['name']}/bento.yaml",
        ]
    return lines + [INDEX_END]


def catalogue_sources(rows: list[dict]) -> list[str]:
    """The `source:` block of an Endpoint's catalogue: each kept feed with its licence (T-4898)."""
    lines = [SOURCES_BEGIN, "    source:"]
    for row in rows:
        host = urlsplit(row["events_url"]).hostname.removeprefix("www.")
        title = json.dumps(f"{row['town']}, {host}", ensure_ascii=False)
        facts = not row["licence_terms"].startswith("CC-BY")
        sk = (
            "Podujatia zo stránky {}; licencia neuvedená, preberáme len fakty (názov, čas, miesto, odkaz)."
            if facts
            else "Podujatia zo stránky {}; licencia {}."
        )
        en = (
            "Events from {}; no licence stated, facts only (name, time, place, link)."
            if facts
            else "Events from {}; licence {}."
        )
        licence = row["licence_terms"].split(" ")[0]
        lines += [
            f"      - url: {json.dumps(row['events_url'])}",
            f"        title: {{ sk: {title}, en: {title} }}",
            "        description:",
            f"          sk: {json.dumps(sk.format(row['events_url'], licence), ensure_ascii=False)}",
            f"          en: {json.dumps(en.format(row['events_url'], licence), ensure_ascii=False)}",
        ]
    return lines + [SOURCES_END]


def with_block(text: str, begin: str, end: str, block: str) -> str:
    """The text with its generated block replaced, or the block appended when it has none."""
    if begin in text:
        return re.sub(
            re.escape(begin) + r".*?" + re.escape(end) + "\n",
            lambda _: block,
            text,
            flags=re.DOTALL,
        )
    return text.rstrip("\n") + "\n" + block


def store_rows(store: dict) -> list[dict]:
    return [r for r in kept_sources() if store["regions"] is None or r["region"] in store["regions"]]


def wanted_files() -> tuple[dict[Path, str], list[Path]]:
    """Every generated file with its text, and the generated files on disk no store wants any more."""
    wanted: dict[Path, str] = {}
    ours: list[Path] = []
    for store in STORES:
        ns, seed = store["ns"], ROOT / "components/context-gateway/seed" / store["ns"]
        rows = store_rows(store)
        # Each cadence's runs are staggered over its window, so no two of them start in the same minute; the
        # bbsk night window is 00:00-02:59 UTC, 01:00-05:00 in Bratislava in winter and summer (owner 2026-10-20).
        for plan in store["cadences"]:
            mine = [r for r in rows if cadence(r, store) is plan]
            assert len(mine) <= plan["window"], f"{ns}: more sources than minutes in a window"
            for i, row in enumerate(mine):
                for file, text in documents(i * plan["window"] // len(mine), row, store).items():
                    wanted[seed / file] = text
        for file, text in venue_documents(store, rows).items():
            wanted[seed / file] = text
        for file in store["endpoints"]:
            path = seed / file
            block = "\n".join(catalogue_sources(rows)) + "\n"
            wanted[path] = with_block(path.read_text(encoding="utf-8"), SOURCES_BEGIN, SOURCES_END, block)
        index = seed / "index.yaml"
        block = "\n".join(index_lines(rows, ns)) + "\n"
        wanted[index] = with_block(index.read_text(encoding="utf-8"), INDEX_BEGIN, INDEX_END, block)
        ours += [*seed.glob(f"{ns}-datasource-podujatia-*.yaml"), *seed.glob(f"{ns}-pipeline-podujatia-*.yaml")]
    wanted[RESOURCES] = with_block(RESOURCES.read_text(encoding="utf-8"), TAIL_BEGIN, TAIL_END, tail_block())
    return wanted, [p for p in ours if p not in wanted]


def main(check: bool) -> int:
    wanted, stale = wanted_files()
    differ = [
        p
        for p, t in wanted.items()
        if not p.is_file() or p.read_text(encoding="utf-8") != t
    ]
    if check:
        for p in differ + stale:
            print(f"out of date: {p.relative_to(ROOT)}", file=sys.stderr)
        return 1 if differ or stale else 0
    for p in stale:
        p.unlink()
    for p in differ:
        p.write_text(wanted[p], encoding="utf-8")
    counts = ", ".join(f"{s['ns']} {len(store_rows(s))}" for s in STORES)
    print(f"sources: {counts}; {len(differ)} files written, {len(stale)} removed")
    return 0


if __name__ == "__main__":
    sys.exit(main("--check" in sys.argv[1:]))
