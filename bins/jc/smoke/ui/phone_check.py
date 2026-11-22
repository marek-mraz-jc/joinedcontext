"""The 400 px check of crates/portal/ui/e2e/phoneCheck.cts for the Python browser tests (T-7512).

Both run crates/portal/ui/e2e/phoneCheck.browser.js, so the rules live in that file alone.
"""

import hashlib
import json
import re
from pathlib import Path

UI = Path(__file__).resolve().parents[4] / "crates" / "portal" / "ui"


def on_screen(frame):
    while frame.parent_frame:
        element = frame.frame_element()
        box = element.bounding_box() if element.is_visible() else None
        if not box or box["width"] <= 1 or box["height"] <= 1:
            return False
        frame = frame.parent_frame
    return True


def phone_hits(page):
    width = (page.viewport_size or {}).get("width", 10_000)
    if width > 430 or not page.evaluate("() => matchMedia('(pointer: coarse)').matches"):
        raise RuntimeError("the phone check needs a phone: viewport 400 px wide, has_touch and is_mobile")
    axe = (UI / "node_modules" / "axe-core" / "axe.min.js").read_text()
    check = re.sub(r"^/\*.*?\*/\s*", "", (UI / "e2e" / "phoneCheck.browser.js").read_text(), flags=re.S).strip()
    hits = []
    for frame in page.frames:
        if not on_screen(frame):
            continue
        frame.evaluate(axe)
        own = width if frame == page.main_frame else "null"
        hits += frame.evaluate(f"({check})({own})")
    return hits


def write_phone_report(page, shot):
    """`<shot>.phone.json` beside a 400 px shot just taken of this page, as tasks/uijudge reads it."""
    hits = phone_hits(page)
    shot = Path(shot)
    report = {"sha256": hashlib.sha256(shot.read_bytes()).hexdigest(), "hits": hits}
    shot.with_suffix(".phone.json").write_text(json.dumps(report, indent=1) + "\n")
    return hits
