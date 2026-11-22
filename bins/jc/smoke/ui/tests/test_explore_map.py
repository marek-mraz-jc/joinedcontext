"""The catalogue explorer's map when it cannot start (T-6737): python3 -m pytest bins/jc/smoke/ui/tests

The owner's dataset page showed an empty white box under the table: Chromium without WebGL 2 threw
"WebGL2 is required..." as an uncaught page error and drew nothing. These run vendor/viz/site as jc
serves it, against a stub Endpoint holding two events in Bratislava, and read what the box says.
"""

import gzip
import json
import os
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, quote, unquote

import pytest

ROOT = Path(__file__).resolve().parents[5]
VIZ = ROOT / "vendor" / "viz"
# The explorer bundle is the Portal UI's build, the copy bins/jc/build.rs embeds (T-7102).
EXPLORER = ROOT / "crates" / "portal" / "ui" / "dist-explorer"
SLUG = "a1b2c3d4e5f6a7b8"
EVENTS = [
    {"type": "Feature", "id": f"urn:ngsi-ld:Event:{n}", "geometry": {"type": "Point", "coordinates": at},
     "properties": {"type": "Event", "name": name}}
    # The second name is as long as the owner's record (T-6956): it squeezed the card's Close button.
    for n, at, name in ((1, [17.107, 48.144], "Event 1"),
                        (2, [17.112, 48.148], "Zmena priemerného veku v mestských častiach Bratislavy medzi rokom 2000 a 2021, Ružinov"))
]
NO_WEBGL = ["--disable-gpu", "--disable-webgl", "--disable-3d-apis"]
SHOTS = os.environ.get("SHOTS_DIR")
# bbsk-podujatia as the broker answered it on 2026-11-22 (T-7503): [position or null, starts today or later].
PODUJATIA = [
    {"type": "Feature", "id": f"urn:ngsi-ld:Event:p{n}", "geometry": at and {"type": "Point", "coordinates": at},
     "properties": {"type": "Event", "name": f"Podujatie {n}", "upcoming": later}}
    for n, (at, later) in enumerate(json.loads((Path(__file__).parent / "fixtures" / "bbsk_podujatia.json").read_text()))
]


def matching(events, q):
    """The stub broker's q: the tests ask `upcoming==true`, the map's counts add `!location`."""
    if "!location" in q:
        events = [e for e in events if not e["geometry"]]
    if "upcoming" in q:
        events = [e for e in events if e["properties"].get("upcoming")]
    return events


def in_view(events, asked):
    if "coordinates" not in asked:
        return events
    ring = json.loads(asked["coordinates"][0])[0]
    (w, s), (e, n) = ring[0], ring[2]
    return [f for f in events if f["geometry"] and w <= f["geometry"]["coordinates"][0] <= e and s <= f["geometry"]["coordinates"][1] <= n]


def serve(style_ok, events=EVENTS, cap=1000, points_ok=lambda: True, fields=()):
    # build.rs leaves empty placeholders without a UI build: an empty bundle is a missing one.
    missing = [n for n in ("explorer.js", "explorer.css") if not (EXPLORER / n).is_file() or not (EXPLORER / n).stat().st_size]
    if missing:
        pytest.fail(f"{EXPLORER} lacks {', '.join(missing)}: run `pnpm install --frozen-lockfile && pnpm build` in crates/portal/ui (and crates/portal/sdk) first", pytrace=False)
    """explore.html with jc's meta naming this server as api and tiles; `style_ok()` says whether the basemap style answers."""

    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, status, ctype, body, extra=None):
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            path, _, query = self.path.partition("?")
            own = f"http://127.0.0.1:{self.server.server_port}"
            api = f"/api/endpoint/{SLUG}/ngsi-ld/v1/"
            if path.startswith("/viz/"):
                rel = path[len("/viz/"):]
                if rel.startswith("explorer/"):
                    f = EXPLORER / rel[len("explorer/"):]
                else:
                    f = VIZ / rel if rel.startswith("vendor/") else VIZ / "site" / rel
                gz = f.with_name(f.name + ".gz")
                if not f.is_file() and not gz.is_file():
                    return self.send(404, "text/plain", b"")
                body = f.read_bytes() if f.is_file() else gzip.decompress(gz.read_bytes())
                if rel == "explore.html":
                    meta = json.dumps({"api": own, "tiles": own, "catalogue": own})
                    body = body.replace(b"<head>", f"<head><meta name=\"jc-viz\" content='{meta}'>".encode(), 1)
                ctype = {".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".mjs": "text/javascript"}.get(f.suffix, "application/octet-stream")
                return self.send(200, ctype + "; charset=utf-8", body)
            if path.startswith("/styles/"):
                if not style_ok():
                    return self.send(503, "text/plain", b"")
                style = {"version": 8, "sources": {}, "layers": [{"id": "bg", "type": "background", "paint": {"background-color": "#eef1f4"}}]}
                return self.send(200, "application/json", json.dumps(style).encode())
            if path == api + "types":
                types = [{"id": "Event", "type": "EntityType", "typeName": "Event", "attributeNames": ["location", "name"]}]
                return self.send(200, "application/json", json.dumps(types).encode())
            if path == api + "types/Event":
                info = {"id": "Event", "typeName": "Event", "entityCount": len(events), "attributeDetails": [
                    {"attributeName": "location", "attributeTypes": ["GeoProperty"]},
                    {"attributeName": "name", "attributeTypes": ["Property"]}]}
                return self.send(200, "application/json", json.dumps(info).encode())
            # The table's widget: no publisher defaults and no model, so it reads the types alone.
            if path == f"/api/endpoint/{SLUG}/widget.json":
                return self.send(200, "application/json", b"{}")
            if path == f"/api/endpoint/{SLUG}/schema/index.json":
                return self.send(200, "application/json", json.dumps({"endpoint": SLUG, "models": [{"name": "events", "version": 1, "types": ["Event"]}]}).encode())
            if path == f"/api/endpoint/{SLUG}/schema/v1/json-schema":
                props = {"name": {"type": "string", "x-ngsi-ld-kind": "Property"}, "location": {"type": "object", "x-ngsi-ld-kind": "GeoProperty"},
                         **{field: {"type": "string", "x-ngsi-ld-kind": "Property"} for field in fields}}
                return self.send(200, "application/json", json.dumps({"definitions": {"Event": {"type": "object", "properties": props}}}).encode())
            if path.startswith(api + "entities/"):
                id = unquote(path[len(api + "entities/"):])
                row = [{"id": e["id"], "type": "Event", **e["properties"], "location": e["geometry"]} for e in events if e["id"] == id]
                return self.send(200, "application/json", json.dumps(row[0]).encode()) if row else self.send(404, "application/json", b"{}")
            if path == api + "entities":
                asked = parse_qs(query)
                found = in_view(matching(events, asked.get("q", [""])[0]), asked)
                count = {"NGSILD-Results-Count": str(len(found))}
                start = int(asked.get("offset", ["0"])[0])
                page = found[start : start + min(cap, int(asked.get("limit", ["1000"])[0]))]
                if "application/geo+json" in (self.headers.get("Accept") or ""):
                    if not points_ok():
                        return self.send(503, "application/json", b"{}")
                    return self.send(200, "application/geo+json", json.dumps({"type": "FeatureCollection", "features": page}).encode(), count)
                rows = [{"id": e["id"], "type": "Event", **e["properties"], **({"location": e["geometry"]} if e["geometry"] else {})} for e in page]
                return self.send(200, "application/json", json.dumps(rows).encode(), count)
            return self.send(404, "application/json", b"{}")

    srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


@pytest.fixture(scope="module")
def pw():
    sync = pytest.importorskip("playwright.sync_api")
    with sync.sync_playwright() as p:
        yield p


def open_explorer(browser, srv, width, lang="en", q=""):
    page = browser.new_page(viewport={"width": width, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"http://127.0.0.1:{srv.server_port}/viz/explore.html#{SLUG}&type=Event&lang={lang}" + (f"&q={quote(q, safe='')}" if q else ""))
    page.wait_for_selector("#map-section:not([hidden])", timeout=20_000)
    # The map starts lazily, once it has stayed near the reader's view.
    page.locator("#map-section").scroll_into_view_if_needed()
    page.mouse.wheel(0, 2000)
    return page, errors


def shot(page, name):
    if SHOTS:
        Path(SHOTS).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=f"{SHOTS}/{name}.png", full_page=True)


# 300 is the frame on the owner's 360 px phone (the catalogue page's margins), where Close was cut.
@pytest.mark.parametrize("width, lang", [(300, "sk"), (360, "sk"), (400, "sk"), (1440, "en")])
def test_the_row_card_keeps_its_close_button_inside_and_fits_a_phone(pw, width, lang):
    """T-6956: on a phone the long title squeezed Close past the card's edge; the card is a sheet there."""
    browser = pw.chromium.launch(args=["--disable-dev-shm-usage", *NO_WEBGL])
    srv = serve(lambda: True)
    try:
        page, errors = open_explorer(browser, srv, width, lang)
        page.mouse.wheel(0, -4000)
        cell = page.locator('tbody tr[aria-rowindex="3"] .jc-grid-open').first  # the second event, the long name
        cell.click()
        card = page.locator("#detail")
        card.wait_for(state="visible", timeout=20_000)
        # The owner's record's type label: its longest word ("PERFORMANCE") set the header's least
        # width, and Close gave way to it.
        page.evaluate("() => { document.getElementById('detail-type').textContent = 'Key performance indicator'; }")
        page.wait_for_timeout(300)
        box = page.evaluate("""() => {
          const d = document.getElementById("detail"), c = document.getElementById("detail-close");
          const r = (e) => { const b = e.getBoundingClientRect(); return {l: b.left, t: b.top, r: b.right, b: b.bottom, w: b.width, h: b.height}; };
          return {card: r(d), close: r(c), pad: parseFloat(getComputedStyle(d).paddingRight), vw: innerWidth, vh: innerHeight,
                  cardSw: d.scrollWidth, cardCw: d.clientWidth, closeSw: c.scrollWidth, closeCw: c.clientWidth};
        }""")
        k, c = box["card"], box["close"]
        assert c["w"] >= 44 and c["h"] >= 44, box
        assert box["closeSw"] <= box["closeCw"], box  # the word "Zavrieť" whole, not cut by the card
        assert c["l"] >= k["l"] and c["t"] >= k["t"] and c["r"] <= k["r"] - box["pad"] + 0.5, box
        assert 0 <= c["l"] and c["r"] <= box["vw"] and c["t"] >= 0, box
        assert box["cardSw"] <= box["cardCw"], box  # no sideways scroll inside the card
        assert k["l"] >= 0 and k["r"] <= box["vw"] + 0.5 and k["t"] >= 0 and k["b"] <= box["vh"] + 0.5, box
        if width <= 640:
            # A sheet on the bottom of the screen, as wide as it.
            assert abs(k["w"] - box["vw"]) <= 1 and abs(k["b"] - box["vh"]) <= 1, box
        shot(page, f"row-card-{width}")
        page.keyboard.press("Escape")
        card.wait_for(state="hidden", timeout=5_000)
        assert page.evaluate("() => !!document.activeElement.closest('table')")
        cell.click()
        card.wait_for(state="visible", timeout=20_000)
        page.mouse.click(box["vw"] / 2, 4)  # the backdrop above the sheet or around the card
        card.wait_for(state="hidden", timeout=5_000)
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


@pytest.mark.parametrize("width, lang", [(1440, "en"), (400, "sk")])
def test_a_browser_without_webgl_reads_why_the_map_is_missing_and_no_page_error(pw, width, lang):
    browser = pw.chromium.launch(args=["--disable-dev-shm-usage", *NO_WEBGL])
    srv = serve(lambda: True)
    try:
        page, errors = open_explorer(browser, srv, width, lang)
        off = page.locator("#map-off")
        off.wait_for(state="visible", timeout=20_000)
        text = page.locator("#map-off-text").inner_text()
        assert "WebGL 2" in text and ("table above" in text or "Tabuľka vyššie" in text), text
        # No WebGL 2 is not something a retry fixes; the points line and Map settings go with the map.
        assert page.locator("#map-retry").is_hidden()
        for gone in ("#map", "#map-from", "#map-panel-open", "#map-count"):
            assert page.locator(gone).is_hidden(), gone
        assert not re.search(r"https?:|Error|at \w+ \(", text), text
        page.wait_for_timeout(500)
        shot(page, f"map-off-webgl-{width}")
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


@pytest.mark.parametrize("width", [1440, 400])
def test_a_map_whose_style_fails_offers_a_retry_that_draws_it(pw, width):
    browser = pw.chromium.launch(args=["--disable-dev-shm-usage", "--enable-unsafe-swiftshader", "--use-angle=swiftshader"])
    ok = {"style": False}
    srv = serve(lambda: ok["style"])
    try:
        page, errors = open_explorer(browser, srv, width)
        if not page.evaluate("() => !!document.createElement('canvas').getContext('webgl2')"):
            pytest.skip("this Chromium has no software WebGL 2")
        page.locator("#map-off").wait_for(state="visible", timeout=20_000)
        assert page.locator("#map-off-text").inner_text().startswith("The map could not load.")
        retry = page.locator("#map-retry")
        assert retry.is_visible()
        shot(page, f"map-off-start-{width}")
        ok["style"] = True
        retry.click()
        page.wait_for_selector("#map .maplibregl-canvas", timeout=20_000)
        page.wait_for_function("() => window.jcExploreMap && window.jcExploreMap.getSource('jc-pts')", timeout=20_000)
        assert page.locator("#map-off").is_hidden()
        assert page.locator("#map-from").is_visible() and page.locator("#map-panel-open").is_visible()
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


WEBGL = ["--disable-dev-shm-usage", "--use-gl=swiftshader", "--enable-unsafe-swiftshader"]
# The drawn badges and the view, in the map's own pixels.
DRAWN = """() => {
  const m = self.jcExploreMap;
  const badges = m.getSource("jc-pts").serialize().data.features.map((f) => {
    const p = m.project(f.geometry.coordinates);
    return {x: p.x, y: p.y, r: f.properties.jcR, n: f.properties.jcStack};
  });
  const c = m.getContainer();
  return {badges, bounds: m.getBounds().toArray(), w: c.clientWidth, h: c.clientHeight};
}"""


def wait_drawn(page):
    page.wait_for_function("() => self.jcExploreMap && !document.getElementById('map-count').hidden && self.jcExploreMap.loaded()", timeout=40_000)


def overlaps(badges):
    return [(a, b) for i, a in enumerate(badges) for b in badges[i + 1:]
            if abs(a["x"] - b["x"]) < a["r"] + b["r"] and abs(a["y"] - b["y"]) < a["r"] + b["r"]]


@pytest.mark.parametrize("width, lang", [(1440, "en"), (400, "sk")])
@pytest.mark.parametrize("case", ["all", "default-filter", "all-located", "none-located", "bratislava"])
def test_the_map_opens_on_every_point_with_true_counts_and_apart_badges(pw, width, lang, case):
    """T-7503: the owner's page read "49 of 200" beside 234 Events, the badges one on another."""
    events = {
        "all-located": [e for e in PODUJATIA if e["geometry"]],
        "none-located": [{**e, "geometry": None} for e in PODUJATIA[:7]],
        "bratislava": EVENTS,
    }.get(case, PODUJATIA)
    q = "upcoming==true" if case == "default-filter" else ""
    shown = matching(events, q)
    points = [e["geometry"]["coordinates"] for e in shown if e["geometry"]]
    n, none = len(points), len(shown) - len(points)
    browser = pw.chromium.launch(args=WEBGL)
    srv = serve(lambda: True, events)
    try:
        page, errors = open_explorer(browser, srv, width, lang, q)
        if not points:
            # No row has a place: the reason stands in the map's box, with no empty map above it.
            page.wait_for_function("() => document.getElementById('map-section').classList.contains('map-empty')", timeout=40_000)
            note = page.locator("#map-note").inner_text()
            assert note.startswith({"en": f"None of the {none} records", "sk": f"Žiadny zo záznamov tohto dopytu ({none}) nemá polohu"}[lang])
            assert page.locator("#map").is_hidden() and page.locator("#map-count").is_hidden()
            shot(page, f"map-{case}-open-{width}")
            assert errors == []
            return
        wait_drawn(page)
        line = {"en": f"{n} in view · {n} of {n + none} with a location · {none} without", "sk": f"Vo výreze: {n} · s polohou: {n} z {n + none} · bez polohy: {none}"}[lang]
        assert page.locator("#map-count").inner_text() == line
        of = page.locator("#grid-count")
        if of.is_visible():
            assert not page.locator(".jc-grid-pager .jc-grid-total").is_visible()  # one count, named against its total
        if q:
            of.wait_for(state="visible", timeout=10_000)
            assert of.inner_text() == {"en": "200 of 234 records match the filter.", "sk": "Filtru vyhovuje 200 z 234 záznamov."}[lang]
        else:
            assert of.is_hidden()
        view = page.evaluate(DRAWN)
        (west, south), (east, north) = view["bounds"]
        assert all(west <= x <= east and south <= y <= north for x, y in points), view["bounds"]
        assert sum(b["n"] for b in view["badges"]) == n
        assert all(b["r"] <= b["x"] <= view["w"] - b["r"] and b["r"] <= b["y"] <= view["h"] - b["r"] for b in view["badges"]), view
        assert overlaps(view["badges"]) == []
        page.wait_for_timeout(800)
        shot(page, f"map-{case}-open-{width}")
        zoom = page.evaluate("() => self.jcExploreMap.getZoom()")
        for step in (1, 1.6, 3):
            page.evaluate("(z) => self.jcExploreMap.jumpTo({zoom: z})", zoom + step)
            page.wait_for_function("() => self.jcExploreMap.loaded()", timeout=20_000)
            view = page.evaluate(DRAWN)
            assert sum(b["n"] for b in view["badges"]) == n
            assert overlaps(view["badges"]) == [], step
            page.wait_for_timeout(500)
            if step == 1:
                shot(page, f"map-{case}-zoomed-{width}")
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


def test_a_read_that_stops_short_says_first_n_of_the_total(pw):
    browser = pw.chromium.launch(args=WEBGL)
    srv = serve(lambda: True, PODUJATIA, cap=200)
    try:
        page, errors = open_explorer(browser, srv, 1440)
        wait_drawn(page)
        assert page.locator("#map-note").inner_text() == "The map draws up to 200 records at a time; zoom in for the rest."
        drawn = sum(1 for e in PODUJATIA[:200] if e["geometry"])
        tail = f"{drawn} of 231 with a location drawn · 3 without"
        page.wait_for_function("(tail) => document.getElementById('map-count').textContent.endsWith(tail)", arg=tail, timeout=10_000)
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


def test_a_query_matching_nothing_says_so_in_place_of_the_map(pw):
    browser = pw.chromium.launch(args=WEBGL)
    srv = serve(lambda: True, [{**e, "properties": {**e["properties"], "upcoming": False}} for e in PODUJATIA])
    try:
        page, errors = open_explorer(browser, srv, 400, q="upcoming==true")
        page.wait_for_function("() => document.getElementById('map-section').classList.contains('map-empty')", timeout=40_000)
        assert page.locator("#map-note").inner_text() == "No record matches this query, so there is nothing to map."
        shot(page, "map-no-match-400")
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


@pytest.mark.parametrize("width", [1440, 400])
def test_a_failed_read_of_the_points_shows_no_empty_map_and_retries(pw, width):
    browser = pw.chromium.launch(args=WEBGL)
    ok = {"points": False}
    srv = serve(lambda: True, PODUJATIA, points_ok=lambda: ok["points"])
    try:
        page, errors = open_explorer(browser, srv, width)
        page.locator("#map-off").wait_for(state="visible", timeout=40_000)
        assert page.locator("#map").is_hidden()
        assert "503" in page.locator("#map-off-text").inner_text()
        shot(page, f"map-read-failed-{width}")
        ok["points"] = True
        page.locator("#map-retry").click()
        page.wait_for_selector("#map", state="visible", timeout=20_000)
        wait_drawn(page)
        assert page.locator("#map-off").is_hidden()
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


def test_a_query_with_places_after_one_without_fits_its_points_again(pw):
    browser = pw.chromium.launch(args=WEBGL)
    placeless = [{**e, "geometry": e["geometry"] if not e["properties"]["upcoming"] else None} for e in PODUJATIA]
    srv = serve(lambda: True, placeless)
    try:
        page, errors = open_explorer(browser, srv, 1440, q="upcoming==true")
        page.wait_for_function("() => document.getElementById('map-section').classList.contains('map-empty')", timeout=40_000)
        page.get_by_role("button", name=re.compile("Clear (all )?filters", re.IGNORECASE)).first.click()
        page.wait_for_selector("#map", state="visible", timeout=20_000)
        wait_drawn(page)
        view = page.evaluate(DRAWN)
        (west, south), (east, north) = view["bounds"]
        points = [e["geometry"]["coordinates"] for e in placeless if e["geometry"]]
        assert view["w"] > 0 and all(west <= x <= east and south <= y <= north for x, y in points), view["bounds"]
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


@pytest.mark.parametrize("width", [1440, 400])
def test_the_table_is_one_card_led_by_the_name_and_cuts_no_column(pw, width):
    """T-7503, the owner's "this formating is odd": two button styles, a band for one button, a blank filter row, the id first."""
    browser = pw.chromium.launch(args=["--disable-dev-shm-usage", *NO_WEBGL])
    srv = serve(lambda: True)
    try:
        page, errors = open_explorer(browser, srv, width)
        page.wait_for_selector("#grid tbody .jc-grid-open", timeout=20_000)
        look = page.evaluate("""() => {
          const scroll = document.querySelector(".jc-grid-scroll"), bar = document.querySelector('[data-testid="grid-toolbar"]');
          return {first: document.querySelector("#grid thead th").innerText, heads: document.querySelector("#grid thead tr").innerText,
                  band: !!document.querySelector(".jc-grid .jc-grid-toolbar"), filterRow: !!document.querySelector(".jc-grid-filter-row"),
                  arrange: !!bar.querySelector(".jc-grid-arrange-toggle"), select: !!bar.querySelector(":scope select"),
                  borders: new Set([...bar.querySelectorAll(":scope > details > summary, :scope > button")].map((b) => getComputedStyle(b).borderTopStyle + getComputedStyle(b).borderTopWidth)).size,
                  frame: getComputedStyle(document.querySelector(".jc-grid")).borderTopWidth,
                  sw: scroll.scrollWidth, cw: scroll.clientWidth,
                  cut: [...document.querySelectorAll("#grid tbody td .jc-grid-open")].filter((b) => b.scrollWidth > b.clientWidth + 1).length};
        }""")
        assert look["first"].lower().startswith("name") and not re.search(r"\bID\b|\bType\b", look["heads"]), look
        assert not look["band"] and not look["filterRow"] and look["arrange"], look
        assert not look["select"], look  # one type: nothing to pick
        assert look["borders"] <= 1 and look["frame"] == "0px", look
        assert look["sw"] <= look["cw"] and look["cut"] == 0, look
        shot(page, f"table-{width}")
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


@pytest.mark.parametrize("width", [360, 400])
def test_a_phone_reads_at_body_size_counts_the_readers_hides_and_breaks_no_word(pw, width):
    """T-7503, the review's four phone findings: 10 px labels, a badge of the phone's own hides, a second menu glyph, "Dokumentarn/y"."""
    browser = pw.chromium.launch(args=["--disable-dev-shm-usage", *NO_WEBGL])
    said = {"name": "Dokumentárny film ,,ZÓNY NIKOHO“", "start": "23. 11. 2026, 18:00",
            "organizer": "Fakulta politických vied a medzinárodných vzťahov Univerzity Mateja Bela"}
    # The table reads normalized rows.
    events = [{**event, "properties": {field: {"type": "Property", "value": value} for field, value in said.items()}} for event in EVENTS]
    srv = serve(lambda: True, events, fields=("start", "organizer"))
    try:
        page, errors = open_explorer(browser, srv, width)
        page.wait_for_selector("#grid tbody .jc-grid-open", timeout=20_000)
        bar = page.locator('[data-testid="grid-toolbar"]')
        body = page.evaluate("() => parseFloat(getComputedStyle(document.body).fontSize)")
        sizes = page.evaluate("""() => [...document.querySelectorAll('[data-testid="grid-toolbar"] :is(summary, button, input, select, label, legend), #map-panel-open')]
                                 .map((e) => [e.textContent.trim().slice(0, 30), parseFloat(getComputedStyle(e).fontSize)])""")
        assert [each for each in sizes if each[1] < body] == []
        tools = bar.locator(":scope > details > summary")
        assert tools.inner_text().strip() == "Tools"  # the phone hid two columns, the reader none
        icon = page.evaluate("() => parseFloat(getComputedStyle(document.querySelector('.jc-arrange-slot .jc-grid-menu-icon')).fontSize)")
        assert icon == 0  # a drawn columns icon, not the menu's three lines
        shot(page, f"phone-open-{width}")
        tools.click()
        bar.get_by_text("Hide fields", exact=True).click()
        bar.get_by_label(re.compile("^organizer$", re.I)).check()
        page.wait_for_function("() => document.querySelectorAll('#grid tbody tr:first-child td').length >= 3")
        assert tools.inner_text().strip() == "Tools"
        shot(page, f"phone-menu-{width}")
        page.keyboard.press("Escape")
        page.evaluate("() => document.querySelectorAll('[data-testid=grid-toolbar] details[open]').forEach((d) => (d.open = false))")
        look = page.evaluate("""() => {
          const name = document.querySelector("#grid tbody .jc-grid-open"), scroll = document.querySelector(".jc-grid-scroll");
          const text = document.createTreeWalker(name, NodeFilter.SHOW_TEXT).nextNode(), range = document.createRange(), broken = [];
          for (const word of text.data.matchAll(/\\S+/g)) {
            range.setStart(text, word.index); range.setEnd(text, word.index + word[0].length);
            if (new Set([...range.getClientRects()].map((r) => Math.round(r.top))).size > 1) broken.push(word[0]);
          }
          return {broken, name: name.closest("td").getBoundingClientRect().width, sw: scroll.scrollWidth, cw: scroll.clientWidth,
                  cells: [...new Set([...document.querySelectorAll("#grid tbody td")].map((td) => getComputedStyle(td).backgroundColor))]};
        }""")
        assert look["broken"] == [] and look["name"] >= 120 and look["sw"] > look["cw"], look
        assert look["cells"] == ["rgba(0, 0, 0, 0)"], look  # the edge's shadow shows through every row
        shot(page, f"phone-restored-{width}")
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()


def test_the_wheel_scrolls_the_page_and_only_ctrl_with_it_zooms_the_map(pw):
    """T-7503: a reader scrolling past the map found it zoomed into Banská Bystrica, "49 of 200"."""
    browser = pw.chromium.launch(args=WEBGL)
    srv = serve(lambda: True, PODUJATIA)
    try:
        page, errors = open_explorer(browser, srv, 1440)
        wait_drawn(page)
        page.locator("#map").scroll_into_view_if_needed()
        box = page.locator("#map").bounding_box()
        page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        zoom, top = page.evaluate("() => [self.jcExploreMap.getZoom(), scrollY]")
        for _ in range(4):
            page.mouse.wheel(0, -240)
            page.wait_for_timeout(100)
        page.wait_for_timeout(700)
        after, moved = page.evaluate("() => [self.jcExploreMap.getZoom(), scrollY]")
        assert after == zoom and moved < top
        page.locator("#map").scroll_into_view_if_needed()
        box = page.locator("#map").bounding_box()
        page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        page.keyboard.down("Control")
        page.mouse.wheel(0, -240)
        page.keyboard.up("Control")
        page.wait_for_function("(z) => self.jcExploreMap.getZoom() > z", arg=zoom, timeout=5_000)
        assert errors == []
    finally:
        browser.close()
        srv.shutdown()
