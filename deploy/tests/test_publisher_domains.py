"""Every publisher's policies name its own domain, never the installation's hel.fi (T-4373).

A visitor and a model read `did:web:hel.fi` on Žilina's data as Helsinki's. No right derives
from it (ADR-N-041); the assigner says who grants, so it is the publisher's own.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
DOMAINS = {"zilina": "zilina.sk", "banskabystrica": "banskabystrica.sk", "bbsk": "bbsk.sk",
           "praha": "praha.eu", "slovensko": "joinedcontext.com", "barcelona": "barcelona.cat",
           "bratislava": "bratislava.sk"}


def test_each_publishers_policies_name_its_own_domain():
    seen = 0
    for project, domain in DOMAINS.items():
        for path in sorted((SEED / project).glob("*.yaml")):
            for doc in yaml.safe_load_all(path.read_text()):
                # An App's policy too: the Portal compiles it with its project's publisher (T-4375, PF-108).
                if isinstance(doc, dict) and doc.get("kind") == "Policy":
                    seen += 1
                    assert doc["spec"]["assigner"] == f"did:web:{domain}", path.name
    assert seen > 20


def test_each_publishers_project_names_its_own_domain():
    """The Project carries the same publisher its Policies name (PF-108, T-4388): the Portal reads
    `spec.domain` for an App's assigner, so a seed without it would fall back to the Organization's."""
    for project, domain in DOMAINS.items():
        projects = [doc for path in sorted((SEED / project).glob("*.yaml"))
                    for doc in yaml.safe_load_all(path.read_text())
                    if isinstance(doc, dict) and doc.get("kind") == "Project"
                    and doc["metadata"]["name"] == project]
        assert len(projects) == 1, project
        assert projects[0]["spec"]["domain"] == domain, project
