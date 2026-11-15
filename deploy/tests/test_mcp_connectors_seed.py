"""T-5367: one public MCP connector per publisher, holding every public Endpoint of that publisher by
`select` (MF-55) instead of a hand list, and one for the whole portal (`org/open-data`)."""

from pathlib import Path

import pytest
import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"

# publisher project: its connector's name
PUBLISHERS = {
    "zilina": "otvorene-data",
    "banskabystrica": "otvorene-data",
    "bbsk": "otvorene-data",
    "bratislava": "otvorene-data",
    "helsinki": "otvorene-data",
    "praha": "otvorene-data",
    "barcelona": "dades-obertes",
}


def documents():
    for path in sorted(SEED.rglob("*mcp*.yaml")):
        text = path.read_text()
        if path.name == "index.yaml" or "kind: McpServer" not in text:
            continue
        for doc in yaml.safe_load_all(text):
            if isinstance(doc, dict) and "kind" in doc:
                yield path, doc


def servers() -> dict[tuple[str, str], tuple[Path, dict]]:
    return {(d["metadata"]["namespace"], d["metadata"]["name"]): (p, d) for p, d in documents() if d["kind"] == "McpServer"}


def indexed(path: Path) -> str | None:
    return yaml.safe_load((path.parent / "index.yaml").read_text()).get(path.name)


@pytest.mark.parametrize("project", PUBLISHERS)
def test_each_publisher_has_one_public_connector_selecting_its_own_public_endpoints(project):
    path, server = servers()[(project, PUBLISHERS[project])]
    spec = server["spec"]
    assert spec["audience"] == "public"
    # no hand list: a new public Endpoint joins on the next sync, one turned private leaves
    assert "members" not in spec
    select = spec["select"]
    assert select["accessLevel"] == ["public"]
    scope = select.get("projects") or sorted({s.split("/")[0] for s in select["spaces"]})
    assert scope == [project], select
    assert indexed(path) == f"projects/{project}/mcp/{PUBLISHERS[project]}.yaml"
    for language in ("en",):
        assert "list_endpoints" in server["metadata"]["description"][language]
    # the title is the serverInfo name an AI client shows (EP-92): a name, the description holds the rest
    for language, title in server["metadata"]["title"].items():
        assert len(title) <= 60, (language, title)


def test_bystricas_connector_keeps_the_test_written_air_space_out():
    # public-air (space ovzdusie) holds what the smoke run and the ETSI suite write.
    select = servers()[("banskabystrica", "otvorene-data")][1]["spec"]["select"]
    assert "projects" not in select
    assert not any(s.endswith("/ovzdusie") for s in select["spaces"])


def test_the_portal_wide_connector_selects_every_public_endpoint():
    path, server = servers()[("org", "open-data")]
    spec = server["spec"]
    assert spec["audience"] == "public" and "members" not in spec
    assert spec["select"] == {"accessLevel": ["public"]}
    assert indexed(path) == "mcp/open-data.yaml"


def test_every_public_connector_is_read_only_over_public_endpoints_only():
    # A public server's selector never names a level past public (EP-120); the gateway lists read
    # tools only on it (EP-141), so no seed may widen either.
    for (namespace, name), (_, server) in servers().items():
        spec = server["spec"]
        if spec["audience"] != "public":
            continue
        levels = (spec.get("select") or {}).get("accessLevel", ["public"])
        assert levels == ["public"], (namespace, name)
