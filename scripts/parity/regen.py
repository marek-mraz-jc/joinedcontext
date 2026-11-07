#!/usr/bin/env python3
"""Bloblang parity of ekiben with Bento 1.22 over the joinedcontext-deployment seed pipelines.

    regen.py capture <deployment clone> <capture dir>   run the deployment's Bento tests through
                                                         docker-shim.py; every Bento run becomes a case
    regen.py build <deployment clone> <capture dir>     normalize the cases into tests/parity/, rerun
                                                         each in Bento (determinism) and ekiben, and
                                                         write tests/parity/PARITY.md
    regen.py check                                       rerun the committed cases in ekiben only

Inputs are what the deployment tests already feed the seed mappings (their tests/fixtures and inline
documents). Each case keeps Bento's stdout beside its input, so crates/ekiben-server/tests/parity.rs
checks ekiben without the Bento binary. Needs .bento/bento (scripts/reference-bento.sh) for build.
"""
import base64, concurrent.futures, gzip, hashlib, json, os, re, subprocess, sys, tempfile
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PARITY = REPO / "tests/parity"
BLOBS = PARITY / "blobs"
BENTO = REPO / ".bento/bento"
SHIM = Path(__file__).resolve().parent / "docker-shim.py"
SEEDS = "components/context-gateway/seed"
BLOBLANG_KEYS = {"mapping", "mutation", "bloblang", "check", "request_map", "result_map"}
LOCAL_HTTP = re.compile(r"https?://(127\.0\.0\.1|localhost|0\.0\.0\.0)[:/]")
TIMEOUT = 180
# Components another task builds (lint parity, 2026-11-07): their cases wait for it.
OWNED_ELSEWHERE = {"parquet_decode": "T-6625", "protobuf": "T-6625"}


def ekiben_bin() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO / "target"))
    return target / "debug/ekiben"


def put_blob(data: bytes) -> str:
    sha = hashlib.sha256(data).hexdigest()[:20]
    path = BLOBS / f"{sha}.gz"
    if not path.exists():
        path.write_bytes(gzip.compress(data, 9, mtime=0))
    return sha


def get_blob(sha: str) -> bytes:
    return gzip.decompress((BLOBS / f"{sha}.gz").read_bytes())


def bloblang_texts(node, path=""):
    """Every Bloblang text of a config: processor mappings, conditions and `${! }` fields."""
    if isinstance(node, dict):
        for key, value in node.items():
            sub = f"{path}.{key}" if path else key
            if isinstance(value, str) and (key in BLOBLANG_KEYS or "${!" in value):
                yield sub, value
            else:
                yield from bloblang_texts(value, sub)
    elif isinstance(node, list):
        for i, value in enumerate(node):
            if isinstance(value, str) and "${!" in value:
                yield f"{path}[{i}]", value
            else:
                yield from bloblang_texts(value, f"{path}[{i}]")


def key(text: str) -> str:
    return hashlib.sha256(text.strip().encode()).hexdigest()


# ---------------------------------------------------------------------------------------- capture
def capture(deployment: Path, out: Path):
    tests = sorted(t for t in (deployment / "tests").glob("test_*.py")
                   if re.search(r"open_data|BENTO|warpstreamlabs/bento|bento\(|run_all|blobl", t.read_text()))
    shimbin = Path(tempfile.mkdtemp(prefix="parity-shim-"))
    (shimbin / "docker").symlink_to(SHIM)
    env = {**os.environ, "PATH": f"{shimbin}:{os.environ['PATH']}", "PARITY_BENTO": str(BENTO),
           "PARITY_CAPTURE": str(out)}
    subprocess.run(["pytest", "-q", "-n", "4", "-p", "no:cacheprovider", *map(str, tests)],
                   cwd=deployment, env=env)


# ------------------------------------------------------------------------------------------ build
def normalize(raw: dict) -> dict | None:
    """One recorded Bento run as a replayable case, or None for a run that is no single stream."""
    cmd, tmp = raw["cmd"], raw["tmp"]
    case = {"tests": [raw["test"].split(" (")[0]], "env": raw["env"],
            "stdin": put_blob(base64.b64decode(raw["stdin"]))}
    if cmd[0] == "sh":
        if "exec " not in cmd[2] or " -c " not in cmd[2] or "streams" in cmd[2]:
            return {**case, "mode": "other", "why": f"not a one-stream run: {cmd[2][:80]}"}
        config_path = f"{tmp}/c.json"
        env = dict(raw["env"])
        env.pop("CONFIG", None)
        case["env"] = env
    elif cmd[1] == "blobl":
        case.update(mode="blobl", args=cmd[2:])
        return case
    elif cmd[1] == "-c" and len(cmd) == 3:
        config_path = cmd[2]
    else:
        return {**case, "mode": "other", "why": f"not a one-stream run: {' '.join(cmd[1:])[:80]}"}
    config = base64.b64decode(raw["files"][config_path]).decode()
    case.update(mode="run", config=put_blob(config.encode()))
    if "now()" in config and raw.get("clock"):
        # Its answer moves with the wall clock (praha drops readings older than 3 h): ekiben
        # replays it on the second Bento ran it.
        case["clock"] = raw["clock"]
    if LOCAL_HTTP.search(config):
        case["why"] = "calls the test's own HTTP stub on 127.0.0.1, which the case does not record"
    return case


def run_case(case: dict, binary: Path, bento: bool) -> tuple[int, bytes, str]:
    with tempfile.TemporaryDirectory(prefix="parity-") as root:
        env = {"PATH": os.environ["PATH"], "HOME": root, **case["env"]}
        if case["mode"] == "blobl":
            cmd = [str(binary), "blobl", *case["args"]]
        else:
            config = Path(root, "c.json")
            config.write_bytes(get_blob(case["config"]))
            cmd = [str(binary), "-c", str(config)] if bento else [str(binary), "run", str(config)]
            if not bento and case.get("clock"):
                cmd += ["--clock", case["clock"]]
        try:
            p = subprocess.run(cmd, input=get_blob(case["stdin"]), capture_output=True, env=env,
                               timeout=TIMEOUT, cwd=root)
        except subprocess.TimeoutExpired:
            return -1, b"", "timeout"
        return p.returncode, p.stdout, p.stderr.decode(errors="replace")[-2000:]


def build(deployment: Path, capture_dir: Path):
    BLOBS.mkdir(parents=True, exist_ok=True)
    old_blobs = {p.name for p in BLOBS.glob("*.gz")}
    sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=deployment, capture_output=True,
                         text=True).stdout.strip()
    cases: dict[str, dict] = {}
    for f in sorted(capture_dir.glob("*.json")):
        raw = json.loads(f.read_text())
        case = normalize(raw)
        if case is None:
            continue
        case["bento"] = {"code": raw["code"], "stdout": put_blob(base64.b64decode(raw["stdout"]))}
        ident = hashlib.sha256(json.dumps({k: case.get(k) for k in ("mode", "args", "config", "env", "stdin")},
                                          sort_keys=True).encode()).hexdigest()[:12]
        if ident in cases:
            cases[ident]["tests"] = sorted(set(cases[ident]["tests"] + case["tests"]))
        else:
            cases[ident] = {"id": ident, **case}

    runnable = [c for c in cases.values() if c["mode"] != "other" and "why" not in c]
    with concurrent.futures.ThreadPoolExecutor(6) as pool:
        again = dict(zip([c["id"] for c in runnable],
                         pool.map(lambda c: run_case(c, BENTO, True), runnable)))
    for c in runnable:
        code, out, err = again[c["id"]]
        if code != c["bento"]["code"] or out != get_blob(c["bento"]["stdout"]):
            c["why"] = "Bento itself answers differently on a second run (clock, random or order)"
    check(list(cases.values()), write=True)

    used = {c["stdin"] for c in cases.values()} | {c["config"] for c in cases.values() if "config" in c} \
        | {c["bento"]["stdout"] for c in cases.values()}
    for name in old_blobs | {p.name for p in BLOBS.glob("*.gz")}:
        if name[:-3] not in used and (BLOBS / name).exists():
            (BLOBS / name).unlink()
    (PARITY / "cases.json").write_text(json.dumps(
        {"deployment": sha, "bento": "1.22.0", "cases": sorted(cases.values(), key=lambda c: c["id"])},
        indent=1, sort_keys=True) + "\n")
    table(deployment, sha, cases)


def check(cases: list[dict], write: bool = False) -> int:
    binary = ekiben_bin()
    runnable = [c for c in cases if c["mode"] != "other" and "why" not in c]
    with concurrent.futures.ThreadPoolExecutor(6) as pool:
        results = list(pool.map(lambda c: run_case(c, binary, False), runnable))
    bad = 0
    for c, (code, out, err) in zip(runnable, results):
        same = code == c["bento"]["code"] and out == get_blob(c["bento"]["stdout"])
        missing = re.search(r"type '(\w+)' was not recognised as a processor", err)
        if write and not same and missing and missing.group(1) in OWNED_ELSEWHERE:
            c["why"] = f"ekiben has no {missing.group(1)} processor yet ({OWNED_ELSEWHERE[missing.group(1)]})"
            continue
        if write:
            c["ekiben"] = "identical" if same else "different"
        if not same:
            bad += 1
            print(f"DIFFERENT {c['id']} {c['tests'][0]} code {code} vs {c['bento']['code']}: {err[-300:]}")
    print(f"{len(runnable) - bad} of {len(runnable)} cases identical")
    return bad


# ------------------------------------------------------------------------------------------ table
def table(deployment: Path, sha: str, cases: dict):
    import yaml
    by_text = defaultdict(list)
    for c in cases.values():
        texts = []
        if c["mode"] == "run":
            try:
                texts = [t for _, t in bloblang_texts(yaml.safe_load(get_blob(c["config"])))]
            except yaml.YAMLError:
                pass
        elif c["mode"] == "blobl":
            texts = [a for a in c["args"] if not a.startswith("-")]
        for t in texts:
            by_text[key(t)].append(c)
    rows, counts = [], defaultdict(int)
    for f in sorted((deployment / SEEDS).rglob("*-bento.yaml")):
        rel = f.relative_to(deployment / SEEDS)
        try:
            doc = yaml.safe_load(f.read_text())
        except yaml.YAMLError as e:
            rows.append((str(rel), "-", "-", f"not runnable (YAML: {str(e)[:60]})"))
            counts["not runnable"] += 1
            continue
        for path, text in bloblang_texts(doc):
            hits = by_text.get(key(text), [])
            ran = [c for c in hits if "ekiben" in c]
            if ran:
                verdict = "different" if any(c["ekiben"] == "different" for c in ran) else "identical"
                inputs = ", ".join(sorted({c["id"] for c in ran})[:4]) + (" …" if len(ran) > 4 else "")
            elif hits:
                verdict = f"not runnable ({hits[0]['why']})"
                inputs = ", ".join(sorted({c["id"] for c in hits})[:4])
            else:
                verdict = "not runnable (no deployment test feeds this mapping an input)"
                inputs = "-"
            counts[verdict.split(" (")[0]] += 1
            rows.append((str(rel), path, inputs, verdict))
    runnable = counts["identical"] + counts["different"]
    lines = [
        "# Bloblang parity with Bento 1.22.0",
        "",
        f"Generated by `scripts/parity/regen.py build` from joinedcontext-deployment `{sha}`",
        f"({SEEDS}/**/*-bento.yaml). Inputs are the documents the deployment's own Bento tests feed",
        "each mapping; a case is identical when ekiben's stdout and exit code equal Bento's byte for byte.",
        "A text is identical when every case that runs it is, different when one of them differs (the",
        "difference may sit in another step of that case), not runnable when no case runs it.",
        "",
        f"- Bloblang texts: {len(rows)}",
        f"- identical: {counts['identical']}, different: {counts['different']}, "
        f"not runnable: {counts['not runnable']}",
        f"- identical of runnable: {100 * counts['identical'] / runnable:.1f} %" if runnable else "- runnable: 0",
        f"- cases: {len(cases)} ({sum(1 for c in cases.values() if c.get('ekiben') == 'identical')} identical, "
        f"{sum(1 for c in cases.values() if c.get('ekiben') == 'different')} different, "
        f"{sum(1 for c in cases.values() if 'ekiben' not in c)} not runnable)",
        "",
        "| File | Path | Input (case ids in cases.json) | Result |",
        "|---|---|---|---|",
    ]
    lines += [f"| {f} | `{p}` | {i} | {v} |" for f, p, i, v in rows]
    (PARITY / "PARITY.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[6:10]))


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "capture":
        capture(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    elif cmd == "build":
        build(Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve())
    elif cmd == "check":
        cases = json.loads((PARITY / "cases.json").read_text())["cases"]
        sys.exit(1 if check(cases) else 0)
    else:
        sys.exit(__doc__)


main()
