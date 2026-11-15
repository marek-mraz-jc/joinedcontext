"""One catalogue dataset is one Smart Data Model type, or a group of types that reference each other
(T-4344, owner 2026-10-16). Every seeded Endpoint that publishes to CKAN serves only types joined by
relationships, serves every mandatory surface (EP-118), and is narrowed to types the space already
grants the public, under the public role, so a split exposes nothing new."""

import re
from functools import cache
from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"

# Which type references which, read from dev's public NGSI-LD on 2026-10-16 (the Relationship
# attributes of up to 100 entities per type of each published Endpoint):
#   GET /api/endpoint/{slug}/ngsi-ld/v1/entities?type={T}&limit=100
# PublicTransportRoute (TransitRoute before T-4329) carries no Relationship: it is HSL's stops of
# one line, keyed by the GtfsRoute's id.
# GtfsShape has no entity on dev yet; GTFS draws a shape for the trips of one route.
# GtfsDailyDepartures carries no Relationship either: it counts the GtfsRoute lines of one mode.
REFERENCES = {
    "WasteContainer": {"WasteContainerIsle"},
    "WasteContainerIsle": {"AdministrativeArea"},
    "GtfsRoute": {"GtfsAgency"},
    "GtfsStop": {"GtfsStation"},
    "PublicTransportRoute": {"GtfsRoute"},
    "GtfsShape": {"GtfsRoute"},
    "GtfsDailyDepartures": {"GtfsRoute"},
    "Vehicle": {"GtfsStop"},
    "Alert": {"GtfsStop", "GtfsRoute"},
    "OffStreetParking": {"GtfsStation"},
    "WaterQualityObserved": {"PointOfInterest"},
    # An event names its venue (T-5364, DM-141).
    "Event": {"PointOfInterest"},
    "CityWork": {"AdministrativeArea"},
    # A street segment names the paid parking section of its tariff (T-5424, refParkingSite).
    "ParkingGroup": {"OnStreetParking"},
    # Citizen participation (T-5065): every record names its process; a Result its parent Result.
    "Proposal": {"ParticipatoryProcess"},
    "BudgetProject": {"ParticipatoryProcess"},
    "Result": {"ParticipatoryProcess", "Result"},
    "Meeting": {"ParticipatoryProcess"},
    # An organization's catalogue (T-5493): a Dataset names its Organization (refPublisher) and its
    # DataServices (refDataService), jc-core ngsi_catalog.rs.
    "Dataset": {"Organization", "DataService"},
    "Debate": {"ParticipatoryProcess"},
    "Survey": {"ParticipatoryProcess"},
}
# What the bundles served before the split; none of them may be a dataset again.
BUNDLES = {"praha-mesto", "helsinki-all", "banskabystrica-verejne", "zilina-verejne", "bbsk-registre"}
MANDATORY = {"ngsi-ld", "mcp", "websocket", "csv"}
# OGC API Features is mandatory when the data has a location (EP-118); these types have none.
UNLOCATED = {"KeyPerformanceIndicator", "BudgetLine", "Article", "GtfsAgency"}


@cache
def docs(kind: str) -> list[tuple[Path, dict]]:
    found = []
    for path in sorted(SEED.glob("*/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == kind:
                found.append((path, doc))
    return found


def space_of(spec: dict) -> str:
    ref = spec["contextSpaceRef"]
    return ref["name"] if isinstance(ref, dict) else ref


@cache
def public_policies() -> dict[tuple[str, str], dict[str, dict]]:
    """The public role's Policies, by (project, space), then by name."""
    out: dict[tuple[str, str], dict[str, dict]] = {}
    for _, doc in docs("Policy"):
        spec = doc["spec"]
        if spec["assignee"] == {"kind": "role", "id": "public"}:
            out.setdefault((doc["metadata"]["namespace"], space_of(spec)), {})[doc["metadata"]["name"]] = spec
    return out


def types_of(policy: dict) -> set[str]:
    return {e["type"] for info in policy.get("information", []) for e in info.get("entities", [])}


def published() -> list[tuple[Path, dict, dict[str, dict]]]:
    """Each CKAN-published Endpoint with the public Policies it binds, as the gateway binds them:
    its policyRef's one Policy, else every Policy of its space (store.rs `bound_policy`)."""
    policies = public_policies()
    out = []
    for path, doc in docs("Endpoint"):
        spec = doc["spec"]
        if not spec.get("publish", {}).get("ckan"):
            continue
        of_space = policies.get((doc["metadata"]["namespace"], space_of(spec)), {})
        ref = spec.get("policyRef")
        bound = {n: p for n, p in of_space.items() if ref is None or n == ref.rsplit(":", 1)[1]}
        out.append((path, doc, bound))
    return out


def connected(types: set[str]) -> bool:
    if not types:
        return False
    edges = {t: set() for t in types}
    for a, refs in REFERENCES.items():
        for b in refs:
            if a in types and b in types:
                edges[a].add(b)
                edges[b].add(a)
    seen, todo = set(), [next(iter(types))]
    while todo:
        t = todo.pop()
        if t not in seen:
            seen.add(t)
            todo.extend(edges[t] - seen)
    return seen == types


def test_relationship_check_tells_a_group_from_a_bundle():
    assert connected({"WasteContainerIsle", "WasteContainer"})
    assert connected({"GtfsAgency", "GtfsRoute", "GtfsStop", "GtfsStation", "Vehicle", "Alert", "OffStreetParking", "PublicTransportRoute"})
    assert connected({"BudgetLine"})
    assert not connected({"AirQualityObserved", "Event", "Organization", "WeatherObserved"})
    assert not connected({"Hospital", "Bridge"})
    assert not connected(set())


def test_every_published_dataset_serves_one_model_or_a_referencing_group():
    rows = published()
    assert len(rows) >= 30
    for path, doc, bound in rows:
        assert bound, f"{path.name}: publishes but binds no public Policy"
        types = set().union(*(types_of(p) for p in bound.values()))
        assert connected(types), f"{path.name}: {sorted(types)} are unrelated types in one dataset"


def test_no_bundle_is_published_any_more():
    names = {doc["metadata"]["name"] for _, doc, _ in published()}
    assert not names & BUNDLES


def test_every_published_dataset_serves_every_mandatory_surface():
    for path, doc, bound in published():
        served = set(doc["spec"]["enabledRepresentations"])
        types = set().union(*(types_of(p) for p in bound.values()))
        mandatory = MANDATORY if types <= UNLOCATED else MANDATORY | {"ogc-features"}
        assert mandatory <= served, f"{path.name} lacks {sorted(mandatory - served)}"
        assert "zip" not in served, path.name
        assert doc["spec"]["audience"] == "public", path.name


def test_every_published_events_dataset_serves_a_calendar():
    """Owner 2026-10-26 (T-5763): every events dataset offers iCalendar, one named output at least,
    so the catalogue row lists /ical/v1/{name}.ics."""
    events = []
    for path, doc, bound in published():
        if "Event" not in set().union(*(types_of(p) for p in bound.values())):
            continue
        events.append(path.name)
        spec = doc["spec"]
        assert "ical" in spec["enabledRepresentations"], f"{path.name} serves no iCalendar"
        outputs = spec.get("representations", {}).get("ical", [])
        assert isinstance(outputs, list) and outputs, f"{path.name} names no ical output"
        assert all(o.get("timezone") and o.get("title") for o in outputs), path.name
    # Barcelona events left the catalogue while prod holds none (T-5788).
    assert len(events) >= 6, events


def test_a_split_policy_grants_nothing_its_space_did_not_grant_the_public():
    policies = public_policies()
    split = [(path, doc) for path, doc in docs("Policy") if "(T-4344)" in path.read_text()]
    assert len(split) >= 22
    for path, doc in split:
        spec = doc["spec"]
        assert spec["assignee"] == {"kind": "role", "id": "public"}, path.name
        assert not spec.get("constraints"), path.name
        others = policies[(doc["metadata"]["namespace"], space_of(spec))]
        wider = set().union(*(types_of(p) for n, p in others.items() if n != doc["metadata"]["name"]))
        assert types_of(spec) <= wider, f"{path.name}: {sorted(types_of(spec) - wider)} were not public"


def test_a_helsinki_part_hides_the_contact_line_as_the_bundle_does():
    for path, doc, _ in published():
        if doc["metadata"]["namespace"] == "helsinki" and doc["spec"].get("projection") is not None:
            assert "contactPoint" in doc["spec"]["projection"]["hiddenAttributes"], path.name
    for path, doc in docs("Endpoint"):
        if path.name.startswith("helsinki-endpoint-") and doc["spec"].get("publish") and doc["spec"].get("policyRef"):
            types = types_of(public_policies()[("helsinki", "helsinki")][doc["spec"]["policyRef"].rsplit(":", 1)[1]])
            if types & {"PointOfInterest", "Event", "BikeHireDockingStation", "CityWork", "AdministrativeArea",
                        "OnStreetParking", "WaterQualityObserved", "Vehicle", "GtfsStop"}:
                assert "contactPoint" in doc["spec"].get("projection", {}).get("hiddenAttributes", []), path.name


def test_every_endpoint_and_policy_of_the_split_is_in_its_seed_index():
    for path, doc, bound in published():
        index = (path.parent / "index.yaml").read_text()
        assert f"\n{path.name}:" in index, path.name


# A type name as a quoted literal of a mapping: the osm mapping picks "EVChargingStation" or
# "PointOfInterest" per OSM object while its Pipeline declares only the second (T-5643).
LITERAL = re.compile(r'"([A-Z][A-Za-z0-9]*)"')
# The Portal's catalogue sync writes each organisation's catalogue space (jc-core ngsi_catalog.rs)
# under a Policy granting its reconciler account the batch writes.
RECONCILER = {"kind": "user", "id": "service-account-portal-reconciler"}
# Endpoints that advertise a type nothing feeds yet, each with its reason; an entry whose gap is
# closed fails the test, so the list only shrinks.
# bratislava: T-5271 seeded AirQualityObserved as the model's first class, and the Bratislava hub
# has no air sensor feed (T-5273); a pipeline (EEA stations, as banskabystrica public-air) or
# dropping the class closes it.
UNFED = {
    "bratislava/bratislava-endpoint.yaml": ["AirQualityObserved"],
    # The legacy uploads whose one copy was the CKAN DataStore, filled back from this Endpoint (T-4742):
    # their entities stay in the Endpoint and no Pipeline reads the catalogue for them (T-5060, T-5729).
    "banskabystrica/banskabystrica-od-endpoint.yaml": ["Contract", "CouncilMeeting", "Grant", "KeyPerformanceIndicator",
                                                       "PurchaseOrder"],
    # The conformance suites' own door (F-BNT1-002): their fixtures live only for a run, no Pipeline
    # feeds the space, and the Endpoint is neither public nor catalogued.
    "banskabystrica/endpoint-conformance.yaml": ["AirQualityObserved"],
}


def fed_types() -> dict[tuple[str, str], set[str]]:
    """What can fill each (project, space): its Pipelines' declared output types (some Helsinki
    Pipelines declare none, their mapping names the type) and the type literals of their mappings (inline or the -bento.yaml beside the Pipeline), and the types a
    ContextSourceRegistration federates into it."""
    spaces = {(doc["metadata"]["namespace"], doc["metadata"]["name"]): space_of(doc["spec"])
              for _, doc in docs("Endpoint")}
    out: dict[tuple[str, str], set[str]] = {}
    for path, doc in docs("Pipeline"):
        spec, ns = doc["spec"], doc["metadata"]["namespace"]
        outputs = spec.get("outputs") or [{**spec.get("output", {}), "targetEndpoint": spec.get("targetEndpoint")}]
        bento = path.with_name(f"{path.stem}-bento.yaml")
        literals = set(LITERAL.findall(path.read_text() + (bento.read_text() if bento.exists() else "")))
        for o in outputs:
            space = spaces.get((ns, o["targetEndpoint"].rsplit(":", 1)[1]))
            assert space, f"{path.name}: targets {o['targetEndpoint']}, no seeded Endpoint"
            out.setdefault((ns, space), set()).update({o.get("type")} - {None} | literals)
    for _, doc in docs("ContextSourceRegistration"):
        out.setdefault((doc["metadata"]["namespace"], space_of(doc["spec"])), set()).update(types_of(doc["spec"]))
    for _, doc in docs("Policy"):
        spec = doc["spec"]
        if spec["assignee"] == RECONCILER and "upsertBatch" in spec["operations"]:
            out.setdefault((doc["metadata"]["namespace"], space_of(spec)), set()).update(types_of(spec))
    return out


def advertised(doc: dict) -> set[str]:
    """The types get_endpoint lists: its policyRef's Policy's, else its space's DataModel's."""
    spec, ns = doc["spec"], doc["metadata"]["namespace"]
    if ref := spec.get("policyRef"):
        [policy] = [p["spec"] for _, p in docs("Policy")
                    if (p["metadata"]["namespace"], p["metadata"]["name"]) == (ns, ref.rsplit(":", 1)[1])]
        return types_of(policy)
    [space] = [s["spec"] for _, s in docs("ContextSpace")
               if (s["metadata"]["namespace"], s["metadata"]["name"]) == (ns, space_of(spec))]
    if not (model := space.get("dataModelRef")):
        return set()
    return {t for _, m in docs("DataModel")
            if (m["metadata"]["namespace"], m["metadata"]["name"]) == (ns, model["name"])
            for t in m["spec"].get("classes", [])}


def unfed(fed: dict[tuple[str, str], set[str]]) -> dict[str, list[str]]:
    out = {}
    for path, doc in docs("Endpoint"):
        missing = advertised(doc) - fed.get((doc["metadata"]["namespace"], space_of(doc["spec"])), set())
        if missing:
            out[f"{path.parent.name}/{path.name}"] = sorted(missing)
    return out


def test_every_endpoint_type_has_something_that_writes_it_in_its_space():
    """An Endpoint that names a type nothing of its space writes advertises an empty dataset
    (T-5643: get_endpoint listed EVChargingStation, the space held none)."""
    assert len(docs("Endpoint")) >= 400
    assert unfed(fed_types()) == UNFED


def test_the_check_names_an_endpoint_whose_type_lost_its_writer():
    fed = fed_types()
    fed[("banskabystrica", "banskabystrica-verejne")].discard("EVChargingStation")
    gaps = unfed(fed)
    lost = {name: types for name, types in gaps.items() if name not in UNFED}
    assert {"banskabystrica/endpoint-nabijanie.yaml", "banskabystrica/endpoint-verejne.yaml"} <= lost.keys()
    assert all(types == ["EVChargingStation"] for types in lost.values()), lost


def test_every_catalogue_source_carries_what_jc_core_requires():
    """jc-core's catalog `Source` requires `url`, `title` and `description` and refuses any other
    field (catalog.rs, deny_unknown_fields): one source without its description makes the Portal
    refuse the whole Endpoint, so it never publishes (T-5648: all 339 of Bratislava's)."""
    checked = 0
    for path, doc in docs("Endpoint"):
        for source in (doc["spec"].get("catalog") or {}).get("source") or []:
            checked += 1
            assert set(source) == {"url", "title", "description"}, (
                f"{path.name}: catalog.source {source.get('url')} has {sorted(source)}"
            )
            for field in ("title", "description"):
                texts = source[field]
                assert isinstance(texts, dict) and texts and all(
                    isinstance(t, str) and t.strip() for t in texts.values()
                ), f"{path.name}: catalog.source {source['url']} {field} is empty"
    assert checked >= 300
