"""T-0034: scheduled pipelines are CronJobs, and the cadence mapping of PL-27.

Cron cannot express anything shorter than a minute, so a pipeline that polls every 45 seconds
runs every minute and the Bento trigger fires floor(60 / 45) times inside that one pod. The
same rule lives in `jcctl::pipelines::cron_job` for a `kind: Pipeline`; the numbers here are
the manifest half of it.
"""

import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

CHART = Path(__file__).resolve().parent.parent / "components/pipeline-runner/charts/cronjob"
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")

# A pipeline that says nothing about how it fetches: the cadence is what these tests are about.
CONFIG = {"pipeline": {"processors": [{"mapping": "root = this"}]}, "output": {"drop": {}}}


def render(tmp_path: Path, pipelines: dict, **values) -> subprocess.CompletedProcess:
    payload = {
        "project": "ovzdusie",
        "image": {"repository": "ghcr.io/warpstreamlabs/bento", "tag": "1.22.0"},
        "pipelines": pipelines,
        **values,
    }
    tmp_path.mkdir(parents=True, exist_ok=True)
    values_file = tmp_path / "values.yaml"
    values_file.write_text(yaml.safe_dump(payload))
    return subprocess.run(
        ["helm", "template", "pipeline-runner-scheduled", str(CHART), "-f", str(values_file)],
        capture_output=True, text=True,
    )


def objects(result: subprocess.CompletedProcess) -> dict:
    assert result.returncode == 0, result.stderr
    return {d["kind"]: d for d in yaml.safe_load_all(result.stdout) if d}


def bento_of(docs: dict) -> dict:
    return yaml.safe_load(docs["ConfigMap"]["data"]["bento.yaml"])


@requires_helm
@pytest.mark.parametrize(
    "pipeline, schedule, interval, count",
    [
        ({"periodSeconds": 45}, "* * * * *", "45s", 1),
        ({"periodSeconds": 30}, "* * * * *", "30s", 2),
        ({"periodSeconds": 90}, "* * * * *", "90s", 1),
        ({"periodSeconds": 300, "schedule": "*/5 * * * *"}, "*/5 * * * *", None, 1),
        ({"schedule": "0 3 * * *"}, "0 3 * * *", None, 1),
    ],
    ids=["45s", "30s-floor", "90s-no-cron", "five-minutes", "daily"],
)
def test_the_cadence_becomes_a_schedule_and_a_trigger(tmp_path, pipeline, schedule, interval, count):
    docs = objects(render(tmp_path, {"batch": {**pipeline, "config": CONFIG}}))
    assert docs["CronJob"]["spec"]["schedule"] == schedule
    generate = bento_of(docs)["input"]["generate"]
    assert generate.get("interval") == interval
    assert generate["count"] == count


@requires_helm
def test_a_period_under_the_thirty_second_rule_is_refused(tmp_path):
    """PL-26: that pipeline belongs in the resident runner. Rendering it as a CronJob would
    poll every minute instead of every fifteen seconds and nothing would say so."""
    result = render(tmp_path, {"fast": {"periodSeconds": 15, "config": CONFIG}})
    assert result.returncode != 0
    assert "thirty-second rule" in result.stderr


@requires_helm
def test_a_pipeline_with_no_cadence_at_all_is_refused(tmp_path):
    result = render(tmp_path, {"orphan": {"config": CONFIG}})
    assert result.returncode != 0
    assert "periodSeconds or a cron schedule" in result.stderr


@requires_helm
def test_a_configuration_that_declares_its_own_input_is_refused(tmp_path):
    """The chart owns `input`: it is where the cadence lands. An authored input would be
    silently replaced, and the pipeline would poll at the wrong rate."""
    config = {"input": {"http_client": {"url": "https://example.org/x.csv"}}, **CONFIG}
    result = render(tmp_path, {"batch": {"periodSeconds": 60, "config": config}})
    assert result.returncode != 0
    assert "must not carry an input of its own" in result.stderr


@requires_helm
def test_the_authors_configuration_is_carried_through_unchanged(tmp_path):
    docs = objects(render(tmp_path, {"batch": {"periodSeconds": 60, "config": CONFIG}}))
    rendered = bento_of(docs)
    assert rendered["pipeline"] == CONFIG["pipeline"]
    assert rendered["output"] == CONFIG["output"]


@requires_helm
def test_the_run_is_ephemeral_and_never_overlaps(tmp_path):
    """PL-10: the pod exists for the fetch. A run that hangs is killed by the deadline, a run
    that is late is skipped rather than stacked, and a failure waits for the next tick."""
    docs = objects(render(tmp_path, {"batch": {"periodSeconds": 60, "config": CONFIG}}))
    spec = docs["CronJob"]["spec"]
    assert spec["concurrencyPolicy"] == "Forbid"
    assert spec["startingDeadlineSeconds"] == 30
    job = spec["jobTemplate"]["spec"]
    assert job["activeDeadlineSeconds"] > 0
    assert job["backoffLimit"] == 0
    pod = job["template"]["spec"]
    assert pod["restartPolicy"] == "Never"


@requires_helm
def test_the_pod_carries_no_kubernetes_token_and_runs_as_its_project(tmp_path):
    """The Context Gateway refuses ServiceAccount tokens (Architecture/08 §5.1), so a mounted
    token could only ever be used against the API server."""
    docs = objects(render(tmp_path, {"batch": {"periodSeconds": 60, "config": CONFIG}}))
    pod = docs["CronJob"]["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False
    assert docs["ServiceAccount"]["automountServiceAccountToken"] is False
    assert pod["serviceAccountName"] == "pipeline-ovzdusie"
    assert docs["ServiceAccount"]["metadata"]["name"] == "pipeline-ovzdusie"


@requires_helm
def test_the_security_context_and_the_image_digest_reach_the_container(tmp_path):
    docs = objects(render(
        tmp_path,
        {"batch": {"periodSeconds": 60, "config": CONFIG}},
        image={"repository": "ghcr.io/warpstreamlabs/bento", "tag": "1.22.0", "digest": "sha256:abc"},
        podSecurityContext={"runAsNonRoot": True, "runAsUser": 10001},
        securityContext={"readOnlyRootFilesystem": True, "allowPrivilegeEscalation": False},
    ))
    pod = docs["CronJob"]["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    assert pod["securityContext"]["runAsNonRoot"] is True
    container = pod["containers"][0]
    assert container["image"].endswith("@sha256:abc")
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    # readOnlyRootFilesystem without a writable /tmp is a Bento that cannot buffer.
    assert {m["mountPath"] for m in container["volumeMounts"]} == {"/config", "/tmp"}


@requires_helm
def test_a_meshed_run_can_finish(tmp_path):
    """An injected proxy is an ordinary container that never exits, so a meshed Job stays
    Active until its deadline and the run counts as failed. As a native sidecar the proxy is
    stopped with the last main container."""
    docs = objects(render(
        tmp_path,
        {"batch": {"periodSeconds": 60, "config": CONFIG}},
        serviceMesh={"enabled": True},
    ))
    annotations = docs["CronJob"]["spec"]["jobTemplate"]["spec"]["template"]["metadata"]["annotations"]
    assert annotations["linkerd.io/inject"] == "enabled"
    assert annotations["config.alpha.linkerd.io/proxy-enable-native-sidecar"] == "true"


@requires_helm
def test_nothing_renders_without_a_pipeline(tmp_path):
    """The default is an empty map: a release that owns no CronJob owns nothing at all."""
    result = render(tmp_path, {})
    assert result.returncode == 0, result.stderr
    assert [d for d in yaml.safe_load_all(result.stdout) if d] == []
