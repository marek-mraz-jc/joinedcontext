"""T-0049: the air-gap archive is complete, and loading it reaches nothing but the target.

Both scripts are run against stub `skopeo` and `helm` binaries on PATH, the way
test_emergency_revoke.py runs the revocation script, so every call is recorded and a test can
assert both what the scripts did and what they never did (OPS-19, OPS-20, OPS-21).
"""

import os
import shutil
import stat
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
PACKAGE = PROJECT_ROOT / "scripts" / "package-airgap.sh"
LOAD = PROJECT_ROOT / "scripts" / "load-airgap.sh"
INVENTORY = PROJECT_ROOT / "scripts" / "airgap-inventory.py"


@pytest.fixture(scope="module")
def inventory() -> dict:
    result = subprocess.run(
        ["python3", str(INVENTORY), str(PROJECT_ROOT)], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    return yaml.safe_load(result.stdout)


def stub(directory: Path, name: str, body: str) -> None:
    path = directory / name
    path.write_text("#!/bin/sh\n" + textwrap.dedent(body))
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


@pytest.fixture
def bin_dir(tmp_path) -> Path:
    directory = tmp_path / "bin"
    directory.mkdir()
    calls = tmp_path / "calls"
    # skopeo writes the OCI layout its caller asked for, so the archive it produces is a
    # real one with real directories, and records every source and destination it saw.
    stub(directory, "skopeo", f"""\
        echo "skopeo $*" >> {calls}
        for a in "$@"; do
          case "$a" in
            oci:*) d=$(echo "$a" | sed 's|^oci:||; s|:[^:/]*$||'); mkdir -p "$d"; echo '{{}}' > "$d/index.json" ;;
          esac
        done
        exit 0
        """)
    stub(directory, "helm", f"""\
        echo "helm $*" >> {calls}
        dest=""; chart=""; version=""
        while [ $# -gt 0 ]; do
          case "$1" in
            --destination) dest=$2; shift 2 ;;
            --version) version=$2; shift 2 ;;
            --repo) shift 2 ;;
            pull) shift ;;
            *) chart=$1; shift ;;
          esac
        done
        mkdir -p "$dest" && echo stub > "$dest/$chart-$version.tgz"
        exit 0
        """)
    return directory


def run(script: Path, bin_dir: Path, *args: str) -> subprocess.CompletedProcess:
    env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}"}
    return subprocess.run([str(script), *args], capture_output=True, text=True,
                          cwd=str(PROJECT_ROOT), env=env)


def calls_of(bin_dir: Path) -> list[str]:
    path = bin_dir.parent / "calls"
    return path.read_text().splitlines() if path.exists() else []


# --- the inventory the archive is built from -----------------------------------------


def test_every_pinned_image_and_upstream_chart_is_in_the_inventory(inventory):
    """The archive is complete by construction: it is built from the same pins the
    deployment renders from, so an image the cluster can ask for is one that was packaged."""
    references = {i["reference"] for i in inventory["images"]}
    for path in sorted(PROJECT_ROOT.glob("components/*/images.yaml")):
        document = yaml.safe_load(path.read_text()) or {}
        for component, entries in document.items():
            for part, values in (entries or {}).items():
                if not isinstance(values, dict) or not values.get("repository"):
                    continue
                name = values["repository"]
                if values.get("registry"):
                    name = f"{values['registry']}/{name}"
                assert f"{name}@{values['digest']}" in references, f"{component}.{part} missing"


def test_the_inventory_names_every_image_by_digest(inventory):
    for image in inventory["images"]:
        assert image["reference"].split("@")[1].startswith("sha256:")


def test_a_local_chart_is_not_packaged(inventory):
    """`charts/workload` and the other in-repository charts travel in the checkout the
    operator carries in; pulling them from a repository that does not host them would fail
    the packaging run for no reason."""
    repositories = {c["repository"] for c in inventory["charts"]}
    assert all(r.startswith("http") for r in repositories)
    assert "vector" in {c["chart"] for c in inventory["charts"]}


def test_an_image_pinned_by_tag_alone_stops_the_packaging(tmp_path):
    """An archive of a floating tag is an archive of whatever the registry served that day,
    and the copy inside the zone would not be the copy that was reviewed."""
    root = tmp_path / "repo"
    (root / "components/loose").mkdir(parents=True)
    (root / "components/loose/images.yaml").write_text(
        yaml.safe_dump({"loose": {"part": {"repository": "example.org/thing", "tag": "latest"}}}))
    result = subprocess.run(["python3", str(INVENTORY), str(root)], capture_output=True, text=True)
    assert result.returncode != 0
    assert "pins no digest" in result.stderr


# --- packaging -----------------------------------------------------------------------


@pytest.fixture
def archive(bin_dir, tmp_path, inventory) -> Path:
    if shutil.which("tar") is None:
        pytest.skip("tar not installed")
    result = run(PACKAGE, bin_dir, "--out", str(tmp_path / "media"), "--name", "test-archive")
    assert result.returncode == 0, result.stdout + result.stderr
    return tmp_path / "media" / "test-archive.tar.gz"


def test_the_archive_carries_the_manifest_the_images_and_the_charts(archive, tmp_path, inventory):
    listing = subprocess.run(["tar", "-tzf", str(archive)],
                             capture_output=True, text=True, check=True).stdout.split()
    assert "manifest.yaml" in listing
    for image in inventory["images"]:
        directory = image["digest"].replace(":", "_")
        assert any(entry.startswith(f"images/{directory}") for entry in listing), directory
    for chart in inventory["charts"]:
        assert f"charts/{chart['chart']}-{chart['version']}.tgz" in listing


def test_packaging_copies_every_platform_of_every_image(archive, bin_dir, inventory):
    """The machine that carries the archive in is rarely the architecture the cluster runs,
    so a single-platform copy produces an archive that cannot start a pod inside the zone."""
    copies = [c for c in calls_of(bin_dir) if c.startswith("skopeo copy")]
    assert len(copies) == len(inventory["images"])
    assert all("--all" in c for c in copies)
    assert all("docker://" in c and "oci:" in c for c in copies)


# --- loading -------------------------------------------------------------------------


@pytest.fixture
def loaded(archive, bin_dir, tmp_path) -> tuple[subprocess.CompletedProcess, Path]:
    charts_dir = tmp_path / "airgap-charts"
    (bin_dir.parent / "calls").unlink(missing_ok=True)
    result = run(LOAD, bin_dir, "--archive", str(archive), "--registry", "registry.internal:5000",
                 "--charts-dir", str(charts_dir))
    assert result.returncode == 0, result.stdout + result.stderr
    return result, charts_dir


def test_loading_reaches_the_target_registry_and_nothing_else(loaded, bin_dir, inventory):
    """This is the zero-outbound-call property of OPS-21, asserted the only way a script can
    assert it: every destination the script ever names is recorded, and every one of them is
    the registry the operator gave."""
    copies = [c for c in calls_of(bin_dir) if c.startswith("skopeo copy")]
    assert len(copies) == len(inventory["images"])
    destinations = [word for c in copies for word in c.split() if word.startswith("docker://")]
    assert len(destinations) == len(inventory["images"])
    assert all(d.startswith("docker://registry.internal:5000/") for d in destinations), destinations
    sources = [word for c in copies for word in c.split() if word.startswith("oci:")]
    assert len(sources) == len(inventory["images"])
    assert not [c for c in calls_of(bin_dir) if c.startswith("helm")]


def test_loading_writes_the_values_override_that_points_at_the_registry(loaded, inventory):
    """Without this the operator hand-edits sixteen repository fields, and the one they miss
    is the pod that stays in ImagePullBackOff with no route out of the zone."""
    _, charts_dir = loaded
    overrides = yaml.safe_load((charts_dir / "airgap-images.yaml.gotmpl").read_text())["images"]
    for image in inventory["images"]:
        part = overrides[image["component"]][image["part"]]
        assert part["repository"].startswith("registry.internal:5000/")
        assert part["digest"] == image["digest"], "the digest must survive the mirror"
        assert not part["repository"].startswith("registry.internal:5000/docker.io/")


def test_the_charts_are_unpacked_where_helmfile_can_read_them(loaded, inventory):
    _, charts_dir = loaded
    for chart in inventory["charts"]:
        assert (charts_dir / f"{chart['chart']}-{chart['version']}.tgz").is_file()


def test_a_registry_given_as_a_url_is_refused(bin_dir, archive, tmp_path):
    """`docker://https://host/x` is not a reference. Refusing it here is one line; finding
    it out is a failed mirror run inside a zone with no way to look anything up."""
    result = run(LOAD, bin_dir, "--archive", str(archive),
                 "--registry", "https://registry.internal:5000")
    assert result.returncode == 64
    assert "is a host, not a URL" in result.stderr


def test_an_archive_without_a_manifest_is_refused(bin_dir, tmp_path):
    empty = tmp_path / "empty"
    empty.mkdir()
    (empty / "images").mkdir()
    broken = tmp_path / "broken.tar.gz"
    subprocess.run(["tar", "-C", str(empty), "-czf", str(broken), "images"], check=True)
    result = run(LOAD, bin_dir, "--archive", str(broken), "--registry", "registry.internal:5000")
    assert result.returncode == 65
    assert "no way to know what is in it" in result.stderr
