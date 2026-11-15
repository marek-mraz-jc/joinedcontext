"""Every hand-written seed manifest is in the one layout `jcctl fmt` writes (T-5052).

The Portal commits every manifest through the same formatter, so a change made in the Portal
and a hand edit look the same in a git diff. The check runs the platform's own `jcctl fmt
--check`: `JC_JCCTL` names a local binary, otherwise the pinned `checkouts` image (the
platform image that carries jcctl) runs it in a container.

A generated seed keeps its generator's layout: the generator's own test holds it to what the
generator writes, and a formatter run over it would only make that test red. `GENERATED` names
those files and the generator that owns them.
"""

import fnmatch
import os
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-jcctl-fmt")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components" / "context-gateway" / "seed"

#: Seed files a generator writes, by generator; relative to SEED.
GENERATED = {
    "scripts/open-data/generate.py": [
        "*/*-od-*",
        "*/*-otvorene-data.linkml.yaml",
    ],
    # The DATAcube topics and the model files it copies into each of them byte for byte.
    "scripts/open-data/datacube.py": ["slovensko/susr*/*"],
    "scripts/air-sk/generate.py": ["slovensko/slovensko-pipeline-ovzdusie-bento.yaml"],
    "seeds/bbsk-events/generate.py": [
        "*/*-datasource-podujatia-*",
        "*/*-pipeline-podujatia-*",
        "*/*-datasource-miesta-podujati*",
        "*/*-pipeline-miesta-podujati*",
        # A hand-written Endpoint whose `source:` list the generator writes between its markers.
        "*/*-endpoint-podujatia*",
        "praha/praha-endpoint-akce.yaml",
    ],
    "seeds/decidim/generate.py": [
        "slovensko/slovensko-*-decidim-*",
        # A hand-written Endpoint whose `source:` list the generator writes between its markers.
        "slovensko/slovensko-endpoint-participacia.yaml",
    ],
}


def generated(rel: str) -> bool:
    return any(fnmatch.fnmatch(rel, g) for globs in GENERATED.values() for g in globs)


def hand_written() -> list[Path]:
    files = sorted(p for p in SEED.rglob("*") if p.suffix in (".yaml", ".yml"))
    return [p for p in files if not generated(p.relative_to(SEED).as_posix())]


def checkouts_image() -> str:
    pin = yaml.safe_load(
        (ROOT / "components" / "context-gateway" / "images.yaml").read_text(encoding="utf-8")
    )["context-gateway"]["checkouts"]
    return f"{pin['repository']}:{pin['tag']}@{pin['digest']}"


def jcctl_fmt_check(files: list[Path]) -> subprocess.CompletedProcess:
    rel = [p.relative_to(ROOT).as_posix() for p in files]
    local = os.environ.get("JC_JCCTL")
    if local:
        cmd = [local, "fmt", "--check", *rel]
    elif shutil.which("docker"):
        cmd = [
            "docker", "run", "--rm", "--network", "none", "--read-only",
            "--user", f"{os.getuid()}:{os.getgid()}",
            "-v", f"{ROOT}:/w:ro", "-w", "/w",
            "--entrypoint", "/usr/local/bin/jcctl",
            checkouts_image(), "fmt", "--check", *rel,
        ]
    else:
        pytest.skip("no JC_JCCTL and no docker: jcctl fmt cannot run here")
    return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=600)


def test_every_hand_written_seed_is_in_the_canonical_layout():
    files = hand_written()
    assert len(files) > 500, "the seed tree moved: update SEED"
    result = jcctl_fmt_check(files)
    assert result.returncode == 0, (
        "run `jcctl fmt <file>` on these (T-5052):\n" + result.stdout + result.stderr
    )


def in_cron_range(field: str, low: int, high: int) -> bool:
    return all(
        low <= int(number) <= high
        for part in field.split(",")
        for number in part.split("/")[0].split("-")
        if number != "*"
    )


def test_every_pipeline_schedule_is_a_cron_the_portal_accepts():
    """The Portal refuses a schedule out of range and the Pipeline never runs (`77 3 * * 3`)."""
    ranges = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)]
    bad = []
    for path in SEED.rglob("*-pipeline-*.yaml"):
        for doc in yaml.safe_load_all(path.read_text()):
            schedule = (doc or {}).get("spec", {}).get("schedule")
            if (doc or {}).get("kind") != "Pipeline" or schedule is None:
                continue
            fields = schedule.split()
            if len(fields) != 5 or not all(
                in_cron_range(field, *limit) for field, limit in zip(fields, ranges)
            ):
                bad.append(f"{path.name}: {schedule}")
    assert not bad, bad


def test_each_generated_glob_names_a_file_and_its_generator_exists():
    files = [p.relative_to(SEED).as_posix() for p in SEED.rglob("*.yaml")]
    for generator, globs in GENERATED.items():
        assert (ROOT / generator).is_file(), generator
        for g in globs:
            assert any(fnmatch.fnmatch(f, g) for f in files), f"{g} matches no seed"
