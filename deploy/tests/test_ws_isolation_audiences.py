"""The conformance account's audiences for the WebSocket isolation run (T-4909).

Conformance e2e journey 48 proposes two sandbox Endpoints under fixed slugs and reaches them with
the token of `banskabystrica-conformance`; the gateway accepts a token only when its audience names
the slug. The slugs stay reserved for that run: no seeded Endpoint may hold one, or the conformance
token would reach it.
"""

from pathlib import Path

import yaml
from conftest import seed_documents

ROOT = Path(__file__).resolve().parent.parent
CLIENTS = ROOT / "components/context-gateway/keycloak-clients.yaml"
SEED = ROOT / "components/context-gateway/seed"
SEEDED_SLUG = "dv7iyelohbhylxfilexaneobbl"  # the conformance space's Endpoint (F-BNT1-002)
ISOLATION_SLUGS = {
    "hcwfp3wxr45cjxjwqsfcyxgv4klxb2ob",
    "mieqdukdwfv24izi5qxkia3e3qolhbvp",
}


def audiences() -> set[str]:
    client = yaml.safe_load(CLIENTS.read_text())["banskabystrica-conformance"]
    return {
        m["config"]["included.custom.audience"]
        for m in client["rawValues"]["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }


def test_the_conformance_token_names_the_seeded_endpoint_and_the_two_isolation_slugs_only():
    assert audiences() == {SEEDED_SLUG} | ISOLATION_SLUGS


def test_the_conformance_client_stays_a_development_test_object():
    client = yaml.safe_load(CLIENTS.read_text())["banskabystrica-conformance"]
    assert client["profiles"] == ["development"] and client["test"] is True
    assert client["rawValues"]["serviceAccountsEnabled"] is True
    assert client["rawValues"]["standardFlowEnabled"] is False


def test_no_seeded_endpoint_holds_an_isolation_slug():
    held = {}
    for path, doc in seed_documents():
        if isinstance(doc, dict) and doc.get("kind") == "Endpoint":
            held[doc["spec"]["slug"]] = path
    assert not ISOLATION_SLUGS & held.keys(), {
        s: str(held[s]) for s in ISOLATION_SLUGS & held.keys()
    }
