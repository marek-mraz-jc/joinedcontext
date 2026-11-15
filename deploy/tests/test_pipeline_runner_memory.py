"""The pipeline runner's Go soft memory limit follows its container limit (T-3537)."""

import pathlib
import re

import pytest

ENVIRONMENTS = ("local", "dev", "production")


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_runner_s_gomemlimit_is_65_percent_of_its_limit_under_the_smoke_line(rendered, environment):
    # 75 % left dev's peak working set 319 Mi above GOMEMLIMIT, past smoke's 80 % line (T-5797).
    runner = next(d for d in rendered(environment) if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    container = next(c for c in runner["spec"]["template"]["spec"]["containers"] if c["name"] == "pipeline-runner-runner")
    limit = container["resources"]["limits"]["memory"]
    units = {"Mi": 2**20, "Gi": 2**30}
    limit_bytes = int(limit[:-2]) * units[limit[-2:]]
    env = {e["name"]: e.get("value") for e in container["env"]}
    assert env["GOMEMLIMIT"] == str(limit_bytes * 65 // 100), limit


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_runner_times_with_histograms_not_summaries(rendered, environment):
    # Per-label summaries held 45 % of the runner's heap on dev; the Portal reads p99 from the
    # buckets (T-3625), so the summaries never come back (T-3537).
    runner = next(d for d in rendered(environment) if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    container = next(c for c in runner["spec"]["template"]["spec"]["containers"] if c["name"] == "pipeline-runner-runner")
    args = container["args"]
    assert "metrics.prometheus.use_histogram_timing=true" in args, args
    assert args[args.index("metrics.prometheus.use_histogram_timing=true") - 1] == "--set", args
    assert "http.debug_endpoints=true" not in args, args


@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_runner_keeps_only_the_series_the_portal_reads(rendered, environment):
    # Bento keeps a deleted stream's series until it restarts: without the mapping every scheduled
    # pipeline's name held about 900 lines, 2.64 GiB on prod (T-5282, T-5285). The families are what
    # joinedcontext-portal src/api/pipelines.rs `family` reads; the writes' latency by `is_write_label`.
    runner = next(d for d in rendered(environment) if d.get("kind") == "Deployment" and d["metadata"]["name"] == "pipeline-runner")
    container = next(c for c in runner["spec"]["template"]["spec"]["containers"] if c["name"] == "pipeline-runner-runner")
    args = container["args"]
    mapping = next(a for a in args if a.startswith("metrics.mapping="))
    assert args[args.index(mapping) - 1] == "--set", args
    assert "," not in mapping, "--set splits on commas"
    for family in ("input_received", "output_sent", "output_error", "processor_error", "buffer_backlog", "output_latency_ns"):
        assert family in mapping, family
    assert '@label.or("").re_match("^(.*_)?write_[0-9]+$")' in mapping
    assert "@stream != null" in mapping, "series outside a stream (resources, http) stay whole"


def test_the_seeded_profile_runs_three_batch_pipelines_at_once():
    # One batch run at a time for about 5,400 a day left slovensko's pipelines, queued last by
    # name, without a run on dev or prod for a day (T-5778); the runner's 3Gi holds three runs
    # beside its resident streams. The development values reach prod (valuesProfile).
    values = pathlib.Path(__file__).resolve().parent.parent / "components/portal/values/portal/development-values.yaml.gotmpl"
    found = re.findall(r'^  JC_PORTAL_PIPELINES_BATCH_CONCURRENCY: "(\d+)"$', values.read_text(), re.M)
    assert found == ["3"], found
