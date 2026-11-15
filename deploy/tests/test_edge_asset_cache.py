"""T-4201: CKAN's fingerprinted static files are kept by the browser for a year, errors never.

CKAN answers its `/webassets/*` (a content hash in every name) `no-cache`, so every catalogue
page revalidated each script and stylesheet against the per-address limit. The route carries a
`serverless-post-function` that sets `Cache-Control: public, max-age=31536000, immutable` on a 200
or 304 only. A route's plugin replaces the plugin config's plugin of the same name, so the
config's CSP baseline function must come first, unchanged. (The Portal's `/assets/*` needs no
route of its own: the Portal answers them immutable itself, src/assets.rs, and dev-smoke checks it.)

The first tests read the committed configuration; the last runs the pinned APISIX image with the
committed functions, because only an answer proves which status gets the header.
"""

import shutil
from pathlib import Path

import pytest
import yaml

from test_apisix_429_contract import answer, pinned_apisix

ROOT = Path(__file__).resolve().parent.parent
IMMUTABLE = "public, max-age=31536000, immutable"
# route file, route, its plugin config's file, the plugin config, the path it serves
ASSET_ROUTES = [
    ("components/ckan/apisix-routes.yaml", "ckan-webassets", "components/ckan/apisix-plugins.yaml", "ckan", "/webassets/*"),
]


def load(path: str) -> dict:
    # The route files are Helm templates; the `{{ … }}` upstreams are plain strings to YAML.
    return yaml.safe_load((ROOT / path).read_text())


@pytest.mark.parametrize("routes,route,plugins,config,uri", ASSET_ROUTES, ids=[r[1] for r in ASSET_ROUTES])
def test_the_asset_route_keeps_its_config_and_adds_the_cache_line(routes, route, plugins, config, uri):
    entry = load(routes)[route]
    assert entry["uri"] == uri
    assert entry.get("pluginConfig", route) == config
    mine = entry["plugins"]["serverless-post-function"]
    theirs = load(plugins)[config]["plugins"]["serverless-post-function"]
    assert mine["phase"] == theirs["phase"] == "header_filter"
    assert mine["functions"][: len(theirs["functions"])] == theirs["functions"], "the CSP baseline, word for word"
    added = mine["functions"][len(theirs["functions"]) :]
    assert len(added) == 1, added
    assert "ngx.status == 200 or ngx.status == 304" in added[0], "an error is never cached"
    assert f'"{IMMUTABLE}"' in added[0]
    # Besides its own bucket (T-4717), the session and the security headers are the config's.
    assert set(entry["plugins"]) == {"serverless-post-function", "limit-count"}, entry["plugins"]
    limit = entry["plugins"]["limit-count"]
    page = load(plugins)[config]["plugins"]["limit-count"]
    assert (limit["count"], limit["key"], limit["rejected_code"]) == (3000, "remote_addr", 429)
    assert (page["count"], page["key"]) == (600, "remote_addr"), "pages and the action API: 10 a second"


def test_no_other_route_is_cached_for_a_year():
    """Only content-hashed paths may be immutable: a page or an API answer would go stale."""
    for path in sorted(ROOT.glob("components/*/apisix-*.yaml")):
        for name, entry in (yaml.safe_load(path.read_text()) or {}).items():
            if IMMUTABLE in yaml.safe_dump(entry or {}):
                assert name in {r[1] for r in ASSET_ROUTES}, f"{path.name}:{name}"


def test_the_pinned_image_caches_a_200_and_not_an_error(tmp_path):
    if not shutil.which("docker"):
        pytest.skip("docker not installed")
    functions = load("components/ckan/apisix-routes.yaml")["ckan-webassets"]["plugins"]["serverless-post-function"]
    plugin = yaml.safe_dump({"serverless-post-function": functions}, width=1000)
    indented = "".join(f"      {line}\n" for line in plugin.splitlines())
    rules = f"""
routes:
  - uri: /webassets/ok.css
    plugins:
      mocking:
        response_status: 200
        response_example: "ok"
        content_type: text/plain
{indented}
    upstream:
      nodes:
        "127.0.0.1:9": 1
      type: roundrobin
  - uri: /webassets/broken.css
    plugins:
{indented}
    upstream:
      nodes:
        "127.0.0.1:9": 1
      type: roundrobin
#END
"""
    with pinned_apisix(tmp_path, rules) as base:
        status, headers, _ = answer(base + "/webassets/ok.css")
        assert status == 200
        assert headers["Cache-Control"] == IMMUTABLE, headers
        assert headers["Content-Security-Policy"].startswith("frame-ancestors 'self'"), "the baseline still runs"
        status, headers, _ = answer(base + "/webassets/broken.css")
        assert status == 502
        assert IMMUTABLE not in headers.get("Cache-Control", ""), headers


def test_the_portal_bundle_counts_in_a_bucket_of_its_own():
    """T-6539: a cold dataset page fetches ~45 chunks; in the page's web-class bucket a few
    visitors behind one address got 429 on a chunk and a broken page. The route keeps the chain
    of portal-public and replaces only its limit-count."""
    entry = load("components/portal/apisix-routes.yaml")["portal-public-assets"]
    assert entry["pluginConfig"] == "portal-public"
    assert "rateClass" not in entry, "the Organization's page rate must not replace this bucket"
    assert set(entry["plugins"]) == {"limit-count"}, entry["plugins"]
    limit = entry["plugins"]["limit-count"]
    assert (limit["count"], limit["time_window"], limit["key"], limit["rejected_code"]) == (3000, 60, "remote_addr", 429)
