#!/usr/bin/env python3
"""Which images a push builds (T-2485): the paths it changed, one per line on stdin, become the
build matrix of image.yml on stdout. `ALL` (a tag, a dispatch, a range git cannot diff) builds
every image.

- `ui/**` or `sdk/**` builds `portal-ui`: the UI links the SDK (`link:../sdk`).
- Any path outside `ui/` builds the binary: it embeds the SDK's kit, the docs and the migrations.
- The two builder images only when what they carry moved (T-6154): their Dockerfile and lane
  (`builder/`), the SDK (AP-82), the apps' crates and the locks their crate store is fetched from.
- A change only under `ui/` builds `portal-ui` alone (T-6158, the cut-over of T-2485): every
  installation reads its pages from `portal-ui` (JC_PORTAL_UI_URL, deployment components/portal
  `ui.enabled`), so the copy of ui/dist the binary still embeds is a fallback for a Portal run
  without the bundle (local `cargo run`), and pin-portal.sh keeps the binary pinned for such a
  commit. `--binary` (a ci-full commit promote.yml attests, which needs the binary of its own
  commit) adds the binary whenever the commit lacks one.

    git diff --name-only "$BEFORE" "$GITHUB_SHA" | dev/image-matrix.py owner/repo owner [--binary]
    python3 dev/image-matrix.py --selftest
"""
import json
import sys
from pathlib import Path


def images(repository: str, owner: str) -> dict[str, dict]:
    return {
        "portal": {"name": "portal", "file": "Dockerfile", "image": f"ghcr.io/{repository}"},
        "portal-ui": {
            "name": "portal-ui",
            "file": "Dockerfile",
            "target": "ui-bundle",
            "image": f"ghcr.io/{repository}-ui",
        },
        "app-builder": {
            "name": "app-builder",
            "file": "builder/Dockerfile",
            "image": f"ghcr.io/{owner}/joinedcontext-app-builder",
        },
        # The rust-1.90 runner of fullstack Apps: the same Dockerfile's `rust` stage (AP-106).
        "app-builder-rust": {
            "name": "app-builder-rust",
            "file": "builder/Dockerfile",
            "target": "rust",
            "image": f"ghcr.io/{owner}/joinedcontext-app-builder-rust",
        },
    }


# What builder/Dockerfile copies besides the Cargo manifests and locks anywhere in the tree.
BUILDER_INPUTS = ("builder/", "sdk/", "apps/", "scripts/app-integrity.mjs")


def wanted(paths: list[str], binary: bool = False) -> list[str]:
    paths = [p.strip() for p in paths if p.strip()]
    if "ALL" in paths:
        return ["portal", "portal-ui", "app-builder", "app-builder-rust"]
    names = []
    if any(p.startswith(("ui/", "sdk/")) for p in paths):
        names.append("portal-ui")
    if binary or any(not p.startswith("ui/") for p in paths):
        names = ["portal", *names]
        if any(p.startswith(BUILDER_INPUTS) or p.rsplit("/", 1)[-1] in ("Cargo.toml", "Cargo.lock") for p in paths):
            names += ["app-builder", "app-builder-rust"]
    return names


def matrix(paths: list[str], repository: str, owner: str, binary: bool = False) -> dict:
    known = images(repository, owner)
    return {"include": [known[name] for name in wanted(paths, binary)]}


def selftest() -> None:
    # A ui/-only change: the bundle alone, ~2 min instead of the binary's ~11 (T-6158).
    assert wanted(["ui/src/App.tsx", "ui/package.json"]) == ["portal-ui"]
    assert wanted(["ui/src/App.tsx"], binary=True) == ["portal", "portal-ui"]
    assert wanted([], binary=True) == ["portal"]
    assert wanted(["src/main.rs"]) == ["portal"]
    for path in ("builder/lane.mjs", "scripts/app-integrity.mjs", "apps/air-quality/src/main.rs", "Cargo.lock", "apps/hsl-transport/server/Cargo.toml"):
        assert wanted([path]) == ["portal", "app-builder", "app-builder-rust"], path
    assert wanted(["src/a.rs", "Cargo.lock"]) == ["portal", "app-builder", "app-builder-rust"]
    assert wanted(["sdk/src/sdk/index.ts"]) == ["portal", "portal-ui", "app-builder", "app-builder-rust"]
    assert wanted(["ui/src/a.tsx", "src/b.rs"]) == ["portal", "portal-ui"]
    # A docs-only push builds the binary as before: its public docs are embedded (src/public_docs.rs).
    assert wanted(["public-docs/index.md"]) == ["portal"]
    assert wanted(["ALL"]) == ["portal", "portal-ui", "app-builder", "app-builder-rust"]
    # Nothing changed (an empty merge): nothing to build, and image.yml skips the job.
    assert wanted([]) == [] and wanted(["", "  "]) == []
    # `ui` as a prefix of another folder is not the UI, and a path no rule names builds the binary.
    assert wanted(["uikit/x"]) == ["portal"]
    assert wanted(["newdir/x.txt"]) == ["portal"]
    built = matrix(["ui/x"], "o/joinedcontext-portal", "o")["include"]
    assert built == [
        {
            "name": "portal-ui",
            "file": "Dockerfile",
            "target": "ui-bundle",
            "image": "ghcr.io/o/joinedcontext-portal-ui",
        }
    ], built
    # The rust runner check pulls the pushed image beside the builder's own copy: the builder is
    # pruned first, or the runner's disk fills (T-6406).
    workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/image.yml").read_text()
    prune, run = workflow.find("docker buildx prune --all --force"), workflow.find("--entrypoint sh \"${{ matrix.image }}@")
    assert 0 <= prune < run, "image.yml: prune the buildx builder before the rust runner check pulls the image"
    print("image-matrix: selftest ok")


def main(argv: list[str]) -> int:
    if argv[1:] == ["--selftest"]:
        selftest()
        return 0
    binary = argv[3:] == ["--binary"]
    if len(argv) != 3 + binary:
        print(__doc__, file=sys.stderr)
        return 2
    print(json.dumps(matrix(sys.stdin.read().splitlines(), argv[1], argv[2], binary)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
