"""The Portal and the context gateway keep answering while they roll (T-2927, OPS-07).

Both run one replica on dev. A rollout that stops the old pod before a new one is ready leaves
APISIX with no endpoint, and every App and Open page answers 503/504 until the new pod is up.
So each rolls with maxUnavailable 0 and a surge, and the new pod takes traffic only once its
readiness probe (the Portal's waits for the mirror to load) passes. Readiness must be a
different check from liveness: a liveness probe that waits on the mirror would restart a pod
that is only loading.
"""

import pytest

ROLLING = ("portal", "context-gateway")


@pytest.mark.parametrize("env", ["dev", "production"])
@pytest.mark.parametrize("name", ROLLING)
def test_a_rollout_starts_the_new_pod_before_the_old_one_stops(rendered, env, name):
    found = [
        d for d in rendered(env)
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == name
    ]
    assert len(found) == 1, f"{env} renders {len(found)} Deployments named {name}"
    spec = found[0]["spec"]
    strategy = spec.get("strategy", {})
    assert strategy.get("type", "RollingUpdate") == "RollingUpdate", f"{env}/{name}: {strategy}"
    rolling = strategy.get("rollingUpdate", {})
    assert rolling.get("maxUnavailable") == 0, f"{env}/{name} stops a pod before its successor is ready"
    assert rolling.get("maxSurge") not in (None, 0, "0", "0%"), f"{env}/{name} has no room to surge"

    containers = {c["name"]: c for c in spec["template"]["spec"]["containers"]}
    main = containers.get(name) or next(iter(containers.values()))
    readiness = main.get("readinessProbe")
    assert readiness, f"{env}/{name} takes traffic before it can answer"
    liveness = main.get("livenessProbe")
    if liveness:
        assert readiness.get("httpGet", {}).get("path") != liveness.get("httpGet", {}).get("path"), (
            f"{env}/{name}: liveness and readiness are one check, so a loading pod gets restarted"
        )

    # T-3008: a stopping pod keeps serving until APISIX and the mesh stop sending to it.
    template = spec["template"]
    assert main.get("lifecycle", {}).get("preStop", {}).get("sleep", {}).get("seconds", 0) >= 5, (
        f"{env}/{name} stops listening while the edge still routes to it"
    )
    wait = (template["metadata"].get("annotations") or {}).get("config.alpha.linkerd.io/proxy-wait-before-exit-seconds")
    if (template["metadata"].get("annotations") or {}).get("linkerd.io/inject") == "enabled":
        assert wait and int(wait) > main["lifecycle"]["preStop"]["sleep"]["seconds"], f"{env}/{name}: the proxy stops first"
    assert template["spec"].get("terminationGracePeriodSeconds", 30) > int(wait or 0)


@pytest.mark.parametrize("env", ["dev", "production"])
def test_the_edge_keeps_serving_while_it_rolls(rendered, env):
    """T-3024, OPS-27: every request of every host passes APISIX, one pod on dev. Its chart pauses
    30 s before the stop, but the meshed proxy stopped at once and the kubelet killed at 30 s:
    a roll answered ~3 s of alternating 502s on both hosts. The proxy outlives the pause and the
    grace period outlives the proxy."""
    found = [
        d for d in rendered(env)
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "apisix"
    ]
    assert len(found) == 1, f"{env} renders {len(found)} APISIX Deployments"
    template = found[0]["spec"]["template"]
    gateway = next(c for c in template["spec"]["containers"] if c["name"] == "apisix")
    command = gateway["lifecycle"]["preStop"]["exec"]["command"]
    pause = int(command[-1].removeprefix("sleep "))
    assert pause >= 5, f"{env}: the gateway stops listening while the ingress still routes to it"
    wait = int(template["metadata"]["annotations"]["config.alpha.linkerd.io/proxy-wait-before-exit-seconds"])
    assert wait > pause, f"{env}: the mesh proxy stops before the gateway's pause ends"
    assert template["spec"]["terminationGracePeriodSeconds"] > wait, (
        f"{env}: the kubelet kills the pod before the proxy's wait ends"
    )


def test_every_jc_prod_container_is_restarted_when_it_stops_answering(rendered):
    """G-16, T-6727: a hung edge or service is restarted, not left in place. Both probes on every
    long-running container of jc-prod; liveness gives up later than readiness, so a busy pod
    leaves the Service before the kubelet restarts it."""
    missing = []
    for d in rendered("jc-prod"):
        if d.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet"):
            continue
        for c in d["spec"]["template"]["spec"]["containers"]:
            where = f"{d['metadata']['name']}/{c['name']}"
            if not (c.get("livenessProbe") and c.get("readinessProbe")):
                missing.append(where)
                continue
            if d["metadata"]["name"] == "apisix":
                live, ready = c["livenessProbe"], c["readinessProbe"]
                assert live["periodSeconds"] * live["failureThreshold"] > (
                    ready["periodSeconds"] * ready["failureThreshold"]), f"{where} restarts before it leaves the Service"
    assert not missing, f"no liveness or readiness probe: {missing}"
