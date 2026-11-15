"""T-4694: the standalone assistant on ai.{domain}. The rendered dev environment carries a
read-only static server, the bundle with the chat host filled in, the route and its certificate
name; the page's policy runs only its own scripts and asks only the chat's host."""

import gzip
import hashlib
import os
import shutil
import subprocess
from pathlib import Path
import re

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
COMPONENT = ROOT / "components/ai"
SITE = COMPONENT / "charts/app/files/site"
HOST = "ai.dev.joinedcontext.com"
API = "https://assistant.dev.joinedcontext.com"


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected one {kind}/{name}, got {len(found)}"
    return found[0]


def test_dev_renders_a_read_only_server_and_its_route_on_its_own_host(dev):
    pod = one(dev, "Deployment", "ai-app")["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False and pod["securityContext"]["runAsNonRoot"] is True
    container = pod["containers"][0]
    assert "@sha256:" in container["image"]
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    one(dev, "Service", "ai-app")
    routes = yaml.safe_load((COMPONENT / "apisix-routes.yaml").read_text().replace("{{", "#{{"))
    assert routes["ai"]["subDomain"] == "ai" and routes["ai"]["uri"] == "/*"
    assert HOST in one(dev, "Certificate", "apisix-edge")["spec"]["dnsNames"]


def test_the_bundle_asks_only_the_chat_host_and_runs_only_its_own_scripts(dev):
    site = one(dev, "ConfigMap", "ai-app-site")["data"]
    assert sorted(site) == ["app.js", "embed.js", "index.html", "visitor.js", "widget.css", "widget.js"]
    page = site["index.html"]
    policy = re.search(r'http-equiv="Content-Security-Policy" content="([^"]+)"', page).group(1)
    assert "script-src 'self';" in policy and f"connect-src {API};" in policy
    assert "frame-src https://viz.dev.joinedcontext.com;" in policy
    assert "unsafe" not in policy and "*" not in policy
    assert f'<meta name="jc-api" content="{API}">' in page
    assert "__API__" not in "".join(site.values()) and "__FRAMES__" not in "".join(site.values())
    assert not re.search(r"<script>|style=", page), "no inline script or style"
    assert sum(len(gzip.compress(text.encode())) for text in site.values()) <= 200 * 1024
    assert not re.search(r"sk-or-v1-|client_secret|PRIVATE KEY", "".join(site.values()))
    conf = one(dev, "ConfigMap", "ai-app")["data"]["default.conf"]
    assert f"connect-src {API};" in conf and "__API__" not in conf
    assert "location = /index.html { return 404; }" in conf


LOCK = COMPONENT / "bundle.lock"


def test_the_committed_bundle_is_the_locked_one():
    """bundle.lock names the platform commit and each file's sha256 (scripts/ai-bundle.sh)."""
    lines = LOCK.read_text().splitlines()
    assert re.fullmatch(r"platform: [0-9a-f]{40}", lines[0])
    locked = dict(reversed(line.split("  ", 1)) for line in lines[1:])
    assert sorted(locked) == sorted(p.name for p in SITE.iterdir())
    for name, digest in locked.items():
        assert hashlib.sha256((SITE / name).read_bytes()).hexdigest() == digest, f"{name} was edited by hand"
    page = (SITE / "index.html").read_text()
    assert "connect-src __API__;" in page and '<meta name="jc-api" content="__API__">' in page


def test_the_locked_platform_commit_builds_the_committed_bundle(tmp_path):
    """CI fetches the locked commit into JC_PLATFORM_DIR; locally the sibling clone serves."""
    platform = Path(os.environ.get("JC_PLATFORM_DIR") or ROOT.parent / "joinedcontext-platform")
    sha = LOCK.read_text().splitlines()[0].split(": ", 1)[1]
    if shutil.which("node") is None or subprocess.run(
        ["git", "-C", str(platform), "cat-file", "-e", f"{sha}^{{commit}}"], capture_output=True
    ).returncode != 0:
        pytest.skip(f"no node, or platform {sha} not in {platform}")
    out = tmp_path / "site"
    subprocess.run([str(ROOT / "scripts/ai-bundle.sh"), str(platform), sha, str(out)], check=True)
    for path in SITE.iterdir():
        assert (out / path.name).read_bytes() == path.read_bytes(), f"{path.name} differs from platform {sha}"
    assert sorted(p.name for p in out.iterdir()) == sorted(p.name for p in SITE.iterdir())


def test_every_answer_of_the_server_states_the_header_floor(dev):
    """T-5107 (ZAP 10055, 10063): nginx drops the server's add_header in a location that sets its
    own, so the page restates Permissions-Policy, and the default policy names the directives that
    do not fall back to default-src."""
    conf = one(dev, "ConfigMap", "ai-app")["data"]["default.conf"]
    assert conf.count("add_header Content-Security-Policy") == conf.count("add_header Permissions-Policy") == 2
    default = re.search(r"\n    add_header Content-Security-Policy \"([^\"]+)\"", conf).group(1)
    assert default == "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"


def test_the_chart_needs_the_domain_alone(tmp_path):
    """T-6218 (R2DEP1 F-021): `host` was required and rendered nowhere; the page's hosts all come
    from `domain`."""
    if shutil.which("helm") is None:
        pytest.skip("no helm")
    chart = Path(__file__).resolve().parent.parent / "components/ai/charts/app"
    result = subprocess.run(["helm", "template", "ai", str(chart), "--set", "domain=example.test"],
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "https://assistant.example.test" in result.stdout
