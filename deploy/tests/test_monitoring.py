"""The monitoring component as production renders it: what only it can get wrong (T-1669, OPS-16).

`test_production_baseline.py` already holds its targets, interval, paths, namespaces, alerts and
that metrics stay inside the cluster. What it does not hold is the join with the other components:
a monitor selects a Service by label and names its port, and a component that renames either
leaves its monitor scraping nothing, which reads the same as a target that is down."""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


def of_kind(docs, kind):
    return [d for d in docs if d.get("kind") == kind]


def ours(production):
    """The monitors this component renders (the charts of APISIX and Keycloak render their own)."""
    return [
        m for m in of_kind(production, "ServiceMonitor")
        if (m["metadata"].get("labels") or {}).get("app.kubernetes.io/name") == "monitoring"
    ]


@requires_helmfile
def test_the_component_renders_monitors_in_production(production):
    assert ours(production), "production renders no monitor of the monitoring component"


@requires_helmfile
def test_every_monitor_selects_a_service_that_carries_the_port_it_names(production):
    services = of_kind(production, "Service")
    for monitor in ours(production):
        spec = monitor["spec"]
        wanted = spec["selector"]["matchLabels"]
        namespaces = spec["namespaceSelector"]["matchNames"]
        matching = [
            s for s in services
            if s["metadata"].get("namespace") in namespaces
            and wanted.items() <= (s["metadata"].get("labels") or {}).items()
        ]
        name = monitor["metadata"]["name"]
        assert matching, f"{name} selects {wanted} and no Service carries those labels"
        for endpoint in spec["endpoints"]:
            ports = {p.get("name") for s in matching for p in s["spec"]["ports"]}
            assert endpoint["port"] in ports, f"{name} scrapes port {endpoint['port']!r}; the Service names {sorted(ports)}"


@requires_helmfile
def test_every_monitor_lives_in_the_namespace_it_scrapes(production):
    for monitor in ours(production):
        assert monitor["spec"]["namespaceSelector"]["matchNames"] == [monitor["metadata"]["namespace"]]


@requires_helmfile
def test_no_monitor_scrapes_over_a_scheme_the_target_does_not_serve(production):
    for monitor in ours(production):
        for endpoint in monitor["spec"]["endpoints"]:
            assert endpoint.get("scheme", "http") == "http", monitor["metadata"]["name"]
            assert "bearerTokenSecret" not in endpoint and "basicAuth" not in endpoint, (
                "a scrape of a cluster-internal port carries no credential"
            )
