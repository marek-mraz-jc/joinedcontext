"""The Portal takes traffic only once its mirror holds the repository (OPS-51): the readiness
probe reads the ready route, and the liveness probe stays on the unconditional health route, so
a slow first sync never restarts the pod."""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@requires_helmfile
def test_readiness_reads_the_ready_route_and_liveness_the_health_route(rendered):
    docs = rendered("dev")
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    container = portal["spec"]["template"]["spec"]["containers"][0]
    assert container["readinessProbe"]["httpGet"]["path"] == "/api/v1/ready"
    assert container["livenessProbe"]["httpGet"]["path"] == "/api/v1/health"
