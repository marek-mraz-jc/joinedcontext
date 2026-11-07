#!/usr/bin/env python3
"""Every cargo target of the workspace is stubbed in the Dockerfile's dependency layer (T-6108).

The image build copies each member's Cargo.toml and writes a stub at every target path before
`cargo build`, so the compiled dependencies are a layer the gha cache keeps. Cargo refuses a
manifest whose targets are missing, and `--locked` refuses a workspace that lost a member of
Cargo.lock, so an app crate added under `apps/*` and not stubbed there breaks the image build, which
nothing in the fast lane would say. Port of the platform's check (T-3366), with the Portal's
`members = ["apps/*"]` glob expanded the way cargo does: every matching directory not excluded.

    python3 scripts/ci/check-dockerfile-members.py [Cargo.toml] [Dockerfile]
    python3 scripts/ci/check-dockerfile-members.py --selftest
"""
import sys
import tomllib
from pathlib import Path

DEFAULT_DIRS = {"bin": "src/bin", "test": "tests", "bench": "benches", "example": "examples"}


def target_paths(manifest: str) -> list[str]:
    """Paths of the lib and the targets a Cargo.toml names explicitly, relative to the package."""
    parsed = tomllib.loads(manifest)
    paths = [parsed["lib"].get("path", "src/lib.rs")] if "lib" in parsed else []
    return paths + [
        target.get("path", f"{folder}/{target['name']}.rs")
        for kind, folder in DEFAULT_DIRS.items()
        for target in parsed.get(kind, [])
    ]


def members(root: Path, cargo: str) -> list[str]:
    workspace = tomllib.loads(cargo)["workspace"]
    excluded = set(workspace.get("exclude", []))
    found = []
    for pattern in workspace["members"]:
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            if path.is_dir() and rel not in excluded:
                found.append(rel)
    return found


def missing(cargo: str, dockerfile: str, manifests: dict[str, str]) -> list[str]:
    """`manifests`: member path ("" for the root package) -> its Cargo.toml text."""
    out = []
    for member, manifest in manifests.items():
        prefix = f"{member}/" if member else ""
        name = member or "the root package"
        if member and f"COPY {member}/Cargo.toml {member}/Cargo.toml" not in dockerfile:
            out.append(f"{name}: no `COPY {member}/Cargo.toml` in the dependency layer")
        for path in target_paths(manifest):
            if f"> {prefix}{path}" not in dockerfile:
                out.append(f"{name}: the stub build writes no `{prefix}{path}` its Cargo.toml names")
    return out


def selftest() -> None:
    cargo = '[workspace]\nmembers = ["apps/*"]\n'
    lib_bin = '[lib]\npath = "src/lib.rs"\n[[bin]]\nname = "a"\npath = "src/main.rs"\n'
    good = (
        "COPY apps/a/Cargo.toml apps/a/Cargo.toml\n"
        " && echo 'pub fn _dep_cache() {}' > apps/a/src/lib.rs \\\n"
        " && echo 'fn main() {}' > apps/a/src/main.rs \\\n"
    )
    assert missing(cargo, good, {"apps/a": lib_bin}) == []
    assert missing(cargo, good, {"apps/a": lib_bin, "apps/b": "[package]\n"}) == [
        "apps/b: no `COPY apps/b/Cargo.toml` in the dependency layer",
    ]
    bench = lib_bin + '[[bench]]\nname = "speed"\n'
    assert missing(cargo, good, {"apps/a": bench}) == [
        "apps/a: the stub build writes no `apps/a/benches/speed.rs` its Cargo.toml names",
    ]
    assert missing(cargo, good, {"": lib_bin}) == [
        "the root package: the stub build writes no `src/lib.rs` its Cargo.toml names",
        "the root package: the stub build writes no `src/main.rs` its Cargo.toml names",
    ]


def main(argv: list[str]) -> int:
    if argv[:1] == ["--selftest"]:
        selftest()
        print("check-dockerfile-members selftest ok")
        return 0
    cargo_path, docker_path = (argv[:2] + ["Cargo.toml", "Dockerfile"][len(argv[:2]):])[:2]
    root = Path(cargo_path).parent
    cargo = Path(cargo_path).read_text()
    manifests = {"": cargo} | {m: (root / m / "Cargo.toml").read_text() for m in members(root, cargo)}
    problems = missing(cargo, Path(docker_path).read_text(), manifests)
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
