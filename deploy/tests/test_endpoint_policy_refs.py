"""Every seed Endpoint names the Policies it binds (P-18, T-6735).

An Endpoint without `policyRefs` evaluates every Policy of its space, so a Policy written for one
Endpoint opens every other Endpoint of that space. scripts/policy-refs-migrate.py wrote the
Policies each seed Endpoint bound into it; these tests keep the seed that way and keep each
reference binding a Policy, as the gateway resolves it (`store.rs` `bound_policies`). A Pipeline's
derived `pl-*` grants are not written: since T-6736 each binds the Endpoint its manifest names,
whatever that Endpoint lists (T-6741).
"""

import importlib.util
import textwrap
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("policy_refs_migrate", ROOT / "scripts/policy-refs-migrate.py")
migrate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migrate)


def seed():
    return list(migrate.documents(sorted(migrate.SEED.rglob("*.yaml"))))


def test_every_seed_endpoint_names_its_policies():
    todo, _ = migrate.plan(seed())
    assert not todo, (
        "Endpoints without policyRefs bind every Policy of their space; run scripts/policy-refs-migrate.py --write: "
        + ", ".join(f"{path.relative_to(ROOT)} {name}" for path, name, _ in todo)
    )


def pipeline_doors(docs):
    """(project, pipeline, endpoint name, writes) of each Endpoint a Pipeline writes or reads through."""
    for _, d in docs:
        if d["kind"] != "Pipeline":
            continue
        project, s = d["metadata"].get("namespace"), d.get("spec") or {}
        for o in [s, *(s.get("outputs") or [])]:
            if isinstance(o, dict) and o.get("targetEndpoint"):
                yield project, d["metadata"]["name"], o["targetEndpoint"].rsplit(":", 1)[-1], True
        for x in [s.get("source") or {}, *(s.get("sources") or [])]:
            if isinstance(x, dict) and x.get("endpointRef"):
                yield project, d["metadata"]["name"], migrate.ref_name(x["endpointRef"]), False


def test_each_pipeline_door_is_an_endpoint_of_its_project():
    # A Pipeline's derived grant binds an Endpoint with policyRefs only when its manifest names that
    # Endpoint of its own project (T-6736 `endpoint_grants`); any other door would answer its writes 404.
    docs = seed()
    endpoints = {(d["metadata"].get("namespace"), d["metadata"]["name"]) for _, d in docs if d["kind"] == "Endpoint"}
    doors = list(pipeline_doors(docs))
    assert doors
    missing = [f"{project}/{pipeline} -> {name}" for project, pipeline, name, _ in doors if (project, name) not in endpoints]
    assert not missing, "Pipelines naming no Endpoint of their project: " + ", ".join(missing)


def test_the_jc_pin_binds_a_pipeline_through_an_endpoint_with_policyrefs():
    # jc builds before T-6736 bind a Pipeline's derived grants only on an Endpoint naming no Policy:
    # a Pipeline's target Endpoint with policyRefs needs the pinned jc to carry T-6736, or dev answers its writes 404.
    docs = seed()
    named = {(d["metadata"].get("namespace"), d["metadata"]["name"]) for _, d in docs
             if d["kind"] == "Endpoint" and ((d.get("spec") or {}).get("policyRefs") or (d.get("spec") or {}).get("policyRef"))}
    if not any(writes and (project, name) in named for project, _, name, writes in pipeline_doors(docs)):
        return
    pin = (ROOT / "components/jc/images.yaml").read_text(encoding="utf-8")
    assert (yaml.safe_load(pin).get("jc") or {}).get("server"), "components/jc/images.yaml: jc.server pin missing"
    server = pin.split("  server:", 1)[0].rsplit("jc:", 1)[-1]
    assert "T-6736" in server, (
        "seed Endpoints a Pipeline writes through name policyRefs: pin a jc build carrying T-6736 (the Pipeline door)"
        " in components/jc/images.yaml and name T-6736 in its comment"
    )


def test_each_reference_binds_a_policy_of_its_space():
    docs = seed()
    by_kind = {}
    for _, doc in docs:
        by_kind.setdefault(doc["kind"], []).append(doc)
    orgs = {d["metadata"]["name"]: (d.get("spec") or {}).get("domain") for d in by_kind["Organization"]}
    domains = {}
    for d in by_kind["Project"]:
        s = d.get("spec") or {}
        domains[d["metadata"]["name"]] = {s.get("domain"), orgs.get(migrate.ref_name(s.get("organizationRef")))} - {None}
    segments = {
        (d["metadata"].get("namespace"), d["metadata"]["name"]): (d.get("spec") or {}).get("urnSegment")
        for d in by_kind["ContextSpace"]
    }
    policies = {
        (d["metadata"].get("namespace"), migrate.ref_name(d["spec"].get("contextSpaceRef")), d["metadata"]["name"].split("#")[0])
        for d in by_kind["Policy"]
    }
    unbound = []
    for d in by_kind["Endpoint"]:
        project, s = d["metadata"].get("namespace"), d.get("spec") or {}
        space = migrate.ref_name(s.get("contextSpaceRef"))
        segment = segments.get((project, space)) or f"{project}-{space}"
        for ref in s.get("policyRefs") or [s.get("policyRef")] * bool(s.get("policyRef")):
            _, _, kind, domain, ref_segment, local_id = ref.split(":")
            known = domains.get(project) or {migrate.INSTALLATION_DOMAIN}
            if kind != "Policy" or domain not in known or ref_segment != segment or (project, space, local_id) not in policies:
                unbound.append(f"{d['metadata']['name']}: {ref}")
    assert not unbound, "references that bind no Policy of the Endpoint's space: " + ", ".join(unbound)


FIXTURE = """
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: air-public
  namespace: city
spec:
  contextSpaceRef: air
  audience: public
"""


def test_migration_writes_exactly_todays_bound_set():
    path = Path("city/endpoint.yaml")
    docs = [
        (Path("org.yaml"), {"kind": "Organization", "metadata": {"name": "city"}, "spec": {"domain": "city.sk"}}),
        (Path("p.yaml"), {"kind": "Project", "metadata": {"name": "city"}, "spec": {"organizationRef": "city"}}),
        (Path("s.yaml"), {"kind": "ContextSpace", "metadata": {"name": "air", "namespace": "city"}, "spec": {}}),
        (Path("a.yaml"), {"kind": "Policy", "metadata": {"name": "read", "namespace": "city"}, "spec": {"contextSpaceRef": "air"}}),
        (Path("b.yaml"), {"kind": "Policy", "metadata": {"name": "write", "namespace": "city"}, "spec": {"contextSpaceRef": {"kind": "ContextSpace", "name": "air"}}}),
        (Path("c.yaml"), {"kind": "Policy", "metadata": {"name": "other", "namespace": "city"}, "spec": {"contextSpaceRef": "noise"}}),
        (Path("d.yaml"), {"kind": "Policy", "metadata": {"name": "read", "namespace": "town"}, "spec": {"contextSpaceRef": "air"}}),
        (Path("app.yaml"), {"kind": "App", "metadata": {"name": "map", "namespace": "city"}, "spec": {}}),
        (Path("e1.yaml"), {"kind": "Endpoint", "metadata": {"name": "app-map", "namespace": "city"}, "spec": {"contextSpaceRef": "air"}}),
        (Path("e2.yaml"), {"kind": "Endpoint", "metadata": {"name": "empty", "namespace": "city"}, "spec": {"contextSpaceRef": "bare"}}),
        (Path("e3.yaml"), {"kind": "Endpoint", "metadata": {"name": "named", "namespace": "city"}, "spec": {"contextSpaceRef": "air", "policyRef": "urn:ngsi-ld:Policy:city.sk:city-air:read"}}),
        (Path("e4.yaml"), {"kind": "Endpoint", "metadata": {"name": "feed-door", "namespace": "city"}, "spec": {"contextSpaceRef": "feed"}}),
        (Path("f.yaml"), {"kind": "Policy", "metadata": {"name": "feed-read", "namespace": "city"}, "spec": {"contextSpaceRef": "feed"}}),
        (Path("g.yaml"), {"kind": "Pipeline", "metadata": {"name": "ingest", "namespace": "city"}, "spec": {"targetEndpoint": "urn:ngsi-ld:Endpoint:city.sk:city-feed:feed-door"}}),
        (path, yaml.safe_load(FIXTURE)),
    ]
    todo, skipped = migrate.plan(docs)
    refs = ["urn:ngsi-ld:Policy:city.sk:city-air:read", "urn:ngsi-ld:Policy:city.sk:city-air:write"]
    # feed-door names its space's manifest Policy only: Pipeline ingest's derived write binds it as its target (T-6736).
    assert todo == [(Path("e4.yaml"), "feed-door", ["urn:ngsi-ld:Policy:city.sk:city-feed:feed-read"]), (path, "air-public", refs)]
    assert [name for _, name, _ in skipped] == ["app-map", "empty"]

    written = yaml.safe_load(migrate.write_refs(textwrap.dedent(FIXTURE), "air-public", refs))
    assert written["spec"]["policyRefs"] == refs
    assert written["spec"]["contextSpaceRef"] == "air" and written["spec"]["audience"] == "public"
