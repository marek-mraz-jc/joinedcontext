"""The runner's per-stream caps (T-5370): a step that never returns or that eats memory is
stopped and the other streams, and every other project's runner, keep running.

Bento cannot free a stream whose step loops: DELETE /streams/{id} hangs and leaves it listed
`"active":false` with its core still held. The Portal caps each run (run_queue.rs) and deletes
it; the runner's liveness command fails on a stream still listed stopped ten minutes later, and
the kubelet's restart frees the core. A memory bomb meets the container's own limit. The chaos test
runs the pinned image under the probe the chart renders, one container per project as the
`runner.project` release is one process per project.
"""

import json
import shutil
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
pytestmark = pytest.mark.xdist_group("docker-pipeline-runner")
requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def docker_works():
    if shutil.which("docker") is None:
        return False
    return subprocess.run(["docker", "info"], check=False, capture_output=True).returncode == 0


requires_docker = pytest.mark.skipif(not docker_works(), reason="no docker daemon")

GOOD = {"input": {"generate": {"interval": "200ms", "mapping": "root.n = counter()"}}, "output": {"drop": {}}}
ONE = {"generate": {"interval": "1s", "count": 1, "mapping": "root = {}"}}
LOOP = {"input": ONE, "pipeline": {"processors": [{"jq": {"query": "def f: f; f"}}]}, "output": {"drop": {}}}
BOMB = {
    "input": ONE,
    "pipeline": {"processors": [{"mapping": "root = range(0, 100000000).map_each(i -> i.string())"}]},
    "output": {"drop": {}},
}


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def runner_liveness(docs):
    deployment = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    return deployment["spec"]["template"]["spec"]["containers"][0]["livenessProbe"]


@requires_helmfile
def test_the_runner_lives_by_a_stop_that_ends(dev):
    probe = runner_liveness(dev)
    assert "httpGet" not in probe
    command = probe["exec"]["command"]
    assert command[:2] == ["sh", "-c"] and "/streams" in command[2]
    # Two failures a minute apart: a stream held stopped for two to three minutes.
    assert (probe["periodSeconds"], probe["failureThreshold"]) == (60, 2)


PATIENCE = 600


def probe_command(patience=PATIENCE):
    """The command base-values renders, read from its YAML block (it holds no template); a
    docker test shortens the ten minutes a stopped stream may stay listed."""
    text = (ROOT / "components/pipeline-runner/values/runner/base-values.yaml.gotmpl").read_text()
    start = text.index("  liveness:\n    exec:")
    end = text.index("\n    periodSeconds", start)
    command = yaml.safe_load(text[start:end])["liveness"]["exec"]
    rendered = f"-v keep={PATIENCE} "
    assert rendered in command[2]
    return [*command[:2], command[2].replace(rendered, f"-v keep={patience} ")]


def test_the_probe_command_is_the_one_the_chart_renders():
    command = probe_command()
    assert command[:2] == ["sh", "-c"]
    assert "http://127.0.0.1:4195/streams" in command[2]


def stopped(name, uptime):
    return {name: {"active": False, "uptime": uptime, "uptime_str": f"{uptime}s"}}


RUNNING = {"helsinki.feed": {"active": True, "uptime": 812.4, "uptime_str": "13m32.4s"}}


@pytest.fixture
def probe(tmp_path):
    """Runs the rendered probe over /streams answers read from fixtures at a given clock: a
    `wget` and a `date` on PATH print the next answer and the time, and the probe keeps its
    state under tmp_path instead of /tmp. Returns (alive, what the probe printed)."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    answer, clock = tmp_path / "streams.json", tmp_path / "clock"
    for name, body in (("wget", f"cat {answer}"), ("date", f"cat {clock}")):
        stub = bin_dir / name
        stub.write_text(f"#!/bin/sh\n{body}\n")
        stub.chmod(0o755)
    command = probe_command()[2].replace("/tmp/", f"{tmp_path}/")

    def run(streams, at):
        answer.write_text(json.dumps(streams, separators=(",", ":")))
        clock.write_text(f"{1_793_000_000 + at}\n")
        env = {"PATH": f"{bin_dir}:/usr/bin:/bin"}
        done = subprocess.run(["sh", "-c", command], env=env, check=False, capture_output=True, text=True)
        return done.returncode == 0, done.stdout

    return run


def test_two_runs_of_one_pipeline_each_seen_finished_keep_the_runner_alive(probe):
    # T-6140: helsinki.hsl-departures runs often under one stream id; each probe saw a different
    # run that had just finished, and the id-only probe restarted the runner every ~40 min.
    for minute, uptime in enumerate((15.204118553, 14.871903377, 15.002771649) * 5):
        alive, _ = probe({**RUNNING, **stopped("helsinki.hsl-departures", uptime)}, at=60 * minute)
        assert alive, f"minute {minute}"


def test_a_finished_run_the_portal_deletes_minutes_late_keeps_the_runner_alive(probe):
    # T-6140 07:22Z: one finished run listed across three probes restarted ~300 streams. The
    # Portal deletes a finished run on its queue tick, which under load came minutes late.
    late = stopped("praha.pid-vehicles", 50.16)
    for minute in range(10):
        alive, said = probe({**RUNNING, **late}, at=60 * minute)
        assert alive and said == "", f"minute {minute}"
    assert probe(RUNNING, at=600) == (True, "")


def test_a_stream_held_stopped_ten_minutes_fails_the_probe_and_names_it(probe):
    held = stopped("helsinki.loop", 6.012405858)
    assert probe({**RUNNING, **held}, at=0)[0], "the first sight of a stopped stream is a finished run"
    assert probe({**RUNNING, **held}, at=540)[0], "nine minutes is the Portal still deleting"
    alive, said = probe({**RUNNING, **held, **stopped("a.once", 1.5)}, at=600)
    assert not alive, "the same stopped stream ten minutes later never ended"
    assert "helsinki.loop (uptime 6.012405858 s, stopped 600 s)" in said
    assert "a.once" not in said, "only the held stream is named"
    assert not probe({**RUNNING, **held}, at=660)[0], "it keeps failing until the restart"


def test_runs_and_deletes_between_probes_keep_the_runner_alive(probe):
    assert probe(RUNNING, at=0)[0]
    assert probe({**RUNNING, **stopped("a.once", 0.000664608)}, at=60)[0]
    assert probe(RUNNING, at=120)[0]
    assert probe({}, at=180)[0]
    # The same id and uptime back later is a new sighting, not the old one held.
    assert probe({**RUNNING, **stopped("a.once", 0.000664608)}, at=900)[0]


def test_a_state_file_from_the_older_probe_counts_as_a_first_sighting(probe, tmp_path):
    # /tmp outlives a container restart; the id-only probe wrote lines without a time.
    (tmp_path / "stopped").write_text('"helsinki.loop":{"active":false,"uptime":6.01\n')
    assert probe({**RUNNING, **stopped("helsinki.loop", 6.01)}, at=0)[0]


def test_a_streams_api_that_does_not_answer_fails_the_probe(tmp_path):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    (bin_dir / "wget").write_text("#!/bin/sh\nexit 1\n")
    (bin_dir / "wget").chmod(0o755)
    command = probe_command()[2].replace("/tmp/", f"{tmp_path}/")
    done = subprocess.run(["sh", "-c", command], env={"PATH": f"{bin_dir}:/usr/bin:/bin"},
                          check=False, capture_output=True, text=True)
    assert done.returncode != 0 and "did not answer" in done.stdout


def image():
    pin = yaml.safe_load((ROOT / "components/pipeline-runner/images.yaml").read_text())["pipeline-runner"]["runner"]
    return f"{pin['repository']}:{pin['tag']}@{pin['digest']}"


PROBE_PATIENCE = 2


class Runner:
    """One project's runner: the pinned image in its own container, capped like the pod."""

    def __init__(self, name, port, memory="512m"):
        self.name, self.url = name, f"http://localhost:{port}"
        subprocess.run(["docker", "rm", "-f", name], check=False, capture_output=True)
        subprocess.run(
            ["docker", "run", "-d", "--name", name, "--cpus", "1", "--memory", memory, "--read-only",
             "--tmpfs", "/tmp", "-p", f"{port}:4195", image(), "streams"],
            check=True, capture_output=True,
        )
        self.wait()

    def wait(self):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                self.call("GET", "/ping", timeout=1)
                return
            except OSError:
                time.sleep(0.3)
        raise AssertionError(f"{self.name} did not start")

    def call(self, verb, path, body=None, timeout=2.0):
        data = json.dumps(body).encode() if body is not None else None
        request = urllib.request.Request(self.url + path, data=data, method=verb)
        with urllib.request.urlopen(request, timeout=timeout) as answer:
            text = answer.read()
            return json.loads(text) if text and text[:1] in b"{[" else None

    def timed(self, verb, path, body=None):
        began = time.monotonic()
        answer = self.call(verb, path, body)
        return answer, time.monotonic() - began

    def live(self):
        """The rendered probe with a patience of PROBE_PATIENCE seconds instead of ten minutes."""
        result = subprocess.run(["docker", "exec", self.name, *probe_command(PROBE_PATIENCE)],
                                check=False, capture_output=True)
        return result.returncode == 0

    def cpu(self):
        out = subprocess.run(["docker", "stats", "--no-stream", "--format", "{{.CPUPerc}}", self.name],
                             capture_output=True, text=True, check=True).stdout
        return float(out.strip().rstrip("%"))

    def busiest(self, seconds):
        """The highest of the samples taken within `seconds`, up to the first above 80 %: one
        `docker stats` sample on a runner whose xdist workers fill every core can catch the
        loop's process off its core for that instant (T-4931, ci 40594287401)."""
        deadline = time.monotonic() + seconds
        best = self.cpu()
        while best <= 80 and time.monotonic() < deadline:
            best = max(best, self.cpu())
        return best

    def restarted(self):
        subprocess.run(["docker", "restart", "-t", "1", self.name], check=True, capture_output=True)
        self.wait()

    def oom_killed(self):
        out = subprocess.run(["docker", "inspect", "-f", "{{.State.OOMKilled}}", self.name],
                             capture_output=True, text=True, check=True).stdout
        return out.strip() == "true"

    def remove(self):
        subprocess.run(["docker", "rm", "-f", self.name], check=False, capture_output=True)


@pytest.fixture
def runners():
    made = []

    def make(name, port, **kw):
        made.append(Runner(name, port, **kw))
        return made[-1]

    yield make
    for runner in made:
        runner.remove()


def answers_within_2s(runner):
    held, seconds = runner.timed("GET", "/streams")
    assert seconds < 2, f"{runner.name} took {seconds:.1f} s"
    return held


@requires_docker
def test_a_looping_stream_is_stopped_and_every_other_stream_runs_on(runners):
    a = runners("t5370-project-a", 42011)
    b = runners("t5370-project-b", 42012)
    a.call("POST", "/streams/a.good", GOOD)
    b.call("POST", "/streams/b.good", GOOD)
    a.call("POST", "/streams/a.loop", LOOP)
    time.sleep(3)
    assert a.busiest(10) > 80, "the loop holds a core"
    assert a.live(), "a stream that runs is alive"

    # The Portal's delete at the cap: Bento never answers it, and a parallel read still does.
    with pytest.raises(OSError):
        a.call("DELETE", "/streams/a.loop", timeout=3)
    # Under CI load Bento can flag the stream inactive a moment after the delete times out.
    deadline = time.monotonic() + 10
    while answers_within_2s(a)["a.loop"]["active"] and time.monotonic() < deadline:
        time.sleep(0.5)
    assert answers_within_2s(a)["a.loop"]["active"] is False
    assert answers_within_2s(b)["b.good"]["active"] is True
    assert a.call("GET", "/streams/a.good")["active"] is True

    # Held stopped past the patience: liveness fails and the kubelet restarts the process.
    assert a.live(), "the first sight of a stopped stream is a finished run"
    time.sleep(PROBE_PATIENCE + 1)
    assert not a.live(), "the same stream still held is a stop that never ended"
    a.restarted()
    assert a.live()
    time.sleep(2)
    assert a.cpu() < 20, "the restart freed the core"
    assert answers_within_2s(a) == {}
    assert answers_within_2s(b)["b.good"]["active"] is True


@requires_docker
def test_a_finished_run_deleted_between_probes_keeps_the_runner_alive(runners):
    a = runners("t5370-finished", 42013)
    a.call("POST", "/streams/a.once", {"input": ONE, "output": {"drop": {}}})
    time.sleep(3)
    assert answers_within_2s(a)["a.once"]["active"] is False
    assert a.live()
    a.call("DELETE", "/streams/a.once")
    assert a.live()


@requires_docker
def test_a_pipeline_run_again_under_its_id_between_probes_keeps_the_runner_alive(runners):
    a = runners("t6140-rerun", 42016)
    once = {"input": ONE, "output": {"drop": {}}}
    a.call("POST", "/streams/a.often", once)
    time.sleep(3)
    first = answers_within_2s(a)["a.often"]
    assert first["active"] is False
    assert a.live()
    # The Portal's next run of the pipeline replaces the stream under the same id.
    a.call("PUT", "/streams/a.often", once)
    time.sleep(3)
    second = answers_within_2s(a)["a.often"]
    assert second["active"] is False and second["uptime"] != first["uptime"]
    assert a.live(), "a new run that finished is not the earlier run held"
    time.sleep(PROBE_PATIENCE + 1)
    assert not a.live(), "the same finished run still listed past the patience is held"


@requires_docker
def test_a_memory_bomb_meets_its_own_limit_and_the_other_project_answers(runners):
    a = runners("t5370-bomb", 42014, memory="256m")
    b = runners("t5370-neighbour", 42015)
    b.call("POST", "/streams/b.good", GOOD)
    try:
        a.call("POST", "/streams/a.bomb", BOMB, timeout=10)
    except OSError:
        pass  # the bomb may take the process down before it answers
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline and not a.oom_killed():
        assert answers_within_2s(b)["b.good"]["active"] is True
        time.sleep(1)
    assert a.oom_killed(), "the bomb was stopped by its container's limit"
    assert answers_within_2s(b)["b.good"]["active"] is True
