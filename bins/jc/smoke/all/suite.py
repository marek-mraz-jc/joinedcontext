"""Suite `all`: every suite of bins/jc/smoke against one base URL, plus jc's e2e tests, one verdict.

   bins/jc/smoke/run all --base-url <url> --out <dir> [--export e] [--state s] [--log-file f]
       [--admin-url u] [--admin-token-file f] [--person-token-file f] [--e2e]
       [--only a,b] [--skip a,b] [--with <suite>=<args>]... [--suite-timeout s]

Each suite runs as its own process (`run <suite>`) into <out>/<suite>/ with the common flags it
declares; logs runs last and scans the others' result dirs. A suite that dies without results.json
is one failing row; one refused for a missing required flag is one skip row naming it. --e2e runs
`cargo test --locked -p jc --tests` and turns every test into a row. Writes <out>/results.json
(one row per check, ids prefixed with the suite) and <out>/REPORT.md (suite, checks, pass, fail,
skip, then the failures). Answers 0 only when no check failed."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
REPO = SMOKE.parent.parent.parent
sys.path.insert(0, str(SMOKE))
from common import FAIL, PASS, SKIP, loopback_only  # noqa: E402

# Flags `all` hands on, each only to a suite whose suite.py declares it.
COMMON = ("--export", "--state", "--log-file", "--admin-url", "--admin-token-file", "--person-token-file")
LAST = ("logs",)


def suites() -> list[str]:
    names = sorted(p.parent.name for p in SMOKE.glob("*/suite.py") if p.parent.name != "all")
    return [n for n in names if n not in LAST] + [n for n in LAST if n in names]


def declared(suite: str) -> set[str]:
    src = (SMOKE / suite / "suite.py").read_text()
    return {f for f in COMMON + ("--results-dir",) if f'"{f}"' in src}


def argv_for(suite: str, a, out: Path, done: list[Path]) -> list[str]:
    flags = declared(suite)
    argv = ["--base-url", a.base_url, "--out", str(out / suite)]
    for f in COMMON:
        v = getattr(a, f[2:].replace("-", "_"))
        if v and f in flags:
            argv += [f, v]
    if "--results-dir" in flags:
        for d in done:
            argv += ["--results-dir", str(d)]
    return argv + shlex.split(a.extra.get(suite, ""))


def run_suite(suite: str, argv: list[str], out: Path, timeout: float) -> dict:
    """Runs one suite; answers its rows (prefixed) and a summary line."""
    d = out / suite
    d.mkdir(parents=True, exist_ok=True)
    (d / "results.json").unlink(missing_ok=True)
    log = d / "run.log"
    cmd = [sys.executable, str(SMOKE / "run"), suite, *argv]
    started = time.time()
    with log.open("wb") as fo:
        # Its own session, so a timeout kills the suite's children (browsers, curl) too.
        p = subprocess.Popen(cmd, stdout=fo, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, 9)
            p.wait()
            code = "timeout"
    seconds = round(time.time() - started)
    call = "run " + suite + " " + " ".join(shlex.quote(x) for x in argv)
    text = log.read_text(errors="replace")
    res = d / "results.json"
    if res.exists():
        doc = json.loads(res.read_text())
        rows = [{**r, "id": f"{suite}/{r['id']}"} for r in doc.get("checks", [])]
        if code not in (0, 1) and not any(r["result"] == FAIL for r in rows):
            rows.append(row(suite, "exit", call, "exit 0 or 1", f"exit {code}", FAIL, tail(text)))
    elif code == 2 and "the following arguments are required" in text:
        rows = [row(suite, "run", call, "the suite runs", "not run", SKIP, tail(text, 1))]
    else:
        rows = [row(suite, "run", call, "results.json", f"exit {code}, no results.json", FAIL, tail(text))]
    return {"suite": suite, "rows": rows, "seconds": seconds, "exit": code, "call": call}


def row(suite, id, call, expected, actual, result, reason=""):
    return {"id": f"{suite}/{id}", "target": suite, "call": call, "expected": expected,
            "actual": actual, "result": result, "reason": reason}


def tail(text: str, n: int = 3) -> str:
    lines = [ln for ln in text.strip().splitlines() if ln.strip()]
    return " / ".join(lines[-n:])[:400]


TEST_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)", re.M)
RUNNING = re.compile(r"^\s+Running (?:unittests )?(\S+)", re.M)


def parse_e2e(text: str, call: str) -> list[dict]:
    """One row per `test x ... ok|FAILED|ignored` line, named <binary>::<test>."""
    rows, binary = [], "jc"
    events = sorted([(m.start(), "bin", m.group(1)) for m in RUNNING.finditer(text)] +
                    [(m.start(), "test", m) for m in TEST_LINE.finditer(text)], key=lambda e: e[0])
    for _, kind, v in events:
        if kind == "bin":
            binary = Path(v).stem
            continue
        name, status = v.group(1), v.group(2)
        result = {"ok": PASS, "FAILED": FAIL, "ignored": SKIP}[status]
        rows.append(row("e2e", f"{binary}::{name}", call, "ok", status, result,
                        "#[ignore]" if result == SKIP else ""))
    return rows


def run_e2e(out: Path, timeout: float) -> dict:
    d = out / "e2e"
    d.mkdir(parents=True, exist_ok=True)
    cmd = ["cargo", "test", "--locked", "-p", "jc", "--tests", "--no-fail-fast"]
    call = " ".join(cmd)
    started = time.time()
    with (d / "run.log").open("wb") as fo:
        p = subprocess.Popen(cmd, cwd=REPO, stdout=fo, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, 9)
            p.wait()
            code = "timeout"
    text = (d / "run.log").read_text(errors="replace")
    rows = parse_e2e(text, call)
    if code != 0 and not any(r["result"] == FAIL for r in rows):
        rows.append(row("e2e", "cargo", call, "exit 0", f"exit {code}", FAIL, tail(text)))
    return {"suite": "e2e", "rows": rows, "seconds": round(time.time() - started), "exit": code, "call": call}


def totals(rows) -> dict:
    t = {PASS: 0, FAIL: 0, SKIP: 0}
    for r in rows:
        t[r["result"]] += 1
    return t


def write(out: Path, base: str, runs: list[dict], seconds: int) -> bool:
    rows = [r for x in runs for r in x["rows"]]
    t = totals(rows)
    green = t[FAIL] == 0 and any(r["result"] == PASS for r in rows)
    verdict = "GREEN" if green else "RED"
    summary = [{"suite": x["suite"], "checks": len(x["rows"]), **totals(x["rows"]),
                "seconds": x["seconds"], "exit": x["exit"]} for x in runs]
    doc = {"suite": "all", "base": base, "verdict": verdict, "seconds": seconds, "totals": t,
           "suites": summary, "checks": rows}
    (out / "results.json").write_text(json.dumps(doc, indent=1, ensure_ascii=False))
    lines = [f"# Smoke suite `all`: {verdict}", "",
             f"Base {base}; {len(rows)} checks, {t[PASS]} pass, {t[FAIL]} fail, {t[SKIP]} skip ({seconds} s).", "",
             "| suite | checks | pass | fail | skip | seconds | report |", "|---|---|---|---|---|---|---|"]
    for s in summary:
        rep = f"{s['suite']}/REPORT.md" if (out / s["suite"] / "REPORT.md").exists() else f"{s['suite']}/run.log"
        lines.append(f"| {s['suite']} | {s['checks']} | {s[PASS]} | {s[FAIL]} | {s[SKIP]} | {s['seconds']} | {rep} |")
    lines.append(f"| **all** | {len(rows)} | {t[PASS]} | {t[FAIL]} | {t[SKIP]} | {seconds} | |")
    if not any(r["result"] == PASS for r in rows):
        lines += ["", "No check passed: a run that only skips is RED."]
    fails = [r for r in rows if r["result"] == FAIL]
    lines += ["", f"## Failures ({len(fails)})", ""]
    if fails:
        lines += ["| id | call | expected | actual | reason |", "|---|---|---|---|---|"]

        def cell(s):
            return str(s).replace("|", "\\|").replace("\n", " ")[:200]

        lines += ["| " + " | ".join(cell(r[k]) for k in ("id", "call", "expected", "actual", "reason")) + " |"
                  for r in fails[:200]]
        if len(fails) > 200:
            lines.append(f"\n{len(fails) - 200} more in results.json.")
    else:
        lines.append("None.")
    (out / "REPORT.md").write_text("\n".join(lines) + "\n")
    return green


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run all", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    for f in COMMON:
        p.add_argument(f)
    p.add_argument("--e2e", action="store_true", help="also cargo test -p jc --tests (from the repository root)")
    p.add_argument("--only", default="", help="comma list of suites (default: every suite present)")
    p.add_argument("--skip", default="", help="comma list of suites to leave out")
    p.add_argument("--with", dest="extra_args", action="append", default=[], metavar="SUITE=ARGS",
                   help="more flags for one suite, e.g. --with 'endpoints=--max-endpoints 5'")
    p.add_argument("--suite-timeout", type=float, default=3600)
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    a.extra = {}
    for x in a.extra_args:
        k, _, v = x.partition("=")
        if k not in suites():
            p.error(f"--with names no suite {k!r}")
        a.extra[k] = (a.extra.get(k, "") + " " + v).strip()
    chosen = [s for s in suites() if (not a.only or s in a.only.split(",")) and s not in a.skip.split(",")]
    unknown = [s for s in a.only.split(",") if s and s not in suites()]
    if unknown:
        p.error(f"no suite {', '.join(unknown)}; suites: {' '.join(suites())}")
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    started = time.time()
    runs, done = [], []
    for s in chosen:
        print(f"all: {s} ...", flush=True)
        r = run_suite(s, argv_for(s, a, out, done), out, a.suite_timeout)
        runs.append(r)
        done.append(out / s)
        t = totals(r["rows"])
        print(f"all: {s} {t[PASS]} pass, {t[FAIL]} fail, {t[SKIP]} skip ({r['seconds']} s)", flush=True)
    if a.e2e:
        print("all: e2e (cargo test -p jc --tests) ...", flush=True)
        runs.append(run_e2e(out, a.suite_timeout))
    else:
        runs.append({"suite": "e2e", "seconds": 0, "exit": None, "call": "",
                     "rows": [row("e2e", "cargo", "cargo test --locked -p jc --tests", "ok", "not run", SKIP,
                                  "pass --e2e to run jc's e2e tests")]})
    green = write(out, a.base_url, runs, round(time.time() - started))
    print((out / "REPORT.md").read_text().split("\n## Failures")[0])
    return 0 if green else 1


if __name__ == "__main__":
    sys.exit(main())
