#!/usr/bin/env python3
"""Source-to-output throughput of one resident stream, ekiben against Bento (T-6646).

Feeds N copies of captured HSL HFP messages ('<topic> <json>' lines, e.g. 5,000 lines from
mqtt.hsl.fi) through stdin into scripts/bench/hfp-stream.yaml and writes NGSI-LD entities to a
file. Process start is part of each wall time. Prints the host load first.

Checks: ekiben runs with its DEFAULT quotas (no --quotas) and must succeed; with pipeline threads 1
both engines must write the same set of entities; with threads -1 ekiben must write every one of
them, and its summary's messages_out must equal the lines written.

    stream-bench.py --bento ./bento --ekiben target/release/ekiben --input hfp-topic.txt
"""
import argparse, json, os, statistics, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))


def run(cmd, infile, out, threads, workdir):
    env = dict(os.environ, JC_ORG_DOMAIN="hel.fi", JC_SPACE="transport",
               BENCH_OUT=out, BENCH_THREADS=str(threads))
    if os.path.exists(out):
        os.remove(out)
    started = time.perf_counter()
    # `ekiben run` prints its run summary as the last stderr line (stdout belongs to the stream).
    with open(infile) as stdin, tempfile.TemporaryFile() as stderr:
        p = subprocess.Popen(cmd, stdin=stdin, stdout=subprocess.DEVNULL, stderr=stderr,
                             env=env, cwd=workdir)
        _, status, ru = os.wait4(p.pid, 0)
        wall = time.perf_counter() - started
        stderr.seek(0)
        printed = stderr.read().decode(errors="replace").strip().splitlines()
    lines = sum(1 for _ in open(out)) if os.path.exists(out) else 0
    summary = None
    if printed:
        try:
            summary = json.loads(printed[-1])
        except ValueError:
            pass
    return {"wall": wall, "cpu": ru.ru_utime + ru.ru_stime, "rss_mb": ru.ru_maxrss // 1024,
            "lines": lines, "status": os.waitstatus_to_exitcode(status), "summary": summary}


def entities(path):
    """The distinct entities written: the pipeline drops duplicates per batch only, and where the
    1 s batching period cuts batches depends on timing, so the duplicate count varies run to run
    (Bento's own output count moves by ~700 between runs)."""
    with open(path) as f:
        return {json.dumps(json.loads(l), sort_keys=True) for l in f}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bento", required=True)
    ap.add_argument("--ekiben", required=True)
    ap.add_argument("--input", required=True, help="captured '<topic> <json>' lines")
    ap.add_argument("--copies", type=int, default=60)
    ap.add_argument("--rounds", type=int, default=3)
    a = ap.parse_args()

    work = tempfile.mkdtemp(prefix="stream-bench-")
    conf = os.path.join(HERE, "hfp-stream.yaml")
    cmd = {"bento": [os.path.abspath(a.bento), "-c", conf],
           "ekiben": [os.path.abspath(a.ekiben), "run", conf]}
    src = open(a.input).read()
    if not src.endswith("\n"):
        src += "\n"
    infile = os.path.join(work, "in.txt")
    with open(infile, "w") as f:
        f.write(src * a.copies)
    total = src.count("\n") * a.copies

    print(f"load {' '.join(open('/proc/loadavg').read().split()[:3])}, {os.cpu_count()} cores, {total} messages")
    failures = []
    rates = {"bento": [], "ekiben": []}
    for r in range(a.rounds):
        for name in ("bento", "ekiben"):
            out = os.path.join(work, f"out-{name}.jsonl")
            res = run(cmd[name], infile, out, -1, work)
            rates[name].append(total / res["wall"])
            print(f"round {r + 1} {name:6} threads -1: {res['wall']:6.2f} s wall, {res['cpu']:6.2f} s CPU, "
                  f"{total / res['wall']:8.0f} msg/s, out {res['lines']}, peak RSS {res['rss_mb']} MB, exit {res['status']}")
            if name == "ekiben":
                s = res["summary"] or {}
                if res["status"] != 0 or not s.get("success"):
                    failures.append(f"round {r + 1}: ekiben run failed: {s.get('error')}")
                if s.get("messages_out") != res["lines"]:
                    failures.append(f"round {r + 1}: messages_out {s.get('messages_out')} != {res['lines']} lines")
    med = {k: statistics.median(v) for k, v in rates.items()}
    print(f"median msg/s: bento {med['bento']:.0f}, ekiben {med['ekiben']:.0f} ({med['ekiben'] / med['bento']:.2f}x)")
    if med["ekiben"] < med["bento"]:
        failures.append("ekiben median msg/s below Bento's")

    sets = {}
    for name in ("bento", "ekiben"):
        out = os.path.join(work, f"det-{name}.jsonl")
        res = run(cmd[name], infile, out, 1, work)
        print(f"deterministic {name:6} threads 1: {res['wall']:6.2f} s wall, out {res['lines']}")
        sets[name] = entities(out)
    print(f"threads 1 output sets equal: {sets['bento'] == sets['ekiben']} ({len(sets['ekiben'])} entities)")
    if sets["bento"] != sets["ekiben"]:
        failures.append("threads 1: ekiben and Bento wrote different entities")
    parallel = entities(os.path.join(work, "out-ekiben.jsonl"))
    print(f"ekiben threads -1 wrote every threads 1 entity: {parallel == sets['ekiben']}")
    if parallel != sets["ekiben"]:
        failures.append("threads -1: ekiben lost or duplicated messages")

    for f in failures:
        print("FAIL", f)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
