"""Batch throughput of the pipeline writer (owner 2026-11-08): what Antares receives when a stream
the Portal rendered runs once, measured, nothing rebuilt.

  python3 bench.py --ekiben <ekiben bin> --broker http://127.0.0.1:<port>/broker \
      --admin-token-file <jc state>/antares-admin-token --out <dir> [--fixtures <dir>] \
      [--seed <project.name> ...] [--synthetic 5000] [--sweep 5000] [--repeat 3]

Streams come from crates/ekiben/tests/portal-contract/streams.jsonl.gz (what the Portal's
StreamDeployer hands its runner for the deployment seed, tools/portal-dump) and run with
`ekiben run` one-shot, every call at proxy.py: sources from the recorded fixtures, the gateway
forwarded to the broker (one NGSILD-Tenant per run), Portal callbacks answered.

Variants of the same stream:
  http        as rendered: split {size 1000, byte_size 1 MiB} + archive + branch/http upsert
  native-N    the branch/http upsert and its write_refused step replaced by one `ngsi_ld`
              processor (same label, URL, oauth2, rate limit, drop_on) with max_in_flight N; the
              split/archive steps removed, so the writer packs the batch itself (1000, 1 MiB)
Sweep: the Portal's source-delete stream (pipeline_source_deletes.rs render(), ported) over the
synthetic run's ids at query/delete chunk 100 (CHUNK today) and 1000.

Data sources: request counts and entities per request from the proxy rows (the body of each
entityOperations call), broker time per request = the proxy's forward time (`upstream_ms`), run
start/end = the first and last outcome row the stream posts, peak RSS = /proc/<pid>/status VmHWM
of the ekiben process (sampled every 50 ms), counts in Antares =
GET entities?type=T&count=true&limit=0 per tenant after the run."""

from __future__ import annotations

import argparse
import base64
import copy
import gzip
import json
import os
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
EKIBEN = ROOT / "crates" / "ekiben"
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(EKIBEN / "scripts" / "e2e"))
import parity  # noqa: E402

from proxy import Proxy, tenant_of  # noqa: E402

STREAMS = EKIBEN / "tests" / "portal-contract" / "streams.jsonl.gz"
RESOURCES = EKIBEN / "tests" / "portal-contract" / "resources.yaml"
QUOTAS = EKIBEN / "scripts" / "e2e" / "runner-quotas.yaml"
BATCH_MAPPING = 'root = if this.type() == "array" { this } else { [this] }'


def load_streams() -> dict[str, dict]:
    """Every rendered stream by id, each `json_schema.schema` put back from schemas.json.gz (as
    crates/ekiben-server/tests/portal_contract_common does)."""
    with gzip.open(STREAMS.parent / "schemas.json.gz", "rt") as f:
        schemas = json.load(f)

    def inline(node):
        if isinstance(node, dict):
            j = node.get("json_schema")
            if isinstance(j, dict) and str(j.get("schema", "")).startswith("sha256:"):
                j["schema"] = schemas[j["schema"][len("sha256:"):]]
            for v in node.values():
                inline(v)
        elif isinstance(node, list):
            for v in node:
                inline(v)

    with gzip.open(STREAMS, "rt") as f:
        out = {d["id"]: d for d in map(json.loads, f) if "stream" in d}
    for d in out.values():
        inline(d["stream"])
    return out


# ------------------------------------------------------------------------------ variants

def write_index(procs: list) -> int:
    for i, p in enumerate(procs):
        b = p.get("branch") or {}
        for inner in b.get("processors") or []:
            if "entityOperations/upsert" in json.dumps(inner.get("http", {}).get("url", "")):
                return i
    raise ValueError("no upsert branch in the stream")


def native(stream: dict, in_flight: int, keep_split: bool = False) -> dict:
    """The stream with its rendered upsert replaced by the native ngsi_ld processor."""
    s = copy.deepcopy(stream)
    procs = s["pipeline"]["processors"]
    w = write_index(procs)
    http = procs[w]["branch"]["processors"][0]["http"]
    url = http["url"]
    base, _, query = url.partition("/entityOperations/upsert")
    conf = {"url": base, "operation": "upsert",
            "options": "update" if "options=update" in query else "replace",
            "timeout": http.get("timeout", "30s"), "drop_on": http.get("drop_on", [400, 413, 422]),
            "oauth2": http["oauth2"], "max_in_flight": in_flight}
    if "rate_limit" in http:
        conf["rate_limit"] = http["rate_limit"]
    end = w + 1
    if end < len(procs) and procs[end].get("label") == "write_refused":
        end += 1
    start = w
    if not keep_split and w >= 2 and "split" in procs[w - 2] and "archive" in procs[w - 1]:
        start = w - 2
    procs[start:end] = [{"label": procs[w].get("label", "write_0"), "ngsi_ld": conf}]
    return s


def synthetic(template: dict, n: int, attrs: int = 1, half: int = 0) -> dict:
    """The template's stream with its source and Bento steps replaced by one generated page of
    `n` small entities of type BenchThing (validation dropped: BenchThing is in no model); the
    taps, batching, write, outcomes and identity stay as the Portal rendered them. With `half`,
    entity i carries id i mod half: every id is written twice, and a write order that changes
    leaves the older value standing."""
    s = copy.deepcopy(template)
    procs = s["pipeline"]["processors"]
    b = max(i for i, p in enumerate(procs) if p.get("mapping") == BATCH_MAPPING)
    head = [p for p in procs[:b] if (p.get("label") or "").startswith(("tap_", "pass")) or p == {"catch": []}]
    tail = [p for p in procs[b:] if not (p.get("label") or "").startswith("validation")
            and not ("catch" in p and "record" in json.dumps(p) and "jc_step" in json.dumps(p))]
    props = ", ".join(f'"p{k}": {{"type": "Property", "value": i * {k + 1}}}' for k in range(attrs))
    if half:
        # entity i carries id i mod half and seq i: each id is written twice, the later one last.
        mapping = (f'root = range(0, {n}).map_each(i -> {{"id": "urn:ngsi-ld:BenchThing:" + '
                   f'(i % {half}).string(), "type": "BenchThing", "seq": {{"type": "Property", "value": i}}, {props}}})')
    else:
        mapping = (f'root = range(0, {n}).map_each(i -> {{"id": "urn:ngsi-ld:BenchThing:" + '
                   f'i.string(), "type": "BenchThing", {props}}})')
    s["input"] = {"label": "input", "generate": {"count": 1, "interval": "", "mapping": mapping}}
    s["pipeline"]["processors"] = head + tail
    return s


def source_deletes(project: str, pipeline: str, slug: str, run: str, ids: list[str], chunk: int,
                   report: str) -> dict:
    """crates/portal/src/pipeline_source_deletes.rs render() with CHUNK = `chunk` (Python port,
    same steps; the JC-Reason header is fixed)."""
    wrote = [int(time.time()) + 3600] * len(ids)
    chunks = [{"run": run, "ids": ids[i:i + chunk], "wrote": wrote[i:i + chunk]}
              for i in range(0, len(ids), chunk)]
    encoded = base64.b64encode(json.dumps(chunks).encode()).decode()
    oauth2 = {"enabled": True, "client_key": f"{project}/{pipeline}", "token_url": "${JC_PIPELINE_TOKEN_URL}"}
    entities = f"${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1"
    nf = 'e.error.status.or(0) == 404 || e.error.type.or("").string().has_suffix("ResourceNotFound")'
    return {
        "input": {"generate": {"count": 1, "interval": "",
                               "mapping": f'root = "{encoded}".decode("base64").parse_json()'}},
        "pipeline": {"processors": [
            {"unarchive": {"format": "json_array"}},
            {"try": [
                {"branch": {
                    "request_map": 'root = {"type": "Query", "entities": [{"type": "*", "id": this.ids}]}',
                    "processors": [{"http": {
                        "url": f"{entities}/entityOperations/query?options=sysAttrs,keyValues&limit={chunk}",
                        "verb": "POST", "headers": {"Content-Type": "application/json", "Accept": "application/json"},
                        "oauth2": oauth2, "timeout": "30s", "retries": 1}}],
                    "result_map": 'root.found = this.map_each(e -> {"id": e.id.or(""), "at": e.modifiedAt.or("").ts_unix().catch(0)})'}},
                {"mapping": "let found = this.found\nlet wrote = this.wrote\n"
                            "let edited = this.ids.enumerated().filter(p -> $found.any(f -> f.id == p.value && f.at > $wrote.index(p.index).or(0) + 5)).map_each(p -> p.value)\n"
                            'root = this.without("found", "wrote")\nroot.edited = $edited\n'
                            "root.ids = this.ids.filter(i -> !$edited.contains(i))"},
                {"branch": {
                    "request_map": "root = if this.ids.length() == 0 { deleted() } else { this.ids }",
                    "processors": [{"http": {
                        "url": f"{entities}/entityOperations/delete", "verb": "POST",
                        "headers": {"Content-Type": "application/json", "Accept": "application/json",
                                    "JC-Reason": "bench source deletes"},
                        "oauth2": oauth2, "timeout": "30s", "retries": 1}}],
                    "result_map": "root.answer = if content().length() == 0 { null } else { this }"}},
                {"mapping": "let errors = this.answer.errors.or([])\nroot.run = this.run\n"
                            "root.deleted = if this.answer == null { this.ids } else { this.answer.success.or([]) }\n"
                            f"root.gone = $errors.filter(e -> {nf}).map_each(e -> e.entityId)\n"
                            f'root.failed = $errors.filter(e -> !({nf})).map_each(e -> {{"id": e.entityId, "message": e.error.title.or(e.error.detail).or("refused").string()}})\n'
                            "root.edited = this.edited"},
            ]},
            {"catch": [{"mapping": 'root = {"run": this.run, "deleted": [], "gone": [], "edited": [], "failed": this.ids.map_each(id -> {"id": id, "message": error().or("the gateway did not answer")})}'}]},
        ]},
        "output": {"http_client": {"url": report, "verb": "POST", "headers": {"Content-Type": "application/json"},
                                   "timeout": "10s", "oauth2": oauth2}},
    }


# ------------------------------------------------------------------------------ one run

def pct(values: list[float], p: float) -> float | None:
    if not values:
        return None
    v = sorted(values)
    return round(v[min(len(v) - 1, int(round(p / 100 * (len(v) - 1))))], 1)


def vm_hwm(pid: int) -> int | None:
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1])
    except OSError:
        return None
    return None


def run_stream(args, proxy: Proxy, project: str, stream: dict, run: str, prepared: bool = False,
               timeout: float = 900) -> dict:
    case = {"project": project, "stream": stream,
            "resources": parity.resources(project, str(RESOURCES))}
    cfg = stream if prepared else parity.prepare(case, proxy.url, run)
    if prepared:
        cfg = copy.deepcopy(stream)
        cfg.update(case["resources"])
        _rewrite_internal(cfg, proxy.url, run)
    env = parity.env_for(case, proxy.url, run)
    with tempfile.NamedTemporaryFile("w", suffix=".json", dir=args.out, delete=False) as f:
        json.dump(cfg, f)
        path = f.name
    cmd = [args.ekiben, "run", "--allow-private-ips", "--quotas", str(QUOTAS), path]
    t0 = time.time()
    p = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    hwm = [0]
    done = threading.Event()

    def sample():
        while not done.is_set():
            v = vm_hwm(p.pid)
            if v:
                hwm[0] = max(hwm[0], v)
            done.wait(0.05)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    try:
        out, err = p.communicate(timeout=timeout)
        code = p.returncode
    except subprocess.TimeoutExpired:
        p.kill()
        out, err = p.communicate()
        code = "timeout"
    done.set()
    wall = time.time() - t0
    os.unlink(path)
    return {"run": run, "exit": code, "wall_s": round(wall, 2), "vmhwm_kib": hwm[0] or None,
            "stderr_tail": err.decode("utf-8", "replace")[-1500:], "t0": t0}


def _rewrite_internal(cfg, stub, run):
    def walk(node):
        if isinstance(node, dict):
            for k, v in node.items():
                if k == "url" and isinstance(v, str) and v.startswith(parity.PORTAL_INTERNAL):
                    node[k] = f"{stub}/{run}{v[len(parity.PORTAL_INTERNAL):]}"
                else:
                    walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)
    walk(cfg)


def summarize(rows: list[dict], result: dict) -> dict:
    writes = [r for r in rows if r.get("class", "").startswith("gw:entityOperations/upsert")]
    outcomes = [r for r in rows if r.get("class") == "internal:outcomes"]
    statuses: dict[str, int] = {}
    for r in writes:
        statuses[str(r["status"])] = statuses.get(str(r["status"]), 0) + 1
    ents = [r["entities"] or 0 for r in writes]
    sent = sum(r.get("sent", 0) for r in outcomes)
    failed = sum(r.get("failed", 0) for r in outcomes)
    span = None
    if writes:
        span = max(r["t0"] + r["ms"] / 1000 for r in writes) - min(r["t0"] for r in writes)
    run_span = None
    if outcomes:
        run_span = max(r["t0"] for r in outcomes) - min(r["t0"] for r in outcomes)
    written = sum(e for r, e in zip(writes, ents) if r["status"] in (200, 201, 204, 207))
    return {
        **{k: result[k] for k in ("run", "exit", "wall_s", "vmhwm_kib")},
        "upserts": len(writes), "statuses": statuses,
        "entities_sent": sum(ents), "entities_per_request": {
            "min": min(ents) if ents else None, "max": max(ents) if ents else None,
            "mean": round(statistics.mean(ents), 1) if ents else None},
        "bytes_per_request": {"max": max((r["bytes"] for r in writes), default=None),
                              "mean": round(statistics.mean([r["bytes"] for r in writes])) if writes else None},
        "broker_ms": {"p50": pct([r["upstream_ms"] for r in writes if r["upstream_ms"] is not None], 50),
                      "p95": pct([r["upstream_ms"] for r in writes if r["upstream_ms"] is not None], 95)},
        "proxy_ms": {"p50": pct([r["ms"] for r in writes], 50), "p95": pct([r["ms"] for r in writes], 95)},
        "write_span_s": round(span, 3) if span is not None else None,
        "outcome_span_s": round(run_span, 3) if run_span is not None else None,
        "outcome_posts": len(outcomes), "outcome_sent": sent, "outcome_failed": failed,
        "entities_per_s_wall": round(written / result["wall_s"], 1) if result["wall_s"] else None,
        "entities_per_s_writes": round(written / span, 1) if span else None,
        "other_calls": _classes([r for r in rows if r not in writes]),
        "first_error": next((r["error"] for r in writes if r.get("error")), None),
    }


def _classes(rows):
    out: dict[str, int] = {}
    for r in rows:
        out[r.get("class", "?")] = out.get(r.get("class", "?"), 0) + 1
    return out


def broker_count(args, run: str, type_: str) -> int | None:
    req = urllib.request.Request(
        f"{args.broker}/ngsi-ld/v1/entities?type={type_}&count=true&limit=0",
        headers={"Authorization": f"Bearer {args.token}", "NGSILD-Tenant": tenant_of(run)})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return int(r.headers.get("NGSILD-Results-Count"))
    except Exception:  # noqa: BLE001 - a count that cannot be read is reported as None
        return None


def broker_get(args, run: str, eid: str):
    req = urllib.request.Request(
        f"{args.broker}/ngsi-ld/v1/entities/{urllib.parse.quote(eid, safe='')}?format=keyValues",
        headers={"Authorization": f"Bearer {args.token}", "NGSILD-Tenant": tenant_of(run)})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    except Exception:  # noqa: BLE001
        return None


def tenant_types(args, run: str) -> dict[str, int]:
    req = urllib.request.Request(f"{args.broker}/ngsi-ld/v1/types",
                                 headers={"Authorization": f"Bearer {args.token}",
                                          "NGSILD-Tenant": tenant_of(run), "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            doc = json.loads(r.read())
    except Exception:  # noqa: BLE001
        return {}
    names = doc.get("typeList") if isinstance(doc, dict) else None
    if names is None and isinstance(doc, list):
        names = [t.get("typeName") or t.get("id") for t in doc]
    return {t: broker_count(args, run, t) for t in (names or [])}


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("--ekiben", required=True)
    ap.add_argument("--broker", required=True, help="Antares base, e.g. http://127.0.0.1:42141/broker")
    ap.add_argument("--admin-token-file", required=True)
    ap.add_argument("--fixtures", default="/workspace/agents/jc-ekiben-portal/work/fixtures")
    ap.add_argument("--out", required=True)
    ap.add_argument("--seed", action="append", default=[])
    ap.add_argument("--synthetic", type=int, default=0)
    ap.add_argument("--attrs", type=int, default=1)
    ap.add_argument("--template", default="barcelona.pedestrian-crossings")
    ap.add_argument("--variants", default="http,native-1,native-4")
    ap.add_argument("--sweep", default="", help="chunk sizes of the delete sweep, e.g. 100,300,1000")
    ap.add_argument("--order", action="store_true", help="duplicate-id order check at native-1/-4")
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--fail-every", type=int, default=0)
    a = ap.parse_args(argv)
    a.token = Path(a.admin_token_file).read_text().strip()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    a.out = str(out)
    streams = load_streams()
    proxy = Proxy(a.broker, a.token, a.fixtures, fail_every=a.fail_every).start()
    stamp = time.strftime("%H%M%S")
    results = []

    def one(label, project, stream, run, prepared=False):
        res = run_stream(a, proxy, project, stream, run, prepared)
        rows = proxy.take(run)
        s = summarize(rows, res)
        s.update(label=label)
        if res["exit"] != 0:
            s["stderr_tail"] = res["stderr_tail"]
        print(json.dumps(s), flush=True)
        results.append(s)
        return s, rows

    variants = [v for v in a.variants.split(",") if v]

    def build(variant, stream):
        if variant == "http":
            return stream
        if variant.startswith("native-"):
            return native(stream, int(variant.split("-")[1]))
        raise SystemExit(f"unknown variant {variant}")

    for sid in a.seed:
        d = streams[sid]
        for rep in range(a.repeat):
            for v in variants:
                run = f"s{stamp}-{rep}-{v}-{sid.replace('.', '-')}"[:60]
                s, _ = one(f"seed {sid} {v}", d["project"], build(v, d["stream"]), run)
                s["antares"] = tenant_types(a, run)
                print(json.dumps({"run": run, "antares": s["antares"]}), flush=True)

    if a.synthetic:
        t = streams[a.template]
        base = synthetic(t["stream"], a.synthetic, a.attrs)
        for rep in range(a.repeat):
            for v in variants:
                run = f"n{stamp}-{rep}-{v}-{a.synthetic}"
                s, _ = one(f"synthetic {a.synthetic}x{a.attrs} {v}", t["project"], build(v, base), run)
                s["antares"] = {"BenchThing": broker_count(a, run, "BenchThing")}
                print(json.dumps({"run": run, "antares": s["antares"]}), flush=True)
                if a.sweep and v == variants[0] and rep == 0:
                    ids = [f"urn:ngsi-ld:BenchThing:{i}" for i in range(a.synthetic)]
                    slug = write_slug(base)
                    for chunk in [int(c) for c in a.sweep.split(",")]:
                        # The same ids written again into a fresh tenant per chunk size.
                        wrun = f"w{stamp}-{chunk}"
                        one(f"sweep-fill {chunk}", t["project"], build("http", base), wrun)
                        before = broker_count(a, wrun, "BenchThing")
                        drun = wrun  # the delete stream runs in the same tenant
                        cfg = source_deletes(t["project"], t["name"], slug, "bench", ids, chunk,
                                             f"{parity.PORTAL_INTERNAL}/internal/pipelines/{t['project']}/{t['name']}/source-deletes")
                        proxy.rows = [r for r in proxy.rows if r["run"] != drun]
                        res = run_stream(a, proxy, t["project"], cfg, drun, prepared=True)
                        rows = proxy.take(drun)
                        q = [r for r in rows if r.get("class") == "gw:entityOperations/query"]
                        dl = [r for r in rows if r.get("class") == "gw:entityOperations/delete"]
                        span = (max(r["t0"] + r["ms"] / 1000 for r in rows if r["class"].startswith("gw:"))
                                - min(r["t0"] for r in rows if r["class"].startswith("gw:"))) if q else None
                        sw = {"label": f"sweep {a.synthetic} chunk {chunk}", "run": drun, "exit": res["exit"],
                              "wall_s": res["wall_s"], "queries": len(q), "deletes": len(dl),
                              "query_ids": sum(r["entities"] or 0 for r in q),
                              "delete_ids": sum(r["entities"] or 0 for r in dl),
                              "statuses": _classes([{"class": f"{r['class']} {r['status']}"} for r in q + dl]),
                              "query_broker_ms": {"p50": pct([r["upstream_ms"] for r in q], 50),
                                                  "p95": pct([r["upstream_ms"] for r in q], 95),
                                                  "sum": round(sum(r["upstream_ms"] for r in q))},
                              "delete_broker_ms": {"p50": pct([r["upstream_ms"] for r in dl], 50),
                                                   "p95": pct([r["upstream_ms"] for r in dl], 95),
                                                   "sum": round(sum(r["upstream_ms"] for r in dl))},
                              "gw_span_s": round(span, 3) if span else None,
                              "before": before, "after": broker_count(a, drun, "BenchThing"),
                              "reports": len([r for r in rows if r.get("class") == "internal:source-deletes"]),
                              "first_error": next((r["error"] for r in q + dl if r.get("error")), None)}
                        if res["exit"] != 0:
                            sw["stderr_tail"] = res["stderr_tail"]
                        print(json.dumps(sw), flush=True)
                        results.append(sw)

    if a.order:
        t = streams[a.template]
        n = a.synthetic or 5000
        dup = synthetic(t["stream"], n, a.attrs, half=n // 2)
        for v in ("native-1", "native-4", "http"):
            for rep in range(a.repeat):
                run = f"o{stamp}-{rep}-{v}"
                s, _ = one(f"order {n} dup/2 {v}", t["project"], build(v, dup), run)
                # id j was written with seq j, then with seq j + n/2: the later must stand
                wrong, checked = 0, 0
                for j in range(0, n // 2, max(1, n // 400)):
                    e = broker_get(a, run, f"urn:ngsi-ld:BenchThing:{j}")
                    checked += 1
                    if not e or e.get("seq") != j + n // 2:
                        wrong += 1
                s["order"] = {"checked": checked, "older_value_kept": wrong}
                print(json.dumps({"run": run, "order": s["order"]}), flush=True)

    proxy.stop()
    (out / f"bench-{stamp}.json").write_text(json.dumps(results, indent=1))
    print(f"wrote {out}/bench-{stamp}.json")


def write_slug(stream: dict) -> str:
    procs = stream["pipeline"]["processors"]
    url = procs[write_index(procs)]["branch"]["processors"][0]["http"]["url"]
    return url.split("/api/endpoint/")[1].split("/")[0]


if __name__ == "__main__":
    main()
