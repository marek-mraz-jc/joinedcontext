"""T-5897: the platform's memory requests fit the node it runs on, and its limits stop growing.

dev is one 16 GB node and smartcity (prod) one 24 GB node, both rendering the development profile.
The owner wants the platform to live there with headroom: what this deployment requests stays within
12.5 GiB, which leaves dev 3.5 GiB of its node to k3s, the kernel, the kit's pods (Traefik 216Mi and
Kyverno 247Mi measured on dev 2026-10-28), a helmfile sync and the nightly pipeline window.

The sum counts every pod the render makes: each container of a Deployment, StatefulSet or
DaemonSet times its replicas, each CNPG Cluster instance, the Prometheus and Alertmanager pods the
operator makes from their resources, and one injected linkerd-proxy request per meshed pod (the
namespace annotation the prepare hook writes from global.serviceMesh.proxyResources). Jobs and
CronJobs are left out: they run minutes at a time and hold nothing between runs.

The scheduler also needs room for a roll: a Deployment rolling with a surge starts its new pod
beside the old one, so the node must keep the largest surge pod's request free besides what every
pod books, the kit's pods included (deployer 2026-10-28: at 95 % booked a model-tools surge pod
stayed Pending, "Insufficient memory"). helmfile syncs four releases at once; a second surge pod
waits until another roll frees its old pod, so one surge pod at a time is what must fit.

Limits are overcommitted by design (a limit is the room for a burst, not a reservation), so they
get a ceiling instead of a budget: the measured sum of today, which a raised limit must lower
somewhere else or move here with its reason. The generic `production` profile is for a cluster of
several nodes (three Postgres instances of 8Gi); it gets the same two ceilings so it cannot grow
unseen either.
"""

import math
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
UNITS = {"Ki": 1 / 1024, "Mi": 1, "Gi": 1024, "K": 1000 / 1024**2, "M": 1000**2 / 1024**2, "G": 1000**3 / 1024**2, "": 1 / 1024**2}

NODE_REQUEST_BUDGET_MIB = 12.5 * 1024
# dev's cpx42 node: allocatable 14136272Ki; the pods the render does not make (Traefik, Kyverno,
# cert-manager, CoreDNS, metrics-server, the Linkerd control plane) booked 13222Mi live minus the
# render's 12232Mi on 2026-10-28.
NODE_ALLOCATABLE_MIB = 14136272 / 1024
KIT_REQUESTS_MIB = 990
# prod's cpx52 node (T-4677): allocatable 22172700Ki, kubectl describe node 2026-11-02.
ALLOCATABLE_MIB = {"dev": NODE_ALLOCATABLE_MIB, "smartcity": 22172700 / 1024}
# (requests, limits) ceilings in MiB, the sums of 2026-10-28 rounded up to 256Mi. dev's limit moved
# on 2026-11-02 for observability-collector (T-6431): 512Mi plus its proxy's 250Mi, the price of
# gateway MCP calls in a project's Activity; it books 128Mi plus its proxy's 10Mi of requests. It
# moved again on 2026-11-02 for Grafana (T-6513): 256Mi plus its proxy's 250Mi, the project
# dashboards on dev; it books 138Mi of requests.
# smartcity's limit moved on 2026-11-02 for Decidim (T-6448, owner's order): web 1274Mi, Sidekiq
# 1018Mi and Redis 410Mi with their proxies, Postgres 2Gi -> 2560Mi for their connections; it books
# 1182Mi of requests. It moved again on 2026-11-03 for dev's components (T-6536, owner's order:
# stage and prod run what dev runs): observability-collector, Grafana, ai and sandbox-reaper with
# their proxies, 47558Mi rendered.
CEILINGS = {
    "dev": (NODE_REQUEST_BUDGET_MIB, 45824),
    "smartcity": (NODE_REQUEST_BUDGET_MIB, 47616),
    "production": (37120, 67584),
}


def mib(quantity) -> float:
    match = re.fullmatch(r"(\d+(?:\.\d+)?)(Ki|Mi|Gi|K|M|G)?", str(quantity))
    assert match, f"not a Kubernetes memory quantity: {quantity!r}"
    return float(match[1]) * UNITS[match[2] or ""]


def proxy_mib() -> tuple[float, float]:
    mesh = yaml.safe_load((ROOT / "defaults/environment/global.yaml").read_text())["global"]["serviceMesh"]
    resources = mesh["proxyResources"]
    return mib(resources["memoryRequest"]), mib(resources["memoryLimit"])


def memory(resources: dict | None) -> tuple[float, float]:
    resources = resources or {}
    return (
        mib(resources.get("requests", {}).get("memory", 0)),
        mib(resources.get("limits", {}).get("memory", 0)),
    )


def pods(docs: list[dict]):
    """(name, replicas, meshed, [(request, limit) per container]) for every long-running pod."""
    for d in docs:
        kind, spec, name = d.get("kind"), d.get("spec") or {}, d.get("metadata", {}).get("name")
        if kind in ("Deployment", "StatefulSet", "DaemonSet"):
            template = spec["template"]
            annotations = (template.get("metadata") or {}).get("annotations") or {}
            meshed = annotations.get("linkerd.io/inject") != "disabled" and d["metadata"].get("namespace") != "kube-system"
            replicas = 1 if kind == "DaemonSet" else spec.get("replicas", 1)
            yield name, replicas, meshed, [memory(c.get("resources")) for c in template["spec"]["containers"]]
        elif kind == "Cluster" and "postgresql" in spec:
            yield name, spec.get("instances", 1), True, [memory(spec.get("resources"))]
        elif kind in ("Prometheus", "Alertmanager"):
            yield name, spec.get("replicas", 1), True, [memory(spec.get("resources"))]


def pod_request(containers, meshed: bool) -> float:
    return sum(r for r, _ in containers) + (proxy_mib()[0] if meshed else 0)


def largest_surge(docs: list[dict]) -> tuple[float, str]:
    """The largest request a rolling Deployment adds beside its old pods while it rolls."""
    largest = (0.0, "none")
    rolling = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "Deployment"}
    for name, replicas, meshed, containers in pods(docs):
        spec = rolling.get(name, {}).get("spec")
        strategy = (spec or {}).get("strategy") or {}
        if spec is None or strategy.get("type", "RollingUpdate") != "RollingUpdate":
            continue
        surge = str((strategy.get("rollingUpdate") or {}).get("maxSurge", "25%"))
        count = math.ceil(replicas * int(surge[:-1]) / 100) if surge.endswith("%") else int(surge)
        largest = max(largest, (count * pod_request(containers, meshed), name))
    return largest


def sums(docs: list[dict]) -> tuple[float, float, list[str]]:
    _, proxy_limit = proxy_mib()
    requests = limits = 0.0
    rows = []
    for name, replicas, meshed, containers in pods(docs):
        request = pod_request(containers, meshed)
        limit = sum(l for _, l in containers) + (proxy_limit if meshed else 0)
        requests += replicas * request
        limits += replicas * limit
        rows.append(f"{name} x{replicas}: {request:.0f}/{limit:.0f}Mi")
    return requests, limits, rows


def test_the_sum_counts_containers_replicas_and_proxies():
    docs = [
        {"kind": "Deployment", "metadata": {"name": "a", "namespace": "x"}, "spec": {"replicas": 2, "template": {"spec": {"containers": [
            {"resources": {"requests": {"memory": "100Mi"}, "limits": {"memory": "1Gi"}}},
            {"resources": {"requests": {"memory": "28Mi"}}},
        ]}}}},
        {"kind": "DaemonSet", "metadata": {"name": "b", "namespace": "kube-system"}, "spec": {"template": {"spec": {"containers": [
            {"resources": {"requests": {"memory": "24Mi"}, "limits": {"memory": "96Mi"}}},
        ]}}}},
        {"kind": "Cluster", "metadata": {"name": "pg"}, "spec": {"instances": 3, "postgresql": {}, "resources": {"requests": {"memory": "1Gi"}, "limits": {"memory": "2Gi"}}}},
        {"kind": "Prometheus", "metadata": {"name": "p"}, "spec": {"resources": {"requests": {"memory": "512Mi"}}}},
        {"kind": "CronJob", "metadata": {"name": "c"}, "spec": {}},
    ]
    proxy_request, proxy_limit = proxy_mib()
    requests, limits, _ = sums(docs)
    assert requests == 2 * (128 + proxy_request) + 24 + 3 * (1024 + proxy_request) + 512 + proxy_request
    assert limits == 2 * (1024 + proxy_limit) + 96 + 3 * (2048 + proxy_limit) + proxy_limit


def test_a_raised_request_goes_red():
    big = {"kind": "Deployment", "metadata": {"name": "big", "namespace": "x"}, "spec": {"template": {"spec": {"containers": [
        {"resources": {"requests": {"memory": "13Gi"}, "limits": {"memory": "13Gi"}}},
    ]}}}}
    assert sums([big])[0] > NODE_REQUEST_BUDGET_MIB


def deployment(name, memory, strategy=None):
    spec = {"template": {"spec": {"containers": [{"resources": {"requests": {"memory": memory}}}]}}}
    if strategy:
        spec["strategy"] = strategy
    return {"kind": "Deployment", "metadata": {"name": name, "namespace": "x"}, "spec": spec}


def test_the_largest_surge_skips_recreate_and_zero_surge():
    proxy_request, _ = proxy_mib()
    docs = [
        deployment("rolls", "512Mi"),
        deployment("recreates", "2Gi", {"type": "Recreate"}),
        deployment("no-surge", "2Gi", {"type": "RollingUpdate", "rollingUpdate": {"maxSurge": 0, "maxUnavailable": 1}}),
        {"kind": "StatefulSet", "metadata": {"name": "sts", "namespace": "x"}, "spec": {"template": {"spec": {"containers": [
            {"resources": {"requests": {"memory": "4Gi"}}},
        ]}}}},
    ]
    assert largest_surge(docs) == (512 + proxy_request, "rolls")


def test_a_node_without_room_for_the_surge_goes_red():
    docs = [deployment("full", f"{NODE_ALLOCATABLE_MIB - KIT_REQUESTS_MIB - 100:.0f}Mi"), deployment("rolls", "200Mi")]
    surge, _ = largest_surge(docs)
    assert sums(docs)[0] + KIT_REQUESTS_MIB + surge > NODE_ALLOCATABLE_MIB


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_the_node_keeps_room_for_the_largest_rolling_surge(rendered, env):
    docs = rendered(env)
    requests, _, _ = sums(docs)
    surge, name = largest_surge(docs)
    booked = requests + KIT_REQUESTS_MIB + surge
    assert booked <= ALLOCATABLE_MIB[env], (
        f"{env}: requests {requests:.0f}Mi + kit {KIT_REQUESTS_MIB}Mi + {name}'s surge pod {surge:.0f}Mi"
        f" = {booked:.0f}Mi over the node's {ALLOCATABLE_MIB[env]:.0f}Mi allocatable"
    )


@pytest.mark.parametrize("env", sorted(CEILINGS))
def test_requests_and_limits_stay_within_the_budget(rendered, env):
    requests, limits, rows = sums(rendered(env))
    request_ceiling, limit_ceiling = CEILINGS[env]
    table = "\n".join(sorted(rows))
    assert requests <= request_ceiling, f"{env}: requests {requests:.0f}Mi over {request_ceiling:.0f}Mi\n{table}"
    assert limits <= limit_ceiling, f"{env}: limits {limits:.0f}Mi over the {limit_ceiling:.0f}Mi ceiling\n{table}"
