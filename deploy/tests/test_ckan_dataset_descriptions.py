"""T-4066: every seeded Endpoint that publishes to CKAN describes itself.

The gateway's DCAT record takes `dct:description` from the Endpoint's own `metadata.description`
and falls back to the whole space's text, which is how the helsinki-transport dataset came to read
about events, beaches and parking. A published dataset says what it serves, in English at least.
"""

from pathlib import Path

import yaml

SEEDS = Path(__file__).resolve().parent.parent / "components"


def published_endpoints():
    for path in sorted(SEEDS.glob("*/seed/**/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Endpoint" \
                    and ((doc.get("spec") or {}).get("publish") or {}).get("ckan"):
                yield path, doc


def test_there_are_published_endpoints():
    assert len(list(published_endpoints())) >= 10


def test_every_published_endpoint_has_its_own_description():
    missing = []
    for path, doc in published_endpoints():
        description = doc["metadata"].get("description") or {}
        text = description if isinstance(description, str) else description.get("en")
        if not (isinstance(text, str) and len(text.strip()) >= 40):
            missing.append(str(path.relative_to(SEEDS)))
    assert missing == []


def test_transport_describes_vehicles_stops_and_alerts():
    (doc,) = [d for p, d in published_endpoints() if d["metadata"]["name"] == "helsinki-transport"]
    text = doc["metadata"]["description"]["en"].lower()
    for word in ("positions", "stops", "lines", "alerts", "park and ride"):
        assert word in text
    assert "beach" not in text and "events" not in text
