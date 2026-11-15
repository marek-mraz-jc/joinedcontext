#!/usr/bin/env python3
"""Update an installation to a release in one command (T-3794).

    scripts/upgrade.py <environment> <version> [--check] [--i-have-a-backup] [--min-free-gib N]
                       [--domain D] [--timeout-minutes N] [--smoke scripts/<name>.sh]

In order, stopping at the first refusal:

1. the release: the tag `<version>` exists and carries `releases/<version>.md`, whose front
   matter names its images and its version-specific steps;
2. the cluster answers, the version it runs is read from the ConfigMap `jc-release`, and every
   node has `--min-free-gib` of free disk;
3. a backup of every CloudNativePG cluster, taken now and waited for until it completes. A
   cluster with no backup configured refuses the run, unless `--i-have-a-backup` says the
   operator holds one, which is logged with who and when;
4. the release is checked out into a temporary worktree with this checkout's `deployment/`
   copied in, so the operator's checkout never moves, and `helmfile diff` is shown;
5. `helmfile sync`, the rollouts awaited, the release's steps run in order, then the smoke test
   against the installation's own domain (`--smoke`, a script of the release's own tree,
   `scripts/smoke.sh` unless named);
6. green: the version is recorded in `jc-release`. Red: every helm release that changed is
   rolled back to its previous revision, and the database step is printed; nothing is deleted.

`--check` runs 1, 2, 4 and the plan, and touches nothing. `dev` is applied with
`just dev-apply` by its deployer, so it only takes `--check` here.

Exit 0 when the installation runs the release, 1 when it was refused or rolled back.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.compile(r"^v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")
RECORD = "jc-release"
SMOKE = re.compile(r"^scripts/[a-z0-9][a-z0-9-]*\.sh$")
GIB = 1024**3


class Refused(Exception):
    """A precondition does not hold; nothing was changed."""


def run(cmd: list[str], *, cwd: Path | None = None, check: bool = True, stdin: str | None = None) -> str:
    """Runs one command, its output captured; a non-zero exit is `Refused` when `check`."""
    result = subprocess.run(cmd, cwd=cwd, input=stdin, capture_output=True, text=True)
    if check and result.returncode != 0:
        raise Refused(f"`{' '.join(cmd[:3])}` failed: {result.stderr.strip()[-400:]}")
    return result.stdout


def stream(cmd: list[str], *, cwd: Path | None = None) -> bool:
    """Runs one command with its output on the terminal; True when it exited 0."""
    print(f"$ {' '.join(cmd)}", flush=True)
    return subprocess.run(cmd, cwd=cwd).returncode == 0


@dataclass
class Release:
    version: str
    images: list[str] = field(default_factory=list)
    steps: list[dict] = field(default_factory=list)


def parse_notes(version: str, text: str) -> Release:
    """The front matter of `releases/<version>.md`: `version`, `images`, `steps`.

    A step is `{name, run}`, `run` a script path inside the release, run with bash. The notes
    are generated (scripts/release-notes.py), so a missing or foreign member is refused rather
    than guessed.
    """
    match = re.match(r"^---\n(.*?)\n---\n", text, re.S)
    if not match:
        raise Refused(f"releases/{version}.md has no front matter")
    import yaml  # the deployment's tests already need it; the rest of this script does not

    meta = yaml.safe_load(match.group(1)) or {}
    if meta.get("version") != version:
        raise Refused(f"releases/{version}.md is the notes of {meta.get('version')!r}")
    steps = meta.get("steps") or []
    for step in steps:
        if not isinstance(step, dict) or set(step) != {"name", "run"}:
            raise Refused(f"releases/{version}.md: a step is {{name, run}}, not {step!r}")
        if step["run"].startswith("/") or ".." in Path(step["run"]).parts:
            raise Refused(f"releases/{version}.md: step {step['name']!r} runs outside the release")
    return Release(version, list(meta.get("images") or []), steps)


def free_bytes(summary: dict) -> int:
    """A node's free filesystem bytes from the kubelet's stats summary."""
    return int(summary.get("node", {}).get("fs", {}).get("availableBytes", 0))


def backup_method(cluster: dict) -> str | None:
    """How a CNPG cluster can take a backup now, or None when none is configured."""
    backup = cluster.get("spec", {}).get("backup") or {}
    if backup.get("barmanObjectStore"):
        return "barmanObjectStore"
    if backup.get("volumeSnapshot"):
        return "volumeSnapshot"
    return None


def backup_name(cluster: str, version: str, stamp: str) -> str:
    """A Backup's name: a DNS label (lower-case alphanumerics and `-`, 63 at most, alphanumeric at
    both ends), so a long cluster name or a `+build` version cannot make the backup step fail."""
    name = re.sub(r"[^a-z0-9-]", "-", f"{cluster}-pre-{version}-{stamp}".lower())
    return name[:63].strip("-")


def changed_releases(before: list[dict], after: list[dict]) -> list[tuple[str, str, int]]:
    """(name, namespace, previous revision) of every release `helm list` shows advanced."""
    old = {(r["name"], r["namespace"]): int(r["revision"]) for r in before}
    return sorted(
        (r["name"], r["namespace"], old[(r["name"], r["namespace"])])
        for r in after
        if (r["name"], r["namespace"]) in old and int(r["revision"]) > old[(r["name"], r["namespace"])]
    )


def new_releases(before: list[dict], after: list[dict]) -> list[str]:
    old = {(r["name"], r["namespace"]) for r in before}
    return sorted(f"{r['namespace']}/{r['name']}" for r in after if (r["name"], r["namespace"]) not in old)


class Upgrade:
    def __init__(self, args: argparse.Namespace):
        self.env = args.environment
        self.version = args.version
        self.check = args.check
        self.have_backup = args.i_have_a_backup
        self.min_free = int(args.min_free_gib * GIB)
        self.domain = args.domain
        self.timeout = args.timeout_minutes * 60
        self.concurrency = args.concurrency
        if not SMOKE.match(args.smoke):
            raise Refused(f"--smoke {args.smoke!r} is not a script scripts/<name>.sh of the release")
        self.smoke = args.smoke
        self.backups: list[str] = []

    # 1. the release

    def release(self) -> Release:
        if not VERSION.match(self.version):
            raise Refused(f"{self.version!r} is not a release tag v<major>.<minor>.<patch>")
        run(["git", "fetch", "--quiet", "--tags"], cwd=ROOT, check=False)
        run(["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{self.version}"], cwd=ROOT)
        notes = run(["git", "show", f"{self.version}:releases/{self.version}.md"], cwd=ROOT)
        return parse_notes(self.version, notes)

    # 2. the cluster

    def cluster(self) -> str:
        run(["kubectl", "get", "--raw", "/readyz"])
        records = json.loads(run(["kubectl", "get", "configmap", "-A", "--field-selector",
                                  f"metadata.name={RECORD}", "-o", "json"]))["items"]
        current = records[0]["data"].get("version", "unknown") if records else "unknown"
        for node in json.loads(run(["kubectl", "get", "nodes", "-o", "json"]))["items"]:
            name = node["metadata"]["name"]
            summary = json.loads(run(["kubectl", "get", "--raw", f"/api/v1/nodes/{name}/proxy/stats/summary"]))
            free = free_bytes(summary)
            if free < self.min_free:
                raise Refused(f"node {name} has {free / GIB:.1f} GiB free, the upgrade needs {self.min_free / GIB:.1f}")
        return current

    def domain_of_installation(self) -> str:
        if self.domain:
            return self.domain
        names = json.loads(run(["kubectl", "get", "certificates.cert-manager.io", "-A", "--field-selector",
                                "metadata.name=apisix-edge", "-o", "json"]))["items"]
        if not names:
            raise Refused("no apisix-edge Certificate names the installation's domain; pass --domain")
        return names[0]["spec"]["dnsNames"][0]

    # 3. the backup

    def backup(self) -> None:
        clusters = json.loads(run(["kubectl", "get", "clusters.postgresql.cnpg.io", "-A", "-o", "json"]))["items"]
        unconfigured = [f"{c['metadata']['namespace']}/{c['metadata']['name']}" for c in clusters if not backup_method(c)]
        if unconfigured:
            if not self.have_backup:
                raise Refused(
                    f"no backup is configured for {', '.join(unconfigured)}: configure one "
                    "(Deployment/07-backup-restore) or pass --i-have-a-backup")
            print(f"WARNING {datetime.now(UTC):%Y-%m-%dT%H:%M:%SZ} {getpass.getuser()} passed "
                  f"--i-have-a-backup for {', '.join(unconfigured)}: no backup taken there", flush=True)
        stamp = datetime.now(UTC).strftime("%Y%m%d%H%M%S")
        for cluster in clusters:
            method = backup_method(cluster)
            if not method:
                continue
            ns, name = cluster["metadata"]["namespace"], cluster["metadata"]["name"]
            backup = backup_name(name, self.version, stamp)
            manifest = {"apiVersion": "postgresql.cnpg.io/v1", "kind": "Backup",
                        "metadata": {"name": backup, "namespace": ns,
                                     "labels": {"joinedcontext.com/upgrade": self.version.replace("+", "-")}},
                        "spec": {"cluster": {"name": name}, "method": method}}
            run(["kubectl", "apply", "-f", "-"], stdin=json.dumps(manifest))
            self.wait_backup(ns, backup)
            self.backups.append(f"{ns}/{backup}")
            print(f"backup {ns}/{backup} completed", flush=True)

    def wait_backup(self, ns: str, name: str) -> None:
        deadline = time.monotonic() + self.timeout
        while time.monotonic() < deadline:
            out = run(["kubectl", "get", "backups.postgresql.cnpg.io", name, "-n", ns, "-o", "json"], check=False)
            status = (json.loads(out) if out else {}).get("status", {})
            phase = status.get("phase", "")
            if phase == "completed" and status.get("stoppedAt"):
                return
            if phase == "failed":
                raise Refused(f"backup {ns}/{name} failed: {status.get('error', 'no reason given')}")
            time.sleep(10)
        raise Refused(f"backup {ns}/{name} did not complete within {self.timeout // 60} minutes")

    # 4. the release's tree

    def worktree(self) -> Path:
        if not (ROOT / "deployment/environments" / self.env).is_dir():
            raise Refused(f"deployment/environments/{self.env} does not exist in this checkout")
        tree = Path(tempfile.mkdtemp(prefix=f"jc-upgrade-{self.version}-"))
        shutil.rmtree(tree)
        run(["git", "worktree", "add", "--detach", "--quiet", str(tree), self.version], cwd=ROOT)
        shutil.copytree(ROOT / "deployment", tree / "deployment", dirs_exist_ok=True)
        return tree

    def helmfile(self, tree: Path, verb: str) -> list[str]:
        return ["helmfile", "-f", str(tree / "deployment/helmfile.yaml"), "-e", self.env, verb,
                "--concurrency", str(self.concurrency)]

    def instance(self, tree: Path) -> str:
        """The environment's `global.instanceSlug`, else the defaults' one, as helmfile merges
        them: the namespace prefix wait-rollouts and smoke read. Environment `testing` installs
        instance `dev`, `production` instance `jc` (ci-full 39342677129)."""
        for f in (tree / "deployment/environments" / self.env / "global.yaml.gotmpl",
                  tree / "defaults/environment/global.yaml"):
            found = re.search(r"^  instanceSlug:\s*['\"]?([a-z0-9-]+)", f.read_text(), re.M) if f.is_file() else None
            if found:
                return found[1]
        return self.env

    # 5. and 6.

    def apply(self, tree: Path, release: Release, domain: str) -> bool:
        before = json.loads(run(["helm", "list", "-A", "-o", "json"]))
        slug = self.instance(tree)
        os.environ.setdefault("JC_INSTANCE", slug)  # smoke.sh's cluster-side checks
        if not (tree / self.smoke).is_file():
            raise Refused(f"{self.version} has no {self.smoke}; nothing was applied")
        ok = stream(self.helmfile(tree, "sync"), cwd=tree) and stream(
            [str(tree / "scripts/wait-rollouts.sh"), slug], cwd=tree)
        for step in release.steps if ok else []:
            print(f"step {step['name']}", flush=True)
            ok = stream(["bash", step["run"]], cwd=tree)
            if not ok:
                print(f"step {step['name']} failed", flush=True)
                break
        ok = ok and stream([str(tree / self.smoke), f"https://{domain}", f"https://idm.{domain}"], cwd=tree)
        if ok:
            try:
                self.record()
            except Refused as refused:
                print(f"{self.version} is applied and green, but {RECORD} was not written: {refused}", flush=True)
            return True
        self.rollback(before)
        return False

    def record(self) -> None:
        records = json.loads(run(["kubectl", "get", "configmap", "-A", "--field-selector",
                                  f"metadata.name={RECORD}", "-o", "json"]))["items"]
        namespace = records[0]["metadata"]["namespace"] if records else self.namespace()
        manifest = {"apiVersion": "v1", "kind": "ConfigMap",
                    "metadata": {"name": RECORD, "namespace": namespace},
                    "data": {"version": self.version, "appliedAt": f"{datetime.now(UTC):%Y-%m-%dT%H:%M:%SZ}",
                             "commit": run(["git", "rev-parse", f"{self.version}^{{commit}}"], cwd=ROOT).strip(),
                             "environment": self.env, "appliedBy": getpass.getuser(),
                             "backups": ",".join(self.backups)}}
        run(["kubectl", "apply", "-f", "-"], stdin=json.dumps(manifest))

    def namespace(self) -> str:
        clusters = json.loads(run(["kubectl", "get", "clusters.postgresql.cnpg.io", "-A", "-o", "json"]))["items"]
        return clusters[0]["metadata"]["namespace"] if clusters else "default"

    def rollback(self, before: list[dict]) -> None:
        after = json.loads(run(["helm", "list", "-A", "-o", "json"], check=False) or "[]")
        print("RED: rolling back every helm release this run changed", flush=True)
        for name, namespace, revision in changed_releases(before, after):
            # helm's own 5 min default ended the wait while an old pod was still terminating
            # (ci-full 39353463338: "Pending termination: 1"); the rollback gets the run's budget.
            if not stream(["helm", "rollback", name, str(revision), "-n", namespace, "--wait",
                           "--timeout", f"{self.timeout}s"]):
                print(f"  {namespace}/{name} did not roll back to revision {revision}", flush=True)
        for release in new_releases(before, after):
            print(f"  {release} is new in this release and stays installed: remove it by hand if "
                  "the old version must not run beside it", flush=True)
        backups = ", ".join(self.backups) or "the backup you said you hold"
        print("The databases keep every migration the new version ran. If the old version refuses "
              f"its schema, restore {backups} as Deployment/07-backup-restore §3 step 2 describes. "
              "Nothing was deleted.", flush=True)

    def main(self) -> int:
        if self.env == "dev" and not self.check:
            raise Refused("dev is applied with `just dev-apply` by its deployer; here it takes --check only")
        release = self.release()
        current = self.cluster()
        print(f"{self.env}: {current} -> {self.version}", flush=True)
        print(f"images: {len(release.images)}", flush=True)
        for step in release.steps:
            print(f"step after apply: {step['name']} ({step['run']})", flush=True)
        if not self.check:
            self.backup()
        tree = self.worktree()
        try:
            stream(self.helmfile(tree, "diff") + ["--context", "3"], cwd=tree)
            if self.check:
                print("--check: nothing was changed", flush=True)
                return 0
            return 0 if self.apply(tree, release, self.domain_of_installation()) else 1
        finally:
            run(["git", "worktree", "remove", "--force", str(tree)], cwd=ROOT, check=False)


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("environment")
    p.add_argument("version")
    p.add_argument("--check", action="store_true", help="show what would change; change nothing")
    p.add_argument("--i-have-a-backup", action="store_true",
                   help="run where a CNPG cluster has no backup configured; logged")
    p.add_argument("--min-free-gib", type=float, default=5.0)
    p.add_argument("--domain", default="", help="the installation's apex; read from apisix-edge when empty")
    p.add_argument("--timeout-minutes", type=int, default=30)
    p.add_argument("--concurrency", type=int, default=4)
    p.add_argument("--smoke", default="scripts/smoke.sh",
                   help="the release's smoke script, run with the apex and identity URLs")
    return p


if __name__ == "__main__":
    try:
        sys.exit(Upgrade(parser().parse_args()).main())
    except Refused as refused:
        print(f"REFUSED: {refused}", file=sys.stderr)
        sys.exit(1)
