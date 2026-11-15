"""T-4687: the BBSK demo is outside the platform's deployment and holds only BB and BBSK.

`demos/bbsk/` renders its own catalogue, database, edge and NetworkPolicies into namespace
`demo-bbsk` and nothing else; the platform's environments render none of it. Its catalogue harvests
only the city's and the region's sources, says DEMO on every page and keeps search engines out,
and its assistant reads the region's and the city's public sources through one McpServer (owner 2026-10-19T19:50Z).
"""

from pathlib import Path

from test_public_assistant_seed import manifests

ROOT = Path(__file__).resolve().parent.parent
HOSTS = {"dev": "bbsk-demo.apps.dev.joinedcontext.com", "smartcity": "bbsk.smartcity.joinedcontext.com"}
PUBLISHERS = {"banskabystrica", "bbsk"}


def doc_text(docs, key: str) -> str:
    return "".join(d.get("data", {}).get(key, "") for d in docs if d["kind"] == "ConfigMap")


def test_the_demo_assistant_reads_the_regions_and_the_citys_public_data_through_one_server():
    # Owner 2026-10-19T19:50Z (replacing 17:55Z's BBSK only): the region and the city, the two
    # publishers the demo catalogue harvests; T-4750: the data through one public McpServer that
    # selects both, never a connector list beside it.
    spec = deployment_named("bbsk-demo")["spec"]
    assert spec["channel"] == "public" and spec["publicId"] == "bbsk-demo"
    assert "connectors" not in spec and "select" not in spec
    assert spec["scope"] == {"projects": ["banskabystrica"]}
    own = ["web", "data-portal", "catalogue"]
    assert spec["sources"] == own + [f"banskabystrica/{n}" for n in own]
    for named in spec["sources"]:
        project, _, name = named.rpartition("/")
        assert (project or "bbsk") in PUBLISHERS, named
        assert manifests("KnowledgeSource", project or "bbsk")[name]["spec"]["visibility"] == "public", named
    assert spec["mcpServer"] == "bbsk/kraj-a-mesto"
    server = manifests("McpServer", "bbsk")["kraj-a-mesto"]["spec"]
    assert server["audience"] == "public" and "members" not in server
    assert server["select"]["accessLevel"] == ["public"]
    assert {s.split("/")[0] for s in server["select"]["spaces"]} == PUBLISHERS
    prompt = spec["systemPrompt"]
    assert prompt.startswith("Si ukážkový asistent (DEMO) nad otvorenými dátami Banskobystrického samosprávneho kraja a")
    assert "mesta Banská Bystrica" in prompt and "Táto ukážka obsahuje len dáta" in prompt
    for text in (*spec["theme"]["subtitle"].values(), spec["theme"]["greeting"], *spec["disclaimer"].values()):
        assert "Bystric" in text, text
    assert spec["allowedOrigins"] == [f"https://{HOSTS['smartcity']}", f"https://{HOSTS['dev']}"]
    # Owner 2026-10-19T13:49Z: two cards from www.bbsk.sk, then the open-data ones, only those proven 3 of 3.
    # The Zvolen schools card is off since the owner's 16:22Z run read Organization (supervisor 16:35Z);
    # the population since prod's 2026-10-20 run (T-4769); office hours back with dataset links (T-4793):
    # at most three, each language alike.
    assert len(spec["suggestions"]["sk"]) == len(spec["suggestions"]["en"]) in (2, 3)
    assert not any("Zvolen" in q for q in spec["suggestions"]["sk"] + spec["suggestions"]["en"])
    # T-4740: the panel is "AI asistent" in the landing page's blue, never a name its initials stand for.
    assert spec["theme"]["title"]["sk"] == "AI asistent" and spec["theme"]["primaryColor"] == "#264a96"


def deployment_named(public_id: str) -> dict:
    found = [d for d in manifests("AssistantDeployment", "bbsk").values() if d["spec"]["publicId"] == public_id]
    assert len(found) == 1, public_id
    return found[0]
