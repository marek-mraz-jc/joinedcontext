#!/usr/bin/env python3
"""What a scheduled pipeline leaves in the runner's memory after its run (T-5285).

  scripts/runner-rss-rig.py [--names 500] [--port 42150] [--check 1.0]
  scripts/runner-rss-rig.py --runs 500 [--streams 20] [--profiles DIR] [--check 0.1] (T-5797)

Starts the pinned Bento twice in streams mode on the local Docker, once with the runner's `--set`
arguments from components/pipeline-runner/values/runner/base-values.yaml.gotmpl and once with all
but `metrics.mapping`, then creates and deletes --names distinct streams in each, the way the Portal
runs a scheduled pipeline. Prints per runner the series lines one stream leaves in /metrics and the
anonymous RSS growth per name. --check fails when the shipped runner grows by that many MB or more
per name. Needs docker; reads /proc/<pid>/status of the container's process.

--runs re-runs --streams stream names round the way a scheduled pipeline reuses its name (T-5797):
each run declares the project's write budget over the API as the Portal does before every send,
creates the stream, waits for its input to end and deletes it, through the seeds' input kinds
(generate + http with oauth2 and json_array unarchive, an http_client input, a zip unarchive)
against a stub feed and gateway this script serves. The runner gets the shipped `--set` arguments
plus `http.debug_endpoints=true` (the rig only; dev keeps them off). It prints anonymous RSS, Go
heap in use and goroutines after a warm-up round and at runs 100 and --runs, saves heap and
goroutine profiles there into --profiles, and --check fails at that many MB of growth per run.
"""

import argparse
import io
import json
import re
import subprocess
import sys
import threading
import time
import urllib.request
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VALUES = ROOT / "components/pipeline-runner/values/runner/base-values.yaml.gotmpl"
IMAGES = ROOT / "components/pipeline-runner/images.yaml"


def set_args(values: str) -> list[str]:
    """The values of every `- --set` pair in the runner's args, unquoted."""
    out = []
    for raw in re.findall(r"^\s*- --set\n\s*- (.+)$", values, re.MULTILINE):
        raw = raw.strip()
        if raw[:1] == raw[-1:] and raw[:1] in "'\"":
            raw = raw[1:-1]
        out.append(raw)
    return out


def image(images: str) -> str:
    runner = images.split("runner:", 1)[1]
    repo = re.search(r"repository: '([^']+)'", runner).group(1)
    digest = re.search(r"digest: '([^']+)'", runner).group(1)
    return f"{repo}@{digest}"


def stream(processors: int = 30) -> dict:
    """One message through `processors` labelled steps, two of them writes, like a rendered pipeline."""
    steps = [{"label": f"mapping_{i}", "mapping": "root = this"} for i in range(processors - 2)]
    steps += [{"label": "write_0", "mapping": "root = this"}, {"label": "route_1_write_0", "mapping": "root = this"}]
    return {"input": {"label": "input", "generate": {"count": 1, "interval": "", "mapping": 'root = {"a": 1}'}},
            "pipeline": {"processors": steps}, "output": {"label": "output", "drop": {}}}


def per_name_mb(before_kb: int, after_kb: int, names: int) -> float:
    return (after_kb - before_kb) / 1024 / names


def rss_anon_kb(status: str) -> int:
    return int(re.search(r"^RssAnon:\s+(\d+) kB", status, re.MULTILINE).group(1))


def call(method: str, url: str, body: bytes | None = None) -> str:
    req = urllib.request.Request(url, data=body, method=method, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return resp.read().decode()


def run(img: str, sets: list[str], port: int, names: int) -> tuple[int, float]:
    name = f"runner-rss-rig-{port}"
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    argv = ["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:4195", img, "-w"]
    for s in sets:
        argv += ["--set", s]
    subprocess.run(argv + ["streams"], capture_output=True, check=True)
    try:
        base = f"http://127.0.0.1:{port}"
        for _ in range(50):
            try:
                call("GET", base + "/ping")
                break
            except OSError:
                time.sleep(0.2)
        pid = subprocess.run(["docker", "inspect", "-f", "{{.State.Pid}}", name],
                             capture_output=True, text=True, check=True).stdout.strip()
        body = json.dumps(stream()).encode()
        call("POST", base + "/streams/probe", body)
        time.sleep(1)
        lines = sum(1 for line in call("GET", base + "/metrics").splitlines() if 'stream="probe"' in line)
        call("DELETE", base + "/streams/probe")
        before = rss_anon_kb(Path(f"/proc/{pid}/status").read_text())
        for i in range(names):
            call("POST", f"{base}/streams/rig-{i}", body)
            call("DELETE", f"{base}/streams/rig-{i}")
        time.sleep(2)
        after = rss_anon_kb(Path(f"/proc/{pid}/status").read_text())
        return lines, per_name_mb(before, after, names)
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)


FEED = json.dumps([{"id": f"urn:ngsi-ld:Thing:{i}", "name": f"thing {i}", "value": i * 1.5, "text": "x" * 200}
                   for i in range(500)]).encode()
# A page the size of dev's largest runs' (T-5669: ~10 KB per entity, 70k entities a run), scaled
# down to about 20 MB so a round stays minutes long.
BIG = json.dumps([{"id": f"urn:ngsi-ld:Thing:{i}", "text": "y" * 9000} for i in range(2200)]).encode()


def feed_zip() -> bytes:
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("stops.txt", "stop_id,stop_name\n" + "".join(f"{i},stop {i}\n" for i in range(2000)))
        z.writestr("other.txt", "a,b\n1,2\n")
    return out.getvalue()


class Stub(BaseHTTPRequestHandler):
    """The feeds a run reads, the token endpoint and the gateway it writes to, with keep-alive."""
    protocol_version = "HTTP/1.1"
    zipped = feed_zip()

    def answer(self, body: bytes, kind: str = "application/json") -> None:
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        body = {"/feed.zip": self.zipped, "/big": BIG}.get(self.path, FEED)
        self.answer(body, "application/zip" if self.path == "/feed.zip" else "application/json")

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        if self.path == "/token":
            self.answer(b'{"access_token":"t","token_type":"Bearer","expires_in":300}')
        else:
            self.answer(b"{}")

    def log_message(self, *_):
        pass


def run_streams(stub: str) -> dict[str, dict]:
    """One scheduled run per input kind the seeds render (Portal render.rs `datasource_input`)."""
    oauth2 = {"enabled": True, "client_key": "c", "client_secret": "s", "token_url": stub + "/token"}
    write = {"http": {"url": stub + "/write", "verb": "POST", "oauth2": oauth2, "rate_limit": "rig__pipeline_egress"}}
    tail = [{"mapping": 'root = this.map_each(e -> e.merge({"type": "Thing"}))'}, write]
    get = {"url": stub + "/feed", "verb": "GET", "oauth2": oauth2}
    tick = {"generate": {"count": 1, "interval": "", "mapping": 'root = ""'}}
    unzip = [{"try": [{"unarchive": {"format": "zip"}}]},
             {"mapping": 'root = if @archive_filename != "stops.txt" { deleted() } else { content().string().parse_csv() }'},
             {"archive": {"format": "json_array"}}, {"mapping": "root = this.flatten()"}]
    return {
        "generate_http": {"input": tick, "pipeline": {"processors": [
            {"try": [{"http": get}]}, {"mutation": "root = if errored() { deleted() }"},
            {"unarchive": {"format": "json_array"}}, {"archive": {"format": "json_array"}}] + tail},
            "output": {"drop": {}}},
        "http_client": {"input": {"read_until": {"check": "true", "input": {"http_client": get}}},
                        "pipeline": {"processors": tail}, "output": {"drop": {}}},
        "big": {"input": tick, "pipeline": {"processors": [
            {"try": [{"http": dict(get, url=stub + "/big")}]}, {"unarchive": {"format": "json_array"}},
            {"archive": {"format": "json_array"}}] + tail}, "output": {"drop": {}}},
        "zip": {"input": tick, "pipeline": {"processors": [
            {"try": [{"http": dict(get, url=stub + "/feed.zip")}]}] + unzip + tail}, "output": {"drop": {}}},
    }


def heap_inuse_mb(heap_debug: str) -> float:
    """HeapInuse from Go's /debug/pprof/heap?debug=1 trailer, in MB."""
    return int(re.search(r"^# HeapInuse = (\d+)", heap_debug, re.MULTILINE).group(1)) / 1024 / 1024


def goroutines(profile: str) -> int:
    return int(re.search(r"^goroutine profile: total (\d+)", profile).group(1))


def wait_ended(base: str, sid: str, cap: float = 30) -> None:
    deadline = time.monotonic() + cap
    while time.monotonic() < deadline:
        if not json.loads(call("GET", f"{base}/streams/{sid}"))["active"]:
            return
        time.sleep(0.05)
    raise RuntimeError(f"stream {sid} still active after {cap} s")


def reuse(img: str, sets: list[str], port: int, runs: int, streams: int, profiles: Path | None,
          settle: float = 100, env: dict[str, str] | None = None) -> dict:
    stub = ThreadingHTTPServer(("0.0.0.0", 0), Stub)
    threading.Thread(target=stub.serve_forever, daemon=True).start()
    kinds = run_streams(f"http://host.docker.internal:{stub.server_address[1]}")
    name = f"runner-rss-rig-{port}"
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    argv = ["docker", "run", "-d", "--name", name, "--add-host", "host.docker.internal:host-gateway",
            "-p", f"127.0.0.1:{port}:4195"]
    for k, v in (env or {}).items():
        argv += ["-e", f"{k}={v}"]
    argv += [img, "-w"]
    for s in sets + ["http.debug_endpoints=true"]:
        argv += ["--set", s]
    subprocess.run(argv + ["streams"], capture_output=True, check=True)
    base = f"http://127.0.0.1:{port}"
    seen = {}
    try:
        for _ in range(50):
            try:
                call("GET", base + "/ping")
                break
            except OSError:
                time.sleep(0.2)
        pid = subprocess.run(["docker", "inspect", "-f", "{{.State.Pid}}", name],
                             capture_output=True, text=True, check=True).stdout.strip()
        budget = json.dumps({"local": {"count": 500, "interval": "1s"}}).encode()
        bodies = [json.dumps(k).encode() for k in kinds.values()]

        def one(i: int) -> None:
            sid = f"rig-{i % streams}"
            call("POST", base + "/resources/rate_limit/rig__pipeline_egress", budget)
            call("POST", f"{base}/streams/{sid}", bodies[i % len(bodies)])
            wait_ended(base, sid)
            call("DELETE", f"{base}/streams/{sid}")

        def look(at: int) -> None:
            # Past Bento's 90 s idle_connection_timeout: a finished run's keep-alive connections
            # (a goroutine pair each) are still open before it, which is not what a run leaves.
            time.sleep(settle)
            profile = urllib.request.urlopen(base + "/debug/pprof/heap?gc=1", timeout=30).read()
            heap = call("GET", base + "/debug/pprof/heap?debug=1")
            gor = call("GET", base + "/debug/pprof/goroutine?debug=1")
            seen[at] = {"rss_mb": rss_anon_kb(Path(f"/proc/{pid}/status").read_text()) / 1024,
                        "heap_mb": heap_inuse_mb(heap), "goroutines": goroutines(gor)}
            if profiles:
                profiles.mkdir(parents=True, exist_ok=True)
                (profiles / f"heap-{at}.pb.gz").write_bytes(profile)
                (profiles / f"goroutine-{at}.txt").write_text(gor)

        warm = streams * len(bodies)
        for i in range(warm):
            one(i)
        look(0)
        for i in range(runs):
            one(warm + i)
            if i + 1 == min(100, runs) or i + 1 == runs:
                look(i + 1)
        return seen
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
        stub.shutdown()


def per_run_mb(seen: dict) -> float:
    last = max(seen)
    return (seen[last]["rss_mb"] - seen[0]["rss_mb"]) / last


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--names", type=int, default=500)
    ap.add_argument("--port", type=int, default=42150)
    ap.add_argument("--check", type=float, help="fail at this many MB per name (or per run) or more")
    ap.add_argument("--runs", type=int, help="re-run --streams names this many times instead (T-5797)")
    ap.add_argument("--streams", type=int, default=20)
    ap.add_argument("--settle", type=float, default=100, help="seconds --runs waits before each look")
    ap.add_argument("--profiles", type=Path, help="where --runs saves heap and goroutine profiles")
    a = ap.parse_args()
    img = image(IMAGES.read_text())
    shipped = set_args(VALUES.read_text())
    if a.runs:
        seen = reuse(img, shipped, a.port, a.runs, a.streams, a.profiles, a.settle)
        for at, v in sorted(seen.items()):
            print(f"after {at} runs: {v['rss_mb']:.1f} MB anonymous RSS, {v['heap_mb']:.1f} MB heap in use, "
                  f"{v['goroutines']} goroutines")
        mb = per_run_mb(seen)
        print(f"{mb:.4f} MB anonymous RSS per run ({a.runs} runs over {a.streams} names x {len(run_streams(''))} kinds)")
        if a.check is not None and mb >= a.check:
            print(f"FAIL: {mb:.4f} MB per run, limit {a.check}", file=sys.stderr)
            return 1
        return 0
    unmapped = [s for s in shipped if not s.startswith("metrics.mapping=")]
    result = {}
    for label, sets, port in (("shipped", shipped, a.port), ("without mapping", unmapped, a.port + 1)):
        lines, mb = run(img, sets, port, a.names)
        result[label] = mb
        print(f"{label}: {lines} series lines per stream, {mb:.3f} MB anonymous RSS per deleted name ({a.names} names)")
    if a.check is not None and result["shipped"] >= a.check:
        print(f"FAIL: {result['shipped']:.3f} MB per name, limit {a.check}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
