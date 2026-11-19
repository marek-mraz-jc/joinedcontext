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
    assistant = ["zilina=zilina@zilina/web"]  # ORG=PUBLICID[@PROJECT[/SOURCE],...], optional; @ what it searches
    app_hosts = []                   # ORG=HOST, optional

Each run:
1. Snapshot, read-only: jc.toml from the ConfigMap jc's pod mounts (the jc image has no `cat`),
   then one query each on the Portal's and the assistant's databases on the CNPG primary, inside SET default_transaction_read_only (the
   Organizations, their served hosts and catalogue, each project's Organization; each knowledge
   source's root, last crawl, embedder refusals and up to 12 titles of its public, embedded pages and
   documents, sampled anew each run for the assistant's question). Counts, addresses and public page
   titles only: no passage text, no person.
2. `run orgs` on that, into <state>/runs/<UTC stamp>/ (kept 7 days).
3. One line appended to <state>/orgs.log (lines older than 7 days dropped):
   `<UTC> <cluster>: N checks, F fail; <org> f/n, ...; red <ids>`.
4. Incidents, one per (cluster, Organization), in <state>/incidents.json (T-7378): an Organization
   with a red check and no incident opens a task on the board (owner, @ note) listing its red checks;
   the list is rewritten when the red checks change; the task closes when a complete run (snapshot
   read, suite finished) shows none of them red. Rows of no Organization, a broken run (`snapshot`,
   `suite`) and a deployment-wide outage (every Organization's portal check red) are one incident
   of the cluster. A run that is not complete closes only an incident whose checks it saw pass.

Rollout window: a deployer announcing a rollout writes an ISO time into <state>/maintenance-until
(`date -u -d '+30 min' +%FT%TZ > <state>/maintenance-until`). Runs before that time, capped at 60 min
after the file was written, still run and log their line with "maintenance" but file, update and
close nothing. A missing, unreadable or past file means normal filing.
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
) ORDER BY o.name), '[]'),
  'projects', (SELECT coalesce(json_agg(json_build_object('name', p.name, 'organization', p.organization)
               ORDER BY p.name), '[]')
               FROM project_ids p WHERE p.parent IS NULL AND NOT p.reserved AND p.organization IS NOT NULL)
) FROM organizations o"""

ASSISTANT_SQL = """SELECT json_build_object('knowledge', coalesce(json_agg(json_build_object(
  'domain', s.organization, 'project', s.project, 'source', s.source,
  'lastCrawl', to_char(s.last_crawl AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
  'root', (SELECT p.url FROM pages p WHERE p.site_id = s.id AND p.depth = 0 ORDER BY p.id LIMIT 1),
  'pending', (SELECT count(*) FROM chunks c WHERE c.site_id = s.id AND c.embedding IS NULL{pending}),
  'errors', {errors},
  'titles', (SELECT coalesce(json_agg(json_build_object('title', t.title, 'lang', t.language)), '[]')
             FROM (SELECT u.title, u.language FROM (SELECT DISTINCT coalesce(p.title, d.title) AS title,
                   coalesce(p.language, c.lang) AS language
                   FROM chunks c LEFT JOIN pages p ON p.id = c.page_id AND p.included
                   LEFT JOIN documents d ON d.id = c.document_id AND d.included
                   WHERE c.project = s.project AND c.site_id = s.id AND c.visibility = 'public'
                     AND c.embedding IS NOT NULL AND s.visibility = 'public'
                     AND length(btrim(coalesce(p.title, d.title))) BETWEEN 3 AND 200) u
                   ORDER BY random() LIMIT 12) t)
) ORDER BY s.organization, s.project, s.source), '[]')) FROM sites s"""
# chunks.embed_error arrives with assistant migration 0025: before it, the errors are unknown (null).
EMBED_ERROR_SQL = """SELECT json_build_object('embedError', EXISTS (SELECT 1 FROM information_schema.columns
  WHERE table_schema = current_schema() AND table_name = 'chunks' AND column_name = 'embed_error'))"""


def assistant_sql(embed_error: bool) -> str:
    if embed_error:
        return ASSISTANT_SQL.format(
            pending=" AND c.embed_error IS NULL",
            errors="(SELECT count(*) FROM chunks c WHERE c.site_id = s.id AND c.embed_error IS NOT NULL)")
    return ASSISTANT_SQL.format(pending="", errors="NULL")

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
    embed_error = query(cfg, pod, "jc_assistant", EMBED_ERROR_SQL).get("embedError") is True
    snap = {**query(cfg, pod, "jc_portal", PORTAL_SQL),
            **query(cfg, pod, "jc_assistant", assistant_sql(embed_error))}
    path = run / "snapshot.json"
    path.write_text(json.dumps(snap, indent=1))
    return toml, path


def previous_results(runs: Path, run: Path) -> Path | None:
    """The newest earlier run's results.json, whose questions this run does not ask again."""
    done = sorted(d / "orgs" / "results.json" for d in runs.iterdir() if d.name < run.name) if runs.is_dir() else []
    return next((f for f in reversed(done) if f.is_file()), None)


def suite_argv(cfg: dict, toml: Path, snap: Path | None, run: Path) -> list[str]:
    argv = [sys.executable, str(SMOKE / "run"), "orgs", "--out", str(run / "orgs"), "--jc-config", str(toml),
            "--deployment", cfg["cluster"], "--domain", cfg["domain"], "--edge", cfg["edge"]]
    if snap:
        argv += ["--snapshot", str(snap)]
    if prev := previous_results(run.parent, run):
        argv += ["--previous", str(prev)]
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

    def open(self, key: str, rows: list[dict], at: str) -> str:
        if not self.path or self.dry:
            return f"dry-{key}"
        tid = self.tool("newid").splitlines()[0]
        org = key.removeprefix(ORG)
        what = f"Organization {org} smoke red" if key != CLUSTER else "deployment-wide smoke red"
        slug = re.sub(r"[^a-z0-9]+", "-", org.lower()).strip("-")[:60]
        (self.path / f"{tid}-incident-{self.cluster}-orgs-{slug}.md").write_text(
            f"""---
id: {tid}
title: "Incident on {self.cluster}: {what}"
repo: joinedcontext-deployment
status: todo
admitted: owner 2026-11-19 T-7309 (per-Organization periodic smoke: a red Organization raises one incident)
owner: {self.owner}
role: deployer
phase: mvp
priority: 1
group: red-cluster
incident: {self.cluster}/orgs/{org}
---

Opened {at} by bins/jc/smoke/orgs/watch.py (T-7309, T-7378) on {self.cluster}: {what}.

{checks_block(rows, at)}

The watch rewrites the list above on every run whose red checks differ and closes this task on the
first run where all of them pass. The run's rows: <state>/runs/ on s1 (watch.toml `state`).
""")
        self.tool("note", f"@{self.owner} {tid}: {self.cluster} {what}: {len(rows)} check(s) "
                  f"({', '.join(r['id'] for r in rows[:5])}{', ...' if len(rows) > 5 else ''})")
        return tid

    def task_file(self, tid: str) -> Path | None:
        files = [*self.path.glob(f"{tid}-*.md"), *self.path.glob(f"finished/{tid}-*.md")] if self.path else []
        return files[0] if files else None

    def update(self, tid: str, rows: list[dict], at: str) -> None:
        if not self.path or self.dry or tid.startswith("dry-") or not (f := self.task_file(tid)):
            return
        text = f.read_text()
        block = checks_block(rows, at)
        new, n = BLOCK.subn(lambda _: block, text, count=1)
        f.write_text(new if n else f"{text.rstrip()}\n\n{block}\n")

    def close(self, tid: str, key: str, why: str, at: str) -> None:
        if not self.path or self.dry or tid.startswith("dry-"):
            return
        if f := self.task_file(tid):
            with f.open("a") as fh:
                fh.write(f"\nClosed {at} by orgs/watch.py: {key} {why}.\n")
        self.tool("claim", tid, "done", env={"AGENT_NAME": self.owner})


ORG, CLUSTER = "org/", "cluster"
BLOCK = re.compile(r"<!-- orgs-watch:checks -->.*?<!-- /orgs-watch:checks -->", re.DOTALL)


def checks_block(rows: list[dict], at: str) -> str:
    lines = [f"- `{r['id']}`: {r.get('actual', '')} (expected {r.get('expected', '')}; {r.get('call', '')})"
             for r in rows]
    return "\n".join(["<!-- orgs-watch:checks -->", f"Red checks as of {at} ({len(rows)}):", *lines,
                      "<!-- /orgs-watch:checks -->"])


def groups(results: dict | None, broken: str | None) -> dict[str, list[dict]]:
    """The red rows per incident: one per Organization (`org/<name>`), and `cluster` for rows of no
    Organization, a broken run, or a deployment-wide outage (every Organization's portal check red)."""
    orgs = set((results or {}).get("organizations") or {})
    red = [r for r in (results or {}).get("checks") or [] if r.get("result") == "fail"]
    red_ids = {r["id"] for r in red}
    wide = bool(orgs) and all(f"portal.{o}" in red_ids for o in orgs)
    out: dict[str, list[dict]] = {}
    for r in red:
        key = ORG + r["target"] if r.get("target") in orgs and not wide else CLUSTER
        out.setdefault(key, []).append(r)
    if broken:
        key = "snapshot" if "snapshot" in broken else "suite"
        out.setdefault(CLUSTER, []).append({"id": key, "target": "(cluster)", "actual": broken,
                                            "expected": "a complete run"})
    return out


def incidents(state: dict, results: dict | None, complete: bool, broken: str | None,
              board: Board, at: str) -> dict:
    """The open incidents after this run: {key: {"task": id, "checks": [red ids]}}. An entry whose value
    is a bare task id is one of T-7309's per-check incidents: it closes when its check passes, is no
    longer probed, or is listed in this run's incident of its Organization."""
    rows = {r["id"]: r for r in (results or {}).get("checks") or []}
    red = groups(results, broken)
    out = {k: v for k, v in state.items() if isinstance(v, dict)}

    def close(key: str, tid: str, why: str) -> bool:
        try:
            board.close(tid, key, why, at)
            return True
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            print(f"incident {key} ({tid}) not closed, next run tries again: {e}", file=sys.stderr)
            return False

    for key, reds in red.items():
        ids = [r["id"] for r in reds]
        try:
            if key not in out:
                out[key] = {"task": board.open(key, reds, at), "checks": ids}
            elif out[key].get("checks") != ids:
                board.update(out[key]["task"], reds, at)
                out[key]["checks"] = ids
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            print(f"incident {key} not filed or updated, next run tries again: {e}", file=sys.stderr)
    for key, inc in list(out.items()):
        if key in red:
            continue
        # An incomplete run closes only an incident whose every check it ran and saw pass.
        seen_green = all(rows.get(c, {}).get("result") == "pass" for c in inc.get("checks") or [None])
        if (complete or seen_green) and close(key, inc["task"], "all checks pass"):
            del out[key]
    tracked = {r["id"]: out[k]["task"] for k, reds in red.items() if k in out for r in reds}
    for key, tid in state.items():
        if not isinstance(tid, str):
            continue
        row = rows.get(key)
        if row and row.get("result") == "pass":
            why = "passes"
        elif complete and row is None:
            why = "is no longer probed"
        elif key in tracked:
            why = f"is tracked in {tracked[key]}"
        else:
            out[key] = tid
            continue
        if not close(key, tid, why):
            out[key] = tid
    return out


MAX_WINDOW = dt.timedelta(minutes=60)


def maintenance_until(state: Path, now: dt.datetime) -> dt.datetime | None:
    """The end of the rollout window a deployer announced in <state>/maintenance-until (an ISO time),
    capped at 60 min after the file was written; None when the file is missing, unreadable or past."""
    f = state / "maintenance-until"
    try:
        text, written = f.read_text().strip(), dt.datetime.fromtimestamp(f.stat().st_mtime, dt.UTC)
    except OSError:
        return None
    try:
        until = dt.datetime.fromisoformat(text)
    except ValueError:
        print(f"{f}: {text[:40]!r} is not an ISO time; filing as usual", file=sys.stderr)
        return None
    until = until.replace(tzinfo=until.tzinfo or dt.UTC)
    until = min(until, written + MAX_WINDOW)
    return until if now < until else None


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
    if until := maintenance_until(state, now):
        line += f"; maintenance until {until.strftime('%Y-%m-%dT%H:%M:%SZ')}, nothing filed"
    keep_week(state / "orgs.log", line, now)
    prune_runs(state / "runs", now)
    board = Board(Path(cfg["board"]) if cfg.get("board") else None, cfg["cluster"], cfg["owner"], a.dry_run)
    path = state / "incidents.json"
    try:
        open_now = json.loads(path.read_text())
    except (OSError, ValueError):
        open_now = {}
    at = now.strftime("%Y-%m-%dT%H:%M:%SZ")
    print(line)
    if until:
        return 0
    path.write_text(json.dumps(incidents(open_now, results, broken is None, broken, board, at), indent=1))
    return 1 if broken or (results or {}).get("totals", {}).get("fail") else 0


if __name__ == "__main__":
    sys.exit(main())
