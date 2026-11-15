"""The public MCP connectors of Banská Bystrica and BBSK (T-4141, ADR-N-043, EP-92..EP-96).

One named `McpServer` per publisher, `/api/mcp/{project}/otvorene-data`, over that publisher's
public Endpoints only: what a person pastes into Claude, ChatGPT or any MCP client reaches the
city's or the region's open data and nothing of another organization. These cases read the seed;
what the gateway serves from it is the outside-client transcript in the task.
"""

from pathlib import Path

import pytest
import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
SERVERS = {
    # project: (seed folder, public Endpoints the server must reach)
    # The fourth (second) member holds every other table of the body's catalogue (T-4255).
    "banskabystrica": ("banskabystrica", {"banskabystrica-verejne", "banskabystrica-otvorene-data"}),
    "bbsk": ("bbsk", {"bbsk-registre", "bbsk-kraj-verejne", "bbsk-kpi", "bbsk-otvorene-data"}),
    # One per public assistant (T-4750): the assistant reads its body's data through it.
    "helsinki": ("helsinki", {"helsinki-weather", "helsinki-events"}),
    "praha": ("praha", {"praha-mesto"}),
    "zilina": ("zilina", {"zilina-verejne", "zilina-kpi", "zilina-uniza"}),
}
# The owner's case of more Endpoints than ten members (T-4750): the city's and the region's public
# Endpoints by `select`, one server; never the space the conformance runs write (public-air).
COMBINED = ("bbsk", "kraj-a-mesto", {
    "spaces": ["banskabystrica/banskabystrica-verejne", "banskabystrica/banskabystrica-otvorene-data",
               "bbsk/bbsk-registre", "bbsk/bbsk-kraj", "bbsk/bbsk-kpi", "bbsk/bbsk-otvorene-data",
               "bbsk/bbsk-podujatia"],
    "accessLevel": ["public"]})
# What an anonymous Policy of a member may grant: reads, and the subscriptions a WebSocket
# needs (T-4073), which the gateway hides from anonymous MCP callers (T-4168). No entity write.
# Since T-4258 also the discovery reads, so an anonymous MCP client lists types and attributes.
DISCOVERY = {
    "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveEntityTypeInfo",
    "retrieveAttrTypes", "retrieveAttrTypeDetails", "retrieveAttrTypeInfo",
}
# The history reads of the types that keep one (T-4372).
HISTORY = {"queryTemporal", "retrieveTemporal"}
ANONYMOUS_OPERATIONS = {"retrieveOps", "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"} | DISCOVERY | HISTORY


def manifests(folder: Path):
    for path in sorted(folder.glob("*.yaml")):
        if path.name == "index.yaml" or path.name.endswith("-bento.yaml"):
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and "kind" in doc:
                yield path, doc


def server(project: str):
    folder = SEED / SERVERS[project][0]
    found = [(p, d) for p, d in manifests(folder) if d["kind"] == "McpServer" and d["metadata"]["name"] == "otvorene-data"]
    assert len(found) == 1, f"{folder.name} holds {len(found)} otvorene-data servers"
    return found[0]


@pytest.mark.parametrize("project", SERVERS)
def test_the_server_is_public_and_placed_where_the_gateway_reads_it(project):
    path, doc = server(project)
    assert doc["apiVersion"] == "joinedcontext.com/v1alpha1"
    assert doc["metadata"] == doc["metadata"] | {"name": "otvorene-data", "namespace": project}
    assert doc["spec"]["audience"] == "public"
    assert "allowedProjects" not in doc["spec"]
    # The AI client reads the English text as its instructions (EP-92), the Portal the Slovak.
    for text in ("title", "description"):
        assert set(doc["metadata"][text]) == {"sk", "en"}
    index = yaml.safe_load((path.parent / "index.yaml").read_text())
    # jc-core McpServerSpec::PROJECT_PATH_TEMPLATE
    assert index[path.name] == f"projects/{project}/mcp/otvorene-data.yaml"


def selected(project: str, endpoints: dict) -> bool:
    """Whether the server of `project` selects this Endpoint (T-5367: `select`, not `members`)."""
    select = server(project)[1]["spec"]["select"]
    return f"{project}/{endpoints['contextSpaceRef']}" in select.get("spaces", []) or project in select.get("projects", [])


@pytest.mark.parametrize("project", SERVERS)
def test_the_server_selects_the_publishers_own_public_endpoints(project):
    folder, expected = SERVERS[project]
    _, doc = server(project)
    # Every public Endpoint of the publisher's own spaces, resolved per call (MF-55, T-5367).
    assert "members" not in doc["spec"]
    select = doc["spec"]["select"]
    assert select["accessLevel"] == ["public"]
    assert set(select) <= {"spaces", "projects", "accessLevel"}
    assert select.get("projects", [project]) == [project], select
    assert all(s.startswith(f"{project}/") for s in select.get("spaces", [])), select
    endpoints = {d["metadata"]["name"]: d for _, d in manifests(SEED / folder) if d["kind"] == "Endpoint"}
    for name in expected:
        assert endpoints[name]["spec"]["audience"] == "public", name
        assert endpoints[name]["spec"].get("mcp") is not False, f"{name} turns its MCP off"
        assert selected(project, endpoints[name]["spec"]), f"{name} is outside the server's select"


@pytest.mark.parametrize("project", SERVERS)
def test_an_anonymous_caller_of_a_member_may_read_and_never_write(project):
    folder, expected = SERVERS[project]
    docs = [d for _, d in manifests(SEED / folder)]
    spaces = {
        d["metadata"]["name"]: d["spec"]["contextSpaceRef"]
        for d in docs
        if d["kind"] == "Endpoint" and d["metadata"]["name"] in expected
    }
    for member, space in spaces.items():
        anonymous = [
            d["spec"]
            for d in docs
            if d["kind"] == "Policy"
            and d["spec"]["assignee"] == {"kind": "role", "id": "public"}
            and d["spec"]["contextSpaceRef"]["name"] == space
        ]
        assert anonymous, f"{member}: no anonymous Policy on {space}, the public server would answer nothing"
        for policy in anonymous:
            assert set(policy["operations"]) <= ANONYMOUS_OPERATIONS, (member, policy["operations"])


def test_the_conformance_endpoint_is_no_member():
    """`public-air` holds what the smoke run and the ETSI suite write; it is no open data."""
    endpoints = {d["metadata"]["name"]: d["spec"] for _, d in manifests(SEED / "banskabystrica") if d["kind"] == "Endpoint"}
    assert not selected("banskabystrica", endpoints["public-air"])


def test_every_public_grant_reads_and_discovers_and_writes_no_entity():
    """T-4258, EP-116: each Policy of the role `public` in the seed grants the six discovery
    reads beside `retrieveOps`, which answer only the names the grant already reads (EP-26), and
    no entity write."""
    found = 0
    for folder in sorted(p for p in SEED.iterdir() if p.is_dir()):
        for path, doc in manifests(folder):
            spec = doc.get("spec") or {}
            if doc["kind"] != "Policy" or (spec.get("assignee") or {}).get("id") != "public":
                continue
            found += 1
            # a Policy of the history alone sits beside the read that discovers (T-4372)
            assert DISCOVERY <= set(spec["operations"]) or set(spec["operations"]) == HISTORY, path.name
            assert set(spec["operations"]) <= ANONYMOUS_OPERATIONS, (path.name, spec["operations"])
    assert found >= 18


def test_more_than_ten_endpoints_are_one_server_by_select():
    project, name, select = COMBINED
    found = [(p, d) for p, d in manifests(SEED / project) if d["kind"] == "McpServer" and d["metadata"]["name"] == name]
    assert len(found) == 1, found
    path, doc = found[0]
    assert doc["metadata"]["namespace"] == project
    assert doc["spec"] == doc["spec"] | {"audience": "public", "select": select}
    # a selector never reaches past the server's audience; members stay for the curated sets
    assert "members" not in doc["spec"] and "allowedProjects" not in doc["spec"]
    endpoints = {d["metadata"]["name"]: d["spec"] for _, d in manifests(SEED / "banskabystrica") if d["kind"] == "Endpoint"}
    assert f"banskabystrica/{endpoints['public-air']['contextSpaceRef']}" not in select["spaces"]
    for text in ("title", "description"):
        assert set(doc["metadata"][text]) == {"sk", "en"}
    index = yaml.safe_load((path.parent / "index.yaml").read_text())
    assert index[path.name] == f"projects/{project}/mcp/{name}.yaml"


def test_helsinkis_public_assistant_reads_through_its_server():
    deployments = [d for _, d in manifests(SEED / "helsinki") if d["kind"] == "AssistantDeployment" and d["metadata"]["name"] == "helsinki-public"]
    assert len(deployments) == 1
    spec = deployments[0]["spec"]
    assert spec["mcpServer"] == "helsinki/otvorene-data"
    assert "connectors" not in spec and "scope" not in spec


def test_helsinkis_server_searches_the_city_website_and_the_data_portal():
    """One address for Claude: datasets and hel.fi pages (MF-57, T-6167)."""
    docs = [d for _, d in manifests(SEED / "helsinki")]
    server = next(d for d in docs if d["kind"] == "McpServer" and d["metadata"]["name"] == "otvorene-data")
    attached = server["spec"]["knowledgeSources"]
    assert attached == ["web", "data-portal"]
    sources = {d["metadata"]["name"]: d["spec"] for d in docs if d["kind"] == "KnowledgeSource"}
    for name in attached:
        # a public server attaches only public sources: an anonymous caller reads every passage
        assert sources[name]["visibility"] == "public", name
