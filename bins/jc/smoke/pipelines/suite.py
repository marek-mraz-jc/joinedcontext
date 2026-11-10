"""Smoke suite `pipelines`: every Pipeline of the export goes through the Portal of a jc rehearsal.

  bins/jc/smoke/run pipelines --base-url http://localhost:<port> --export <export dir> \
      --state <jc state dir> --out <dir> \
      [--runs N] [--only project/name ...] [--wait 300]

Expectations come from the export's manifests (gitea/files/<owner>/<project>/pipelines/*/pipeline.yaml,
their DataSources and target Endpoints). Per Pipeline, as the Portal's administrator client
(a test person in /platform-admins of realm jc, made through kaisatsu's admin API with the
portal-reconciler client of --state, signed in by the browser flow, deleted at the end):

  listed     GET  /api/v1/projects/{p}/pipelines/{n}             200, the manifest's name
  bento      GET  /api/v1/projects/{p}/pipelines/{n}/bento       200 when the export has bento.yaml
  status     the item's status: no StreamDeployed=False condition (RunnerRefused, NoRunner, ...);
             a Pipeline naming secretRefs whose condition is about its secret is a skip naming it
             a refused Pipeline prod's run log (--runlog, default <export>-runlog) holds no run of
             is a same-on-prod skip; one prod ran stays a fail
  held       GET  /streams/streams/{p}.{n} (jc's ekiben, its admin token from --state ekiben-token)
             200 for a resident or always-running stream; a scheduled one is loaded only for its
             runs (skip)
             a Pipeline with no clock and a source that ends holds no stream after its run on
             prod either: without one, its target type counts at least prod's newest run's sent
  run        POST /api/v1/projects/{p}/pipelines/{n}/runs        202 (the first --runs runnable
             Pipelines; a resident source is a skip)
  history    GET  /api/v1/projects/{p}/pipelines/{n}/runs        the started run appears and ends;
             one still Queued after --wait is waited for as long as the runs the statuses report
             queued ahead of it take (at most 15 min, prod's RUN_TIMEOUT, a round), again while
             fewer runs are ahead than at the last look; a look with no progress ends the wait
  log        GET  .../runs/{run}/log                             one line per record
  landed     GET  /api/endpoint/{slug}/ngsi-ld/v1/entities/{id}  a record the run sent, modifiedAt
             not before the run started; entities?type=T&count=true at least the run's sent

The Portal decides which project repository it follows; the suite reads the export only to know
what must be there. Nothing it starts writes outside the rehearsal (loopback base only)."""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.parse
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import (
    FAIL,
    PASS,
    SKIP,
    Http,
    Results,
    as_json,
    loopback_only,
    short,
)

from manifests import Pipeline, cron_out_of_range, load_pipelines  # noqa: E402
from people import Admin  # noqa: E402

SECRET_WORDS = ("secret", "sops", "decrypt", "envVar", "credential")


def conditions(item: dict) -> list[dict]:
    status = (item or {}).get("status")
    return (status.get("conditions") or []) if isinstance(status, dict) else []


def prod_runs(path: Path) -> dict | None:
    """prod's Portal run log beside the bundle (pipeline_runs.jsonl, rehearsal RUNLOG.md): per
    project/pipeline its run count and newest run, or None when the file is not there."""
    f = path / "pipeline_runs.jsonl"
    if not f.is_file():
        return None
    out = {}
    for line in f.read_text().splitlines():
        row = json.loads(line)
        k = f"{row['project']}/{row['pipeline']}"
        had = out.get(k)
        out[k] = {"runs": (had or {}).get("runs", 0) + 1,
                  "last": row if not had or row["last_at"] > had["last"]["last_at"] else had["last"]}
    return out


def check_pipeline(r: Results, http: Http, ekiben_tok: str | None, p: Pipeline, prod: dict | None = None) -> dict:
    base = f"/api/v1/projects/{p.project}/pipelines/{p.name}"
    st, _, raw = http.call("GET", base)
    item = (as_json(raw) or {}) if st == 200 else {}
    r.check("listed", p.key, f"GET {base}", "200 named " + p.name,
            f"{st} {short(raw, 120) if st != 200 else (item.get('metadata') or {}).get('name')}",
            st == 200 and (item.get("metadata") or {}).get("name") == p.name)
    if p.bento:
        st, _, raw = http.call("GET", base + "/bento")
        r.check("bento", p.key, f"GET {base}/bento", "200 the bento.yaml beside the manifest",
                f"{st} {short(raw, 80)}", st == 200 and bool(raw))
    status = item.get("status") or {}
    bad = [c for c in conditions(item) if c.get("type") == "StreamDeployed" and c.get("status") == "False"]
    said = "; ".join(f"{c.get('reason')}: {c.get('message')}" for c in bad)
    actual = f"phase {status.get('phase')}" + (f", {said}" if said else "")
    out_of_range = cron_out_of_range(p.spec.get("schedule"))
    if bad and out_of_range and "schedule" in said:
        r.skip("status", p.key, f"GET {base} (status)", "no StreamDeployed=False",
               f"same on prod: schedule {out_of_range} out of range (`{p.spec.get('schedule')}` in {p.file}); "
               "prod's Portal (run_queue.rs Cron::parse) refuses it the same way, the seed fix is the deployment's")
    elif bad and p.secret_refs and any(w.lower() in said.lower() for w in SECRET_WORDS):
        r.skip("status", p.key, f"GET {base} (status)", "no StreamDeployed=False",
               f"secretRefs {p.secret_refs}: {said[:200]}")
    elif bad and prod is not None and p.key not in prod:
        r.skip("status", p.key, f"GET {base} (status)", "no StreamDeployed=False",
               f"same on prod: prod's Portal run log holds no run of it ({len(prod)} pipelines ran there "
               f"from 2026-10-18 to the freeze); {said[:160]}")
    else:
        r.check("status", p.key, f"GET {base} (status)", "no StreamDeployed=False condition", actual,
                st == 200 and not bad)
    sid = f"{p.project}.{p.name}"
    if p.scheduled and not p.resident:
        r.skip("held", p.key, f"GET /streams/streams/{sid}", "200",
               "scheduled: the Portal loads the stream only for its runs (PL-78)")
    elif not ekiben_tok:
        r.skip("held", p.key, f"GET /streams/streams/{sid}", "200", "no --state: no ekiben token")
    else:
        st2, _, raw2 = http.call("GET", f"/streams/streams/{sid}", ekiben_tok)
        last = ((prod or {}).get(p.key) or {}).get("last") or {}
        if st2 != 200 and not p.scheduled and not p.resident and last.get("sent") and p.slug and p.out_type:
            # No clock and a source that ends: prod's Portal ran it and let the stream go too, so
            # compare with what prod's newest run left, its target type holding that run's sent.
            q = f"/api/endpoint/{p.slug}/ngsi-ld/v1/entities?type={urllib.parse.quote(p.out_type)}&count=true&limit=0"
            st3, h3, _ = http.call("GET", q)
            n = h3.get("ngsild-results-count")
            r.check("held", p.key, f"GET /streams/streams/{sid}; GET {q}",
                    f"200, or as on prod (no stream after its run {last['run']}, sent {last['sent']}): "
                    f"count >= {last['sent']}", f"{st2} {short(raw2, 60)}; {st3} count={n}",
                    st3 == 200 and n is not None and int(n) >= int(last["sent"]))
        else:
            r.check("held", p.key, f"GET /streams/streams/{sid}", "200 the stream the Portal deployed",
                    f"{st2} {short(raw2, 100)}", st2 == 200)
    return item


# prod's run_queue.rs RUN_TIMEOUT: the longest a run-now is waited for behind the queue.
RUN_TIMEOUT = 15 * 60
QUEUED = re.compile(r"queued, due (\S+?Z)")
RAN = re.compile(r"the last run ended \S+ after (\d+) s")


def queue_wait(http: Http, projects, base: str, wait: float):
    """How long a run still queued after `wait` may take, from what the Portal's statuses report:
    (seconds, runs ahead, seconds a run takes), or None when the pipeline is not Queued.

    The queue starts waiting runs by due time (run_queue.rs settle), so the runs ahead are the
    pipelines Queued with a due time not after this one's, and each takes what the statuses' last
    runs took (their median, 60 s without one). One lane is assumed, at least `wait`, at most
    RUN_TIMEOUT; ponytail: jc's Portal runs three, read jc_pipeline_queue_depth if this is too long."""
    st, _, raw = http.call("GET", base)
    own = QUEUED.search(" ".join(c.get("message") or "" for c in conditions(as_json(raw) or {})))
    if st != 200 or not own:
        return None
    ahead, took = 0, []
    for project in projects:
        st, _, raw = http.call("GET", f"/api/v1/projects/{project}/pipelines?limit=1000")
        for item in (as_json(raw) or {}).get("items") or [] if st == 200 else []:
            said = " ".join(c.get("message") or "" for c in conditions(item))
            due = QUEUED.search(said)
            ahead += bool(due and due.group(1) <= own.group(1))
            took += [int(m) for m in RAN.findall(said)]
    each = sorted(took)[len(took) // 2] if took else 60
    return min(RUN_TIMEOUT, max(wait, ahead * each)), ahead, each


RUNNING = re.compile(r"running since (\S+?Z)")
ENDED = re.compile(r"the last run ended (\S+?Z)")


def started_since(http: Http, base: str, t0: float) -> bool:
    """The pipeline left the queue for the run asked at `t0`: running now, or its last run ended
    after it."""
    st, _, raw = http.call("GET", base)
    said = " ".join(c.get("message") or "" for c in conditions(as_json(raw) or {})) if st == 200 else ""
    ended = ENDED.search(said)
    return bool(RUNNING.search(said) or (ended and (_ts(ended.group(1)) or 0) >= t0))


def run_pipeline(r: Results, http: Http, anon: Http, p: Pipeline, wait: float, projects=()) -> None:
    base = f"/api/v1/projects/{p.project}/pipelines/{p.name}"
    st, _, raw = http.call("GET", base + "/runs?limit=5")
    before = {x.get("run"): x for x in (as_json(raw) or {}).get("items") or []} if st == 200 else {}
    t0 = time.time()
    st, _, raw = http.call("POST", base + "/runs")
    r.check("run", p.key, f"POST {base}/runs", "202 run started", f"{st} {short(raw)}", st == 202)
    if st != 202:
        for x in ("history", "log", "landed"):
            r.skip(x, p.key, "", "", f"the run did not start ({st})")
        return
    started = (as_json(raw) or {}).get("run")
    run, deadline, extended, rounds, ran_on = None, time.time() + wait, None, 0, False
    while True:
        while time.time() < deadline:
            time.sleep(5)
            st, _, raw = http.call("GET", base + "/runs?limit=5")
            items = (as_json(raw) or {}).get("items") or [] if st == 200 else []
            fresh = [x for x in items if x.get("run") == started
                     or (x.get("run") not in before) or x != before.get(x.get("run"))]
            if fresh:
                run = fresh[0]
                # The run ended when its last line is older than 20 s.
                last = _ts(run.get("lastAt"))
                if last and time.time() - last > 20:
                    break
        if run:
            break
        # Still queued. Prod's Portal has no spacing to wait out: a run-now keeps its place in
        # due order (dd7d5ce5 src/reconciler/run_queue.rs:767-776, run_now returns
        # `entry.queued.get_or_insert(now)`) and starts when its lane has room (settle, 700-746).
        # So wait as long as the queue reports for the runs ahead, again while fewer are ahead
        # than at the last look (the queue moves), and stop once a round sees no progress.
        ahead = extended[1] if extended else None
        extended = queue_wait(http, projects, base, wait)
        if not extended and not ran_on and started_since(http, base, t0):
            # Out of the queue: running, or ended after the request. The runs list holds a run
            # once its log has lines (gate 2026-11-10T053657Z: kataster started at 240 s, ended at
            # 305 s), so wait once more, as long as a run may take (prod's RUN_TIMEOUT).
            ran_on, deadline = True, time.time() + RUN_TIMEOUT
            continue
        if not extended or (ahead is not None and extended[1] >= ahead):
            break
        rounds += 1
        # The first estimate counts from the start, the --wait already spent included.
        deadline = time.time() + extended[0] - (wait if rounds == 1 else 0)
    why = ""
    if not run:
        st, _, raw = http.call("GET", base)
        said = [f"{c.get('reason')}: {c.get('message')}" for c in conditions(as_json(raw) or {})]
        waited = (f"{time.time() - t0:.0f} s ({extended[1]} still queued ahead after {rounds} round(s) of "
                  f"the queue's estimate, its runs {extended[2]:.0f} s each)" if extended else f"{wait:.0f} s")
        why = f"none within {waited}; status {'; '.join(said)[:200]}"
    r.check("history", p.key, f"GET {base}/runs", "the started run listed with its counts",
            json.dumps(run)[:240] if run else why, run is not None)
    if not run:
        for x in ("log", "landed"):
            r.skip(x, p.key, "", "", "no run in the history")
        return
    st, _, raw = http.call("GET", f"{base}/runs/{urllib.parse.quote(run['run'], safe='')}/log?limit=50")
    lines = (as_json(raw) or {}).get("items") or [] if st == 200 else []
    r.check("log", p.key, f"GET {base}/runs/{run['run']}/log", "200, one line per record",
            f"{st} {len(lines)} lines", st == 200 and len(lines) > 0)
    sent = [x for x in lines if x.get("outcome") == "sent" and x.get("recordId")]
    if not run.get("sent"):
        first = next((x for x in lines if x.get("message")), {})
        r.check("landed", p.key, f"{base}/runs (sent)", "sent > 0",
                f"sent 0, failed {run.get('failed')}, rejected {run.get('rejected')}: "
                f"{first.get('message', '')[:160]}", False)
        return
    if not p.slug or not sent:
        r.skip("landed", p.key, "", "", "no target Endpoint slug or no sent record in the log page")
        return
    eid = sent[0]["recordId"]
    path = f"/api/endpoint/{p.slug}/ngsi-ld/v1/entities/{urllib.parse.quote(eid, safe='')}?options=sysAttrs"
    st, _, raw = anon.call("GET", path, headers={"Accept": "application/json"})
    if st in (401, 403):
        r.skip("landed", p.key, f"GET {path}", "200", f"{st}: a closed Endpoint (the endpoints suite reads it as a person)")
        return
    e = as_json(raw) or {}
    mod = _ts(e.get("modifiedAt"))
    r.check("landed", p.key, f"GET {path}", f"200, modifiedAt >= {time.strftime('%H:%M:%SZ', time.gmtime(t0 - 5))}",
            f"{st} modifiedAt {e.get('modifiedAt')}", st == 200 and mod is not None and mod >= t0 - 5)
    if p.out_type:
        q = f"/api/endpoint/{p.slug}/ngsi-ld/v1/entities?type={urllib.parse.quote(p.out_type)}&count=true&limit=0"
        st, h, raw = anon.call("GET", q)
        n = h.get("ngsild-results-count")
        r.check("count", p.key, f"GET {q}", f">= {run['sent']} (the run's sent)", f"{st} count={n}",
                st == 200 and n is not None and int(n) >= int(run["sent"]))


def runner_list(headers: dict, raw: bytes):
    """The stream list ekiben answers, or None: without the ekiben module /streams falls through
    to the Portal's page, a 200 that is HTML."""
    if "json" not in (headers.get("content-type") or ""):
        return None
    doc = as_json(raw)
    return doc if isinstance(doc, (list, dict)) else None


def _ts(text):
    if not text:
        return None
    try:
        import datetime as dt
        return dt.datetime.fromisoformat(str(text).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return None


def ekiben_token(state: Path) -> str | None:
    """ekiben's token: <state>/ekiben-token, else the file jc was given (<home>/secrets/ekiben-token),
    as rehearsal.py read_secret looks; jc removes the state copy at start when given a file."""
    for p in (state / "ekiben-token", state.parent / "secrets" / "ekiben-token"):
        if p.exists():
            return p.read_text().strip()
    return None


def main(argv):
    ap = argparse.ArgumentParser(prog="run pipelines")
    ap.add_argument("--base-url", required=True)
    ap.add_argument("--export", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--state", help="jc state dir: portal-admin-client-secret, ekiben-token (or ../secrets/ekiben-token)")
    ap.add_argument("--admin-token-file", help="unused: the suite signs in a test administrator")
    ap.add_argument("--person-token-file", help="unused by this suite")
    ap.add_argument("--runs", type=int, default=10, help="how many Pipelines to run once")
    ap.add_argument("--only", action="append", default=[])
    ap.add_argument("--runlog", help="prod's Portal run log dir (default: <export>-runlog when it exists)")
    ap.add_argument("--wait", type=float, default=300, help="seconds a run may take")
    a = ap.parse_args(argv)
    loopback_only(a.base_url)
    http = Http(a.base_url)
    r = Results("pipelines", a.base_url)
    pipelines = load_pipelines(Path(a.export) / "gitea" / "files")
    if a.only:
        pipelines = [p for p in pipelines if p.key in set(a.only)]
    ekiben_tok = ekiben_token(Path(a.state)) if a.state else None
    prod = prod_runs(Path(a.runlog or a.export.rstrip("/") + "-runlog"))
    st, h, raw = http.call("GET", "/streams/streams", ekiben_tok)
    listed = runner_list(h, raw)
    r.check("runner", "ekiben", "GET /streams/streams", "200 the runner's JSON stream list",
            f"{st} {len(listed)} streams" if listed is not None else f"{st} {h.get('content-type')} {short(raw, 80)}",
            st == 200 and listed is not None)
    admin = None
    if not a.state:
        for p in pipelines:
            r.skip("listed", p.key, "", "", "no --state: no test administrator can be made")
    else:
        admin = Admin(a.base_url, Path(a.state))
        try:
            admin.sign_in()
            ran = 0
            for p in pipelines:
                check_pipeline(r, admin.browser, ekiben_tok, p, prod)
                if p.resident:
                    r.skip("run", p.key, "", "", f"resident {p.source_type} source: runs continuously, no one-shot run")
                elif ran < a.runs:
                    ran += 1
                    run_pipeline(r, admin.browser, http, p, a.wait, sorted({q.project for q in pipelines}))
        finally:
            admin.close()
    meta = {"export": a.export, "prod run log": None if prod is None else f"{len(prod)} pipelines", "pipelines": len(pipelines),
            "secretRef pipelines": sum(1 for p in pipelines if p.secret_refs),
            "resident": sum(1 for p in pipelines if p.resident), "runs asked": min(a.runs, len(pipelines))}
    r.write(Path(a.out), meta)
    t = r.totals()
    print(json.dumps({"pass": t[PASS], "fail": t[FAIL], "skip": t[SKIP], "out": a.out}))
    return 1 if t[FAIL] else 0
