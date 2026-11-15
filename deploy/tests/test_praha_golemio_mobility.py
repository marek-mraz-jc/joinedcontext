"""The Mr.Parkit garages with their tariffs and the car-sharing cars of Prague from Golemio (T-5680,
PL-03).

Bento runs each committed mapping over answers recorded from api.golemio.cz on 2026-10-25 at 15:30Z
(`fixtures/praha/golemio-*.json`): five garages, one of them in Hradec Králové, with the five
tariffs they name, and four shared cars, one of each company and a second Bolt. The tariffs the
garage mapping reads itself come from a stand-in on this machine, which also checks that the
request carries the key the runner holds. The edge cases are those answers edited in the one place
they name. Each entity is checked against the space's generated JSON Schema, which the gateway
enforces.
"""

import copy
import http.server
import json
import shutil
import subprocess
import threading
from pathlib import Path
from typing import ClassVar

import open_data
import pytest
import yaml
from open_data import BENTO, RESOURCES, requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-golemio-mobility")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
SCHEMA = PRAHA / "praha.v1.schema.json"
FIXTURES = Path(__file__).resolve().parent / "fixtures/praha"
GARAGES = json.loads((FIXTURES / "golemio-mr-parkit-garages.json").read_text())
TARIFFS = json.loads((FIXTURES / "golemio-mr-parkit-tariffs.json").read_text())
CARS = json.loads((FIXTURES / "golemio-shared-cars.json").read_text())
HOST = "https://api.golemio.cz"
TOKEN = "stand-in-key"
NOW = "2026-10-25T15:30:00Z"
SPACE = "praha-mesto"


class StandIn(http.server.BaseHTTPRequestHandler):
    """Answers the tariff path with `body` when the request carries the key, else 403."""

    status: ClassVar[int] = 200
    body: ClassVar[bytes] = b"[]"
    seen: ClassVar[list[tuple[str, str]]] = []

    def do_GET(self):
        StandIn.seen.append((self.path, self.headers.get("X-Access-Token", "")))
        keyed = self.headers.get("X-Access-Token") == TOKEN
        ok = self.path == "/v3/parking-tariffs?primarySource=mr_parkit&limit=10000" and keyed
        self.send_response(StandIn.status if ok else 403)
        self.end_headers()
        self.wfile.write(StandIn.body if ok else b'{"error_status":403}')

    def log_message(self, *_):
        pass


def bento(mapping: str, document: dict, base: str = HOST) -> tuple[list[dict], list[str]]:
    """The mapping over one answer, with now() pinned and Golemio's host replaced by `base`: the
    entities written and the error of a message the runner refuses."""
    text = (PRAHA / mapping).read_text().replace(HOST, base)
    text = text.replace("now()", f'"{NOW}".ts_parse("2006-01-02T15:04:05Z07:00")')
    processors = yaml.safe_load(text)["pipeline"]["processors"]
    processors.append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": {"processors": processors},
        "rate_limit_resources": yaml.safe_load(RESOURCES.read_text())["rate_limit_resources"],
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "none"},
    }
    done = subprocess.run(
        ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=praha.eu", "-e", f"JC_SPACE={SPACE}",
         "-e", f"DS_GOLEMIO_MR_PARKIT_TOKEN={TOKEN}", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=json.dumps(document).encode(), capture_output=True, timeout=180, check=False,
    )
    assert done.returncode == 0, done.stderr.decode()
    entities, errors = [], []
    for line in done.stdout.decode().splitlines():
        if line.strip():
            message = json.loads(line)
            if isinstance(message, dict) and "__error" in message:
                errors.append(message["__error"])
            else:
                entities.extend(message if isinstance(message, list) else [message])
    return entities, errors


def garages(document: dict = GARAGES, tariffs: object = TARIFFS, status: int = 200) -> tuple[list[dict], list[str]]:
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), StandIn)
    StandIn.status, StandIn.seen = status, []
    StandIn.body = tariffs if isinstance(tariffs, bytes) else json.dumps(tariffs).encode()
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        return bento("praha-pipeline-golemio-garages-bento.yaml", document, f"http://localhost:{server.server_port}")
    finally:
        server.shutdown()


def by_key(entities: list[dict]) -> dict[str, dict]:
    return {e["id"].rsplit(":", 1)[1]: e for e in entities}


def written_ok(result: tuple[list[dict], list[str]]) -> dict[str, dict]:
    entities, errors = result
    assert not errors, errors
    return by_key(entities)


def edited(document: dict, edit) -> dict:
    copied = copy.deepcopy(document)
    edit(copied["features"])
    return copied


@pytest.fixture(scope="module")
def parked():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return written_ok(garages())


@pytest.fixture(scope="module")
def cars():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    entities, errors = bento("praha-pipeline-golemio-shared-cars-bento.yaml", CARS)
    assert not errors, errors
    return by_key(entities)


# Garages ---------------------------------------------------------------------------------------

@requires_docker
def test_every_garage_in_prague_is_one_car_park_the_gateway_accepts(parked):
    # Five garages: the one in Hradec Králové stays out.
    assert sorted(parked) == [
        "golemio-mr-parkit-a1a376da-9369-49ba-9bb2-207d3165e249",
        "golemio-mr-parkit-ac497b54-7c11-48a0-b48a-0c96efff5599",
        "golemio-mr-parkit-ad87218b-ecf1-4609-be04-5f2d09443713",
        "golemio-mr-parkit-b7e3c3ea-1567-4850-b161-268d3c170975",
    ], sorted(parked)
    for entity in parked.values():
        assert entity["type"] == "OffStreetParking"
        assert entity["id"].startswith(f"urn:ngsi-ld:OffStreetParking:praha.eu:{SPACE}:golemio-mr-parkit-")
        assert not open_data.schema_errors(SCHEMA, entity), (entity["id"], open_data.schema_errors(SCHEMA, entity))
    # The tariffs were read once, with the runner's key.
    assert StandIn.seen == [("/v3/parking-tariffs?primarySource=mr_parkit&limit=10000", TOKEN)]


@requires_docker
def test_a_garage_carries_its_name_address_reservation_page_and_tariff(parked):
    rokytka = parked["golemio-mr-parkit-a1a376da-9369-49ba-9bb2-207d3165e249"]
    assert rokytka["name"]["languageMap"] == {"cs": "Parking Rokytka - O2 arena - Piskáčkových 1024/3"}
    assert rokytka["address"]["value"] == {"streetAddress": "Piskáčkových 1024/3", "addressLocality": "Praha", "addressCountry": "CZ"}
    assert rokytka["url"]["value"] == "https://www.mrparkit.com/"
    assert rokytka["location"]["value"] == {"type": "Point", "coordinates": [14.497767187186914, 50.1074693520369]}
    # 24.9 CZK an hour, 299 a day (the recorded tariff a1a376da).
    assert rokytka["priceRatePerMinute"]["value"] == 0.415
    assert rokytka["pricePerDay"]["value"] == 299
    assert rokytka["priceCurrency"]["value"] == "CZK"
    assert rokytka["dateModified"]["value"] == "2026-10-25T15:15:00Z"
    assert not {"availableSpotNumber", "occupiedSpotNumber", "totalSpotNumber"} & set(rokytka)
    # A record with only the formatted line keeps its street part.
    aragonit = parked["golemio-mr-parkit-ac497b54-7c11-48a0-b48a-0c96efff5599"]
    assert aragonit["address"]["value"]["streetAddress"] == "Pekařská 635/6"


@requires_docker
def test_out_of_area_zero_and_nameless_garages_are_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["properties"]["name"] = " "
        features[2]["properties"]["primary_source_id"] = "../x"
    keys = written_ok(garages(edited(GARAGES, edit)))
    assert list(keys) == ["golemio-mr-parkit-ad87218b-ecf1-4609-be04-5f2d09443713"]


@requires_docker
def test_a_duplicate_garage_keeps_the_newer_record():
    def edit(features):
        twin = copy.deepcopy(features[0])
        twin["properties"]["last_updated_at"] = "2026-10-25T16:00:00.000Z"
        twin["properties"]["name"] = "Parking Rokytka"
        features.append(twin)
    keys = written_ok(garages(edited(GARAGES, edit)))
    assert len(keys) == 4
    assert keys["golemio-mr-parkit-a1a376da-9369-49ba-9bb2-207d3165e249"]["name"]["languageMap"]["cs"] == "Parking Rokytka"


@requires_docker
def test_an_out_of_range_price_is_left_out_and_the_garage_kept():
    tariffs = copy.deepcopy(TARIFFS)
    rokytka = next(t for t in tariffs if t["id"] == "a1a376da-9369-49ba-9bb2-207d3165e249")
    for band in rokytka["charge_bands"]:
        for charge in band["charges"]:
            charge["charge"] = "-5" if charge["charge_interval"] == 3600 else "9999999"
    garage = written_ok(garages(tariffs=tariffs))["golemio-mr-parkit-a1a376da-9369-49ba-9bb2-207d3165e249"]
    assert "location" in garage
    assert not {"priceRatePerMinute", "pricePerDay", "priceCurrency"} & set(garage)


@requires_docker
def test_a_garage_whose_tariff_is_missing_has_no_price():
    tariffs = [t for t in TARIFFS if t["id"] != "a1a376da-9369-49ba-9bb2-207d3165e249"]
    keys = written_ok(garages(tariffs=tariffs))
    assert len(keys) == 4
    assert "priceCurrency" not in keys["golemio-mr-parkit-a1a376da-9369-49ba-9bb2-207d3165e249"]


@pytest.mark.parametrize(
    ("document", "tariffs", "status", "reason"),
    [
        ({"error_status": 500}, TARIFFS, 200, "not a feature list"),
        ({"type": "FeatureCollection", "features": []}, TARIFFS, 200, "no Mr.Parkit garage"),
        (GARAGES, {"error_status": 403}, 200, "tariffs are not a list"),
        (GARAGES, b"x" * 8388609, 200, "more than the 8 MiB"),
        (GARAGES, TARIFFS, 500, ""),
    ],
    ids=["no-feature-list", "empty", "tariffs-not-a-list", "tariffs-too-big", "tariffs-fail"],
)
@requires_docker
def test_a_failed_read_fails_the_run_and_writes_nothing(document, tariffs, status, reason):
    entities, errors = garages(document, tariffs, status)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


# Shared cars -----------------------------------------------------------------------------------

@requires_docker
def test_every_free_shared_car_in_prague_is_one_vehicle_the_gateway_accepts(cars):
    assert sorted(cars) == ["sharedcar-7sa7592", "sharedcar-8ab4387", "sharedcar-9p48966", "sharedcar-9p49571"], sorted(cars)
    for entity in cars.values():
        assert entity["type"] == "Vehicle"
        assert entity["vehicleType"]["value"] == "car" and entity["category"]["value"] == ["tracked"]
        assert not open_data.schema_errors(SCHEMA, entity), (entity["id"], open_data.schema_errors(SCHEMA, entity))


@requires_docker
def test_a_shared_car_carries_its_model_company_and_expiry(cars):
    yaris = cars["sharedcar-8ab4387"]
    # Golemio writes Anytime's make twice ("Toyota Toyota Yaris").
    assert yaris["name"]["languageMap"] == {"cs": "Toyota Yaris", "en": "Toyota Yaris"}
    assert yaris["operatorName"]["value"] == "Anytime"
    assert yaris["location"]["value"] == {"type": "Point", "coordinates": [14.48588, 50.100166]}
    assert yaris["dateObserved"]["value"] == "2026-10-25T15:24:00Z"
    assert yaris["location"]["observedAt"] == "2026-10-25T15:24:00Z"
    # A car rented out leaves Golemio's list: its entity ends 15 minutes after the report.
    assert yaris["expiresAt"] == "2026-10-25T15:39:00Z"
    assert cars["sharedcar-9p48966"]["name"]["languageMap"]["cs"] == "Audi A3"


@requires_docker
def test_out_of_area_zero_and_keyless_cars_are_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["geometry"]["coordinates"] = [16.6, 49.2]
        features[2]["properties"]["id"] = "cesky-carsharing-"
    entities, errors = bento("praha-pipeline-golemio-shared-cars-bento.yaml", edited(CARS, edit))
    assert not errors and list(by_key(entities)) == ["sharedcar-9p49571"], (entities, errors)


@requires_docker
def test_a_duplicate_car_keeps_the_newer_position():
    def edit(features):
        twin = copy.deepcopy(features[3])
        twin["properties"]["updated_at"] = "2026-10-25T15:26:00.000Z"
        twin["geometry"]["coordinates"] = [14.4, 50.05]
        features.append(twin)
    entities, errors = bento("praha-pipeline-golemio-shared-cars-bento.yaml", edited(CARS, edit))
    car = by_key(entities)["sharedcar-9p49571"]
    assert not errors and len(entities) == 4
    assert car["location"]["value"]["coordinates"] == [14.4, 50.05]


@pytest.mark.parametrize(
    ("document", "reason"),
    [
        ({"error_status": 500}, "not a feature list"),
        ({"type": "FeatureCollection", "features": []}, "no shared car"),
        (edited(CARS, lambda f: [c["properties"].update(updated_at="2026-10-25T14:00:00.000Z") for c in f]), "older than 30 minutes"),
    ],
    ids=["no-feature-list", "empty", "stale"],
)
@requires_docker
def test_a_failed_car_read_fails_the_run_and_writes_nothing(document, reason):
    entities, errors = bento("praha-pipeline-golemio-shared-cars-bento.yaml", document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


# Who reads what --------------------------------------------------------------------------------

def policy(name: str) -> dict:
    return yaml.safe_load((PRAHA / f"praha-policy-{name}.yaml").read_text())["spec"]


def test_each_dataset_policy_reads_only_its_own_rows():
    # The park-and-ride dataset keeps to the IPR car parks, the garage dataset to Mr.Parkit's.
    assert policy("public-parkovani-pr")["information"][0]["entities"][0]["idPattern"].endswith(":ipr-[0-9a-f-]{36}$")
    assert policy("public-garaze")["information"][0]["entities"][0]["idPattern"].endswith(":golemio-mr-parkit-[0-9a-f-]{36}$")
    # PID vehicles and shared cars share the type Vehicle: each Policy names its own kind.
    assert policy("public-vozidla")["q"] == 'vehicleType=="tram","bus","trolley","other"'
    assert policy("public-sdilena-auta")["information"][0]["entities"][0]["idPattern"].endswith(":sharedcar-[a-z0-9]{1,20}$")
