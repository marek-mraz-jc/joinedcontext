"""T-2476, T-2839: apps are served on hosts of their own, never on the Portal's host.

On `portal.{domain}` an app's `connect-src 'self'` would be the Portal API, and the session
cookie would ride along with every call it makes. Every App is served on `{name}.apps.{apex}`
(ADR-N-037, AP-133), and the Portal refuses any other host for an App only when it knows the
apex, so every environment has to hand it one: the domain, not the Portal's host. An App pod
reaches its endpoint on the gateway's Service in the cluster (AP-134), so the Portal is handed
that address too, never a public one.
"""

from urllib.parse import urlsplit

import pytest


def portal_env(docs):
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    return {e["name"]: e.get("value") for e in portal["spec"]["template"]["spec"]["containers"][0].get("env", [])}


@pytest.mark.parametrize("environment", ["dev", "local"])
def test_the_portal_knows_the_apps_origin_and_it_is_not_its_own(rendered, environment):
    env = portal_env(rendered(environment))
    apps = urlsplit(env["JC_PORTAL_APPS_URL"])
    portal = urlsplit(env["JC_PORTAL_PUBLIC_URL"])

    assert apps.scheme == "https"
    assert apps.path in ("", "/") and not apps.query, "an origin, nothing after it"
    assert apps.hostname != portal.hostname
    assert portal.hostname == f"portal.{apps.hostname}", "the apex every App's host sits under"

    gateway = urlsplit(env["JC_PORTAL_GATEWAY_URL"])
    assert gateway.scheme == "http" and gateway.hostname.endswith(".svc.cluster.local"), (
        "an App pod calls its endpoint in the cluster, where its NetworkPolicy names the gateway"
    )
    assert gateway.hostname.startswith("context-gateway."), gateway.hostname


@pytest.mark.parametrize("environment", ["dev", "local"])
def test_the_portal_names_every_gateway_pod_by_a_headless_service(rendered, environment):
    """F-POR4-014: a space move copies only once every gateway pod's /metrics shows the hold, so
    the Portal is handed a name with one record per pod, and that Service exists headless in the
    gateway's namespace, selecting the gateway's pods on the port the Portal asks."""
    docs = rendered(environment)
    pods = urlsplit(portal_env(docs)["JC_PORTAL_GATEWAY_PODS_URL"])
    name, namespace = pods.hostname.split(".")[:2]
    service = next(
        d
        for d in docs
        if d.get("kind") == "Service" and d["metadata"]["name"] == name and d["metadata"]["namespace"] == namespace
    )
    assert service["spec"]["clusterIP"] == "None"
    assert service["spec"]["selector"]["app.kubernetes.io/name"] == "context-gateway-gateway"
    assert pods.port in [p["port"] for p in service["spec"]["ports"]]
