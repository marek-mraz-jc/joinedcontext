#!/usr/bin/env python3
"""The release gate before every prod apply (T-5899): every standard and a load stage on dev.

  scripts/standards-gate.py run [<rev>] [--rate 8] [--duration 5m] [--only STAGE ...]
  scripts/standards-gate.py check [<rev>]       # prod-apply: refuse <rev> without a green record
  scripts/standards-gate.py --self-test

`run` gates the deployment commit dev runs (`just dev-apply` of <rev>, default HEAD) and writes one
record to $JC_RELEASE_GATE_DIR/<sha>/ (default /workspace/tasks/evidence/release-gate):
record.json (sha, image pins, every stage and case), report.md and junit.xml. The stages are dev's
smoke, the standards suites of joinedcontext-conformance (NGSI-LD ETSI smoke.robot with writes when
NGSILD_TOKEN and WRITE_ENDPOINT are set, OGC API Features ETS + GeoJSON, OData, SensorThings, iCal,
WebSocket, MCP), the roundtrip (tests/roundtrip, T-5914: one t-roundtrip- Entity written through NGSI-LD
on the conformance Endpoint with NGSILD_TOKEN, read back through every served surface current, temporal
and stream, then deleted), DCAT-AP SHACL of dev's CKAN, the Portal live sweep (every journey, MQTT included, the critic-* shot walks not)
the k6 API mix at --rate for --duration (p95 < 500 ms, 0 failed requests), and antares-sweep (T-6151): a green
run of the Antares weekly sweep (sweep.yml, every check that left Antares's per-push gate) on the exact commit the
broker pin names. The run dispatches that sweep at its start when the commit has none, so it runs beside the
other stages; the stage waits for it and names the run in the record. Before the first stage the
run refuses in seconds when a stage would only fail on setup: ANTARES_DIR not at the Antares commit the
candidate's broker pin names (ogc, odata, dcat), no NGSILD_TOKEN (roundtrip), a demo password of the sweep
missing. Each stage ends after --stage-timeout seconds (--sweep-timeout for the sweep; its process group is
killed, the stage is red) and
the record is rewritten after every stage, so a stopped run still leaves a RED record. A stage passes when it
exits 0, or when every red case it left matches ci/release-gate-known-red.txt (each line names the
task that owns the red; a stage red with no case to show, a Python traceback or a red load stays red).
A record is green when every stage ran and passed and no case green in the previous green record is
red now, unless the known-red file parks that case.

Every target is dev and nothing else: the gate has no target parameter, so it never writes to prod.
The ETSI TTF suite never runs here (owner rule). Credentials come only from the environment the
caller set from the secrets mount (ANTARES_DIR, NGSILD_TOKEN, the *_PASSWORD of the sweep); the
record holds no environment.

The hold (T-6318): dev's pins may move while a long stage runs, but never under a stage that
measures them. Before each stage the run writes $JC_RELEASE_GATE_DIR/.hold.json: the components
(components/<c>/images.yaml) that stage and every later one measure (MEASURES), so the hold only
shrinks during a run, and an `until` after which a gate that died stops holding. The supervisor's
pin-apply cron asks `held <base> <head>` before `just dev-apply-pins` and applies only a range whose
pins the hold leaves free:

  scripts/standards-gate.py held <base> <head>  # exit 1 naming the held components, 0 when free

The run also refuses to start unless dev's ConfigMap jc-release names a commit with the candidate's
pins, and reads it again before and after every stage: a stage under which a component it measures
ran other pins (a hand apply, a race) is red, so a record still judges the candidate's images only.

`check` is the step `just prod-apply` runs after `release.py verify`: it passes only when a green
record names the same image pins and either the same commit or an ancestor that differs from it
only in what `release.py cut` writes (releases/, CHANGELOG.md).
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RECORDS = Path(os.environ.get("JC_RELEASE_GATE_DIR", "/workspace/tasks/evidence/release-gate"))
DOMAIN = "dev.joinedcontext.com"
API = f"https://api.{DOMAIN}"
# What `release.py cut` adds on top of the gated main commit.
RELEASE_FILES = re.compile(r"^(releases/[^/]+|CHANGELOG\.md)$")
SUITES = ("ngsi-ld", "ogc", "odata", "sta", "ical", "ws", "mcp")
# The critic-* walks record 1440 + 400 shots and timings for the UI critic tasks (T-5523, T-5982); they
# judge no release and took 1,619 s of the first record's sweep hour, so the gate's sweep leaves them out.
SWEEP_SKIP = r"/critic-[a-z]+\.spec\.ts$"
# The sweep stage's cap follows its size (T-6354): gate 39a90ab9 ran 291 of its 324 tests in 17,304 s at
# Playwright's one worker (59.5 s each, reds waiting out their timeouts included) and hit the old fixed 18,000 s.
# The cap is the counted tests times that, with 25 % room; SWEEP_TESTS stands in when the count fails.
SWEEP_TEST_S = 60
SWEEP_ROOM = 0.75
SWEEP_TESTS = 324
# sweep.sh runs SWEEP_WORKERS specs at once since T-6404 (the Portal's e2e/live/stateGroups.ts keeps specs that
# share state apart): the cap divides the measured one-worker time by them. A Portal pin whose sweep.sh knows no
# SWEEP_WORKERS still runs on one worker and gets the one-worker cap.
SWEEP_WORKERS = 4
REQUIRED = ("dev-smoke", *SUITES, "roundtrip", "dcat", "portal-sweep", "load", "antares-sweep")
# What a stage needs from the caller's environment, or it fails on setup after hours instead of seconds.
NEEDS_ANTARES = ("ogc", "odata", "dcat")
NEEDS_ENV = {"roundtrip": ("NGSILD_TOKEN",),
             "portal-sweep": ("PORTAL_PASSWORD", "APPROVER_PASSWORD", "VIEWER_PASSWORD", "EDITOR_PASSWORD")}
BROKER_IMAGES = "components/context-broker/images.yaml"
# Antares's weekly sweep (T-6151): its run name is `sweep <commit>`, its `sha` input a commit on dev.
ANTARES_REPO = "marek-mraz-jc/Antares-NGSI-LD-Context-Broker"
SWEEP = "sweep.yml"
# The write-here read-everywhere Endpoint (T-5914): the conformance space nothing publishes, written only
# by the conformance ServiceAccount (Policy conformance-write); WRITE_ENDPOINT overrides its slug.
CONFORMANCE_ENDPOINT = ROOT / "components/context-gateway/seed/banskabystrica/endpoint-conformance.yaml"
KNOWN_RED = ROOT / "ci/release-gate-known-red.txt"
# What each stage measures on dev, by components/<c>/images.yaml ("*": every component). The smoke and the
# Portal sweep cross everything; the standards suites, the roundtrip and the load read the data plane, whose
# views the Portal reconciler writes (a Portal restart re-reconciles them under a running suite); dcat reads the
# CKAN the Portal publishes to; antares-sweep runs on GitHub against the pinned broker commit and reads no dev pod.
DATA_PLANE = ("context-broker", "context-gateway", "portal")
MEASURES = {"dev-smoke": ("*",), "portal-sweep": ("*",), "dcat": ("ckan", *DATA_PLANE), "antares-sweep": (),
            **{name: DATA_PLANE for name in (*SUITES, "roundtrip", "load")}}
HOLD = ".hold.json"
# Past a stage's timeout the run stops it and moves on within seconds; a hold older than that is a dead gate's.
HOLD_SLACK = 600
TASK_ID = re.compile(r"T-\d{4,}")


class Refused(Exception):
    """The gate does not hold; nothing was applied."""


def git(*args: str, cwd: Path = ROOT) -> str:
    done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=False)
    if done.returncode:
        raise Refused(f"git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout.strip()


def pins(rev: str, cwd: Path = ROOT) -> dict[str, str]:
    """component -> the text of its components/<component>/images.yaml at `rev`."""
    out = {}
    for path in git("ls-tree", "-r", "--name-only", rev, "components", cwd=cwd).splitlines():
        found = re.fullmatch(r"components/([^/]+)/images\.yaml", path)
        if found:
            out[found.group(1)] = git("show", f"{rev}:{path}", cwd=cwd)
    return out


def images_at(rev: str, cwd: Path = ROOT) -> list[str]:
    """The image pins of `rev`, read by release-notes.py from that commit's components/*/images.yaml."""
    spec = importlib.util.spec_from_file_location("release_notes", ROOT / "scripts/release-notes.py")
    notes = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(notes)
    with tempfile.TemporaryDirectory() as tmp:
        for component, text in pins(rev, cwd).items():
            (Path(tmp) / "components" / component).mkdir(parents=True, exist_ok=True)
            (Path(tmp) / "components" / component / "images.yaml").write_text(text)
        return notes.images(Path(tmp) / "components")


def candidate(rev: str, cwd: Path = ROOT) -> dict:
    return {"sha": git("rev-parse", "--verify", f"{rev}^{{commit}}", cwd=cwd), "images": images_at(rev, cwd)}


def broker_commit(rev: str, cwd: Path = ROOT) -> tuple[str, str]:
    """(tag, Antares commit) of the broker `rev` pins: the tag, and the `Antares dev <sha> (<tag>` note above it."""
    text = git("show", f"{rev}:{BROKER_IMAGES}", cwd=cwd)
    tags = re.findall(r"^\s+tag:\s*'?([\w.-]+)'?\s*$", text, re.MULTILINE)
    if not tags:
        raise Refused(f"{BROKER_IMAGES} at {rev} pins no broker tag")
    shas = re.findall(rf"Antares dev ([0-9a-f]{{7,40}}) \({re.escape(tags[-1])}\b", text)
    if not shas:
        raise Refused(f"{BROKER_IMAGES} at {rev} names no 'Antares dev <sha> ({tags[-1]}' commit for its tag")
    return tags[-1], shas[-1]


def preflight(rev: str, names: list[str], env=os.environ, cwd: Path = ROOT) -> dict[str, str]:
    """What the planned stages need, checked before the first one; the extra env (ANTARES_DIR) they get."""
    problems, extra = [], {}
    if any(n in NEEDS_ANTARES for n in names):
        tag, sha = broker_commit(rev, cwd)
        where = env.get("ANTARES_DIR", "").strip()
        try:
            # Only a checkout root (.git dir or worktree file): a plain dir inside another repo would report that repo's HEAD.
            head = git("rev-parse", "HEAD", cwd=Path(where)) if where and (Path(where) / ".git").exists() else ""
        except (Refused, OSError):
            head = ""
        if not head.startswith(sha):
            problems.append(f"ANTARES_DIR={where or '(unset)'} is {head[:12] or 'no checkout'}, the candidate's broker "
                            f"{tag} is Antares {sha}: git -C <Antares clone> worktree add --detach <dir> {sha}")
        extra["ANTARES_DIR"] = where
    for name in names:
        missing = [k for k in NEEDS_ENV.get(name, ()) if not env.get(k, "").strip()]
        if missing:
            problems.append(f"{name} needs {', '.join(missing)} from the secrets mount")
    if problems:
        raise Refused("; ".join(problems))
    return extra


# ---------------------------------------------------------------------------- the hold on dev


def dev_commit() -> str:
    """The deployment commit dev's last green apply recorded (ConfigMap jc-release, scripts/release.py)."""
    done = subprocess.run(["kubectl", "-n", "dev", "get", "configmap", "jc-release", "-o",
                           "jsonpath={.data.commit}"], capture_output=True, text=True, check=False)
    sha = done.stdout.strip()
    if done.returncode or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise Refused(f"dev records no applied commit (ConfigMap dev/jc-release: {done.stderr.strip() or 'no commit'}): "
                      "finish `just dev-apply <rev>` first")
    return sha


def moved(cand: str, dev: str, measured, cwd: Path = ROOT) -> list[str]:
    """The measured components whose pins differ between the candidate and what dev runs."""
    if dev != cand and subprocess.run(["git", "cat-file", "-e", f"{dev}^{{commit}}"], cwd=cwd,
                                      capture_output=True, check=False).returncode:
        git("fetch", "-q", "origin", dev, cwd=cwd)
    a, b = pins(cand, cwd), pins(dev, cwd)
    names = set(a) | set(b) if "*" in measured else set(measured)
    return sorted(c for c in names if a.get(c) != b.get(c))


def holding(names) -> list[str]:
    """What the stages `names` (the running one and every later one) measure, together."""
    held = {c for n in names for c in MEASURES[n]}
    return ["*"] if "*" in held else sorted(held)


def write_hold(root: Path, sha: str, stage: str, components: list[str], seconds: float) -> None:
    until = datetime.fromtimestamp(time.time() + seconds + HOLD_SLACK, UTC).strftime("%FT%TZ")
    root.mkdir(parents=True, exist_ok=True)
    tmp = root / f"{HOLD}.{os.getpid()}"
    tmp.write_text(json.dumps({"sha": sha, "stage": stage, "components": components, "until": until,
                               "pid": os.getpid(), "host": os.uname().nodename}) + "\n")
    os.replace(tmp, root / HOLD)


def release_hold(root: Path, sha: str) -> None:
    path = root / HOLD
    try:
        mine = json.loads(path.read_text()).get("sha") == sha
    except (OSError, ValueError):
        return
    if mine:
        path.unlink(missing_ok=True)


def held(base: str, head: str, root: Path = RECORDS, cwd: Path = ROOT, now: float | None = None) -> list[str]:
    """The components base..head moves that a running gate holds; [] when the range may be applied."""
    try:
        hold = json.loads((root / HOLD).read_text())
    except FileNotFoundError:
        return []
    except (OSError, ValueError) as e:
        raise Refused(f"unreadable hold {root / HOLD}: {e}") from e
    until = datetime.strptime(str(hold.get("until", "")), "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=UTC).timestamp()
    if (time.time() if now is None else now) >= until:
        return []
    components = hold.get("components")
    if not isinstance(components, list):
        raise Refused(f"hold {root / HOLD} names no component list")
    return moved(base, head, components, cwd)


# ---------------------------------------------------------------------------- the Antares sweep


def gh(*args: str) -> str:
    """`gh` with the caller's GH_TOKEN; a failure refuses with gh's own message."""
    try:
        done = subprocess.run(["gh", *args], capture_output=True, text=True, check=False, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise Refused(f"gh {' '.join(args[:2])}: {e}") from e
    if done.returncode:
        raise Refused(f"gh {' '.join(args[:2])}: {done.stderr.strip()[:300]}")
    return done.stdout


def sweep_runs(sha: str, call=gh) -> list[dict]:
    """The sweep.yml runs of Antares commit `sha` (7-40 hex either side), newest first."""
    data = json.loads(call("api", f"repos/{ANTARES_REPO}/actions/workflows/{SWEEP}/runs?per_page=100"))
    runs = []
    for run in data.get("workflow_runs", []):
        named = run.get("display_title", "").removeprefix("sweep ")
        if re.fullmatch(r"[0-9a-f]{7,40}", named) and (named.startswith(sha) or sha.startswith(named)):
            runs.append(run)
    return sorted(runs, key=lambda r: r.get("created_at", ""), reverse=True)


def antares_sweep(rev: str, wait: bool, call=gh, sleep=time.sleep, poll: float = 60, cwd: Path = ROOT) -> int:
    """0 when the broker commit `rev` pins has a green sweep (printed as an `ok` case naming the run), 1 when its
    newest finished sweep is red. Without one it dispatches a sweep of that commit; `wait` polls until it ends."""
    _, sha = broker_commit(rev, cwd)
    dispatched = False
    while True:
        runs = sweep_runs(sha, call)
        green = next((r for r in runs if r.get("conclusion") == "success"), None)
        if green:
            print(f"  ok  sweep run {green['id']} of Antares {sha}: {green.get('html_url', '')}", flush=True)
            return 0
        running = [r for r in runs if r.get("status") != "completed"]
        red = next((r for r in runs if r.get("conclusion") in ("failure", "timed_out", "startup_failure")), None)
        if red and not running:
            print(f"  FAIL  sweep run {red['id']} of Antares {sha}: {red['conclusion']}, {red.get('html_url', '')}; "
                  f"fix it on Antares dev and pin the fix, or rerun a flake: gh run rerun {red['id']} -R {ANTARES_REPO}",
                  flush=True)
            return 1
        if not running and not dispatched:
            call("workflow", "run", SWEEP, "-R", ANTARES_REPO, "--ref", "dev", "-f", f"sha={sha}")
            dispatched = True
            print(f"antares-sweep: dispatched {SWEEP} on Antares {sha}", flush=True)
        if not wait:
            return 0
        sleep(poll)


# ---------------------------------------------------------------------------- the stages


def stages(args, out: Path) -> list[tuple[str, list[str], dict, Path]]:
    """(name, argv, extra env, cwd) per stage; each writes what it reads back into out/<name>."""
    conf, portal = args.conformance.resolve(), args.portal.resolve()
    std = conf / "tests/standards/run.sh"
    plan = [("dev-smoke", ["just", "dev-smoke-full"], {"JUST_NO_DOTENV": "true"}, ROOT)]
    for suite in SUITES:
        plan.append((suite, [str(std)], {"TARGET": "dev", "API": API, "SUITES": suite,
                                          "OUT": str(out / suite)}, conf))
    plan.append(("roundtrip", ["python3", str(conf / "tests/roundtrip/roundtrip.py"), "--endpoint",
                               f"{API}/api/endpoint/{write_endpoint()}", "--out", str(out / "roundtrip")],
                 {}, conf))
    plan.append(("dcat", [str(conf / "tests/prod-nightly/run.sh")],
                 {"API": API, "CATALOGUE": f"https://opendata.{DOMAIN}", "SUITES": "dcat",
                  "OUT": str(out / "dcat"), "PAUSE": "0"}, conf))
    plan.append(("portal-sweep", [str(portal / "ui/e2e/live/sweep.sh")],
                 {"PORTAL_URL": f"https://portal.{DOMAIN}", "SWEEP_OUT": str(out / "portal-sweep"),
                  "TASKS_DIR": "", "SWEEP_SKIP": SWEEP_SKIP, "SWEEP_WORKERS": str(SWEEP_WORKERS)}, portal / "ui"))
    load = out / "load"
    requests = args.requests or load / "requests.tsv"
    script = (f"set -euo pipefail; [ -s {quote(requests)} ] || python3 tests/k6/make_requests.py "
              f"https://opendata.{DOMAIN} > {quote(requests)}; exec tests/k6/run.sh")
    plan.append(("load", ["bash", "-c", script],
                 {"JC_K6_SCRIPT": "api-mix-100rps.js", "JC_K6_REQUESTS": str(requests),
                  "JC_K6_RATE": str(args.rate), "JC_K6_DURATION": args.duration, "JC_K6_P95_MS": "500",
                  "JC_REPORTS_DIR": str(load)}, conf))
    plan.append(("antares-sweep", [sys.executable, str(Path(__file__).resolve()), "antares-sweep",
                                   getattr(args, "rev", "HEAD")], {}, ROOT))
    return [s for s in plan if not args.only or s[0] in args.only]


def sweep_cap(tests: int, workers: int = SWEEP_WORKERS) -> int:
    """Seconds the portal-sweep stage gets for `tests` tests on `workers` workers: the measured time each,
    spread over the workers, 25 % room left."""
    return round(tests * SWEEP_TEST_S / max(workers, 1) / SWEEP_ROOM)


def sweep_workers(argv: list[str]) -> int:
    """How many workers the sweep stage `argv` runs on: SWEEP_WORKERS when its sweep.sh reads it, else one."""
    try:
        return SWEEP_WORKERS if "SWEEP_WORKERS" in Path(argv[0]).read_text() else 1
    except OSError:
        return 1


def sweep_tests(argv: list[str], env: dict, cwd: Path, log: Path) -> int | None:
    """How many tests the sweep stage `argv` runs: sweep.sh with SWEEP_LIST=1 prints Playwright's list into
    `log`, ending "Total: N tests in M files". None when it cannot tell; a sweep.sh without SWEEP_LIST would
    run the whole sweep, so it is not started at all."""
    try:
        if "SWEEP_LIST" not in Path(argv[0]).read_text():
            return None
    except OSError:
        return None
    log.parent.mkdir(parents=True, exist_ok=True)
    code, _ = execute(argv, cwd, {**os.environ, **env, "SWEEP_LIST": "1"}, log, 120)
    found = re.findall(r"^Total: (\d+) tests? in \d+ files?$", log.read_text(errors="replace"), re.MULTILINE)
    return int(found[-1]) if code == 0 and found else None


def sweep_stage_cap(argv: list[str], env: dict, cwd: Path, log: Path) -> int:
    """The portal-sweep stage's cap from its counted tests, SWEEP_TESTS when the count fails; printed."""
    tests = sweep_tests(argv, env, cwd, log)
    workers = sweep_workers(argv)
    cap = sweep_cap(tests or SWEEP_TESTS, workers)
    print(f"release gate: portal-sweep lists {tests if tests else f'no count (taking {SWEEP_TESTS})'} tests: "
          f"{cap} s at {SWEEP_TEST_S} s each on {workers} worker{'s' if workers > 1 else ''} with 25 % room", flush=True)
    return cap


def write_endpoint() -> str:
    """The roundtrip stage's Endpoint slug: WRITE_ENDPOINT, else the seeded conformance Endpoint's."""
    slug = os.environ.get("WRITE_ENDPOINT", "").strip()
    if not slug:
        m = re.search(r"^\s+slug:\s*([a-z2-7]+)\s*$", CONFORMANCE_ENDPOINT.read_text(), re.MULTILINE)
        slug = m.group(1) if m else ""
    if not re.fullmatch(r"[a-z0-9-]{1,64}", slug):
        raise Refused(f"roundtrip: {slug!r} is not an Endpoint slug (WRITE_ENDPOINT or {CONFORMANCE_ENDPOINT.name})")
    return slug


def quote(path) -> str:
    return "'" + str(path).replace("'", "'\\''") + "'"


def known_red(path: Path = KNOWN_RED) -> list[tuple[str, re.Pattern]]:
    """(task id, pattern) per line of the known-red file; a malformed line refuses the whole run."""
    out = []
    for n, line in enumerate(path.read_text().splitlines(), 1) if path.exists() else []:
        if not line.strip() or line.startswith("#"):
            continue
        task, _, pattern = line.partition(" ")
        if not TASK_ID.fullmatch(task) or not pattern.strip():
            raise Refused(f"{path.name}:{n}: want '<task id> <regex>', got {line!r}")
        try:
            out.append((task, re.compile(pattern.strip())))
        except re.error as e:
            raise Refused(f"{path.name}:{n}: {e}") from e
    return out


def parked(text: str, known: list[tuple[str, re.Pattern]]) -> str | None:
    """The task that parks this red case, if a known-red line matches it."""
    return next((task for task, pattern in known if pattern.fullmatch(text)), None)


def cases(name: str, folder: Path, log: str, details: dict[str, str] | None = None) -> dict[str, str]:
    """Every case a stage left behind, `<stage>::<case>` -> pass | fail; `details` gets each smoke line's text."""
    found: dict[str, str] = {}
    details = {} if details is None else details
    for xml in sorted(folder.rglob("*.xml")):
        try:
            tree = ET.parse(xml)
        except ET.ParseError:
            continue
        for case in tree.iter("testcase"):
            if case.find("skipped") is not None:
                continue
            red = case.find("failure") is not None or case.find("error") is not None
            key = f"{name}::{xml.relative_to(folder)}::{case.get('classname', '')}::{case.get('name', '')}"
            found[key] = "fail" if red else "pass"
    tsv = folder / "results.tsv"
    if tsv.exists():
        for line in tsv.read_text().splitlines()[1:]:
            cols = line.split("\t")
            if len(cols) < 3 or cols[2] not in ("pass", "fail"):
                continue
            # A red suite row whose own junit lists its red cases is judged by those cases alone.
            inside = f"{name}::{cols[0]}/{cols[1]}/"
            if cols[2] == "fail" and any(k.startswith(inside) and v == "fail" for k, v in found.items()):
                continue
            found[f"{name}::{cols[0]}/{cols[1]}"] = cols[2]
    summary = folder / "summary.json"
    if summary.exists():
        data = json.loads(summary.read_text())
        for result in data.get("results", []) if isinstance(data, dict) else []:
            if result.get("verdict") in ("pass", "fail", "error"):
                found[f"{name}::{result['key']}"] = "pass" if result["verdict"] == "pass" else "fail"
    for m in re.finditer(r"^  (ok|FAIL)  +([^:\n]+)(?::[ \t]*(.*))?", log, re.MULTILINE):
        key = f"{name}::{m.group(2).strip()}"
        found[key] = "pass" if m.group(1) == "ok" else "fail"
        details[key] = f"{key}: {m.group(3)}" if m.group(3) else key
    return found


def load_verdict(folder: Path) -> list[str]:
    """What the k6 summary breaks of the gate's own rule: 0 failed requests, p95 < 500 ms."""
    path = folder / "summary.json"
    if not path.exists():
        return ["k6 wrote no summary.json"]
    metrics = json.loads(path.read_text()).get("metrics", {})
    failed = metrics.get("http_req_failed", {}).get("values", {}).get("passes")
    p95 = metrics.get("http_req_duration", {}).get("values", {}).get("p(95)")
    problems = []
    if failed != 0:
        problems.append(f"{failed} failed requests (the gate allows 0)")
    if p95 is None or p95 >= 500:
        problems.append(f"p95 {p95} ms (the gate allows < 500)")
    return problems


def execute(argv, cwd, env, log: Path, timeout: float) -> tuple[int, str]:
    """Run argv into log; past timeout kill its whole process group (npx and Chromium hold the pipes)."""
    with log.open("w") as sink:
        try:
            proc = subprocess.Popen(argv, cwd=cwd, env=env, stdout=sink, stderr=subprocess.STDOUT,
                                    start_new_session=True)
        except OSError as e:
            return 127, f"did not run: {e}"
        try:
            return proc.wait(timeout=timeout), ""
        except subprocess.TimeoutExpired:
            for sig, grace in ((signal.SIGTERM, 30), (signal.SIGKILL, None)):
                try:
                    os.killpg(proc.pid, sig)
                    proc.wait(timeout=grace)
                    break
                except (ProcessLookupError, subprocess.TimeoutExpired):
                    continue
            return 124, f"timed out after {int(timeout)} s (its stage cap); stopped"


def run_stage(name, argv, env, cwd, out: Path, known: list | None = None, timeout: float = 3600) -> dict:
    folder = out / name
    folder.mkdir(parents=True, exist_ok=True)
    started = datetime.now(UTC)
    code, why = execute(argv, cwd, {**os.environ, **env}, folder / "output.txt", timeout)
    if why:
        with (folder / "output.txt").open("a") as sink:
            sink.write(f"\n{name} {why}\n")
    log = (folder / "output.txt").read_text(errors="replace")
    details: dict[str, str] = {}
    found = cases(name, folder, log, details)
    red = {k: details.get(k, k) for k, v in found.items() if v == "fail"}
    excused = {k: parked(text, known or []) for k, text in red.items()}
    problems = [] if code == 0 else [f"exit {code}: {tail(log)}"]
    # A non-zero exit is forgiven only when it shows its reds and a task owns each one.
    explained = (bool(red) and all(excused.values()) and name != "load" and code != 124
                 and "Traceback (most recent" not in log)
    if explained:
        problems = []
    if name == "load" and code == 0:
        problems += load_verdict(folder)
    found[name] = "fail" if problems else "pass"
    return {"name": name, "verdict": found[name], "problems": problems, "cases": found,
            "known_red": {k: f"{t}: {red[k]}" for k, t in excused.items() if t},
            "seconds": int((datetime.now(UTC) - started).total_seconds())}


def tail(log: str) -> str:
    lines = [line for line in log.strip().splitlines() if line.strip()]
    return " | ".join(lines[-3:])[:400]


# ---------------------------------------------------------------------------- records


def records(root: Path = RECORDS) -> list[dict]:
    out = []
    for path in root.glob("*/record.json"):
        try:
            out.append(json.loads(path.read_text()))
        except (OSError, ValueError):
            continue
    return sorted(out, key=lambda r: r.get("finished", ""))


def previous_green(root: Path, sha: str) -> dict | None:
    """The newest green record of another candidate: the baseline a regression is measured on."""
    greens = [r for r in records(root) if r.get("green") and r.get("sha") != sha]
    return greens[-1] if greens else None


def regressions(before: dict | None, now: dict[str, str], parked_now: frozenset = frozenset()) -> list[str]:
    if not before:
        return []
    return sorted(k for k, v in before.get("cases", {}).items()
                  if v == "pass" and now.get(k) == "fail" and k not in parked_now)


def verdict(stage_results: list[dict], regressed: list[str]) -> bool:
    ran = {s["name"]: s["verdict"] for s in stage_results}
    return all(ran.get(name) == "pass" for name in REQUIRED) and not regressed


def report(rec: dict) -> str:
    head = "GREEN" if rec["green"] else "RED"
    lines = [f"# Release gate {rec['sha'][:12]}: {head}", "",
             (f"Deployment {rec['sha']} on {DOMAIN}, {rec['started']} to {rec['finished']}; "
             f"load {rec['rate']}/s for {rec['duration']}; baseline {rec.get('baseline') or 'none'}."), "",
             "| Stage | Verdict | Cases | Seconds | Problems |", "|---|---|---|---|---|"]
    for s in rec["stages"]:
        reds = sum(1 for v in s["cases"].values() if v == "fail")
        lines.append(f"| {s['name']} | {s['verdict']} | {len(s['cases'])} ({reds} red) | {s['seconds']} | "
                     f"{'; '.join(s['problems']).replace('|', '/') or ''} |")
    missing = [n for n in REQUIRED if n not in {s["name"] for s in rec["stages"]}]
    if missing:
        lines += ["", f"Not run (a partial record never opens prod): {', '.join(missing)}"]
    held = {k: v for s in rec["stages"] for k, v in s.get("known_red", {}).items()}
    lines += ["", f"## Known reds, each owned by a task ({len(held)})", ""]
    lines += [f"- {v.replace('|', '/')}" for _, v in sorted(held.items())] or ["none"]
    if rec.get("unmatched"):
        lines += ["", "Known-red lines no case matched (fixed? remove them in their task):", ""]
        lines += [f"- `{u}`" for u in rec["unmatched"]]
    lines += ["", f"## Regressions against the previous green record ({len(rec['regressions'])})", ""]
    lines += [f"- {k}" for k in rec["regressions"]] or ["none"]
    lines += ["", "## Image pins", ""] + [f"- `{i}`" for i in rec["images"]]
    return "\n".join(lines) + "\n"


def junit(rec: dict) -> str:
    suites = ET.Element("testsuites", name=f"release-gate {rec['sha'][:12]}")
    for s in rec["stages"]:
        suite = ET.SubElement(suites, "testsuite", name=s["name"], tests=str(len(s["cases"])),
                              failures=str(sum(v == "fail" for v in s["cases"].values())))
        for key, v in sorted(s["cases"].items()):
            case = ET.SubElement(suite, "testcase", classname=s["name"], name=key)
            if v == "fail":
                why = s.get("known_red", {}).get(key)
                ET.SubElement(case, "failure", message="; ".join(s["problems"]) if key == s["name"]
                              else f"known red, owned by {why.split(':', 1)[0]}" if why else "red")
    return ET.tostring(suites, encoding="unicode")


def write(rec: dict, root: Path) -> Path:
    folder = root / rec["sha"]
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "record.json").write_text(json.dumps(rec, indent=1, sort_keys=True) + "\n")
    (folder / "report.md").write_text(report(rec))
    (folder / "junit.xml").write_text(junit(rec))
    return folder


def run(args) -> int:
    cand = candidate(args.rev)
    root = args.records
    out = root / cand["sha"] / "stages"
    started = datetime.now(UTC).strftime("%FT%TZ")
    known = known_red()
    plan = stages(args, out)
    extra = preflight(args.rev, [s[0] for s in plan])
    # the candidate is what dev runs, or the record would name images no stage measured
    other = moved(cand["sha"], dev_commit(), ("*",))
    if other:
        raise Refused(f"dev runs other pins than {cand['sha'][:12]} for {', '.join(other)}: "
                      f"`just dev-apply {cand['sha'][:12]}` first")
    if any(s[0] == "antares-sweep" for s in plan):
        # started now so it runs beside the other stages; the stage itself waits for it
        try:
            antares_sweep(args.rev, wait=False)
        except Refused as e:
            print(f"release gate: antares-sweep not started: {e}", flush=True)
    results: list[dict] = []
    try:
        for i, (name, argv, env, cwd) in enumerate(plan):
            print(f"release gate: {name} ...", flush=True)
            stage_env = {**env, **{k: v for k, v in extra.items() if name in NEEDS_ANTARES}}
            timeout = args.stage_timeout
            if name == "portal-sweep":
                timeout = args.sweep_timeout or sweep_stage_cap(argv, env, cwd, out / name / "list.txt")
            write_hold(root, cand["sha"], name, holding(s[0] for s in plan[i:]), timeout)
            before = dev_reading(cand["sha"], name)
            result = run_stage(name, argv, stage_env, cwd, out, known, timeout)
            ran_on = [before, dev_reading(cand["sha"], name)]
            result["dev"] = [r[0] for r in ran_on]
            result["problems"] += [p for _, p in ran_on if p and p not in result["problems"]]
            if result["problems"]:
                result["verdict"] = result["cases"][name] = "fail"
            results.append(result)
            print(f"release gate: {name} {result['verdict']} {'; '.join(result['problems'])}", flush=True)
            rec = record_of(cand, results, known, root, started, args)
            folder = write(rec, root)
    finally:
        release_hold(root, cand["sha"])
    print(f"release gate {cand['sha'][:12]}: {'GREEN' if rec['green'] else 'RED'}; report {folder / 'report.md'}")
    return 0 if rec["green"] else 1


def dev_reading(sha: str, stage: str) -> tuple[str | None, str]:
    """(dev's commit, a problem when it ran other pins of what `stage` measures, or was unreadable)."""
    try:
        dev = dev_commit()
        other = moved(sha, dev, MEASURES[stage])
    except Refused as e:
        return None, str(e)
    return dev, "" if not other else f"dev ran other pins of {', '.join(other)} (deployment {dev[:12]}) under this stage"


def record_of(cand: dict, results: list[dict], known: list, root: Path, started: str, args) -> dict:
    """The record so far: a run stopped between stages leaves it RED with the missing stages listed."""
    every = {k: v for s in results for k, v in s["cases"].items()}
    base = previous_green(root, cand["sha"])
    held = {k: v for s in results for k, v in s["known_red"].items()}
    regressed = regressions(base, every, frozenset(held))
    texts = [v.split(": ", 1)[1] for v in held.values()]
    unmatched = [f"{t} {p.pattern}" for t, p in known if not any(p.fullmatch(x) for x in texts)]
    return {**cand, "unmatched": unmatched, "started": started, "finished": datetime.now(UTC).strftime("%FT%TZ"),
            "rate": args.rate, "duration": args.duration, "baseline": base and base["sha"], "stages": results,
            "cases": every, "regressions": regressed, "green": verdict(results, regressed)}


# ---------------------------------------------------------------------------- check


def check(rev: str = "HEAD", root: Path = RECORDS, cwd: Path = ROOT) -> dict:
    """The green record that covers `rev`, or Refused naming why none does."""
    cand = candidate(rev, cwd)
    seen = []
    for rec in reversed(records(root)):
        if not rec.get("green"):
            continue
        if rec.get("images") != cand["images"]:
            seen.append(f"{rec['sha'][:12]} pins other images")
            continue
        if rec["sha"] == cand["sha"]:
            return rec
        known = subprocess.run(["git", "merge-base", "--is-ancestor", rec["sha"], cand["sha"]], cwd=cwd,
                               capture_output=True, check=False).returncode == 0
        if not known:
            seen.append(f"{rec['sha'][:12]} is not an ancestor")
            continue
        changed = git("diff", "--name-only", rec["sha"], cand["sha"], cwd=cwd).splitlines()
        other = [p for p in changed if not RELEASE_FILES.match(p)]
        if not other:
            return rec
        seen.append(f"{rec['sha'][:12]} differs in {', '.join(other[:5])}")
    why = "; ".join(seen[:5]) or f"no green record in {root}"
    raise Refused(f"{cand['sha'][:12]} has no green release gate record ({why}): run "
                  f"`just release-gate {cand['sha'][:12]}` on dev after dev-apply of that commit")


# ---------------------------------------------------------------------------- self-test


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo, root = Path(tmp) / "repo", Path(tmp) / "records"
        (repo / "components/portal").mkdir(parents=True)
        pin = "image: {repository: ghcr.io/x/portal, tag: a, digest: sha256:%s}\n"

        def commit(msg, files):
            for path, text in files.items():
                (repo / path).parent.mkdir(parents=True, exist_ok=True)
                (repo / path).write_text(text)
            git("add", "-A", cwd=repo)
            git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", msg, cwd=repo)
            return git("rev-parse", "HEAD", cwd=repo)

        git("init", "-q", cwd=repo)
        main = commit("main", {"components/portal/images.yaml": pin % ("1" * 64)})
        stage = {"name": "x", "verdict": "pass", "problems": [], "cases": {}, "seconds": 0}
        green = [{**stage, "name": n, "cases": {n: "pass"}} for n in REQUIRED]

        def record(sha, ok, cases=None, finished="2026-10-28T00:00:00Z"):
            rec = {**candidate(sha, repo), "green": ok, "finished": finished, "cases": cases or {}}
            (root / sha).mkdir(parents=True, exist_ok=True)
            (root / sha / "record.json").write_text(json.dumps(rec))

        def refused(rev):
            try:
                check(rev, root, repo)
            except Refused as e:
                return str(e)
            return ""

        assert "no green record" in refused(main), "no record must refuse"
        # a red suite blocks the verdict, and so does a stage that never ran
        assert verdict(green, [])
        assert not verdict([*green[:-1], {**green[-1], "verdict": "fail"}], [])
        assert not verdict(green[1:], [])
        # a case green in the previous record and red now blocks
        assert regressions({"cases": {"a": "pass", "b": "fail"}}, {"a": "fail", "b": "fail"}) == ["a"]
        assert not verdict(green, ["a"])
        record(main, False)
        assert refused(main), "a red record must refuse"
        record(main, True)
        assert check(main, root, repo)["sha"] == main
        # the release commit `cut` writes on top of the gated main commit is covered
        tag = commit("release v1", {"releases/v1.md": "notes\n", "CHANGELOG.md": "log\n"})
        assert check(tag, root, repo)["sha"] == main
        # a stale record for another sha blocks: other code, or other image pins
        code = commit("code", {"charts/x.yaml": "a: 1\n"})
        assert "differs in charts/x.yaml" in refused(code)
        pinned = commit("pin", {"components/portal/images.yaml": pin % ("2" * 64)})
        assert "pins other images" in refused(pinned)
        # the stage readers: junit, results.tsv, sweep summary, smoke lines, k6 summary
        folder = Path(tmp) / "stage"
        folder.mkdir()
        (folder / "r.xml").write_text('<testsuite><testcase classname="c" name="ok"/>'
                                      '<testcase classname="c" name="bad"><failure/></testcase>'
                                      '<testcase classname="c" name="skip"><skipped/></testcase></testsuite>')
        (folder / "results.tsv").write_text("endpoint\tsuite\tverdict\tdetail\tlog\ne1\togc\tfail\tx\ty\n")
        (folder / "summary.json").write_text(json.dumps({"results": [{"key": "a.spec.ts › t", "verdict": "pass"}],
                                                          "metrics": {}}))
        got = cases("s", folder, "  ok    portal: 200\n  FAIL  mqtt: refused\n")
        assert got == {"s::r.xml::c::ok": "pass", "s::r.xml::c::bad": "fail", "s::e1/ogc": "fail",
                       "s::a.spec.ts › t": "pass", "s::portal": "pass", "s::mqtt": "fail"}, got
        (folder / "summary.json").write_text(json.dumps({"metrics": {
            "http_req_failed": {"values": {"passes": 3}}, "http_req_duration": {"values": {"p(95)": 120}}}}))
        # a red suite row whose junit lists its red cases is judged by those; one with none stays red itself
        (folder / "e2" / "ws").mkdir(parents=True)
        (folder / "e2" / "ws" / "p.xml").write_text('<testsuite><testcase classname="c" name="bad"><failure/></testcase></testsuite>')
        (folder / "results.tsv").write_text("endpoint\tsuite\tverdict\tdetail\tlog\ne1\togc\tfail\tx\ty\ne2\tws\tfail\tx\ty\n")
        got = cases("s", folder, "")
        assert got["s::e1/ogc"] == "fail" and "s::e2/ws" not in got and got["s::e2/ws/p.xml::c::bad"] == "fail", got
        assert load_verdict(folder) == ["3 failed requests (the gate allows 0)"]
        # a stage past --stage-timeout is stopped with its children and stays red, whatever it printed
        slow = run_stage("s", ["bash", "-c", "printf '  FAIL  catalogue (no dataset) (ab): x\\n'; sleep 60 & wait"],
                         {}, tmp, Path(tmp) / "t", [("T-0001", re.compile(r"s::catalogue .*"))], timeout=1)
        assert slow["verdict"] == "fail" and "timed out after 1 s" in slow["problems"][0], slow
        # preflight: the broker's Antares commit, ANTARES_DIR at it, the env each stage needs
        commit("broker", {BROKER_IMAGES: "x:\n  # Antares dev aaaaaaa (dev-9, ...)\n  # Antares dev "
                          + git("rev-parse", "--short=7", "HEAD", cwd=repo) + " (dev-91, over aaaaaaa)\n    tag: 'dev-91'\n"})
        tag, sha = broker_commit("HEAD", repo)
        assert tag == "dev-91" and git("rev-parse", "HEAD~1", cwd=repo).startswith(sha), (tag, sha)
        git("checkout", "-q", "HEAD~1", cwd=repo)
        good = {"ANTARES_DIR": str(repo), "NGSILD_TOKEN": "t"}
        try:
            preflight("HEAD@{1}", ["ogc"], good, repo)
        except Refused as e:
            raise AssertionError(f"ANTARES_DIR at the pinned commit must pass: {e}") from e
        for env, names, want in (({}, ["odata"], "ANTARES_DIR=(unset)"), ({"ANTARES_DIR": tmp}, ["dcat"], "no checkout"),
                                 (good, ["portal-sweep"], "PORTAL_PASSWORD"), ({}, ["roundtrip"], "NGSILD_TOKEN")):
            try:
                preflight("HEAD@{1}", names, env, repo)
            except Refused as e:
                assert want in str(e), (want, str(e))
                continue
            raise AssertionError(f"preflight must refuse {names} with {env}")
        assert preflight("HEAD@{1}", ["sta", "ws"], {}, repo) == {}
        assert run_stage("load", ["false"], {}, tmp, Path(tmp) / "out")["verdict"] == "fail"
        # a stage red only on known reds passes; an unknown red, a silent exit or a traceback stays red
        listed = Path(tmp) / "known.txt"
        listed.write_text("# parked\nT-0001 s::catalogue \\(no dataset\\) \\(ab\\): .*\n")
        known = known_red(listed)

        def smoke(text):
            return run_stage("s", ["bash", "-c", f"printf %s {quote(text)}; exit 1"], {}, tmp, Path(tmp) / "o", known)

        held = smoke("  ok    portal: 200\n  FAIL  catalogue (no dataset) (ab): no dataset\n")
        assert held["verdict"] == "pass" and held["known_red"] == {
            "s::catalogue (no dataset) (ab)": "T-0001: s::catalogue (no dataset) (ab): no dataset"}, held
        assert smoke("  FAIL  catalogue (no dataset) (cd): no dataset\n")["verdict"] == "fail"
        assert smoke("  FAIL  catalogue (no dataset) (ab): x\n  FAIL  mqtt: refused\n")["verdict"] == "fail"
        assert smoke("  ok    portal: 200\n")["verdict"] == "fail"
        assert smoke("  FAIL  catalogue (no dataset) (ab): x\nTraceback (most recent call last):\n")["verdict"] == "fail"
        assert regressions({"cases": {"a": "pass"}}, {"a": "fail"}, frozenset({"a"})) == []
        for bad in ("catalogue .*\n", "T-1 x\n", "T-0002 (\n"):
            listed.write_text(bad)
            try:
                known_red(listed)
            except Refused:
                continue
            raise AssertionError(f"a malformed known-red line must refuse: {bad!r}")
        assert known_red(), "the committed known-red file parses"
    print("standards-gate self-test: ok")


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    if argv == ["--self-test"]:
        self_test()
        return 0
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="command", required=True)
    r = sub.add_parser("run")
    r.add_argument("rev", nargs="?", default="HEAD")
    # The floor one source may send through the edge (T-5900): class 3 admits 10 requests a second
    # per client address and bearer, and the gate runs from one address, so 1000/s measured only the
    # limiter (99.5 % 429, b629a881). Capacity itself is the bench rig's table in T-5900.
    r.add_argument("--rate", type=int, default=8, help="k6 arrival rate per second (default 8)")
    r.add_argument("--duration", default="5m", help="k6 hold time (default 5m)")
    r.add_argument("--requests", type=Path, help="k6 request list (default: make_requests.py of dev's CKAN)")
    r.add_argument("--only", nargs="+", choices=REQUIRED, help="a partial run, never green for prod")
    r.add_argument("--conformance", type=Path, default=ROOT.parent / "joinedcontext-conformance")
    r.add_argument("--portal", type=Path, default=ROOT.parent / "joinedcontext-portal")
    r.add_argument("--records", type=Path, default=RECORDS)
    r.add_argument("--stage-timeout", type=float, default=3600, help="seconds per stage before it is stopped red")
    # Without it the cap follows the sweep's size: its counted tests x SWEEP_TEST_S / SWEEP_ROOM (T-6354).
    r.add_argument("--sweep-timeout", type=float, help="seconds for the portal-sweep stage "
                   f"(default: its listed tests x {SWEEP_TEST_S} s with 25 %% room)")
    h = sub.add_parser("held", help="exit 1 when a running gate holds a component base..head moves")
    h.add_argument("base")
    h.add_argument("head")
    h.add_argument("--records", type=Path, default=RECORDS)
    w = sub.add_parser("antares-sweep", help="the stage: a green Antares sweep of the pinned broker commit")
    w.add_argument("rev", nargs="?", default="HEAD")
    c = sub.add_parser("check")
    c.add_argument("rev", nargs="?", default="HEAD")
    c.add_argument("--records", type=Path, default=RECORDS)
    args = p.parse_args(argv)
    try:
        if args.command == "run":
            return run(args)
        if args.command == "held":
            busy = held(args.base, args.head, args.records)
            hold = json.loads((args.records / HOLD).read_text()) if busy else {}
            print(f"held by the release gate of {hold.get('sha', '')[:12]} ({hold.get('stage')}, until "
                  f"{hold.get('until')}): {', '.join(busy)}" if busy else "free")
            return 1 if busy else 0
        if args.command == "antares-sweep":
            return antares_sweep(args.rev, wait=True)
        rec = check(args.rev, args.records)
        print(f"release gate: green record {rec['sha'][:12]} of {rec['finished']} covers {args.rev}")
        return 0
    except Refused as e:
        print(f"refusing: {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
