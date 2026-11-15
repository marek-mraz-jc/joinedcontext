"""Every seeded Pipeline holds a write grant for what it writes (T-5717).

A Pipeline writes as its own account pl-{pipeline} under a Policy derived for it (PL-19, PL-20,
jc-core pipeline_identity::derive): upsert, create and query on the space of each output's Endpoint,
over the output's one type. The Portal compiles every Policy of a space, the derived ones included,
into one rule document per broker tenant (reconciler views.rs rule_document) and PUTs it to Antares
`/q/policy/{tenant}`. On dev, two seed shapes left pipelines with no grant, each run refused 403
"no policy grants this operation":

- slovensko-podujatia's document passed Antares' old 64 KiB bound at 302 pipelines; the broker
  refused it and kept the revision from before 35 new sources, so each of them wrote nothing.
- a mapping writing a second entity type (GtfsAgency beside GtfsRoute) outside its one derived type.
"""

import json
import re
from functools import cache
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"

# Antares antares-ops views/rule_documents.rs MAX_DOCUMENT_BYTES and MAX_DOCUMENT_VALUES, raised
# from 64 KiB and 8192 by T-5717. A document is held to half of each: the estimate below leaves
# out the views map and writes each operation as one clause.
MAX_DOCUMENT_BYTES = 1024 * 1024
MAX_DOCUMENT_VALUES = 131_072
OLD_MAX_DOCUMENT_BYTES = 64 * 1024

# An entity id a mapping writes as a literal: "id": "urn:ngsi-ld:Type:..." or {"type": "Type", ...}.
# Pipelines that run a shared mapping in a narrower mode than its text reads: the Decidim light pass
# (seeds/decidim/pipeline.yaml, cfg.light) reads only the running processes' budgets, BudgetProject.
NARROWED = re.compile(r"^decidim-[a-z0-9-]+-hlasovanie$")
LITERAL_ID = re.compile(r'"id":\s*(?:"urn:ngsi-ld:([A-Z]\w*):|\{\s*"type":\s*"([A-Z]\w*)")')


@cache
def manifests() -> tuple[tuple[Path, dict], ...]:
    found = []
    for path in sorted(SEED.rglob("*.yaml")):
        try:
            loaded = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        found += [(path, doc) for doc in loaded if isinstance(doc, dict) and "kind" in doc]
    return tuple(found)


def of(kind: str) -> list[tuple[Path, dict]]:
    return [(path, doc) for path, doc in manifests() if doc["kind"] == kind]


def name_of(ref) -> str:
    return ref["name"] if isinstance(ref, dict) else ref


@cache
def endpoint_spaces() -> dict[tuple[str, str], str]:
    return {
        (doc["metadata"]["namespace"], doc["metadata"]["name"]): name_of(doc["spec"]["contextSpaceRef"])
        for _, doc in of("Endpoint")
    }


def outputs(spec: dict) -> list[dict]:
    """Pipeline outputs() of jc-core: the v1alpha2 list and each route's, or the v1alpha1 one."""
    if spec.get("outputs") or spec.get("routes"):
        found = list(spec.get("outputs") or [])
        for route in spec.get("routes") or []:
            found += route.get("outputs") or []
        return [{"endpoint": o["targetEndpoint"], "type": o.get("type")} for o in found]
    if not spec.get("targetEndpoint"):
        return []
    return [{"endpoint": spec["targetEndpoint"], "type": (spec.get("output") or {}).get("type")}]


def derived() -> list[tuple[str, str, str, dict]]:
    """(project, space, pipeline, policy document) per write grant, as the Portal writes it."""
    found = []
    for _, doc in of("Pipeline"):
        project, name = doc["metadata"]["namespace"], doc["metadata"]["name"]
        for at, output in enumerate(outputs(doc["spec"]), start=1):
            space = endpoint_spaces().get((project, output["endpoint"].rsplit(":", 1)[-1]))
            if space is None:
                continue
            policy = {
                "id": f"{project}/pl-{name}-w-{at}",
                "effect": "permit",
                "assignee": {"kind": "serviceAccount", "id": f"pl-{name}"},
                "operations": ["5.6.8", "5.6.7", "5.7.2", "5.6.5"],
            }
            if output["type"]:
                policy["information"] = [{"entities": [{"type": output["type"]}]}]
            found.append((project, space, name, policy))
    return found


def documents() -> dict[tuple[str, str], list[dict]]:
    docs: dict[tuple[str, str], list[dict]] = {}
    for project, space, _, policy in derived():
        docs.setdefault((project, space), []).append(policy)
    for _, doc in of("Policy"):
        spec = doc["spec"]
        policy = {
            "id": f"{doc['metadata']['namespace']}/{doc['metadata']['name']}",
            "effect": spec.get("effect", "permit"),
            "assignee": spec.get("assignee"),
            "operations": spec.get("operations") or [],
        }
        for key in ("information", "validity", "q", "scopeQ", "geoQ", "temporalQ"):
            if spec.get(key):
                policy[key] = spec[key]
        key = (doc["metadata"]["namespace"], name_of(spec["contextSpaceRef"]))
        docs.setdefault(key, []).append(policy)
    return docs


def values(doc) -> int:
    if isinstance(doc, dict):
        return 1 + sum(values(v) for v in doc.values())
    if isinstance(doc, list):
        return 1 + sum(values(v) for v in doc)
    return 1


def size(policies: list[dict]) -> int:
    return len(json.dumps({"policies": policies}, separators=(",", ":"), ensure_ascii=False).encode())


def test_every_space_rule_document_fits_the_broker_bound():
    over = [
        f"{project}/{space}: {len(policies)} policies, {size(policies)} bytes, "
        f"{values({'policies': policies})} values"
        for (project, space), policies in sorted(documents().items())
        if size(policies) > MAX_DOCUMENT_BYTES // 2 or values({"policies": policies}) > MAX_DOCUMENT_VALUES // 2
    ]
    assert not over, (
        "these spaces' rule documents near Antares' bound: past it the broker refuses the document "
        "and every pipeline added since writes nothing (T-5717); split the space's pipelines or raise "
        f"the bound in Antares first: {over}"
    )


def test_the_estimate_finds_the_document_the_old_bound_refused():
    # The 302 podujatia pipelines of 2026-10-24 crossed 64 KiB on dev; the estimate must see it.
    assert size(documents()[("slovensko", "slovensko-podujatia")]) > OLD_MAX_DOCUMENT_BYTES


@cache
def granted() -> dict[tuple[str, str], set[str]]:
    """The types each pl- account holds through a Policy of the seed, by (project, account)."""
    found: dict[tuple[str, str], set[str]] = {}
    for _, doc in of("Policy"):
        assignee = doc["spec"].get("assignee") or {}
        if not str(assignee.get("id", "")).startswith("pl-"):
            continue
        types = {
            selector["type"]
            for info in doc["spec"].get("information") or []
            for selector in info.get("entities") or []
        }
        found.setdefault((doc["metadata"]["namespace"], assignee["id"]), set()).update(types)
    return found


def written_types(pipeline: Path) -> set[str]:
    bento = pipeline.with_name(pipeline.name.replace(".yaml", "-bento.yaml"))
    if pipeline.name == "pipeline.yaml":
        bento = pipeline.with_name("bento.yaml")
    if not bento.exists():
        return set()
    return {a or b for a, b in LITERAL_ID.findall(bento.read_text())}


def test_every_type_a_mapping_writes_is_granted_to_its_pipeline():
    refused = []
    for path, doc in of("Pipeline"):
        project, name = doc["metadata"]["namespace"], doc["metadata"]["name"]
        declared = {o["type"] for o in outputs(doc["spec"])}
        if None in declared or not declared or NARROWED.match(name):
            continue  # no type named: the derived grant covers the space's model (PL-60)
        extra = written_types(path) - declared - granted().get((project, f"pl-{name}"), set())
        if extra:
            refused.append(f"{project}/{name} writes {sorted(extra)} beyond {sorted(declared)}")
    assert not refused, (
        "these pipelines write a type their derived Policy does not grant, so Antares refuses those "
        "entities 403; add a Policy for the pl- account naming them (helsinki-policy-gtfs-routes-"
        f"account.yaml): {refused}"
    )


def test_the_type_check_reads_a_literal_id_and_a_urn_template():
    text = '"id": "urn:ngsi-ld:Proposal:%s".format(x)\n"id": { "type": "GtfsAgency", "id": $id }.apply("urn")'
    assert {a or b for a, b in LITERAL_ID.findall(text)} == {"Proposal", "GtfsAgency"}
    assert not LITERAL_ID.findall('"object": { "type": "GtfsStation", "id": $p }')
