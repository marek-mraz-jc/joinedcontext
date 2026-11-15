"""T-5542: tests/drills/walk_ops.sh, the three timed operations drills in project walk-ops.

The drills need dev, so the suite runs the script against one fake on localhost that answers as
the Portal, the gateway and Keycloak do, with a `jcctl` stub that hands each proposed file to the
fake as if a person approved it at once. A scenario switch makes one signal missing, so each drill
is shown to pass on the signal it asserts and to fail without it. The manifests the script
proposes are checked with `jcctl validate` (JC_JCCTL, else the pinned checkouts image).
"""

import json
import os
import shutil
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "tests" / "drills" / "walk_ops.sh"
TEMPLATE = ROOT / "tests" / "drills" / "walk-ops.yaml"
PLURALS = {
    "ContextSpace": "contextspaces", "DataSource": "datasources", "Pipeline": "pipelines",
    "Endpoint": "endpoints", "ServiceAccount": "serviceaccounts", "Policy": "policies",
}
#: Where jcctl validate wants each kind (MF-06).
LAYOUT = {
    "ContextSpace": "spaces/{name}/space.yaml", "DataSource": "datasources/{name}.yaml",
    "Pipeline": "pipelines/{name}/pipeline.yaml", "Endpoint": "spaces/walk/endpoints/{name}.yaml",
    "ServiceAccount": "access/serviceaccounts/{name}.yaml", "Policy": "spaces/walk/policies/{name}.yaml",
}

JCCTL_STUB = """#!/usr/bin/env python3
import json, sys, urllib.request, yaml
args = sys.argv[1:]
verb, path, server = args[0], args[args.index("-f") + 1], args[args.index("--server") + 1]
docs = [d for d in yaml.safe_load_all(open(path)) if d]
request = urllib.request.Request(server + "/_jcctl/" + verb, data=json.dumps(docs).encode(), method="POST")
urllib.request.urlopen(request).read()
for d in docs:
    print(d["kind"].lower() + "s/" + d["metadata"]["name"] + ": proposed, deploying")
"""

SLEEP_STUB = "#!/bin/sh\nexec /bin/sleep 0.02\n"


class Fake:
    """The Portal, gateway and Keycloak state one drill run sees."""

    def __init__(self, scenario: set[str]):
        self.scenario = scenario
        self.project = False
        self.resources: dict[str, dict] = {}
        self.secret = "first-secret"
        self.rotated_at = ""
        self.tokens: set[str] = set()
        self.minted = 0
        self.health_reads = 0
        self.calls = 0
        self.flood_seen = False
        self.log: list[str] = []

    def url_of_source(self) -> str:
        return self.resources.get("datasources/walk-source", {}).get("spec", {}).get("http", {}).get("url", "")

    def health(self) -> dict:
        self.health_reads += 1
        if "pipelines/walk-fetch" not in self.resources:
            return {"items": []}
        if self.url_of_source().endswith("walk-ops-drill-404") and "health-never-fails" not in self.scenario:
            row = {"pipeline": "walk-fetch", "state": "failing", "failedRuns": 1,
                   "lastError": "http GET answered 404 Not Found"}
        else:
            row = {"pipeline": "walk-fetch", "state": "ok", "failedRuns": 0}
        return {"items": [row]}

    @property
    def client(self) -> bool:
        """The Portal keeps the account's Keycloak client while the account exists (T-5626)."""
        return "serviceaccounts/walk-caller" in self.resources

    def rotate(self) -> None:
        """A person's Issue new secret on the Portal: the old one is refused, the event recorded."""
        self.secret = "rotated-secret"
        self.rotated_at = time.strftime("%Y-%m-%dT%H:%M:%S.123456Z", time.gmtime())

    def rpm(self) -> int:
        return self.resources.get("endpoints/walk-read", {}).get("spec", {}).get("rateLimits", {}).get("requestsPerMinute", 0)


def handler_for(fake: Fake):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send(self, code, body=None, headers=None):
            raw = b"" if body is None else json.dumps(body).encode()
            self.send_response(code)
            for k, v in (headers or {}).items():
                self.send_header(k, v)
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def body(self):
            n = int(self.headers.get("content-length") or 0)
            return self.rfile.read(n).decode() if n else ""

        def bearer(self):
            return (self.headers.get("authorization") or "").removeprefix("Bearer ")

        def do_GET(self):
            u = urlparse(self.path)
            p = u.path
            fake.log.append("GET " + p)
            if p == "/api/v1/health":
                return self.send(200, {"status": "ok"})
            if p == "/api/v1/health/walk-ops-drill-404":
                return self.send(404, {"title": "Not Found"})
            if p == "/api/v1/projects/walk-ops":
                return self.send(200 if fake.project else 404, {"metadata": {"name": "walk-ops"}})
            if p == "/api/v1/projects/walk-ops/pipeline-health":
                return self.send(200, fake.health())
            if p == "/api/v1/projects/walk-ops/activity" and parse_qs(u.query).get("kind") == ["access.revoked"]:
                rotated = [{"kind": "access.revoked", "time": fake.rotated_at}] if fake.rotated_at else []
                return self.send(200, {"items": rotated})
            if p == "/api/v1/projects/walk-ops/activity":
                items = [{"kind": "endpoint.rate-limited", "actor": "serviceaccount:walk-caller"}] \
                    if fake.flood_seen and "activity-names-caller" in fake.scenario else []
                return self.send(200, {"items": items})
            if p.startswith("/api/v1/projects/walk-ops/"):
                key = p.removeprefix("/api/v1/projects/walk-ops/")
                return self.send(200, fake.resources[key]) if key in fake.resources else self.send(404, {})
            if p.startswith("/api/endpoint/"):
                return self.gateway()
            return self.send(404, {})

        def gateway(self):
            if self.bearer() not in fake.tokens:
                return self.send(401, {})
            revoked = "policies/walk-caller-read" not in fake.resources
            if revoked and "policy-never-refuses" not in fake.scenario:
                return self.send(403, {})
            fake.calls += 1
            headers = {"ratelimit-limit": str(fake.rpm())}
            if fake.calls > 3:
                fake.flood_seen = True
                if "edge-429" in fake.scenario:
                    return self.send(429, {}, {"x-ratelimit-limit": "600", "retry-after": "1"})
                return self.send(429, {}, {**headers, "retry-after": "1"})
            return self.send(200, [], headers)

        def do_POST(self):
            p = urlparse(self.path).path
            body = self.body()
            fake.log.append("POST " + p)
            if p == "/api/v1/projects":
                assert json.loads(body)["name"] == "walk-ops"
                fake.project = True
                return self.send(202, {})
            if p in ("/_jcctl/apply", "/_jcctl/delete"):
                for doc in json.loads(body):
                    key = PLURALS[doc["kind"]] + "/" + doc["metadata"]["name"]
                    if p.endswith("apply"):
                        fake.resources[key] = doc
                    else:
                        fake.resources.pop(key, None)
                return self.send(200, {})
            if p == "/realms/dev/protocol/openid-connect/token":
                form = parse_qs(body)
                if not fake.client or form.get("client_secret") != [fake.secret] \
                        or form.get("client_id") != ["walk-ops-walk-caller"]:
                    return self.send(401, {"error": "unauthorized_client"})
                fake.minted += 1
                token = f"token-{fake.minted}"
                fake.tokens.add(token)
                return self.send(200, {"access_token": token})
            return self.send(404, {})

        def do_DELETE(self):
            p = urlparse(self.path).path
            fake.log.append("DELETE " + p)
            if p == "/api/v1/projects/walk-ops":
                fake.project = False
                return self.send(202, {})
            return self.send(404, {})

    return Handler


@pytest.fixture
def stubs(tmp_path):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for name, text in (("jcctl", JCCTL_STUB), ("sleep", SLEEP_STUB)):
        (bin_dir / name).write_text(text)
        (bin_dir / name).chmod(0o755)
    token = tmp_path / "token"
    token.write_text("person-token")
    return bin_dir, token


def drill(tmp_path, stubs, scenario, *extra):
    bin_dir, token = stubs
    fake = Fake(set(scenario))
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(fake))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://localhost:{server.server_address[1]}"
    env = {k: v for k, v in os.environ.items() if not k.startswith(("JC_", "KEYCLOAK_"))}
    env["PATH"] = f"{bin_dir}:{os.environ['PATH']}"
    process = subprocess.Popen(
        ["bash", str(SCRIPT), "--run", "--server", base, "--org", "example.org", "--owner", "ana",
         "--token-file", str(token), "--idm", f"{base}/realms/dev", "--rate", "2", "--seconds", "3",
         "--poll", "1", "--wait-merge", "5", "--bound-a", "5", "--bound-c", "3", *extra],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    lines: list[str] = []

    def person() -> None:
        """Answers each prompt as the person on the Portal would: issue, then paste."""
        for line in process.stdout:
            lines.append(line)
            if "issue the first secret of" in line:
                process.stdin.write(fake.secret + "\n")
                process.stdin.flush()
            elif "C: issue a new secret of" in line:
                fake.rotate()
                process.stdin.write(fake.secret + "\n")
                process.stdin.flush()

    reader = threading.Thread(target=person, daemon=True)
    reader.start()
    try:
        process.wait(timeout=120)
        reader.join(timeout=10)
        stderr = process.stderr.read()
    finally:
        server.shutdown()
    return subprocess.CompletedProcess(process.args, process.returncode, "".join(lines), stderr), fake


def test_dry_by_default_prints_the_plan_and_calls_nothing(tmp_path, stubs):
    bin_dir, _ = stubs
    (bin_dir / "curl").write_text(f"#!/bin/sh\necho called >> {tmp_path}/curl.log\n")
    (bin_dir / "curl").chmod(0o755)
    env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}"}
    result = subprocess.run(["bash", str(SCRIPT)], check=False, capture_output=True, text=True, env=env, timeout=30)
    assert result.returncode == 0, result.stderr
    assert "dry, nothing is called" in result.stdout
    for step in ("setup", "A ", "B ", "C ", "down"):
        assert step in result.stdout
    assert not (tmp_path / "curl.log").exists()


@pytest.mark.parametrize(
    "args, message",
    [
        (["--rate", "21"], "--rate is 1..20"),
        (["--seconds", "61"], "--seconds is 1..60"),
        (["--rate", "20", "--seconds", "60"], "under 600 calls"),
        (["--rate", "10", "--seconds", "60"], "under 600 calls"),
        (["--drill", "x"], "--drill takes"),
        (["--run"], "--server"),
        (["--run", "--server", "http://portal.example.org", "--org", "o", "--owner", "a",
          "--token-file", "/etc/hostname"], "https only"),
        (["--run", "--server", "https://portal.example.org", "--org", "o", "--owner", "a",
          "--token-file", "/etc/hostname", "--drill", "c"], "--idm"),
    ],
)
def test_unsafe_or_missing_arguments_are_refused(args, message):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("JC_", "KEYCLOAK_"))}
    result = subprocess.run(["bash", str(SCRIPT), *args], check=False, capture_output=True, text=True, env=env, timeout=30)
    assert result.returncode == 2
    assert message in result.stderr


def test_every_drill_passes_on_the_signal_it_asserts_and_cleans_up(tmp_path, stubs):
    result, fake = drill(tmp_path, stubs, {"activity-names-caller"})
    assert result.returncode == 0, result.stdout + result.stderr
    out = result.stdout
    assert "A detect" in out and "by pipeline-health (failing)" in out
    assert "A diagnose" in out and "A recover" in out
    assert "B detect" in out and "B diagnose" in out and "the Activity names walk-caller" in out
    assert "walk-read limited to 60/min" in out
    assert "C revoke-to-401" in out and "C policy-to-refusal" in out
    # Teardown removed everything the drill made.
    assert fake.resources == {} and not fake.client and not fake.project
    # No secret or token reached the output.
    for secret in ("first-secret", "rotated-secret", "person-token", "token-1"):
        assert secret not in out + result.stderr
    # No Keycloak administrator was needed (T-5626).
    assert not [line for line in fake.log if "/admin/" in line]
    # Nothing shared was restarted and no other project was touched.
    assert not [line for line in fake.log if "/projects/" in line and "walk-ops" not in line]


def test_drill_a_fails_when_health_never_shows_the_failing_run(tmp_path, stubs):
    result, _ = drill(tmp_path, stubs, {"activity-names-caller", "health-never-fails"}, "--drill", "a")
    assert result.returncode == 1
    assert "A: pipeline-health never showed walk-fetch failing" in result.stdout


def test_drill_b_fails_when_the_activity_does_not_name_the_caller(tmp_path, stubs):
    """Today's gateway: the 429 log line names only the slug, and the Activity records MCP tool
    calls alone, so an operator following runbook 3 cannot name the caller (T-5624)."""
    result, _ = drill(tmp_path, stubs, set(), "--drill", "b")
    assert result.returncode == 1
    assert "B: the project's Activity never named the caller walk-caller" in result.stdout
    assert "B detect" in result.stdout


def test_drill_b_fails_when_the_429_is_the_edge_s_and_not_the_gateway_s(tmp_path, stubs):
    """The edge's limit-count sends X-RateLimit-* only: its 429 proves nothing about the gateway."""
    result, _ = drill(tmp_path, stubs, {"activity-names-caller", "edge-429"}, "--drill", "b")
    assert result.returncode == 1
    assert "B: the edge answered 429" in result.stdout
    assert "B: no 429 at" in result.stdout


def test_drill_c_fails_when_the_earlier_token_still_reads(tmp_path, stubs):
    result, _ = drill(tmp_path, stubs, {"policy-never-refuses"}, "--drill", "c")
    assert result.returncode == 1
    assert "C revoke-to-401" in result.stdout
    assert "C: the token minted before the revocation still reads walk-read" in result.stdout


def filled() -> str:
    """The template filled in as the script fills it."""
    text = TEMPLATE.read_text(encoding="utf-8")
    for key, value in {"@ORG@": "example.org", "@OWNER@": "ana", "@RPM@": "600",
                       "@SOURCE_URL@": "https://portal.example.org/api/v1/health",
                       "@READ_SLUG@": "a" * 26, "@WRITE_SLUG@": "b" * 26}.items():
        text = text.replace(key, value)
    assert "@" not in text.replace("@context", "")
    return text


def rendered(tmp_path: Path) -> Path:
    """The filled template in the repository layout jcctl validate reads (MF-06)."""
    text = filled()
    repo = tmp_path / "repo"
    project = repo / "projects" / "walk-ops"
    project.mkdir(parents=True)
    (project / "project.yaml").write_text(yaml.safe_dump({
        "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Project",
        "metadata": {"name": "walk-ops", "namespace": "org"},
        "spec": {"organizationRef": "hel", "domain": "example.org"},
    }))
    for doc in yaml.safe_load_all(text):
        path = project / LAYOUT[doc["kind"]].format(name=doc["metadata"]["name"])
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(yaml.safe_dump(doc, sort_keys=False))
    return repo


def test_the_proposed_manifests_validate(tmp_path):
    repo = rendered(tmp_path)
    local = os.environ.get("JC_JCCTL")
    if local:
        cmd = [local, "validate", "--repo-dir", str(repo)]
    elif shutil.which("docker"):
        from test_manifest_format import checkouts_image

        cmd = ["docker", "run", "--rm", "--network", "none", "--read-only",
               "--user", f"{os.getuid()}:{os.getgid()}", "-v", f"{repo}:/w:ro", "-w", "/w",
               "--entrypoint", "/usr/local/bin/jcctl", checkouts_image(), "validate", "--repo-dir", "."]
    else:
        pytest.skip("no JC_JCCTL and no docker: jcctl validate cannot run here")
    result = subprocess.run(cmd, check=False, capture_output=True, text=True, timeout=600)
    assert result.returncode == 0, result.stdout + result.stderr


def test_the_drill_endpoint_is_rate_limited_and_the_space_expires_by_itself():
    docs = {d["kind"] + "/" + d["metadata"]["name"]: d for d in yaml.safe_load_all(filled())}
    assert docs["Endpoint/walk-read"]["spec"]["rateLimits"] == {"requestsPerMinute": 600, "burst": 60}
    assert docs["ContextSpace/walk"]["spec"]["isSandbox"] is True
    assert {d["metadata"]["namespace"] for d in docs.values()} == {"walk-ops"}
