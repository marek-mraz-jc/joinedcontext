"""OpenStreetMap points of Banská Bystrica (T-4514): the osm mapping run in Bento against a saved
Overpass answer (2026-10-18, the district's 59 pharmacies, hospitals, clinics, defibrillators
and charging stations) and a stand-in gateway, so the entities, the attribution and the removal
of what the map no longer lists are checked without the network."""
import http.server
import json
import re
import shutil
import subprocess
import threading
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-banskabystrica-osm")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/banskabystrica"
MAPPING = CITY / "pipeline-osm-bento.yaml"
ANSWER = Path(__file__).resolve().parent / "fixtures/banskabystrica/overpass-osm.json"
SPACE = "banskabystrica-verejne"
SLUG = "rpfqszwlbp5bonvg7kckhdsne5qedjor"
PRIMARY = "https://overpass-api.de"
MIRROR = "https://overpass.kumi.systems"
ATTRIBUTION = "© OpenStreetMap contributors, ODbL 1.0"


def urn(kind: str, local: str) -> str:
    return f"urn:ngsi-ld:{kind}:banskabystrica.sk:{SPACE}:{local}"


class Stand(http.server.BaseHTTPRequestHandler):
    """Overpass, its mirror, the token endpoint and the gateway, each answering as told."""

    primary: tuple[int, bytes] = (200, b"")
    mirror: tuple[int, bytes] = (200, b"")
    held: list[dict] = []
    seen: list[tuple[str, bytes]] = []

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        Stand.seen.append((self.path, body))
        if self.path == "/primary/api/interpreter":
            status, answer = Stand.primary
        elif self.path == "/mirror/api/interpreter":
            status, answer = Stand.mirror
        elif self.path == "/token":
            status, answer = 200, b'{"access_token":"stand-in","token_type":"Bearer","expires_in":300}'
        elif self.path.startswith(f"/api/endpoint/{SLUG}/ngsi-ld/v1/entityOperations/query"):
            status, answer = 200, json.dumps(Stand.held).encode()
        elif self.path == f"/api/endpoint/{SLUG}/ngsi-ld/v1/entityOperations/delete":
            status, answer = 204, b""
        else:
            status, answer = 404, b""
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, *_):
        pass


def saved() -> bytes:
    return ANSWER.read_bytes()


def osm(primary=None, mirror=(503, b""), held=()) -> subprocess.CompletedProcess:
    Stand.primary, Stand.mirror = primary or (200, saved()), mirror
    Stand.held, Stand.seen = list(held), []
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stand)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    here = f"http://127.0.0.1:{server.server_port}"
    try:
        text = MAPPING.read_text().replace(PRIMARY, here + "/primary").replace(MIRROR, here + "/mirror")
        config = {
            "input": {"generate": {"mapping": 'root = ""', "count": 1, "interval": ""}},
            # The runner writes no message Bento flagged errored: the last step turns the flag
            # into the output, so written() refuses it the way the run does.
            "pipeline": {"processors": [*yaml.safe_load(text)["pipeline"]["processors"],
                                        {"mapping": 'root = if errored() { {"errored": error()} } else { this }'}]},
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "all-bytes"}},
            "logger": {"level": "error"},
        }
        env = {"JC_SPACE": SPACE, "JC_GATEWAY_URL": here, "JC_TOKEN_URL": here + "/token",
               "JC_CLIENT_ID": "pipelines", "JC_CLIENT_SECRET": "stand-in"}
        return subprocess.run(
            ["docker", "run", "--rm", "--network", "host",
             *[a for k, v in env.items() for a in ("-e", f"{k}={v}")],
             "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def written(result: subprocess.CompletedProcess) -> list[dict]:
    """The entities a run hands to the writer; a message Bento flagged errored is never written."""
    assert result.returncode == 0, result.stderr.decode()
    out = json.loads(result.stdout)
    assert isinstance(out, list), out
    return out


def calls(suffix: str) -> list[bytes]:
    return [body for path, body in Stand.seen if path.split("?")[0].endswith(suffix)]


@pytest.fixture(scope="module")
def entities():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {e["id"]: e for e in written(osm())}


@requires_docker
def test_every_mapped_object_is_one_entity_the_models_take_with_its_attribution(entities):
    elements = json.loads(saved())["elements"]
    assert len(entities) == len(elements) == 59
    kinds = [e["type"] for e in entities.values()]
    assert kinds.count("EVChargingStation") == 22 and kinds.count("PointOfInterest") == 37
    for entity in entities.values():
        assert not open_data.schema_errors(CITY / "banskabystrica-verejne.v1.schema.json", entity), entity["id"]
        assert entity["dataProvider"]["value"] == ATTRIBUTION
        assert entity["source"]["value"].startswith("https://www.openstreetmap.org/")
        assert entity["location"]["value"]["type"] == "Point"
        if entity["type"] == "PointOfInterest":
            assert open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity) == [], entity["id"]
        elif {"socketType", "capacity", "allowedVehicleType"} <= set(entity):
            # SDM requires all three; a station the map tags less fully keeps what it has.
            assert open_data.sdm_errors("dataModel.Transportation/EVChargingStation", entity) == [], entity["id"]


@requires_docker
def test_a_hospital_mapped_as_an_area_sits_at_its_centre_with_its_address(entities):
    hospital = entities[urn("PointOfInterest", "osm-way-24863329")]
    assert hospital["name"]["languageMap"] == {"sk": "Fakultná nemocnica s poliklinikou F.D. Roosevelta"}
    assert hospital["category"]["value"] == ["hospital"]
    assert hospital["location"]["value"]["coordinates"] == [19.1167046, 48.7435095]
    assert hospital["address"]["value"] == {"streetAddress": "Námestie Ludvíka Svobodu 1", "postalCode": "97517",
                                            "addressLocality": "Banská Bystrica", "addressCountry": "SK"}
    assert hospital["source"]["value"] == "https://www.openstreetmap.org/way/24863329"
    # The hospital's phone and web links stay in OSM; the entity carries the model's slots only.
    assert "phone" not in json.dumps(hospital)


@requires_docker
def test_opening_hours_are_copied_as_osm_writes_them(entities):
    pharmacy = entities[urn("PointOfInterest", "osm-node-562621210")]
    assert pharmacy["openingHours"]["value"] == "Mo-Fr 08:00-16:30"
    assert pharmacy["category"]["value"] == ["pharmacy"]


@requires_docker
def test_a_charging_station_takes_its_sockets_capacity_and_vehicles_from_its_tags(entities):
    full = entities[urn("EVChargingStation", "osm-node-5834295073")]
    assert full["socketType"]["value"] == ["CCS/SAE", "CHAdeMO", "Type2"]
    assert full["capacity"]["value"] == 2
    assert full["allowedVehicleType"]["value"] == ["car"]
    assert full["operator"]["value"] == "GreenWay"
    # No capacity tag: the socket counts stand in.
    counted = entities[urn("EVChargingStation", "osm-node-4275500389")]
    assert counted["socketType"]["value"] == ["CHAdeMO"] and counted["capacity"]["value"] == 1
    # A bicycle station: motorcar=no keeps cars out.
    bikes = entities[urn("EVChargingStation", "osm-node-4983596192")]
    assert bikes["allowedVehicleType"]["value"] == ["bicycle"] and "socketType" not in bikes
    # A station with no tag beyond its amenity keeps a name and invents nothing else.
    bare = entities[urn("EVChargingStation", "osm-node-4275500489")]
    assert bare["name"]["languageMap"] == {"sk": "Nabíjacia stanica", "en": "Charging station"}
    assert set(bare) == {"id", "type", "name", "location", "dataProvider", "source"}


@requires_docker
def test_an_unnamed_defibrillator_is_named_for_what_it_is(entities):
    aeds = [e for e in entities.values() if e.get("category", {}).get("value") == ["defibrillator"]]
    assert len(aeds) == 3
    assert all(e["name"]["languageMap"] == {"sk": "Defibrilátor (AED)", "en": "Defibrillator (AED)"} for e in aeds)


@requires_docker
def test_an_object_gone_from_the_map_is_deleted_and_the_rest_kept():
    gone = urn("PointOfInterest", "osm-node-1")
    kept = urn("PointOfInterest", "osm-node-562621210")
    result = osm(held=[{"id": gone, "type": "PointOfInterest"}, {"id": kept, "type": "PointOfInterest"}])
    assert len(written(result)) == 59
    (query,) = calls("/entityOperations/query")
    patterns = {e["type"]: e["idPattern"] for e in json.loads(query)["entities"]}
    assert patterns == {t: rf"^urn:ngsi-ld:{t}:banskabystrica\.sk:{SPACE}:osm-" for t in ("PointOfInterest", "EVChargingStation")}
    assert [json.loads(b) for b in calls("/entityOperations/delete")] == [[gone]]


@requires_docker
def test_nothing_gone_deletes_nothing():
    result = osm(held=[{"id": urn("PointOfInterest", "osm-node-562621210"), "type": "PointOfInterest"}])
    assert len(written(result)) == 59
    assert calls("/entityOperations/query") and not calls("/entityOperations/delete")


@requires_docker
def test_the_first_run_into_an_empty_space_writes_every_object():
    # T-5320: prod's run failed 59 times with "child processors resulted in zero messages":
    # an empty removal step flagged every entity errored and the space stayed empty.
    result = osm(held=[])
    entities = written(result)
    assert len(entities) == 59
    assert [e["type"] for e in entities].count("EVChargingStation") == 22
    assert calls("/entityOperations/query") and not calls("/entityOperations/delete")


@requires_docker
def test_a_full_page_of_held_entities_is_not_judged_from_one_page():
    held = [{"id": urn("PointOfInterest", f"osm-node-{n}"), "type": "PointOfInterest"} for n in range(1000)]
    result = osm(held=held)
    assert len(written(result)) == 59
    assert not calls("/entityOperations/delete")


@requires_docker
def test_the_mirror_answers_when_the_main_server_fails():
    result = osm(primary=(504, b"<html>Gateway Timeout</html>"), mirror=(200, saved()))
    assert len(written(result)) == 59
    assert calls("/primary/api/interpreter") and calls("/mirror/api/interpreter")


@requires_docker
def test_the_mirror_is_not_asked_when_the_main_server_answers():
    assert len(written(osm())) == 59
    assert calls("/primary/api/interpreter") and not calls("/mirror/api/interpreter")


@pytest.mark.parametrize("answer, said", [
    ({"elements": json.loads(ANSWER.read_text())["elements"][:5]}, b"fewer than 20"),
    ({**json.loads(ANSWER.read_text()), "remark": "runtime error: Query timed out"}, b"cut its answer short"),
])
@requires_docker
def test_a_short_answer_fails_the_run_and_deletes_nothing(answer, said):
    held = [{"id": urn("PointOfInterest", "osm-node-1"), "type": "PointOfInterest"}]
    result = osm(primary=(200, json.dumps(answer).encode()), held=held)
    assert result.returncode != 0 or result.stdout.strip() in (b"", b"[]", b"{}") or b'"errored"' in result.stdout
    assert said in result.stderr + result.stdout
    assert not calls("/entityOperations/query") and not calls("/entityOperations/delete")


def test_the_query_asks_overpass_inside_the_district_boundary_only():
    processors = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    assert "area(id:3600388240)" in processors[0]["mapping"]
    urls = [q["http"]["url"] for p in processors if "branch" in p for q in p["branch"]["processors"] if "http" in q]
    assert urls[:2] == [PRIMARY + "/api/interpreter", MIRROR + "/api/interpreter"]
    for p in processors:
        for q in p.get("branch", {}).get("processors", []):
            assert "skip_cert_verify" not in json.dumps(q)


def test_the_datasets_are_odbl_with_the_osm_credit_and_the_pipeline_alone_may_delete():
    for part, kind in (("zdravie", "PointOfInterest"), ("nabijanie", "EVChargingStation")):
        spec = yaml.safe_load((CITY / f"endpoint-{part}.yaml").read_text())["spec"]
        assert spec["contextSpaceRef"] == SPACE and spec["audience"] == "public"
        # A dev-empty Endpoint leaves the catalogue until it holds data (T-5767); the rest publish under ODbL.
        if spec["catalog"].get("harvest") is False:
            assert "publish" not in spec
        else:
            assert spec["publish"]["ckan"]["license"] == "odc-odbl"
        assert spec["catalog"]["license"] == "ODC_ODBL"
        assert "© prispievatelia OpenStreetMap" in spec["catalog"]["attribution"]["sk"]
        assert "© OpenStreetMap contributors" in spec["catalog"]["attribution"]["en"]
        policy = yaml.safe_load((CITY / f"policy-{part}-read.yaml").read_text())["spec"]
        assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == [kind]
    delete = yaml.safe_load((CITY / "policy-verejne-osm-delete.yaml").read_text())["spec"]
    assert delete["operations"] == ["deleteBatch"]
    assert delete["assignee"] == {"kind": "serviceAccount", "id": "pipelines"}
    assert [e["type"] for rule in delete["information"] for e in rule["entities"]] == ["PointOfInterest", "EVChargingStation"]
    pipeline = yaml.safe_load((CITY / "pipeline-osm.yaml").read_text())["spec"]
    assert pipeline["period"] == "168h"
    assert pipeline["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:banskabystrica.sk:{SPACE}:{SPACE}"


def grants(account: str) -> dict[str, set[str]]:
    """The operations `account` holds per type in the space: the Policies naming it, and for the
    pipeline's own account (PL-19) the one jc-core pipeline_identity::derive grants, its output's
    type with upsertBatch, createBatch and queryBatch (deleteBatch only with spec.expiry)."""
    held: dict[str, set[str]] = {}
    for path in CITY.glob("policy-*.yaml"):
        spec = yaml.safe_load(path.read_text())["spec"]
        if spec["assignee"] != {"kind": "serviceAccount", "id": account} or spec["contextSpaceRef"]["name"] != SPACE:
            continue
        for info in spec["information"]:
            for selector in info["entities"]:
                held.setdefault(selector["type"], set()).update(spec["operations"])
    pipeline = yaml.safe_load((CITY / "pipeline-osm.yaml").read_text())
    if account == "pl-" + pipeline["metadata"]["name"]:
        derived = {"upsertBatch", "createBatch", "queryBatch"} | ({"deleteBatch"} if "expiry" in pipeline["spec"] else set())
        held.setdefault(pipeline["spec"]["output"]["type"], set()).update(derived)
    return held


@pytest.mark.parametrize("account", ["pipelines", "pl-osm"])
def test_the_account_the_runner_writes_as_may_write_and_remove_every_type_the_mapping_writes(account):
    # T-5320: on dev the runner writes as the pipeline's own account pl-osm, whose derived Policy
    # grants only the output's type, so all 22 charging stations came back "no policy grants this
    # operation". Each runner mode's account needs both types: the writes and the removal.
    written_types = {"PointOfInterest", "EVChargingStation"}
    assert written_types <= set(re.findall(r'"(PointOfInterest|EVChargingStation)"', MAPPING.read_text()))
    held = grants(account)
    for entity_type in sorted(written_types):
        assert {"upsertBatch", "createBatch", "queryBatch", "deleteBatch"} <= held.get(entity_type, set()), (account, entity_type, held)
