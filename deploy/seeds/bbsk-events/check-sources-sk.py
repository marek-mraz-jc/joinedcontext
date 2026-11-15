#!/usr/bin/env python3
"""Re-fetch every kept source of sources-sk.csv (Slovakia outside BBSK, T-4899) with check-sources.py.

Same rules as check-sources.py (one GET per row, robots.txt first, 1 s apart, named User-Agent); this
file only adds the feed kinds the national inventory found beyond BBSK:

- wp-rest: a WordPress custom post type list (/wp-json/wp/v2/<type>), same shape as mec-rest
- eventon-rest: EventON's ajde_events post type list, same shape
- lsvr-rest: the LSVR Townpress theme's lsvr_event post type list, same shape (dates in each event page's JSON-LD)
- joomla-rss: a Joomla category feed (?format=feed&type=rss), plain RSS like wp-rss
- jsonld: schema.org Event objects in the page's application/ld+json blocks

    python3 seeds/bbsk-events/check-sources-sk.py [--all] [sources-sk.csv]
    python3 seeds/bbsk-events/check-sources-sk.py --selftest   # the jsonld parser, offline
"""

import importlib.util
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "check_sources", HERE / "check-sources.py"
)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
base.UA = base.UA.replace("BBSK", "Slovakia")


def jsonld(body):
    """Count schema.org Event objects (any *Event type, also inside @graph) and those with a geo point."""
    n = geo = 0
    for block in re.findall(
        r"<script[^>]+application/ld\+json[^>]*>(.*?)</script>",
        body,
        re.DOTALL | re.IGNORECASE,
    ):
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
                if any(
                    isinstance(i, str) and i.endswith("Event")
                    for i in (t if isinstance(t, list) else [t])
                ):
                    n += 1
                    loc = x.get("location")
                    geo += isinstance(loc, dict) and bool(loc.get("geo"))
                stack += [
                    x[k]
                    for k in ("@graph", "itemListElement", "item", "subEvent")
                    if k in x
                ]
    if n == 0 and "application/ld+json" not in body:
        raise ValueError("no JSON-LD in the page")
    return n, geo


base.PARSERS.update(
    {
        "wp-rest": base.wp_rest_list,
        "eventon-rest": base.wp_rest_list,
        "lsvr-rest": base.wp_rest_list,
        "joomla-rss": base.feed,
        "jsonld": jsonld,
    }
)


def selftest():
    page = (
        '<script type="application/ld+json">{"@graph": [{"@type": "MusicEvent", '
        '"location": {"geo": {"latitude": 48.1}}}, {"@type": "Event"}]}</script>'
        '<script type="application/ld+json">{"@type": "BreadcrumbList"}</script>'
    )
    assert jsonld(page) == (2, 1)
    assert jsonld('<script type="application/ld+json">not json</script>') == (0, 0)
    try:
        jsonld("<html></html>")
    except ValueError:
        return
    raise AssertionError("a page without JSON-LD must fail")


if __name__ == "__main__":
    args = sys.argv[1:]
    if args == ["--selftest"]:
        sys.exit(selftest())
    if not [a for a in args if a != "--all"]:
        args.append(str(HERE / "sources-sk.csv"))
    sys.exit(base.main(args))
