#!/usr/bin/env python3
"""Every cargo target of the workspace is stubbed in the Dockerfile's dependency layer (T-6108).

The image build copies each member's Cargo.toml and writes a stub at every target path before
`cargo build`, so the compiled dependencies are a layer the gha cache keeps. Cargo refuses a
manifest whose targets are missing, and `--locked` refuses a workspace that lost a member of
Cargo.lock, so an app crate added under `apps/*` and not stubbed there breaks the image build, which
nothing in the fast lane would say. Port of the platform's check (T-3366), with the Portal's
`members = ["apps/*"]` glob expanded the way cargo does: every matching directory not excluded.

    python3 scripts/ci/check-dockerfile-members.py [Cargo.toml] [Dockerfile]
    python3 scripts/ci/check-dockerfile-members.py --apps Dockerfile ../../bins/jc/Dockerfile
    python3 scripts/ci/check-dockerfile-members.py --selftest

`--apps` compares the App bundles two Dockerfiles ship in /srv/apps: the Portal's (build context
crates/portal) and jc's (context the repository root, so every source path carries
`crates/portal/`). Both must build the same Apps from the same sources, copy /srv/apps into the
runtime image and set JC_PORTAL_APPS_DIR to it, or one image answers 404 for an App the other
serves.
"""
import re
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


def shipped_apps(dockerfile: str, prefix: str) -> dict[str, list[str]]:
    """What a Dockerfile ships in /srv/apps: the App names, the app sources it copies (with
    `prefix` cut off) and the runtime lines that serve them."""
    stage = dockerfile.split(" AS apps\n", 1)[-1].split("\nFROM ", 1)[0]
    names = set(re.findall(r"app-integrity\.mjs \"?/srv/apps/([A-Za-z0-9_-]+)", stage))
    for loop in re.findall(r"for name in ([^;]+); do\s*\\?\s*cp -r [^ ]+ \"/srv/apps/\$name\"", stage):
        names |= set(loop.split())
    # Every source of a COPY into the build tree, the last word being its destination.
    sources = {
        src.removeprefix(prefix) if src.startswith(prefix) else f"(no {prefix}) {src}"
        for line in re.findall(r"^COPY (.+ \./(?:apps|sdk|scripts)/\S*)$", stage, re.MULTILINE)
        for src in line.split()[:-1]
    }
    runtime = [
        line
        for line in ("COPY --from=apps /srv/apps /srv/apps", "ENV JC_PORTAL_APPS_DIR=/srv/apps")
        if line in dockerfile
    ]
    return {"apps": sorted(names), "sources": sorted(sources), "runtime": runtime}


def apps_differ(portal: str, jc: str) -> list[str]:
    ours, theirs = shipped_apps(portal, ""), shipped_apps(jc, "crates/portal/")
    out = []
    if not ours["apps"]:
        out.append("the Portal Dockerfile ships no App in /srv/apps")
    for key, what in (("apps", "app"), ("sources", "source"), ("runtime", "runtime line")):
        for item in sorted(set(ours[key]) - set(theirs[key])):
            out.append(f"bins/jc/Dockerfile lacks {what} `{item}` the Portal Dockerfile has")
        for item in sorted(set(theirs[key]) - set(ours[key])):
            out.append(f"bins/jc/Dockerfile has {what} `{item}` the Portal Dockerfile lacks")
    return out


def apps_selftest() -> None:
    portal = (
        "FROM node AS apps\n"
        "COPY apps/a/ui/package.json apps/a/ui/pnpm-lock.yaml ./apps/a/ui/\n"
        "COPY apps/a/ui/ ./apps/a/ui/\n"
        "COPY scripts/app-integrity.mjs ./scripts/\n"
        "    && node /work/scripts/app-integrity.mjs /srv/apps/a\n"
        "RUN for name in s-one s-two; do \\\n"
        '      cp -r apps/p/bundle "/srv/apps/$name" \\\n'
        '      && node /work/scripts/app-integrity.mjs "/srv/apps/$name" || exit 1; \\\n'
        "FROM scratch AS build\n"
        "COPY sdk/template ./sdk/template\n"
        "FROM distroless\n"
        "COPY --from=apps /srv/apps /srv/apps\n"
        "ENV JC_PORTAL_APPS_DIR=/srv/apps\n"
    )
    assert shipped_apps(portal, "")["apps"] == ["a", "s-one", "s-two"]
    jc = re.sub(r"(?<=[ ])(apps/|scripts/)(?=\S* )", r"crates/portal/\1", portal)
    assert apps_differ(portal, jc) == [], apps_differ(portal, jc)
    assert apps_differ(portal, jc.replace("/srv/apps/a\n", "/srv/apps/b\n")) == [
        "bins/jc/Dockerfile lacks app `a` the Portal Dockerfile has",
        "bins/jc/Dockerfile has app `b` the Portal Dockerfile lacks",
    ]
    assert apps_differ(portal, jc.replace("s-one s-two", "s-one")) == [
        "bins/jc/Dockerfile lacks app `s-two` the Portal Dockerfile has",
    ]
    assert apps_differ(portal, jc.replace("COPY crates/portal/apps/a/ui/ ./apps/a/ui/\n", "")) == [
        "bins/jc/Dockerfile lacks source `apps/a/ui/` the Portal Dockerfile has",
    ]
    # A second source left without its crates/portal/ prefix (jc's context is the repository root).
    unprefixed = jc.replace(" crates/portal/apps/a/ui/pnpm-lock.yaml", " apps/a/ui/pnpm-lock.yaml")
    assert apps_differ(portal, unprefixed) == [
        "bins/jc/Dockerfile lacks source `apps/a/ui/pnpm-lock.yaml` the Portal Dockerfile has",
        "bins/jc/Dockerfile has source `(no crates/portal/) apps/a/ui/pnpm-lock.yaml` the Portal Dockerfile lacks",
    ], apps_differ(portal, unprefixed)
    assert apps_differ(portal, jc.replace("ENV JC_PORTAL_APPS_DIR=/srv/apps\n", "")) == [
        "bins/jc/Dockerfile lacks runtime line `ENV JC_PORTAL_APPS_DIR=/srv/apps` the Portal Dockerfile has",
    ]
    assert apps_differ("", "") == ["the Portal Dockerfile ships no App in /srv/apps"]


def selftest() -> None:
    apps_selftest()
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
    if argv[:1] == ["--apps"]:
        portal_path, jc_path = (argv[1:3] + ["Dockerfile", "../../bins/jc/Dockerfile"][len(argv[1:3]):])[:2]
        problems = apps_differ(Path(portal_path).read_text(), Path(jc_path).read_text())
        for problem in problems:
            print(problem, file=sys.stderr)
        if not problems:
            print(f"{len(shipped_apps(Path(portal_path).read_text(), '')['apps'])} Apps in both Dockerfiles")
        return 1 if problems else 0
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
