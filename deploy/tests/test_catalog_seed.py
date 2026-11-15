"""The NGSI-LD open-data catalogue's seed (T-4525, T-4500): one Endpoint named `catalog` on the
space named `catalog`, read by anyone, written only by the Portal reconciler; and a place on
every space whose Dataset the catalogue locates."""

import json
from pathlib import Path

import yaml
from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
OD_DATASETS = SEED.parent.parent.parent / "charts/ckan/files/od-datasets.json"
WRITES = {"create", "update", "upsert", "delete", "append", "merge", "replace", "purge"}
# An organization's catalogue is a published dataset with a WebSocket surface (T-5493): its public
# reader subscribes, which writes no entity.
SUBSCRIPTIONS = {"createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"}


def manifests():
    for path, doc in seed_documents():
        if path.name == "index.yaml" or path.name.endswith(".linkml.yaml"):
            continue
        if isinstance(doc, dict) and "kind" in doc:
            yield path, doc


def space_of(spec):
    ref = spec.get("contextSpaceRef")
    return ref.get("name") if isinstance(ref, dict) else ref


def test_one_catalog_endpoint_public_on_the_catalog_space_and_placed_in_the_index():
    doors = [(p, d) for p, d in manifests() if d["kind"] == "Endpoint" and d["metadata"]["name"] == "catalog"]
    for path, door in doors:
        assert space_of(door["spec"]) == "catalog" and door["spec"]["audience"] == "public", path
        assert {"ngsi-ld", "mcp"} <= set(door["spec"]["enabledRepresentations"]), path
        project = door["metadata"]["namespace"]
        index = yaml.safe_load((path.parent / "index.yaml").read_text())
        placed = {name for name in index if "catalog" in name}
        assert {p.name for p, d in manifests() if d.get("metadata", {}).get("namespace") == project
                and p.parent == path.parent and "catalog" in d["metadata"]["name"]} <= placed, project
        assert "catalog.linkml.yaml" in placed, project


def segments() -> dict[str, str]:
    """The `catalog` space's urnSegment, by project."""
    return {d["metadata"]["namespace"]: d["spec"]["urnSegment"] for _, d in manifests()
            if d["kind"] == "ContextSpace" and d["metadata"]["name"] == "catalog"}


def test_the_installation_has_one_whole_catalogue_and_each_organization_its_slice():
    # T-5493 (T-4746 owner addendum): the Portal writes all projects to the one Endpoint
    # `catalog` whose space has the segment `catalog`, and a project's own slice to one whose
    # segment is `{project}-catalog`; two whole catalogues stop the wave (reconciler/catalog.rs).
    seg = segments()
    assert [p for p, s in seg.items() if s == "catalog"] == ["slovensko"]
    for project in ["bbsk", "banskabystrica", "zilina", "praha", "helsinki"]:
        assert seg.get(project) == f"{project}-catalog", project
    assert all(s in {"catalog", f"{p}-catalog"} for p, s in seg.items()), seg
    doors = {d["metadata"]["namespace"]: d for _, d in manifests() if d["kind"] == "Endpoint"
             and d["metadata"]["name"] == "catalog"}
    assert set(doors) == set(seg)


def test_each_organizations_catalogue_publishes_to_its_own_ckan_organization():
    ckan = {d["metadata"]["namespace"]: d["metadata"]["name"] for _, d in manifests() if d["kind"] == "CkanInstance"}
    for project, seg in segments().items():
        if seg == "catalog":
            continue
        door = next(d for _, d in manifests() if d["kind"] == "Endpoint" and d["metadata"]["name"] == "catalog"
                    and d["metadata"]["namespace"] == project)
        # A door that holds no record yet stays off the catalogue (T-5767): it publishes nowhere.
        if door["spec"].get("catalog", {}).get("harvest", True) is not False:
            assert door["spec"]["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": ckan[project]}
        else:
            assert "publish" not in door["spec"], project
        # No policyRef: the Portal's write and the public read are both its space's Policies.
        assert "policyRef" not in door["spec"], project
        titles = door["metadata"]["title"]
        assert titles["en"].endswith(" – data catalogue"), titles
        assert any(t.endswith((" – katalóg dát", " – katalog dat", " – datakatalogi")) for t in titles.values()), titles


def test_anyone_reads_the_catalogue_and_only_the_portal_reconciler_writes_it():
    policies = [d for _, d in manifests() if d["kind"] == "Policy" and space_of(d["spec"]) == "catalog"]
    writers = {(d["spec"]["assignee"]["kind"], d["spec"]["assignee"]["id"]) for d in policies
               if any(any(w in op.lower() for w in WRITES) for op in set(d["spec"]["operations"]) - SUBSCRIPTIONS)}
    assert writers == {("user", "service-account-portal-reconciler")}
    public = [d for d in policies if d["spec"]["assignee"] == {"kind": "role", "id": "public"}]
    assert public and all(op.startswith("retrieve") or op in SUBSCRIPTIONS for d in public for op in d["spec"]["operations"])
    for d in policies:
        assert {e["type"] for i in d["spec"]["information"] for e in i["entities"]} == {"Dataset", "DataService", "Organization"}


def test_every_placed_space_has_one_area_in_wgs84():
    placed = {space_of(d["spec"]) for _, d in manifests() if d["kind"] == "Endpoint" and d["spec"].get("audience") == "public"}
    areas = {}
    for _, d in manifests():
        if d["kind"] == "ContextSpace":
            for rule in d["spec"].get("quality") or []:
                if rule["kind"] == "area":
                    areas.setdefault(d["metadata"]["name"], rule["bbox"])
    for name in ["ovzdusie", "banskabystrica-verejne", "banskabystrica-otvorene-data", "bbsk-otvorene-data",
                 "bbsk-registre", "praha-mesto", "ovzdusie-sk", "zilina-verejne", "helsinki"]:
        assert name in placed and name in areas, name
    for name, (w, s, e, n) in areas.items():
        assert -180 <= w < e <= 180 and -90 <= s < n <= 90, name
    # The bb air Dataset's station sits inside its space's area (georel=near around 19.146, 48.736).
    w, s, e, n = areas["ovzdusie"]
    assert w <= 19.146 <= e and s <= 48.736 <= n


def test_every_public_endpoint_is_a_catalogue_dataset_or_a_door_the_feeds_leave_out():
    # T-4744, EP-139: the gateway's harvest feeds list every public Endpoint but an App's door and
    # one with `catalog.harvest: false`; the catalogue smoke counts each listed one without a CKAN
    # dataset as a gap. So a public Endpoint publishes to CKAN, is named by a source dataset of the
    # catalogue chart, or says it is no dataset; `harvest: false` beside a publication is refused.
    named = {d.get("endpoint") for d in json.loads(OD_DATASETS.read_text()).values()}
    gaps, contradictions = [], []
    for path, doc in manifests():
        spec = doc.get("spec") or {}
        if doc["kind"] != "Endpoint" or spec.get("audience") != "public" or spec.get("callerRole"):
            continue
        published = bool((spec.get("publish") or {}).get("ckan"))
        harvested = (spec.get("catalog") or {}).get("harvest", True) is not False
        if harvested and not published and spec.get("slug") not in named:
            gaps.append(str(path.relative_to(SEED)))
        if published and not harvested:
            contradictions.append(str(path.relative_to(SEED)))
    assert not gaps, f"public, in the harvest feeds, and no catalogue dataset names it: {gaps}"
    assert not contradictions, f"harvest: false beside spec.publish.ckan: {contradictions}"


def own_slots(model, name):
    """The Attributes the Portal lists for a class shown whole (reconciler views.rs class_slots):
    its slots and attributes and those of the classes it extends inside the model, less the id."""
    classes, slots, seen, todo = model.get("classes") or {}, set(), set(), [name]
    while todo:
        current = todo.pop()
        definition = classes.get(current)
        if current in seen or not isinstance(definition, dict):
            continue
        seen.add(current)
        slots.update(definition.get("slots") or [], definition.get("attributes") or {})
        todo.extend([definition["is_a"]] if definition.get("is_a") else [])
        todo.extend(definition.get("mixins") or [])
    identifiers = {s for s, d in (model.get("slots") or {}).items() if isinstance(d, dict) and d.get("identifier")}
    return slots - identifiers - {"id", "type"}


def test_every_type_a_policy_shows_whole_has_attributes_in_its_space_model():
    # T-5752: a Policy naming a type without propertyNames shows it whole, and the Portal lists
    # that type's Attributes from the class in the space's model. A class with no slot shows no
    # Attribute, the Portal refuses the Endpoint's view, and the broker answers every read and
    # write of the Endpoint 404: the six catalog Endpoints served nothing on dev.
    linkml = {path.resolve(): doc for path, doc in seed_documents() if path.name.endswith(".linkml.yaml")}
    models = {}
    for path, doc in manifests():
        if doc["kind"] == "DataModel" and doc["spec"].get("linkml"):
            model = linkml.get((path.parent / doc["spec"]["linkml"]).resolve())
            models[(doc["metadata"]["namespace"], space_of(doc["spec"]))] = model
    empty = []
    for path, doc in manifests():
        spec = doc.get("spec") or {}
        if doc["kind"] != "Policy" or spec.get("effect") == "prohibition":
            continue
        model = models.get((doc["metadata"]["namespace"], space_of(spec)))
        for information in spec.get("information") or []:
            if information.get("propertyNames") or information.get("relationshipNames") or model is None:
                continue
            for entity in information.get("entities") or []:
                kind = entity.get("type")
                if kind in (model.get("classes") or {}) and not own_slots(model, kind):
                    empty.append(f"{path.relative_to(SEED)}: {kind}")
    assert not empty, f"shown whole, and the space's model gives the class no Attribute: {empty}"
