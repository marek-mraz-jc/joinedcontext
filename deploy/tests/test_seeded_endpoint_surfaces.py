"""Every hand-written seeded Endpoint serves WebSocket notifications and OGC API Features, and every
Policy a person or the public reads through grants the subscription operations a WebSocket needs
(T-4073, EP-104, EP-106). No entity write rides along: a reader keeps the entity operations it had.

App Endpoints (`generated-by: portal/app-reconciler`) are compiled from their App's dataNeeds: they
serve `ogc-features` too (T-4074), and `websocket` only where the App asks, since a subscription
operation is a write for an App (AP-09)."""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
SUBSCRIPTIONS = {"createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"}
READS = {"retrieveOps", "queryEntity", "retrieveEntity"}
# The only entity write a reader Policy holds: a steward's correction of a docking station.
READER_WRITES = {"updateAttrs"}
ENTITY_WRITES = {
    "createEntity", "updateEntity", "appendAttrs", "updateAttrs", "deleteAttrs", "deleteEntity", "createBatch",
    "upsertBatch", "updateBatch", "deleteBatch", "mergeEntity", "replaceEntity", "replaceAttrs", "mergeBatch",
    "purgeEntity", "upsertTemporal", "appendAttrsTemporal", "deleteAttrsTemporal", "updateAttrInstanceTemporal",
    "deleteAttrInstanceTemporal", "deleteTemporal", "writeOps",
}
# The open-data catalogue is no dataset (Architecture/21 §10, T-4525): its door serves NGSI-LD and
# MCP for an agent's first call, no WebSocket or OGC Features, so its public reader holds no
# subscription operation; its one writer is the Portal reconciler, which lists what it wrote and
# so holds retrieveOps beside its batch writes. These files, and only these, are outside the
# dataset rules below. An organization's catalogue (T-5493) is a published dataset: its door and
# public reader keep the rules, and only its Portal write is outside them.
CATALOGUE = {
    "slovensko-endpoint-catalog.yaml",
    "slovensko-policy-catalog-public-read.yaml",
    "slovensko-policy-catalog-portal-write.yaml",
    "bbsk-policy-catalog-portal-write.yaml",
    "policy-catalog-portal-write.yaml",
    "zilina-policy-catalog-portal-write.yaml",
    "praha-policy-catalog-portal-write.yaml",
    "helsinki-policy-catalog-portal-write.yaml",
}


def manifests(kind: str, generated: bool = False) -> list[tuple[Path, dict]]:
    found = []
    for path in sorted(SEED.glob("*/*.yaml")):
        try:
            docs = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        for doc in docs:
            if isinstance(doc, dict) and doc.get("kind") == kind:
                by = doc["metadata"].get("annotations", {}).get("joinedcontext.com/generated-by")
                if (by == "portal/app-reconciler") == generated and path.name not in CATALOGUE:
                    found.append((path, doc))
    return found


def readers() -> list[tuple[Path, dict]]:
    return [(path, doc) for path, doc in manifests("Policy")
            if doc["spec"]["assignee"]["kind"] != "serviceAccount" and READS & set(doc["spec"]["operations"])]


def test_every_seeded_endpoint_serves_websocket_and_ogc_features():
    endpoints = manifests("Endpoint")
    assert len(endpoints) >= 23
    for path, doc in endpoints:
        assert {"websocket", "ogc-features"} <= set(doc["spec"]["enabledRepresentations"]), path.name


def test_every_reader_policy_grants_the_subscription_operations():
    policies = readers()
    assert len(policies) >= 23
    for path, doc in policies:
        assert SUBSCRIPTIONS <= set(doc["spec"]["operations"]), path.name


def test_an_endpoint_naming_its_policy_names_a_reader_that_subscribes():
    by_urn = {}
    for _, doc in manifests("Policy"):
        meta = doc["metadata"]
        by_urn[meta["name"]] = doc
    named = [(path, doc) for path, doc in manifests("Endpoint") if doc["spec"].get("policyRef")]
    assert named
    for path, doc in named:
        policy = by_urn[doc["spec"]["policyRef"].rsplit(":", 1)[1]]
        assert SUBSCRIPTIONS <= set(policy["spec"]["operations"]), path.name


def test_no_reader_policy_gains_an_entity_write():
    for path, doc in readers():
        spec = doc["spec"]
        writes = ENTITY_WRITES & set(spec["operations"])
        assert writes <= READER_WRITES, path.name
        if writes:
            assert spec["assignee"]["kind"] == "user", path.name


def test_every_app_endpoint_serves_ogc_features():
    endpoints = manifests("Endpoint", generated=True)
    assert len(endpoints) >= 18
    for path, doc in endpoints:
        assert "ogc-features" in doc["spec"]["enabledRepresentations"], path.name


def test_the_catalogue_exception_names_files_that_exist():
    assert {path.name for path in SEED.glob("*/*.yaml")} >= CATALOGUE


# The file representations an Endpoint no longer publishes: file.csv and the APIs stay (owner
# 2026-10-18, T-4597, T-4601). The gateway refuses a manifest naming one.
REMOVED = {"xlsx", "parquet", "json", "zip", "geojson"}
APPS = SEED.parent.parent / "gitea/apps"


def test_no_seed_or_app_names_a_removed_representation():
    found = []
    for path in sorted([*SEED.rglob("*.yaml"), *APPS.rglob("*.yaml")]):
        try:
            docs = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        stack = [d for d in docs if d is not None]
        while stack:
            node = stack.pop()
            if isinstance(node, dict):
                for key, value in node.items():
                    if key in ("enabledRepresentations", "disabledRepresentations", "representations") \
                            and isinstance(value, list):
                        found += [f"{path.relative_to(SEED.parent.parent)}: {key} {v}" for v in value if v in REMOVED]
                    stack.append(value)
            elif isinstance(node, list):
                stack.extend(node)
    assert len(list(APPS.rglob("app.yaml"))) > 0
    assert not found, found


NUMERIC = {"integer", "float", "double", "decimal"}


def first(nodes: list[dict], key: str):
    """The slot_usage's value of `key`, else the global slot's (LinkML's own precedence)."""
    return next((n[key] for n in nodes if n.get(key) is not None), None)


def measurements(linkml: dict) -> list[tuple[str, str]]:
    """jc-core `linkml_measurements`: each (class, slot) whose range is numeric and that declares a
    `unit`, the Datastream.unitOfMeasurement SensorThings needs (STA 1.1 §9.2.5, EP-125)."""
    found = []
    slots = linkml.get("slots") or {}
    for name, declared in (linkml.get("classes") or {}).items():
        declared = declared or {}
        for slot in declared.get("slots") or []:
            nodes = [(declared.get("slot_usage") or {}).get(slot) or {}, slots.get(slot) or {}]
            if first(nodes, "range") in NUMERIC and first(nodes, "unit") is not None:
                found.append((name, slot))
        for slot, attribute in (declared.get("attributes") or {}).items():
            attribute = attribute or {}
            if attribute.get("range") in NUMERIC and attribute.get("unit") is not None:
                found.append((name, slot))
    return found


def test_sta_is_listed_only_where_the_space_model_has_a_measurement():
    """`sta` makes every numeric Property a Datastream, which needs its unit (EP-125, T-5299); the
    Portal and jcctl validate refuse an Endpoint whose space model declares none, so a seed
    listing `sta` there would never apply."""
    models = {}
    for path, doc in manifests("DataModel"):
        if doc["spec"].get("linkml") and doc["spec"].get("lifecycle") in ("published", "deprecated"):
            linkml = yaml.safe_load((path.parent / doc["spec"]["linkml"]).read_text())
            models[(doc["metadata"]["namespace"], doc["spec"]["contextSpaceRef"])] = linkml
    listed = [(path, doc) for path, doc in manifests("Endpoint") if "sta" in doc["spec"]["enabledRepresentations"]]
    for path, doc in listed:
        space = doc["spec"]["contextSpaceRef"]
        space = space["name"] if isinstance(space, dict) else space
        model = models.get((doc["metadata"]["namespace"], space))
        assert model is not None, f"{path.name}: `sta` needs a published data model in space {space}"
        assert measurements(model), f"{path.name}: `sta` needs a numeric attribute that declares its unit in {space}"


def test_measurements_reads_slot_usage_global_slots_and_attributes():
    linkml = {
        "slots": {"pm10": {"range": "float", "unit": {"ucum_code": "ug/m3"}}, "count": {"range": "integer"}},
        "classes": {
            "Air": {"slots": ["pm10", "count"]},
            "Article": {"slots": ["count"], "attributes": {"words": {"range": "integer"}}},
            "Road": {"slots": ["count"], "slot_usage": {"count": {"unit": {"ucum_code": "1"}}},
                     "attributes": {"length": {"range": "float", "unit": {"ucum_code": "km"}}}},
        },
    }
    assert measurements(linkml) == [("Air", "pm10"), ("Road", "count"), ("Road", "length")]
