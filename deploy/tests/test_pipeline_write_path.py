"""Every seeded Pipeline writes through an Endpoint that grants its account the write (T-5033).

A Pipeline writes as its own derived account `pl-{pipeline}`, granted upsertBatch, createBatch and
queryBatch on its output's space by a Policy derived with it (PL-19, PL-20; platform
jc-core/src/kinds/pipeline_identity.rs). The gateway (context-gateway/src/store.rs
`endpoints_with_models`) and the Portal's view compiler (reconciler/views.rs `bound`) bind those
derived Policies to an Endpoint only when it names no `policyRef` and is not an App's generated
Endpoint: a `policyRef` binds the one Policy it names, an App's Endpoint its own grants. A Pipeline
aimed at such an Endpoint gets a view without batch writes, and every upsert answers 404 (the
28 bbsk podujatia-* pipelines aimed at the public read Endpoint bbsk-podujatia-slovensko, retired by T-5047).
"""

from pathlib import Path

import pytest
from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"


def documents():
    for path, doc in seed_documents():
        if path.name.endswith(("-bento.yaml", ".linkml.yaml")) or path.name == "index.yaml":
            continue
        if isinstance(doc, dict) and "kind" in doc and isinstance(doc.get("spec"), dict):
            yield path, doc


def space_name(ref):
    return ref["name"] if isinstance(ref, dict) else ref


DOCS = list(documents())
ENDPOINTS = {(d["metadata"]["namespace"], d["metadata"]["name"]): d["spec"] for _, d in DOCS if d["kind"] == "Endpoint"}
APPS = {(d["metadata"]["namespace"], d["metadata"]["name"]) for _, d in DOCS if d["kind"] == "App"}
TARGETS = [
    pytest.param(path, doc["metadata"]["namespace"], output["targetEndpoint"],
                 id=f"{doc['metadata']['namespace']}/{doc['metadata']['name']}#{index}")
    for path, doc in DOCS if doc["kind"] == "Pipeline"
    for index, output in enumerate(doc["spec"].get("outputs") or [doc["spec"]])
    if "targetEndpoint" in output
]


def test_the_seed_has_pipelines_to_check():
    assert len(TARGETS) > 100


@pytest.mark.parametrize(("path", "project", "target"), TARGETS)
def test_a_pipelines_target_endpoint_binds_its_derived_write_policy(path, project, target):
    # urn:ngsi-ld:Endpoint:{org}:{space}:{name}
    parts = target.split(":")
    assert len(parts) == 6 and parts[:3] == ["urn", "ngsi-ld", "Endpoint"], f"{path.name}: {target}"
    space, name = parts[4], parts[5]
    spec = ENDPOINTS.get((project, name))
    assert spec is not None, f"{path.name}: no Endpoint {project}/{name}, so its pipeline is granted nothing"
    assert space_name(spec["contextSpaceRef"]) == space, f"{path.name}: {name} serves another space than {target}"
    assert "policyRef" not in spec, (
        f"{path.name}: {name} binds only {spec.get('policyRef')}, never the pipeline's own write Policy"
    )
    app = name.removeprefix("app-")
    assert not (name.startswith("app-") and (project, app) in APPS), f"{path.name}: {name} is the App {app}'s own Endpoint"


def test_a_policy_ref_endpoint_refuses_the_target_a_pipeline_needs():
    """The check above fails on the defect it guards: the read Endpoint the pipelines named before."""
    spec = ENDPOINTS[("slovensko", "slovensko-podujatia")]
    assert spec["audience"] == "public" and "policyRef" in spec
