"""The events model's SHACL (LinkML -> SHACL, as each Endpoint serves it at schema/v1/model.shacl.ttl): name,
startDate and url are required, organizer and location are not (owner 2026-10-22, T-5135)."""

import shutil
from pathlib import Path

import pytest
from linkml.generators.shaclgen import ShaclGenerator
from linkml_runtime.utils.schemaview import SchemaView
from pyshacl import validate
from rdflib import RDF, BNode, Graph, Literal, URIRef

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / "components/context-gateway/seed/bbsk/bbsk-podujatia.linkml.yaml"
CORE = Path(__file__).resolve().parent / "fixtures/linkml/ngsi-ld-core.yaml"


@pytest.fixture(scope="module")
def model(tmp_path_factory):
    work = tmp_path_factory.mktemp("model")
    shutil.copy(MODEL, work / MODEL.name)
    shutil.copy(CORE, work / CORE.name)
    shapes = Graph().parse(data=ShaclGenerator(str(work / MODEL.name)).serialize(), format="turtle")
    return shapes, SchemaView(str(work / MODEL.name))


def graph(view: SchemaView, event: dict) -> Graph:
    """The event's key-values as triples, each member on its slot's URI."""
    g = Graph()
    uri = lambda slot: URIRef(view.get_uri(view.induced_slot(slot, "Event"), expand=True))
    subject = URIRef(event["id"])
    g.add((subject, RDF.type, URIRef(view.get_uri(view.get_class("Event"), expand=True))))
    g.add((subject, uri("type"), Literal("Event")))
    for slot, value in event.items():
        if slot in ("id", "type") or value is None:
            continue
        if slot == "organizer":
            node = BNode()
            g.add((subject, uri(slot), node))
            g.add((node, RDF.type, URIRef(view.get_uri(view.get_class("Organizer"), expand=True))))
            g.add((node, URIRef(view.get_uri(view.induced_slot("name", "Organizer"), expand=True)), Literal(value["name"])))
        elif slot == "url":
            g.add((subject, uri(slot), URIRef(value)))
        else:
            g.add((subject, uri(slot), Literal(value)))
    return g


EVENT = {
    "id": "urn:ngsi-ld:Event:bbsk-podujatia:0123456789abcdef",
    "type": "Event",
    "name": "Koncert v knižnici",
    "startDate": "2026-11-05T18:00:00+01:00",
    "url": "https://svkbb.eu/udalosti/koncert/",
    "organizer": {"name": "Štátna vedecká knižnica"},
    "venue": "Štátna vedecká knižnica",
}


@pytest.mark.parametrize("changes", [
    pytest.param({}, id="whole"),
    pytest.param({"organizer": None}, id="no organizer"),
    pytest.param({"organizer": None, "venue": None}, id="no organizer, no location, no venue"),
])
def test_shacl_accepts_an_event_without_organizer_or_location(model, changes):
    shapes, view = model
    conforms, _, text = validate(graph(view, {**EVENT, **changes}), shacl_graph=shapes)
    assert conforms, text


@pytest.mark.parametrize("missing", ["name", "startDate", "url"])
def test_shacl_refuses_an_event_without_name_start_or_url(model, missing):
    shapes, view = model
    conforms, report, text = validate(graph(view, {**EVENT, missing: None}), shacl_graph=shapes)
    assert not conforms
    path = URIRef(view.get_uri(view.induced_slot(missing, "Event"), expand=True))
    assert path in set(report.objects(None, URIRef("http://www.w3.org/ns/shacl#resultPath"))), text
