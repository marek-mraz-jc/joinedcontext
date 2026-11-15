"""T-4167, T-4281 (Architecture/23 §6): one public assistant per body, over its own knowledge.

Each of Banská Bystrica (the city), BBSK (the region), Žilina and Prague answers anyone on the
assistant host, the demo page and the data catalogue under its own publicId. Everything it reads
is public and its own body's: its web source crawls the body's site alone, its CKAN source lists
the body's organization alone, its connectors name public Endpoints of its own project that serve
MCP with read-only tools, and its limits are T-4149's with a daily budget per body (T-4281).
No other body's site, organization or Endpoint is reachable from it.
"""

import json
from pathlib import Path
from urllib.parse import urlsplit

import pytest
import yaml
from conftest import (
    PROJECT_ROOT,
    dev_catalogue,
    render_catalogue_assistant,
    seed_documents,
)

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
# jc-assistant's READ_ONLY_TOOLS (platform crates/assistant/src/chat/mcp.rs), which the data loop
# offers the answering model (T-4294).
READ_ONLY = {"list_types", "get_type", "list_attributes", "query_entities", "summarize", "get_entity",
             "get_attribute_value", "query_temporal", "retrieve_temporal", "describe_schema", "describe_access"}
# publicId: (project, web host, CKAN organization, language first, cost per day)
BODIES = {
    "banskabystrica": ("banskabystrica", "www.banskabystrica.sk", "banskabystrica", "sk", 1),
    "bbsk": ("bbsk", "www.bbsk.sk", "bbsk", "sk", 1),
    "zilina": ("zilina", "www.zilina.sk", "zilina", "sk", 0.5),
    "praha": ("praha", "www.praha.eu", "praha", "cs", 0.5),
}
# Words that name a body, for the check that one body's assistant names no other.
NAMES = {
    "banskabystrica": ("banskabystrica", "banská bystrica"),
    "bbsk": ("bbsk", "samosprávn"),
    "zilina": ("zilina", "žilin"),
    "praha": ("praha", "prague", "prahy"),
}
# The national dataset every public assistant reads besides its own (T-4340, owner 2026-10-16).
NATIONAL = {"slovensko/ovzdusie-sk"}
ORIGINS = [
    "https://assistant.dev.joinedcontext.com",
    "https://dev.joinedcontext.com",
    "https://opendata.dev.joinedcontext.com",
    # The standalone assistant's own host (T-4694): its page asks the chat from there.
    "https://ai.dev.joinedcontext.com",
]


def manifests(kind: str, project: str) -> dict[str, dict]:
    found = {}
    # A topic folder, such as slovensko/susr-pr/, holds its own manifests (T-4390).
    for path, doc in seed_documents(project):
        if path.name != "index.yaml" and isinstance(doc, dict) and doc.get("kind") == kind:
            found[doc["metadata"]["name"]] = doc
    return found


def deployment(public_id: str) -> dict:
    project = BODIES[public_id][0]
    found = [d for d in manifests("AssistantDeployment", project).values() if d["spec"]["publicId"] == public_id]
    assert len(found) == 1, public_id
    return found[0]


@pytest.mark.parametrize("public_id", BODIES)
def test_each_body_has_one_public_assistant_on_its_own_id(public_id):
    doc = deployment(public_id)
    spec = doc["spec"]
    assert doc["metadata"]["namespace"] == BODIES[public_id][0]
    assert spec["channel"] == "public"
    assert spec.get("enabled", True) is True
    # Its own project and the national open data only: no source and no other connector of another
    # project (T-4281, T-4340).
    assert spec["scope"] == {"projects": ["slovensko"]}
    assert all("/" not in name for name in spec["sources"])
    assert {c["endpoint"] for c in spec["connectors"] if "/" in c["endpoint"]} == NATIONAL


@pytest.mark.parametrize("public_id", BODIES)
def test_every_source_is_public_and_the_bodys_own(public_id):
    project, host, organization, _, _ = BODIES[public_id]
    spec = deployment(public_id)["spec"]
    sources = manifests("KnowledgeSource", project)
    assert set(spec["sources"]) == {"web", "data-portal", "catalogue"}
    for name in spec["sources"]:
        source = sources[name]["spec"]
        assert source["visibility"] == "public", name
        if source["source"] == "website":
            assert {urlsplit(url).hostname for url in source["startUrls"]} == {host}
        if source["source"] == "ckan":
            assert source["organizations"] == [organization], name
            instance = manifests("CkanInstance", project)[source["ckanInstanceRef"]]["spec"]
            assert instance["url"] == "https://opendata.dev.joinedcontext.com"


@pytest.mark.parametrize("public_id", BODIES)
def test_every_connector_is_a_public_mcp_endpoint_of_the_body_with_read_only_tools(public_id):
    project = BODIES[public_id][0]
    spec = deployment(public_id)["spec"]
    endpoints = manifests("Endpoint", project)
    assert spec["connectors"], "the assistant reads live values through connectors"
    for connector in spec["connectors"]:
        own, _, name = connector["endpoint"].rpartition("/")
        endpoint = (manifests("Endpoint", own) if own else endpoints)[name]
        assert endpoint["spec"]["audience"] == "public", connector["endpoint"]
        assert "mcp" in endpoint["spec"]["enabledRepresentations"], connector["endpoint"]
        assert set(connector["tools"]) <= READ_ONLY, connector
        # The data loop reads the schema and history before it queries (T-4294).
        assert {"get_type", "list_attributes", "query_temporal"} <= set(connector["tools"]), connector
        # T-4680: a broad read gives the answer the broker's profile of the type, not 15 rows.
        assert "summarize" in connector["tools"], connector


@pytest.mark.parametrize("public_id", BODIES)
def test_the_assistant_reads_its_bodys_data_through_its_public_mcp_server(public_id):
    # T-4256 (owner 2026-10-16): the assistant queries all the open data by default; T-4750: it
    # reads it through the public `otvorene-data` server of its project, so an Endpoint added
    # there joins without a hand edit here and the tool set is the server's, not one per member.
    project = BODIES[public_id][0]
    spec = deployment(public_id)["spec"]
    assert spec["mcpServer"] == f"{project}/otvorene-data"
    server = manifests("McpServer", project)["otvorene-data"]["spec"]
    assert server["audience"] == "public"
    # only the national dataset stays a connector of its own
    assert {c["endpoint"] for c in spec["connectors"]} == NATIONAL


def selected(server: dict, project: str) -> list[str]:
    """The Endpoints of `project` a server's `select` takes as the gateway does (MF-55): public, serving MCP, of a
    selected project or space. The seed's own Endpoints; the Apps' public ones carry no themes."""
    select = server.get("select")
    if select is None:
        return []
    names = []
    for name, endpoint in manifests("Endpoint", project).items():
        spec = endpoint["spec"]
        space = spec["contextSpaceRef"]
        space = space if isinstance(space, str) else space["name"]
        if spec["audience"] != "public" or "mcp" not in spec.get("enabledRepresentations", []):
            continue
        if "projects" in select and project not in select["projects"]:
            continue
        if "spaces" in select and f"{project}/{space}" not in select["spaces"]:
            continue
        names.append(name)
    return sorted(names)


def test_the_public_mcp_servers_list_members_that_serve_mcp():
    # The gateway drops a member that does not serve MCP, so a server whose members lack it
    # lists no tool at all (T-4141's `otvorene-data` before T-4167).
    for project in ("banskabystrica", "bbsk", "helsinki", "praha", "zilina"):
        endpoints = manifests("Endpoint", project)
        for name, server in manifests("McpServer", project).items():
            for member in server["spec"].get("members", []):
                assert "mcp" in endpoints[member["name"]]["spec"]["enabledRepresentations"], (project, name, member)


@pytest.mark.parametrize("public_id", BODIES)
def test_no_other_body_is_named_and_no_secret_is_carried(public_id):
    project = BODIES[public_id][0]
    spec = deployment(public_id)["spec"]
    text = yaml.safe_dump(deployment(public_id)).lower()
    for source in spec["sources"]:
        text += yaml.safe_dump(manifests("KnowledgeSource", project)[source]).lower()
    for other, words in NAMES.items():
        if other == public_id:
            continue
        for word in words:
            assert word not in text, (public_id, word)
    for word in ("hel.fi", "helsinki", "secretref", "apikey", "api_key", "token:", "password"):
        assert word not in text, (public_id, word)


@pytest.mark.parametrize("public_id", BODIES)
def test_it_is_placed_on_the_host_the_demo_page_and_the_catalogue_with_its_own_budget(public_id):
    spec = deployment(public_id)["spec"]
    assert spec["allowedOrigins"] == ORIGINS
    limit = spec["rateLimit"]
    assert (limit["perClientPerMinute"], limit["perClientPerHour"], limit["perClientPerDay"], limit["streamsPerClient"]) == (10, 60, 200, 2)
    budget = spec["budget"]
    assert budget["costPerDayUsd"] == BODIES[public_id][4]
    assert budget["outputTokensPerAnswer"] == 700
    assert budget["messagesPerConversation"] == 30
    assert budget["questionChars"] == 1000
    # The cost cap is the guard; the shared token cap stays above it (T-4286).
    assert budget["tokensPerDay"] <= 5_000_000
    assert budget["tokensPerConversation"] <= budget["tokensPerDay"]
    assert spec["model"]["answer"] and spec["model"]["router"]


def test_the_public_budgets_stay_inside_the_keys_five_dollars():
    total = sum(deployment(p)["spec"]["budget"]["costPerDayUsd"] for p in BODIES)
    assert total + general()["spec"]["budget"]["costPerDayUsd"] <= 5


def test_a_conversation_holds_100000_tokens_and_costs_at_most_15_cents():
    # T-4761 (owner 2026-10-19 20:1xZ): bbsk-demo's 40,000 refused the owner's second question
    # ("Tento rozhovor je už dlhý"); one data answer alone can spend that. Owner: ten questions
    # in a row, about 15 cents at most: the conversation's cost cap is the money guard, and
    # T-4763 cuts what one answer costs. The data-eval copies measure answers and keep 3,000,000.
    seen = 0
    for path, doc in seed_documents():
        if isinstance(doc, dict) and doc.get("kind") == "AssistantDeployment":
            seen += 1
            budget = doc["spec"]["budget"]
            assert budget["tokensPerConversation"] >= 100_000, path.name
            if not doc["metadata"]["name"].startswith("data-eval-"):
                assert 0 < budget["costPerConversationUsd"] <= 0.15, path.name
                assert budget["costPerConversationUsd"] <= budget["costPerDayUsd"], path.name
    assert seen >= 15


@pytest.mark.parametrize("public_id", BODIES)
def test_the_chat_speaks_the_bodys_language_first(public_id):
    spec = deployment(public_id)["spec"]
    first = BODIES[public_id][3]
    assert spec["languages"][0] == first
    assert 3 <= len(spec["suggestions"][first]) <= 6
    assert spec["disclaimer"][first] and spec["disclaimer"]["en"]
    assert len(spec["theme"]["greeting"]) <= 500


# What each widget's header shows (T-4296, Architecture/22 §4).
TITLES = {
    "banskabystrica": "Banská Bystrica",
    "bbsk": "Banskobystrický kraj",
    "zilina": "Žilina",
    "praha": "Praha",
}


@pytest.mark.parametrize("public_id", BODIES)
def test_the_header_names_the_body(public_id):
    title = deployment(public_id)["spec"]["theme"]["title"]
    assert 1 <= len(title) <= 60
    assert title == TITLES[public_id]


@pytest.mark.parametrize("public_id", BODIES)
def test_the_sources_and_the_deployment_are_seeded_at_their_kinds_paths(public_id):
    project = BODIES[public_id][0]
    index = yaml.safe_load((SEED / project / "index.yaml").read_text())
    by_path = {path: name for name, path in index.items()}
    name = deployment(public_id)["metadata"]["name"]
    assert f"projects/{project}/assistant/deployments/{name}.yaml" in by_path
    for source in deployment(public_id)["spec"]["sources"]:
        assert f"projects/{project}/assistant/sources/{source}.yaml" in by_path, source
    for file in by_path.values():
        assert (SEED / project / file).exists(), file


def test_the_region_is_indexed_once():
    # T-4281 item 2: bbsk.sk is crawled in the bbsk project alone; the city's project holds no
    # second copy.
    for doc in manifests("KnowledgeSource", "banskabystrica").values():
        hosts = {urlsplit(url).hostname for url in doc["spec"].get("startUrls", [])}
        assert "www.bbsk.sk" not in hosts, doc["metadata"]["name"]
        assert "bbsk" not in doc["spec"].get("organizations", []), doc["metadata"]["name"]


def test_no_assistant_reads_a_planted_eval_page():
    # F-BNT1-009: the T-4183/T-4688 eval copies and their planted injection page outlived their
    # runs as public assistants anyone with the publicId could ask; the copies and the page are gone.
    for project in [p.name for p in SEED.iterdir() if p.is_dir()]:
        assert "eval-uploads" not in manifests("KnowledgeSource", project), project
        for name, doc in manifests("AssistantDeployment", project).items():
            assert "eval-uploads" not in doc["spec"].get("sources", []), (project, name)
            assert not name.startswith(("bb-eval-", "web-eval-")), (project, name)


def general() -> dict:
    """dev's catalogue assistant, rendered from its instance block (T-4636)."""
    return yaml.safe_load(render_catalogue_assistant(dev_catalogue(), "dev.joinedcontext.com"))


def test_the_catalogue_frames_the_general_assistant_on_every_page():
    # T-4342 (owner 2026-10-16): data.dev offers one general assistant, never a city's; the
    # template's per-organization map stays for other installations and is empty here.
    values = Path(__file__).resolve().parent.parent / "ci/example-deployments/environments/dev/ckan.yaml.gotmpl"
    catalogue = yaml.safe_load(values.read_text())["ckan"]["catalogue"]
    assert catalogue["assistant"] == "data" and "assistantByOrganization" not in catalogue
    assert general()["spec"]["publicId"] == "data"
    assert general()["spec"]["allowedOrigins"] == ["https://opendata.dev.joinedcontext.com"]


# The CKAN organization each body publishes under, and the project its Endpoints live in.
PUBLISHED = {"bbsk": "bbsk", "banskabystrica": "banskabystrica", "zilina": "zilina", "praha": "praha", "hel-fi": "helsinki", "slovensko": "slovensko", "barcelona": "barcelona"}


def test_the_general_assistant_reads_every_publishers_public_endpoints():
    spec = general()["spec"]
    assert spec["channel"] == "public" and spec.get("enabled", True) is True
    # And the national open data (T-4340).
    assert spec["scope"]["projects"] == ["banskabystrica", "zilina", "praha", "helsinki", "slovensko", "barcelona"]
    reached = set()
    for connector in spec["connectors"]:
        project, _, name = connector["endpoint"].rpartition("/")
        project = project or "bbsk"
        assert project == "bbsk" or project in spec["scope"]["projects"], connector
        endpoint = manifests("Endpoint", project)[name]["spec"]
        assert endpoint["audience"] == "public", connector["endpoint"]
        assert "mcp" in endpoint["enabledRepresentations"], connector["endpoint"]
        assert set(connector["tools"]) <= READ_ONLY, connector
        assert name != "public-air", "test entities are written through it"
        reached.add(project)
    assert reached == set(PUBLISHED.values()), "an Endpoint of every published organization"


def test_the_general_assistant_reads_every_national_public_endpoint():
    # T-4680: each ŠÚ SR topic is its own Endpoint of one type, KeyPerformanceIndicator; the
    # router tells them apart by their titles, so every one joins and none is left unreachable.
    # The catalogue's own records (`catalog`) are read through the search sources instead.
    endpoints = manifests("Endpoint", "slovensko")
    national = {f"slovensko/{name}" for name, e in endpoints.items() if name != "catalog"
                and e["spec"]["audience"] == "public" and "mcp" in e["spec"]["enabledRepresentations"]}
    assert len([n for n in national if n.startswith("slovensko/susr-")]) == 45
    connected = {c["endpoint"] for c in general()["spec"]["connectors"]}
    assert national <= connected, sorted(national - connected)
    for name in national:
        title = endpoints[name.split("/")[1]]["metadata"]["title"]
        assert title.get("en") and len(title["en"]) <= 80, name


def test_the_general_assistant_reads_barcelonas_surface_parking():
    # T-6566 (owner 2026-11-03): asked where to park near Barcelona's centre, the catalogue's
    # assistant answered that the published data holds no parking for Barcelona and sent the
    # person to Žilina and Helsinki, while barcelona-parking (OnStreetParking, totalSpotNumber) is
    # public. Every public Endpoint of the city is a connector, and the prompt names the city.
    spec = general()["spec"]
    connected = {c["endpoint"] for c in spec["connectors"]}
    endpoints = manifests("Endpoint", "barcelona")
    city = {f"barcelona/{name}" for name, e in endpoints.items() if name != "barcelona"
            and e["spec"]["audience"] == "public" and "mcp" in e["spec"]["enabledRepresentations"]}
    assert "barcelona/barcelona-parking" in city
    assert city <= connected, sorted(city - connected)
    assert "barcelona" in spec["scope"]["projects"]
    assert "Barcelona = barcelona" in spec["systemPrompt"]


# jc-core reach::MAX_RESOLVED (AG-122): jc-assistant serves the first this many Endpoints by slug
# and drops the rest without a word, so a connector past it is never read.
MAX_RESOLVED = 100


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_the_general_assistant_resolves_every_connector_with_the_air_of_banska_bystrica(env):
    # T-5786: 67 connectors under a bound of 50 dropped the 17 with the highest slugs, among them
    # banskabystrica-verejne and ovzdusie-sk, the two that hold station SK0263A, and prod answered
    # that Banská Bystrica publishes no air quality.
    values = PROJECT_ROOT / f"ci/example-deployments/environments/{env}/ckan.yaml.gotmpl"
    connectors = yaml.safe_load(values.read_text())["ckan"]["catalogue"]["assistantSeed"]["connectors"]
    assert len(connectors) == len(set(connectors)), "a connector named twice"
    assert len(connectors) <= MAX_RESOLVED, f"{len(connectors)} connectors, past AG-122's {MAX_RESOLVED}"
    assert {"banskabystrica/banskabystrica-verejne", "slovensko/ovzdusie-sk"} <= set(connectors)
    pipelines = manifests("Pipeline", "banskabystrica")
    assert any("AirQualityObserved" in json.dumps(p) and "banskabystrica-verejne" in json.dumps(p)
               for p in pipelines.values()), "a pipeline writes AirQualityObserved into banskabystrica-verejne"


def test_the_general_assistant_knows_the_whole_catalogue_and_each_publishers_own_site():
    # Its knowledge is the catalogue's records (every organization of data.dev, refreshed after
    # the nightly harvest), each body's Endpoint descriptions and, since T-4680 (the owner sent
    # bbsk.sk: its map portal and cycle pages answer what the catalogue cannot), each
    # publisher's own website, the weekly crawl its body's assistant reads. No other site.
    spec = general()["spec"]
    assert spec.get("answer", {}).get("routes", "both") == "both"
    webs = set()
    for name in spec["sources"]:
        project, _, source = name.rpartition("/")
        project = project or "bbsk"
        assert project == "bbsk" or project in spec["scope"]["projects"], name
        doc = manifests("KnowledgeSource", project)[source]["spec"]
        assert doc["visibility"] == "public", name
        assert doc["source"] in ("ckan", "catalogue", "website"), name
        if doc["source"] == "website":
            webs.add(project)
            # The body's own site, the one its own assistant crawls (T-4167).
            own = {"bbsk": "www.bbsk.sk", "banskabystrica": "www.banskabystrica.sk", "zilina": "www.zilina.sk",
                   "praha": "www.praha.eu", "helsinki": "www.hel.fi"}[project]
            assert {urlsplit(url).hostname for url in doc["startUrls"]} == {own}, name
    assert webs == {"bbsk", "banskabystrica", "zilina", "praha", "helsinki"}
    portal = manifests("KnowledgeSource", "bbsk")["portal"]["spec"]
    assert portal["source"] == "ckan" and "organizations" not in portal
    instance = manifests("CkanInstance", "bbsk")[portal["ckanInstanceRef"]]["spec"]
    assert instance["url"] == "https://opendata.dev.joinedcontext.com"
    assert portal["schedule"] == "50 3 * * *"  # after charts/ckan's harvest at 03:23
    # Barcelona keeps no source of its own (T-6566): its datasets come through `portal`, its
    # numbers through its connectors.
    indexed = [p for p in spec["scope"]["projects"] if p != "barcelona"]
    assert {f"{p}/catalogue" for p in indexed} | {"portal", "catalogue"} | \
        {"web"} | {f"{p}/web" for p in indexed if p != "slovensko"} == set(spec["sources"])


def test_the_general_assistant_names_no_city_and_carries_the_portal_guide():
    spec = general()["spec"]
    greeting = spec["theme"]["greeting"]
    assert set(greeting) == {"sk", "en", "fi"}, "a greeting per language (T-4365)"
    title = spec["theme"]["title"]
    assert title == {"sk": "Otvorené dáta", "en": "Open data", "fi": "Avoin data"}, "no host name in the header (T-4635)"
    shown = (" ".join(title.values()) + " " + " ".join(greeting.values())).lower()
    for words in NAMES.values():
        assert not any(word in shown for word in words), shown
    assert "helsin" not in shown
    assert "data.dev" not in shown
    assert spec["languages"] == ["sk", "en", "fi"]
    assert set(spec["suggestions"]) == set(spec["disclaimer"]) == {"sk", "en", "fi"}
    # Judge round 3 (T-4342): one chip set in every language, each chip on one line of the 400px panel.
    chips = spec["suggestions"]
    assert len({len(c) for c in chips.values()}) == 1, chips
    assert all(len(c) <= 50 for cs in chips.values() for c in cs), chips
    prompt = spec["systemPrompt"]
    assert len(prompt) <= 6000, "the guide stays in about 1,500 tokens of the cached prefix"
    for fact in ("https://api.dev.joinedcontext.com/api/mcp", "ngsi-ld/v1/entities", "bbox=", "Smart Data Model",
                 "300 otázok", "vydavateľa"):
        assert fact in prompt, fact
    for word in ("secretref", "apikey", "api_key", "token:", "password"):
        assert word not in yaml.safe_dump(general()).lower(), word


def test_a_conversation_of_the_general_assistant_holds_a_question_over_five_publishers():
    # T-4342: one data question measured 33,744 tokens on dev; the cities' 40,000 refused the
    # next one. The cost cap stays the guard and the per-visitor limits stay the cities'.
    spec = general()["spec"]
    assert spec["budget"]["tokensPerConversation"] >= 2 * 33_744
    # T-4660: the owner's fourth question was refused at 117,171 tokens; a conversation holds five
    # questions at the platform's 60,000 per question.
    assert spec["budget"]["tokensPerConversation"] >= 5 * 60_000
    # T-4737: 300,000 refused the owner's second data question on prod; a conversation holds ten
    # data answers at the measured p90 (296,985 tokens, dev turns 2026-10-19).
    assert spec["budget"]["tokensPerConversation"] >= 10 * 296_985
    assert spec["budget"]["costPerDayUsd"] == 1
    # T-4761 (owner 2026-10-19): ten questions in a row, about 15 cents at most.
    assert spec["budget"]["costPerConversationUsd"] == 0.15
    assert spec["rateLimit"] == deployment("bbsk")["spec"]["rateLimit"]


def test_the_general_assistant_is_seeded_at_its_kinds_paths():
    # T-4636: the deployment renders from the instance block into its project, gitea's
    # catalogueAssistants writes it at projects/<project>/assistant/deployments/<publicId>.yaml, and
    # no fixed copy in the seed folder competes with it.
    index = yaml.safe_load((SEED / "bbsk" / "index.yaml").read_text())
    catalogue = dev_catalogue()
    assert (catalogue["assistantSeed"]["project"], catalogue["assistant"]) == ("bbsk", "data")
    assert general()["metadata"] == {"name": "data", "namespace": "bbsk"}
    assert "projects/bbsk/assistant/deployments/data.yaml" not in index.values()
    assert index["bbsk-knowledge-portal.yaml"] == "projects/bbsk/assistant/sources/portal.yaml"


# What 2026-10-16 measured on dev (T-4286): banskabystrica's 56 requests spent 263,512 tokens
# (253,475 in, 10,037 out) for 227,764 micro-dollars, about $0.86 a million tokens.
MEASURED_USD_PER_TOKEN = 227_764e-6 / 263_512


def test_no_anonymous_deployment_runs_out_of_tokens_before_its_cost_cap():
    """T-4286: tokensPerDay 300,000 held the public assistant to about 65 answers a day for
    everyone, and at 08:25Z every visitor read a refusal. The cost cap is the guard: on every
    deployment anonymous visitors reach, the day's tokens at the measured price buy more than
    costPerDayUsd, so the cost cap binds first, and past it the answers are links."""
    checked = 0
    for path, doc in seed_documents():
        if len(path.relative_to(SEED).parts) != 2:
            continue
        if not (isinstance(doc, dict) and doc.get("kind") == "AssistantDeployment"):
            continue
        spec = doc["spec"]
        if spec["channel"] == "internal":
            continue
        budget = spec["budget"]
        cost = budget.get("costPerDayUsd", 3)
        assert budget["tokensPerDay"] * MEASURED_USD_PER_TOKEN >= cost, (
            f"{path.name}: tokensPerDay {budget['tokensPerDay']} buys about "
            f"${budget['tokensPerDay'] * MEASURED_USD_PER_TOKEN:.2f}, less than costPerDayUsd {cost}"
        )
        checked += 1
    assert checked >= 3, "the public, catalogue and eval deployments are checked"


def test_the_open_data_eval_copies_are_data_with_another_model():
    # T-4359: a comparison of models on data.dev's assistant; each copy is `data` but for its
    # name, publicId and the model it runs, router and answer alike, and spends its own day.
    deployments = manifests("AssistantDeployment", "bbsk")
    base = general()["spec"]
    # T-4656 (owner): Haiku, GPT-6 Luna, GLM 5.3 on its EU providers alone, Argon as the ceiling;
    # no DeepSeek and no Mistral model.
    def same(model, **more):
        return {"router": model, "answer": model, **more}
    models = {"data-eval-gemini": same("google/gemini-3.8-flash"), "data-eval-haiku": same("anthropic/claude-haiku-5.5"),
              "data-eval-haiku-medium": same("anthropic/claude-haiku-5.5", reasoning={"answer": "medium"}),
              "data-eval-luna": same("openai/gpt-6-luna"), "data-eval-argon": same("google/gemini-4-argon"),
              "data-eval-glm": same("z-ai/glm-5.3", provider={"order": ["mistral", "nebius"], "allowFallbacks": False})}
    assert sorted(n for n in deployments if n.startswith("data-eval-")) == sorted(models)
    for name, model in models.items():
        spec = deployments[name]["spec"]
        assert spec["publicId"] == name and "eval" in name.split("-")
        assert spec["model"] == model, name
        assert {k: v for k, v in spec.items() if k not in ("publicId", "model", "budget")} == \
            {k: v for k, v in base.items() if k not in ("publicId", "model", "budget")}, name
        # T-4729: only the day's cost may differ, and only upward to the run's own $4 (its tokensPerDay buys about $4.32).
        day = spec["budget"]["costPerDayUsd"]
        # The visitors' 15-cent conversation cap (T-4761) is not a measurement's: a copy is measured whole.
        assert {**spec["budget"], "costPerDayUsd": base["budget"]["costPerDayUsd"],
                "costPerConversationUsd": base["budget"]["costPerConversationUsd"]} == base["budget"], name
        assert base["budget"]["costPerDayUsd"] <= day <= 4, name


# The EU data themes in their table's order (platform jc-core DataTheme::ALL).
THEMES = ["AGRI", "ECON", "EDUC", "ENER", "ENVI", "GOVE", "HEAL", "INTR", "JUST", "REGI", "SOCI", "TECH", "TRAN"]


@pytest.mark.parametrize("public_id", BODIES)
def test_the_categories_are_the_themes_of_what_it_reads(public_id):
    """T-4694 (AG-124): the categories the context page shows are the themes of its connectors and server members, each once, in
    the table's order; a connector added or dropped without its theme fails here."""
    spec = deployment(public_id)["spec"]
    own = BODIES[public_id][0]
    read = set()
    # the members of its McpServer (T-4750) beside its own connectors
    server = manifests("McpServer", own)[spec["mcpServer"].split("/")[1]]["spec"]
    named = [c["endpoint"] for c in spec["connectors"]] + [m.get("namespace", own) + "/" + m["name"] for m in server.get("members", [])]
    named += [f"{own}/{name}" for name in selected(server, own)]
    for full in named:
        project, _, name = full.rpartition("/")
        endpoint = manifests("Endpoint", project or own)[name]
        read |= set(endpoint["spec"].get("catalog", {}).get("themes", []))
    assert spec["categories"] == [t for t in THEMES if t in read]
