#!/usr/bin/env python3
"""The releases a range changes, for `just dev-apply` (T-6135) and `just dev-apply-pins` (T-6110).

    scripts/apply-pins.py plan <environment> <base> [<fullSyncAt>]
                                                          "full", or the releases whose render moved since <base>
    scripts/apply-pins.py releases <environment> <base>   `<release> <component> local|remote` per release a
                                                          pin-only range moved (refuses any other file)

An images.yaml feeds every component's values (`.Values.images`, defaults/environment), so the
Portal's pin also moves agent-runner, gitea-runner and mqtt-brokers objects. Guessing by
component would miss those. `plan` renders the environment at <base> (a git worktree) and
at HEAD, one directory per release, and names each release with an object added or changed.
`releases` (T-6155, the per-change path) compares the merged values helmfile hands each release
instead (`helmfile write-values`, half a render's time): a pin-only range changes no chart and no
helmfile, so a release renders anything new only when its values moved. `local` marks a release
whose chart is a directory of its component, which `helmfile sync --skip-deps` syncs without
adding a chart repository.
Secret values and checksum/* annotations are masked as scripts/release-diff.py masks them: the
secrets chart invents fresh values on every render.

HEAD is rendered twice: an object whose two HEAD renders differ carries random values (a
chart's generated password), so only that object is compared masked; every other object,
deterministic Secrets included, is compared byte for byte.

`plan` answers "full" (the reason on stderr) whenever a scoped sync could leave a change
unapplied: no trusted base (none recorded, a dirty one, an unreadable range), a change to how
helmfile builds releases outside one component (deployment/, the assembled defaults, the example
helmfile), a failed render, a release added or removed, a CustomResourceDefinition added, removed
or changed, or no full sync recorded within JC_FULL_SYNC_HOURS (default 6): a scoped sync judges
the render, never the live objects, so the periodic full sync is what converges drift and objects
deleted by hand. An empty range plans no release.

A component's own helmfile or component.yaml (T-6187) changes its releases' hooks, needs and
conditions, which the render does not show: `plan` adds every installed release labelled with
that component (`helmfile list`) to the releases whose render moved, instead of a full sync of all
~370 releases (0ca12ec4: one comment in the prepare hook upgraded 369 releases in 1781 s). A
component with no release of its own (prepare: namespaces and mesh annotations, a hook the scoped
sync skips with JC_PREPARE_SKIP=pins) is the exception: its change is a full sync.

`releases` refuses (exit 3) a range that touches any file other than a components/<c>/images.yaml.
Both need deployment/ assembled at HEAD (`just _dev-assemble`); `plan` also needs the
environment's chart repositories added.
"""

from __future__ import annotations

import contextlib
import functools
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent


def _module(path: Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


changes = _module(ROOT / "scripts/ci/changes.py", "changes")
release_diff = _module(ROOT / "scripts/release-diff.py", "release_diff")


class Refused(Exception):
    pass


FULL = "full"
# Files that change how helmfile builds a release (needs, hooks, conditions, the release list)
# rather than what it renders: a scoped sync cannot be judged from the render.
STRUCTURE = re.compile(r"(^|/)helmfile[^/]*\.(ya?ml|gotmpl)$|^components/[^/]+/component\.yaml$"
                       r"|^defaults/deployment/|^\ci/example-deployments/helmfile\.yaml$")
# A structure file inside one component changes only that component's releases.
COMPONENT = re.compile(r"^components/([^/]+)/")
FULL_SYNC_HOURS = 6
# The forge seed and the sample Apps are ConfigMaps of their own releases, committed to the forge
# only by gitea-bootstrap's post-upgrade Job, whose render does not move with them: a scoped sync
# of the ConfigMaps alone left dev's forge on the old seed until the next full sync (T-6593).
SEED_RELEASE = re.compile(r"^gitea-(seed|sample-apps)(-[0-9]+)?$")
SEED_WRITER = "gitea-bootstrap"


def objects(release_dir: Path) -> dict[str, tuple[str, str]]:
    """`kind namespace/name` -> (the object, the object masked), over every file helmfile wrote for a release."""
    out = {}
    for path in sorted(release_dir.rglob("*.yaml")):
        for doc in yaml.safe_load_all(path.read_text(encoding="utf-8")):
            if isinstance(doc, dict) and doc.get("kind"):
                meta = doc.get("metadata") or {}
                key = f"{doc['kind']} {meta.get('namespace') or '-'}/{meta.get('name')}"
                exact = yaml.safe_dump(doc, sort_keys=True)
                out[key] = (exact, yaml.safe_dump(release_diff.normalise(doc), sort_keys=True))
    return out


def _differs(old: dict, new: dict, again: dict | None) -> list[str]:
    """Object keys added, removed or changed; an object `again` (a second HEAD render) does not
    reproduce is compared masked, every other one exactly."""
    out = sorted(old.keys() ^ new.keys())
    for key in old.keys() & new.keys():
        noisy = again is None or again.get(key, ("", ""))[0] != new[key][0]
        if old[key][1 if noisy else 0] != new[key][1 if noisy else 0]:
            out.append(key)
    return out


def moved(old: Path, new: Path, again: Path | None = None) -> list[str]:
    """Release names under `new` whose objects differ from `old`; a release gone is refused."""
    gone = sorted({p.name for p in old.iterdir()} - {p.name for p in new.iterdir()})
    if gone:
        raise Refused(f"releases gone at HEAD: {', '.join(gone)}; run the full `just dev-apply`")
    out = []
    for r in sorted(new.iterdir(), key=lambda p: p.name):
        twice = objects(again / r.name) if again and (again / r.name).is_dir() else None
        if not (old / r.name).is_dir() or _differs(objects(old / r.name), objects(r), twice):
            out.append(r.name)
    return out


def scoped(old: Path, new: Path, again: Path) -> list[str]:
    """The releases to sync, or Refused when only a full sync is safe: a release added or
    removed, or a CustomResourceDefinition that moved (other releases' objects depend on it)."""
    added = sorted({p.name for p in new.iterdir()} - {p.name for p in old.iterdir()})
    if added:
        raise Refused(f"releases added at HEAD: {', '.join(added)}")
    names = moved(old, new, again)
    for name in names:
        twice = objects(again / name) if (again / name).is_dir() else None
        crds = [k for k in _differs(objects(old / name), objects(new / name), twice)
                if k.startswith("CustomResourceDefinition ")]
        if crds:
            raise Refused(f"{name} changes {', '.join(crds[:3])}")
    return names


def render(tree: Path, environment: str, out: Path) -> None:
    done = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", environment, "template", "--skip-deps", "-q",
         "--output-dir", str(out), "--output-dir-template", "{{ .OutputDir }}/{{ .Release.Name }}"],
        cwd=tree, capture_output=True, text=True, check=False)
    if done.returncode:
        raise Refused(f"render of {environment} in {tree} failed:\n{done.stderr[-4000:]}")


def git(*args: str) -> str:
    done = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, check=False)
    if done.returncode:
        raise Refused(f"git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout


def releases(environment: str, base: str) -> list[str]:
    files = changes.changed(base, "HEAD")
    if files is None:
        raise Refused(f"cannot read the range {base}..HEAD; run the full `just dev-apply`")
    if not files:
        return []
    if not changes.pins_only(files):
        others = [f for f in files if not changes.PIN.match(f)]
        raise Refused(f"{base[:12]}..HEAD changes more than pins ({', '.join(others[:5])}); "
                      "run the full `just dev-apply`")
    with _base_tree(base) as tree:
        with ThreadPoolExecutor(2) as pool:
            for job in [pool.submit(write_values, tree, environment, tree.parent / "old"),
                        pool.submit(write_values, ROOT, environment, tree.parent / "new")]:
                job.result()
        return values_moved(tree.parent / "old", tree.parent / "new")


VALUES = ('{{ .Release.Namespace }}/{{ .Release.Name }}/{{ index .Release.Labels "component" }}/'
          '{{ .Release.Chart | urlquery }}.yaml')


def write_values(tree: Path, environment: str, out: Path) -> None:
    """Each release's merged values to out/<namespace>/<release>/<component>/<chart, url-quoted>.yaml."""
    done = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", environment, "write-values", "--skip-deps",
         "--output-file-template", f"{out}/{VALUES}"],
        cwd=tree, capture_output=True, text=True, check=False)
    if done.returncode:
        raise Refused(f"write-values of {environment} in {tree} failed:\n{done.stderr[-4000:]}")


def release_values(out: Path) -> dict[str, tuple[str, str, object]]:
    """`namespace/release` -> (component, chart, merged values)."""
    found = {}
    for path in out.glob("*/*/*/*.yaml"):
        namespace, name, component = path.parts[-4:-1]
        found[f"{namespace}/{name}"] = (component, urllib.parse.unquote_plus(path.stem),
                                        yaml.safe_load(path.read_text(encoding="utf-8")))
    return found


def values_moved(old: Path, new: Path) -> list[str]:
    """`<release> <component> local|remote` for each release whose values, chart or component moved."""
    before, after = release_values(old), release_values(new)
    if not after:
        raise Refused("write-values wrote no release")
    if before.keys() != after.keys():
        names = sorted(before.keys() ^ after.keys())
        raise Refused(f"releases added or removed: {', '.join(names[:5])}; run the full `just dev-apply`")
    out = set()
    for key, (component, chart, _) in after.items():
        if before[key] == after[key]:
            continue
        if component == "<no value>":
            raise Refused(f"no component label on the release {key}")
        local = (ROOT / "components" / component / chart).is_dir()
        out.add(f"{key.split('/', 1)[1]} {component} {'local' if local else 'remote'}")
    return sorted(out)


def full_sync_due(full_at: str | None, now: datetime) -> str | None:
    """Why the periodic full sync is due, or None while the last one (`fullSyncAt`) is recent."""
    hours = float(os.environ.get("JC_FULL_SYNC_HOURS") or FULL_SYNC_HOURS)
    try:
        at = datetime.strptime(full_at, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=UTC)
    except (TypeError, ValueError):
        return f"no full sync recorded ({full_at or 'none'})"
    age = (now - at).total_seconds() / 3600
    return f"the last full sync ran {age:.1f} h ago (every {hours:g} h)" if age >= hours else None


def plan(environment: str, base: str, full_at: str | None, now: datetime) -> list[str]:
    """The releases `just dev-apply` syncs for base..HEAD at `now`; Refused names why it must be a full sync."""
    if not re.fullmatch(r"[0-9a-f]{40}", base or ""):
        raise Refused(f"no trusted applied commit ({base or 'none recorded'})")
    due = full_sync_due(full_at, now)
    if due:
        raise Refused(due)
    files = changes.changed(base, "HEAD")
    if files is None:
        raise Refused(f"cannot read the range {base[:12]}..HEAD")
    structure = [f for f in files if STRUCTURE.search(f)]
    shared = [f for f in structure if not COMPONENT.match(f)]
    if shared:
        raise Refused(f"{base[:12]}..HEAD changes how helmfile builds releases ({', '.join(shared[:5])})")
    if not files:
        return []
    names = _renders(environment, base, scoped)
    components = {COMPONENT.match(f).group(1) for f in structure}
    if components:
        # A component with no release of its own (prepare: the namespace and proxy-annotation
        # hooks) acts on every release, so a scoped sync of its releases would sync nothing.
        bare = sorted(c for c in components if not component_releases(environment, {c}))
        if bare:
            raise Refused(f"{base[:12]}..HEAD changes the hooks of {', '.join(bare)}, which installs no release of its own")
        names = sorted(set(names) | set(component_releases(environment, components)))
    if any(SEED_RELEASE.match(n) for n in names):
        names = sorted(set(names) | {SEED_WRITER})
    return names


def component_releases(environment: str, components: set[str]) -> list[str]:
    """The installed releases at HEAD labelled with one of `components`."""
    return releases_of(_listed(environment), components)


@functools.cache
def _listed(environment: str) -> list[dict]:
    """`helmfile list --output json` at HEAD, read once per run."""
    done = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", environment, "list", "--skip-charts", "--output", "json"],
        cwd=ROOT, capture_output=True, text=True, check=False)
    if done.returncode:
        raise Refused(f"helmfile list of {environment} failed:\n{done.stderr[-4000:]}")
    return json.loads(done.stdout)


def releases_of(listed: list[dict], components: set[str]) -> list[str]:
    """Names of the installed releases in a `helmfile list --output json` whose component label is in `components`."""
    out = []
    for r in listed:
        labels = dict(kv.split(":", 1) for kv in (r.get("labels") or "").split(",") if ":" in kv)
        if r.get("installed") and labels.get("component") in components:
            out.append(r["name"])
    return sorted(out)


def _renders(environment: str, base: str, judge) -> list[str]:
    """Renders base once and HEAD twice, and hands the three directories to `judge`."""
    with _base_tree(base) as tree:
        tmp = tree.parent
        # the renders at once: each waits on chart pulls and helm, not on one core
        with ThreadPoolExecutor(3) as pool:
            for job in [pool.submit(render, tree, environment, tmp / "old"),
                        pool.submit(render, ROOT, environment, tmp / "new"),
                        pool.submit(render, ROOT, environment, tmp / "again")]:
                job.result()
        return judge(tmp / "old", tmp / "new", tmp / "again")


@contextlib.contextmanager
def _base_tree(base: str):
    """<base> checked out and assembled in a git worktree under a scratch directory."""
    with tempfile.TemporaryDirectory(prefix="apply-pins-") as tmp:
        tree = Path(tmp) / "base"
        git("worktree", "add", "--detach", "-q", str(tree), base)
        try:
            subprocess.run(["just", "_dev-assemble"], cwd=tree, check=True, capture_output=True,
                           env={**os.environ, "JUST_NO_DOTENV": "true"})
            yield tree
        finally:
            git("worktree", "remove", "--force", str(tree))


def main(argv: list[str]) -> int:
    try:
        if len(argv) == 3 and argv[0] == "releases":
            print("\n".join(releases(argv[1], argv[2])))
        elif len(argv) in (3, 4) and argv[0] == "plan":
            try:
                print("\n".join(plan(argv[1], argv[2], argv[3] if len(argv) == 4 else None, datetime.now(UTC))))
            except Refused as e:
                print(f"apply-pins: full sync: {e}", file=sys.stderr)
                print(FULL)
        else:
            print(__doc__.split("\n\n")[1], file=sys.stderr)
            return 2
    except Refused as e:
        print(f"apply-pins: {e}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
