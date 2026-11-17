"""The smoke's tenant names equal jc-core's: one fixture, read here and by jc-core's
names::tests::the_shared_tenant_fixture_holds_in_rust (T-7038, ARCH/TEN-10)."""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "endpoints"))
import export as ex  # noqa: E402

FIXTURE = Path(__file__).resolve().parents[4] / "crates/shared/jc-types/fixtures/tenant_names.json"


def test_the_shared_tenant_fixture_holds_in_python():
    fixture = json.loads(FIXTURE.read_text())
    for case in fixture["minted"]:
        tenant = ex.tenant_name(case["organization"], case["project"], case["space"])
        assert tenant == case["tenant"]
        assert ex.is_tenant_name(tenant)
    for good in fixture["accepted"]:
        assert ex.is_tenant_name(good), good
    for bad in fixture["refused"]:
        assert not ex.is_tenant_name(bad), bad


def test_a_space_takes_its_project_key_once_the_export_carries_it(tmp_path):
    case = json.loads(FIXTURE.read_text())["minted"][0]
    files = tmp_path / "gitea" / "files"
    files.mkdir(parents=True)
    assert ex.space_tenant("hel", "mobility", None, ex.project_keys(files)) == "hel-mobility"
    (tmp_path / "antares").mkdir()
    (tmp_path / "antares" / "project-keys.json").write_text(
        json.dumps({"hel": [case["organization"], case["project"]]}))
    keys = ex.project_keys(files)
    assert ex.space_tenant("hel", case["space"], "pinned", keys) == case["tenant"]
    assert ex.space_tenant("other", "mobility", None, keys) == "other-mobility"


def _map_tool():
    import importlib.util
    path = Path(__file__).resolve().parents[4] / "crates/antares/dev/tenant-rename-map.py"
    spec = importlib.util.spec_from_file_location("tenant_rename_map", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_the_rename_map_maps_each_tenant_of_a_registered_project_once():
    case = json.loads(FIXTURE.read_text())["minted"][0]
    key = (case["organization"], case["project"])
    tool = _map_tool()
    projects = {"hel": key, "a": key, "a-b": key}
    out = tool.plan(
        ["hel-" + case["space"], "a-b-c", "lost-x", "default", "antares-views", "pinned", case["tenant"]],
        projects, {"pinned": ["hel", "air"]})
    assert out["map"] == {"hel-" + case["space"]: case["tenant"],
                          "pinned": ex.tenant_name(*key, "air")}
    assert set(out["unmapped"]) == {"a-b-c", "lost-x", case["tenant"]}
    assert "ambiguous" in out["unmapped"]["a-b-c"]
    twice = tool.plan(["hel-air", "pinned"], projects, {"pinned": ["hel", "air"]})
    assert twice["map"] == {} and set(twice["unmapped"]) == {"hel-air", "pinned"}
