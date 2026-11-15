#!/usr/bin/env python3
"""Every image and upstream chart this deployment pins, as one inventory (OPS-19, OPS-20).

The pins already live in `components/*/images.yaml` and `components/*/charts.yaml`, one file
per component, because that is where they are re-pinned. This reads all of them and writes a
single YAML document that `package-airgap.sh` pulls from and `load-airgap.sh` pushes from, so
the archive and the deployment can never name different bytes.

    scripts/airgap-inventory.py [repo-root] > manifest.yaml

An image is named by digest wherever one is pinned, which is everywhere the deployment is
correct: the tag is carried alongside for readability and is not what is pulled.
"""

import sys
from pathlib import Path

import yaml


def load(path: Path) -> dict:
    if not path.is_file():
        return {}
    return yaml.safe_load(path.read_text()) or {}


def parts(document: dict):
    for component, entries in document.items():
        if not isinstance(entries, dict):
            continue
        for part, values in entries.items():
            if isinstance(values, dict):
                yield component, part, values


def images_of(root: Path) -> list[dict]:
    found = []
    for path in sorted(root.glob("components/*/images.yaml")):
        for component, part, values in parts(load(path)):
            repository = values.get("repository")
            if not repository:
                continue
            registry = values.get("registry")
            name = f"{registry}/{repository}" if registry else repository
            digest, tag = values.get("digest"), values.get("tag")
            if not digest:
                raise SystemExit(
                    f"{path}: {component}.{part} pins no digest. An air-gapped archive of a "
                    f"floating tag is an archive of whatever the registry served that day, "
                    f"and the copy inside the zone would not be the copy that was reviewed."
                )
            found.append({
                "component": component, "part": part, "repository": name,
                "tag": tag, "digest": digest, "reference": f"{name}@{digest}",
            })
    return found


def charts_of(root: Path) -> list[dict]:
    found = []
    for path in sorted(root.glob("components/*/charts.yaml")):
        for component, part, values in parts(load(path)):
            repository = str(values.get("repository") or "")
            # A chart without a repository URL is one of this repository's own, already in
            # the checkout the operator carries in.
            if not repository.startswith(("http://", "https://")):
                continue
            chart = str(values.get("chart") or "")
            version = values.get("version")
            if not version:
                raise SystemExit(f"{path}: {component}.{part} pins no chart version")
            found.append({
                "component": component, "part": part, "repository": repository,
                # `chart` is written as `<repo-alias>/<name>`; the alias is helmfile's and
                # means nothing to `helm pull --repo`.
                "chart": chart.split("/")[-1], "version": str(version),
            })
    return found


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    images = images_of(root)
    charts = charts_of(root)
    if not images:
        raise SystemExit(f"{root}: no components/*/images.yaml found")
    unique_images = {i["reference"]: i for i in images}
    unique_charts = {(c["repository"], c["chart"], c["version"]): c for c in charts}
    yaml.safe_dump({
        "images": sorted(unique_images.values(), key=lambda i: i["reference"]),
        "charts": sorted(unique_charts.values(), key=lambda c: (c["chart"], c["version"])),
    }, sys.stdout, default_flow_style=False, sort_keys=False)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
