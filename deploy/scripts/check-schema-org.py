#!/usr/bin/env python3
"""Is every dataset page findable as a schema.org Dataset? (T-5123)

usage: check-schema-org.py <catalogue-url>        e.g. https://opendata.dev.joinedcontext.com
       check-schema-org.py --file <page.html or record.json>...

Reads /robots.txt, the sitemap it names, then every /dataset/ page the sitemap lists, takes the
page's `application/ld+json` and checks it as a schema.org Dataset: the fields Google's dataset
search requires (name, a description of 50 to 5000 characters) and the ones this catalogue
promises (license, creator, distribution with contentUrl and encodingFormat, spatialCoverage,
temporalCoverage). One line per page. Exits 1 on any problem, on a page without the record and
on a sitemap that lists no dataset: an empty sitemap has nothing wrong in it and finds nothing.
"""

import json
import re
import sys
import urllib.request
from html.parser import HTMLParser
from pathlib import Path
from xml.etree import ElementTree

URL = re.compile(r"^https?://[^\s<>\"]+$")
# A date, a date-time or an interval of them; ".." is an open end (ISO 8601-2, schema.org).
WHEN = r"\d{4}(-\d{2}(-\d{2}(T[0-9:.]+(Z|[+-]\d{2}:?\d{2})?)?)?)?"
INTERVAL = re.compile(rf"^({WHEN}|\.\.)(/({WHEN}|\.\.))?$")
SITEMAP = "{http://www.sitemaps.org/schemas/sitemap/0.9}"


def _text(value):
    return isinstance(value, str) and value.strip() != ""


def problems(record) -> list[str]:
    """What is wrong with one record; empty when it is a valid Dataset."""
    if not isinstance(record, dict):
        return ["not a JSON object"]
    out = []
    if record.get("@context") not in ("https://schema.org/", "https://schema.org", "http://schema.org/"):
        out.append("@context is not schema.org")
    if record.get("@type") != "Dataset":
        out.append("@type is not Dataset")
    if not _text(record.get("name")):
        out.append("name missing")
    description = record.get("description")
    if not _text(description) or not 50 <= len(description) <= 5000:
        out.append("description missing or not 50 to 5000 characters")
    if not (isinstance(record.get("license"), str) and URL.match(record["license"])):
        out.append("license missing or not a URL")
    creator = record.get("creator")
    if not (isinstance(creator, dict) and creator.get("@type") in ("Organization", "Person") and _text(creator.get("name"))):
        out.append("creator missing or without @type and name")
    distribution = record.get("distribution")
    if not isinstance(distribution, list) or not distribution:
        out.append("distribution missing")
    else:
        for index, item in enumerate(distribution):
            if not (isinstance(item, dict) and item.get("@type") == "DataDownload"
                    and isinstance(item.get("contentUrl"), str) and URL.match(item["contentUrl"])
                    and _text(item.get("encodingFormat"))):
                out.append(f"distribution[{index}] is not a DataDownload with contentUrl and encodingFormat")
    place = record.get("spatialCoverage")
    if not (isinstance(place, dict) and place.get("@type") == "Place" and (_text(place.get("name")) or place.get("geo"))):
        out.append("spatialCoverage missing or not a named Place")
    if not (isinstance(record.get("temporalCoverage"), str) and INTERVAL.match(record["temporalCoverage"])):
        out.append("temporalCoverage missing or not an ISO 8601 date or interval")
    return out


class _Records(HTMLParser):
    def __init__(self):
        super().__init__()
        self.records, self._inside = [], False

    def handle_starttag(self, tag, attrs):
        self._inside = tag == "script" and dict(attrs).get("type") == "application/ld+json"
        if self._inside:
            self.records.append("")

    def handle_endtag(self, tag):
        self._inside = False

    def handle_data(self, data):
        if self._inside:
            self.records[-1] += data


def records_of(page: str) -> list:
    """Every schema.org Dataset in a page's `application/ld+json` scripts."""
    parser = _Records()
    parser.feed(page)
    found = []
    for text in parser.records:
        try:
            value = json.loads(text)
        except ValueError:
            found.append(None)
            continue
        for item in value if isinstance(value, list) else [value]:
            if isinstance(item, dict) and item.get("@type") == "Dataset":
                found.append(item)
    return found


def check_page(page: str) -> list[str]:
    found = records_of(page)
    if len(found) != 1:
        return [f"{len(found)} schema.org Dataset records, expected 1"]
    return problems(found[0]) if found[0] is not None else ["the ld+json is not JSON"]


def _get(url: str) -> str:
    request = urllib.request.Request(url, headers={"User-Agent": "joinedcontext-schema-org-check"})
    with urllib.request.urlopen(request, timeout=60) as answer:
        return answer.read(20_000_000).decode("utf-8")


def sitemap_datasets(site: str) -> list[str]:
    robots = _get(site + "/robots.txt")
    named = re.findall(r"(?im)^sitemap:\s*(\S+)", robots)
    if named != [site + "/sitemap.xml"]:
        raise SystemExit(f"robots.txt names {named or 'no sitemap'}, expected {site}/sitemap.xml")
    root = ElementTree.fromstring(_get(named[0]))
    return [loc.text for loc in root.iter(SITEMAP + "loc") if "/dataset/" in (loc.text or "")]


def main(argv: list[str]) -> int:
    if argv[:1] == ["--file"] and argv[1:]:
        pages = [(name, Path(name).read_text(encoding="utf-8")) for name in argv[1:]]
        results = [(name, problems(json.loads(text)) if name.endswith(".json") else check_page(text)) for name, text in pages]
    elif len(argv) == 1:
        urls = sitemap_datasets(argv[0].rstrip("/"))
        if not urls:
            print("the sitemap lists no dataset")
            return 1
        results = [(url, check_page(_get(url))) for url in urls]
    else:
        print(__doc__.split("\n\n")[1])
        return 2
    for name, found in results:
        print(f"{name}: {'ok' if not found else '; '.join(found)}")
    failed = sum(1 for _, found in results if found)
    print(f"{len(results)} pages, {failed} with problems")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
