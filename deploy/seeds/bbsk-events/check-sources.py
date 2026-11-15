#!/usr/bin/env python3
"""Re-fetch every kept source of sources.csv and check its format still parses.

One GET per row (plus robots.txt once per host), 1 s apart, with a User-Agent that names the
platform. A row whose robots.txt refuses our agent is reported, never fetched. Exit 1 when a kept
row no longer parses or robots.txt now refuses it, so T-4896 sees a source that changed shape; an
empty but well-formed feed is reported as "empty" and does not fail (towns post events in waves).

    python3 seeds/bbsk-events/check-sources.py [--all] [sources.csv]

--all also checks the skip rows (useful when re-reading the inventory).
"""

import csv
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import urllib.robotparser
import xml.etree.ElementTree as ET
from datetime import date
from pathlib import Path

UA = "joinedcontext-platform-events-inventory/1.0 (+https://dev.joinedcontext.com; open data inventory, BBSK)"
MAX_BYTES = 4_000_000
TIMEOUT = 20


class SameHostRedirect(urllib.request.HTTPRedirectHandler):
    """Follows a redirect only to https on the host asked: robots.txt was read for that host alone."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        to = urllib.parse.urlsplit(urllib.parse.urljoin(req.full_url, newurl))
        if to.scheme != "https" or to.hostname != urllib.parse.urlsplit(req.full_url).hostname:
            raise urllib.error.HTTPError(req.full_url, code, f"redirect off the host to {to.scheme}://{to.hostname}", headers, fp)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


OPENER = urllib.request.build_opener(SameHostRedirect)


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with OPENER.open(req, timeout=TIMEOUT) as r:
        body = r.read(MAX_BYTES + 1)
        if len(body) > MAX_BYTES:
            raise ValueError(f"the answer is larger than {MAX_BYTES} bytes, more than a source page should be")
        return r.headers.get("content-type", ""), body.decode("utf-8", "replace")


def today():
    return date.today().isoformat()


# Each parser returns (events seen, events with a GPS point) or raises when the format changed.
def wp_rest_sc(body):
    items = json.loads(body)
    up = [
        i for i in items if (i.get("meta_box") or {}).get("date_start", "") >= today()
    ]
    geo = [i for i in up if ((i["meta_box"].get("coordinates") or {}).get("latitude"))]
    return len(up), len(geo)


def tribe_rest(body):
    d = json.loads(body)
    ev = d["events"]
    return len(ev), sum(1 for e in ev if (e.get("venue") or {}).get("geo_lat"))


def wp_rest_list(body):
    items = json.loads(body)
    if not isinstance(items, list) or (items and "link" not in items[0]):
        raise ValueError("not a WordPress REST post list")
    return len(items), 0


def feed(body):
    root = ET.fromstring(body.encode("utf-8"))
    n = sum(1 for el in root.iter() if el.tag.rsplit("}", 1)[-1] in ("item", "entry"))
    return n, 0


def ical(body):
    if not body.lstrip().startswith("BEGIN:VCALENDAR"):
        raise ValueError("no VCALENDAR")
    return body.count("BEGIN:VEVENT"), body.count("\nGEO:")


def html_links(pattern):
    def parse(body):
        return len(set(re.findall(pattern, body))), 0

    return parse


PARSERS = {
    "wp-rest-sc": wp_rest_sc,
    "tribe-rest": tribe_rest,
    "mec-rest": wp_rest_list,
    "weby-feed": feed,
    "wp-rss": feed,
    "news-rss": feed,
    "ical": ical,
}


def robots_ok(url, cache):
    p = urllib.parse.urlsplit(url)
    base = f"{p.scheme}://{p.netloc}"
    if base not in cache:
        rp = urllib.robotparser.RobotFileParser(base + "/robots.txt")
        try:
            _, txt = fetch(base + "/robots.txt")
            rp.parse(txt.splitlines())
        except urllib.error.HTTPError as e:
            # RFC 9309: 4xx means no rules; 5xx means assume disallowed.
            rp.parse([] if e.code < 500 else ["User-agent: *", "Disallow: /"])
        cache[base] = rp
        time.sleep(1)
    return cache[base].can_fetch(UA, url)


def check(row, cache):
    url, fmt = row["check_url"], row["feed_kind"]
    if not url:
        return "skip", "no check_url"
    try:
        allowed = robots_ok(url, cache)
    except OSError as e:
        return "fail", f"robots.txt unreachable: {e}"
    if not allowed:
        return "fail", "robots.txt disallows"
    parser = PARSERS.get(fmt) or (
        html_links(row["item_pattern"]) if row["item_pattern"] else None
    )
    if parser is None:
        return "fail", f"no parser for feed_kind {fmt}"
    try:
        _, body = fetch(url)
        n, geo = parser(body)
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as e:
        return "fail", f"{type(e).__name__}: {str(e)[:80]}"
    finally:
        time.sleep(1)
    return ("ok" if n > 0 else "empty"), f"events={n} geo={geo}"


def main(argv):
    every = "--all" in argv
    args = [a for a in argv if a != "--all"]
    path = Path(args[0]) if args else Path(__file__).with_name("sources.csv")
    cache, bad = {}, 0
    with path.open(newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            kept = row["decision"].startswith("pipeline")
            if not (kept or every) or not row["check_url"]:
                continue
            status, note = check(row, cache)
            if kept and status == "fail":
                bad += 1
            print(
                f"{status:5} {row['town']:<22} {row['feed_kind']:<11} {note}  {row['check_url']}",
                flush=True,
            )
    print(f"checked {date.today()}: {bad} kept source(s) failing")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
