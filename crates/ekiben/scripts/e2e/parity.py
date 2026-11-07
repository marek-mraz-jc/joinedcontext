#!/usr/bin/env python3
"""End-to-end parity of ekiben with Bento 1.22.0 over the deployment seed pipelines (T-6624).

Each rendered stream (tools/portal-render output, one JSON line per pipeline) runs once in Bento
and once in `ekiben run`, one-shot (the input's clock fires once), every HTTP call pointed at
scripts/e2e/stub.py. Compared per pipeline: the messages the stream ends with (content and
metadata, in order), the requests it made (method, path, query, body, in order) and the exit
status. A time that differs is accepted when it lies as far from its own run's start in both runs
(within 10 s): `now()` reads each run's clock, and a recorded Bento run may be days old.

  parity.py run    --rendered R.jsonl --out DIR [--record] [--only P/N ...]   both runners
  parity.py check  --cases DIR --out DIR                                       ekiben vs committed Bento results
  parity.py freeze --out DIR --cases DIR --only P/N ...                         commit cases for `check`
"""
import argparse
import copy
import datetime as dt
import json
import os
import queue
import re
import shutil
import socket
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# Bento's run is cut at BENTO_TIMEOUT; ekiben's at SLOWER times Bento's own time (at least
# MIN_TIMEOUT, at most BENTO_TIMEOUT): a run that much slower is a difference of its own.
BENTO_TIMEOUT = 180
MIN_TIMEOUT = 30
SLOWER = 5
TIME = re.compile(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?"
                  r"|(?<![\d.])1[78]\d{8}(?:\d{3})?(?![\d])")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def resources(project, resources_yaml, real_rate_limits=False):
    """The runner's resources file plus the project's egress bucket the Portal declares. Unless
    `real_rate_limits`, every local bucket is widened: pacing a replayed source changes no byte a
    run writes, and the 1-per-second buckets alone held a full run past an hour."""
    import yaml

    with open(resources_yaml) as f:
        res = yaml.safe_load(f) or {}
    label = project.replace("-", "_") + "__pipeline_egress"
    limits = res.setdefault("rate_limit_resources", [])
    if all(r.get("label") != label for r in limits):
        limits.append({"label": label, "local": {"count": 500, "interval": "1s"}})
    if not real_rate_limits:
        for limit in limits:
            if "local" in limit:
                limit["local"] = {"count": 100000, "interval": "1s"}
    return res


def runnable(stream):
    """Why a stream cannot run one-shot here, or None."""
    gen = stream.get("input", {}).get("generate")
    if gen is None:
        kind = next((k for k in stream.get("input", {}) if k != "label"), "?")
        return f"{kind} input: a resident source needs a live broker"
    return None


def prepare(case, stub, run, real_pacing=False):
    """The stream as one run sees it: one tick, URLs at the stub, every end message on stdout.
    Unless `real_pacing`, a `sleep` processor sleeps 0 s: a politeness pause before each call to a
    replayed source changes no byte, and the events pipelines' 1 s pauses took 30 s a run."""
    stream = copy.deepcopy(case["stream"])
    gen = stream["input"]["generate"]
    gen["count"], gen["interval"] = 1, ""

    def walk(node):
        if isinstance(node, dict):
            for k, v in node.items():
                if k == "url" and isinstance(v, str) and re.match(r"https?://", v):
                    scheme, rest = v.split("://", 1)
                    node[k] = f"{stub}/{run}/up/{scheme}/{rest}"
                elif k == "sleep" and isinstance(v, dict) and not real_pacing:
                    v["duration"] = "0s"
                else:
                    walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    walk(stream)
    cases = stream["output"]["switch"]["cases"]
    cases[-1] = {"output": {
        "stdout": {"codec": "lines"},
        "processors": [{"mapping": 'root = {"content": content().string(), "meta": @}'}],
    }}
    stream.update(case["resources"])
    return stream


def env_for(case, stub, run):
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "TZ", "LANG")}
    project = case["project"]
    secret = "JC_CLIENT_SECRET_" + "".join(c.upper() if c.isalnum() else "_" for c in project)
    env.update({
        "JC_GATEWAY_URL": f"{stub}/{run}/gw",
        "JC_TOKEN_URL": f"{stub}/{run}/token",
        # The runner's own environment (pipeline-runner values): the installation domain.
        "JC_ORG_DOMAIN": "example.org",
        secret: "stub-secret",
    })
    return env


def run_one(cmd, cfg_path, env, timeout):
    start = time.time()
    try:
        p = subprocess.run(cmd + [cfg_path], env=env, capture_output=True, timeout=timeout, check=False)
        code, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired as t:
        code, out, err = f"timeout after {timeout:.0f} s", t.stdout or b"", t.stderr or b""
    return {"exit": code, "stdout": out.decode("utf-8", "replace"),
            "stderr": err.decode("utf-8", "replace")[-4000:], "window": [start, time.time()]}


def stamp_of(text):
    """A timestamp's seconds since the epoch, or None."""
    s = text.replace(" ", "T")
    try:
        t = dt.datetime.fromisoformat(s)
    except ValueError:
        return None
    if t.tzinfo is None:
        t = t.replace(tzinfo=dt.UTC)
    return t.timestamp()


def align(x, y, shift, early=0.0, late=0.0):
    """`y` with each timestamp or epoch number replaced by the one at the same place in `x` when the
    two lie within 10 s once `y` is moved back by `shift`, how much later its run started. `early`
    and `late` widen that by the runs' lengths: a now() read anywhere inside both runs is the same
    clock reading, whichever runtime reached it sooner."""
    if x == y:
        return y
    xs, ys = list(TIME.finditer(x)), list(TIME.finditer(y))
    if len(xs) != len(ys) or TIME.sub("", x) != TIME.sub("", y):
        return y
    out, at = [], 0
    for a, b in zip(xs, ys):
        out.append(y[at:b.start()])
        ta, tb = seconds(a.group(0)), seconds(b.group(0))
        same = ta is not None and tb is not None and -10 - late <= ta - (tb - shift) <= 10 + early
        out.append(a.group(0) if same else b.group(0))
        at = b.end()
    out.append(y[at:])
    return "".join(out)


def seconds(token):
    if token.isdigit():
        v = int(token)
        return v / 1000 if v > 10**11 else v
    return stamp_of(token)


def observed(result, log_path):
    """What one run did: its end messages, its requests, its exit status and when it ran."""
    messages = [line for line in result["stdout"].splitlines() if line.startswith('{"content"')]
    requests = []
    if os.path.exists(log_path):
        with open(log_path) as f:
            requests = [line.rstrip("\n") for line in f]
    code = result["exit"]
    return {"exit": "ok" if code == 0 else f"failed ({code})", "messages": messages,
            "requests": requests, "started": result["window"][0]}


RANDOM_ORDER = re.compile(r"\.(key_values|values)\(\)")


def order_free(text):
    """`text` with every JSON array sorted, nested JSON text included: Bento's key_values() and
    values() walk a Go map, whose order is random per run, so only the multiset can be compared."""
    def walk(v):
        if isinstance(v, str) and v[:1] in "[{":
            try:
                return json.dumps(walk(json.loads(v)), sort_keys=True, ensure_ascii=False)
            except ValueError:
                return v
        if isinstance(v, dict):
            return {k: walk(x) for k, x in v.items()}
        if isinstance(v, list):
            return sorted((walk(x) for x in v), key=lambda x: json.dumps(x, sort_keys=True, ensure_ascii=False))
        return v
    return walk(text)


REPLAYED = "Bento replays its rejected message until cut, ekiben ends failed"


def replays_forever(bento, ekiben):
    """Bento's one-shot run re-reads a message its output rejected (a nack) until it is cut, so the
    run that ends failed in ekiben is a timeout in Bento: the same, when Bento's requests are
    ekiben's over and over (the token is fetched once) and neither wrote a message."""
    if not (bento["exit"].startswith("failed (timeout") and ekiben["exit"].startswith("failed")):
        return False
    if bento["messages"] or ekiben["messages"]:
        return False
    def calls(run):
        return [r for r in run["requests"] if '"route": "token"' not in r]
    a, b = calls(bento), calls(ekiben)
    shift = ekiben["started"] - bento["started"]
    early, late = bento.get("seconds", 0), ekiben.get("seconds", 0)
    return bool(b) and len(a) >= len(b) and all(
        align(x, b[i % len(b)], shift, early, late) == x for i, x in enumerate(a))


def order_free_items(lines):
    """Every line's members as one sorted list: a batch message's content array and a request's
    body array give one item each, so batches cut at other places still compare. The rest of a
    line (method, route, the names of its metadata) stays an item of its own; metadata values are
    left out, since a batch carries its first member's, which the random order picks."""
    items = []
    for line in lines:
        try:
            doc = json.loads(line)
        except ValueError:
            items.append(line)
            continue
        key = "content" if "content" in doc else "body"
        part = doc.get(key)
        if isinstance(part, str):
            try:
                part = json.loads(part)
            except ValueError:
                pass
        if isinstance(doc.get("meta"), dict):
            doc["meta"] = sorted(doc["meta"])
        if isinstance(part, list):
            items.extend(order_free(json.dumps(x, ensure_ascii=False)) for x in part)
            doc[key] = None
        else:
            doc[key] = part
        items.append(order_free(json.dumps(doc, ensure_ascii=False)))
    return sorted(items)


def first_difference(bento, ekiben, random_order=False):
    if replays_forever(bento, ekiben):
        return None
    shift = ekiben["started"] - bento["started"]
    early, late = bento.get("seconds", 0), ekiben.get("seconds", 0)
    for field in ("exit", "messages", "requests"):
        a, b = bento[field], ekiben[field]
        if field != "exit":
            if random_order:
                a, b = order_free_items(a), order_free_items(b)
            b = [align(x, y, shift, early, late) for x, y in zip(a, b)] + b[len(a):]
        if a == b:
            continue
        if field == "exit":
            return f"exit: bento {a}, ekiben {b}"
        for i, (x, y) in enumerate(zip(a, b)):
            if x != y:
                at = next((j for j, (c, d) in enumerate(zip(x, y)) if c != d), min(len(x), len(y)))
                return f"{field}[{i}] at char {at}: bento …{x[max(0, at - 40):at + 60]!r} ekiben …{y[max(0, at - 40):at + 60]!r}"
        return f"{field}: bento {len(a)}, ekiben {len(b)}"
    return None


def execute(case, args, stub, runners):
    name = f"{case['project']}/{case['name']}"
    why = runnable(case["stream"])
    if why:
        return {"pipeline": name, "verdict": "not runnable", "why": why}
    work = os.path.join(args.out, "runs", case["project"], case["name"])
    os.makedirs(work, exist_ok=True)
    seen, took = {}, {}
    for label, cmd in runners.items():
        run = f"{case['project']}__{case['name']}__{label}".replace(".", "_")
        log = os.path.join(args.out, "requests", run + ".jsonl")
        if os.path.exists(log):
            os.remove(log)
        cfg = os.path.join(work, f"{label}.json")
        with open(cfg, "w") as f:
            json.dump(prepare(case, stub, run, args.real_rate_limits), f, indent=1)
        bento_took = took.get("bento", case.get("bento", {}).get("seconds", BENTO_TIMEOUT / SLOWER))
        timeout = BENTO_TIMEOUT if label == "bento" else min(BENTO_TIMEOUT, max(MIN_TIMEOUT, SLOWER * bento_took))
        result = run_one(cmd, cfg, env_for(case, stub, run), timeout)
        with open(os.path.join(work, f"{label}.stderr"), "w") as f:
            f.write(result["stderr"])
        seen[label] = observed(result, log)
        took[label] = round(result["window"][1] - result["window"][0], 1)
        seen[label]["seconds"] = took[label]
        with open(os.path.join(work, f"{label}.observed.json"), "w") as f:
            json.dump(seen[label], f, indent=1, ensure_ascii=False)
    bento = seen.get("bento") or case["bento"]
    unreachable = unrecorded(args, case, fixtures_dir(args))
    if unreachable:
        return {"pipeline": name, "verdict": "not runnable", "why": f"source unreachable while recording: {unreachable}"}
    random_order = bool(RANDOM_ORDER.search(json.dumps(case["stream"])))
    diff = first_difference(bento, seen["ekiben"], random_order)
    same = ("identical (order-free: key_values/values)" if random_order else "identical")
    if diff is None and replays_forever(bento, seen["ekiben"]):
        same = f"identical ({REPLAYED})"
    out = {"pipeline": name, "verdict": same if diff is None else "different",
           "messages": len(bento["messages"]), "requests": len(bento["requests"]), "seconds": took}
    if diff:
        out["why"] = diff
    return out


def fixtures_dir(args):
    return os.path.join(args.cases, "fixtures") if args.mode == "check" else args.fixtures


def unrecorded(args, case, fixtures):
    """The first outside URL Bento's run could not record (598), or None."""
    run = f"{case['project']}__{case['name']}__bento".replace(".", "_")
    used = os.path.join(args.out, "requests", run + ".fixtures")
    if args.mode == "check" or not os.path.exists(used):
        return None
    with open(used) as f:
        keys = dict.fromkeys(f.read().split())
    for key in keys:
        path = os.path.join(fixtures, key[:2], key + ".json")
        if os.path.exists(path):
            with open(path) as f:
                fixture = json.load(f)
            if fixture["status"] == 598:
                return fixture["url"]
    return None


def start_stub(args, fixtures):
    port = free_port()
    cmd = [sys.executable, os.path.join(ROOT, "scripts/e2e/stub.py"), "--port", str(port),
           "--fixtures", fixtures, "--log", os.path.join(args.out, "requests")]
    if getattr(args, "record", False):
        cmd.append("--record")
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE)
    proc.stdout.readline()
    return proc, f"http://127.0.0.1:{port}"


def load_rendered(path, resources_yaml, only, real_rate_limits):
    cases = []
    with open(path) as f:
        for line in f:
            row = json.loads(line)
            if only and f"{row['project']}/{row['name']}" not in only:
                continue
            if "stream" in row:
                row["resources"] = resources(row["project"], resources_yaml, real_rate_limits)
            cases.append(row)
    return cases


def table(rows, path, header):
    rows = sorted(rows, key=lambda r: r["pipeline"])
    runnable_rows = [r for r in rows if r["verdict"] != "not runnable"]
    same = sum(r["verdict"].startswith("identical") for r in rows)
    pct = 100 * same / len(runnable_rows) if runnable_rows else 0
    secs = [r.get("seconds", {}) for r in runnable_rows]
    slower = sum(1 for t in secs if "bento" in t and "ekiben" in t and t["ekiben"] > max(t["bento"], 1.0))
    summary = (f"{len(rows)} pipelines: {same} identical, "
               f"{sum(r['verdict'] == 'different' for r in rows)} different, "
               f"{len(rows) - len(runnable_rows)} not runnable; identical = {pct:.1f} % of runnable; "
               f"{slower} ekiben runs slower than Bento (past 1 s).")
    lines = [header, "", summary, "",
             "| Pipeline | Verdict | Messages | Requests | Bento s | ekiben s | First difference / why |",
             "|---|---|---|---|---|---|---|"]
    for r in rows:
        why = r.get("why", "").replace("|", "\\|").replace("\n", " ")[:300]
        t = r.get("seconds", {})
        lines.append(f"| {r['pipeline']} | {r['verdict']} | {r.get('messages', '')} | {r.get('requests', '')} "
                     f"| {t.get('bento', '')} | {t.get('ekiben', '')} | {why} |")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    return same, len(runnable_rows)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["run", "check", "freeze"])
    ap.add_argument("--rendered")
    ap.add_argument("--cases")
    ap.add_argument("--out", required=True)
    ap.add_argument("--fixtures", default=os.path.join(ROOT, ".parity/fixtures"))
    ap.add_argument("--resources", default=os.environ.get("EKIBEN_RUNNER_RESOURCES", ""))
    ap.add_argument("--bento", default=os.path.join(ROOT, ".bento/bento"))
    ap.add_argument("--ekiben", default=os.environ.get("EKIBEN_BIN", "ekiben"))
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--record", action="store_true")
    ap.add_argument("--real-rate-limits", action="store_true", help="keep the runner's local rate limits and sleep pauses")
    ap.add_argument("--only", nargs="*", default=[])
    ap.add_argument("--table", default=None)
    ap.add_argument("--source", default="", help="what the cases were rendered from, for the table")
    args = ap.parse_args()
    os.makedirs(os.path.join(args.out, "requests"), exist_ok=True)
    ekiben = [args.ekiben, "run", "--allow-private-ips", "--quotas",
              os.path.join(ROOT, "scripts/e2e/runner-quotas.yaml")]

    if args.mode == "freeze":
        return freeze(args)
    if args.mode == "run":
        cases = load_rendered(args.rendered, args.resources, set(args.only), args.real_rate_limits)
        fixtures = args.fixtures
        runners = {"bento": [args.bento, "--log.level", "warn", "-c"], "ekiben": ekiben}
        with open(os.path.join(args.out, "cases.jsonl"), "w") as f:
            f.writelines(json.dumps(c, ensure_ascii=False) + "\n" for c in cases)
    else:
        cases, fixtures = [], os.path.join(args.cases, "fixtures")
        for entry in sorted(os.listdir(args.cases)):
            p = os.path.join(args.cases, entry, "case.json")
            if os.path.exists(p):
                with open(p) as f:
                    cases.append(json.load(f))
        runners = {"ekiben": ekiben}
    # One stub per job: a single Python stub served every job under one GIL, and a run of 5 000
    # small requests (helsinki/service-property-energy) crawled at 11 a second beside the others.
    stubs = [start_stub(args, fixtures) for _ in range(args.jobs)]
    free = queue.Queue()
    for _, base in stubs:
        free.put(base)

    def one(c):
        if "skip" in c:
            return {"pipeline": f"{c['project']}/{c['name']}", "verdict": "not runnable", "why": c["skip"]}
        base = free.get()
        try:
            return execute(c, args, base, runners)
        finally:
            free.put(base)

    try:
        with ThreadPoolExecutor(args.jobs) as pool:
            rows = list(pool.map(one, cases))
    finally:
        for stub, _ in stubs:
            stub.terminate()
    with open(os.path.join(args.out, "results.json"), "w") as f:
        json.dump(rows, f, indent=1, ensure_ascii=False)
    same, total = table(rows, args.table or os.path.join(args.out, "table.md"),
                        "# ekiben end-to-end parity with Bento 1.22.0\n\n" + args.source)
    print(f"{same}/{total} runnable pipelines identical")
    return 0 if same == total else 1


def freeze(args):
    """Copies the named runs of `run` into committed cases: stream, Bento's observation, fixtures."""
    with open(os.path.join(args.out, "cases.jsonl")) as f:
        rendered = {f"{c['project']}/{c['name']}": c for c in map(json.loads, f)}
    fixtures_out = os.path.join(args.cases, "fixtures")
    for name in args.only:
        case = rendered[name]
        project, pipe = name.split("/")
        with open(os.path.join(args.out, "runs", project, pipe, "bento.observed.json")) as f:
            case["bento"] = json.load(f)
        d = os.path.join(args.cases, f"{project}__{pipe}")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "case.json"), "w") as f:
            json.dump(case, f, indent=1, ensure_ascii=False, sort_keys=True)
        used = os.path.join(args.out, "requests", f"{project}__{pipe}__bento".replace(".", "_") + ".fixtures")
        keys = []
        if os.path.exists(used):
            with open(used) as f:
                keys = list(dict.fromkeys(f.read().split()))
        for key in keys:
            os.makedirs(os.path.join(fixtures_out, key[:2]), exist_ok=True)
            shutil.copy(os.path.join(args.fixtures, key[:2], key + ".json"),
                        os.path.join(fixtures_out, key[:2], key + ".json"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
