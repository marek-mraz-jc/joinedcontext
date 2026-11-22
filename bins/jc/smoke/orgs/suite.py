"""Smoke suite `orgs` (T-7309): each Organization of a live deployment, as it is configured now,
anonymous and read-only behind live.py's guard.

    bins/jc/smoke/run orgs --jc-config <jc.toml> --deployment <name> --domain <d> --edge <ip>
        --out <dir> [--snapshot <json>] [--assistant ORG=PUBLICID[@PROJECT[/SOURCE],...]]... [--app-host ORG=HOST]...
        [--previous <results.json>] [--max-crawl-hours 48] [--manifests <dir>] [--rate 2]

The Organizations are jc.toml's [[organizations]] (the seed) joined with the registry rows of the
snapshot (`watch.py` reads it read-only from the Portal's and the assistant's databases): a new
Organization, host, crawl source or harvest source is covered on the next run with no edit here.
Snapshot, every key optional:

    {"organizations": [{"name", "host", "catalogueId", "hosts": [{"host", "role"}]}],
     "projects": [{"name", "organization"}],
     "knowledge": [{"domain", "project", "source", "root", "lastCrawl", "pending", "errors",
                    "lastDone": {"at", "kind"}, "lastEnd": {"state", "at", "kind", "errorClass"},
                    "titles": [{"title", "lang"}]}]}

A knowledge source belongs to O through its project: the Portal registry's project row names the
Organization (sites.organization holds the site's domain, never the Organization's name).
Rows, per Organization O (target O; ids carry O so an incident is one Organization and one check):
- portal.O: its working host answers 2xx/3xx.
- oidc.O: its realm's discovery (O on hosts.idm, else the installation realm on the sign-in host)
  answers 200 over verified TLS and names exactly that issuer.
- prm.O: the protected-resource metadata of the Portal MCP on its working host (/api/v1/mcp, the
  Portal's door) names that host as the resource and that realm alone.
- prm.gateway: the gateway's own host (the first [[mounts.gateway]] without a path prefix) serves
  the metadata of /api/mcp naming itself as the resource and the installation realm alone.
- catalogue.O: its open data catalogue answers package_search (success true).
- mcp.O: MCP initialize and tools/list on its API host, anonymous, list at least one tool.
- app.O.<host>: each App host (--app-host) answers 2xx/3xx.
- harvest.O.<name>: each enabled [[dataroom.harvest]] source of O answers HEAD (any status < 500).
- crawl.O.<project>/<source>: each crawl root answers HEAD (< 500).
- crawl.O.<project>/<source>.job: the source's newest finished crawl job (crawl_jobs, full or
  sitemap) is done; a failed one is red with its error class (the text before the first colon).
- crawl.O.<project>/<source>.fresh (T-7398): the newest done crawl job of any kind (the weekly full
  crawl or the daily sitemap check), or sites.last_crawl when newer, is younger than the source's
  own cadence plus 24 h: its KnowledgeSource manifest's schedule (under --manifests, the seed in
  Git), daily when the source is a website with sitemap checks (the assistant checks it every day
  the schedule does not name), the assistant's default 0 3 * * * when the manifest names none, and
  --max-crawl-hours when no manifest names the source.
- embed.O: no passage of O's crawl sources holds an embedder refusal (`chunks.embed_error`); a
  skip on an assistant schema before migration 0025 (no such column: `errors` null).
- assistant.O: with --assistant O=<publicId>, one question through the visitor flow takes the
  search route (the `done` event's stagesMs has `search`; `cached` true or no search is a fail
  named `cached` or `direct`), answers (no `error` event, an `answer`) and cites at least one page
  of O's own crawl roots. The question names one page title of O's public, embedded index (only
  the projects or project/source pairs after `@`, the ones the deployment searches, when the flag
  names them; the manifests that say so live in Git, not in a database the watch reads), picked
  at random each run and never the previous run's question (--previous), so the answer cache
  cannot answer it and the router has to search O's pages.
- pipelines.delivering.<project>/<pipeline> (T-7445), per Pipeline of O's projects that ran in the
  last 7 days (the snapshot's pipeline_runs): fails when it delivered (sent + unchanged > 0) at
  least once in those 7 days, delivered nothing in the window, and failed in it; the window is 2 h
  or two of its periods (the median gap between its newest runs), whichever is longer. The actual
  names the failures and the newest failure's error text. Never delivered in 7 days, or no run in
  the window (paused, stopped, not due), is a skip; a run that delivered nothing and failed nothing
  passes. ci/known.txt refuses a line holding these rows (all/suite.py load_known).
Rows of the installation (target the deployment; T-7458), from the snapshot's jc-server log window:
- pipelines.refused: no write of a Pipeline's account (a verified workload) refused by the gateway
  in the window (an open-data Endpoint, no Policy, a read-only token, an unverified domain, no
  reason); the actual names project/account, count and reason per workload, most first. A 401 has
  no verified caller and names no Pipeline, so it is not counted. A refusal line with no account
  field (a jc before T-7458) fails the row too, named ?/slug <slug>. The gateway's counter
  jc_gateway_writes_refused_total over 30 min decides the row (T-7467, watch.py reads it from
  Prometheus); the log, which rotates in minutes at jc-prod's volume, can fail it or leave it
  skipped, never pass it.
- pipelines.unplaced: the Portal's newest report in the window names no project whose Pipelines
  have no Organization; the actual names each project and its count of Pipelines. No report in
  the window is a skip. A log the watch could not read fails both rows.
  ci/known.txt refuses a line holding either row, like the delivering rows.
- database.pool (T-7494): the Portal's database pool handed out a connection within the last 2 min
  (jc_portal_db_unavailable_seconds, which watch.py reads from Prometheus); unread is a skip.
- pipelines.recorded (T-7494): some Pipeline's run was recorded in the last 30 min, by the
  database's own clock. A deployment that recorded none in 7 days is a skip. Neither row may be
  known-listed.
- database.held (T-7499): no session of a jc database sits in a transaction for over 2 min, or
  idle after a ROLLBACK for over 2 min (the shape of a pool slot that never returned); the actual
  names database, application_name (sqlx sets none) and login. A healthy pool ends on a ROLLBACK
  too, and closes such a session within twice its idle timeout: 2 min by jc_pool::options (the
  row allows 10 s more), 8 min at Antares (the server ends its idle sessions at 10 min). Never
  known-listed.
An outside host (crawl and harvest) is HEAD only and only when DNS names public addresses alone.
Result lines carry statuses and counts, never a response body or a person's data."""

from __future__ import annotations

import argparse
import datetime as dt
import itertools
import json
import random
import re
import sys
import tomllib
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
for p in (SMOKE, SMOKE / "mcp"):
    sys.path.insert(0, str(p))

import live
from client import Mcp
from common import FAIL, PASS, SKIP, Http, Results, as_json, events, solve

SUITE = "orgs"
NAME = re.compile(r"[a-z][a-z0-9_-]{0,62}")
# Two phrasings per language, so one titled page still yields a question unlike the previous run's.
ASK = {
    "sk": ("Čo sa píše na vašej stránke „{t}“?", "Zhrňte mi, prosím, vašu stránku „{t}“."),
    "cs": ("Co se píše na vaší stránce „{t}“?", "Shrňte mi, prosím, vaši stránku „{t}“."),
    "fi": ("Mitä sivullanne ”{t}” kerrotaan?", "Tiivistäkää sivunne ”{t}” sisältö."),
    "de": ("Was steht auf Ihrer Seite „{t}“?", "Fassen Sie bitte Ihre Seite „{t}“ zusammen."),
    "en": ("What does your page \"{t}\" say?", "Please sum up your page \"{t}\"."),
}
TITLE_MAX = 120
LOG_WINDOW = "30m"  # watch.py LOG_WINDOW
POOL_FAILING_SECONDS = 120  # T-7494: the pool handed out nothing for minutes
HELD_SECONDS = 120  # T-7499: in a transaction
# T-7499: idle after a ROLLBACK, past the pool's reaper (twice its idle timeout) and 10 s for the close
IDLE_HELD_SECONDS = {"jc_antares": 490}  # antares-sql POOL_IDLE 240 s; every other pool jc_pool's 60 s
IDLE_HELD_DEFAULT = 130
RECORDED_MINUTES = 30  # T-7494: the deployment recorded no run
ROLES = {"working": "portal", "opendata": "opendata", "api": "api", "signin": "idm", "apps": "apps"}


def pairs(values: list[str], what: str) -> dict[str, list[str]]:
    """{org: [value]} of repeated ORG=VALUE flags."""
    out: dict[str, list[str]] = {}
    for v in values:
        org, sep, val = v.partition("=")
        if not sep or not NAME.fullmatch(org) or not val:
            raise SystemExit(f"{what} {v!r}: ORG=VALUE")
        out.setdefault(org, []).append(val.strip().lower())
    return out


def clean_title(t) -> str:
    """A page title as one line: controls dropped, spaces collapsed, cut at TITLE_MAX."""
    t = "".join(c if c.isprintable() else " " for c in str(t or ""))
    return " ".join(t.split())[:TITLE_MAX].strip()


def question(titles: list[dict], previous: str | None, rng: random.Random) -> str | None:
    """One question naming a page title of O's index, never `previous`; a title other than the
    previous run's first. None when the index holds no usable title."""
    asks = []
    for t in titles:
        title = clean_title(t.get("title") if isinstance(t, dict) else None)
        if len(title) >= 3:
            lang = str(t.get("lang") or "").lower()[:2]
            asks += [(title, q.format(t=title)) for q in ASK.get(lang, ASK["en"])]
    asks = [(t, q) for t, q in asks if q != previous]
    fresh = [q for t, q in asks if not previous or t not in previous]
    pool = fresh or [q for _, q in asks]
    return rng.choice(pool) if pool else None


def route(evs: list) -> str:
    """The route an anonymous answer took, from its `done` event: cached, search or direct."""
    done = next((d for n, d in evs if n == "done" and isinstance(d, dict)), {})
    if done.get("cached") is True:
        return "cached"
    stages = done.get("stagesMs")
    return "search" if isinstance(stages, dict) and "search" in stages else "direct"


def build(doc: dict, snap: dict, assistants: dict | None = None, apps: dict | None = None) -> dict[str, dict]:
    """{org: what its rows probe}: jc.toml's seed, the registry's rows over it, the harvest seeds
    and crawl sources it owns. An Organization the registry marks removed is left out."""
    orgs: dict[str, dict] = {}

    def org(name: str) -> dict:
        return orgs.setdefault(name, {"hosts": {}, "catalogue": None, "harvest": [], "crawl": [],
                                      "assistant": None, "asks": None, "apps": [], "pipelines": []})

    for o in doc.get("organizations") or []:
        if not NAME.fullmatch(o.get("name") or "") or o.get("host") == "auto":
            continue
        e = org(o["name"])
        e["hosts"].update({k: v.lower() for k, v in (o.get("hosts") or {}).items()
                           if isinstance(v, str) and v and k != "mcp"})
        e["catalogue"] = o.get("catalogue_id")
    removed = set()
    for row in snap.get("organizations") or []:
        name = row.get("name") or ""
        if not NAME.fullmatch(name):
            continue
        if row.get("state", "active") != "active":
            removed.add(name)
            continue
        e = org(name)
        if row.get("host"):
            e["hosts"].setdefault("portal", row["host"].lower())
        for h in row.get("hosts") or []:
            key = ROLES.get(h.get("role") or "")
            if key and h.get("host"):
                e["hosts"][key] = h["host"].lower()
        e["catalogue"] = row.get("catalogueId") or e["catalogue"]
    for seed in (doc.get("dataroom") or {}).get("harvest") or []:
        if seed.get("organization") in orgs and seed.get("enabled", True):
            orgs[seed["organization"]]["harvest"].append({"name": seed["name"], "url": seed["url"]})
    owner = {p.get("name"): p.get("organization") for p in snap.get("projects") or [] if isinstance(p, dict)}
    for k in snap.get("knowledge") or []:
        if owner.get(k.get("project")) in orgs:
            orgs[owner[k["project"]]]["crawl"].append(k)
    for pl in snap.get("pipelines") or []:
        if isinstance(pl, dict) and owner.get(pl.get("project")) in orgs:
            orgs[owner[pl["project"]]]["pipelines"].append(pl)
    for name, ids in (assistants or {}).items():
        if name in orgs:
            pid, _, projects = ids[-1].partition("@")  # the last flag wins
            orgs[name]["assistant"] = pid
            orgs[name]["asks"] = set(projects.split(",")) - {""} or None
    for name, hosts in (apps or {}).items():
        if name in orgs:
            orgs[name]["apps"] = sorted(set(hosts))
    for name in removed:
        orgs.pop(name, None)
    return {n: e for n, e in sorted(orgs.items()) if e["hosts"].get("portal")}


def host_of(url: str) -> str:
    return (urllib.parse.urlsplit(url).hostname or "").lower()


def outside_hosts(orgs: dict) -> set[str]:
    """The crawl and harvest hosts the guard admits HEAD to: the ones the configuration names."""
    return {h for e in orgs.values() for s in [*e["harvest"], *({"url": c.get("root") or ""} for c in e["crawl"])]
            if (h := host_of(s["url"]))}


def org_hosts(orgs: dict) -> set[str]:
    return {h for e in orgs.values() for k, h in e["hosts"].items() if k != "apps"} | \
        {h for e in orgs.values() for h in e["apps"]}


def parse_time(s: str | None) -> dt.datetime | None:
    try:
        return dt.datetime.fromisoformat(str(s)) if s else None
    except ValueError:
        return None


SEED = HERE.parents[3] / "deploy" / "components" / "context-gateway" / "seed"
DEFAULT_CRON = "0 3 * * *"  # the assistant's schedule::DEFAULT
SLACK_HOURS = 24.0
CRON_RANGES = ((0, 59), (0, 23), (1, 31), (1, 12), (0, 7))


def cron_sets(cron: str) -> list[set[int]] | None:
    """The five fields of `cron` as sets of values, read as jc_core::cron does (lists, ranges,
    steps, weekday 7 = Sunday); None for anything else."""
    fields = str(cron).split()
    if len(fields) != 5:
        return None
    out = []
    for f, (lo, hi) in zip(fields, CRON_RANGES):
        vals: set[int] = set()
        for part in f.split(","):
            base, slash, step = part.partition("/")
            try:
                a, b = (lo, hi) if base == "*" else (int(x) for x in (base.split("-", 1) * 2)[:2])
                if slash and "-" not in base and base != "*":
                    b = hi
                n = int(step) if slash else 1
            except ValueError:
                return None
            if not (lo <= a <= b <= hi) or n < 1:
                return None
            vals.update(range(a, b + 1, n))
        out.append(vals)
    if 7 in out[4]:
        out[4] = (out[4] - {7}) | {0}
    return [*out, fields[2] == "*", fields[4] == "*"]


def cron_period_hours(cron: str) -> float | None:
    """The longest gap between two runs of `cron`, to the hour, over a year from a fixed Monday;
    None when it is not a schedule or runs less than twice."""
    sets = cron_sets(cron)
    if not sets or not sets[0]:
        return None
    _, hours, days, months, weekdays, any_day, any_weekday = sets
    t, runs = dt.datetime(2026, 1, 5, tzinfo=dt.UTC), []
    for _ in range(400 * 24):
        day_ok, wd_ok = t.day in days, (t.isoweekday() % 7) in weekdays
        if t.hour in hours and t.month in months and \
                ((day_ok or wd_ok) if not (any_day or any_weekday) else (day_ok and wd_ok)):
            runs.append(t)
        t += dt.timedelta(hours=1)
    gaps = [(b - a).total_seconds() / 3600 for a, b in itertools.pairwise(runs)]
    return max(gaps) if gaps else None


def schedules(root: Path) -> dict[tuple[str, str], dict]:
    """{(project, source): its KnowledgeSource spec} of every manifest under `root`; a file that
    does not read as YAML (a template) is left out."""
    import yaml
    out: dict[tuple[str, str], dict] = {}
    for f in sorted(root.rglob("*.yaml")) if root.is_dir() else []:
        try:
            docs = list(yaml.safe_load_all(f.read_text()))
        except (OSError, yaml.YAMLError):
            continue
        for d in docs:
            meta = d.get("metadata") if isinstance(d, dict) else None
            if d and d.get("kind") == "KnowledgeSource" and isinstance(meta, dict) and isinstance(d.get("spec"), dict):
                out[(str(meta.get("namespace")), str(meta.get("name")))] = d["spec"]
    return out


def crawl_limit(spec: dict | None, fallback: float) -> tuple[float, str]:
    """The hours a source's newest crawl may be old, and why: its cadence plus SLACK_HOURS."""
    if spec is None:
        return fallback, "no manifest names the source"
    cron = str(spec.get("schedule") or DEFAULT_CRON)
    checks = spec.get("source") == "website" and spec.get("sitemap") is True
    sets = cron_sets(cron)
    cadence = cron_period_hours(f"{cron.split()[0]} {cron.split()[1]} * * *" if checks and sets else cron)
    if cadence is None:
        return fallback, f"schedule {cron!r} not readable"
    return cadence + SLACK_HOURS, f"schedule {cron}" + (" with daily sitemap checks" if checks else "")


def newest_crawl(c: dict) -> tuple[dt.datetime | None, str]:
    """The newest done crawl of a source and what it was: a crawl job of any kind or last_crawl."""
    done = c.get("lastDone") if isinstance(c.get("lastDone"), dict) else {}
    job, last = parse_time(done.get("at")), parse_time(c.get("lastCrawl"))
    if job and (not last or job >= last):
        return job, f"{clean_title(done.get('kind'))[:20] or 'crawl'} job"
    return last, "sites.last_crawl"


def delivering(pl: dict, now: dt.datetime) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of one Pipeline's runs: failing and silent for its window
    after delivering in the last 7 days is a fail (T-7444: 253 silent pipelines for 29 h, no alarm)."""
    runs = [(at, int(r.get("delivered") or 0), int(r.get("failed") or 0))
            for r in pl.get("runs") or [] if isinstance(r, dict) and (at := parse_time(r.get("at")))]
    last = parse_time(pl.get("lastDelivered"))
    if not last:
        return SKIP, "never delivered in 7 days"
    times = sorted(at for at, _, _ in runs)
    gaps = sorted((b - a).total_seconds() for a, b in itertools.pairwise(times))
    period = gaps[len(gaps) // 2] if gaps else 0.0
    window = max(2 * 3600.0, 2 * period)
    since = now - dt.timedelta(seconds=window)
    inside = [(d, f) for at, d, f in runs if at >= since]
    hours = f"{window / 3600:g} h"
    if not inside:
        return SKIP, f"no run in the last {hours}: paused, stopped or not due"
    ago = f"last delivered {(now - last).total_seconds() / 3600:.1f} h ago"
    if last >= since:
        return PASS, f"{ago}, {len(inside)} runs in the last {hours}"
    failed = sum(f for _, f in inside)
    got = f"0 delivered, {failed} failed in {len(inside)} runs of the last {hours}; {ago}"
    if not failed:
        return PASS, got + " (the source sent nothing)"
    return FAIL, got + f"; newest error: {clean_title(pl.get('error'))[:160] or 'no error text kept'}"


def named_refusals(rows: list[dict]) -> str:
    named = "; ".join(f"{clean_title(r.get('project'))[:63]}/{clean_title(r.get('account'))[:70]} "
                      f"{int(r.get('count') or 0)} ({clean_title(r.get('reason'))[:80]})" for r in rows[:12])
    return named + (f"; {len(rows) - 12} more" if len(rows) > 12 else "")


def pool_available(snap: dict) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of database.pool, from the Portal's own gauge."""
    got = snap.get("poolUnavailable") if isinstance(snap.get("poolUnavailable"), dict) else {}
    seconds = got.get("seconds")
    if not isinstance(seconds, int):
        why = clean_title(got.get("error"))[:160] or "no gauge in the snapshot (watch.py reads it)"
        return SKIP, f"the pool's gauge was not read: {why}"
    if seconds >= POOL_FAILING_SECONDS:
        return FAIL, f"the Portal's database pool has handed out no connection for {seconds} s"
    return PASS, f"unavailable for {seconds} s"


def sessions_held(snap: dict) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of database.held, from pg_stat_activity."""
    if not isinstance(snap.get("heldSessions"), list):
        return SKIP, "no sessions in the snapshot (watch.py reads them)"
    held = []
    for s in snap["heldSessions"]:
        db, state = clean_title(s.get("database")), clean_title(s.get("state"))
        limit = IDLE_HELD_SECONDS.get(db, IDLE_HELD_DEFAULT) if state == "idle" else HELD_SECONDS
        if (seconds := int(s.get("seconds") or 0)) > limit:
            what = "idle after ROLLBACK" if state == "idle" else state
            held.append(f"{db} application_name={clean_title(s.get('application')) or '(none)'} "
                        f"login={clean_title(s.get('login'))} {what} {seconds} s")
    if not held:
        return PASS, "no session held"
    more = f"; {len(held) - 12} more" if len(held) > 12 else ""
    return FAIL, f"{len(held)} held: {'; '.join(held[:12])}{more}"


def runs_recorded(snap: dict) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of pipelines.recorded, by the database's clock."""
    now, last = parse_time(snap.get("pipelinesAt")), parse_time(snap.get("lastRecorded"))
    if not now or not last or now - last > dt.timedelta(days=7):
        return SKIP, "no Pipeline run recorded in 7 days, or none in the snapshot"
    minutes = int((now - last).total_seconds() // 60)
    if minutes >= RECORDED_MINUTES:
        return FAIL, f"no Pipeline run recorded for {minutes} min, since {snap['lastRecorded']}"
    return PASS, f"a Pipeline run recorded {minutes} min before the snapshot"


def refused_writes(snap: dict) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of pipelines.refused. The gateway's counter over 30 min decides
    (T-7467); the jc-server log, which rotates in minutes at jc-prod's volume, can only fail the row
    or leave it skipped, never pass it."""
    rows = [r for r in snap.get("refusedWrites") or [] if isinstance(r, dict)]
    # a jc before T-7458 logs no account: its refusals count by slug, the caller unknown
    rows += [{**r, "project": "?", "account": f"slug {r.get('slug')}"}
             for r in snap.get("unattributedRefusals") or [] if isinstance(r, dict)]
    total = sum(int(r.get("count") or 0) for r in rows)
    counter = snap.get("refusedCounter") if isinstance(snap.get("refusedCounter"), dict) else {}
    if isinstance(counter.get("rows"), list):
        crows = [r for r in counter["rows"] if isinstance(r, dict)]
        if ctotal := sum(int(r.get("count") or 0) for r in crows):
            return FAIL, f"{ctotal} refused in the last {counter.get('window')} (gateway counter): {named_refusals(crows)}"
        if total:
            return FAIL, (f"{total} refused in the jc-server log of the last {snap.get('logWindow')} "
                          f"(the counter shows 0): {named_refusals(rows)}")
        return PASS, f"0 refused writes of a Pipeline in the last {counter.get('window')} (gateway counter)"
    why = clean_title(counter.get("error"))[:160] or "no gateway counter in the snapshot (watch.py reads it)"
    if "logWindow" not in snap:
        return SKIP, f"the gateway counter was not read: {why}"
    if snap.get("logError"):
        return FAIL, f"neither the counter ({why}) nor the jc-server log was read: {clean_title(snap['logError'])[:160]}"
    if total:
        return FAIL, f"{total} refused in the jc-server log of the last {snap['logWindow']}: {named_refusals(rows)}"
    read = f"{snap['logMinutes']} min of {snap['logWindow']}" if isinstance(snap.get("logMinutes"), int) else snap["logWindow"]
    return SKIP, f"0 refused in the jc-server log ({read} read), which never passes this row; the gateway counter was not read: {why}"


def unplaced_projects(snap: dict) -> tuple[str, str]:
    """(pass, fail or skip, the actual) of pipelines.unplaced over the snapshot's log window."""
    if "logWindow" not in snap:
        return SKIP, "no jc-server log in the snapshot (watch.py reads it)"
    if snap.get("logError"):
        return FAIL, f"the jc-server log was not read: {clean_title(snap['logError'])[:160]}"
    report = snap.get("unplacedReport")
    if report is None:
        return SKIP, f"no report of the Portal's 5-min Organization round in the last {snap['logWindow']}"
    rows = [r for r in report if isinstance(r, dict)]
    if not rows:
        return PASS, "every project with Pipelines has an Organization"
    named = ", ".join(f"{clean_title(r.get('project'))[:63]} ({int(r.get('pipelines') or 0)} Pipelines)"
                      for r in rows[:20])
    return FAIL, f"{len(rows)} without an Organization: {named}"


def answered(st: int) -> bool:
    return 200 <= st < 400


class Suite:
    def __init__(self, doc: dict, orgs: dict, res: Results, a):
        self.doc, self.orgs, self.r, self.a = doc, orgs, res, a
        self.snap: dict = {}
        self.previous = previous_questions(a.previous)
        self.asked: dict[str, str] = {}
        self.rng = random.Random()
        self.now = dt.datetime.now(dt.UTC)
        self.scheme = a.scheme
        self.schedules = schedules(Path(a.manifests))
        self.installation = (doc.get("identity") or {}).get("realm") or "jc"
        mount = (doc.get("mounts") or {}).get("kaisatsu")
        mount = mount if isinstance(mount, dict) else {}
        self.login = mount.get("host") or doc.get("base_host", "")
        # Slug mode serves the sign-in module as /login on base_host (bins/jc/src/module.rs Module::slug).
        self.login_path = "" if mount.get("host") or doc.get("mode") != "slug" else mount.get("path_prefix", "/login")
        mount = (doc.get("mounts") or {}).get("assistant")
        self.assistant_host = mount.get("host") if isinstance(mount, dict) else None
        mounts = (doc.get("mounts") or {}).get("gateway") or []
        self.gateway = next((m["host"].lower() for m in ([mounts] if isinstance(mounts, dict) else mounts)
                             if isinstance(m, dict) and m.get("host") and not m.get("path_prefix")), None)

    def http(self, host: str) -> Http:
        return Http(f"{self.scheme}://{host}", timeout=self.a.timeout)

    def run(self) -> None:
        if self.gateway:
            self.prm("prm.gateway", "gateway", self.gateway, "/api/mcp",
                     f"{self.scheme}://{self.login}{self.login_path}/realms/{self.installation}")
        for o, e in self.orgs.items():
            self.one(o, e)
        call = f"jc-server log, last {self.snap.get('logWindow', LOG_WINDOW)} (read-only snapshot)"
        for rid, judge, read, want in (
                ("database.pool", pool_available, "jc_portal_db_unavailable_seconds (Prometheus)",
                 "the Portal's database pool hands out connections"),
                ("database.held", sessions_held, "pg_stat_activity (read-only snapshot)",
                 "no database session of jc held past 2 min"),
                ("pipelines.recorded", runs_recorded, "max(pipeline_runs.last_at) (read-only snapshot)",
                 f"a Pipeline run recorded in the last {RECORDED_MINUTES} min"),
                ("pipelines.refused", refused_writes, call, "no write of a Pipeline refused by the gateway"),
                ("pipelines.unplaced", unplaced_projects, call, "every project with Pipelines has an Organization")):
            result, got = judge(self.snap)
            if result == SKIP:
                self.r.skip(rid, self.a.deployment, read, want, got)
            else:
                self.r.check(rid, self.a.deployment, read, want, got, result == PASS)

    def one(self, o: str, e: dict) -> None:
        hs = e["hosts"]
        portal, api = hs["portal"], hs.get("api") or hs["portal"]
        st, _, _ = self.http(portal).call("GET", "/")
        self.r.check(f"portal.{o}", o, f"GET {portal}/", "2xx/3xx", str(st), answered(st))

        signin, at, realm = (hs["idm"], "", o) if hs.get("idm") else (self.login, self.login_path, self.installation)
        issuer = f"{self.scheme}://{signin}{at}/realms/{realm}"
        st, _, raw = self.http(signin).call("GET", f"{at}/realms/{realm}/.well-known/openid-configuration")
        got = (as_json(raw) or {}).get("issuer") if st == 200 else None
        self.r.check(f"oidc.{o}", o, f"GET {signin}{at}/realms/{realm}/.well-known/openid-configuration",
                     f"200, issuer {issuer}", f"{st} {got or ''}".strip(), st == 200 and got == issuer)

        self.prm(f"prm.{o}", o, portal, "/api/v1/mcp", issuer)

        if hs.get("opendata"):
            od, path = hs["opendata"], "/api/3/action/package_search?rows=0"
        else:
            od, path = portal, "/data/api/3/action/package_search?rows=0"
        st, _, raw = self.http(od).call("GET", path)
        ok = st == 200 and (as_json(raw) or {}).get("success") is True
        self.r.check(f"catalogue.{o}", o, f"GET {od}{path}", "200, success true", str(st), ok)

        self.mcp(o, api)
        for h in e["apps"]:
            st, _, _ = self.http(h).call("GET", "/")
            self.r.check(f"app.{o}.{h}", o, f"GET {h}/", "2xx/3xx", str(st), answered(st))
        for s in e["harvest"]:
            self.reach(f"harvest.{o}.{s['name']}", o, s["url"])
        now = dt.datetime.now(dt.UTC)
        for c in e["crawl"]:
            rid = f"crawl.{o}.{c.get('project')}/{c.get('source')}"
            if not c.get("root"):
                self.r.skip(rid, o, "the crawl root", "a root page", "no root page: an upload source or not crawled yet")
                continue
            self.job(f"{rid}.job", o, c)
            last, what = newest_crawl(c)
            age = (now - last).total_seconds() / 3600 if last else None
            if not self.reach(rid, o, c["root"]):
                continue
            limit, why = crawl_limit(self.schedules.get((str(c.get("project")), str(c.get("source")))),
                                     self.a.max_crawl_hours)
            self.r.check(f"{rid}.fresh", o, "crawl_jobs done (full or sitemap), sites.last_crawl",
                         f"under {limit:g} h ({why})",
                         f"{age:.1f} h ({what})" if age is not None else "never",
                         age is not None and age <= limit)
        if e["crawl"] and all(c.get("errors") is None for c in e["crawl"]):
            self.r.skip(f"embed.{o}", o, "chunks.embed_error of O's sources", "0 refused passages",
                        "the assistant schema has no chunks.embed_error yet (assistant migration 0025)")
        elif e["crawl"]:
            errors = sum(int(c.get("errors") or 0) for c in e["crawl"])
            pending = sum(int(c.get("pending") or 0) for c in e["crawl"])
            self.r.check(f"embed.{o}", o, "chunks.embed_error of O's sources", "0 refused passages",
                         f"{errors} refused, {pending} waiting", errors == 0)
        self.assistant(o, e)
        for pl in e["pipelines"]:
            result, got = delivering(pl, self.now)
            rid = f"pipelines.delivering.{pl.get('project')}/{pl.get('pipeline')}"
            call = "pipeline_runs of the last 7 days (read-only snapshot)"
            want = "a Pipeline that delivered in 7 days delivers, or does not fail, in its window"
            if result == SKIP:
                self.r.skip(rid, o, call, want, got)
            else:
                self.r.check(rid, o, call, want, got, result == PASS)

    def job(self, rid: str, o: str, c: dict) -> None:
        """The source's newest finished crawl job is done; a failed one names its error class."""
        end = c.get("lastEnd") if isinstance(c.get("lastEnd"), dict) else None
        if not end or end.get("state") not in ("done", "failed"):
            self.r.skip(rid, o, "crawl_jobs", "the newest finished job done", "no finished crawl job")
            return
        kind = clean_title(end.get("kind"))[:20] or "crawl"
        got = f"{kind} {end['state']}" + (f": {clean_title(end.get('errorClass'))[:80] or 'no error text'}"
                                          if end["state"] == "failed" else "")
        self.r.check(rid, o, "crawl_jobs", "the newest finished job done", got, end["state"] == "done")

    def reach(self, rid: str, o: str, url: str) -> bool:
        u = urllib.parse.urlsplit(url)
        path = (u.path or "/") + (f"?{u.query}" if u.query else "")
        st, _, raw = Http(f"{u.scheme}://{u.netloc}", timeout=self.a.timeout).call("HEAD", path)
        why = raw.decode(errors="replace")[:160] if st == 0 else ""
        ok = 0 < st < 500
        self.r.check(rid, o, f"HEAD {u.netloc}{u.path}", "an answer under 500", str(st), ok, why)
        return ok

    def prm(self, rid: str, o: str, host: str, door: str, issuer: str) -> None:
        """The protected-resource metadata of `door` on `host`: resource on that host, `issuer` the only realm."""
        path = f"/.well-known/oauth-protected-resource{door}"
        st, _, raw = self.http(host).call("GET", path)
        doc = (as_json(raw) if st == 200 else None) or {}
        resource = doc.get("resource") if isinstance(doc.get("resource"), str) else ""
        named = sorted({s.rstrip("/") for s in doc.get("authorization_servers") or []
                        if isinstance(s, str) and "/realms/" in s})
        own = host_of(resource) == host
        self.r.check(rid, o, f"GET {host}{path}", f"200, resource on {host}, naming {issuer} alone",
                     f"{st} {'' if own else 'resource ' + (host_of(resource) or 'missing') + ' '}{' '.join(named)}".strip(),
                     own and named == [issuer])

    def mcp(self, o: str, api: str) -> None:
        m = Mcp(self.http(api), "/api/mcp", retries=1)
        init = m.initialize()
        if not init.ok:
            self.r.check(f"mcp.{o}", o, f"POST {api}/api/mcp initialize", "initialize, tools/list",
                         f"initialize {init.status}", False)
            return
        ans, tools = m.tools()
        self.r.check(f"mcp.{o}", o, f"POST {api}/api/mcp initialize, tools/list", "at least one tool",
                     f"{ans.status}, {len(tools)} tools", ans.ok and len(tools) > 0)

    def assistant(self, o: str, e: dict) -> None:
        pid = e["assistant"]
        rid = f"assistant.{o}"
        if not pid:
            self.r.skip(rid, o, "POST /api/v1/d/<publicId>/chat", "an answer citing O's own pages",
                        f"no public assistant named for {o} (watch --assistant {o}=<publicId>)")
            return
        asks = e["asks"]
        searched = [c for c in e["crawl"]
                    if not asks or c.get("project") in asks or f"{c.get('project')}/{c.get('source')}" in asks]
        titles = [t for c in searched for t in c.get("titles") or []]
        q = question(titles, self.previous.get(o), self.rng)
        if not q:
            self.r.check(rid, o, "a page title of O's public, embedded index", "a question naming one of O's pages",
                         f"no title in {len(searched)} knowledge sources searched", False,
                         "the sources it searches hold no public, embedded page or document with a title")
            return
        self.asked[o] = q
        h = self.http(self.assistant_host or e["hosts"]["portal"])
        call = f"POST {h.base.split('://')[1]}/api/v1/d/{pid}/chat"
        st, _, raw = h.call("GET", f"/api/v1/d/{pid}/challenge")
        ch = as_json(raw) or {}
        if st != 200 or not ch.get("challenge"):
            self.r.check(rid, o, call, "a visitor token", f"challenge {st}", False)
            return
        nonce = solve(ch["challenge"], int(ch.get("difficulty") or 0))
        st, _, raw = h.call("POST", f"/api/v1/d/{pid}/token", body={"challenge": ch["challenge"], "nonce": nonce})
        tok = (as_json(raw) or {}).get("token") if st == 200 else None
        if not tok:
            self.r.check(rid, o, call, "a visitor token", f"token {st}", False)
            return
        st, _, raw = h.call("POST", f"/api/v1/d/{pid}/chat", body={"message": q},
                            headers={"X-JC-Visitor": tok, "Accept": "text/event-stream"})
        evs = events(raw) if st == 200 else []
        errors = [d.get("status") for n, d in evs if n == "error" and isinstance(d, dict)]
        cited = [c.get("url") for n, d in evs if n == "citations" and isinstance(d, list)
                 for c in d if isinstance(c, dict) and c.get("url")]
        own = {host_of(c.get("root") or "") for c in e["crawl"]} - {""}
        mine = [u for u in cited if not own or host_of(u) in own]
        has_answer = any(n == "answer" for n, _ in evs)
        took = route(evs) if st == 200 else "none"
        actual = f"{st}, route {took}, {'answer' if has_answer else 'no answer'}, " \
            f"{len(mine)}/{len(cited)} own citations" + (f", error {errors}" if errors else "")
        why = {"cached": "cached: the answer cache answered, nothing was searched",
               "direct": "direct: the router answered without searching O's index"}.get(took, "")
        self.r.check(rid, o, call, "route search, an answer, no error event, a citation of O's own pages", actual,
                     st == 200 and took == "search" and has_answer and not errors and bool(mine), why)


def previous_questions(path: str | None) -> dict[str, str]:
    """{org: question} the previous run asked (its results.json `questions`); {} when unreadable."""
    try:
        got = json.loads(Path(path).read_text()).get("questions") if path else None
    except (OSError, ValueError, AttributeError):
        return {}
    return {o: q for o, q in got.items() if isinstance(q, str)} if isinstance(got, dict) else {}


def main(argv) -> int:
    p = argparse.ArgumentParser(prog="run orgs")
    p.add_argument("--out", required=True)
    p.add_argument("--base-url", help="ignored: the hosts come from --jc-config and --snapshot")
    p.add_argument("--jc-config", required=True, help="the deployment's host- or slug-mode jc.toml")
    p.add_argument("--deployment", required=True, help="its name, e.g. jc-prod")
    p.add_argument("--domain", required=True, help="its base_host, e.g. smartcity.joinedcontext.com")
    p.add_argument("--edge", required=True, help="the edge's IP address every host of the deployment reaches")
    p.add_argument("--edge-port", type=int, default=443)
    p.add_argument("--scheme", default="https", help=argparse.SUPPRESS)  # http: the tests' loopback edge
    p.add_argument("--snapshot", help="the registry and knowledge snapshot watch.py reads (JSON)")
    p.add_argument("--assistant", action="append", default=[], help="ORG=PUBLICID[@PROJECT[/SOURCE],...] of O's public assistant; @ what it searches")
    p.add_argument("--app-host", action="append", default=[], help="ORG=HOST of one of O's Apps")
    p.add_argument("--previous", help="the previous run's results.json: its questions are not asked again")
    p.add_argument("--max-crawl-hours", type=float, default=48,
                   help="the freshness limit of a source no manifest under --manifests names")
    p.add_argument("--manifests", default=str(SEED),
                   help="the KnowledgeSource manifests whose schedules set each source's freshness limit")
    p.add_argument("--rate", type=float, default=2.0, help="requests per second, all hosts together")
    p.add_argument("--timeout", type=float, default=30)
    a = p.parse_args(argv)
    out = Path(a.out)
    jc_toml = Path(a.jc_config).read_text()
    doc = tomllib.loads(jc_toml)
    snap = json.loads(Path(a.snapshot).read_text()) if a.snapshot else {}
    orgs = build(doc, snap, pairs(a.assistant, "--assistant"), pairs(a.app_host, "--app-host"))
    cfg = live.config(a.deployment, a.domain, a.edge, jc_toml, out, rate=a.rate, edge_port=a.edge_port,
                      scheme=a.scheme, org_hosts=org_hosts(orgs), outside=outside_hosts(orgs))
    if live.active():
        raise SystemExit("run orgs on its own (orgs/watch.py): it installs a guard of its own hosts")
    live.install(cfg)
    res = Results(SUITE, a.deployment)
    if not orgs:
        res.skip("organizations", a.deployment, "jc.toml and the registry", "an Organization",
                 "no Organization with a working host")
    suite = Suite(doc, orgs, res, a)
    # the pipeline windows count from the database's clock at the snapshot, not this host's
    suite.now = parse_time(snap.get("pipelinesAt")) or suite.now
    suite.snap = snap
    try:
        suite.run()
    finally:
        live.uninstall()
    per = {o: {"checks": 0, "fail": 0} for o in orgs}
    for row in res.rows:
        if row["target"] in per:
            per[row["target"]]["checks"] += 1
            per[row["target"]]["fail"] += row["result"] == FAIL
    res.write(out, {"organizations": per, "questions": suite.asked})
    return 1 if res.totals()["fail"] else 0
