"""The public transport endpoint publishes HSL's transport types and nothing else of the space
(T-3895, EP-14): it evaluates its one Policy, and that Policy names exactly these types, read only."""

from pathlib import Path

import yaml

HELSINKI = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/helsinki"

TRANSPORT = {"Vehicle", "GtfsStop", "PublicTransportRoute", "GtfsStation", "GtfsAgency", "GtfsRoute", "GtfsShape",
             "GtfsDailyDepartures", "Alert", "OffStreetParking"}


def load(name: str) -> dict:
    return yaml.safe_load((HELSINKI / name).read_text())


def test_a_public_reader_of_the_endpoint_sees_the_transport_types_and_nothing_else():
    endpoint = load("helsinki-endpoint-transport.yaml")["spec"]
    policy = load("helsinki-policy-public-transport.yaml")
    assert endpoint["audience"] == "public"
    assert endpoint["policyRef"] == f"urn:ngsi-ld:Policy:hel.fi:helsinki:{policy['metadata']['name']}"
    spec = policy["spec"]
    assert spec["assignee"] == {"kind": "role", "id": "public"}
    # T-4049: a citizen subscribes over the WebSocket; reading stays the only entity operation.
    assert spec["operations"] == [
        "retrieveOps", "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveEntityTypeInfo",
        "retrieveAttrTypes", "retrieveAttrTypeDetails", "retrieveAttrTypeInfo",
        "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription",
    ]
    granted = [entity["type"] for rule in spec["information"] for entity in rule["entities"]]
    assert sorted(granted) == sorted(TRANSPORT)
    # Every granted type is a class of the space's model, so none is a typo that grants nothing.
    model = load("helsinki.linkml.yaml")
    assert TRANSPORT <= set(model["classes"])
    # The rest of the space (events, bikes, weather, permits, districts...) stays out.
    assert {"Event", "BikeHireDockingStation", "WeatherObserved", "CityWork", "AdministrativeArea"} & set(granted) == set()


def test_the_endpoint_projects_no_internal_attribute_and_credits_its_sources():
    endpoint = load("helsinki-endpoint-transport.yaml")["spec"]
    assert "contactPoint" in endpoint["projection"]["hiddenAttributes"]
    catalog = endpoint["catalog"]
    assert catalog["license"] == "CC_BY_4_0"
    assert "Fintraffic" in catalog["attribution"]["en"] and "HSL" in catalog["attribution"]["en"]
    sources = {source["url"] for source in catalog["source"]}
    assert {"https://hsl.fi/en/hsl/open-data", "https://parking.fintraffic.fi/api/v1"} <= sources


def test_the_live_vehicles_reach_a_map_over_the_endpoints_public_websocket():
    """T-4035: the owner's order, the WebSocket is public. Since T-4073 every hand-written seeded
    Endpoint serves it too (test_seeded_endpoint_surfaces.py)."""
    endpoint = load("helsinki-endpoint-transport.yaml")["spec"]
    assert "websocket" in endpoint["enabledRepresentations"]
    # T-4067: the same vehicles as OGC API Features, for QGIS and the open data portal.
    assert "ogc-features" in endpoint["enabledRepresentations"]
    assert endpoint["audience"] == "public"


READS = {"retrieveOps", "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveEntityTypeInfo",
         "retrieveAttrTypes", "retrieveAttrTypeDetails", "retrieveAttrTypeInfo", "queryTemporal", "retrieveTemporal",
         "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"}


def test_only_the_pipelines_account_writes_a_vehicle_and_through_the_write_endpoint():
    """T-5046: every HSL vehicle lands through the hsl-hfp-vehicles pipeline's door only. Its
    target binds the pipeline's derived Policy (no policyRef, T-5033), and every Policy of the space
    that lets anyone change a Vehicle is the pipelines account's; the public reads every mode."""
    pipeline = load("helsinki-pipeline-hfp.yaml")["spec"]
    assert pipeline["targetEndpoint"] == "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"
    assert "policyRef" not in load("helsinki-endpoint-all.yaml")["spec"]
    writers = []
    for path in sorted(HELSINKI.glob("helsinki-policy-*.yaml")):
        spec = load(path.name)["spec"]
        types = {entity.get("type") for rule in spec.get("information", []) for entity in rule.get("entities", [])}
        if set(spec["operations"]) - READS and (not types or "Vehicle" in types):
            writers.append((path.name, spec["assignee"]))
    assert writers == [("helsinki-policy-pipelines-write.yaml", {"kind": "serviceAccount", "id": "pipelines"})]
    (vehicle,) = [entity for rule in load("helsinki-policy-public-transport.yaml")["spec"]["information"]
                  for entity in rule["entities"] if entity["type"] == "Vehicle"]
    assert vehicle == {"type": "Vehicle"}
