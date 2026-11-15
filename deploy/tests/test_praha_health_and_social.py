"""Prague's health care and social services (T-5691, PL-03), one PointOfInterest each, published as the
dataset praha-zdravotni-a-socialni-sluzby.

Bento runs each committed mapping over real records downloaded on 2026-10-25: five rows of the NRPZS
file (`fixtures/praha/nrpzs.csv`, four places of legal persons in Prague and one outside it, the
professional representative, phone, fax, e-mail and data box blanked), and six rows of MPSV's SPARQL
answer with the two ČÚZK answers the run asks for (`fixtures/praha/rpss.json`): by address point code
and, for a retired code, by address text. The edge cases are those answers edited in the one place
they name. Each entity is checked against the space's generated JSON Schema, which the gateway
enforces, and the pinned Smart Data Models PointOfInterest schema.
"""

import csv
import io
import json
import os
import re
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-health-and-social")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
FIXTURES = Path(__file__).resolve().parent / "fixtures/praha"
NRPZS = (FIXTURES / "nrpzs.csv").read_text(encoding="utf-8")
RPSS = json.loads((FIXTURES / "rpss.json").read_text())
SPACE = "praha-mesto"
CUZK = "https://ags.cuzk.cz/arcgis/rest/services/RUIAN/Prohlizeci_sluzba_nad_daty_RUIAN/MapServer/1/query"
POLICY = yaml.safe_load((PRAHA / "praha-policy-public-zdravotni-a-socialni-sluzby.yaml").read_text())
ID_PATTERN = POLICY["spec"]["information"][0]["entities"][0]["idPattern"]


def mapping(name: str, answers: tuple = (), keep_errors: bool = True) -> Path:
    """The committed mapping of `name`, each ČÚZK http step answered by the next of `answers`
    (a document, or a string the step answers as its body), errors answered as a message."""
    pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text())
    pending = list(answers)
    for step in pipeline["pipeline"]["processors"][0]["try"]:
        if "branch" in step:
            (http,) = step["branch"]["processors"]
            assert http["http"]["url"] == CUZK and http["http"]["verb"] == "POST"
            answer = pending.pop(0)
            body = answer if isinstance(answer, str) else json.dumps(answer, ensure_ascii=False)
            step["branch"]["processors"] = [{"mapping": "root = %s" % json.dumps(body)}]
    if keep_errors:
        pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    handle, path = tempfile.mkstemp(suffix=".yaml")
    os.close(handle)
    Path(path).write_text(yaml.safe_dump(pipeline, allow_unicode=True))
    return Path(path)


def outcome(name: str, document: bytes, answers: tuple = ()) -> tuple[list[dict], list[str]]:
    """The entities the mapping writes over one answer, or the error that fails the run."""
    pinned = mapping(name, answers)
    try:
        messages = open_data.run_all(pinned, document, SPACE, "praha.eu")
    finally:
        pinned.unlink()
    errors = [m["__error"] for m in messages if "__error" in m]
    return [m for m in messages if "__error" not in m], errors


def health(document: str = NRPZS) -> dict[str, dict]:
    entities, errors = outcome("nrpzs-health-care", document.encode())
    assert not errors, errors
    return {e["id"].rsplit(":nrpzs-", 1)[1]: e for e in entities}


def social(sparql: dict | None = None, by_code=None, by_address=None) -> tuple[list[dict], list[str]]:
    answers = (RPSS["byCode"] if by_code is None else by_code, RPSS["byAddress"] if by_address is None else by_address)
    return outcome("rpss-social-services", json.dumps(sparql or RPSS["sparql"]).encode(), answers)


def nrpzs_edited(edit) -> str:
    rows = list(csv.DictReader(io.StringIO(NRPZS)))
    rows = edit(rows) or rows
    out = io.StringIO()
    writer = csv.DictWriter(out, fieldnames=list(csv.DictReader(io.StringIO(NRPZS)).fieldnames), quoting=csv.QUOTE_ALL,
                            lineterminator="\r\n")
    writer.writeheader()
    writer.writerows(rows)
    return out.getvalue()


def prague_rows() -> list[dict]:
    return [r for r in csv.DictReader(io.StringIO(NRPZS)) if r["ZZ_kraj_kod"] == "CZ010"]


def valid(entity: dict) -> None:
    assert re.match(ID_PATTERN, entity["id"]), entity["id"]
    assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
    assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity), entity["id"]
    lon, lat = entity["location"]["value"]["coordinates"]
    assert 14.2 <= lon <= 14.75 and 49.9 <= lat <= 50.2, entity["id"]


@requires_docker
def test_every_place_of_a_legal_person_in_prague_is_one_point_of_interest_at_the_registers_point():
    written = health()
    assert sorted(written) == sorted(r["ZZ_misto_poskytovani_ID"] for r in prague_rows())
    for row in prague_rows():
        entity = written[row["ZZ_misto_poskytovani_ID"]]
        valid(entity)
        assert entity["name"]["languageMap"] == {"cs": row["ZZ_nazev"]}
        assert entity["serviceCategory"]["value"] == "healthCareProvider"
        assert entity["facilityType"]["value"] == row["ZZ_druh_nazev"]
        assert entity["address"]["value"] == {
            "streetAddress": f'{row["ZZ_ulice"]} {row["ZZ_cislo_domovni_orientacni"]}', "addressLocality": "Praha",
            "addressCountry": "CZ"}
        # POINT(latitude longitude) in the register, longitude first in GeoJSON.
        lat, lon = (float(n) for n in row["ZZ_GPS"][6:-1].split())
        assert entity["location"]["value"]["coordinates"] == [lon, lat]
        assert entity["dataProvider"]["value"].endswith("poloha z registru")
    # A web address without a scheme gets https; a place without one has no url.
    assert written["246608"]["url"]["value"] == "https://www.mamababystudio.cz"
    assert "url" not in written["263221"]


@requires_docker
def test_a_sole_trader_a_place_outside_prague_without_a_point_or_twice_listed_is_dropped():
    def edit(rows):
        rows[0]["poskytovatel_osoba_typ"] = "fyzická osoba"  # a sole trader: name and maybe home
        rows[1]["ZZ_GPS"] = ""  # no point
        rows[2]["ZZ_GPS"] = "POINT(49.19 16.60)"  # Brno, though the row says Prague
        rows.append(dict(rows[3]))  # the same place twice
        # Three more places, so more than half the legal persons' rows survive.
        rows.extend(dict(rows[3], ZZ_misto_poskytovani_ID=f"90000{n}", ZZ_nazev=f"Klinika {n} a.s.") for n in range(3))
    written = health(nrpzs_edited(edit))
    assert sorted(written) == sorted([prague_rows()[3]["ZZ_misto_poskytovani_ID"], "900000", "900001", "900002"])


@requires_docker
def test_a_file_without_the_columns_read_or_without_a_prague_row_fails_the_run():
    renamed = NRPZS.replace('"ZZ_GPS"', '"GPS"', 1)
    entities, errors = outcome("nrpzs-health-care", renamed.encode())
    assert not entities and "lacks the columns ZZ_GPS" in errors[0]
    elsewhere = nrpzs_edited(lambda rows: [r for r in rows if r["ZZ_kraj_kod"] != "CZ010"])
    entities, errors = outcome("nrpzs-health-care", elsewhere.encode())
    assert not entities and "no place of care in Prague" in errors[0]


@requires_docker
def test_fewer_than_half_the_places_with_a_point_fails_the_run_so_the_written_ones_stay():
    def edit(rows):
        for row in rows[:3]:
            row["ZZ_GPS"] = "POINT(0 0)"
    entities, errors = outcome("nrpzs-health-care", nrpzs_edited(edit).encode())
    assert not entities and "only 1 of 4" in errors[0]


def services() -> set[str]:
    return {f'{b["sluzba"]["value"]}-{b["kod"]["value"]}' for b in RPSS["sparql"]["results"]["bindings"]}


@requires_docker
def test_every_social_service_and_facility_address_is_one_point_of_interest_geocoded_at_cuzk():
    entities, errors = social()
    assert not errors, errors
    written = {e["id"].rsplit(":rpss-", 1)[1]: e for e in entities}
    # The register lists one row per renamed facility: the service at one address is one place.
    assert sorted(written) == sorted(services())
    for key, entity in written.items():
        valid(entity)
        assert entity["serviceCategory"]["value"] == "socialService"
        assert entity["source"]["value"] == "https://data.mpsv.cz/od/soubory/rpss/rpss.json"
    by_code = {f["attributes"]["kod"]: f["geometry"] for f in RPSS["byCode"]["features"]}
    centre = written["3105-22373284"]
    assert centre["name"]["languageMap"] == {"cs": "Gerontologické centrum"}
    assert centre["facilityType"]["value"] == "denní stacionáře"
    assert centre["address"]["value"]["streetAddress"] == "Šimůnkova 1600/5"
    assert centre["location"]["value"]["coordinates"] == [by_code[22373284]["x"], by_code[22373284]["y"]]
    assert "adresního místa RÚIAN 22373284" in centre["dataProvider"]["value"]
    # A code RÚIAN retired is found again by its address text, and the entity says so.
    home = written["4674-80093344"]
    assert home["location"]["value"]["coordinates"] == [14.36324347472157, 50.091089890311935]
    assert "z adresy U vojenské nemocnice 1200/1 (adresní místo RÚIAN 85647811)" in home["dataProvider"]["value"]


@requires_docker
def test_an_address_cuzk_answers_twice_or_not_at_all_leaves_the_service_out():
    twice = json.loads(json.dumps(RPSS["byAddress"]))
    twice["features"].append(dict(twice["features"][0], attributes={"kod": 1, "adresa": "U vojenské nemocnice 1200/1, Jinde, 10000 Praha 9"}))
    for answer in (twice, dict(RPSS["byAddress"], features=[])):
        entities, errors = social(by_address=answer)
        assert not errors, errors
        assert sorted(e["id"].rsplit(":rpss-", 1)[1] for e in entities) == sorted(services() - {"4674-80093344"})


@requires_docker
def test_an_empty_or_failed_answer_fails_the_run():
    empty = {"head": {"vars": []}, "results": {"bindings": []}}
    entities, errors = social(sparql=empty)
    assert not entities and "listed no social service in Prague" in errors[0]
    entities, errors = social(by_code={"error": {"code": 500, "message": "Unable to complete operation."}})
    assert not entities and "Unable to complete operation." in errors[0]
    # A secret address is never asked for: the query names utajenaAdresa 0.
    url = yaml.safe_load((PRAHA / "praha-datasource-rpss-social-services.yaml").read_text())["spec"]["http"]["url"]
    assert "utajenaAdresa%200" in url and "vedouci" not in url and "telefon" not in url.lower()


def test_both_feeds_are_seeded_published_and_readable_only_through_their_policy():
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-zdravotni-a-socialni-sluzby.yaml").read_text())
    assert endpoint["spec"]["policyRef"] == "urn:ngsi-ld:Policy:praha.eu:praha-mesto:public-zdravotni-a-socialni-sluzby"
    named = {s["url"] for s in endpoint["spec"]["catalog"]["source"]}
    for name in ("nrpzs-health-care", "rpss-social-services"):
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        datasource = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == datasource["metadata"]["name"] == name
        assert pipeline["spec"]["output"] == {"type": "PointOfInterest", "mode": "upsert"}
        assert pipeline["spec"]["quotas"]["maxMemoryMb"] <= 256
        assert datasource["spec"]["http"]["url"] in named
        for seeded in (f"praha-datasource-{name}.yaml", f"praha-pipeline-{name}.yaml", f"praha-pipeline-{name}-bento.yaml"):
            assert seeded in index, seeded
    for allowed in ("urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:nrpzs-263221",
                    "urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:rpss-4674-80093344"):
        assert re.match(ID_PATTERN, allowed)
    for other in ("urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:geo-health-centres-x",
                  "urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:golemio-police-1",
                  "urn:ngsi-ld:PointOfInterest:praha.eu:praha-mesto:nrpzs-1-2"):
        assert not re.match(ID_PATTERN, other)
