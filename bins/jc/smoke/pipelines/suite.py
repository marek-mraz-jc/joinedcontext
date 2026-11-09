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
  held       GET  /streams/streams/{p}.{n} (jc's ekiben, its admin token from --state ekiben-token)
             200 for a resident or always-running stream; a scheduled one is loaded only for its
             runs (skip)
  run        POST /api/v1/projects/{p}/pipelines/{n}/runs        202 (the first --runs runnable
             Pipelines; a resident source is a skip)
  history    GET  /api/v1/projects/{p}/pipelines/{n}/runs        the started run appears and ends
  log        GET  .../runs/{run}/log                             one line per record
  landed     GET  /api/endpoint/{slug}/ngsi-ld/v1/entities/{id}  a record the run sent, modifiedAt
             not before the run started; entities?type=T&count=true at least the run's sent

The Portal decides which project repository it follows; the suite reads the export only to know
what must be there. Nothing it starts writes outside the rehearsal (loopback base only)."""

from __future__ import annotations

import argparse
import json
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

from manifests import Pipeline, load_pipelines  # noqa: E402
from people import Admin  # noqa: E402

SECRET_WORDS = ("secret", "sops", "decrypt", "envVar", "credential")


def conditions(item: dict) -> list[dict]:
    status = (item or {}).get("status")
    return (status.get("conditions") or []) if isinstance(status, dict) else []


def check_pipeline(r: Results, http: Http, ekiben_tok: str | None, p: Pipeline) -> dict:
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
    if bad and p.secret_refs and any(w.lower() in said.lower() for w in SECRET_WORDS):
        r.skip("status", p.key, f"GET {base} (status)", "no StreamDeployed=False",
               f"secretRefs {p.secret_refs}: {said[:200]}")
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
        r.check("held", p.key, f"GET /streams/streams/{sid}", "200 the stream the Portal deployed",
                f"{st2} {short(raw2, 100)}", st2 == 200)
    return item


def run_pipeline(r: Results, http: Http, anon: Http, p: Pipeline, wait: float) -> None:
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
    run, deadline = None, time.time() + wait
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
    why = ""
    if not run:
        st, _, raw = http.call("GET", base)
        said = [f"{c.get('reason')}: {c.get('message')}" for c in conditions(as_json(raw) or {})]
        why = f"none within {wait:.0f} s; status {'; '.join(said)[:200]}"
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
    ap.add_argument("--wait", type=float, default=300, help="seconds a run may take")
    a = ap.parse_args(argv)
    loopback_only(a.base_url)
    http = Http(a.base_url)
    r = Results("pipelines", a.base_url)
    pipelines = load_pipelines(Path(a.export) / "gitea" / "files")
    if a.only:
        pipelines = [p for p in pipelines if p.key in set(a.only)]
    ekiben_tok = ekiben_token(Path(a.state)) if a.state else None
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
                check_pipeline(r, admin.browser, ekiben_tok, p)
                if p.resident:
                    r.skip("run", p.key, "", "", f"resident {p.source_type} source: runs continuously, no one-shot run")
                elif ran < a.runs:
                    ran += 1
                    run_pipeline(r, admin.browser, http, p, a.wait)
        finally:
            admin.close()
    meta = {"export": a.export, "pipelines": len(pipelines),
            "secretRef pipelines": sum(1 for p in pipelines if p.secret_refs),
            "resident": sum(1 for p in pipelines if p.resident), "runs asked": min(a.runs, len(pipelines))}
    r.write(Path(a.out), meta)
    t = r.totals()
    print(json.dumps({"pass": t[PASS], "fail": t[FAIL], "skip": t[SKIP], "out": a.out}))
    return 1 if t[FAIL] else 0
