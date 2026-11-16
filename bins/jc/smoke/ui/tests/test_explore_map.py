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
from urllib.parse import unquote

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


def serve(style_ok):
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
                info = {"id": "Event", "typeName": "Event", "entityCount": 2, "attributeDetails": [
                    {"attributeName": "location", "attributeTypes": ["GeoProperty"]},
                    {"attributeName": "name", "attributeTypes": ["Property"]}]}
                return self.send(200, "application/json", json.dumps(info).encode())
            # The table's widget: no publisher defaults and no model, so it reads the types alone.
            if path == f"/api/endpoint/{SLUG}/widget.json":
                return self.send(200, "application/json", b"{}")
            if path == f"/api/endpoint/{SLUG}/schema/index.json":
                return self.send(200, "application/json", json.dumps({"endpoint": SLUG, "models": [{"name": "events", "version": 1, "types": ["Event"]}]}).encode())
            if path == f"/api/endpoint/{SLUG}/schema/v1/json-schema":
                props = {"name": {"type": "string", "x-ngsi-ld-kind": "Property"}, "location": {"type": "object", "x-ngsi-ld-kind": "GeoProperty"}}
                return self.send(200, "application/json", json.dumps({"definitions": {"Event": {"type": "object", "properties": props}}}).encode())
            if path.startswith(api + "entities/"):
                id = unquote(path[len(api + "entities/"):])
                row = [{"id": e["id"], "type": "Event", **e["properties"], "location": e["geometry"]} for e in EVENTS if e["id"] == id]
                return self.send(200, "application/json", json.dumps(row[0]).encode()) if row else self.send(404, "application/json", b"{}")
            if path == api + "entities":
                count = {"NGSILD-Results-Count": str(len(EVENTS))}
                if "application/geo+json" in (self.headers.get("Accept") or ""):
                    fc = {"type": "FeatureCollection", "features": EVENTS[: 0 if "limit=0" in query else None]}
                    return self.send(200, "application/geo+json", json.dumps(fc).encode(), count)
                rows = [] if "limit=0" in query else [{"id": e["id"], "type": "Event", **e["properties"], "location": e["geometry"]} for e in EVENTS]
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


def open_explorer(browser, srv, width, lang="en"):
    page = browser.new_page(viewport={"width": width, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(f"http://127.0.0.1:{srv.server_port}/viz/explore.html#{SLUG}&type=Event&lang={lang}")
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
