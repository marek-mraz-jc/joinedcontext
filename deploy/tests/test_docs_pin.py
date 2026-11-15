"""The docs the release serves are pinned with it and held to the code it runs (T-4483, TS-19).

components/portal/images.yaml names the Portal commit beside its digest and the docs commit its
/docs pages are; components/context-gateway/images.yaml names the platform commit. CI runs
scripts/check-docs-pin.sh on those pins; this file proves the gate turns red on docs that name
something the pinned code does not have, so its green on main means something.
"""

import io
import re
import subprocess
import tarfile
import urllib.request
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent


def pins() -> dict[str, str]:
    portal = yaml.safe_load((ROOT / "components/portal/images.yaml").read_text())["portal"]["portal"]
    gateway = yaml.safe_load((ROOT / "components/context-gateway/images.yaml").read_text())["context-gateway"]["gateway"]
    return {"portal": portal.get("commit", ""), "docs": portal.get("docs", ""), "platform": gateway.get("commit", "")}


def test_the_release_pins_whole_commits():
    for name, commit in pins().items():
        assert re.fullmatch(r"[0-9a-f]{40}", commit), f"{name}: {commit!r} is not one commit"


def docs_tree(commit: str, into: Path) -> bool:
    url = f"https://codeload.github.com/marek-mraz-jc/joinedcontext-docs/tar.gz/{commit}"
    try:
        with urllib.request.urlopen(url, timeout=60) as answer:
            data = answer.read()
    except OSError:
        return False
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive.getmembers():
            parts = Path(member.name).parts[1:]
            if not parts or member.issym() or member.islnk() or ".." in parts:
                continue
            member.name = str(Path(*parts))
            archive.extract(member, into, filter="data")
    return True


# Built, never written whole: this tree is part of the code the checker reads, and a literal
# name here would count as read.
MISSING = "JC_NO_SUCH_" + "VARIABLE"


def test_docs_naming_a_route_and_a_variable_the_pinned_code_lacks_are_refused(tmp_path):
    if not docs_tree(pins()["docs"], tmp_path):
        pytest.skip("GitHub cannot be reached")
    page = tmp_path / "API/99-fixture.md"
    page.write_text(
        "---\ntitle: Fixture\n---\n\n# Fixture\n\n## 1. Route\n\n"
        "```http\nGET /api/v1/no-such-route\n```\n\n"
        f"Set `{MISSING}` to turn it on.\n"
    )
    run = subprocess.run([str(ROOT / "scripts/check-docs-pin.sh"), "--docs", str(tmp_path)],
                         capture_output=True, text=True, timeout=600)
    out = run.stdout + run.stderr
    if "is not in the registry" in out or "Could not resolve host" in out:
        pytest.skip("the registry or GitHub cannot be reached")
    assert run.returncode == 1, out
    assert "API/99-fixture.md" in out and "GET /api/v1/no-such-route is in no path" in out, out
    assert MISSING in out, out
