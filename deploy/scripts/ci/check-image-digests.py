#!/usr/bin/env python3
"""Every container of the rendered deployment is pinned by digest (OPS-28, SEC-GAP-03).

    scripts/ci/check-image-digests.py rendered.yaml

A tag moves. An image pinned by `:main` is whatever the registry served the minute the kubelet
pulled it, which is not the image anybody reviewed, and a compromised registry needs no
signature to replace it. `components/*/images.yaml` is already held to this by
`scripts/airgap-inventory.py`; what this reads is the rendered output, where an upstream
chart's own default tag is the one that gets through.
"""

import sys
from pathlib import Path

import yaml

# `image:` under these keys is a container image. Anywhere else in a rendered document the
# word means a values block, an annotation or a comment about one.
CONTAINER_KEYS = ("containers", "initContainers", "ephemeralContainers")


def containers(document):
    """Every container of a document, whatever workload shape carries the pod template."""
    if isinstance(document, dict):
        for key, value in document.items():
            if key in CONTAINER_KEYS and isinstance(value, list):
                yield from (c for c in value if isinstance(c, dict))
            else:
                yield from containers(value)
    elif isinstance(document, list):
        for item in document:
            yield from containers(item)


def deployed(path: Path):
    """Every container the cluster actually runs, as `(where, container)`."""
    for document in yaml.safe_load_all(path.read_text()):
        if not isinstance(document, dict):
            continue
        # A Helm *test* hook is never applied by `helmfile sync`, so it is not a deployed
        # workload; `scripts/render.sh --deployed` and the Kyverno gate drop it for the same
        # reason. Every other hook is applied and is read like anything else.
        annotations = (document.get("metadata") or {}).get("annotations") or {}
        if "test" in str(annotations.get("helm.sh/hook", "")).split(","):
            continue
        # A Kyverno policy's `containers:` is a pattern (`image: '*@*'`), not a workload.
        if str(document.get("apiVersion", "")).startswith("kyverno.io/"):
            continue
        where = f"{document.get('kind', '?')}/{(document.get('metadata') or {}).get('name', '?')}"
        for container in containers(document):
            yield where, container


def unpinned(path: Path) -> list[str]:
    return [
        f"{where}: {container.get('name', '?')} runs {container['image']}"
        for where, container in deployed(path)
        if isinstance(container.get("image"), str) and "@sha256:" not in container["image"]
    ]


def drifted(path: Path) -> list[str]:
    """One repository, one digest (OPS-13, AG-52).

    Several workloads run the same image on purpose: the context gateway, the agent proxy and
    jc-functions are three deployments of one `joinedcontext-platform` build, so a token is never
    minted by one and verified by another. Each pins its own digest in its own component, and
    nothing kept the three equal — the gateway was once eleven commits ahead of the proxy, which
    shipped a fix *to the proxy* (T-1477) everywhere except the process that performs it. Two
    digests behind one repository is that drift, whatever the component files say.
    """
    seen: dict[str, dict[str, list[str]]] = {}
    for where, container in deployed(path):
        image = container.get("image")
        if not isinstance(image, str) or "@sha256:" not in image:
            continue  # unpinned() reports this one
        reference, digest = image.split("@", 1)
        repository = reference.split(":", 1)[0]
        seen.setdefault(repository, {}).setdefault(digest, []).append(
            f"{where}/{container.get('name', '?')}"
        )
    found = []
    for repository, digests in sorted(seen.items()):
        if len(digests) > 1:
            builds = "; ".join(
                f"{digest[:19]}… on {', '.join(sorted(places))}"
                for digest, places in sorted(digests.items())
            )
            found.append(f"{repository} runs {len(digests)} builds at once: {builds}")
    return found


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    rendered = Path(argv[1])
    found = unpinned(rendered)
    for line in found:
        print(f"::error::{line}", file=sys.stderr)
    if found:
        print(
            f"{len(found)} container(s) run a mutable tag. Pin the digest in the component's "
            f"images.yaml, or in the chart values for an upstream chart that carries its own "
            f"default.",
            file=sys.stderr,
        )
        return 1
    found = drifted(rendered)
    for line in found:
        print(f"::error::{line}", file=sys.stderr)
    if found:
        print(
            f"{len(found)} repository/repositories are pinned to more than one digest. The "
            f"components that share an image share its build: set the same digest in every "
            f"components/*/images.yaml that names that repository.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
