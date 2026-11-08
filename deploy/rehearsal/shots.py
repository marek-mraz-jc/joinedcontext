#!/usr/bin/env python3
"""Screenshots of the rehearsal's main pages at 1440 and 400 px, signed in as a test person.

  python3 shots.py --base http://localhost:42101 --state <home>/state --out <dir> [--project helsinki]

Writes <out>/<page>-<width>.png and <out>/shots.json (page, width, HTTP status, h1, console errors).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import rehearsal as rh  # noqa: E402
from playwright.sync_api import sync_playwright  # noqa: E402

PAGES = [
    ("home", "/"),
    ("project", "/projects/{p}"),
    ("project-home", "/projects/{p}/home"),
    ("models", "/projects/{p}/models"),
    ("explore", "/projects/{p}/explore"),
    ("activity", "/projects/{p}/activity"),
    ("health", "/projects/{p}/health"),
    ("dashboards", "/projects/{p}/grafana"),
    ("catalogue", "/catalogue"),
    ("endpoints", "/endpoints"),
    ("organization", "/organization"),
    ("dataroom", "/opendata/"),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--state", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--project", default="helsinki")
    a = ap.parse_args()
    rh.loopback_only(a.base)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    h = rh.Http(a.base)
    token = rh.admin_token(h, Path(a.state))
    _, password = rh.make_person(h, token, "jc", "rehearsal-shots", groups=["/platform-admins"])
    rows = []
    with sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage", "--disable-gpu"])
        for width, height in ((1440, 900), (400, 860)):
            ctx = browser.new_context(viewport={"width": width, "height": height})
            page = ctx.new_page()
            errors: list[str] = []
            page.on("response", lambda r: errors.append(f"{r.status} {r.request.method} {r.url.removeprefix(a.base)}"[:200])
                    if r.status >= 400 else None)
            page.goto(a.base + "/api/v1/auth/login")
            page.fill("input[name=username]", "rehearsal-shots")
            page.fill("input[name=password]", password)
            page.locator("input[name=password]").press("Enter")
            page.wait_for_url(re.compile(re.escape(a.base) + r"/(?!id/).*"), timeout=30000)
            for name, path in PAGES:
                errors.clear()
                resp = page.goto(a.base + path.format(p=a.project), wait_until="networkidle", timeout=60000)
                page.wait_for_timeout(1500)
                file = out / f"{name}-{width}.png"
                page.screenshot(path=str(file), full_page=False)
                h1 = page.locator("h1").first.inner_text(timeout=2000) if page.locator("h1").count() else ""
                rows.append({"page": name, "path": path.format(p=a.project), "width": width,
                             "status": resp.status if resp else None, "h1": h1.strip()[:120],
                             "failedRequests": sorted(set(errors))[:8], "file": file.name})
            ctx.close()
        browser.close()
    (out / "shots.json").write_text(json.dumps(rows, indent=1))
    for r in rows:
        print(f"{r['width']:5} {r['status']} {r['page']:14} h1={r['h1']!r} failed={r['failedRequests']}")


if __name__ == "__main__":
    main()
