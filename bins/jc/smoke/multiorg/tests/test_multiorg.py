"""Unit tests of the multiorg suite's host mode (the live proof is a rehearsal run).

    python3 -m pytest bins/jc/smoke/multiorg/tests -q
"""

import argparse
import importlib.util
import json
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent

_spec = importlib.util.spec_from_file_location("smoke_multiorg_suite", HERE.parent / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

TOML = """
[[organizations]]
name = "hel"
catalogue_id = "00000000-0000-7000-8000-0000000000a1"
hosts = { portal = "Portal.Hel.localhost:42301", opendata = "opendata.hel.localhost:42301", api = "api.hel.localhost:42301", mcp = "mcp.hel.localhost:42301" }
"""


def test_the_hosts_of_each_organization_are_read_lowercased(tmp_path):
    (tmp_path / "jc.toml").write_text(TOML)
    hosts = suite.organization_hosts(tmp_path / "jc.toml")
    assert hosts == {"hel": {"portal": "portal.hel.localhost:42301", "opendata": "opendata.hel.localhost:42301",
                             "api": "api.hel.localhost:42301", "mcp": "mcp.hel.localhost:42301"}}
    (tmp_path / "none.toml").write_text("")
    assert suite.organization_hosts(tmp_path / "none.toml") == {}


def test_a_request_goes_to_loopback_with_its_hosts_header_and_jar(monkeypatch):
    sent = []

    def call(self, method, path, body=None, headers=None, form=None, timeout=60):
        sent.append((method, path, headers["Host"], id(self.jar)))
        self.jar["seen"] = headers["Host"]

    monkeypatch.setattr(suite.R.Http, "call", call)
    http = suite.HostHttp("http://127.0.0.1:42301", "portal.hel.localhost:42301",
                          ["portal.hel.localhost:42301", "portal.ba.localhost:42301"])
    http.call("GET", "/api/v1/projects?x=1")
    http.call("GET", "http://portal.ba.localhost:42301/api/v1/me")
    http.call("GET", "/api/v1/me")
    assert [s[:3] for s in sent] == [
        ("GET", "http://127.0.0.1:42301/api/v1/projects?x=1", "portal.hel.localhost:42301"),
        ("GET", "http://127.0.0.1:42301/api/v1/me", "portal.ba.localhost:42301"),
        ("GET", "http://127.0.0.1:42301/api/v1/me", "portal.hel.localhost:42301"),
    ]
    # One jar per host: hel's cookies are never sent to ba.
    assert sent[0][3] == sent[2][3] != sent[1][3]
    assert http.jars["portal.ba.localhost:42301"] == {"seen": "portal.ba.localhost:42301"}


def test_an_address_off_this_jc_is_refused_and_never_sent(monkeypatch):
    monkeypatch.setattr(suite.R.Http, "call", lambda *a, **k: pytest.fail("sent"))
    http = suite.HostHttp("http://127.0.0.1:42301", "portal.hel.localhost:42301", ["portal.hel.localhost:42301"])
    with pytest.raises(RuntimeError, match="not a host of this jc"):
        http.call("GET", "https://dev.joinedcontext.com/api/v1/me")


def args(tmp_path, state=None):
    exp = tmp_path / "export"
    exp.mkdir()
    (exp / "manifest.json").write_text(json.dumps({"installation": "prod"}))
    return argparse.Namespace(base_url="http://127.0.0.1:9", state=state, export=str(exp), jc_config=None,
                              composed=False)


def test_an_export_without_a_multiorg_block_is_one_named_skip(tmp_path):
    s = suite.Suite(args(tmp_path))
    s.run()
    assert [(r["id"], r["result"]) for r in s.rows] == [("setup", "skip")]
    assert s.rows[0]["reason"].startswith("not applicable: the export is one installation")


def test_a_gate_bundle_that_is_not_composed_or_missing_fails_instead_of_skipping(tmp_path):
    a = args(tmp_path)
    a.composed = True
    s = suite.Suite(a)
    s.run()
    assert [(r["id"], r["result"], r["actual"]) for r in s.rows] == [("setup", "fail", "none")]
    a.export = str(tmp_path / "gone")
    s = suite.Suite(a)
    s.run()
    assert [(r["id"], r["result"], r["actual"]) for r in s.rows] == [("setup", "fail", "no manifest.json")]


def test_the_suite_takes_the_state_realm(tmp_path):
    state = tmp_path / "state"
    state.mkdir()
    (state / "realm-name.txt").write_text("smartcity")
    assert suite.Suite(args(tmp_path, str(state))).realm == "smartcity"


def test_an_organization_endpoint_counts_as_readable_only_with_a_read_policy_in_its_space(tmp_path):
    a = args(tmp_path)
    exp = Path(a.export)
    (exp / "manifest.json").write_text(json.dumps({"multiorg": {"organizations": {"zilina": {"projects": ["zilina"]}}}}))
    (exp / "gitea/files/joinedcontext/configuration/organizations").mkdir(parents=True)
    for space, slug, verb in (("mesto", "aaaa", "createEntity"), ("data", "bbbb", "retrieveOps")):
        d = exp / "gitea/files/joinedcontext/zilina/spaces" / space
        (d / "endpoints").mkdir(parents=True)
        (d / "policies").mkdir()
        (d / "endpoints/e.yaml").write_text(f"kind: Endpoint\nmetadata:\n  name: {space}\nspec:\n  slug: {slug}\n"
                                            "  audience: organization\n")
        (d / "policies/p.yaml").write_text(f"kind: Policy\nspec:\n  operations:\n    - {verb}\n")
    s = suite.Suite(a)
    s.expectations()
    project = s.exp["zilina"]["projects"]["zilina"]
    assert set(project["closed"]) == {"aaaa", "bbbb"} and project["readable"] == ["bbbb"]
