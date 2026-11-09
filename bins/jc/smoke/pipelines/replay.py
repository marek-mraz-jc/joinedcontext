"""Every stream the Portal renders for the seed, run once against Antares through the counting
proxy: sources replayed from the recorded fixtures (jc-ekiben-portal), the gateway forwarded to
the broker, one tenant per pipeline. One results.json row per check (common.Results):

  replay.run     the stream ends 0
  replay.write   every upsert answered 2xx/207, and the outcome rows count what was written
  replay.landed  entities?type=T&count=true per type of the run's tenant equals the distinct ids
                 the upserts carried; one entity of each type reads back by id

  python3 replay.py --ekiben <bin> --broker http://127.0.0.1:<port> --admin-token-file f \
      --out <dir> [--only project.name ...] [--jobs 3]

A resident input (mqtt, nats) and a stream whose source was never recorded are a skip with that
reason (crates/ekiben/tests/portal-contract/RESULTS.md names them)."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent)]
import bench  # noqa: E402
import parity  # noqa: E402
from common import FAIL, PASS, SKIP, Results  # noqa: E402
from proxy import Proxy, tenant_of  # noqa: E402


def ids_by_type(rows_bodies: list[bytes]) -> dict[str, set]:
    out: dict[str, set] = {}
    for body in rows_bodies:
        try:
            doc = json.loads(body)
        except ValueError:
            continue
        for e in doc if isinstance(doc, list) else []:
            if isinstance(e, dict) and e.get("id"):
                out.setdefault(str(e.get("type")), set()).add(e["id"])
    return out


def get(args, run, path):
    req = urllib.request.Request(f"{args.broker}/ngsi-ld/v1/{path}", headers={
        "Authorization": f"Bearer {args.token}", "NGSILD-Tenant": tenant_of(run), "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, r.headers, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers, e.read()
    except OSError as e:
        return 0, {}, str(e).encode()


def one(args, proxy, res: Results, sid: str, d: dict):
    stream = d["stream"]
    if why := parity.runnable(stream):
        res.skip("replay.run", sid, "", "exit 0", why)
        return
    run = "r-" + sid.replace(".", "-")
    proxy.keep_bodies.add(run)
    out = bench.run_stream(args, proxy, d["project"], stream, run)
    rows = proxy.take(run)
    unrecorded = [r for r in rows if r.get("class") == "source" and r.get("status") == 599]
    if unrecorded:
        res.skip("replay.run", sid, f"GET {unrecorded[0]['route'][:120]}", "exit 0",
                 "source unreachable from the sandbox and never recorded (no fixture)")
        return
    res.check("replay.run", sid, f"ekiben run {sid}", "exit 0", f"exit {out['exit']} in {out['wall_s']} s "
              + out["stderr_tail"][-160:].replace("\n", " ") * (out["exit"] != 0), out["exit"] == 0)
    s = bench.summarize(rows, out)
    ok = all(k in ("200", "201", "204", "207") for k in s["statuses"])
    res.check("replay.write", sid, "POST .../entityOperations/upsert (proxy rows)",
              "every upsert 2xx/207; outcome sent = entities written",
              f"{s['upserts']} upserts {s['statuses']} {s['entities_sent']} entities, outcome sent "
              f"{s['outcome_sent']} failed {s['outcome_failed']}; {s['first_error'] or ''}"[:240],
              ok and s["outcome_sent"] + s["outcome_failed"] >= s["entities_sent"] - _refused(rows))
    written = ids_by_type([b for r, b in proxy.bodies.pop(run, [])])
    if not written:
        res.skip("replay.landed", sid, "", "", "the run wrote no entity (an empty source page or a failed read)")
        return
    for t, ids in sorted(written.items()):
        st, h, _ = get(args, run, f"entities?type={urllib.parse.quote(t)}&count=true&limit=0")
        n = h.get("NGSILD-Results-Count") if h else None
        eid = sorted(ids)[0]
        st2, _, raw = get(args, run, f"entities/{urllib.parse.quote(eid, safe='')}")
        res.check("replay.landed", f"{sid} {t}", f"GET entities?type={t}&count=true; GET entities/{eid}",
                  f"count {len(ids)} (distinct ids written); 200 by id",
                  f"{st} count={n}; {st2}", st == 200 and n is not None and int(n) == len(ids) and st2 == 200)


def _refused(rows):
    return sum((r.get("entities") or 0) for r in rows
               if r.get("class", "").startswith("gw:entityOperations/upsert") and r["status"] in (400, 413, 422))


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--ekiben", required=True)
    ap.add_argument("--broker", required=True)
    ap.add_argument("--admin-token-file", required=True)
    ap.add_argument("--fixtures", default="/workspace/agents/jc-ekiben-portal/work/fixtures")
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", action="append", default=[])
    ap.add_argument("--jobs", type=int, default=3)
    a = ap.parse_args(argv)
    a.token = Path(a.admin_token_file).read_text().strip()
    Path(a.out).mkdir(parents=True, exist_ok=True)
    streams = bench.load_streams()
    if a.only:
        streams = {k: v for k, v in streams.items() if k in set(a.only)}
    proxy = Proxy(a.broker, a.token, a.fixtures).start()
    res = Results("pipelines-replay", a.broker)
    with ThreadPoolExecutor(a.jobs) as pool:
        for f in [pool.submit(one, a, proxy, res, sid, d) for sid, d in sorted(streams.items())]:
            try:
                f.result()
            except Exception as e:  # noqa: BLE001 - one stream's harness error is its own row
                res.add("replay.run", "?", "", "", repr(e)[:200], FAIL)
    proxy.stop()
    res.write(Path(a.out), {"streams": len(streams), "source": str(bench.STREAMS), "fixtures": a.fixtures})
    t = res.totals()
    print(json.dumps({"pass": t[PASS], "fail": t[FAIL], "skip": t[SKIP]}))


if __name__ == "__main__":
    main()
