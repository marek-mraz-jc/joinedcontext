"""T-4319: every Endpoint a project repository holds serves MCP, and OGC API Features when its
space holds located entities (owner 2026-10-16: "NGSI-LD, MCP, and OGC API where the data has a
location"). The audience and the Policy still decide who may call each surface.

The manifests are the ones the forge is seeded with: the projects' own (components/context-
gateway/seed) and the sample Apps' generated Endpoints (components/gitea/apps). Live residue an
e2e run leaves in a forge repository is not in either and is not judged here.
"""

import glob
from pathlib import Path

from conftest import load_yaml_all

PROJECT_ROOT = Path(__file__).resolve().parent.parent
SOURCES = (
    "components/context-gateway/seed/*/*.yaml",
    "components/gitea/apps/**/endpoints/*.yaml",
)

#: Spaces whose entities carry a location, from the antares DB on dev (count(location) by
#: tenant, T-4319 inventory, 2026-10-16 10:35Z), plus the two the import fills with geometries
#: from Esri layers (bbsk-otvorene-data) and the bike stations' space (mobility).
LOCATED = {
    "banskabystrica-verejne", "bbsk-registre", "bbsk-otvorene-data", "events", "helsinki",
    "mobility", "ovzdusie", "praha-mesto", "weather", "zilina-verejne",
}


def endpoints() -> list[tuple[str, str, str, list[str]]]:
    found = []
    for pattern in SOURCES:
        for path in sorted(glob.glob(str(PROJECT_ROOT / pattern), recursive=True)):
            for doc in load_yaml_all(Path(path).read_text()):
                if not isinstance(doc, dict) or doc.get("kind") != "Endpoint":
                    continue
                spec = doc["spec"]
                space = spec["contextSpaceRef"]
                space = space["name"] if isinstance(space, dict) else space
                served = spec.get("enabledRepresentations") or []
                found.append((str(Path(path).relative_to(PROJECT_ROOT)), doc["metadata"]["name"], space, served))
    return found


def test_the_walk_finds_the_endpoints():
    found = endpoints()
    assert len(found) >= 59, len(found)
    assert {"helsinki", "praha-mesto", "bbsk-kraj"} <= {space for _, _, space, _ in found}


def test_every_endpoint_serves_mcp():
    missing = [f"{path}: {name}" for path, name, _, served in endpoints() if "mcp" not in served]
    assert not missing, missing


def test_every_endpoint_of_a_located_space_serves_ogc_api_features():
    missing = [
        f"{path}: {name} ({space})"
        for path, name, space, served in endpoints()
        if space in LOCATED and "ogc-features" not in served
    ]
    assert not missing, missing


def test_no_endpoint_serves_a_zip():
    """T-4324 (owner 2026-10-16): a catalogue offers data, never a ZIP of it."""
    zipped = [f"{path}: {name}" for path, name, _, served in endpoints() if "zip" in served]
    assert not zipped, zipped
