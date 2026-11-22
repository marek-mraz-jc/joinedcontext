#!/usr/bin/env python3
"""The periodic per-Organization smoke of one cluster (T-7309): every 30 min from s1's cron,
beside cluster-watch, which already holds each cluster's kubeconfig and the board.

    bins/jc/smoke/orgs/watch.py --config <watch.toml> [--dry-run]

watch.toml (one per cluster, beside the kubeconfig it names; no secret in it):

    cluster = "jc-prod"              # the deployment's name, in every line and incident
    kubeconfig = "/workspace/.secrets/kubeconfig-jc-prod.yaml"
    namespace = "jc"                 # jc's namespace
    jc_configmap = "jc"              # default; the ConfigMap jc mounts at /etc/jc (key jc.toml)
    jc_workload = "deploy/jc"        # default; whose jc-server log the installation rows read
    jc_container = "jc-server"       # default
    prometheus_namespace = "prometheus"  # the Prometheus that scrapes jc's admin listener; unset, pipelines.refused and database.pool never pass
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
   source's root, last crawl, newest done and newest finished crawl job (time, kind and, when it
   failed, the error text before its first colon), embedder refusals and up to 12 titles of its public, embedded pages and
   documents, sampled anew each run for the assistant's question); per Pipeline that ran in 7 days its
   newest runs' delivered and failed counts and the newest failure's masked error text. Counts,
   addresses, public page titles and pipeline error text only: no passage text, no record, no person.
   Then the jc-server log of the last 30 min (T-7458): the gateway's refused writes counted per
   project, workload account and reason, and the Portal's newest report of projects with Pipelines
   and no Organization; names and counts only. Then the gateway's refused-write counter over the same
   30 min from Prometheus (T-7467), which pipelines.refused reads: the log rotates in minutes at
   jc-prod's volume, so it can only fail that row or explain a skip. From the same Prometheus the
   seconds the Portal's database pool has handed out no connection (T-7494, database.pool), and from
   the Portal's database when a pipeline run was last recorded (pipelines.recorded).
2. `run orgs` on that, into <state>/runs/<UTC stamp>/ (kept 7 days).
3. One line appended to <state>/orgs.log (lines older than 7 days dropped):
   `<UTC> <cluster>: N checks, F fail; <org> f/n, ...; red <ids>`.
4. Incidents, one per (cluster, Organization), in <state>/incidents.json (T-7378): an Organization
   with a red check and no incident opens a task on the board (owner, @ note) listing its red checks;
   the list is rewritten when the red checks change; the task closes only when a run shows every
   check it lists pass (T-7467): a check that skipped or did not run stays listed and keeps it open.
   Rows of no Organization, a broken run (`snapshot`, `suite`, which pass on a run that is not
   broken) and a deployment-wide outage (every Organization's portal check red) are one incident
   of the cluster.

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
  'lastDone', (SELECT json_build_object('at', to_char(j.finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                                        'kind', j.kind)
               FROM crawl_jobs j WHERE j.project = s.project AND j.source = s.source AND j.state = 'done'
                 AND j.finished_at IS NOT NULL ORDER BY j.finished_at DESC LIMIT 1),
  'lastEnd', (SELECT json_build_object('state', j.state, 'kind', j.kind,
                                       'at', to_char(j.finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                                       'errorClass', CASE WHEN j.state = 'failed'
                                                     THEN left(split_part(coalesce(j.error, ''), ':', 1), 80) END)
              FROM crawl_jobs j WHERE j.project = s.project AND j.source = s.source
                AND j.state IN ('done', 'failed') AND j.finished_at IS NOT NULL ORDER BY j.finished_at DESC LIMIT 1),
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
# Per Pipeline that ran in the last 7 days (T-7445): when it last delivered (sent + unchanged > 0),
# its newest 12 runs and the newest failed line of its newest failing run (pipeline_log keeps the
# message masked and cut, pipeline_log.rs clean). Counts and the error text only, no record.
PIPELINES_SQL = """WITH p AS (SELECT DISTINCT project, pipeline FROM pipeline_runs WHERE last_at > now() - interval '7 days')
SELECT json_build_object('pipelinesAt', to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
  'lastRecorded', (SELECT to_char(max(last_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM pipeline_runs),
  'pipelines', coalesce(json_agg(json_build_object('project', p.project, 'pipeline', p.pipeline,
  'lastDelivered', (SELECT to_char(max(r.last_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM pipeline_runs r
                    WHERE r.project = p.project AND r.pipeline = p.pipeline AND r.last_at > now() - interval '7 days'
                      AND r.sent + coalesce(r.unchanged, 0) > 0),
  'runs', (SELECT coalesce(json_agg(json_build_object('at', to_char(r.last_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'),
                                                      'delivered', r.sent + coalesce(r.unchanged, 0), 'failed', r.failed)
                                    ORDER BY r.last_at DESC), '[]')
           FROM (SELECT * FROM pipeline_runs r WHERE r.project = p.project AND r.pipeline = p.pipeline
                   AND r.last_at > now() - interval '7 days' ORDER BY r.last_at DESC LIMIT 12) r),
  'error', (SELECT left(l.message, 200) FROM pipeline_log l WHERE l.project = p.project AND l.pipeline = p.pipeline
              AND l.outcome = 'failed' AND l.run = (SELECT r.run FROM pipeline_runs r WHERE r.project = p.project
                AND r.pipeline = p.pipeline AND r.failed > 0 ORDER BY r.last_at DESC LIMIT 1)
            ORDER BY l.id DESC LIMIT 1)
) ORDER BY p.project, p.pipeline), '[]')) FROM p"""
# chunks.embed_error arrives with assistant migration 0025: before it, the errors are unknown (null).
EMBED_ERROR_SQL = """SELECT json_build_object('embedError', EXISTS (SELECT 1 FROM information_schema.columns
  WHERE table_schema = current_schema() AND table_name = 'chunks' AND column_name = 'embed_error'))"""

# The jc-server log of the last 30 min (T-7458), read with the snapshot: the gateway's refused writes
# by the verified workload that sent them (context-gateway handlers/ngsi_ld.rs view_request; "-" is a
# person or nobody signed in, not counted) and the Portal's newest report of the projects whose
# Pipelines have no Organization (host_registry::report_unplaced_pipelines, every 5 min). Counts and
# names only: no body, token or record leaves this function.
LOG_WINDOW = "30m"
ANSI = re.compile(r"\x1b\[[0-9;]*m")
REFUSED = re.compile(r"write refused: (?P<why>[^=]+?) slug=\S+(?: operation=\S+)? "
                     r"account=(?P<account>[a-z0-9][a-z0-9-]{0,62}) account_project=(?P<project>[a-z0-9][a-z0-9-]{0,62})\b")
# A refusal line of a jc before T-7458 names no account: counted by slug, so the row fails on it too.
UNATTRIBUTED = re.compile(r"write refused: (?P<why>[^=]+?) slug=(?P<slug>[a-z0-9]{1,64})(?: operation=\S+)?\s*$")
UNPLACED = re.compile(r"projects with Pipelines and no Organization.*\bprojects=(?P<projects>\S+)"
                      r"|(?P<none>every project with Pipelines has an Organization)")
PROJECT_COUNT = re.compile(r"([a-z0-9][a-z0-9-]{0,62}):(\d{1,6})")
POD_PREFIX = re.compile(r"^\[pod/[^\]\s]+\] ")
# The time a line was written: a text line starts with it, a JSON line with its "timestamp" field.
LINE_AT = re.compile(r'^\s*(?:\{"timestamp":")?(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.\d+)?Z')


def event_line(line: str) -> str:
    """One log line as `message key=value ...`: jc's JSON format (log_format = "json", every
    cluster: {"fields": {"message", ...}}) in its fields' order, a text line as it is (up.sh's
    --log-format text). A `[pod/<pod>/<container>] ` prefix (kubectl logs --all-pods with --prefix)
    is dropped first, or no JSON line would parse."""
    line = POD_PREFIX.sub("", line)
    if not line.lstrip().startswith("{"):
        return line
    try:
        fields = json.loads(line).get("fields")
    except (ValueError, AttributeError):
        return line
    if not isinstance(fields, dict):
        return line
    rest = " ".join(f"{k}={v}" for k, v in fields.items() if k != "message" and isinstance(v, (str, int)))
    return f"{fields.get('message', '')} {rest}".strip()


def log_facts(text: str, now: dt.datetime | None = None) -> dict:
    """{refusedWrites: [{project, account, reason, count}], unattributedRefusals: [{slug, reason, count}],
    unplacedReport: None | [{project, pipelines}], logMinutes: None | int} of a window of the jc-server
    log; unplacedReport is None when the window holds no report. logMinutes is how far back the
    oldest line reaches: kubelet rotates the file and `--since` reads only the current one, so a
    window can hold 4 min of the 30 asked (jc-prod 2026-11-20T18:18Z); None when no line has a time.
    ponytail: the oldest line over every pod, a second pod's shorter file is not seen; per pod once
    kubectl's prefix is reliable."""
    refused: dict[tuple[str, str, str], int] = {}
    unattributed: dict[tuple[str, str], int] = {}
    report = None
    oldest = None
    raw = [POD_PREFIX.sub("", ln) for ln in ANSI.sub("", text).splitlines()]
    for at in (m[1] for ln in raw if (m := LINE_AT.match(ln))):
        oldest = at if oldest is None or at < oldest else oldest
    for line in map(event_line, raw):
        if m := REFUSED.search(line):
            key = (m["project"], m["account"], m["why"].strip()[:80])
            refused[key] = refused.get(key, 0) + 1
        elif m := UNATTRIBUTED.search(line):
            key = (m["slug"], m["why"].strip()[:80])
            unattributed[key] = unattributed.get(key, 0) + 1
        elif m := UNPLACED.search(line):
            report = [] if m["none"] else [{"project": p, "pipelines": int(n)}
                                           for p, n in PROJECT_COUNT.findall(m["projects"])]
    minutes = None
    if oldest:
        since = dt.datetime.strptime(oldest, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.UTC)
        minutes = max(0, int(((now or dt.datetime.now(dt.UTC)) - since).total_seconds() // 60))
    return {"logWindow": LOG_WINDOW, "logMinutes": minutes, "unplacedReport": report,
            "refusedWrites": [{"project": p, "account": a, "reason": why, "count": n}
                              for (p, a, why), n in sorted(refused.items(), key=lambda kv: (-kv[1], kv[0]))],
            "unattributedRefusals": [{"slug": slug, "reason": why, "count": n}
                                     for (slug, why), n in sorted(unattributed.items(), key=lambda kv: (-kv[1], kv[0]))]}


def jc_log(cfg: dict) -> dict:
    """log_facts of the jc-server container of every pod of the jc workload; a read that fails is
    named (logError), so the rows fail instead of passing on nothing."""
    try:
        text = kubectl(cfg, "-n", cfg["namespace"], "logs", cfg.get("jc_workload", "deploy/jc"),
                       "-c", cfg.get("jc_container", "jc-server"),
                       f"--since={LOG_WINDOW}", "--all-pods=true", "--prefix=false", timeout=180)
    except (RuntimeError, subprocess.TimeoutExpired) as err:
        return {"logWindow": LOG_WINDOW, "logError": str(err)[:200]}
    return log_facts(text)


# T-7467: the gateway's refused writes over the window, from the Prometheus that scrapes jc's admin
# listener (components/monitoring, job jc). A series born inside the window (the first refusal of an
# account) counts its whole value, which increase() would drop; the rest count their increase.
REFUSED_QUERY = ('sum by (account_project, account, reason) (({m} unless {m} offset {w}) or increase({m}[{w}]))')
REFUSED_METRIC = 'jc_gateway_writes_refused_total{{namespace="{ns}"}}'
# Scraped through the whole window, or a 0 means nothing.
SCRAPED_QUERY = 'min(min_over_time(up{{namespace="{ns}",endpoint="admin"}}[{w}]))'


# T-7494: how long the Portal's database pool has handed out no connection, as it reports itself.
POOL_QUERY = 'max(jc_portal_db_unavailable_seconds{{namespace="{ns}"}})'


def prometheus(cfg: dict):
    """The instant query of the Prometheus of `prometheus_namespace`, run with promtool in its pod."""
    ns = cfg["prometheus_namespace"]
    pods = kubectl(cfg, "-n", ns, "get", "pod", "-o", "name", "-l", "app.kubernetes.io/name=prometheus").split()
    if not pods:
        raise RuntimeError(f"no pod app.kubernetes.io/name=prometheus in {ns}")

    def instant(expr: str) -> list:
        return json.loads(kubectl(cfg, "-n", ns, "exec", pods[0], "-c", "prometheus", "--", "promtool",
                                  "query", "instant", "-o", "json", "http://localhost:9090", expr))

    return instant


def pool_gauge(cfg: dict) -> dict:
    """{poolUnavailable: {seconds}} of the Portal's database pool; {error} when it cannot be read."""
    if not cfg.get("prometheus_namespace"):
        return {"poolUnavailable": {"error": "watch.toml names no prometheus_namespace"}}
    try:
        got = prometheus(cfg)(POOL_QUERY.format(ns=cfg["namespace"]))
        if not got:
            raise RuntimeError("no jc_portal_db_unavailable_seconds series")
        return {"poolUnavailable": {"seconds": int(float(got[0]["value"][1]))}}
    except (RuntimeError, subprocess.TimeoutExpired, ValueError, KeyError, IndexError, TypeError) as err:
        return {"poolUnavailable": {"error": str(err)[:200]}}


def refused_counter(cfg: dict) -> dict:
    """{refusedCounter: {window, rows: [{project, account, reason, count}]}} of jc's
    jc_gateway_writes_refused_total over the last 30 min, read with promtool inside the Prometheus
    pod of `prometheus_namespace` (the distroless image has no shell; the pod's NetworkPolicy admits
    only Grafana, so no port-forward or service proxy); {window, error} when it cannot be read or the
    admin listener was not scraped the whole window. Names and counts only.
    ponytail: a Prometheus outage inside the window is not seen; count_over_time(up) if one bites."""
    ns = cfg.get("prometheus_namespace")
    if not ns:
        return {"refusedCounter": {"window": LOG_WINDOW, "error": "watch.toml names no prometheus_namespace"}}
    try:
        instant = prometheus(cfg)
        scraped = instant(SCRAPED_QUERY.format(ns=cfg["namespace"], w=LOG_WINDOW))
        if not scraped or float(scraped[0]["value"][1]) < 1:
            raise RuntimeError(f"jc's admin listener in {cfg['namespace']} was not scraped through the last {LOG_WINDOW}")
        metric = REFUSED_METRIC.format(ns=cfg["namespace"])
        rows = [{"project": str(r["metric"].get("account_project", "-")), "account": str(r["metric"].get("account", "-")),
                 "reason": str(r["metric"].get("reason", "-")), "count": round(float(r["value"][1]))}
                for r in instant(REFUSED_QUERY.format(m=metric, w=LOG_WINDOW))]
    except (RuntimeError, subprocess.TimeoutExpired, ValueError, KeyError, IndexError, TypeError) as err:
        return {"refusedCounter": {"window": LOG_WINDOW, "error": str(err)[:200]}}
    rows = sorted((r for r in rows if r["count"] > 0), key=lambda r: (-r["count"], r["project"], r["account"]))
    return {"refusedCounter": {"window": LOG_WINDOW, "rows": rows}}


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
    cfg.setdefault("jc_workload", "deploy/jc")
    cfg.setdefault("jc_container", "jc-server")
    if not re.fullmatch(r"(deploy|deployment|statefulset)/[a-z0-9][a-z0-9-]{0,62}", str(cfg["jc_workload"])):
        raise SystemExit(f"{path}: jc_workload {cfg['jc_workload']!r} is not deploy/<name>")
    for k in ("cluster", "owner", "namespace", "pg_namespace", "jc_configmap", "jc_container", "prometheus_namespace"):
        if k in cfg and not NAME.fullmatch(str(cfg[k])):
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
    snap = {**query(cfg, pod, "jc_portal", PORTAL_SQL), **query(cfg, pod, "jc_portal", PIPELINES_SQL),
            **query(cfg, pod, "jc_assistant", assistant_sql(embed_error)), **jc_log(cfg), **refused_counter(cfg),
            **pool_gauge(cfg)}
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
    """The incidents' tasks: tasks/newid, a task file claimed for the owner, tasks/note; closed with
    tasks/claim done, which changes a task only when tasks/.claims/<id>/owner names the owner."""

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
        self.claim(tid)
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

    def claim(self, tid: str, *status: str) -> None:
        self.tool("claim", tid, *status, env={"AGENT_NAME": self.owner})

    def close(self, tid: str, key: str, why: str, at: str) -> None:
        if not self.path or self.dry or tid.startswith("dry-"):
            return
        f = self.task_file(tid)
        if not f:
            raise RuntimeError(f"no task file {tid}-*.md")
        text = f.read_text()
        # Only a task this watch opened (a per-check incident of T-7309 carries its check id as the key).
        got = re.search(r"^incident: *(.+?) *$", text, re.MULTILINE)
        if not got or got.group(1) != f"{self.cluster}/orgs/{key.removeprefix(ORG)}":
            raise RuntimeError(f"{tid} is not this watch's incident {key} (incident: {got.group(1) if got else 'none'})")
        if re.search(r"^status: *done *$", text, re.MULTILINE):
            return
        # An incident opened before T-7381 carries owner: but no claim, which tasks/claim done refuses.
        if not (self.path / ".claims" / tid).is_dir():
            self.claim(tid)
        with f.open("a") as fh:
            fh.write(f"\nClosed {at} by orgs/watch.py: {key} {why}.\n")
        self.claim(tid, "done")


ORG, CLUSTER = "org/", "cluster"
BLOCK = re.compile(r"<!-- orgs-watch:checks -->.*?<!-- /orgs-watch:checks -->", re.DOTALL)


def checks_block(rows: list[dict], at: str) -> str:
    """The open checks: red ones, and held ones that skipped or did not run (marked), which keep it open."""
    lines = [f"- `{r['id']}`: {'' if r.get('result', 'fail') == 'fail' else '(' + str(r['result']) + ', not yet green) '}"
             f"{r.get('actual', '')} (expected {r.get('expected', '')}; {r.get('call', '')})" for r in rows]
    return "\n".join(["<!-- orgs-watch:checks -->", f"Open checks as of {at} ({len(rows)}):", *lines,
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


def incidents(state: dict, results: dict | None, broken: str | None,
              board: Board, at: str) -> dict:
    """The open incidents after this run: {key: {"task": id, "checks": [ids]}}. An incident closes only
    when every check it holds ran and passed (T-7467): a check that skipped (a rotated log window) or
    did not run stays held, complete run or not. An entry whose value is a bare task id is one of
    T-7309's per-check incidents: it closes when its check passes or is listed in this run's incident
    of its Organization."""
    # A `dry-` id is a placeholder an older --dry-run stored: the incident was never filed, so file it now.
    state = {k: v for k, v in state.items()
             if not str(v.get("task", "") if isinstance(v, dict) else v).startswith("dry-")}
    rows = {r["id"]: r for r in (results or {}).get("checks") or []}
    red = groups(results, broken)
    out = {k: v for k, v in state.items() if isinstance(v, dict)}

    def passed(check: str | None) -> bool:
        # the broken-run rows of groups() pass on a run that is not broken
        if check in ("snapshot", "suite"):
            return broken is None
        return rows.get(check or "", {}).get("result") == "pass"

    # a check red in another incident this run is that incident's now (an outage that narrowed)
    owner = {r["id"]: k for k, reds in red.items() for r in reds}

    def settled(key: str, check: str | None) -> bool:
        return passed(check) or owner.get(check or "", key) != key

    def held(check: str) -> dict:
        return rows.get(check) or {"id": check, "result": "not run", "actual": "not run this time"}

    def close(key: str, tid: str, why: str) -> bool:
        try:
            board.close(tid, key, why, at)
            return True
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            print(f"incident {key} ({tid}) not closed, next run tries again: {e}", file=sys.stderr)
            return False

    for key, reds in red.items():
        # a held check that is not red now but did not pass either stays on the list
        reds = reds + [held(c) for c in (out.get(key) or {}).get("checks") or []
                       if not settled(key, c) and c not in {r["id"] for r in reds}]
        ids = [r["id"] for r in reds]
        try:
            if key not in out:
                out[key] = {"task": board.open(key, reds, at), "checks": ids}
            elif set(out[key].get("checks") or []) != set(ids):
                board.update(out[key]["task"], reds, at)
                out[key]["checks"] = ids
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as e:
            print(f"incident {key} not filed or updated, next run tries again: {e}", file=sys.stderr)
    for key, inc in list(out.items()):
        if key in red:
            continue
        if not all(settled(key, c) for c in inc.get("checks") or [None]):
            continue
        if close(key, inc["task"], "all checks pass"):
            del out[key]
    tracked = {r["id"]: out[k]["task"] for k, reds in red.items() if k in out for r in reds}
    for key, tid in state.items():
        if not isinstance(tid, str):
            continue
        row = rows.get(key)
        if row and row.get("result") == "pass":
            why = "passes"
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
    p.add_argument("--dry-run", action="store_true", help="file no incident and leave incidents.json as it is; print the line")
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
    after = incidents(open_now, results, broken, board, at)
    if not a.dry_run:  # a dry run's ids are placeholders: stored, they would hide real incidents from the next run
        path.write_text(json.dumps(after, indent=1))
    return 1 if broken or (results or {}).get("totals", {}).get("fail") else 0


if __name__ == "__main__":
    sys.exit(main())
