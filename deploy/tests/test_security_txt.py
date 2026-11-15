"""OPS-52, T-1721: every host the edge serves answers RFC 9116 `/.well-known/security.txt`.

The file names where a vulnerability is reported and until when that holds. The edge answers
it by itself, so it is there whatever else a host serves, and a production instance that has
not set its own contact does not render.
"""

import re
import shutil
from datetime import datetime, timedelta, timezone

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

#: The render test fails this long before the date passes, so the file is renewed in time.
RENEW_BEFORE = timedelta(days=30)


def standalone(docs: list[dict]) -> dict:
    return yaml.safe_load(next(
        d for d in docs
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )["data"]["apisix.yaml"])


def served(config: dict) -> tuple[dict, dict]:
    route = next(r for r in config["routes"] if r["id"] == "security-txt")
    plugins = next(p for p in config["plugin_configs"] if p["id"] == route["plugin_config_id"])["plugins"]
    return route, plugins


def fields(body: str) -> dict[str, str]:
    return dict(line.split(": ", 1) for line in body.splitlines() if line)


@requires_helmfile
def test_dev_answers_security_txt_on_every_host_with_the_owners_contact(rendered):
    """OPS-52: the path on every host, above every `/*` route, answered at the edge as text."""
    config = standalone(rendered("dev"))
    route, plugins = served(config)

    assert route["uri"] == "/.well-known/security.txt"
    assert "host" not in route and "hosts" not in route, "bound to one host, the others would miss it"
    assert route["methods"] == ["GET", "HEAD"]
    catch_all = [r["priority"] for r in config["routes"] if r["uri"] == "/*" and "priority" in r]
    assert route["priority"] > max(catch_all)

    abort = plugins["fault-injection"]["abort"]
    assert abort["http_status"] == 200
    body = fields(abort["body"])
    # The address itself is private (the private environments/tests, T-6984, T-6982).
    assert re.fullmatch(r"mailto:[^@\s]+@[^@\s]+", body["Contact"])
    assert plugins["response-rewrite"]["headers"]["set"]["Content-Type"] == "text/plain; charset=utf-8"


@requires_helmfile
def test_the_dev_file_is_valid_for_a_month_and_at_most_a_year(rendered):
    """RFC 9116 §2.5.5: `Expires` is required and should be less than a year ahead."""
    _, plugins = served(standalone(rendered("dev")))
    expires = datetime.fromisoformat(fields(plugins["fault-injection"]["abort"]["body"])["Expires"])
    now = datetime.now(timezone.utc)
    assert expires - now > RENEW_BEFORE, f"security.txt expires {expires:%Y-%m-%d}: set global.securityTxt.expires a year ahead"
    assert expires - now <= timedelta(days=366)


@requires_helmfile
def test_a_production_render_without_a_contact_stops(rendered_variant):
    """OPS-52: a production instance never serves a security.txt that names nobody."""
    def without_contact(root):
        path = root / "deployment/environments/production/global.yaml.gotmpl"
        text = path.read_text()
        assert "contact: mailto:security@platform.example.org" in text
        path.write_text(text.replace("contact: mailto:security@platform.example.org", 'contact: ""'))

    with pytest.raises(AssertionError, match="global.securityTxt.contact is empty"):
        rendered_variant("production", without_contact)


@requires_helmfile
def test_jc_prod_is_judged_by_the_production_guard_under_the_development_profile(rendered_variant):
    """T-6720, F-3: jc-prod serves the smartcity hosts with the development profile's sizes; the
    guard judges it all the same, so an approver without a one-time code stops its render."""
    def without_mfa(root):
        path = root / "deployment/environments/jc-prod/global.yaml.gotmpl"
        text = path.read_text()
        assert "  profile: development\n" in text and "  productionGuard: true\n" in text
        path.write_text(text.replace("  productionGuard: true\n", "  productionGuard: true\n  approverMfa: false\n"))

    with pytest.raises(AssertionError, match="global.approverMfa is false"):
        rendered_variant("jc-prod", without_mfa)
