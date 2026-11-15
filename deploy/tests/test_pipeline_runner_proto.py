"""The GTFS-RT schema of hsl-departures and hsl-service-alerts, as the resident runner mounts it
(T-4037, AP-147).

Bento's `protobuf` step parses every .proto under its import paths. /streams is a ConfigMap
mount: the kubelet writes each key into a dated `..<timestamp>/` directory and links it from the
top, so the schema is there twice and both streams died at init with "symbol
transit_realtime.FeedMessage already defined". The runner mounts the schema alone at /proto by
subPath; both steps import from there. The Bento cases build the kubelet's layout in a temporary
directory and run both streams in one `bento streams` process; without a container runtime they
skip, the render case needs `just _dev-assemble`.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
STREAMS = ROOT / "components/pipeline-runner/streams"
FIXTURE = Path(__file__).resolve().parent / "fixtures/helsinki/gtfs-rt/trip-updates.pb"
MAPPINGS = {"departures": "helsinki-pipeline-departures-bento.yaml",
            "alerts": "helsinki-pipeline-service-alerts-bento.yaml"}
requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def protobuf_step(mapping: str) -> dict:
    (step,) = [p for p in yaml.safe_load((HELSINKI / mapping).read_text())["pipeline"]["processors"] if "protobuf" in p]
    return step


def configmap_mount(at: Path) -> Path:
    """What the kubelet makes of a ConfigMap volume: the keys in a dated directory, linked from the top."""
    dated = at / "..2026_10_12_06_45_20.123456789"
    dated.mkdir(parents=True)
    for key in ("gtfs-realtime.proto", "resources.yaml"):
        shutil.copy(STREAMS / key, dated / key)
        (at / key).symlink_to(Path("..data") / key)
    (at / "..data").symlink_to(dated.name)
    return at


def both_streams(tmp_path: Path, import_paths: list[str] | None = None) -> tuple[dict[str, int], str]:
    """Each stream's decoded trip count, and the runner's log, from one `bento streams` process."""
    streams = tmp_path / "s"
    streams.mkdir()
    feed = FIXTURE.read_bytes().hex()
    for name, mapping in MAPPINGS.items():
        step = protobuf_step(mapping)
        if import_paths is not None:
            step["protobuf"]["import_paths"] = import_paths
        (streams / f"{name}.yaml").write_text(json.dumps({
            "input": {"generate": {"count": 1, "interval": "", "mapping": f'root = "{feed}".decode("hex")'}},
            "pipeline": {"processors": [step, {"mapping": f'root = {{"stream": "{name}", "trips": this.entity.length()}}'}]},
            "output": {"stdout": {}},
        }))
    mount = configmap_mount(tmp_path / "configmap")
    result = subprocess.run(
        ["docker", "run", "--rm", "-v", f"{mount}:/streams:ro",
         "-v", f"{STREAMS / 'gtfs-realtime.proto'}:/proto/gtfs-realtime.proto:ro", "-v", f"{streams}:/s:ro",
         "--entrypoint", "sh", BENTO, "-c", "timeout 20 /bento streams /s/departures.yaml /s/alerts.yaml"],
        capture_output=True, text=True, timeout=120,
    )
    log = result.stdout + result.stderr
    decoded = {}
    for line in result.stdout.splitlines():
        if line.startswith('{"stream"'):
            record = json.loads(line)
            decoded[record["stream"]] = record["trips"]
    return decoded, log


@requires_docker
def test_both_streams_decode_in_one_runner_process_with_the_schema_alone_at_proto(tmp_path):
    decoded, log = both_streams(tmp_path)
    assert "already defined" not in log, log
    assert decoded == {"departures": 13, "alerts": 13}, log


@requires_docker
def test_the_configmap_mount_holds_the_schema_twice_so_it_is_no_import_path(tmp_path):
    decoded, log = both_streams(tmp_path, ["/streams"])
    assert "transit_realtime.FeedMessage" in log and "already defined" in log, log
    assert decoded == {}


def test_both_steps_import_the_schema_from_proto_only():
    for mapping in MAPPINGS.values():
        assert protobuf_step(mapping)["protobuf"]["import_paths"] == ["/proto"], mapping


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


@requires_helmfile
def test_the_runner_mounts_the_schema_alone_at_proto_by_subpath(dev):
    runner = next(d for d in dev if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    pod = runner["spec"]["template"]["spec"]
    config = next(v for v in pod["volumes"] if v["name"] == "config")["configMap"]["name"]
    (mount,) = [m for m in pod["containers"][0]["volumeMounts"] if m["mountPath"].startswith("/proto")]
    assert mount == {"name": "config", "mountPath": "/proto/gtfs-realtime.proto",
                     "subPath": "gtfs-realtime.proto", "readOnly": True}
    configmap = next(d for d in dev if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == config)
    assert "transit_realtime" in configmap["data"]["gtfs-realtime.proto"]
