"""T-4679: the catalogue lives on opendata.{domain}; data.{domain} only answers 301.

A seed, pipeline, CORS list or page that names data.{domain} sends a browser through the redirect,
which a CORS fetch does not survive, so the old host appears nowhere but in the redirect itself.
"""

import re
import subprocess

import pytest

from test_dev_recipes import PROJECT_ROOT
from test_production_is_not_dev import strings

OLD_HOST = r"(^|[^a-z0-9.-])data\.dev\.joinedcontext\.com"
# The redirect's own tests: the old host on the certificate and parity's undo of old seeds.
NAMES_THE_REDIRECT = {"tests/test_apisix_edge.py", "tests/test_parity.py", "tests/test_catalogue_host.py"}


def test_no_file_names_the_old_catalogue_host():
    found = subprocess.run(
        ["git", "grep", "-lE", OLD_HOST], cwd=PROJECT_ROOT, capture_output=True, text=True, check=False
    ).stdout.split()
    assert set(found) <= NAMES_THE_REDIRECT, f"name opendata.{{domain}} instead: {sorted(set(found) - NAMES_THE_REDIRECT)}"


@pytest.mark.parametrize(("env", "domain"), [("dev", "dev.joinedcontext.com"), ("smartcity", "smartcity.joinedcontext.com")])
def test_only_the_redirect_names_the_old_host(rendered, env, domain):
    old = re.compile(r"(^|[^a-z0-9.-])data\." + re.escape(domain) + r"\b")
    redirect = f"opendata.{domain}$request_uri"
    # The 301 route's own host, in the APISIX config that also carries the redirect.
    route_host = f'host: "data.{domain}"'
    named = set()
    for doc in rendered(env):
        if doc.get("kind") in {"Ingress", "Certificate"}:
            continue  # the old host keeps its route and certificate, so the 301 answers on https
        for path, text in strings(doc):
            for line in text.splitlines():
                if not old.search(line) or redirect in line:
                    continue
                if line.strip() == route_host and redirect in text:
                    continue
                named.add(f"{doc.get('kind')}/{doc['metadata']['name']}: {line.strip()[:120]}")
    assert not named, sorted(named)
