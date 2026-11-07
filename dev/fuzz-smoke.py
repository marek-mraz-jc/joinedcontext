#!/usr/bin/env python3
"""Fuzz smoke: every fuzz target a change touches, 30 s each, plus its committed crash replay
(surfaces-test-plan.md section 5, T-3492). The blocking twin of the twice-weekly fuzz.yml.

A target is touched when the change edits:
  - its own source (fuzz/fuzz_targets/<t>.rs) or its regression corpus (fuzz/regressions/<t>/);
  - any file of a workspace crate the target names (`antares_ql::`, `use antares_api`), or of a
    crate those depend on, through [dependencies] and [build-dependencies] (path or
    `workspace = true` deps); dev-dependencies do not reach a fuzz binary;
  - fuzz/Cargo.toml, fuzz/Cargo.lock, the root Cargo.toml or Cargo.lock: a dependency bump can
    change any parser, so every target runs.
The crate map is read from the manifests and the target sources, never a table that can rot:
a new target or a new parser crate joins by existing.

  dev/fuzz-smoke.py --list [--base <rev>]   the touched targets, one per line
  dev/fuzz-smoke.py --run  [--base <rev>]   replay and fuzz each of them
  dev/fuzz-smoke.py --self-test

Without `--base` (a dispatch run, no pull request) every target is touched.
"""

import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
EVERYTHING = {"fuzz/Cargo.toml", "fuzz/Cargo.lock", "Cargo.toml", "Cargo.lock"}
SECONDS = 30
MALLOC_LIMIT_MB = 512


def workspace(root):
    """{package name: (crate dir relative to root, lib name, set of workspace deps)}."""
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    shared = manifest.get("workspace", {}).get("dependencies", {})
    members = manifest.get("workspace", {}).get("members", [])
    packages = {}
    for member in members:
        crate = tomllib.loads((root / member / "Cargo.toml").read_text())
        name = crate["package"]["name"]
        lib = crate.get("lib", {}).get("name", name.replace("-", "_"))
        tables = [crate.get("dependencies", {}), crate.get("build-dependencies", {})]
        for target in crate.get("target", {}).values():
            tables += [target.get("dependencies", {}), target.get("build-dependencies", {})]
        deps = set()
        for table in tables:
            for dep, spec in table.items():
                spec = spec if isinstance(spec, dict) else {}
                package = spec.get("package", dep)
                if "path" in spec or (spec.get("workspace") and "path" in shared.get(package, {})):
                    deps.add(package)
        packages[name] = (member.rstrip("/"), lib, deps)
    return packages


def targets(root):
    """{target name: source path relative to root} from fuzz/Cargo.toml."""
    manifest = tomllib.loads((root / "fuzz/Cargo.toml").read_text())
    return {binary["name"]: f"fuzz/{binary['path']}" for binary in manifest.get("bin", [])}


def crate_dirs(root, source, packages):
    """The crate dirs one target's source reaches, through the workspace dependency graph."""
    text = (root / source).read_text()
    reached = {
        name
        for name, (_, lib, _) in packages.items()
        if re.search(r"\b" + re.escape(lib) + r"\b", text)
    }
    queue = list(reached)
    while queue:
        for dep in packages[queue.pop()][2]:
            if dep in packages and dep not in reached:
                reached.add(dep)
                queue.append(dep)
    return {packages[name][0] for name in reached}


def touched(changed, all_targets, reach):
    """The targets a change touches; `changed` None is "no base", so every target."""
    if changed is None or EVERYTHING & set(changed):
        return sorted(all_targets)
    picked = set()
    for target, source in all_targets.items():
        prefixes = [f"fuzz/regressions/{target}/"] + [f"{d}/" for d in reach[target]]
        if any(path == source or path.startswith(tuple(prefixes)) for path in changed):
            picked.add(target)
    return sorted(picked)


def selection(root, base):
    all_targets = targets(root)
    packages = workspace(root)
    reach = {t: crate_dirs(root, source, packages) for t, source in all_targets.items()}
    changed = None
    if base:
        diff = subprocess.run(
            ["git", "-C", str(root), "diff", "--name-only", f"{base}...HEAD"],
            check=True,
            capture_output=True,
            text=True,
        )
        changed = [line for line in diff.stdout.splitlines() if line]
    return touched(changed, all_targets, reach)


def run(root, picked):
    for target in picked:
        corpus = root / "fuzz/regressions" / target
        steps = []
        if corpus.is_dir() and any(corpus.iterdir()):
            # -runs=0 executes the committed crash inputs and fuzzes nothing new.
            steps.append(["cargo", "+nightly", "fuzz", "run", target, str(corpus), "--", "-runs=0"])
        steps.append(
            [
                "cargo", "+nightly", "fuzz", "run", target, "--",
                f"-max_total_time={SECONDS}", f"-malloc_limit_mb={MALLOC_LIMIT_MB}",
            ]
        )
        for step in steps:
            print("+", " ".join(step), flush=True)
            if subprocess.run(step, cwd=root).returncode != 0:
                print(f"fuzz-smoke: {target} failed; its input is under fuzz/artifacts/{target}/")
                return 1
    print(f"fuzz-smoke: {len(picked)} target(s) green: {' '.join(picked) or 'none touched'}")
    return 0


def self_test():
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        files = {
            "Cargo.toml": '[workspace]\nmembers = ["crates/a", "crates/b", "crates/c", "crates/d"]\n'
            '[workspace.dependencies]\na = { path = "crates/a" }\n',
            "crates/a/Cargo.toml": '[package]\nname = "a"\n',
            "crates/b/Cargo.toml": '[package]\nname = "b"\n[dependencies]\na = { workspace = true }\n'
            '[dev-dependencies]\nd = { path = "../d" }\n',
            "crates/c/Cargo.toml": '[package]\nname = "c"\n[lib]\nname = "see"\n',
            "crates/d/Cargo.toml": '[package]\nname = "d"\n',
            "fuzz/Cargo.toml": '[[bin]]\nname = "t1"\npath = "fuzz_targets/t1.rs"\n'
            '[[bin]]\nname = "t2"\npath = "fuzz_targets/t2.rs"\n',
            "fuzz/fuzz_targets/t1.rs": "use b::parse;\n",
            "fuzz/fuzz_targets/t2.rs": "fuzz_target!(|data| { see::read(data); });\n",
        }
        for path, text in files.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text)
        all_targets = targets(root)
        packages = workspace(root)
        reach = {t: crate_dirs(root, s, packages) for t, s in all_targets.items()}
    cases = [
        ("a crate a target names", ["crates/b/src/lib.rs"], ["t1"]),
        ("a crate one depends on", ["crates/a/src/q.rs"], ["t1"]),
        ("a dev-dependency only", ["crates/d/src/lib.rs"], []),
        ("a renamed lib", ["crates/c/src/lib.rs"], ["t2"]),
        ("the target's own source", ["fuzz/fuzz_targets/t2.rs"], ["t2"]),
        ("its crash corpus", ["fuzz/regressions/t2/crash-1"], ["t2"]),
        ("a dependency bump", ["Cargo.lock"], ["t1", "t2"]),
        ("the fuzz manifest", ["fuzz/Cargo.toml"], ["t1", "t2"]),
        ("docs only", ["docs/x.md", "crates/bee/src/lib.rs"], []),
        ("no base", None, ["t1", "t2"]),
    ]
    failed = [
        f"{name}: {got} != {want}"
        for name, changed, want in cases
        if (got := touched(changed, all_targets, reach)) != want
    ]
    if failed:
        print("fuzz-smoke self-test FAILED:\n  " + "\n  ".join(failed))
        return 1
    print(f"fuzz-smoke self-test: {len(cases)} selection cases as expected")
    return 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if not argv or argv[0] not in ("--list", "--run") or len(argv) not in (1, 3) or (
        len(argv) == 3 and argv[1] != "--base"
    ):
        print("usage: dev/fuzz-smoke.py --list|--run [--base <rev>] | --self-test", file=sys.stderr)
        return 2
    picked = selection(ROOT, argv[2] if len(argv) == 3 else None)
    if argv[0] == "--list":
        print("\n".join(picked))
        return 0
    return run(ROOT, picked)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
