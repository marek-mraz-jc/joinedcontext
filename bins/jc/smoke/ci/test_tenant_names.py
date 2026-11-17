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
