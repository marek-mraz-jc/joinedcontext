"""Validate candidate event sources of a city (T-5666).

Input: a TSV of `name<TAB>org_type<TAB>site` lines. For each site: robots.txt first, then the kinds the
bbsk-events templates read (The Events Calendar REST, iCal, schema.org Event JSON-LD on the home page and on
up to four event-looking pages it links). A source is `usable` when one kind lists at least one upcoming
event (start today or later) with a name, a start and a place; else the verdict names the one reason.
One request per second per host, the inventory User-Agent, a 15 s timeout and a 1 MB read cap per request.

    python3 seeds/city-events/probe.py candidates.tsv > probed.csv
"""
import csv
import datetime as dt
import json
import re
import ssl
import sys
import time
import urllib.request
import urllib.robotparser
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import urljoin, urlsplit

UA = "joinedcontext-platform/1.0 (+https://dev.joinedcontext.com; public events)"
CAP = 1_000_000
TODAY = dt.date.today().isoformat()
LINK = re.compile(r'href="([^"#]*(?:event|akce|akci|udalost|kalendar|podujat|program|tapahtum|kalenteri|calendar)[^"#]*)"', re.IGNORECASE)
#: A page under the home page that holds one event is that event's page, no list to read again tomorrow.
LIST_PATH = re.compile(r"/(events?|program[a-z0-9-]*|kalend[a-z]*|calendar|akce|podujatia|tapahtumat|udalosti)/?$", re.IGNORECASE)
FIELDS = ["name", "org_type", "site", "events_url", "check_url", "feed_kind", "upcoming", "complete",
          "robots", "read_on", "verdict", "reason"]


def get(url: str) -> tuple[str, str]:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "*/*"})
    with urllib.request.urlopen(req, timeout=15, context=ssl.create_default_context()) as r:
        return r.geturl(), r.read(CAP).decode("utf-8", "replace")


def upcoming(start) -> bool:
    return isinstance(start, str) and start[:10] >= TODAY and bool(re.match(r"\d{4}-\d\d-\d\d", start))


def ld_events(body: str) -> list[dict]:
    """Every schema.org Event of a page's JSON-LD."""
    out = []
    for block in re.findall(r"<script[^>]+application/ld\+json[^>]*>(.*?)</script>", body, re.DOTALL | re.IGNORECASE):
        try:
            stack = [json.loads(block.strip())]
        except ValueError:
            continue
        while stack:
            x = stack.pop()
            if isinstance(x, list):
                stack += x
            elif isinstance(x, dict):
                t = x.get("@type")
                if any(isinstance(i, str) and i.endswith("Event") for i in (t if isinstance(t, list) else [t])):
                    out.append(x)
                stack += [x[k] for k in ("@graph", "itemListElement", "item", "subEvent") if k in x]
    return out


def ld_counts(body: str) -> tuple[int, int]:
    every = ld_events(body)
    if not every:
        raise ValueError("no Event JSON-LD")
    ev = [e for e in every if upcoming(e.get("startDate"))]
    return len(ev), sum(1 for e in ev if e.get("name") and e.get("location"))


def tribe_counts(body: str) -> tuple[int, int]:
    ev = [e for e in json.loads(body).get("events", []) if upcoming(e.get("start_date"))]
    return len(ev), sum(1 for e in ev if e.get("title") and e.get("venue"))


def ical_counts(body: str) -> tuple[int, int]:
    if "BEGIN:VCALENDAR" not in body:
        raise ValueError("no VCALENDAR")
    n = full = 0
    for ev in re.findall(r"BEGIN:VEVENT(.*?)END:VEVENT", body.replace("\r\n ", ""), re.DOTALL):
        m = re.search(r"^DTSTART[^:\n]*:(\d{4})(\d\d)(\d\d)", ev, re.MULTILINE)
        if m and f"{m[1]}-{m[2]}-{m[3]}" >= TODAY:
            n += 1
            full += bool(re.search(r"^SUMMARY[^:\n]*:\S", ev, re.MULTILINE) and re.search(r"^(LOCATION[^:\n]*:\S|GEO:)", ev, re.MULTILINE))
    return n, full


def probe(line: str) -> dict:
    name, org_type, site = (line.rstrip("\n").split("\t") + ["", "", ""])[:3]
    row = dict.fromkeys(FIELDS, "") | {"name": name, "org_type": org_type, "site": site, "read_on": TODAY}
    try:
        final, home = get(site)
    except Exception as e:  # one word of reason per source
        return row | {"verdict": "skip", "reason": "down: " + type(e).__name__}
    base = re.match(r"https?://[^/]+", final)[0]
    if not base.startswith("https://"):
        return row | {"verdict": "skip", "reason": "http only"}
    rp = urllib.robotparser.RobotFileParser()
    try:
        rp.parse(get(base + "/robots.txt")[1].splitlines())
    except Exception:
        rp.parse([])
    row["robots"] = "y" if rp.can_fetch(UA, final) else "n"
    if row["robots"] == "n":
        return row | {"verdict": "skip", "reason": "robots.txt"}
    tries = [("jsonld", final, ld_counts)]
    for path, kind, count in (("/wp-json/tribe/events/v1/events?per_page=50&start_date=now", "tribe-rest", tribe_counts),
                              ("/?ical=1", "ical", ical_counts), ("/events/?ical=1", "ical", ical_counts)):
        tries.append((kind, base + path, count))
    host = urlsplit(base).hostname
    links = [urljoin(final, h) for h in LINK.findall(home)]
    links = list(dict.fromkeys(u for u in links if urlsplit(u).hostname == host and u.startswith("https://")))[:4]
    tries += [("jsonld", u, ld_counts) for u in links]
    tries += [("ical", urljoin(final, h), ical_counts) for h in re.findall(r'href="([^"]+\.ics)"', home)[:1]]
    best, parsed, single = None, False, False
    for kind, url, count in tries:
        if not rp.can_fetch(UA, url):
            continue
        try:
            body = home if url == final else (time.sleep(1), get(url))[1][1]
            n, full = count(body)
        except Exception:
            continue
        if kind == "jsonld" and url != final and n == 1 and not LIST_PATH.search(urlsplit(url).path):
            single = True
            continue
        parsed = True
        if n and (best is None or (full, n) > (best[2], best[1])):
            best = (kind, n, full, url)
    if best is None:
        return row | {"verdict": "skip", "reason": "none upcoming" if parsed else
                     "event pages only" if single else "no machine-readable events"}
    kind, n, full, url = best
    row |= {"feed_kind": kind, "check_url": url, "events_url": url.split("?")[0] if kind != "jsonld" else url,
            "upcoming": n, "complete": full}
    if not full:
        return row | {"verdict": "skip", "reason": "no place" if n else "none upcoming"}
    return row | {"verdict": "usable", "reason": f"{full} upcoming with name, start and place"}


if __name__ == "__main__":
    lines = [l for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
    w = csv.DictWriter(sys.stdout, FIELDS)
    w.writeheader()
    with ThreadPoolExecutor(12) as ex:
        for r in ex.map(probe, lines):
            w.writerow(r)
            sys.stdout.flush()
