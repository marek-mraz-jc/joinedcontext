"""T-3225 (AG-116, MF-51): every project with Endpoints answers from its own NGSI-LD catalogue.

Each of the five projects seeds one `catalogue` KnowledgeSource at its kind's path. It is public,
so the worker reads its public Endpoints alone and a private space never reaches a public channel
(the worker's own test proves the filter, crates/assistant/tests/catalogue_tests.rs). Helsinki's
two public assistants answer from it.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
PROJECTS = ("banskabystrica", "bbsk", "praha", "helsinki", "zilina")


def documents(project: str) -> list[dict]:
    return [
        doc
        for path in sorted((SEED / project).glob("*.yaml"))
        if path.name != "index.yaml" and not path.name.endswith((".linkml.yaml", "-bento.yaml"))
        for doc in yaml.safe_load_all(path.read_text())
        if isinstance(doc, dict) and "kind" in doc
    ]


def test_every_project_seeds_a_public_catalogue_source_at_its_path():
    for project in PROJECTS:
        sources = [d for d in documents(project) if d["kind"] == "KnowledgeSource" and d["spec"]["source"] == "catalogue"]
        assert len(sources) == 1, project
        source = sources[0]
        assert source["metadata"] == {**source["metadata"], "name": "catalogue", "namespace": project}
        spec = source["spec"]
        assert spec["visibility"] == "public", project
        # It reads the repository and the gateway, never a URL a person typed.
        assert "startUrls" not in spec and "ckanInstanceRef" not in spec, project
        assert spec.get("contextSpaces", []) == [], "every space: the worker keeps the public Endpoints"
        assert any(d["kind"] == "Endpoint" and d["spec"]["audience"] == "public" for d in documents(project)), project
        index = yaml.safe_load((SEED / project / "index.yaml").read_text())
        assert index[f"{project}-knowledge-catalogue-ngsi.yaml"] == f"projects/{project}/assistant/sources/catalogue.yaml"


def test_helsinkis_public_assistants_answer_from_its_catalogue():
    deployments = {d["metadata"]["name"]: d["spec"] for d in documents("helsinki") if d["kind"] == "AssistantDeployment"}
    for name in ("helsinki-public", "helsinki-catalogue"):
        assert "catalogue" in deployments[name]["sources"], name


def test_helsinki_seeds_the_user_guide_for_its_internal_and_public_assistants():
    """T-3226 (AG-118): one public `guide` source, read by the staff assistant the workflow evals
    ask and by the public one; it names nothing to fetch."""
    guides = [d for d in documents("helsinki") if d["kind"] == "KnowledgeSource" and d["spec"]["source"] == "guide"]
    assert len(guides) == 1
    guide = guides[0]
    assert guide["metadata"]["name"] == "guide"
    assert guide["spec"]["visibility"] == "public"
    assert not {"startUrls", "ckanInstanceRef", "contextSpaces"} & set(guide["spec"])
    index = yaml.safe_load((SEED / "helsinki" / "index.yaml").read_text())
    assert index["helsinki-knowledge-guide.yaml"] == "projects/helsinki/assistant/sources/guide.yaml"
    deployments = {d["metadata"]["name"]: d["spec"] for d in documents("helsinki") if d["kind"] == "AssistantDeployment"}
    assert deployments["knowledge"]["channel"] == "internal"
    assert deployments["helsinki-public"]["channel"] == "public"
    for name in ("knowledge", "helsinki-public"):
        assert "guide" in deployments[name]["sources"], name


def test_seed_knowledge_sources_never_serve_an_eval_source():
    """T-4659: `eval-*` sources hold the eval fixtures (T-4144's injection page among them) and are
    public for the eval copies alone. No McpServer, no hub list and no AssistantDeployment other
    than an eval copy names one, so `search_documents` and the public chat never serve it."""
    named = []
    for project in sorted(p.name for p in SEED.iterdir() if p.is_dir()):
        for doc in documents(project):
            spec = doc.get("spec") or {}
            if doc["kind"] == "McpServer":
                named += [(doc["metadata"]["name"], s) for s in spec.get("knowledgeSources", [])]
            elif doc["kind"] == "AssistantDeployment" and "eval-" not in doc["metadata"]["name"]:
                named += [(doc["metadata"]["name"], s) for s in spec.get("sources", [])]
    values = (SEED.parent / "values/gateway/base-values.yaml.gotmpl").read_text()
    (hub,) = [line.split(":", 1)[1].strip().strip('"') for line in values.splitlines() if "JC_GATEWAY_HUB_KNOWLEDGE_SOURCES:" in line]
    named += [("hub", entry) for entry in hub.split(",")]
    assert any(owner == "hub" for owner, _ in named)
    leaked = [(owner, s) for owner, s in named if str(s).split("/")[-1].startswith("eval-")]
    assert leaked == [], leaked
