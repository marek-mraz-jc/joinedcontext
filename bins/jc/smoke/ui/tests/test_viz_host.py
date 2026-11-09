"""The chart host jc serves on viz.{domain} (vendor/viz/site/host.js) reads the view id the catalogue
page puts in its frame address. jc's dataroom mints a view id `view-<uuid>` (crates/dataroom
api/src/ckan.rs) where CKAN minted a bare UUID; a host that names none draws no chart (rehearsal
host mode 2026-11-09, helsinki-weather: 6 frames, 0 charts). Run in Chromium as the page runs it."""

from pathlib import Path

import pytest

HOST_JS = Path(__file__).resolve().parents[5] / "vendor" / "viz" / "site" / "host.js"
CKAN = "6f1c2b9e-0d0a-4c35-9a51-3a3f8f6c1d20"
JC = "view-01a1c218-3e0c-7751-b4c3-11cb4efa0a8b"


@pytest.fixture(scope="module")
def parse():
    sync = pytest.importorskip("playwright.sync_api")
    with sync.sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage"])
        page = browser.new_page()
        # As module.exports, so the page's main() does not start on about:blank.
        page.evaluate("() => { window.module = { exports: {} }; }")
        page.add_script_tag(content=HOST_JS.read_text())
        yield lambda h: page.evaluate("h => window.module.exports.parseHash(h)", h)
        browser.close()


@pytest.mark.parametrize("vid", [CKAN, JC])
def test_the_host_names_the_view_the_catalogue_frames(parse, vid):
    # The address crates/dataroom/web helpers.rs writes: `{viz}/#{id}&inline&lang={lang}`.
    assert parse(f"#{vid}&inline&lang=sk") == {"id": vid, "inline": True, "lang": "sk"}
    assert parse(f"#{vid}") == {"id": vid, "inline": False, "lang": None}


@pytest.mark.parametrize("bad", ["", "#", "#view-", "#view-x", f"#{JC}x", f"#x{CKAN}", f"#view-{JC}",
                                 "#view-../../x", "#https://evil.example/", "#view-%2F%2Fevil"])
def test_any_other_fragment_names_none(parse, bad):
    assert parse(bad) is None
