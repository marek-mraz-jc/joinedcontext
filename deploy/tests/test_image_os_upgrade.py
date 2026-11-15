"""Every image this repository builds upgrades its OS packages in its final stage (T-6114).

A base image ships the distribution packages of the day it was built; the archive carries the
fixes long before upstream rebuilds. pinned-images went red on CVE-2026-111497 (libbrotli1, CISA
KEV) in three images whose fixed package Ubuntu already served: Argo CD's final stage had no
upgrade, and keycloak-config-cli and ckan-solr were pinned as shipped. Each Dockerfile under
`images/` now ends on a stage that runs `apt-get upgrade` or `apk upgrade`, then returns to a
non-root user, and each is built by an `image-*.yml` workflow.
"""

import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
DOCKERFILES = sorted((ROOT / "images").glob("*/Dockerfile"))
UPGRADE = re.compile(r"\b(apt-get upgrade|apk upgrade)\b")


def final_stage(text):
    lines = [line for line in text.splitlines() if not line.lstrip().startswith("#")]
    start = max(i for i, line in enumerate(lines) if line.upper().startswith("FROM "))
    return "\n".join(lines[start:])


def problems(text):
    stage = final_stage(text)
    found = []
    if not UPGRADE.search(stage):
        found.append("its final stage upgrades no OS package")
    users = re.findall(r"^USER\s+(\S+)", stage, re.MULTILINE)
    if users and users[-1] in ("root", "0"):
        found.append("it ends as root")
    return found


def built_contexts():
    contexts = set()
    for workflow in (ROOT / ".github/workflows").glob("image-*.yml"):
        document = yaml.safe_load(workflow.read_text())
        for job in document["jobs"].values():
            include = job.get("strategy", {}).get("matrix", {}).get("include", [])
            for step in job.get("steps", []):
                context = (step.get("with") or {}).get("context", "")
                if "${{ matrix.context }}" in context:
                    contexts.update(f"images/{entry['context']}" for entry in include)
                elif context:
                    contexts.add(context)
    return contexts


@pytest.mark.parametrize("dockerfile", DOCKERFILES, ids=lambda p: p.parent.name)
def test_every_image_upgrades_its_os_packages_and_drops_root(dockerfile):
    assert problems(dockerfile.read_text()) == [], dockerfile


@pytest.mark.parametrize("dockerfile", DOCKERFILES, ids=lambda p: p.parent.name)
def test_every_image_is_built_by_a_workflow(dockerfile):
    assert str(dockerfile.parent.relative_to(ROOT)) in built_contexts()


def test_a_final_stage_without_an_upgrade_is_named():
    old_argocd = (
        "FROM alpine@sha256:1 AS tools\nRUN apk upgrade --no-cache\n"
        "FROM quay.io/argoproj/argocd:v3.5.4@sha256:2\nCOPY --from=tools /out/x /usr/local/bin/\n"
    )
    assert problems(old_argocd) == ["its final stage upgrades no OS package"]


def test_an_upgrade_left_as_root_is_named():
    left_as_root = "FROM ubuntu@sha256:1\nUSER root\nRUN apt-get update && apt-get upgrade -y\n"
    assert problems(left_as_root) == ["it ends as root"]
    assert problems(left_as_root + "USER 65534\n") == []
