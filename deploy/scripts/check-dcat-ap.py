#!/usr/bin/env python3
"""Is the catalogue valid DCAT-AP 3? (T-3010)

usage: check-dcat-ap.py <catalogue-url>          e.g. https://opendata.dev.joinedcontext.com
       check-dcat-ap.py --file <document.ttl>...

Reads /catalog.ttl, every page of it, then /dataset/{id}.ttl for each dataset the catalogue
names, and validates each document against the DCAT-AP 3.0.1 SHACL shapes in dcat-ap-3.0.1/
(github.com/SEMICeu/DCAT-AP, releases/3.0.1/shacl: the cardinalities and the ranges, the pair
the data.europa.eu validator applies). One line per document. Exits 1 on a violation, on a
document with no dcat:Dataset in it and on a catalogue that names no dataset: an empty graph
conforms to every shape, and a harvester finds nothing in it.
"""

import sys
import urllib.error
import urllib.request
from pathlib import Path

from pyshacl import validate
from rdflib import Graph, URIRef
from rdflib.namespace import DCAT, RDF, SH

SHAPES = Path(__file__).resolve().parent / "dcat-ap-3.0.1"
HYDRA_NEXT = URIRef("http://www.w3.org/ns/hydra/core#nextPage")


def shapes() -> Graph:
    graph = Graph()
    for path in sorted(SHAPES.glob("*.ttl")):
        graph.parse(path)
    # The published ranges.ttl names five property shapes it never defines, and pySHACL
    # refuses the whole shapes graph over them; a reference to nothing constrains nothing.
    for shape, prop in list(graph.subject_objects(SH.property)):
        if (prop, None, None) not in graph:
            graph.remove((shape, SH.property, prop))
    return graph


def violations(data: Graph, shapes_graph: Graph, holds=DCAT.Dataset) -> list[str]:
    """Every violation in plain words; the document has to hold at least one `holds`."""
    if (None, RDF.type, holds) not in data:
        return [f"no {holds.n3(data.namespace_manager)} in the document"]
    _, report, _ = validate(data, shacl_graph=shapes_graph, inference="none", allow_warnings=True)
    found = set()
    for result in report.subjects(SH.resultSeverity, SH.Violation):
        found.add(
            f"{report.value(result, SH.focusNode)} {report.value(result, SH.resultPath)}: "
            f"{report.value(result, SH.resultMessage)}"
        )
    return sorted(found)


def fetch(url: str) -> Graph:
    request = urllib.request.Request(url, headers={"Accept": "text/turtle"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return Graph().parse(data=response.read(), format="turtle")


def report(name: str, problems: list[str]) -> bool:
    print(f"{'PASS' if not problems else 'FAIL'} {name}")
    for problem in problems:
        print(f"  {problem}")
    return not problems


def check_catalogue(base: str, shapes_graph: Graph) -> bool:
    ok, datasets, url = True, [], base.rstrip("/") + "/catalog.ttl"
    while url:
        try:
            page = fetch(url)
        except (urllib.error.URLError, ValueError) as error:
            return report(url, [f"cannot read it: {error}"])
        ok &= report(url, violations(page, shapes_graph, DCAT.Catalog))
        datasets += page.objects(None, DCAT.dataset)
        url = str(page.value(None, HYDRA_NEXT) or "")
    if not datasets:
        return report(base, ["the catalogue names no dataset"])
    for dataset in sorted(set(datasets)):
        try:
            graph = fetch(f"{dataset}.ttl")
        except (urllib.error.URLError, ValueError) as error:
            ok &= report(f"{dataset}.ttl", [f"cannot read it: {error}"])
            continue
        ok &= report(f"{dataset}.ttl", violations(graph, shapes_graph))
    return ok


def main(argv: list[str]) -> int:
    if len(argv) >= 3 and argv[1] == "--file":
        shapes_graph = shapes()
        ok = all([report(path, violations(Graph().parse(path), shapes_graph)) for path in argv[2:]])
    elif len(argv) == 2 and argv[1].startswith(("http://", "https://")):
        ok = check_catalogue(argv[1], shapes())
    else:
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
