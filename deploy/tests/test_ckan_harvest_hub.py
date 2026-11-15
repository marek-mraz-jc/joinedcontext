"""The harvest of outside catalogues into CKAN (T-4152): BBSK's ArcGIS Hub into `bbsk`.

`charts/ckan/files/harvest-hub.py` runs over answers of the region's Hub recorded on 2026-10-15
(`fixtures/bbsk-hub/`, eight of its 103 items, descriptions shortened) and writes into a CKAN
that answers the action API in-process. The render cases check what the CronJob may reach.
"""

import importlib.util
import json
import threading
import urllib.error
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import quote

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = Path(__file__).resolve().parent / "fixtures/bbsk-hub"

# What the CronJob runs: od-standards.py, then the script (templates/harvest.yaml).
spec = importlib.util.spec_from_file_location("harvest", ROOT / "charts/ckan/files/harvest-hub.py")
harvest = importlib.util.module_from_spec(spec)
exec(compile((ROOT / "charts/ckan/files/od-standards.py").read_text() + "\n"
             + (ROOT / "charts/ckan/files/harvest-hub.py").read_text(), spec.origin, "exec"), harvest.__dict__)
READ = harvest.od_read  # the real read, for a test that fakes the network under it
OD_DATASETS = json.loads((ROOT / "charts/ckan/files/od-datasets.json").read_text())

APS = "b5486911cb0342e48d475130d1d5e960"
OBCE, OKRESY = "40138c783eaa46cd8cd3c8e09ab67127", "04e229d8b39b415497081c63a74c94f9"
CULTURE = "2f936202febe4340a72c1b7224a664d9"
SUPPLIERS = "825cd0cb3b0f46759d14f3bb0d328b59"


def dev_source():
    values = (ROOT / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    # The file is a template only for the `{{"{{"}}source{{"}}"}}` in one comment.
    sources = yaml.safe_load(values.replace('{{"{{"}}', "{{").replace('{{"}}"}}', "}}"))["harvest"]["sources"]
    entry = next(s for s in sources if s.get("kind") == "hub")
    # What the chart puts into HARVEST_CONFIG: the entry, keyed `source` by kind and name.
    return {"source": f"hub-{entry['name']}", **{k: v for k, v in entry.items() if k != "name"}}


def items():
    return [f["properties"] for f in json.loads((FIXTURES / "items.json").read_text())["features"]]


ARTEFACTS = ["context.jsonld", "model.linkml.yaml", "model.md", "model.owl.ttl", "model.rdf.ttl",
             "model.schema.json", "model.shacl.ttl"]


def model_index(types, artefacts=ARTEFACTS):
    """An Endpoint's schema/index.json models, as the gateway answers it (version 1 of one model)."""
    return [{"name": "m", "version": 1, "types": list(types),
             "artifacts": {f: {"bytes": 10, "sha256": f"sha-{f}", "type": "text/plain"} for f in artefacts}}]


def name_of(ident):
    """The CKAN name the harvest gives a kept item, from its corrected title."""
    item, title = next((i, t) for i, t in harvest.select(items(), dev_source())[0] if i["id"] == ident)
    return harvest.dataset(item, title, dev_source())["name"]


# What an Endpoint's file.csv answers (T-4742): one row unless a test names the URL in SHEETS.
SHEET = b"id,name\r\nurn:ngsi-ld:PointOfInterest:1,Hrad\r\n"
SHEETS = {}


def endpoint_file(url, accept="application/json", limit=1 << 30):
    """od_read as the gateway answers it, refusing what is over `limit` as the real one does."""
    body = SHEETS.get(url, SHEET)
    if len(body) > limit:
        raise ValueError(f"{url}: answered more than {limit} bytes")
    return body


@pytest.fixture
def converted(monkeypatch):
    """Marks source items as converted: their rows are entities of an Endpoint whose model has
    every artefact. Returns the setter; no item is converted until it is called."""
    monkeypatch.setattr(harvest, "OD_GATEWAY", "https://api.dev.joinedcontext.com")
    monkeypatch.setattr(harvest, "OD_DATASETS", {})
    monkeypatch.setattr(harvest, "od_models", lambda endpoint: model_index(["PointOfInterest"]))
    monkeypatch.setattr(harvest, "od_read", endpoint_file)

    def mark(*idents, location=True):
        for ident in idents:
            name = name_of(ident)
            harvest.OD_DATASETS[name] = {"endpoint": "ep", "type": "PointOfInterest", "location": location,
                                         "dataset": f"https://opendata.dev.joinedcontext.com/dataset/{name}"}
    return mark


class FakeCkan:
    """CKAN's action API, as much of it as the harvest calls: the organization, search, create,
    update and delete. Every request must carry the token."""

    def __init__(self):
        # The api-token Job creates the organization; a harvest only reads it (F-DEP1-002).
        self.orgs, self.packages, self.calls = {"bbsk": {"name": "bbsk"}}, {}, []
        self.tables, self.dropped, self.views = {}, [], []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                action = self.path.rsplit("/", 1)[-1]
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                assert self.headers["Authorization"] == "token-1"
                fake.calls.append(action)
                status, answer = fake.answer(action, body)
                data = json.dumps(answer).encode()
                self.send_response(status)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.host = f"127.0.0.1:{self.server.server_port}"

    def answer(self, action, body):
        def ok(result):
            return 200, {"success": True, "result": result}

        if action == "organization_show":
            if body["id"] in self.orgs:
                return ok(self.orgs[body["id"]])
            return 404, {"success": False, "error": {"__type": "Not Found Error", "message": "Not found"}}
        if action == "package_search":
            org = body["fq"].split(":", 1)[1]
            # CKAN's index drops a deleted dataset (`ckan.search.remove_deleted_packages`).
            found = [p for p in self.packages.values() if p["owner_org"] == org and p["state"] == "active"]
            return ok({"count": len(found), "results": found[body["start"]:body["start"] + body["rows"]]})
        if action == "package_show":
            found = self.packages.get(body["id"]) or next(
                (p for p in self.packages.values() if p["name"] == body["id"]), None)
            if found:
                return ok(found)
            return 404, {"success": False, "error": {"__type": "Not Found Error", "message": "Not found"}}
        if action == "package_create":
            # The harvest token is an editor (F-DEP1-002): CKAN takes an `id` from a sysadmin only,
            # and keeps a deleted dataset's name in its unique column.
            if "id" in body:
                return 409, {"success": False, "error": {"__type": "Validation Error", "id": ["Unexpected field"]}}
            assert body["name"] not in {p["name"] for p in self.packages.values()}, "name already in use"
            package = dict(body, state="active", id=f"pkg-{len(self.packages)}")
            self.packages[package["id"]] = package
            return ok(package)
        if action == "package_update":
            kept = {r.get("id") for r in body.get("resources", [])}
            for r in self.packages[body["id"]].get("resources", []):
                if r.get("id") not in kept and r.get("id") in self.tables:
                    self.dropped.append(self.tables.pop(r["id"]))  # CKAN drops a table with its resource
            self.packages[body["id"]] = dict(body)
            return ok(body)
        if action == "package_delete":
            self.packages[body["id"]]["state"] = "deleted"
            return ok(None)
        if action == "datastore_create":
            if "resource" in body:
                package = self.packages[body["resource"]["package_id"]]
                rid = f"table-{len(self.tables) + len(self.dropped)}"
                package.setdefault("resources", []).append(dict(body["resource"], id=rid, url_type="datastore"))
            else:
                rid = body["resource_id"]
                assert rid not in self.tables, "datastore_create on a live table"
            self.tables[rid] = {"fields": body["fields"], "records": list(body["records"])}
            return ok({"resource_id": rid})
        if action == "resource_view_list":
            return ok([v for v in self.views if v["resource_id"] == body["id"]])
        if action == "resource_view_create":
            self.views.append(dict(body, id=f"view-{len(self.views)}"))
            return ok(self.views[-1])
        if action == "datastore_upsert":
            self.tables[body["resource_id"]]["records"] += body["records"]
            return ok(None)
        if action == "datastore_delete":
            self.dropped.append(self.tables.pop(body["resource_id"]))
            return ok(None)
        return 400, {"success": False, "error": {"__type": "Bad request", "message": f"Action name not known: {action}"}}

    def active(self):
        return {harvest.extra(p, "source_id"): p for p in self.packages.values() if p["state"] == "active"}


@pytest.fixture
def ckan():
    fake = FakeCkan()
    yield fake
    fake.server.shutdown()


def test_the_selection_keeps_open_licences_and_names_why_each_other_item_is_out():
    kept, skipped = harvest.select(items(), dev_source())
    assert sorted(item["id"] for item, _ in kept) == sorted([OBCE, OKRESY, CULTURE, SUPPLIERS])
    reasons = {ident: reason for ident, _, reason in skipped}
    assert reasons[APS].startswith("personal data")
    assert sorted(r for r in reasons.values() if r.startswith("no open licence")) == [
        "no open licence (CC-BY-NC-4.0)", "no open licence (none)", "no open licence (none)"]
    # The okres and the obec census layer share a title at the source and are two datasets here.
    titles = {item["id"]: title for item, title in kept}
    assert titles[OBCE] == "Obyvateľstvo podľa rodinného stavu (2021), obce"
    assert titles[OKRESY] == "Obyvateľstvo podľa rodinného stavu (2021), okresy"
    assert titles[SUPPLIERS] == "Dodávatelia", "the source's trailing space survived"
    assert all("Bystrinská" not in title for _, title, _ in skipped)


def test_a_repeated_title_keeps_the_newest_copy_and_names_the_other_as_its_duplicate():
    source = dict(dev_source(), titles={})
    kept, skipped = harvest.select(items(), source)
    newer = max((i for i in items() if i["id"] in (OBCE, OKRESY)), key=lambda i: i["modified"])
    older = OKRESY if newer["id"] == OBCE else OBCE
    assert newer["id"] in [item["id"] for item, _ in kept]
    assert (older, "Obyvateľstvo podľa rodinného stavu (2021)", f"duplicate of {newer['id']}") in skipped


def test_a_dataset_carries_licence_publisher_and_none_of_the_hubs_links(converted):
    converted(CULTURE)
    item = next(i for i in items() if i["id"] == CULTURE)
    package = harvest.dataset(item, item["title"], dev_source())
    assert package["owner_org"] == "bbsk" and package["license_id"] == "cc-by"
    extras = {e["key"]: e["value"] for e in package["extras"]}
    assert extras["publisher_name"] == "Banskobystrický samosprávny kraj"
    assert extras["publisher_uri"].endswith("/37828100")
    assert extras["spatial_uri"] == "http://data.europa.eu/nuts/code/SK032"
    assert extras["license_url"] == "https://creativecommons.org/licenses/by/4.0/"
    assert extras["source_id"] == CULTURE and extras["modified"].endswith("Z")
    # EP-118: no Hub CSV, GeoJSON, Esri REST or Shapefile link, only the platform's (T-4324).
    assert all(r["url"].startswith("https://api.dev.joinedcontext.com/") or r["url"].startswith("wss://api.dev.")
               for r in package["resources"])
    assert not {"Esri REST", "SHP", "ZIP"} & {r["format"] for r in package["resources"]}
    assert "<" not in package["notes"] and "style=" not in package["notes"]
    assert len(package["name"]) <= 100 and package["name"].startswith("bbsk-kulturne-institucie")


def test_a_converted_dataset_lists_the_platform_apis_and_its_model_and_an_unconverted_one_none(monkeypatch):
    """T-4279, EP-118: the dataset whose rows became entities lists a distribution per standard,
    each reading only its rows where the standard can filter, then its data model in all seven
    formalisms; a dataset nobody converted lists nothing, and neither does one whose model lacks
    an artefact (T-4324)."""
    monkeypatch.setattr(harvest, "OD_DATASETS", OD_DATASETS)
    monkeypatch.setattr(harvest, "OD_GATEWAY", "https://api.dev.joinedcontext.com")
    monkeypatch.setattr(harvest, "od_models", lambda endpoint: model_index(["PointOfInterest"]))
    item = next(i for i in items() if i["id"] == CULTURE)
    package = harvest.dataset(item, item["title"], dev_source())
    apis = [r for r in package["resources"] if r.get("resource_type") == "api"]
    model = [r for r in package["resources"] if r not in apis]
    entry = OD_DATASETS[package["name"]]
    assert entry["location"], "the culture layer has points, so OGC API Features is listed"
    root = f"https://api.dev.joinedcontext.com/api/endpoint/{entry['endpoint']}/"
    urls = [r["url"] for r in apis]
    assert [u.split("?")[0].removeprefix(root) for u in urls[:-1]] == [
        "ngsi-ld/v1/entities", "file.csv",
        "ogc/features/collections/PointOfInterest/items", "odata/v4/PointOfInterest", "mcp"]
    assert apis[-1]["url"] == "wss://api.dev.joinedcontext.com/api/endpoint/" + entry["endpoint"] + "/ws"
    dataset = quote(entry["dataset"], safe="")
    mine = "?type=PointOfInterest&q=isPartOf%3D%3D%22" + dataset + "%22"
    assert urls[0].endswith(mine) and urls[1].endswith(mine)
    # OGC API Features and OData keep the same rows in their own syntax (T-4289).
    assert urls[2].endswith("/items?isPartOf=" + dataset)
    assert urls[3].endswith("/PointOfInterest?%24filter=isPartOf%20eq%20%27" + dataset + "%27")
    assert all(r["description"].startswith("These rows") for r in apis[:4])
    # The labels jcctl gives an Endpoint's own dataset, so the format facet counts both (T-4324).
    assert [r["format"] for r in apis] == [
        "NGSI-LD", "CSV", "OGCFEAT", "OData", "MCP", "WebSocket"]
    assert all("conforms_to" in r for r in apis if r["format"] in ("NGSI-LD", "OGCFEAT", "OData", "MCP"))
    # T-4539: what a plain GET of the NGSI-LD address answers, not ld+json, which only an Accept asks for.
    assert next(r for r in apis if r["format"] == "NGSI-LD")["mimetype"] == "application/json"
    assert all(r["description"].startswith("Every PointOfInterest") for r in apis if "?" not in r["url"])
    assert [r["format"] for r in model] == [
        "JSON Schema", "JSON-LD", "LinkML", "Markdown", "OWL", "RDF", "SHACL"]
    assert model[0]["url"] == root + "schema/v1/model.schema.json"
    assert model[0]["hash"] == "sha-model.schema.json" and model[0]["hash_algorithm"] == "sha256"

    # T-4291: a dataset an older seed reads whole lists the same doors over its whole type.
    monkeypatch.setitem(OD_DATASETS, package["name"], dict(entry, filtered=False))
    whole = harvest.od_standards(package["name"])
    assert [r["url"] for r in whole if r.get("resource_type") == "api"] == [u.split("?")[0] for u in urls]
    assert all(r["description"].startswith("Every PointOfInterest") for r in whole if r.get("resource_type") == "api")
    # T-4540: a dataset whose type holds its rows alone is kept by the type, still "these rows".
    monkeypatch.setitem(OD_DATASETS, package["name"], dict(entry, sole=True))
    sole = [r for r in harvest.od_standards(package["name"]) if r.get("resource_type") == "api"]
    assert [r["url"] for r in sole] == [u.split("?")[0] for u in urls]
    assert all(r["description"].startswith("These rows") for r in sole)
    # T-4288: a dataset of measured numbers also lists SensorThings, its Datastreams over the same rows.
    monkeypatch.setitem(OD_DATASETS, package["name"], dict(entry, series=True))
    sta = next(r for r in harvest.od_standards(package["name"]) if r["format"] == "STA")
    assert sta["url"] == (root + "sta/v1.1/Datastreams?%24filter=Thing%2Fproperties%2Ftype%20eq%20%27"
                          + quote(entry["type"]) + "%27%20and%20Thing%2Fproperties%2FisPartOf%20eq%20%27" + dataset + "%27")
    assert sta["conforms_to"] == "http://www.opengis.net/spec/iot_sensing/1.1" and sta["description"].startswith("These rows")
    monkeypatch.setitem(OD_DATASETS, package["name"], entry)
    assert "STA" not in [r["format"] for r in harvest.od_standards(package["name"])], "no series, no SensorThings"

    other = next(i for i in items() if i["id"] == APS)
    assert harvest.dataset(other, other["title"], dev_source())["resources"] == []
    monkeypatch.setattr(harvest, "od_models", lambda endpoint: model_index(["PointOfInterest"], ARTEFACTS[1:]))
    assert harvest.od_standards(package["name"]) == [], "a model without its @context lists nothing"
    monkeypatch.setattr(harvest, "od_models", lambda endpoint: model_index(["Other"]))
    assert harvest.od_standards(package["name"]) == [], "no model of the type lists nothing"
    monkeypatch.setattr(harvest, "OD_GATEWAY", "")
    assert harvest.od_standards(package["name"]) == [], "no gateway set lists nothing"


def test_a_converted_dataset_names_its_endpoint_in_the_extras_and_an_unconverted_one_none(monkeypatch):
    """T-4765: the `endpoint` extra holds the Endpoint's public URL with its trailing slash, the
    value jcctl's publisher writes (ckan.rs), so make_requests.py and the catalogue count every
    converted dataset; a dataset nobody converted, or a run without a gateway, writes none."""
    monkeypatch.setattr(harvest, "OD_DATASETS", OD_DATASETS)
    monkeypatch.setattr(harvest, "OD_GATEWAY", "https://api.dev.joinedcontext.com")
    monkeypatch.setattr(harvest, "od_models", lambda endpoint: model_index(["PointOfInterest"]))
    item = next(i for i in items() if i["id"] == CULTURE)
    package = harvest.dataset(item, item["title"], dev_source())
    endpoint = OD_DATASETS[package["name"]]["endpoint"]
    assert [e["value"] for e in package["extras"] if e["key"] == "endpoint"] == [
        f"https://api.dev.joinedcontext.com/api/endpoint/{endpoint}/"]
    other = next(i for i in items() if i["id"] == APS)
    assert not [e for e in harvest.dataset(other, other["title"], dev_source())["extras"] if e["key"] == "endpoint"]
    monkeypatch.setattr(harvest, "OD_GATEWAY", "")
    assert harvest.od_extras(package["name"]) == []


def test_the_model_index_is_read_once_per_endpoint_and_a_failed_read_raises(monkeypatch):
    monkeypatch.setattr(harvest, "OD_GATEWAY", "https://api.dev.joinedcontext.com")
    monkeypatch.setattr(harvest, "_OD_MODELS", {})
    asked = []

    class Answer:
        def __init__(self, body):
            self.body = body

        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

        def read(self, *args):
            return self.body

    def urlopen(request, timeout):
        asked.append(request.full_url)
        if "broken" in request.full_url:
            raise harvest.urllib.error.URLError("timed out")
        return Answer(json.dumps({"models": model_index(["T"])}).encode())

    monkeypatch.setattr(harvest._request, "urlopen", urlopen)
    slept = []
    monkeypatch.setattr(harvest._time, "sleep", slept.append)
    assert harvest.od_models("ep")[0]["types"] == ["T"]
    harvest.od_models("ep")
    assert asked == ["https://api.dev.joinedcontext.com/api/endpoint/ep/schema/index.json"]
    with pytest.raises(OSError):
        harvest.od_models("broken")
    assert slept == list(harvest.OD_RETRY_WAITS), "a broken connection is read again after each wait"


def test_the_cronjob_carries_the_mapping_and_the_dev_gateway(rendered):
    container = by_name(rendered("dev"), "CronJob", "ckan-harvest-bbsk")["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
    env = {e["name"]: e.get("value") for e in container["env"]}
    # The generated file names dev's old catalogue host; the CronJob carries the instance's own (T-4679).
    moved = json.loads(json.dumps(OD_DATASETS).replace("https://opendata.dev.joinedcontext.com/", "https://opendata.dev.joinedcontext.com/"))
    assert json.loads(env["OD_DATASETS"]) == moved
    assert env["OD_GATEWAY"] == "https://api.dev.joinedcontext.com"
    assert container["args"][0].startswith((ROOT / "charts/ckan/files/od-standards.py").read_text())


def test_html_in_a_description_becomes_text_and_never_markup():
    text = harvest.plain_text("<p style='x'>Prvý&nbsp;odsek</p><p>Druhý <a href='https://x'>odkaz</a>"
                              "<script>alert(1)</script></p>")
    assert text == "Prvý odsek\n\nDruhý odkazalert(1)"
    assert harvest.plain_text(None) == ""


def test_a_missing_organization_writes_nothing(ckan, converted):
    """F-DEP1-002: the harvest user creates no organization; the api-token Job does."""
    converted(OBCE)
    ckan.orgs.clear()
    with pytest.raises(harvest.SourceError, match="api-token Job creates it"):
        harvest.harvest(harvest.Ckan(ckan.host, "token-1"), dev_source(), items())
    assert ckan.packages == {} and "organization_create" not in ckan.calls


def test_the_harvest_creates_then_leaves_alone_then_withdraws_only_its_own(ckan, converted):
    converted(OBCE, OKRESY, CULTURE, SUPPLIERS)
    client = harvest.Ckan(ckan.host, "token-1")
    source = dev_source()
    ckan.packages["hand"] = {"id": "hand", "name": "rucny", "owner_org": "bbsk", "state": "active",
                             "extras": []}

    counts, kept, _ = harvest.harvest(client, source, items())
    assert counts == {"created": 4, "updated": 0, "unchanged": 0, "withdrawn": 0}
    assert set(ckan.active()) == {None, OBCE, OKRESY, CULTURE, SUPPLIERS}

    counts, _, _ = harvest.harvest(client, source, items())
    assert counts == {"created": 0, "updated": 0, "unchanged": 4, "withdrawn": 0}
    assert "organization_create" not in ckan.calls

    renamed = [dict(i, title="Kultúrne inštitúcie") if i["id"] == CULTURE else i
               for i in items() if i["id"] != SUPPLIERS]
    name_before = ckan.active()[CULTURE]["name"]
    counts, _, _ = harvest.harvest(client, source, renamed)
    assert counts == {"created": 0, "updated": 1, "unchanged": 2, "withdrawn": 1}
    assert ckan.active()[CULTURE]["name"] == name_before, "a retitled dataset changed its URL"
    assert SUPPLIERS not in ckan.active()
    assert ckan.packages["hand"]["state"] == "active", "a dataset the harvest did not make was withdrawn"

    counts, _, _ = harvest.harvest(client, source, items())
    assert counts["updated"] == 2 and SUPPLIERS in ckan.active(), "a returning dataset stayed deleted"


def test_a_dataset_whose_rows_are_not_converted_is_withdrawn_and_named(ckan, converted):
    """EP-118 (T-4324): the catalogue lists NGSI-LD data or nothing. The first run listed four;
    once only the culture layer is mapped, the other three are withdrawn with their reason."""
    converted(OBCE, OKRESY, CULTURE, SUPPLIERS)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    for ident in (OBCE, OKRESY, SUPPLIERS):
        del harvest.OD_DATASETS[name_of(ident)]
    counts, kept, skipped = harvest.harvest(client, dev_source(), items())
    assert counts["withdrawn"] == 3 and [item["id"] for item, _ in kept] == [CULTURE]
    assert set(ckan.active()) == {CULTURE}
    reasons = {ident: reason for ident, _, reason in skipped}
    assert reasons[SUPPLIERS] == "not converted to NGSI-LD, so not listed (EP-118)"
    # Withdrawn is CKAN's state=deleted, never a purge: converted again, the same dataset is back.
    withdrawn = next(p for p in ckan.packages.values() if harvest.extra(p, "source_id") == SUPPLIERS)
    converted(SUPPLIERS)
    counts, _, _ = harvest.harvest(client, dev_source(), items())
    assert counts["updated"] == 1 and ckan.packages[withdrawn["id"]]["state"] == "active"


def test_a_dataset_a_sysadmin_withdrew_is_found_by_its_fixed_id_and_revived(ckan, converted):
    """Datasets the sysadmin token created carry the uuid5 id (before F-DEP1-002); a withdrawn one
    is found by that id, so the editor never creates it twice."""
    converted(CULTURE)
    item, title = next((i, t) for i, t in harvest.select(items(), dev_source())[0] if i["id"] == CULTURE)
    legacy = harvest.dataset(item, title, dev_source(), name_of(CULTURE))
    ckan.packages[legacy["id"]] = dict(legacy, state="deleted")
    client = harvest.Ckan(ckan.host, "token-1")
    counts, _, _ = harvest.harvest(client, dev_source(), items())
    assert counts["created"] == 0 and counts["updated"] == 1
    assert ckan.packages[legacy["id"]]["state"] == "active" and len(ckan.packages) == 1


def test_no_dataset_converted_at_all_withdraws_nothing(ckan, converted):
    """A missing gateway or mapping lists no dataset; that is a broken run, not an empty catalogue."""
    converted(CULTURE)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    harvest.OD_DATASETS.clear()
    with pytest.raises(harvest.SourceError, match="nothing is withdrawn"):
        harvest.harvest(client, dev_source(), items())
    assert set(ckan.active()) == {CULTURE} and "package_delete" not in ckan.calls


def test_a_gateway_that_fails_halfway_withdraws_nothing(ckan, converted, monkeypatch):
    converted(OBCE, OKRESY, CULTURE, SUPPLIERS)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())

    def broken(endpoint):
        raise harvest.urllib.error.URLError("dev.joinedcontext.com: timed out")

    monkeypatch.setattr(harvest, "od_models", broken)
    with pytest.raises(OSError):
        harvest.harvest(client, dev_source(), [i for i in items() if i["id"] != SUPPLIERS])
    assert SUPPLIERS in ckan.active() and "package_delete" not in ckan.calls


def test_a_short_read_of_the_hub_is_an_error_not_an_empty_catalogue(monkeypatch):
    page = json.loads((FIXTURES / "items.json").read_text())
    monkeypatch.setattr(harvest, "fetch_json", lambda url, hosts: dict(page, numberMatched=103))
    with pytest.raises(harvest.SourceError, match="read 8 of 103"):
        harvest.hub_items(dev_source())


def test_a_next_page_off_the_hub_is_refused(monkeypatch):
    page = json.loads((FIXTURES / "items.json").read_text())
    page["links"] = [{"rel": "next", "href": "https://evil.example/items?startindex=9"}]
    monkeypatch.setattr(harvest, "fetch_json", lambda url, hosts: page)
    with pytest.raises(harvest.SourceError, match="outside the hub"):
        harvest.hub_items(dev_source())


def test_a_ckan_refusal_is_raised_with_its_reason_and_never_the_token(ckan):
    client = harvest.Ckan(ckan.host, "token-1")
    with pytest.raises(RuntimeError) as error:
        client.call("package_purge", {"id": "x"})
    assert "token-1" not in str(error.value)


def by_name(docs, kind, name):
    match = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert match, f"{kind}/{name} is not in the render"
    return match[0]


def test_the_cronjob_runs_the_catalogue_image_with_the_token_by_reference(rendered):
    docs = rendered("dev")
    job = by_name(docs, "CronJob", "ckan-harvest-bbsk")
    pod = job["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    assert job["spec"]["concurrencyPolicy"] == "Forbid"
    assert pod["automountServiceAccountToken"] is False
    container = pod["containers"][0]
    catalogue = by_name(docs, "Deployment", "ckan")["spec"]["template"]["spec"]["containers"][0]
    assert container["image"] == catalogue["image"] and "@sha256:" in container["image"]
    env = {e["name"]: e for e in container["env"]}
    assert env["CKAN_API_TOKEN"]["valueFrom"]["secretKeyRef"] == {"name": "ckan-api-token-harvest", "key": "token"}
    config = json.loads(env["HARVEST_CONFIG"]["value"])
    assert (config["source"], config["organization"], config["kind"]) == ("hub-bbsk", "bbsk", "hub")
    assert "def hub_items(source):" in container["args"][0], "the hub source runs another script"
    assert "def main():" in container["args"][0]


def test_the_harvest_reaches_dns_the_catalogue_and_https_only(rendered):
    docs = rendered("dev")
    policy = by_name(docs, "NetworkPolicy", "ckan-harvest")["spec"]
    assert not policy.get("ingress")
    # 443 and 8443 to the ingress pod as well (T-4401); test_ckan.py checks which peers they reach.
    assert sorted(p["port"] for rule in policy["egress"] for p in rule["ports"]) == [53, 53, 443, 443, 5000, 8443]


def test_an_empty_or_closed_hub_answer_withdraws_nothing(ckan, converted):
    """Integrator review 2026-10-15: an outage that lists nothing must not empty `bbsk`."""
    converted(OBCE, OKRESY, CULTURE, SUPPLIERS)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    before = set(ckan.active())
    closed = [dict(i, license="none") for i in items()]
    for answer in ([], closed):
        with pytest.raises(harvest.SourceError, match="nothing is withdrawn"):
            harvest.harvest(client, dev_source(), answer)
        assert set(ckan.active()) == before
    assert "package_delete" not in ckan.calls


def test_a_service_off_the_allowed_hosts_is_skipped_and_never_fetched():
    moved = [dict(i, url="https://evil.example/arcgis/rest/services/x/FeatureServer")
             if i["id"] == CULTURE else i for i in items()]
    kept, skipped = harvest.select(moved, dev_source())
    assert CULTURE not in [item["id"] for item, _ in kept]
    assert (CULTURE, "its service is not on an allowed host") in [(i, r) for i, _, r in skipped]


def test_a_fetch_is_refused_off_the_allowed_hosts_and_so_is_a_redirect_there():
    with pytest.raises(harvest.SourceError, match="not an allowed host"):
        harvest.fetch_json("https://evil.example/x.json", {"opendata.bbsk.sk"})
    with pytest.raises(harvest.SourceError, match="not an allowed host"):
        harvest.fetch_json("http://opendata.bbsk.sk/x.json", {"opendata.bbsk.sk"})
    handler = harvest._CheckedRedirects({"opendata.bbsk.sk"})
    request = harvest.urllib.request.Request("https://opendata.bbsk.sk/a")
    with pytest.raises(harvest.SourceError, match="redirect to 169.254.169.254"):
        handler.redirect_request(request, None, 302, "Found", {}, "http://169.254.169.254/latest/")
    assert handler.redirect_request(request, None, 302, "Found", {}, "https://opendata.bbsk.sk/b") is not None


# T-4385: a connection reset on the gateway during a mesh restart (`[Errno 104]`) failed the
# night's bbsk and banskabystrica harvests whole. A reset, a timeout and a 5xx are read again
# after a wait; a 4xx is the answer; a gateway that stays down still raises, so nothing is withdrawn.
class _Answer:
    def __init__(self, body):
        self.body = body

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False

    def read(self, *args):
        return self.body


def _gateway(monkeypatch, failures):
    asked, waited = [], []

    def urlopen(request, timeout):
        asked.append(request.full_url)
        if failures:
            failure = failures.pop(0)
            if isinstance(failure, int):
                raise urllib.error.HTTPError(request.full_url, failure, "x", {}, None)
            raise failure
        return _Answer(json.dumps({"models": [{"version": 1, "types": ["Bridge"]}]}).encode())

    monkeypatch.setattr(harvest._request, "urlopen", urlopen)
    monkeypatch.setattr(harvest._time, "sleep", waited.append)
    monkeypatch.setattr(harvest, "_OD_MODELS", {})
    monkeypatch.setattr(harvest, "OD_GATEWAY", "https://dev.example")
    return asked, waited


@pytest.mark.parametrize("failure", [
    ConnectionResetError(104, "Connection reset by peer"),
    urllib.error.URLError(ConnectionResetError(104, "Connection reset by peer")),
    TimeoutError("timed out"),
    503,
])
def test_a_broken_read_of_the_models_is_read_again(monkeypatch, failure):
    asked, waited = _gateway(monkeypatch, [failure, failure])
    assert harvest.od_models("slug")[0]["types"] == ["Bridge"]
    assert len(asked) == 3 and waited == list(harvest.OD_RETRY_WAITS[:2])


def test_a_refusal_is_not_asked_again(monkeypatch):
    asked, waited = _gateway(monkeypatch, [404])
    with pytest.raises(urllib.error.HTTPError):
        harvest.od_models("slug")
    assert len(asked) == 1 and waited == []


def test_a_gateway_that_stays_down_still_fails_the_run(monkeypatch):
    attempts = len(harvest.OD_RETRY_WAITS) + 1
    asked, waited = _gateway(monkeypatch, [ConnectionResetError(104, "reset")] * attempts)
    with pytest.raises(ConnectionResetError):
        harvest.od_models("slug")
    assert len(asked) == attempts and waited == list(harvest.OD_RETRY_WAITS)


def test_a_failed_harvest_run_is_run_again():
    chart = (ROOT / "charts/ckan/templates/harvest.yaml").read_text()
    assert "backoffLimit: 3" in chart


def datastore(ckan, ident):
    package = ckan.active()[ident]
    return [r for r in package.get("resources", []) if r.get("url_type") == "datastore"]


def sheet_url(ident):
    name = name_of(ident)
    dataset = f"https://opendata.dev.joinedcontext.com/dataset/{name}"
    return ("https://api.dev.joinedcontext.com/api/endpoint/ep/file.csv?type=PointOfInterest&q="
            + quote(f'isPartOf=="{dataset}"', safe=""))


def test_every_listed_dataset_holds_its_endpoint_rows_as_an_active_sheet(ckan, converted, monkeypatch):
    """T-4742: a converted dataset without a sheet showed datastore_active=false on dev and prod.
    The sheet is the Endpoint's own file.csv of the dataset's rows; unchanged rows write nothing,
    changed rows replace the one table in place."""
    converted(OBCE, CULTURE)
    read = []
    monkeypatch.setattr(harvest, "od_read", lambda url, *args: read.append(url) or endpoint_file(url, *args))
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    assert sheet_url(OBCE) in read and sheet_url(CULTURE) in read
    for ident in (OBCE, CULTURE):
        [table] = datastore(ckan, ident)
        assert table["name"] == "PointOfInterest" and table["format"] == "CSV"
        assert ckan.tables[table["id"]] == {
            "fields": [{"id": "id", "type": "text"}, {"id": "name", "type": "text"}],
            "records": [{"id": "urn:ngsi-ld:PointOfInterest:1", "name": "Hrad"}]}

    first = len(ckan.calls)
    counts, _, _ = harvest.harvest(client, dev_source(), items())
    assert counts["unchanged"] == 2
    assert [c for c in ckan.calls[first:] if not c.endswith(("_show", "_search", "_list"))] == []

    monkeypatch.setitem(SHEETS, sheet_url(CULTURE), SHEET + b"urn:ngsi-ld:PointOfInterest:2,Kastiel\r\n")
    counts, _, _ = harvest.harvest(client, dev_source(), items())
    assert counts == {"created": 0, "updated": 1, "unchanged": 1, "withdrawn": 0}
    [table] = datastore(ckan, CULTURE)
    assert [r["name"] for r in ckan.tables[table["id"]]["records"]] == ["Hrad", "Kastiel"]
    assert len(ckan.tables) == 2, "a changed sheet added a second table"


def test_a_lost_sheet_is_written_again_although_the_dataset_is_unchanged(ckan, converted):
    converted(CULTURE)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    package = ckan.active()[CULTURE]
    package["resources"] = [r for r in package["resources"] if r.get("url_type") != "datastore"]
    counts, _, _ = harvest.harvest(client, dev_source(), items())
    assert counts["updated"] == 1 and len(datastore(ckan, CULTURE)) == 1


def test_an_empty_dataset_still_gets_an_active_sheet_of_its_columns(ckan, converted, monkeypatch):
    converted(CULTURE)
    monkeypatch.setitem(SHEETS, sheet_url(CULTURE), b"id,name\r\n")
    harvest.harvest(harvest.Ckan(ckan.host, "token-1"), dev_source(), items())
    [table] = datastore(ckan, CULTURE)
    assert ckan.tables[table["id"]] == {"fields": [{"id": "id", "type": "text"}, {"id": "name", "type": "text"}],
                                        "records": []}
    monkeypatch.setitem(SHEETS, sheet_url(CULTURE), b"")
    assert harvest.od_sheet(name_of(CULTURE))["rows"] == []
    assert harvest.od_fields([], []) == [{"id": "empty", "type": "text"}], "CKAN refuses a table without fields"


def test_a_dataset_the_publish_gate_hides_gets_no_sheet(ckan, converted, monkeypatch):
    """T-4318: a hidden dataset is still harvested, but nothing reads its rows into a sheet."""
    converted(OBCE, CULTURE)
    monkeypatch.setattr(harvest, "OD_HIDDEN", {name_of(OBCE)})
    harvest.harvest(harvest.Ckan(ckan.host, "token-1"), dev_source(), items())
    assert datastore(ckan, OBCE) == [] and len(datastore(ckan, CULTURE)) == 1
    assert OBCE in ckan.active()


def test_a_file_the_endpoint_refuses_leaves_the_dataset_without_a_sheet_and_says_so(ckan, converted, monkeypatch, capsys):
    converted(CULTURE)

    def too_large(url, *args):
        raise harvest.urllib.error.HTTPError(url, 413, "Content Too Large", {}, None)

    monkeypatch.setattr(harvest, "od_read", too_large)
    counts, _, _ = harvest.harvest(harvest.Ckan(ckan.host, "token-1"), dev_source(), items())
    assert counts["created"] == 1 and datastore(ckan, CULTURE) == []
    assert f"{name_of(CULTURE)}: no sheet" in capsys.readouterr().err



def test_a_sheet_read_the_edge_refuses_for_load_keeps_the_live_sheet(ckan, converted, monkeypatch):
    """A 429 (T-4717) or an outage is not an empty sheet: the run fails and the table stays."""
    converted(CULTURE)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    tables = dict(ckan.tables)
    slept = []
    monkeypatch.setattr(harvest._time, "sleep", slept.append)

    def busy(request, timeout):
        raise harvest.urllib.error.HTTPError(request.full_url, 429, "Too Many Requests", {}, None)

    monkeypatch.setattr(harvest, "od_read", READ)
    monkeypatch.setattr(harvest._request, "urlopen", busy)
    with pytest.raises(harvest.urllib.error.HTTPError):
        harvest.harvest(client, dev_source(), items())
    assert slept == list(harvest.OD_RETRY_WAITS), "a 429 is asked again after each wait"
    assert ckan.tables == tables and len(datastore(ckan, CULTURE)) == 1

    def outage(request, timeout):
        raise harvest.urllib.error.URLError("timed out")

    monkeypatch.setattr(harvest._request, "urlopen", outage)
    with pytest.raises(OSError):
        harvest.harvest(client, dev_source(), items())
    assert ckan.tables == tables


def test_the_sheet_is_read_anonymously_so_it_holds_only_public_attributes(monkeypatch):
    """The sheet must carry nothing the Endpoint's Policy hides from the public: the read sends
    no credential, whatever the harvest holds."""
    monkeypatch.setenv("CKAN_API_TOKEN", "token-1")
    sent = []

    class Answer:
        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

        def read(self, *args):
            return b"id\r\n"

    def urlopen(request, timeout):
        sent.append(dict(request.header_items()))
        return Answer()

    monkeypatch.setattr(harvest._request, "urlopen", urlopen)
    assert harvest.od_read("https://api.dev.joinedcontext.com/api/endpoint/ep/file.csv", "text/csv") == b"id\r\n"
    assert sent == [{"Accept": "text/csv"}]


def test_the_cronjob_names_the_datasets_the_gate_hides(rendered):
    container = by_name(rendered("dev"), "CronJob", "ckan-harvest-bbsk")["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
    env = {e["name"]: e.get("value") for e in container["env"]}
    values = (ROOT / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    hidden = yaml.safe_load(values.replace('{{"{{"}}', "{{").replace('{{"}}"}}', "}}"))["hidden"]
    assert json.loads(env["OD_HIDDEN"]) == sorted(hidden) and "bb-adresy" in hidden


def test_each_sheet_has_one_table_view_and_a_located_one_its_map(ckan, converted):
    """T-4742 views: a table view on every sheet, created once and again only when it is gone;
    a located dataset's sheet is named by its type beside its OGC API Features distribution,
    which is what the theme's dataset map reads (T-4639); a register without location has none."""
    converted(CULTURE)
    converted(OBCE, location=False)
    client = harvest.Ckan(ckan.host, "token-1")
    harvest.harvest(client, dev_source(), items())
    for ident in (CULTURE, OBCE):
        [table] = datastore(ckan, ident)
        assert [v["view_type"] for v in ckan.views if v["resource_id"] == table["id"]] == ["datatables_view"]
    harvest.harvest(client, dev_source(), items())
    assert len(ckan.views) == 2, "a second run added a view"
    ckan.views.clear()
    harvest.harvest(client, dev_source(), items())
    assert len(ckan.views) == 2, "a deleted table view was not written again"

    def ogc(ident):
        return [r["url"] for r in ckan.active()[ident]["resources"] if r.get("format") == "OGCFEAT"]

    [items_url] = ogc(CULTURE)
    assert urllib.parse.urlsplit(items_url).path.endswith(f"/collections/{datastore(ckan, CULTURE)[0]['name']}/items")
    assert ogc(OBCE) == [], "a register without location gets no map"


BBSK_SCHOOLS = ["bbsk-materske-skoly-v-banskobystrickom-kraji-3f7d07", "bbsk-zakladne-skoly-v-banskobystrickom-kraji-13a8b6",
                "bbsk-stredne-skoly-v-banskobystrickom-kraji-581554", "bbsk-vysoke-skoly-v-banskobystrickom-kraji-b0be47"]


@pytest.mark.parametrize("name", BBSK_SCHOOLS)
def test_each_bbsk_school_dataset_is_published_with_its_endpoint(name):
    """T-4766: the four school registers answer bbsk-demo's school questions (T-4740), so none is
    hidden and each names the BBSK open-data Endpoint and its type."""
    values = (ROOT / "components/ckan/values/catalogue/development-values.yaml.gotmpl").read_text()
    hidden = yaml.safe_load(values.replace('{{"{{"}}', "{{").replace('{{"}}"}}', "}}"))["hidden"]
    assert name not in hidden
    entry = OD_DATASETS[name]
    assert (entry["endpoint"], entry["type"]) == ("6b4yqhzqieutqcoeew5rnxrx5udqeert", "Organization")
