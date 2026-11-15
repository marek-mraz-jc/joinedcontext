"""A pipeline's batch query carries its selector in the body, never in the URL (T-4108).

`POST /entityOperations/query` takes only its window as query parameters (CIM 009 clause 5.6.9,
6.3.20); the broker's view answers any other with 400 InvalidRequest. On dev an `attrs=type` left
in three Helsinki lookups failed every ask: no free spaces reached the park and ride sites, no HSL
alert was written, and no site was linked to its station.
"""

from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from conftest import seed_documents

SEED = Path(__file__).resolve().parents[1] / "components/context-gateway/seed"
WINDOW = {"limit", "offset", "count", "options", "format", "local"}


def urls(node):
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "url" and isinstance(value, str):
                yield value
            else:
                yield from urls(value)
    elif isinstance(node, list):
        for item in node:
            yield from urls(item)


def test_every_seed_batch_query_url_names_only_its_window():
    asked = {}
    for path, document in seed_documents():
        for url in urls(document):
            if "/entityOperations/query" in url:
                asked[f"{path.relative_to(SEED)}: {url}"] = set(parse_qs(urlsplit(url).query)) - WINDOW
    assert asked, "the Helsinki lookups ask the space through a batch query"
    assert {where: extra for where, extra in asked.items() if extra} == {}
