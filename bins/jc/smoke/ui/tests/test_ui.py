"""Unit tests of the `ui` smoke suite: python3 -m pytest bins/jc/smoke/ui/tests"""

import json
import re
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import checks  # noqa: E402
import suite  # noqa: E402
import walk  # noqa: E402
from common import Results  # noqa: E402

GOOD = """<html lang="en"><head><title>T</title></head><body><div id="root"><main>
<h1>Spaces</h1><p>The spaces of this project and what they hold.</p>
<table><thead><tr><th>ID</th><th>Name</th></tr></thead>
<tbody><tr><td>urn:ngsi-ld:Space:1</td><td>Parking</td></tr></tbody></table>
</main></div></body></html>"""

BAD = """<html lang="en"><head><title>T</title></head><body><div id="root"><main>
<h1>Spaces</h1>
<div data-empty-state><p>Nothing here.</p></div>
<table><thead><tr><th>Owner</th></tr></thead><tbody><tr><td>urn:ngsi-ld:Person:7</td></tr></tbody></table>
<p>{"kind": "Space"}</p><button>urn:ngsi-ld:Space:1</button><button>Delete</button>
</main></div></body></html>"""


@pytest.fixture(scope="module")
def page():
    sync = pytest.importorskip("playwright.sync_api")
    with sync.sync_playwright() as pw:
        browser = pw.chromium.launch(args=["--disable-dev-shm-usage"])
        yield browser.new_page()
        browser.close()


def judge(page, html):
    page.set_content(html)
    page.add_script_tag(content=checks.script())
    out = page.evaluate("o => window.__jcSmoke.pageFindings(o)", {"namespaces": checks.namespaces(), "layout": True})
    return out + page.evaluate(checks.TABLE_URN) + page.evaluate("() => window.__jcSmoke.viewerWriteFindings(true)")


def test_the_portal_checks_load_from_the_portal_source_and_pass_a_good_page(page):
    assert judge(page, GOOD) == []


def test_the_checks_name_every_defect_of_a_bad_page(page):
    found = {f.split(":")[0] for f in judge(page, BAD)}
    assert {"purpose-line", "empty-state-next-step", "raw-json", "bare-urn", "table-urn", "viewer-write"} <= found


def test_the_stripped_source_keeps_no_typescript():
    js = checks.script()
    assert not re.search(r"\bas HTML|: string\b|: Element\b|<string>", js)
    for name in checks.FUNCTIONS:
        assert f"function {name}(" in js


def ts_list(name: str) -> list[str]:
    src = (checks.UI / "e2e" / "live" / "walk-routes.ts").read_text()
    body = re.search(rf"const {name} = \[(.*?)\];", src, re.S).group(1)
    return re.findall(r'"([^"]+)"', body)


@pytest.mark.parametrize("name", ["FORMS", "SECTIONS", "ORGANIZATION_TABS", "SETTINGS_TABS"])
def test_the_route_lists_follow_walk_routes_ts(name):
    assert getattr(walk, name) == ts_list(name)


def test_the_project_pages_follow_walk_routes_ts():
    src = (checks.UI / "e2e" / "live" / "walk-routes.ts").read_text()
    block = src[src.index('"home", "inbox"'):src.index("].map((page)")]
    assert walk.PROJECT_PAGES == re.findall(r'"([^"]+)"', block)


@pytest.mark.parametrize("who,status,url,ok", [
    ("viewer", 403, "http://x/api/v1/projects/p/policies", True),
    ("outsider", 403, "http://x/api/v1/projects/p/policies", True),
    ("admin", 403, "http://x/api/v1/projects/p/policies", False),
    ("admin", 404, "http://x/api/v1/projects/p/drafts/Space/a", True),
    ("admin", 404, "http://x/api/v1/projects/p/spaces/a", False),
    ("admin", 410, "http://x/api/v1/anything", True),
    ("outsider", 404, "http://x/api/v1/projects/p/spaces", True),
    ("anonymous", 401, "http://x/api/v1/auth/me", True),
    ("viewer", 401, "http://x/api/v1/auth/me", False),
    ("viewer", 404, "http://x/api/v1/projects/p/spaces", False),
])
def test_expected_4xx_follows_the_walker(who, status, url, ok):
    assert suite.expected_4xx(who, status, "GET", url) is ok


def test_the_judge_passes_only_scores_of_four_and_up_without_a_high_finding(tmp_path):
    shots = [{"page": f"portal:admin:/{n}", "width": 1440} for n in "abcd"]
    (tmp_path / "j.json").write_text(json.dumps([
        {"page": "portal:admin:/a", "width": 1440, "scores": {"layout": 5, "phone": 4}, "findings": []},
        {"page": "portal:admin:/b", "width": 1440, "scores": {"layout": 3}, "findings": []},
        {"page": "portal:admin:/c", "width": 1440, "scores": {"layout": 5},
         "findings": [{"severity": "high", "text": "raw JSON in the table"}]},
    ]))
    res = Results("ui", "http://localhost")
    suite.fold_judge(res, shots, str(tmp_path / "j.json"))
    got = {r["id"].rsplit("/", 1)[1]: r["result"] for r in res.rows}
    assert got == {"a": "pass", "b": "fail", "c": "fail", "d": "skip"}
