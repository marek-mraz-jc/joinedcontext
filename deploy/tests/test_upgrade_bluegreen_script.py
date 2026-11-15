"""T-0047: scripts/upgrade-broker-bluegreen.sh must fill green before it moves any traffic.

The script talks to helm, kubectl and jcctl, so the suite runs it against stubs on PATH inside
a throwaway copy of the repository. What is asserted is the order: the cut-over is the last
write, and every way green can turn out wrong has to leave blue serving (OPS-13, OPS-14).
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = PROJECT_ROOT / "scripts" / "upgrade-broker-bluegreen.sh"

BLUE_DB = "postgresql://antares:$(PGPASSWORD)@postgres-cluster-rw.jc.svc.cluster.local:5432/antares"
DIGEST_IMAGE = "ghcr.io/marek-mraz/antares-broker@sha256:" + "b" * 64

# Every stub is the same program: it writes its whole command line to the log the test reads
# back, and answers from the spec the test wrote.
STUB = '''#!/usr/bin/env python3
import json, os, sys
spec = json.load(open(os.environ["STUB_SPEC"]))
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
line = name + " " + " ".join(args)
with open(os.environ["STUB_LOG"], "a") as log:
    log.write(line + "\\n")

def answer(table, default=""):
    for match, value in spec.get(table, []):
        needles = match if isinstance(match, list) else [match]
        if all(n in line for n in needles):
            return value
    return default

if name == "helm" and args[:1] == ["list"]:
    wanted = next(a for a in args if a.startswith("^")).strip("^$")
    found = spec.get("releases", {}).get(wanted)
    sys.stdout.write(json.dumps([{"name": wanted, "namespace": found}] if found else [], separators=(",", ":")))
elif name == "helm" and args[:1] == ["get"]:
    sys.stdout.write("replicaCount: 1\\n")
elif name == "helm":
    sys.exit(int(answer("helmExit", 0)))
elif name == "kubectl":
    sys.stdout.write(str(answer("kubectl", "")))
    sys.exit(int(answer("kubectlExit", 0)))
elif name == "jcctl":
    sys.stdout.write(str(answer("jcctl", "")))
    sys.exit(int(answer("jcctlExit", 0)))
elif name == "curl":
    sys.stdout.write(str(answer("curl", "200")))
'''

HEALTHY_KUBECTL = [
    ["maxUnavailable", "0"],
    ["ANTARES_DATABASE_URL", BLUE_DB],
    [["instanceRole=primary", "metadata.name"], "postgres-cluster-1"],
    [["instanceRole=primary", "metadata.namespace"], "jc"],
    ["pg_database", ""],  # the green database does not exist yet
]

HEALTHY = {
    "releases": {"context-broker-broker": "jc", "context-gateway-gateway": "jc"},
    "kubectl": HEALTHY_KUBECTL,
    "curl": [["healthz", "200"]],
    "jcctl": [["plan", "Plan: 0 to add, 0 to change, 0 to delete"]],
}


def sandbox(tmp_path: Path, spec: dict) -> dict:
    scripts = tmp_path / "scripts"
    scripts.mkdir(parents=True, exist_ok=True)
    (scripts / SCRIPT.name).write_text(SCRIPT.read_text())
    (scripts / SCRIPT.name).chmod(0o755)
    (tmp_path / "charts" / "workload").mkdir(parents=True, exist_ok=True)
    (tmp_path / "config").mkdir(parents=True, exist_ok=True)

    stub_bin = tmp_path / "bin"
    stub_bin.mkdir(parents=True, exist_ok=True)
    for name in ("helm", "kubectl", "jcctl", "curl"):
        (stub_bin / name).write_text(STUB)
        (stub_bin / name).chmod(0o755)

    (tmp_path / "spec.json").write_text(json.dumps(spec))
    env = dict(os.environ)
    env["PATH"] = f"{stub_bin}:{env['PATH']}"
    env["STUB_SPEC"] = str(tmp_path / "spec.json")
    env["STUB_LOG"] = str(tmp_path / "calls.log")
    return env


def run(tmp_path: Path, spec: dict, *args: str) -> subprocess.CompletedProcess:
    env = sandbox(tmp_path, spec)
    done = subprocess.run(
        [f"scripts/{SCRIPT.name}", *args],
        cwd=tmp_path,
        env=env,
        capture_output=True,
        text=True,
    )
    log = tmp_path / "calls.log"
    done.calls = log.read_text().splitlines() if log.exists() else []  # type: ignore[attr-defined]
    return done


def upgrade(tmp_path: Path, spec: dict, *extra: str) -> subprocess.CompletedProcess:
    return run(tmp_path, spec, "--image", DIGEST_IMAGE, "--repo-dir", str(tmp_path / "config"), *extra)


def index_of(calls, *needles) -> int:
    for i, call in enumerate(calls):
        if all(n in call for n in needles):
            return i
    return -1


def cutover(calls) -> int:
    """The one call that moves live traffic: the blue gateway told to dial the green broker."""
    return index_of(calls, "helm upgrade context-gateway-gateway", "context-broker-green")


class TestPreconditions:
    def test_a_healthy_instance_passes_the_check(self, tmp_path):
        done = run(tmp_path, HEALTHY, "--check")
        assert done.returncode == 0, done.stderr
        assert "checked only; nothing was written" in done.stdout
        assert index_of(done.calls, "helm upgrade") == -1, done.calls

    def test_a_gateway_that_would_drop_requests_stops_the_upgrade(self, tmp_path):
        spec = {**HEALTHY, "kubectl": [["maxUnavailable", "1"], *HEALTHY_KUBECTL]}
        done = upgrade(tmp_path, spec)
        assert done.returncode == 1
        assert "would drop requests" in done.stderr
        assert cutover(done.calls) == -1

    def test_an_image_by_tag_is_refused(self, tmp_path):
        done = run(tmp_path, HEALTHY, "--image", "ghcr.io/x/antares:v2", "--repo-dir", str(tmp_path / "config"))
        assert done.returncode == 1
        assert "pinned by digest" in done.stderr
        assert index_of(done.calls, "helm upgrade") == -1

    def test_an_unfinished_earlier_upgrade_is_not_run_over(self, tmp_path):
        spec = {**HEALTHY, "releases": {**HEALTHY["releases"], "context-broker-green": "jc"}}
        done = upgrade(tmp_path, spec)
        assert done.returncode == 1
        assert "already exists" in done.stderr

    def test_no_instance_is_not_a_green_field(self, tmp_path):
        done = run(tmp_path, {**HEALTHY, "releases": {}}, "--check")
        assert done.returncode == 1
        assert "deploy the instance first" in done.stderr


class TestOrder:
    def test_the_cutover_is_the_last_write_and_follows_every_check(self, tmp_path):
        done = upgrade(tmp_path, HEALTHY)
        assert done.returncode == 0, done.stderr + done.stdout
        calls = done.calls
        steps = [
            index_of(calls, "CREATE DATABASE"),
            index_of(calls, "helm upgrade --install context-broker-green"),
            index_of(calls, "helm upgrade --install context-gateway-green"),
            index_of(calls, "jcctl apply"),
            index_of(calls, "jcctl plan"),
            index_of(calls, "curl", "healthz"),
            cutover(calls),
        ]
        assert -1 not in steps, calls
        assert steps == sorted(steps), calls

    def test_green_is_filled_through_its_own_gateway_and_never_the_broker(self, tmp_path):
        done = upgrade(tmp_path, HEALTHY)
        replay = [c for c in done.calls if c.startswith("jcctl apply") or c.startswith("jcctl plan")]
        assert replay, done.calls
        for call in replay:
            assert "--gateway-url http://context-gateway-green.jc.svc.cluster.local:8080" in call
            assert "context-broker" not in call

    def test_the_replay_never_writes_into_the_live_database(self, tmp_path):
        done = upgrade(tmp_path, HEALTHY)
        install = done.calls[index_of(done.calls, "helm upgrade --install context-broker-green")]
        assert install.endswith("--wait --timeout 600s")
        url = next(a.split("=", 1)[1] for a in install.split() if a.startswith("env.ANTARES_DATABASE_URL="))
        assert url == BLUE_DB + "_green", url
        assert url != BLUE_DB
        created = done.calls[index_of(done.calls, "CREATE DATABASE")]
        assert "CREATE DATABASE antares_green OWNER antares" in created

    def test_an_existing_green_database_is_not_recreated(self, tmp_path):
        spec = {**HEALTHY, "kubectl": [["pg_database", "1"], *HEALTHY_KUBECTL]}
        done = upgrade(tmp_path, spec)
        assert done.returncode == 0, done.stderr
        assert index_of(done.calls, "CREATE DATABASE") == -1
        assert index_of(done.calls, "CREATE EXTENSION") != -1
        assert cutover(done.calls) != -1

    def test_the_replay_gateway_goes_away_and_the_green_broker_stays(self, tmp_path):
        done = upgrade(tmp_path, HEALTHY)
        assert index_of(done.calls, "helm uninstall context-gateway-green") > cutover(done.calls)
        assert index_of(done.calls, "helm uninstall context-broker-green") == -1

    def test_the_operator_can_keep_the_replay_gateway(self, tmp_path):
        done = upgrade(tmp_path, HEALTHY, "--keep-green-gateway")
        assert done.returncode == 0, done.stderr
        assert index_of(done.calls, "helm uninstall") == -1


class TestBlueKeepsServing:
    @pytest.mark.parametrize(
        "spec,expected",
        [
            (
                {"jcctl": [["plan", "Plan: 3 to add, 0 to change, 0 to delete"]]},
                "does not match the repository",
            ),
            (
                {"curl": [["healthz", "503"]]},
                "answered '503' on /healthz",
            ),
            (
                {"jcctl": [["apply", ""]], "jcctlExit": [["apply", 1]]},
                "",
            ),
        ],
        ids=["a diff after the replay", "a green gateway that does not answer", "a replay that fails"],
    )
    def test_green_going_wrong_moves_no_traffic(self, tmp_path, spec, expected):
        done = upgrade(tmp_path, {**HEALTHY, **spec})
        assert done.returncode != 0
        assert expected in done.stderr
        assert cutover(done.calls) == -1, done.calls
        # And the blue broker's database was never written to by the replay.
        assert index_of(done.calls, "DROP DATABASE") == -1
