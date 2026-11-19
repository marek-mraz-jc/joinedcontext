"""Unit tests of the sharing suite: the matrix, the control pairing, the known list and the lane,
the hidden-attribute check (with one deliberate red) and the fixture manifests. The measured run
is a rehearsal (bins/jc/smoke/ci/gate.sh, then run sharing).

    python3 -m pytest bins/jc/smoke/sharing/tests -q
"""

import argparse
import importlib.util
import json
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("smoke_sharing_suite", HERE.parent / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)


def cell(cid):
    return next(c for c in suite.cells() if c["id"] == cid)


def test_the_matrix_holds_every_way_surface_and_check_once():
    ids = [c["id"] for c in suite.cells()]
    assert len(ids) == len(set(ids))
    for way in suite.WAYS:
        for surface, checks in suite.SURFACES.items():
            for check in checks:
                assert f"{way}.{surface}.{check}" in ids
            assert f"{way}.{surface}.hidden" in ids and f"{way}.{surface}.outsider" in ids
            # a replica outlives a revocation: w3 records the copy, the others must refuse
            assert (f"{way}.{surface}.copy-remains" if way == "w3" else f"{way}.{surface}.revoked") in ids
        assert f"{way}.write.write-refused" in ids
    assert [i for i in ids if i.endswith("write-granted")] == ["w1.write.write-granted"]


def test_reads_need_a_control_and_refusals_judge_themselves():
    assert cell("w1.ngsi-ld.query")["control"] and cell("w2-public.mcp.get-entity")["control"]
    assert cell("w1.linkml.hidden")["control"]  # A's admin must see the attribute B must not
    assert cell("w1.ngsi-ld.revoked")["control"]  # paired with the read before the revocation
    for cid in ("w1.ngsi-ld.outsider", "w1.write.write-refused", "w3.mcp.copy-remains",
                "w1.linkml.mirror"):
        assert not cell(cid)["control"], cid


def test_a_cell_whose_control_failed_is_blind_never_a_pass_or_a_fail():
    c = cell("w1.ngsi-ld.query")
    assert suite.judge(c, True, "200", (False, "404 no such route"))["result"] == "blind"
    assert suite.judge(c, False, "404", None)["result"] == "blind"
    assert suite.judge(c, False, "403", (True, "200"))["result"] == "fail"
    row = suite.judge(c, True, "200", (True, "200"))
    assert row["result"] == "pass" and row["controlActual"] == "200"
    # a refusal row needs no control
    assert suite.judge(cell("w1.ngsi-ld.outsider"), True, "nothing answered")["result"] == "pass"


def test_the_known_list_names_w2_protected_with_its_spec_reference():
    known = suite.load_known()
    assert "45.1" in suite.known_for("w2-protected.ngsi-ld.query", known)
    assert suite.known_for("w2-protected.mcp.get-entity", known)
    # a revocation of a read that is not built cannot be measured: excused with the read
    assert suite.known_for("w2-protected.mcp.revoked", known)
    # an outsider or a write row is never excused (security), nor any row of another way
    for cid in ("w2-protected.ngsi-ld.outsider", "w2-protected.write.write-refused", "w2-protected.linkml.hidden",
                "w1.ngsi-ld.query", "w1.ngsi-ld.revoked", "w2-public.ngsi-ld.query"):
        assert suite.known_for(cid, known) is None, cid


def test_settle_turns_red_only_for_a_failing_cell_off_the_known_list():
    known = suite.load_known()
    rows = [suite.judge(cell("w2-protected.ngsi-ld.query"), False, "200, list of 0", (True, "200")),
            suite.judge(cell("w1.ngsi-ld.query"), False, "403", (True, "200")),
            suite.judge(cell("w2-protected.mcp.get-entity"), True, "200", (True, "200")),
            suite.judge(cell("w1.mcp.get-entity"), True, "200", (False, "404"))]
    s = {r["id"]: r for r in suite.settle(rows, known)}
    assert s["w2-protected.ngsi-ld.query"]["status"] == "not built" and not s["w2-protected.ngsi-ld.query"]["red"]
    assert s["w1.ngsi-ld.query"]["red"]
    assert "stale" in s["w2-protected.mcp.get-entity"]["status"]
    assert s["w1.mcp.get-entity"]["status"] == "blind" and s["w1.mcp.get-entity"]["red"]


def test_the_lane_judges_its_cells_alone():
    rows = [suite.judge(cell("w1.ngsi-ld.query"), False, "403", (True, "200")),
            suite.judge(cell("w1.ngsi-ld.retrieve"), True, "200", (True, "200"))]
    s = {r["id"]: r for r in suite.settle(rows, {}, lane={"w1.ngsi-ld.retrieve"})}
    assert s["w1.ngsi-ld.query"]["status"] == "outside lane" and not s["w1.ngsi-ld.query"]["red"]
    assert s["w1.ngsi-ld.retrieve"]["status"] == "pass"
    lane = json.loads(suite.LANE.read_text())["cells"]
    ids = {c["id"] for c in suite.cells()}
    assert set(lane) <= ids  # a lane entry names a real cell
    assert not any(suite.known_for(c, suite.load_known()) for c in lane)


def entity(hidden_visible: bool):
    e = {"id": suite.URN, "type": suite.TYPE, "level": {"type": "Property", "value": 8},
         "note": {"type": "Property", "value": "second"}}
    if hidden_visible:
        e[suite.HIDDEN] = {"type": "Property", "value": "kept"}
    return e


def test_the_hidden_attribute_check_sees_it_on_every_surface():
    assert not suite.names_attribute([entity(False)], suite.HIDDEN)
    assert suite.names_attribute([entity(True)], suite.HIDDEN)
    # MCP answers carry the entity as text inside the result
    text = suite.mcp_text({"result": {"content": [{"type": "text", "text": json.dumps(entity(True))}]}})
    assert suite.names_attribute(text, suite.HIDDEN)
    assert not suite.names_attribute(suite.mcp_text({"result": {"content": [{"type": "text", "text": json.dumps(entity(False))}]}}),
                                     suite.HIDDEN)
    # LinkML YAML and a JSON-LD @context name it as a key
    assert suite.names_attribute(suite.linkml(), suite.HIDDEN)
    assert suite.names_attribute({"@context": {suite.HIDDEN: "https://joinedcontext.com/models/secret"}}, suite.HIDDEN)
    # a value equal to the name is not the attribute; neither is a longer word holding it
    assert not suite.names_attribute({"note": {"value": "secretary"}}, suite.HIDDEN)


class Stub(suite.Suite):
    """A Suite with no jc: the calls answer from a table."""

    def __init__(self, answers):
        self.cells = {c["id"]: c for c in suite.cells()}
        self.matrix, self.rows, self.answers = [], [], answers
        self.slugs = {n: n for n in ("sh-shared", "sh-open", "sh-fedpub", "sh-fedprot", "sh-copy")}

    def reads(self, key, door):
        return self.answers[key]

    def upsert(self, key, door, attrs):
        return (204, b"") if key == "a-admin" else (403, b"")

    def get(self, key, door, path):
        return 200, entity(True)


def reads(hidden_visible: bool, ok=True):
    doc = entity(hidden_visible)
    out = {}
    for surface, checks in suite.SURFACES.items():
        for check in checks:
            if check not in ("subscription", "mirror"):
                out[(surface, check)] = (ok, "200" if ok else "403", [doc] if ok else {"title": "Forbidden"})
    return out


def test_one_deliberate_red_a_hidden_attribute_made_visible_fails_the_hidden_cells():
    control = reads(True)
    control[("write", "a-admin")] = (True, "A's own write 204", None)
    nothing = reads(False, ok=False)
    green = Stub({"b-member": reads(False), "c-member": nothing})
    green.check_way("w2-public", control)
    hidden = {r["id"]: r["result"] for r in green.matrix if r["check"] == "hidden"}
    assert set(hidden.values()) == {"pass"}, hidden
    red = Stub({"b-member": reads(True), "c-member": nothing})  # the fixture leaks the attribute
    red.check_way("w2-public", control)
    hidden = {r["id"]: r for r in red.matrix if r["check"] == "hidden"}
    assert {r["result"] for r in hidden.values()} == {"fail"}
    assert all(f"'{suite.HIDDEN}' in" in r["actual"] for r in hidden.values())
    # the outsider and the refused write hold in both
    assert {r["result"] for r in red.matrix if r["check"] in ("outsider", "write-refused")} == {"pass"}


def test_the_outsider_cell_fails_when_c_reads_the_entity():
    control = reads(True)
    control[("write", "a-admin")] = (True, "204", None)
    s = Stub({"b-member": reads(False), "c-member": reads(False)})
    s.check_way("w2-public", control)
    assert {r["result"] for r in s.matrix if r["check"] == "outsider"} == {"fail"}


def fixture_suite():
    s = Stub({})
    s.org_a, s.org_b, s.domain_a = "hel", "zilina", "hel.fi"
    s.people = {k: {"id": f"00000000-0000-4000-8000-00000000000{i}"} for i, k in
                enumerate(("a-admin", "b-admin", "b-member", "c-member"))}
    return s


def test_the_fixtures_are_manifests_the_share_policies_leave_the_hidden_attribute_out():
    s = fixture_suite()
    docs = {p: yaml.safe_load(t) for p, t in s.a_files() + s.b_files() if p.endswith(".yaml") and "linkml" not in p}
    shared = docs["spaces/sh-src/endpoints/sh-shared.yaml"]["spec"]
    assert shared["audience"] == "project-list" and shared["allowedProjects"] == [suite.PB]
    for name in ("sh-share-member", "sh-share-admin", "sh-share-pipeline", "sh-public"):
        spec = docs[f"spaces/sh-src/policies/{name}.yaml"]["spec"]
        assert spec["information"][0]["propertyNames"] == list(suite.SHOWN), name
    admin = docs["spaces/sh-src/policies/sh-admin.yaml"]["spec"]
    assert "propertyNames" not in admin["information"][0]  # the control sees every attribute
    csr = docs["spaces/sh-fedprot/registrations/sh-shared.yaml"]["spec"]
    assert csr["endpointRef"] == {"kind": "Endpoint", "name": "sh-shared", "namespace": suite.PA}
    ref = docs[f"shared/{suite.PA}.yaml"]["spec"]
    assert ref["endpointSlug"] == s.slugs["sh-shared"]
    revoked = {p: yaml.safe_load(t) for p, t in s.a_files(revoke="2026-11-19T00:00:00Z") if p.endswith(".yaml") and "linkml" not in p}
    assert revoked["spaces/sh-src/endpoints/sh-shared.yaml"]["spec"]["allowedProjects"] == []
    assert revoked["spaces/sh-src/policies/sh-share-member.yaml"]["spec"]["validity"]["to"] == "2026-11-19T00:00:00Z"
    assert "validity" not in revoked["spaces/sh-src/policies/sh-admin.yaml"]["spec"]


def test_the_report_holds_statuses_and_attribute_names_never_values():
    rows = suite.settle([suite.judge(cell("w1.ngsi-ld.hidden"), False, f"'{suite.HIDDEN}' in ['query']", (True, "A's admin sees"))], {})
    text = suite.report(rows, [])
    assert "w1 | ngsi-ld | hidden | fail" in text
    assert "kept" not in text and "second" not in text
    assert suite.Suite.brief([entity(True)]) == "list of 1"
    assert suite.Suite.brief({"title": "AccessDenied", "detail": "no Policy of this endpoint grants you this write"}) \
        == "title: AccessDenied, detail: no Policy of this endpoint grants you this write"
    assert suite.Suite.brief({"title": "AccessDenied"}) == "title: AccessDenied"
    assert "kept" not in suite.Suite.brief(entity(True))


class RefusingForge:
    """A forge answering every repository create with `status`; records the calls."""
    def __init__(self, status):
        self.status, self.calls = status, []

    def json(self, method, path, headers=None, body=None):
        self.calls.append((method, path))
        return self.status, {}

    def call(self, method, path, headers=None, body=None):
        self.calls.append((method, path))
        return 404, {}, b""


def refused_run(tmp_path, monkeypatch, lane):
    a = argparse.Namespace(base_url="http://127.0.0.1:1", state=None, export=str(tmp_path), out=str(tmp_path),
                           provider="hel", consumer="zilina", outsider="barcelona", sync_timeout=1)
    s = suite.Suite(a, lane)
    forge = RefusingForge(405)
    monkeypatch.setattr(s, "client", lambda *_a, **_k: forge)
    monkeypatch.setattr(s, "domain_of", lambda org: f"{org}.example")
    monkeypatch.setattr(suite.R, "read_secret", lambda *_a: "t")

    def no_persona(*_a):
        raise AssertionError("a persona was made before the fixtures existed")
    monkeypatch.setattr(suite.R, "admin_token", no_persona)
    monkeypatch.setattr(suite.R, "make_person", no_persona)
    s.run()
    return s, forge, s.write(tmp_path / "out", lane)


def test_a_forge_refusing_the_fixtures_is_one_skip_row_and_no_persona(tmp_path, monkeypatch):
    s, forge, fails = refused_run(tmp_path, monkeypatch, None)
    assert fails == 0 and s.matrix == [] and not s.made
    checks = json.loads((tmp_path / "out" / "results.json").read_text())["checks"]
    assert [(c["id"], c["result"], c["actual"]) for c in checks] == [("setup.fixtures", "skip", "405")]
    assert "no fixture rights" in checks[0]["reason"]
    assert [m for m, _ in forge.calls] == ["POST"]  # nothing to clean up: no delete of a repository


def test_in_the_lane_a_forge_refusing_the_fixtures_fails(tmp_path, monkeypatch):
    s, _forge, fails = refused_run(tmp_path, monkeypatch, set(json.loads(suite.LANE.read_text())["cells"]))
    assert fails > 0 and not s.skipped
