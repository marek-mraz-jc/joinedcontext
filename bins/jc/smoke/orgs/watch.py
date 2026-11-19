#!/usr/bin/env python3
"""The periodic per-Organization smoke of one cluster (T-7309): every 30 min from s1's cron,
beside cluster-watch, which already holds each cluster's kubeconfig and the board.

    bins/jc/smoke/orgs/watch.py --config <watch.toml> [--dry-run]

watch.toml (one per cluster, beside the kubeconfig it names; no secret in it):

    cluster = "jc-prod"              # the deployment's name, in every line and incident
    kubeconfig = "/workspace/.secrets/kubeconfig-jc-prod.yaml"
    namespace = "jc"                 # jc's namespace
    jc_configmap = "jc"              # default; the ConfigMap jc mounts at /etc/jc (key jc.toml)
    pg_namespace = "postgres"        # the shared CNPG cluster's namespace
    pg_cluster = "postgres-cluster"  # default
    edge = "203.0.113.10"            # the edge's address
    domain = "smartcity.joinedcontext.com"
    state = "/workspace/evidence/orgs-smoke/jc-prod"
    board = "/workspace/tasks"       # incidents go here; unset, none are filed
    owner = "prod-deployer"          # the incidents' owner, named @ in each note
    assistant = ["zilina=zilina"]    # ORG=PUBLICID, optional
    app_hosts = []                   # ORG=HOST, optional

Each run:
1. Snapshot, read-only: jc.toml from the ConfigMap jc's pod mounts (the jc image has no `cat`),
   then one query each on the Portal's and the assistant's databases on the CNPG primary, inside SET default_transaction_read_only (the
   Organizations, their served hosts and catalogue; each knowledge source's root, last crawl and
   embedder refusals). Counts and addresses only: no passage text, no person.
2. `run orgs` on that, into <state>/runs/<UTC stamp>/ (kept 7 days).
3. One line appended to <state>/orgs.log (lines older than 7 days dropped):
   `<UTC> <cluster>: N checks, F fail; <org> f/n, ...; red <ids>`.
4. Incidents, one per (Organization, check), in <state>/incidents.json: a failing check without
   one opens a task on the board (owner, @ note); an open one closes when its check passes, or
   when a complete run (snapshot read, suite finished) no longer probes it. A run that is not
   complete opens the `snapshot` or `suite` incident of the cluster and closes nothing else.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
KEEP = dt.timedelta(days=7)
STAMP = "%Y%m%dT%H%M%SZ"

PORTAL_SQL = """SELECT json_build_object('organizations', coalesce(json_agg(json_build_object(
  'name', o.name, 'host', o.host, 'state', o.state, 'catalogueId', o.catalogue_id,
  'hosts', (SELECT coalesce(json_agg(json_build_object('host', h.host, 'role', h.role)), '[]')
            FROM organization_hosts h WHERE h.organization = o.name AND h.state IN ('cert-issued', 'served'))
) ORDER BY o.name), '[]')) FROM organizations o"""

ASSISTANT_SQL = """SELECT json_build_object('knowledge', coalesce(json_agg(json_build_object(
  'organization', s.organization, 'project', s.project, 'source', s.source,
  'lastCrawl', to_char(s.last_crawl AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
  'root', (SELECT p.url FROM pages p WHERE p.site_id = s.id AND p.depth = 0 ORDER BY p.id LIMIT 1),
  'pending', (SELECT count(*) FROM chunks c WHERE c.site_id = s.id AND c.embedding IS NULL AND c.embed_error IS NULL),
  'errors', (SELECT count(*) FROM chunks c WHERE c.site_id = s.id AND c.embed_error IS NOT NULL)
) ORDER BY s.organization, s.project, s.source), '[]')) FROM sites s"""

REQUIRED = ("cluster", "kubeconfig", "namespace", "pg_namespace", "edge", "domain", "state", "owner")
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,62}")


def load(path: Path) -> dict:
    cfg = tomllib.loads(path.read_text())
    if missing := [k for k in REQUIRED if not cfg.get(k)]:
        raise SystemExit(f"{path}: missing {', '.join(missing)}")
    cfg.setdefault("jc_configmap", "jc")
    for k in ("cluster", "owner", "namespace", "pg_namespace", "jc_configmap"):
        if not NAME.fullmatch(str(cfg[k])):
            raise SystemExit(f"{path}: {k} {cfg[k]!r} is not a lower-case name")
    cfg.setdefault("pg_cluster", "postgres-cluster")
    return cfg


def kubectl(cfg: dict, *args: str, timeout: int = 60) -> str:
    """kubectl's stdout; its stderr in the error, cut, never a body of the databases."""
    env = {**os.environ, "KUBECONFIG": cfg["kubeconfig"]}
    p = subprocess.run(["kubectl", *args], check=False, capture_output=True, text=True, timeout=timeout, env=env)
    if p.returncode:
        raise RuntimeError(f"kubectl {args[0]} {args[1] if len(args) > 1 else ''}: {p.stderr.strip()[:200]}")
    return p.stdout


def query(cfg: dict, pod: str, db: str, sql: str) -> dict:
    out = kubectl(cfg, "-n", cfg["pg_namespace"], "exec", pod, "-c", "postgres", "--",
                  "psql", "-d", db, "-At", "-X", "-v", "ON_ERROR_STOP=1",
                  "-c", "SET default_transaction_read_only = on", "-c", sql)
    lines = [ln for ln in out.splitlines() if ln.strip() and ln.strip() != "SET"]
    return json.loads(lines[-1])


def snapshot(cfg: dict, run: Path) -> tuple[Path, Path]:
    """(jc.toml, snapshot.json) of the cluster, written into the run directory."""
    toml = run / "jc.toml"
    text = kubectl(cfg, "-n", cfg["namespace"], "get", "configmap", cfg["jc_configmap"], "-o", r"jsonpath={.data.jc\.toml}")
    if not text.strip():
        raise RuntimeError(f"configmap {cfg['jc_configmap']} in {cfg['namespace']} has no jc.toml")
    toml.write_text(text)
    pod = kubectl(cfg, "-n", cfg["pg_namespace"], "get", "pod", "-o", "name", "-l",
                  f"cnpg.io/cluster={cfg['pg_cluster']},cnpg.io/instanceRole=primary").split()[0]
    snap = {**query(cfg, pod, "jc_portal", PORTAL_SQL), **query(cfg, pod, "jc_assistant", ASSISTANT_SQL)}
    path = run / "snapshot.json"
    path.write_text(json.dumps(snap, indent=1))
    return toml, path


def suite_argv(cfg: dict, toml: Path, snap: Path | None, run: Path) -> list[str]:
    argv = [sys.executable, str(SMOKE / "run"), "orgs", "--out", str(run / "orgs"), "--jc-config", str(toml),
            "--deployment", cfg["cluster"], "--domain", cfg["domain"], "--edge", cfg["edge"]]
    if snap:
        argv += ["--snapshot", str(snap)]
    for v in cfg.get("assistant") or []:
        argv += ["--assistant", v]
    for v in cfg.get("app_hosts") or []:
        argv += ["--app-host", v]
    return argv


def summary(cluster: str, at: dt.datetime, results: dict | None, broken: str | None) -> str:
    head = f"{at.strftime('%Y-%m-%dT%H:%M:%SZ')} {cluster}:"
    if results is None:
        return f"{head} no result ({broken})"
    t = results.get("totals") or {}
    n = sum(int(v) for v in t.values())
    per = ", ".join(f"{o} {v['fail']}/{v['checks']}" for o, v in sorted((results.get("organizations") or {}).items()))
    red = [r["id"] for r in results.get("checks") or [] if r.get("result") == "fail"]
    line = f"{head} {n} checks, {t.get('fail', 0)} fail; {per or 'no Organization'}"
    line += f"; red {' '.join(red)}" if red else ""
    return line + (f"; incomplete: {broken}" if broken else "")


def keep_week(log: Path, line: str, now: dt.datetime) -> None:
    kept = []
    if log.exists():
        for ln in log.read_text().splitlines():
            try:
                at = dt.datetime.strptime(ln.split(" ", 1)[0], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.UTC)
            except ValueError:
                continue
            if now - at <= KEEP:
                kept.append(ln)
    log.write_text("\n".join([*kept, line]) + "\n")


def prune_runs(runs: Path, now: dt.datetime) -> None:
    for d in runs.iterdir() if runs.is_dir() else []:
        try:
            at = dt.datetime.strptime(d.name, STAMP).replace(tzinfo=dt.UTC)
        except ValueError:
            continue
        if now - at > KEEP:
            shutil.rmtree(runs / d.name)


class Board:
    """The incidents' tasks: tasks/newid, a task file, tasks/note; closed with tasks/claim done."""

    def __init__(self, path: Path | None, cluster: str, owner: str, dry: bool = False):
        self.path, self.cluster, self.owner, self.dry = path, cluster, owner, dry

    def tool(self, name: str, *args: str, env: dict | None = None) -> str:
        if not self.path or self.dry:
            return ""
        p = subprocess.run([str(self.path / name), *args], check=False, capture_output=True, text=True, timeout=120,
                           env={**os.environ, **(env or {})})
        if p.returncode:
            raise RuntimeError(f"tasks/{name}: {p.stderr.strip()[:200]}")
        return p.stdout.strip()

    def open(self, key: str, row: dict, at: str) -> str:
        if not self.path or self.dry:
            return f"dry-{key}"
        tid = self.tool("newid").splitlines()[0]
        slug = re.sub(r"[^a-z0-9]+", "-", key.lower()).strip("-")[:60]
        org = row.get("target", "")
        (self.path / f"{tid}-incident-{self.cluster}-orgs-{slug}.md").write_text(
            f"""---
id: {tid}
title: "Incident on {self.cluster}: Organization {org} smoke red ({key})"
repo: joinedcontext-deployment
status: todo
admitted: owner 2026-11-19 T-7309 (per-Organization periodic smoke: a red Organization raises one incident)
owner: {self.owner}
role: deployer
phase: mvp
priority: 1
group: red-cluster
incident: {self.cluster}/orgs/{key}
---

Opened {at} by bins/jc/smoke/orgs/watch.py (T-7309) on {self.cluster}, Organization {org}.

- check: {key}
- call: {row.get('call', '')}
- expected: {row.get('expected', '')}
- actual: {row.get('actual', '')}
- reason: {row.get('reason', '')}

The watch closes this task on the first run where the check passes or is no longer probed. The
run's rows: <state>/runs/ on s1 (watch.toml `state`).
""")
        self.tool("note", f"@{self.owner} {tid}: {self.cluster} Organization {org} smoke red: {key} "
                  f"({row.get('actual', '')})")
        return tid

    def close(self, tid: str, key: str, why: str, at: str) -> None:
        if not self.path or self.dry or tid.startswith("dry-"):
            return
        files = [*self.path.glob(f"{tid}-*.md"), *self.path.glob(f"finished/{tid}-*.md")]
        if files:
            with files[0].open("a") as f:
                f.write(f"\nClosed {at} by orgs/watch.py: {key} {why}.\n")
        self.tool("claim", tid, "done", env={"AGENT_NAME": self.owner})


def incidents(state: dict, results: dict | None, complete: bool, broken: str | None,
              board: Board, at: str) -> dict:
    """The open incidents after this run: {key: task id}."""
    rows = {r["id"]: r for r in (results or {}).get("checks") or []}
    red = {k: r for k, r in rows.items() if r.get("result") == "fail"}
    if broken:
        key = "snapshot" if "snapshot" in broken else "suite"
        red[key] = {"id": key, "target": "(cluster)", "actual": broken, "expected": "a complete run"}
    out = dict(state)
    for key, r in red.items():
        if key not in out:
            try:
                out[key] = board.open(key, r, at)
            except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
                print(f"incident {key} not filed, next run tries again: {e}", file=sys.stderr)
    for key, tid in list(out.items()):
        if key in red:
            continue
        row = rows.get(key)
        if row and row.get("result") == "pass":
            why = "passes"
        elif complete and row is None:
            why = "is no longer probed"
        else:
            continue
        try:
            board.close(tid, key, why, at)
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            print(f"incident {key} ({tid}) not closed, next run tries again: {e}", file=sys.stderr)
            continue
        del out[key]
    return out


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="orgs/watch.py")
    p.add_argument("--config", required=True)
    p.add_argument("--dry-run", action="store_true", help="file no incident; print the line")
    a = p.parse_args(argv)
    cfg = load(Path(a.config))
    state = Path(cfg["state"])
    now = dt.datetime.now(dt.UTC).replace(microsecond=0)
    run = state / "runs" / now.strftime(STAMP)
    run.mkdir(parents=True, exist_ok=True)
    broken, snap, toml = None, None, run / "jc.toml"
    try:
        toml, snap = snapshot(cfg, run)
    except (RuntimeError, OSError, ValueError, IndexError, subprocess.TimeoutExpired) as e:
        broken = f"snapshot: {e}"[:240]
    results = None
    if toml.exists():
        try:
            subprocess.run(suite_argv(cfg, toml, snap, run), check=False, capture_output=True, timeout=1500)
        except subprocess.TimeoutExpired:
            broken = broken or "suite: over 1500 s"
        try:
            results = json.loads((run / "orgs" / "results.json").read_text())
        except (OSError, ValueError):
            broken = broken or "suite: no results.json"
    line = summary(cfg["cluster"], now, results, broken)
    keep_week(state / "orgs.log", line, now)
    prune_runs(state / "runs", now)
    board = Board(Path(cfg["board"]) if cfg.get("board") else None, cfg["cluster"], cfg["owner"], a.dry_run)
    path = state / "incidents.json"
    try:
        open_now = json.loads(path.read_text())
    except (OSError, ValueError):
        open_now = {}
    at = now.strftime("%Y-%m-%dT%H:%M:%SZ")
    path.write_text(json.dumps(incidents(open_now, results, broken is None, broken, board, at), indent=1))
    print(line)
    return 1 if broken or (results or {}).get("totals", {}).get("fail") else 0


if __name__ == "__main__":
    sys.exit(main())
