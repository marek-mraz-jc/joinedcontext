"""T-3181 (AG-114, EP-81, MF-52): the data portal's public assistant answers from the whole
catalogue and reads nothing that is not public.

The deployment `helsinki/helsinki-catalogue` answers anyone on opendata.dev.joinedcontext.com
without signing in (since T-4252 the catalogue pages frame Banská Bystrica's assistant instead). Every source it answers
from is a public KnowledgeSource, every connector a public Endpoint that serves MCP, it is
placed on the data portal alone, and its rate and budget are bounded.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
HELSINKI = SEED / "helsinki"
BODIES = ("Banská Bystrica", "Banská Bystrica Self-Governing Region", "Prague", "Helsinki", "Žilina")


def manifests(kind: str) -> dict[str, dict]:
    found = {}
    for path in sorted(HELSINKI.glob("*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == kind:
                found[doc["metadata"]["name"]] = doc
    return found


def deployment() -> dict:
    return manifests("AssistantDeployment")["helsinki-catalogue"]["spec"]


def test_the_catalogue_assistant_answers_from_the_whole_data_portal():
    spec = deployment()
    assert spec["channel"] == "ckan"
    sources = manifests("KnowledgeSource")
    assert spec["sources"], "the catalogue assistant answers from the catalogue"
    portal = sources["data-portal"]["spec"]
    assert portal["source"] == "ckan"
    instance = manifests("CkanInstance")[portal["ckanInstanceRef"]]["spec"]
    # One CKAN holds every body's catalogue; the source lists it whole, not one organization.
    assert instance["url"] == "https://opendata.dev.joinedcontext.com"
    for body in BODIES:
        assert body in spec["systemPrompt"], body
    assert {"sk", "cs", "en", "fi"} <= set(spec["languages"])
    assert {"sk", "cs", "en", "fi"} <= set(portal["languages"])


def test_a_public_channel_reads_public_sources_and_public_endpoints_only():
    spec = deployment()
    sources = manifests("KnowledgeSource")
    for name in spec["sources"]:
        assert sources[name]["spec"].get("visibility") == "public", name
    endpoints = manifests("Endpoint")
    for connector in spec["connectors"]:
        endpoint = endpoints[connector["endpoint"]]["spec"]
        assert endpoint["audience"] == "public", connector["endpoint"]
        assert "mcp" in endpoint["enabledRepresentations"], connector["endpoint"]
        # Read-only tools: the widget answers anyone, so it never writes.
        assert set(connector["tools"]) <= {"list_types", "query_entities", "get_entity"}, connector


def test_it_is_placed_on_the_data_portal_alone_and_its_spending_is_bounded():
    spec = deployment()
    assert spec["allowedOrigins"] == ["https://opendata.dev.joinedcontext.com"]
    assert spec["rateLimit"]["perClientPerMinute"] <= 10
    # The owner keeps model usage down through the cost cap (T-4286, 2026-10-16): a shared
    # token cap of 300k refused everyone after 56 answers, so $3 a day is the guard.
    assert spec["budget"]["costPerDayUsd"] <= 3
    assert spec["budget"]["tokensPerDay"] <= 5_000_000
    assert spec["budget"]["tokensPerConversation"] <= spec["budget"]["tokensPerDay"]


def test_the_source_and_the_deployment_are_seeded_at_their_kinds_paths():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert index["helsinki-knowledge-catalogue.yaml"] == "projects/helsinki/assistant/sources/data-portal.yaml"
    assert (
        index["helsinki-assistant-catalogue.yaml"]
        == "projects/helsinki/assistant/deployments/helsinki-catalogue.yaml"
    )
