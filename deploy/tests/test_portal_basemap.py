"""The basemap under generated maps comes through the Portal on dev (AP-67): the Portal is told
the tile template and its attribution, and its egress rule reaches the tile host on 443."""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@requires_helmfile
def test_the_portal_proxies_an_https_tile_template_with_its_attribution(rendered):
    docs = rendered("dev")
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    env = {e["name"]: e.get("value") for e in portal["spec"]["template"]["spec"]["containers"][0]["env"]}
    url = env["JC_BASEMAP_URL"]
    assert url.startswith("https://")
    assert all(slot in url for slot in ("{z}", "{x}", "{y}"))
    assert env["JC_BASEMAP_ATTRIBUTION"].strip()
    labels = portal["spec"]["template"]["metadata"]["labels"]
    egress_ports = {
        port["port"]
        for policy in docs
        if policy.get("kind") == "NetworkPolicy"
        and policy["spec"].get("podSelector", {}).get("matchLabels", {}).items() <= labels.items()
        and policy["spec"].get("podSelector", {}).get("matchLabels")
        for rule in policy["spec"].get("egress", [])
        for port in rule.get("ports", [])
    }
    assert 443 in egress_ports
