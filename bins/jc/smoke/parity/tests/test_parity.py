"""report(): a feature the old page has and the new page lacks fails; what the old page lacks is a skip."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import pages as P  # noqa: E402
import parity  # noqa: E402


def put(out, side, pid, width, row):
    f = out / "t" / "raw" / side / f"{pid}-{width}.json"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps(row))


def test_missing_feature_fails_absent_on_both_skips_and_a_missing_page_fails(tmp_path, monkeypatch):
    monkeypatch.setitem(P.PAIRS, "t", {"title": "t", "pages": [{"id": "a", "old": "/a", "new": "/a"}, {"id": "b", "old": "/b", "new": None}]})
    feats = lambda grid, map_: [{"feature": "grid", "present": grid, "count": int(grid), "element": "table"},
                                {"feature": "map", "present": map_, "count": int(map_), "element": "canvas"}]
    for w in (1440, 400):
        put(tmp_path, "old", "a", w, {"status": 200, "features": feats(True, False)})
        put(tmp_path, "new", "a", w, {"status": 200, "features": feats(False, False)})
        put(tmp_path, "old", "b", w, {"status": 200, "features": []})
        put(tmp_path, "new", "b", w, {"status": None, "skip": "no such page"})
    rows = {r["id"]: r for r in parity.report(tmp_path, ["t"])}
    assert rows["t/a@1440/page"]["result"] == "pass"
    assert rows["t/a@1440/grid"]["result"] == "fail"
    assert rows["t/a@1440/map"]["result"] == "skip"
    assert rows["t/b@1440/page"]["result"] == "fail"
    assert rows["t/b@400/page"]["actual"] == "no such page"
    n = parity.write_report(tmp_path, list(rows.values()))
    assert n == {"pass": 2, "fail": 3, "skip": 1}
    assert "t/a@1440/grid" in (tmp_path / "REPORT.md").read_text()


def test_the_old_side_guard_lets_only_reads_and_the_sign_in_post_through():
    calls = []

    class Route:
        def continue_(self):
            calls.append("continue")

        def abort(self):
            calls.append("abort")

    class Req:
        def __init__(self, method, url):
            self.method, self.url = method, url

    kc = "idm.dev.joinedcontext.com"
    for method, url, want in [
        ("GET", "https://portal.dev.joinedcontext.com/projects/x", "continue"),
        ("POST", f"https://{kc}/realms/dev/login-actions/authenticate?x=1", "continue"),
        ("POST", "https://portal.dev.joinedcontext.com/api/v1/projects/x/drafts", "abort"),
        ("PUT", f"https://{kc}/admin/realms/dev/users/1", "abort"),
        ("DELETE", "https://dev.joinedcontext.com/grafana/api/dashboards/uid/x", "abort"),
    ]:
        calls.clear()
        parity.guard_old(Route(), Req(method, url), kc)
        assert calls == [want], (method, url)
