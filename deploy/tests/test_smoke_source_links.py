"""scripts/smoke-source-links.py against recorded answers (T-4802): the bridges of prod bbsk-demo,
read through the multi-type Registre kraja, link the dataset bbsk-mosty of the Bridge-only
Endpoint over the same space; a type no dataset publishes links nothing."""

import csv
import importlib.util
import json
import time
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/smoke-source-links.py"
spec = importlib.util.spec_from_file_location("smoke_source_links", SCRIPT)
links = importlib.util.module_from_spec(spec)
spec.loader.exec_module(links)

DOMAIN = "smartcity.example.org"
API = f"https://api.{DOMAIN}/api/endpoint"
REGISTRE, MOSTY = "lws6dd7o56veegwxsjtvak3goeta4mfn", "omto6qk5sfn5ft7zl5pdiaxunj2ljnae"


def mosty(example):
    base = f"{API}/{MOSTY}"
    return {"name": "bbsk-mosty", "extras": [{"key": "endpoint", "value": f"{base}/"}], "resources": [
        {"url": f"{base}/ngsi-ld/v1/", "description":
            f"The first ten:\n\n    curl '{base}/ngsi-ld/v1/entities?type={example}&limit=10'\n"},
        {"url": f"{base}/odata/v4/", "description": "OData"},
        {"url": f"https://opendata.{DOMAIN}/datastore/dump/1", "name": "Bridge"}]}


def answers(example):
    first = {"Bridge": "urn:ngsi-ld:Bridge:bbsk.sk:bbsk-registre:M1006",
             "PointOfInterest": "urn:ngsi-ld:PointOfInterest:bbsk.sk:bbsk-registre:P1"}
    table = {
        f"https://opendata.{DOMAIN}/api/3/action/package_search?rows=1000&start=0":
            {"result": {"count": 1, "results": [mosty(example)]}},
        f"https://api.{DOMAIN}/catalog.jsonld": {"dcat:dataset": [{"@id": f"{API}/{MOSTY}"}]},
        # The door over the space has no catalogue record: the hub lends it (T-4802).
        f"https://api.{DOMAIN}/api/mcp/bbsk/otvorene-data": {"result": {"content": [{"type": "text", "text":
            "slug | title | space | access | types | domain\n"
            f"{REGISTRE} | Registers of the region | bbsk-registre | public | Bridge,PointOfInterest | x\n"
            "zzzzprivate | Private | bbsk-registre | organization | Bridge | x"}]}},
        f"{API}/{MOSTY}/ngsi-ld/v1/types": {"typeList": ["Bridge"]},
    }
    for slug, kinds in ((REGISTRE, ["Bridge", "PointOfInterest"]), (MOSTY, ["Bridge"])):
        for kind in kinds:
            table[f"{API}/{slug}/ngsi-ld/v1/entities?type={kind}&limit=1&pick=id"] = [{"id": first[kind]}]
    return table


class Recorded:
    def __init__(self, table):
        self.table, self.count, self.asked = table, 0, []

    def fetch(self, url, method="GET", headers=None, body=None, limit=0):
        self.count += 1
        return self._send(method, url, headers, body, limit)

    # The parallel per-Endpoint reads (smoke-catalogue's count pass) go through the client's send.
    def _send(self, method, url, headers, body, limit):
        self.asked.append(url)
        if url not in self.table:
            return links.smoke.Answer(404, "application/json", b"{}")
        if self.table[url] == 429:
            return links.smoke.Answer(429, "application/json", b"{}")
        return links.smoke.Answer(200, "application/json", json.dumps(self.table[url]).encode())

    _clock = staticmethod(time.monotonic)
    _sleep = staticmethod(lambda seconds: None)


def table(path):
    with path.open(newline="") as handle:
        return {(r["endpoint"], r["type"]): r for r in csv.DictReader(handle, delimiter="\t")}


def test_rows_of_the_space_link_the_dataset_that_names_their_type(tmp_path):
    client = Recorded(answers("Bridge"))
    assert links.run(DOMAIN, "prod", tmp_path, client, now="T") == 0
    # T-5903: a type one Endpoint alone serves needs no first entity, the run's budget goes to shared ones.
    assert not [u for u in client.asked if "type=PointOfInterest" in u]
    assert [u for u in client.asked if "type=Bridge" in u]
    rows = table(tmp_path / "smoke-source-links-prod-T.tsv")
    dataset = f"https://opendata.{DOMAIN}/dataset/bbsk-mosty"
    assert rows[(REGISTRE, "Bridge")]["resolved"] == dataset
    assert rows[(MOSTY, "Bridge")]["resolved"] == dataset
    assert rows[(REGISTRE, "PointOfInterest")]["resolved"] == "-"
    assert rows[(REGISTRE, "PointOfInterest")]["expected"] == "-"
    fixture = json.loads((tmp_path / "smoke-source-links-prod-T.json").read_text())
    assert fixture["served"][MOSTY] == ["Bridge"]
    assert fixture["owners"] == {MOSTY: [dataset]}
    assert "zzzzprivate" not in fixture["served"], "a non-public Endpoint is not swept"


def test_a_page_that_names_another_type_is_a_ko(tmp_path, capsys):
    # Prod before T-4802: the page's example named AdministrativeArea, a type MOSTY does not serve.
    assert links.run(DOMAIN, "prod", tmp_path, Recorded(answers("AdministrativeArea")), now="T") == 1
    rows = table(tmp_path / "smoke-source-links-prod-T.tsv")
    assert rows[(REGISTRE, "Bridge")]["verdict"] == "ko"
    assert rows[(MOSTY, "Bridge")]["verdict"] == "ok", "its own Endpoint's one page"
    assert "source link " + REGISTRE + " Bridge" in capsys.readouterr().out


def test_a_lasting_rate_limit_judges_nothing(tmp_path, capsys):
    recorded = answers("Bridge")
    recorded[f"{API}/{MOSTY}/ngsi-ld/v1/types"] = 429
    client = Recorded(recorded)
    assert links.run(DOMAIN, "prod", tmp_path, client, now="T") == 1
    assert f"budget ended after 12 requests: 429 after 3 retries on {API}/{MOSTY}/ngsi-ld/v1/types" \
        in capsys.readouterr().out
    assert not list(tmp_path.glob("*.tsv")), "no table from a run the limit cut"
    assert client.count == len(client.asked)


def test_names_that_are_not_plain_never_reach_a_pattern():
    pages = {"https://x/dataset/a": f"{API}/{MOSTY}/odata/v4/Bridge"}
    assert links.resolve(pages, [MOSTY], "Bridge", MOSTY) == "https://x/dataset/a"
    assert links.resolve(pages, [MOSTY], "Br.dge", "other") == ""
    assert links.resolve(pages, ["a|b"], "Bridge", "other") == ""
    assert links.resolve(pages, [MOSTY], "BridgeX", "other") == ""
