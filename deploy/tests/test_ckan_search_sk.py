"""T-5116: a visitor finds a dataset whatever letters and ending the Slovak word was typed with.

The catalogue's Solr (the pinned ckan-solr image, CKAN's own schema) is started with the 102
public datasets of opendata.dev.joinedcontext.com (fixtures/ckan-search/datasets.json, read on
2026-10-22 from package_search: name, title, notes, tags, groups, organization, resource names
and the two worded extras). Each of the 58 searches in queries.json is sent the way CKAN 2.11's
package_search sends it (ckan/lib/search/query.py: dismax, qf, mm, q.op AND), after the theme's
before_dataset_search rewrite. recall@5 per search is the share of its expected datasets (at
most five) in the first five results.

Measured 2026-10-22: stock CKAN 0.739, the theme's stemmer before T-5116 0.977 ("lekáreň" and
"dôchodok" missed their declined forms), with the fleeting-vowel rule 1.0.
"""

import json
import shutil
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

import pytest
import yaml
from test_ckan_theme import theme  # noqa: F401  (the plugin module, loaded as the theme tests load it)

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests/fixtures/ckan-search"
DATASETS = json.loads((FIXTURES / "datasets.json").read_text())
QUERIES = json.loads((FIXTURES / "queries.json").read_text())
# ckan/lib/search/query.py, PackageSearchQuery.run, for a q without a field.
CKAN_PARAMS = {"defType": "dismax", "tie": "0.1", "mm": "2<-1 5<80%", "qf": "name^4 title^4 tags^2 groups^2 text",
               "df": "text", "q.op": "AND", "fq": "+site_id:default +state:active", "fl": "name", "rows": 5,
               "sort": "score desc, metadata_modified desc", "wt": "json"}

pytestmark = [
    pytest.mark.skipif(shutil.which("docker") is None, reason="no container runtime for Solr"),
    pytest.mark.xdist_group("docker-ckan-solr-search"),
]


def image() -> str:
    solr = yaml.safe_load((ROOT / "components/ckan/images.yaml").read_text())["ckan"]["solr"]
    return f"{solr['repository']}:{solr['tag']}@{solr['digest']}"


def post(base, path, body):
    request = urllib.request.Request(base + path, json.dumps(body).encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def document(pkg):
    """The fields CKAN's indexer (ckan/lib/search/index.py) writes for a public active dataset."""
    doc = {"index_id": pkg["name"], "id": pkg["name"], "site_id": "default", "name": pkg["name"],
           "title": pkg["title"], "notes": pkg["notes"], "tags": pkg["tags"], "groups": pkg["groups"],
           "organization": pkg["organization"], "res_name": pkg["res_name"], "capacity": "public",
           "state": "active", "entity_type": "package", "dataset_type": "dataset"}
    doc.update({"extras_" + key: value for key, value in pkg["extras"].items()})
    return doc


@pytest.fixture(scope="module")
def solr():
    name = f"t5116-solr-{uuid.uuid4().hex[:12]}"
    subprocess.run(["docker", "run", "-d", "--name", name, "-P", image()], check=True, capture_output=True)
    try:
        port = subprocess.run(["docker", "port", name, "8983/tcp"], check=True, capture_output=True,
                              text=True).stdout.split("\n")[0].rsplit(":", 1)[1]
        base = f"http://127.0.0.1:{port}/solr/ckan"
        deadline = time.time() + 120
        while time.time() < deadline:
            try:
                urllib.request.urlopen(base + "/admin/ping", timeout=5).close()
                break
            except (OSError, urllib.error.HTTPError):
                time.sleep(2)
        else:
            pytest.fail("Solr did not answer its ping within 2 minutes")
        post(base, "/update?commit=true", [document(pkg) for pkg in DATASETS])
        yield base
    finally:
        subprocess.run(["docker", "rm", "-f", name], check=False, capture_output=True)


def first_five(base, params):
    url = base + "/select?" + urllib.parse.urlencode({**CKAN_PARAMS, **params})
    with urllib.request.urlopen(url, timeout=30) as response:
        return [doc["name"] for doc in json.load(response)["response"]["docs"]]


def recall(base, rewrite):
    """recall@5 over every search, and the searches that missed, for the failure message."""
    total, missed = 0.0, []
    for case in QUERIES:
        found = first_five(base, rewrite(case["q"]))
        share = len(set(found) & set(case["expect"])) / min(len(case["expect"]), 5)
        total += share
        if share < 1:
            missed.append((case["q"], found))
    return total / len(QUERIES), missed


def test_the_fixture_has_fifty_searches_each_naming_datasets_it_holds():
    names = {pkg["name"] for pkg in DATASETS}
    assert len(QUERIES) >= 50
    assert len({case["q"] for case in QUERIES}) == len(QUERIES)
    for case in QUERIES:
        assert case["expect"] and set(case["expect"]) <= names, case


def test_stock_ckan_misses_the_declined_and_folded_words(solr):
    """The measure tells the two apart: CKAN's English stemmer alone misses "most", "pamiatka"."""
    score, missed = recall(solr, lambda q: {"q": q})
    assert score < 0.9, score
    assert "most" in {q for q, _ in missed}


def test_the_theme_finds_nine_in_ten_datasets_in_the_first_five(solr, theme):  # noqa: F811
    plugin = theme.JcThemePlugin()
    score, missed = recall(solr, lambda q: plugin.before_dataset_search({"q": q}))
    assert score >= 0.9, (score, missed)


def test_a_fleeting_vowel_word_finds_its_declined_forms(solr, theme):  # noqa: F811
    plugin = theme.JcThemePlugin()
    for asked, dataset in (("lekáreň", "bbsk-zoznam-lekarni-v-bbk-c564de"), ("dôchodok", "susr-so")):
        assert dataset in first_five(solr, plugin.before_dataset_search({"q": asked})), asked
