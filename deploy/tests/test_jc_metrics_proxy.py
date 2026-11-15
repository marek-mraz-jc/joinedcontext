"""jc-prod's daiya reads dev's Prometheus only through prom-label-proxy, which pins every query to
jc-prod's series (jc-prod-metrics). The proxy runs with the arguments the jc-prod render gives it
against a real Prometheus holding a jc-prod and a dev series of the same metric; every read API
the Prometheus datasource uses answers jc-prod's series alone, a query naming dev's namespace
included. Needs docker (skips without it)."""

import json
import shutil
import socket
import subprocess
import time
import urllib.parse
import urllib.request
from pathlib import Path

import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-jc-metrics-proxy")

ROOT = Path(__file__).resolve().parent.parent
PROXY_URL = "http://jc-metrics-proxy.jc-prod.svc.cluster.local:8080"


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name
             and d["metadata"].get("namespace") == "jc-prod"]
    assert len(found) == 1, f"{kind}/{name} in jc-prod: {len(found)}"
    return found[0]


def _get(url: str):
    with urllib.request.urlopen(url, timeout=5) as r:
        return json.load(r)


def test_jc_prod_daiya_reads_the_proxy_alone(rendered):
    docs = rendered("jc-prod")
    toml = _one(docs, "ConfigMap", "jc")["data"]["jc.toml"]
    assert f'prometheus_reach = "{PROXY_URL}"' in toml
    ds = yaml.safe_load(_one(docs, "ConfigMap", "jc")["data"]["daiya-prometheus.yaml"])["datasources"]
    assert [(d["uid"], d["orgId"], d["url"]) for d in ds] == [("prometheus", 1, PROXY_URL)]
    args = _one(docs, "Deployment", "jc-metrics-proxy")["spec"]["template"]["spec"]["containers"][0]["args"]
    assert "--label=namespace" in args and "--label-value=jc-prod" in args
    # jc-prod's own rule set alone (T-6727): dev's Prometheus evaluates it beside dev's cluster-wide
    # rules, so jc-prod renders no copy of them (test_alert_rules checks each rule is jc-prod-scoped).
    rules = [d for d in docs if d.get("kind") == "PrometheusRule"]
    assert [r["metadata"]["name"] for r in rules] == ["monitoring-monitoring-alerts"]
    assert [g["name"] for g in rules[0]["spec"]["groups"]] == ["jc-prod"]


@pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")
def test_a_query_through_the_proxy_answers_jc_prod_series_only(rendered, tmp_path):
    docs = rendered("jc-prod")
    container = _one(docs, "Deployment", "jc-metrics-proxy")["spec"]["template"]["spec"]["containers"][0]
    images = yaml.safe_load((ROOT / "components/prometheus/images.yaml").read_text())["prometheus"]["prometheus"]
    prom_image = f"{images['repository']}:{images['tag']}@{images['digest']}"
    prom, proxy = _free_port(), _free_port()
    # Prometheus scrapes itself twice: once labelled as a jc-prod target, once as a dev one.
    (tmp_path / "prometheus.yml").write_text(yaml.safe_dump({
        "global": {"scrape_interval": "1s"},
        "scrape_configs": [
            {"job_name": name, "static_configs": [{"targets": [f"127.0.0.1:{prom}"], "labels": {"namespace": name}}]}
            for name in ("jc-prod", "dev")]}))
    tmp_path.chmod(0o755)
    (tmp_path / "prometheus.yml").chmod(0o644)
    args = [a for a in container["args"] if not a.startswith(("--upstream=", "--insecure-listen-address="))]
    names = [f"jcmp-prom-{prom}", f"jcmp-proxy-{proxy}"]
    try:
        subprocess.run(["docker", "run", "-d", "--name", names[0], "--network", "host", "-v",
                        f"{tmp_path}:/etc/t:ro", prom_image, "--config.file=/etc/t/prometheus.yml",
                        f"--web.listen-address=127.0.0.1:{prom}"], check=True, capture_output=True)
        subprocess.run(["docker", "run", "-d", "--name", names[1], "--network", "host", container["image"],
                        f"--insecure-listen-address=127.0.0.1:{proxy}", f"--upstream=http://127.0.0.1:{prom}",
                        *args], check=True, capture_output=True)
        q = lambda base, path, **p: _get(f"http://127.0.0.1:{base}{path}?{urllib.parse.urlencode(p)}")
        deadline = time.time() + 60
        while True:
            try:
                both = {r["metric"]["namespace"] for r in q(prom, "/api/v1/query", query="up")["data"]["result"]}
            except OSError:
                both = set()
            if both == {"jc-prod", "dev"}:
                break
            assert time.time() < deadline, f"Prometheus holds {both} after 60 s"
            time.sleep(1)
        now = time.time()
        for query in ("up", 'up{namespace="dev"}', 'up{namespace=~".+"}', 'sum by (namespace) (up)'):
            for path, extra in (("/api/v1/query", {}), ("/api/v1/query_range", {"start": now - 30, "end": now, "step": 5})):
                got = {r["metric"].get("namespace") for r in q(proxy, path, query=query, **extra)["data"]["result"]}
                assert got <= {"jc-prod"}, f"{path} {query}: {got}"
        assert q(proxy, "/api/v1/query", query="up")["data"]["result"], "jc-prod's own series answer"
        assert q(proxy, "/api/v1/label/namespace/values")["data"] == ["jc-prod"]
        assert {s["namespace"] for s in q(proxy, "/api/v1/series", **{"match[]": "up"})["data"]} == {"jc-prod"}
        jobs = q(proxy, "/api/v1/label/job/values")["data"]
        assert jobs == ["jc-prod"], jobs
    finally:
        subprocess.run(["docker", "rm", "-f", *names], capture_output=True)
