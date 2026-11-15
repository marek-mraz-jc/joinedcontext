"""The CKAN table view shows every DataStore column with its values (T-3047).

A published endpoint's DataStore columns are named after their NGSI-LD paths
(`dataProvider.value`, `location.value.coordinates`). CKAN 2.11's datatables_view gave
DataTables that name as the column's `data`, which DataTables reads as nested-object notation:
every such column rendered empty and a person saw only `_id`, `entity_id` and `type`. The
catalogue image rewrites the accessor to a function; these tests run the image's own RUN line on
the upstream source line and on a source where upgrade moved it.
"""

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCKERFILE = ROOT / "images/ckan/Dockerfile"
UPSTREAM = "        const colDict = { name: colDefn.id, data: colDefn.id, contentPadding: 'MM' }\n"
ASSET = "/srv/app/src/ckan/ckanext/datatablesview/assets/datatablesview.js"


def patch_command() -> str:
    """The Dockerfile's RUN that patches the asset, as one shell command."""
    text = DOCKERFILE.read_text()
    match = re.search(r"^RUN (f=" + re.escape(ASSET) + r".*?)\n\n", text, re.S | re.M)
    assert match, "images/ckan/Dockerfile no longer patches datatablesview.js"
    return match.group(1).replace("\\\n", " ")


def run_on(tmp_path, source: str) -> tuple[int, str]:
    asset = tmp_path / "datatablesview.js"
    asset.write_text(source)
    command = patch_command().replace(ASSET, str(asset), 1)
    done = subprocess.run(["sh", "-c", command], capture_output=True, text=True)
    return done.returncode, asset.read_text()


def test_the_image_reads_each_column_by_its_whole_name(tmp_path):
    code, patched = run_on(tmp_path, "const dynamicCols = []\n" + UPSTREAM)
    assert code == 0
    assert "data: (row) => row[colDefn.id]" in patched
    assert "data: colDefn.id" not in patched
    # The name the ajax view searches and sorts by stays the column's own.
    assert "name: colDefn.id" in patched


def test_an_upgrade_that_moved_the_line_fails_the_build(tmp_path):
    moved = UPSTREAM.replace("contentPadding: 'MM'", "contentPadding: 'MMM'")
    code, unchanged = run_on(tmp_path, moved)
    assert code != 0
    assert unchanged == moved


def test_the_base_image_is_the_one_the_line_was_read_from():
    """The upstream line above is CKAN 2.11.7's (unchanged since 2.11.6); a new base image is a new reading of it."""
    assert "FROM ckan/ckan-base:2.11.7@sha256:" in DOCKERFILE.read_text()
