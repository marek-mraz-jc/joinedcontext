"""The client an invitation's link returns through (PF-108, T-3237).

The Portal asks the realm's execute-actions e-mail to end on the project's home page, through the
client `JC_PORTAL_INVITATION_CLIENT_ID` names. Keycloak refuses a `redirect_uri` the client does
not list, so the client must already cover every page of the Portal host; naming a client whose
redirect URIs had to be widened for it would be a weaker login, which this test refuses.
"""

from pathlib import Path

import yaml


PROJECT_ROOT = Path(__file__).resolve().parent.parent


def portal_settings(docs):
    for doc in docs:
        data = doc.get("data") or {}
        if "JC_PORTAL_INVITATION_CLIENT_ID" in data:
            return data
        if doc.get("kind") == "Deployment" and doc["metadata"]["name"] == "portal":
            for container in doc["spec"]["template"]["spec"]["containers"]:
                env = {e["name"]: e.get("value") for e in container.get("env", [])}
                if "JC_PORTAL_INVITATION_CLIENT_ID" in env:
                    return env
    return {}


def test_the_invitation_returns_through_the_portals_public_client_without_widening_it(rendered):
    named = portal_settings(rendered("local")).get("JC_PORTAL_INVITATION_CLIENT_ID")
    assert named, "the Portal names no client for an invitation's way back"
    clients = yaml.safe_load((PROJECT_ROOT / "components/portal/keycloak-clients.yaml").read_text())
    client = clients.get(named)
    assert client, f"{named} is no client of the Portal"
    assert client.get("subdomain") == "portal"
    assert client.get("redirectPath") == "/*"
    assert not client.get("additionalRedirectPaths") and not client.get("additionalRedirectUris")
    assert client["rawValues"]["publicClient"] is True
