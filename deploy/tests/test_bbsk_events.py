"""The events of Slovakia in one space, the Banská Bystrica Region a view of it (T-4895).

The model fits the iCalendar surface (EP-125: the type Event with `name` and `startDate`
required) and the owner's rule that an event without a name, a start or a GPS point is not
published (2026-10-20); the two Endpoints serve the calendar beside NGSI-LD and OGC Features and
publish Event alone; nobody but the pipelines' account writes. These cases read the seed; the live
feed and the anonymous 403 are `dev-smoke`'s ("events calendar"), the rendering is Antares'
crates/antares-ical/tests/ical.rs.
"""

import copy
import json
from pathlib import Path

import jsonschema
import pytest
import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/bbsk"
SPACE = "bbsk-podujatia"
ENDPOINTS = {"bbsk-podujatia": "podujatia-kraj-read"}
WRITE = "bbsk-podujatia-zapis"
# EP-125's iCalendar requirement (platform crates/jc-core/src/kinds/endpoint.rs ICAL): the source
# attribute of each RFC 5545 field, and the ones the model must mark required.
ICAL_SOURCES = {"startDate", "name", "endDate", "description", "address", "location", "url"}
ICAL_REQUIRED = {"startDate", "name"}
SLOVAKIA = (16.8, 47.7, 22.6, 49.7)


def manifests():
    for path in sorted(SEED.glob("*.yaml")):
        if path.name.endswith(("-bento.yaml", ".linkml.yaml")) or path.name == "index.yaml":
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and "kind" in doc:
                yield path, doc


def of_space(kind):
    return {d["metadata"]["name"]: d for _, d in manifests() if d["kind"] == kind and space_of(d) == SPACE}


def space_of(doc):
    ref = doc.get("spec", {}).get("contextSpaceRef")
    return ref.get("name") if isinstance(ref, dict) else ref


LINKML = yaml.safe_load((SEED / "bbsk-podujatia.linkml.yaml").read_text())
SCHEMA = json.loads((SEED / "bbsk-podujatia.v1.schema.json").read_text())


def event_schema():
    schema = copy.deepcopy(SCHEMA)
    schema.update({"$ref": "#/definitions/Event"})
    return schema


VALID = jsonschema.Draft7Validator(event_schema())


def event(**changes):
    """A key-values Event as the pipelines write it, with `changes` applied (None drops a member)."""
    e = {
        "id": "urn:ngsi-ld:Event:bbsk-podujatia:3f1c2a9b8d7e6f50",
        "type": "Event",
        "name": "Vianočné trhy na námestí",
        "startDate": "2026-12-05T10:00:00+01:00",
        "endDate": "2026-12-05T20:00:00+01:00",
        "location": {"type": "Point", "coordinates": [19.1462, 48.7363]},
        "venue": "Námestie SNP",
        "address": {"addressLocality": "Banská Bystrica", "addressRegion": "Banskobystrický kraj", "addressCountry": "SK"},
        "eventCategory": "trhy",
        "url": "https://www.banskabystrica.sk/podujatia/vianocne-trhy/",
        "organizer": {"name": "Mesto Banská Bystrica", "url": "https://www.banskabystrica.sk"},
        "source": "https://www.banskabystrica.sk/wp-json/tribe/events/v1/events",
        "sourceId": "4711",
    }
    for key, value in changes.items():
        if value is None:
            e.pop(key, None)
        else:
            e[key] = value
    return e


def inside_slovakia(e):
    lon, lat = e["location"]["coordinates"]
    return SLOVAKIA[0] <= lon <= SLOVAKIA[2] and SLOVAKIA[1] <= lat <= SLOVAKIA[3]


def test_the_space_names_its_one_model_and_keeps_no_history():
    space = of_space("ContextSpace").get(SPACE) or next(
        d for _, d in manifests() if d["kind"] == "ContextSpace" and d["metadata"]["name"] == SPACE)
    assert space["spec"]["dataModelRef"] == {"kind": "DataModel", "name": SPACE}
    assert space["spec"]["retention"] == [
        {"entityType": "Event", "history": "off"}, {"entityType": "PointOfInterest", "history": "off"}]
    [rule] = space["spec"]["quality"]
    assert (rule["entityType"], rule["attribute"], tuple(rule["bbox"])) == ("Event", "location", SLOVAKIA)
    [model] = of_space("DataModel").values()
    # The events and the venues they link (T-5364, DM-141).
    assert model["spec"]["classes"] == ["Event", "PointOfInterest"] and model["spec"]["lifecycle"] == "published"


def test_the_event_class_fits_the_ical_surface_and_the_owners_required_fields():
    """EP-125 refuses `ical` on a model whose Event leaves name or startDate optional; the owner
    adds a link (2026-10-21, T-5081); GPS and organizer are expected, not required (owner 2026-10-22).
    jcctl validate applies the first rule to the Endpoints."""
    cls = LINKML["classes"]["Event"]
    assert cls["class_uri"] == "schema:Event"
    required = {s for s, u in cls["slot_usage"].items() if u.get("required")}
    assert required == ICAL_REQUIRED | {"url"}
    assert ICAL_SOURCES - {"location"} <= set(cls["slots"]) | {"location"}
    assert set(SCHEMA["definitions"]["Event"]["required"]) == {"id", "type", "name", "startDate", "url"}
    # The organizer is a name and maybe a web site, nothing else: no person's e-mail or phone fits it.
    organizer = SCHEMA["definitions"]["Organizer"]
    assert (organizer["required"], set(organizer["properties"]), organizer["additionalProperties"]) == (["name"], {"name", "url"}, False)
    # schema.org names, the ones the surface reads (antares-ical mapping.rs).
    slots = LINKML["slots"]
    assert {slots[s]["slot_uri"] for s in ("name", "startDate", "endDate", "description", "url")} == {
        "schema:name", "schema:startDate", "schema:endDate", "schema:description", "schema:url"}
    assert slots["organizer"]["slot_uri"] == "schema:organizer"


@pytest.mark.parametrize("fixture", [
    pytest.param(event(), id="timed, Europe/Bratislava offset"),
    pytest.param(event(startDate="2026-07-24", endDate="2026-07-24"), id="all-day"),
    pytest.param(event(startDate="2026-07-24", endDate="2026-07-26"), id="multi-day"),
    pytest.param(event(startDate="2026-03-29T01:30:00+01:00", endDate="2026-03-29T03:30:00+02:00"), id="across the DST change"),
    pytest.param(event(endDate=None), id="missing end"),
    pytest.param(event(endDate=None, venue=None, address=None, eventCategory=None, organizer={"name": "Stredoslovenské múzeum"}), id="with organizer"),
])
def test_an_event_the_calendar_serves_is_valid(fixture):
    assert list(VALID.iter_errors(fixture)) == []
    assert inside_slovakia(fixture)


@pytest.mark.parametrize("fixture", [
    pytest.param(event(endDate=None, venue=None, address=None, eventCategory=None, location=None, organizer=None), id="the required alone"),
    pytest.param(event(location=None), id="no GPS"),
    pytest.param(event(organizer=None), id="no organizer"),
])
def test_an_event_without_gps_or_organizer_is_kept(fixture):
    # Owner 2026-10-22: only name, startDate and url refuse an event; GPS is expected and filled, never a refusal.
    assert list(VALID.iter_errors(fixture)) == []


@pytest.mark.parametrize(("fixture", "why"), [
    pytest.param(event(name=None), "'name' is a required property", id="no name"),
    pytest.param(event(startDate=None), "'startDate' is a required property", id="no start"),
    pytest.param(event(organizer={"url": "https://www.umb.sk"}), "is not valid under any of the given schemas", id="an organizer without a name"),
    pytest.param(event(organizer={"name": "Ján Novák", "email": "jan@example.sk"}), "is not valid under any of the given schemas", id="an organizer with an e-mail"),
    pytest.param(event(url=None), "'url' is a required property", id="no link"),
    pytest.param(event(startDate="5. 12. 2026"), "does not match", id="a start that is no date"),
    pytest.param(event(startDate="2026-12-05T10:00:00"), "does not match", id="a time without its offset"),
    pytest.param(event(startDate="2026-12-05T10:00"), "does not match", id="a time without offset or seconds"),
    pytest.param(event(location={"type": "Point"}), "'coordinates' is a required property", id="a point without coordinates"),
])
def test_an_event_without_a_name_a_start_or_a_link_or_with_a_bad_value_is_refused(fixture, why):
    errors = [e.message for e in VALID.iter_errors(fixture)]
    assert any(why in m for m in errors), errors


def test_a_point_outside_slovakia_is_flagged():
    assert not inside_slovakia(event(location={"type": "Point", "coordinates": [0.0, 0.0]}))
    assert not inside_slovakia(event(location={"type": "Point", "coordinates": [48.7363, 19.1462]})), "lat/lon swapped"


def test_two_endpoints_serve_the_calendar_events_and_their_venues():
    # The dashboard's own endpoint is the Portal's to compile (T-4897), tested below; the write
    # door is tested on its own.
    endpoints = {n: e for n, e in of_space("Endpoint").items() if not n.startswith("app-") and n != WRITE}
    policies = of_space("Policy")
    assert set(endpoints) == set(ENDPOINTS)
    for name, policy in ENDPOINTS.items():
        spec = endpoints[name]["spec"]
        assert spec["audience"] == "public"
        assert {"ngsi-ld", "ogc-features", "ical"} <= set(spec["enabledRepresentations"])
        assert spec["policyRef"] == f"urn:ngsi-ld:Policy:bbsk.sk:{SPACE}:{policy}"
        read = policies[policy]["spec"]
        assert read["assignee"] == {"kind": "role", "id": "public"}
        assert read["information"] == [{"entities": [{"type": "Event"}, {"type": "PointOfInterest"}]}]
        assert not [op for op in read["operations"] if op.endswith(("Batch", "Entity", "Attrs", "Temporal")) or op == "writeOps"]
    assert {e["spec"]["slug"] for e in endpoints.values()}.isdisjoint(
        {d["spec"].get("slug") for _, d in manifests() if d["kind"] == "Endpoint" and space_of(d) != SPACE})


def test_the_pipelines_write_through_an_organization_endpoint_the_public_cannot_reach():
    """T-5033: each pipeline writes as its derived account pl-{pipeline} under a derived Policy, which
    an Endpoint binds only without a policyRef; the public read Endpoints keep theirs, so the
    pipelines write through their own door, which serves no anonymous caller and no calendar."""
    spec = of_space("Endpoint")[WRITE]["spec"]
    assert spec["audience"] == "organization"
    assert "policyRef" not in spec and "publish" not in spec and "catalog" not in spec
    assert "ical" not in spec["enabledRepresentations"]
    targets = {d["spec"]["targetEndpoint"] for _, d in manifests() if d["kind"] == "Pipeline"
               and d["metadata"]["name"].startswith("podujatia-")}
    assert targets == {f"urn:ngsi-ld:Endpoint:bbsk.sk:{SPACE}:{WRITE}"}
    # jc-core pipeline_identity::derive grants pl-{pipeline} upsertBatch/createBatch/queryBatch of the
    # output type in the target's space, deleteBatch only with spec.expiry: Event writes, no delete.
    for _, d in manifests():
        if d["kind"] == "Pipeline" and d["metadata"]["name"].startswith("podujatia-"):
            assert d["spec"]["output"] == {"type": "Event", "mode": "upsert"}, d["metadata"]["name"]
            assert "expiry" not in d["spec"] and "outputs" not in d["spec"], d["metadata"]["name"]


def test_the_region_reads_its_own_scope_and_the_national_store_is_its_own_space():
    """The region's dataset narrows by scope; Slovakia's events live in slovensko-podujatia (T-5047)."""
    policies = of_space("Policy")
    assert policies["podujatia-kraj-read"]["spec"]["scopeQ"] == "/SK/BB/#"
    assert "podujatia-read" not in policies


def test_only_the_pipelines_write_and_never_delete():
    writers = {n: p["spec"] for n, p in of_space("Policy").items() if p["spec"]["assignee"]["kind"] != "role"}
    assert list(writers) == ["podujatia-pipelines-write"]
    spec = writers["podujatia-pipelines-write"]
    assert spec["assignee"] == {"kind": "serviceAccount", "id": "pipelines"}
    assert set(spec["operations"]) == {"upsertBatch", "createBatch", "queryBatch"}
    assert spec["information"] == [{"entities": [{"type": "Event"}, {"type": "PointOfInterest"}]}]


def test_every_file_is_placed_by_the_index():
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    # T-4896's pipelines and T-4897's App, placed outside spaces/ or tested on their own.
    generated = ("bbsk-datasource-", "bbsk-pipeline-", "bbsk-app-")
    ours = sorted(p.name for p in SEED.glob("bbsk-*podujatia*") if not p.name.startswith(generated))
    assert len(ours) == 11
    assert [f for f in ours if f not in index] == []
    assert {index[f].split("/spaces/")[1].split("/")[0] for f in ours} == {SPACE}


GENERATED = {"joinedcontext.com/generated-by": "portal/app-reconciler"}


def test_the_dashboard_reads_the_region_through_its_own_public_endpoint_and_writes_nothing():
    """T-4897: the events dashboard is a public static App the Portal image ships. Its one need is
    Event in this space, narrowed to the region's scope; the Endpoint and Policy the Portal compiles
    for it are seeded beside it, public, read only, and they serve the calendar it links."""
    (app,) = [d for _, d in manifests() if d["kind"] == "App" and d["metadata"]["name"] == "bbsk-podujatia"]
    assert app["metadata"]["namespace"] == "bbsk"
    assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
    spec = app["spec"]
    assert (spec["kind"], spec["visibility"], spec["lifecycle"]) == ("ui", "public", "published")
    assert spec["source"] == {"path": "./apps/bbsk-podujatia"}
    (need,) = spec["dataNeeds"]
    assert need["contextSpaceRef"]["name"] == SPACE
    assert need["types"] == ["Event"] and need["scopeQ"] == "/SK/BB/#"
    assert need["operations"] == ["queryEntity", "retrieveEntity"]
    assert "ical" in need["representations"]

    endpoint = of_space("Endpoint")["app-bbsk-podujatia"]
    assert endpoint["metadata"]["annotations"] == GENERATED
    assert endpoint["spec"]["audience"] == "public"
    assert set(endpoint["spec"]["enabledRepresentations"]) >= {"ngsi-ld", "ical"}
    policy = of_space("Policy")["app-bbsk-podujatia-1"]
    assert policy["metadata"]["annotations"] == GENERATED
    grant = policy["spec"]
    assert grant["assignee"] == {"kind": "role", "id": "endpoint:bbsk/app-bbsk-podujatia"}
    assert grant["scopeQ"] == "/SK/BB/#"
    assert grant["operations"] == ["queryEntity", "retrieveEntity"]
    assert [e["type"] for e in grant["information"][0]["entities"]] == ["Event"]
    assert grant["information"][0]["propertyNames"] == need["attrs"]

    index = yaml.safe_load((SEED / "index.yaml").read_text())
    assert index["bbsk-app-podujatia.yaml"] == "projects/bbsk/apps/bbsk-podujatia/app.yaml"
    assert index["bbsk-app-podujatia-endpoint.yaml"] == f"projects/bbsk/spaces/{SPACE}/endpoints/app-bbsk-podujatia.yaml"
    assert index["bbsk-app-podujatia-policy.yaml"] == f"projects/bbsk/spaces/{SPACE}/policies/app-bbsk-podujatia-1.yaml"
