"""T-4063, T-4064: dev's branding names the public documentation the Portal serves at
`/docs/{API|User-Guide}/{page}`, so the catalogue's usage texts link their guides (EP-108): the
WebSocket resource's `{base}/API/07-websocket` among them. The Portal and the CKAN theme read
the same block, so both are checked."""

import json
import shutil

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

DOCS = "https://dev.joinedcontext.com/docs"


def _configmap_with(docs, key):
    return [d for d in docs if d.get("kind") == "ConfigMap" and key in (d.get("data") or {})]


@requires_helmfile
def test_dev_branding_names_the_public_documentation(rendered):
    docs = rendered("dev")
    portal = _configmap_with(docs, "branding.yaml")
    assert portal, "the Portal's branding ConfigMap is rendered"
    for configmap in portal:
        assert yaml.safe_load(configmap["data"]["branding.yaml"])["documentationBaseUrl"] == DOCS
    theme = _configmap_with(docs, "branding.json")
    assert theme, "the catalogue theme's branding ConfigMap is rendered"
    for configmap in theme:
        assert json.loads(configmap["data"]["branding.json"])["documentationBaseUrl"] == DOCS
