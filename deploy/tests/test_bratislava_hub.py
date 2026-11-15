"""The Bratislava hub's districts, places and statistics tables become Smart Data Models entities,
checked and counted (T-5274).

Bento runs each committed mapping over answers recorded from data.bratislava.sk's Feature Services
from 2026-10-23 to 2026-10-25 (`fixtures/bratislava/hub/`): the 17 districts, four tables of places
and the 431 statistics tables of the eight statistics pipelines (the first 18 rows of each table, or
of each page of the tables T-5671 reads in pages or as the hub's sums; some tables of the 2026-10-25
batches T-5435 and T-5671 to T-5676 whole). The calls to the hub are the
cut: each table's answer comes from the recording. The tests add the rows the owner's validation
asks for (2026-10-22T18:10Z): a point outside the city, a value out of range and a duplicate, each
dropped with a counted reason. Without a container runtime the Bento cases skip.
"""

import copy
import json
from datetime import date
import re
import subprocess
import tempfile
from datetime import date
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-bratislava-hub")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/bratislava"
FIXTURES = Path(__file__).resolve().parent / "fixtures/bratislava/hub"
SCHEMA = SEED / "bratislava.v1.schema.json"
HUB = "https://services8.arcgis.com/pRlN1m0su5BYaFAS/arcgis/rest/services/"
URN = "urn:ngsi-ld:{}:bratislava.sk:bratislava:{}"


def kpi(local: str) -> str:
    return URN.format("KeyPerformanceIndicator", local)


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text())


def run(
    mapping: str, document: dict, answers: dict | None = None
) -> tuple[list[dict], str]:
    """The committed mapping over `document`; each table request answered from `answers` by the
    path the mapping asks for, an unknown path refused."""
    steps = yaml.safe_load((SEED / mapping).read_text())["pipeline"]["processors"]
    # A path with a dot ("1._ročníka") is a path to `get`: the dots go out of the key. A paged
    # answer ({query, pages}) is answered page by page and only for the query it was recorded
    # with; a single recorded answer is the first page of a table the hub sends whole.
    table = json.dumps({k.replace(".", "%2E"): v for k, v in (answers or {}).items()})
    answer = [
        {
            "mapping": f"let a = {table}.get(@path.replace_all(\".\", \"%2E\")).or(throw(\"unasked\"))\n"
            'root = if $a.pages != null {\n'
            '  if $a.query != @query { throw("asked " + @query + ", recorded " + $a.query) }\n'
            '  else { $a.pages.index(@offset.number().int64() / 1000).or(throw("no page at " + @offset)) }\n'
            '} else if @offset.or("0") == "0" { $a } else { throw("no page at " + @offset) }'
        }
    ]
    for step in steps:
        page = step.get("for_each", [{}])[0].get("branch", {}).get("processors", [{}])[0].get("while")
        if page:
            http = page["processors"][0]["branch"]["processors"][-1]["http"]
            assert http["url"].startswith(HUB + "${! @path }/FeatureServer/0/query?${! @query }&"), http["url"]
            page["processors"][0]["branch"]["processors"] = answer
        elif "branch" in step:
            http = step["branch"]["processors"][-1]["http"]
            assert http["url"].startswith(HUB + "${! @path }/FeatureServer/"), (
                http["url"]
            )
            step["branch"]["processors"] = answer
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": {"processors": steps},
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "info", "format": "logfmt"},
    }
    # The recorded answers make the config too long for an argument: it goes in as a file.
    with tempfile.TemporaryDirectory() as work:
        Path(work, "c.json").write_text(json.dumps(config))
        Path(work).chmod(0o755)
        result = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "-i",
                "-e",
                "JC_ORG_DOMAIN=bratislava.sk",
                "-e",
                "JC_SPACE=bratislava",
                "-v",
                f"{work}:/work:ro",
                "--entrypoint",
                "/bento",
                BENTO,
                "-c",
                "/work/c.json",
            ],
            input=json.dumps(document).encode(),
            capture_output=True,
            timeout=180,
            check=False,
        )
    lines = [
        line for line in result.stdout.decode().splitlines() if line.startswith("[")
    ]
    return [
        e for line in lines for e in json.loads(line)
    ], result.stderr.decode() + result.stdout.decode()


def valid(entity: dict, model: str) -> None:
    assert not open_data.schema_errors(SCHEMA, entity), entity
    assert not open_data.sdm_errors(model, entity), entity


def statistics() -> dict:
    """The recorded tables plus the test's own rows: offences in a district the city does not
    have, an unemployment rate of 140 % and the Nové Mesto offences of 2025 twice."""
    answers = copy.deepcopy(fixture("statistics.json"))
    offences = next(
        v for k, v in answers.items() if k.startswith("Po%C4%8Det_priestupkov")
    )["features"]
    rows = next(
        v
        for k, v in answers.items()
        if k.startswith("Miera_evidovanej_nezamestnanosti")
    )["features"]
    nove = next(f for f in offences if f["attributes"]["MESTSKA_CAST"] == "Nové Mesto")
    offences += [
        {"attributes": dict(nove["attributes"], MESTSKA_CAST="Bratislava I")},
        copy.deepcopy(nove),
    ]
    rows.append(
        {
            "attributes": dict(
                rows[0]["attributes"], NEZAMESTNANOST_V__=140.0, ROK="2019"
            )
        }
    )
    return answers


@requires_docker
def test_the_district_layer_maps_to_17_valid_areas():
    areas, log = run(
        "bratislava-pipeline-districts-bento.yaml", fixture("districts.geojson")
    )
    assert len(areas) == 17, log
    by_id = {a["id"]: a for a in areas}
    nove = by_id[URN.format("AdministrativeArea", "district-529346")]
    assert (
        nove["name"]["value"] == "Nové Mesto"
        and nove["districtCode"]["value"] == "529346"
    )
    assert nove["location"]["value"]["type"] == "Polygon"
    for area in areas:
        valid(area, "extensions/AdministrativeArea")


@requires_docker
def test_a_district_outline_outside_the_city_writes_nothing():
    layer = copy.deepcopy(fixture("districts.geojson"))
    layer["features"][0]["geometry"]["coordinates"][0][0] = [19.14, 48.74]
    areas, log = run("bratislava-pipeline-districts-bento.yaml", layer)
    assert areas == [] and "the outline of Staré Mesto leaves Bratislava" in log, log
    layer["features"] = layer["features"][:16]
    areas, log = run("bratislava-pipeline-districts-bento.yaml", layer)
    assert areas == [] and "holds 16 districts, not 17" in log, log


@requires_docker
def test_the_recorded_tables_of_places_map_to_valid_places_and_each_bad_row_is_dropped_by_reason():
    answers = copy.deepcopy(fixture("places.json"))
    fix = next(v for k, v in answers.items() if k.startswith("Fixpointy"))["features"]
    # The same drop point again, and once more 5 m away under a new address line.
    fix.append(copy.deepcopy(fix[0]))
    fix.append(
        {
            "attributes": dict(
                fix[0]["attributes"],
                Adresa="Stavbárska 40A",
                Latitude=str(
                    float(fix[0]["attributes"]["Latitude"].replace(",", ".")) + 0.00004
                ),
            )
        }
    )
    places, log = run(
        "bratislava-pipeline-places-bento.yaml", fixture("services.json"), answers
    )
    # The recording's own defects: Kreatívne centrum Arténa at 48.735, 19.136 is Banská Bystrica,
    # seven animal farms of the region lie outside the city, the education table lists one school
    # twice at one address; 80 rows whose address the register does not place have no position
    # (T-5444, and one city organisation of T-5690).
    assert 'dropped by reason {\\"duplicate\\":26,\\"no position\\":80,\\"outside Bratislava\\":8}' in log, (
        log
    )
    by_key = {}
    for place in places:
        by_key.setdefault(place["id"].split(":")[-1].split(".")[0], []).append(place)
        valid(place, "dataModel.PointOfInterest/PointOfInterest")
    assert {k: len(v) for k, v in by_key.items()} == {
        "bezbarierovost": 161,
        "celozivotne-vzdelavanie": 98,
        "fixpointy": 13,
        "sluzby-bez-domova": 36,
        "veterinarne-zariadenia": 451,
        "ochladzovacie-miesta": 25,
        "kontaktne-miesta": 6,
        "vzdelavacie-institucie": 457,
        "centra-volneho-casu": 15,
        "materske-skoly": 147,
        "zakladne-umelecke-skoly": 93,
        "zakladne-skoly": 109,
        "mestske-organizacie": 50,
    }
    # A layer's own point geometry, read in WGS 84.
    vet = next(p for p in by_key["veterinarne-zariadenia"] if p["name"]["value"] == "Sloboda zvierat o.z.")
    assert vet["location"]["value"]["coordinates"] == [17.061178, 48.174148]
    # An address-only kindergarten at the register's point for "Bohrova 1" in Petržalka.
    school = next(p for p in by_key["materske-skoly"] if p["name"]["value"] == "MŠ Bohrova")
    assert school["location"]["value"]["coordinates"] == [17.096945, 48.130352]
    assert school["url"]["value"] == "https://msbohrova.edupage.org/"
    # Each place names how it got its point (T-5690): the register for an address-only table.
    assert school["dataProvider"]["value"].endswith("GPS: Register adries (ra.synchronix.sk), by address")
    assert vet["dataProvider"]["value"].endswith("GPS: the source's own coordinates")
    # A city organisation's postal address is placed by its street and number.
    library = next(p for p in by_key["mestske-organizacie"] if p["name"]["value"] == "Mestská knižnica Bratislava")
    assert "Register adries" in library["dataProvider"]["value"] and "physical" not in library["source"]["value"]
    assert library["category"]["value"] == ["city-organisation", "Príspevková organizácia"]
    # An address the register does not hold ("Marie Curie-Sklodowskej 1") is dropped, never guessed.
    assert not [p for p in places if p["address"]["value"].get("streetAddress") == "Marie Curie-Sklodowskej 1"]
    # The lifelong-learning table writes the latitude in X: the provider lands in Ružinov.
    provider = next(
        p
        for p in by_key["celozivotne-vzdelavanie"]
        if p["name"]["value"] == "3EC International a. s."
    )
    assert provider["location"]["value"]["coordinates"] == [17.159578, 48.145443]
    assert provider["url"]["value"] == "https://www.3ec.sk"
    assert provider["address"]["value"]["streetAddress"] == "Hraničná 18"


@requires_docker
def test_a_table_the_hub_no_longer_lists_is_left_out_and_logged():
    services = copy.deepcopy(fixture("services.json"))
    services["services"] = [
        s for s in services["services"] if not s["name"].startswith("Fixpointy")
    ]
    places, log = run(
        "bratislava-pipeline-places-bento.yaml", services, fixture("places.json")
    )
    assert "the hub no longer lists fixpointy" in log, log
    assert not [p for p in places if ":fixpointy." in p["id"]] and places


@requires_docker
def test_the_recorded_statistics_map_to_valid_indicators_and_each_bad_figure_is_dropped_by_reason():
    answers = statistics()
    indicators, log = run(
        "bratislava-pipeline-statistics-bento.yaml",
        fixture("services.json"),
        answers,
    )
    # The test's three rows on top of the recording: one district too many, one rate out of range
    # and one figure twice (beside the recording's own duplicate in the census tables).
    figures, dropped = reading("statistics", answers)
    assert logged(log) == dropped
    baseline = reading("statistics", fixture("statistics.json"))[1]
    assert dropped["out of range"] == baseline.get("out of range", 0) + 1
    assert dropped["not a city district"] == baseline["not a city district"] + 1
    assert dropped["duplicate"] == baseline.get("duplicate", 0) + 1
    by_id = {i["id"]: i for i in indicators}
    assert len(by_id) == len(indicators) == len(figures)
    for indicator in indicators:
        valid(indicator, "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator")
    offences = by_id[kpi("priestupky.pocet.529346.2025")]
    assert (
        offences["kpiValue"]["value"] == 20704
        and offences["area"]["value"] == "Nové Mesto"
    )
    assert offences["calculationPeriod"]["value"] == {
        "from": "2025-01-01",
        "to": "2025-12-31",
    }
    # The mosquito table's mojibake ("Raèa") and thousands with a space ("1 445") still land.
    assert by_id[kpi("komare.kontroly.529354.2025")]["kpiValue"]["value"] == 1445
    # The prognoses abbreviate ("D. N. Ves") and the fertility table counts five-year periods.
    assert kpi("prognoza-obyvatelia.pocet.529371.2035") in by_id
    fertility = by_id[kpi("plodnost.uhrnna.529435.2017-2021")]
    assert fertility["calculationPeriod"]["value"] == {
        "from": "2017-01-01",
        "to": "2021-12-31",
    }
    assert (
        by_id[kpi("nezamestnanost.miera.529401.2022-q3")]["calculationFrequency"][
            "value"
        ]
        == "quarterly"
    )
    # Only the all-founders row of a school table, never the districts' own schools beside it.
    assert by_id[kpi("ms-kapacita.miesta.528595.2021")]["kpiValue"]["value"] == 1953


def test_each_dataset_has_its_endpoint_and_policy_narrowed_to_its_own_rows():
    """Every table key of the places and statistics mappings is one public dataset: an Endpoint
    whose Policy reads only the ids of that table, harvested into the city's CKAN organisation."""
    kinds = {"places": "PointOfInterest"} | {p: "KeyPerformanceIndicator" for p in STATISTICS}
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    for pipeline, kind in kinds.items():
        text = (SEED / f"bratislava-pipeline-{pipeline}-bento.yaml").read_text()
        for key in sorted(set(re.findall(r'\{ ?"(?:key|name)": "([a-z0-9-]+)"', text))):
            endpoint = yaml.safe_load(
                (SEED / f"bratislava-endpoint-{key}.yaml").read_text()
            )
            policy = yaml.safe_load(
                (SEED / f"bratislava-policy-public-{key}.yaml").read_text()
            )
            assert (
                endpoint["spec"]["policyRef"]
                == f"urn:ngsi-ld:Policy:bratislava.sk:bratislava:public-{key}"
            )
            assert endpoint["spec"]["catalog"]["pipelineRef"] == {
                "kind": "Pipeline",
                "name": pipeline,
            }
            assert endpoint["spec"]["publish"]["ckan"]["instanceRef"] == {
                "kind": "CkanInstance",
                "name": "bratislava",
            }
            entities = policy["spec"]["information"][0]["entities"]
            assert entities == [
                {
                    "type": kind,
                    "idPattern": f"^urn:ngsi-ld:{kind}:[a-z0-9.-]+:bratislava:{key}[.][a-z0-9.-]+$",
                }
            ]
            assert "upsertBatch" not in policy["spec"]["operations"]
            assert (
                f"bratislava-endpoint-{key}.yaml" in index
                and f"bratislava-policy-public-{key}.yaml" in index
            )


STATISTICS = {"statistics": "district", "statistics-okresy": "okres", "statistics-mesto": "city", "statistics-denne": "city",
              "statistics-zsj": "zsj", "statistics-obce": "obec", "statistics-zsj-obyvatelia": "zsj", "statistics-zsj-vek": "zsj"}
CITY_DISTRICTS = {
    "stare mesto": "528595", "podunajske biskupice": "529311", "p biskupice": "529311", "ruzinov": "529320",
    "vrakuna": "529338", "nove mesto": "529346", "raca": "529354", "vajnory": "529362", "devinska nova ves": "529371",
    "d n ves": "529371", "dubravka": "529389", "karlova ves": "529397", "devin": "529401", "lamac": "529419",
    "zahorska bystrica": "529427", "z bystrica": "529427", "cunovo": "529435", "jarovce": "529443",
    "petrzalka": "529460", "rusovce": "529494",
}
OKRESY = {f"bratislava {n}": str(100 + i) for i, ns in enumerate([("i", "1"), ("ii", "2"), ("iii", "3"), ("iv", "4"), ("v", "5")], 1) for n in ns}
KRAJ = {"bratislavsky kraj": "sk010"}
FOLDED = str.maketrans("áäàâčçďéěëíĺľňóôöőŕřšťúüűůýžèò", "aaaaccdeeeillnoooorrstuuuuyzcn", ".")
YEAR = re.compile(r"(19|20)\d\d")


def fold(text) -> str:
    return " ".join(str(text).lower().translate(FOLDED).split())


def tables(pipeline: str) -> list[dict]:
    text = (SEED / f"bratislava-pipeline-{pipeline}-bento.yaml").read_text()
    start = text.index("let tables = [") + len("let tables = ")
    return json.loads(text[start : text.index("\n        ]", start) + 10])


def years(value) -> str | None:
    v = str(value).strip()
    if re.fullmatch(r"(19|20)\d\d", v) or re.fullmatch(r"(19|20)\d\d-(0[1-9]|1[0-2])", v):
        return v
    if re.fullmatch(r"(19|20)\d\d\s*[–-]\s*(19|20)\d\d", v):
        return "-".join(m.group() for m in YEAR.finditer(v))
    if re.fullmatch(r"(19|20)\d\d\s*[–-]\s*(19|20)\d\d a (19|20)\d\d\s*[–-]\s*(19|20)\d\d", v):
        return "-".join(m.group() for m in YEAR.finditer(v))
    if re.fullmatch(r"(19|20)\d\d-\d\d-\d\d", v):
        try:
            return date.fromisoformat(v).isoformat()
        except ValueError:
            return None
    return None


def number(value) -> float | None:
    if value is None:
        return None
    text = re.sub(r"[\s %]", "", str(value)).replace(",", ".")
    return float(text) if re.fullmatch(r"-?\d+(\.\d+)?", text) else None


def reading(pipeline: str, answers: dict) -> tuple[dict, dict]:
    """The figures each table of `pipeline` holds by the matrix's rule, read without the mapping:
    {id: value} and the count of dropped figures by reason."""
    figures, dropped, seen = {}, {}, 0
    for t in tables(pipeline):
        answer = answers[t["path"]]
        for row in [f for page in answer.get("pages", [answer]) for f in page["features"]]:
            r = row["attributes"]
            if "only" in t and not str(r.get(t["only"][0]) or "").startswith(t["only"][1]):
                continue
            name = fold(r.get(t["area"]) or "") if t.get("area") else "bratislava"
            zsj = str(r.get(t["area"]) or "").strip() if t["areas"] == "zsj" else ""
            code = {
                "district": CITY_DISTRICTS.get(name),
                "okres": OKRESY.get(name),
                "kraj": KRAJ.get(name),
                "zsj": zsj if re.fullmatch(r"\d{7}", zsj) and str(r.get(t.get("within")) or "").strip()
                in CITY_DISTRICTS.values() and str(r.get(t.get("label")) or "").strip() else None,
                "obec": re.sub(r"[^a-z0-9]+", "-", name).strip("-") or None,
            }.get(t["areas"], "582000" if name == "bratislava" else None)
            if "spans" in t:
                slug = years(t["spans"].get(str(r.get(t["year"]) or "").strip(), ""))
            elif "fixed" in t:
                slug = years(t["fixed"])
            elif "quarter" in t:
                y = str(r.get(t["year"]) or "").strip()
                q = re.sub(r"^([1-4])\s*\.?\s*[Qq]$", r"\1", str(r.get(t["quarter"]) or "").strip())
                slug = f"{y}-q{q}" if re.fullmatch(r"(19|20)\d\d", y) and q in {"1", "2", "3", "4"} else None
            elif "month" in t:
                y, m = str(r.get(t["year"]) or "").strip(), str(r.get(t["month"]) or "").strip().removeprefix("0")
                slug = f"{y}-{int(m):02d}" if re.fullmatch(r"(19|20)\d\d", y) and re.fullmatch(r"[1-9]|1[0-2]", m) else None
            else:
                slug = years(r.get(t["year"]) or "")
            labels = t.get("labels", {})
            by = [b if isinstance(b, dict) else {"code": b} for b in t.get("by", [])]
            # "Nan" is the hub's word for a category it did not detect: no breakdown (T-5673).
            raw = [str(r.get(b["code"]) or "").strip() for b in by]
            raw = ["" if v.lower() == "nan" else labels.get(v, v) for v in raw]
            cells = [(re.sub(r"[^a-z0-9]+", "-", fold(v)).strip("-"), b.get("optional", False)) for v, b in zip(raw, by)]
            for column, segment, _, low, high in t["values"]:
                n = number(r.get(column))
                n = abs(n) if n is not None and t.get("abs") else n
                reason = (
                    {"district": "not a city district", "okres": "not a Bratislava okres", "zsj": "not a ZSJ of the city",
                     "obec": "no municipality", "kraj": "not the Bratislava region"}.get(t["areas"], "not Bratislava")
                    if code is None
                    else "no period" if slug is None
                    else "in the future" if t.get("past") and slug[:4] > str(date.today().year)
                    else "no breakdown" if ("", False) in cells
                    else "no number" if n is None
                    else "out of range" if (low is not None and n < low) or (high is not None and n > high)
                    else None
                )
                if reason:
                    dropped[reason] = dropped.get(reason, 0) + 1
                    continue
                seen += 1
                figures.setdefault(kpi(".".join([t["name"], segment, *(c for c, _ in cells if c), code, slug])), n)
    if seen > len(figures):
        dropped["duplicate"] = seen - len(figures)
    return figures, dropped


def logged(log: str) -> dict:
    found = re.search(r"dropped by reason (\{.*?\})\"", log.replace('\\"', '"'))
    assert found, log
    return json.loads(found.group(1))


@requires_docker
@pytest.mark.parametrize("pipeline", STATISTICS)
def test_each_statistics_pipeline_agrees_with_an_independent_reading_of_its_tables(pipeline):
    """Id for id and value for value, the mapping writes what the tables hold by the matrix's rule
    (Research/bratislava-open-data.md 6.4), and drops the same figures for the same reasons; every
    figure is a valid KeyPerformanceIndicator of its pipeline's areas."""
    answers = fixture("statistics.json")
    indicators, log = run(f"bratislava-pipeline-{pipeline}-bento.yaml", fixture("services.json"), answers)
    figures, dropped = reading(pipeline, answers)
    assert {i["id"]: i["kpiValue"]["value"] for i in indicators} == figures
    assert len(indicators) == len(figures)
    assert logged(log) == dropped
    for indicator in indicators:
        valid(indicator, "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator")
        assert all(indicator["calculationPeriod"]["value"].values())


def test_the_statistics_pipelines_run_one_mapping_text_split_by_area():
    """The statistics pipelines differ only in their tables, each table in the pipeline of its areas,
    and no table twice."""
    def body(pipeline: str) -> str:
        text = (SEED / f"bratislava-pipeline-{pipeline}-bento.yaml").read_text()
        start = text.index("        let tables = [")
        return text[text.index("pipeline:"): start] + text[text.index("\n        ]", start):]

    assert len({body(p) for p in STATISTICS}) == 1
    keys = [t["name"] for p in STATISTICS for t in tables(p)]
    assert len(keys) == len(set(keys)) == 344 + 19 + 12 + 12 + 14 + 13 + 17
    for pipeline, areas in STATISTICS.items():
        # The city's pipeline also holds the two tables of the region beside Slovakia (T-5676).
        assert {t["areas"] for t in tables(pipeline)} == ({areas, "kraj"} if pipeline == "statistics-mesto" else {areas})
        manifest = yaml.safe_load((SEED / f"bratislava-pipeline-{pipeline}.yaml").read_text())
        assert manifest["spec"]["source"]["dataSourceRef"]["name"] == "bratislava-hub"
        assert manifest["spec"]["output"] == {"type": "KeyPerformanceIndicator", "mode": "upsert"}


@requires_docker
def test_a_city_table_keeps_bratislava_beside_slovakia_and_a_region_table_its_own_okresy():
    """The fixture's own cases: a Bratislava-and-Slovakia table drops the Slovakia rows, a table of
    the whole region the okresy outside Bratislava, a census table a breakdown left as "-"."""
    answers = fixture("statistics.json")
    city, log = run("bratislava-pipeline-statistics-mesto-bento.yaml", fixture("services.json"), answers)
    assert logged(log)["not Bratislava"] > 0
    assert {i["area"]["value"] for i in city} == {"Bratislava", "Bratislavský kraj"}
    assert all(":582000." not in i["id"] or i["id"].count(".582000.") == 1 for i in city)
    okresy, log = run("bratislava-pipeline-statistics-okresy-bento.yaml", fixture("services.json"), answers)
    assert {i["area"]["value"] for i in okresy} == {f"Bratislava {n}" for n in ("I", "II", "III", "IV", "V")}
    drivers = kpi("pocet-evidovanych-vodicov.pocet-evidovanych-vodicov.101.2013")
    assert {i["id"]: i for i in okresy}[drivers]["kpiValue"]["value"] == 22038


def paged(name: str, pipeline: str = "statistics-mesto") -> dict:
    return next(t for t in tables(pipeline) if t["name"] == name)


@requires_docker
def test_a_table_over_2000_rows_is_read_page_by_page_and_counted_by_the_hub():
    """T-5671: the sports classes table answers in four pages of at most 1,000 rows (the recording
    keeps 18 of each): every page reaches an indicator; the DTMB deposits are the hub's counts per
    year, and the five deposits dated 2028 and the two header rows of the ledger write nothing."""
    answers = fixture("statistics.json")
    sports = paged("pocet-ziakov-sportovych-tried", "statistics-okresy")
    assert len(answers[sports["path"]]["pages"]) == 4
    okresy, _ = run("bratislava-pipeline-statistics-okresy-bento.yaml", fixture("services.json"), answers)
    years = {i["id"].rsplit(".", 1)[1] for i in okresy if ":pocet-ziakov-sportovych-tried." in i["id"]}
    pages = [{str(f["attributes"]["Rok"]) for f in p["features"]} for p in answers[sports["path"]]["pages"]]
    assert set().union(*pages) == years and len(pages[-1] - pages[0]) > 0

    city, log = run("bratislava-pipeline-statistics-mesto-bento.yaml", fixture("services.json"), answers)
    deposits = {i["id"]: i["kpiValue"]["value"] for i in city if ":vklady-dtmb-2020-2025." in i["id"]}
    assert deposits[kpi("vklady-dtmb-2020-2025.pocet.582000.2020")] == 1159
    assert sorted(k.rsplit(".", 1)[1] for k in deposits) == ["2020", "2021", "2022", "2023", "2024", "2025", "2026"]
    assert logged(log)["in the future"] == 1
    budget = {i["id"]: i for i in city if ":cerpanie-rozpoctu-2021." in i["id"]}
    revenue = budget[kpi("cerpanie-rozpoctu-2021.cerpanie-eur.prijmy.11.582000.2021")]
    assert revenue["kpiValue"]["value"] == 177323796.86
    assert revenue["name"]["value"] == "Čerpanie k 31. 12. (EUR), príjmy, 11, Bratislava, 2021"


@requires_docker
def test_a_table_with_more_than_20_pages_or_a_refused_page_writes_nothing_of_itself():
    """A table whose 20th page still says there is more fails whole, and so does one whose second
    page the hub refuses; the other tables of the run still write."""
    answers = copy.deepcopy(fixture("statistics.json"))
    endless = paged("autorizacie-dtmb-2020-2025")
    page = dict(answers[endless["path"]]["pages"][0], exceededTransferLimit=True)
    answers[endless["path"]]["pages"] = [page] * 21
    refused = paged("pocet-priestupkov-a-vyska-pokut")
    first = dict(answers[refused["path"]]["pages"][0], exceededTransferLimit=True)
    answers[refused["path"]]["pages"] = [first, {"error": {"code": 400, "message": "Invalid query"}}]
    city, log = run("bratislava-pipeline-statistics-mesto-bento.yaml", fixture("services.json"), answers)
    written = {i["id"].split(":")[-1].split(".")[0] for i in city}
    assert "autorizacie-dtmb-2020-2025" not in written and "pocet-priestupkov-a-vyska-pokut" not in written
    assert "autorizacie-dtmb-2019" in written and "rozpocet-2014-vydavky" in written
    assert "autorizacie-dtmb-2020-2025 not read: " in log and "more than 20 pages" in log
    assert "pocet-priestupkov-a-vyska-pokut not read: " in log and "Invalid query" in log


@requires_docker
def test_a_month_column_and_a_period_from_the_item_description_give_each_figure_its_period():
    """T-5672: the quarantine facility's months become monthly indicators with the month's last day
    (a month outside 1 to 12 is no period), a table whose description names the years carries that
    range, and the CVČ table's 170 blank rows drop as no period instead of failing the table."""
    answers = copy.deepcopy(fixture("statistics.json"))
    hradska = next(v for k, v in answers.items() if k.startswith("Mestsk%C3%A9%20karant%C3%A9ne"))["features"]
    hradska.append({"attributes": dict(hradska[0]["attributes"], Mesiac="13", Rok="2021")})
    city, log = run("bratislava-pipeline-statistics-mesto-bento.yaml", fixture("services.json"), answers)
    by_id = {i["id"]: i for i in city}
    feb = by_id[kpi("mestske-karantene-zariadenie-hradska.pocet-prijatych-klientov.582000.2021-02")]
    assert feb["calculationPeriod"]["value"] == {"from": "2021-02-01", "to": "2021-02-28"}
    assert feb["calculationFrequency"]["value"] == "monthly"
    oct_ = by_id[kpi("mestske-karantene-zariadenie-hradska.maximalna-kapacita-mkz.582000.2020-10")]
    assert oct_["kpiValue"]["value"] == 50 and oct_["calculationPeriod"]["value"]["to"] == "2020-10-31"
    assert logged(log) == reading("statistics-mesto", answers)[1]
    assert len([i for i in city if ":mestske-karantene-zariadenie-hradska." in i["id"]]) == 77
    founders = by_id[kpi("pocet-deti-a-specialnych-skol.pocet.ss-pocet-ziakov.iny-organ-statnej-spravy.582000.2021")]
    assert founders["kpiValue"]["value"] == 136
    okresy, log = run("bratislava-pipeline-statistics-okresy-bento.yaml", fixture("services.json"), answers)
    life = {i["id"]: i for i in okresy}[kpi("prispevky-dlzka-zivota-muzi-roky.prispevky-roky.0-19.101.2000-2019")]
    assert life["kpiValue"]["value"] == 0.31
    assert life["calculationPeriod"]["value"] == {"from": "2000-01-01", "to": "2019-12-31"}
    assert "calculationFrequency" not in life
    assert logged(log)["no period"] >= 170


@requires_docker
def test_a_census_table_reads_each_basic_settlement_unit_of_the_city_and_a_model_its_spans():
    """T-5674: a census 2021 table of the basic settlement units (ZSJ) keys each figure by the ZSJ's
    code, names it with its city district and drops a unit outside the city's 17; a growth model of
    the region's municipalities reads its 1991 as 1991-2021 and drops a year it does not span and a
    row without a municipality."""
    answers = copy.deepcopy(fixture("statistics.json"))
    floor = next(v for k, v in answers.items() if k.startswith("Po%C4%8Det%20bytov%20pod%C4%BEa%20celkovej"))["features"]
    malacky = floor[0]["attributes"] | {"Obec___kód": "507423", "Základná_sídelná_jednotka___kód": "1234567"}
    floor.append({"attributes": malacky})
    growth = next(v for k, v in answers.items() if k.startswith("Model_rastu_po%C4%8Dtu"))["features"]
    growth += [{"attributes": dict(growth[0]["attributes"], ROK="2051")}, {"attributes": dict(growth[0]["attributes"], OBEC=" ")}]
    zsj, log = run("bratislava-pipeline-statistics-zsj-bento.yaml", fixture("services.json"), answers)
    assert logged(log) == reading("statistics-zsj", answers)[1]
    assert logged(log)["not a ZSJ of the city"] == reading("statistics-zsj", fixture("statistics.json"))[1].get("not a ZSJ of the city", 0) + 5
    by_id = {i["id"]: i for i in zsj}
    small = by_id[kpi("pocet-bytov-podla-celkovej-plochy-zsj.do-40-m2.2040640.2021")]
    assert small["kpiValue"]["value"] == 5 and small["area"]["value"] == "Hradný svah (Staré Mesto)"
    assert small["calculationPeriod"]["value"] == {"from": "2021-01-01", "to": "2021-12-31"}
    assert not any(".1234567." in i for i in by_id)
    obce, log = run("bratislava-pipeline-statistics-obce-bento.yaml", fixture("services.json"), answers)
    assert logged(log) == reading("statistics-obce", answers)[1]
    assert logged(log)["no period"] == 1 and logged(log)["no municipality"] == 1
    vinicne = {i["id"]: i for i in obce}[kpi("model-rastu-poctu-obyvatelov-v-obciach-bsk.index-rastu.vinicne.1991-2021")]
    assert vinicne["kpiValue"]["value"] == 1.867268937 and vinicne["area"]["value"] == "Viničné"
    assert vinicne["calculationPeriod"]["value"] == {"from": "1991-01-01", "to": "2021-12-31"}
    assert "calculationFrequency" not in vinicne


@requires_docker
def test_the_p5_tables_read_each_zsj_column_as_a_figure_and_each_municipality_by_name():
    """T-5675: a census table of the residents of the basic settlement units (ZSJ) writes one figure
    per column and unit, an empty cell (a unit with nobody or an anonymised count) writes nothing; the
    age table one figure per year of age; a table of the region's municipalities keys each by its
    folded name, and the sports classes table of the okresy drops a row without a year."""
    answers = fixture("statistics.json")
    units, _ = run("bratislava-pipeline-statistics-zsj-obyvatelia-bento.yaml", fixture("services.json"), answers)
    by_id = {i["id"]: i for i in units}
    women = by_id[kpi("obyvatelia-zsj-pohlavie.zeny.2040640.2021")]
    assert women["kpiValue"]["value"] == 110 and women["area"]["value"] == "Hradný svah (Staré Mesto)"
    assert women["name"]["value"] == "Obyvatelia: Ženy, Hradný svah (Staré Mesto), 2021"
    # The castle (Hrad) has no residents: the hub leaves its cells empty and nothing is written.
    assert not any(".2040560." in i for i in by_id if "obyvatelia-zsj-pohlavie" in i)
    ages, _ = run("bratislava-pipeline-statistics-zsj-vek-bento.yaml", fixture("services.json"), answers)
    assert {i["id"] for i in ages} >= {kpi("obyvatelia-zsj-vek.vek-0.2059910.2021"), kpi("obyvatelia-zsj-vek.vek-99.2059910.2021")}
    assert {i["id"]: i for i in ages}[kpi("obyvatelia-zsj-vek.vek-0.2059910.2021")]["kpiValue"]["value"] == 77
    towns, _ = run("bratislava-pipeline-statistics-obce-bento.yaml", fixture("services.json"), answers)
    flats = {i["id"]: i for i in towns}[kpi("bytovy-fond-v-obciach-bsk.pocet-bytov.rohoznik-okres-malacky.1991")]
    assert flats["area"]["value"] == "Rohožník (Okres Malacky)" and flats["kpiValue"]["value"] == 866
    okresy, _ = run("bratislava-pipeline-statistics-okresy-bento.yaml", fixture("services.json"), answers)
    sport = [i for i in okresy if ":ziaci-sportovych-tried-podla-sportu." in i["id"]]
    assert sport and {i["area"]["value"] for i in sport} <= {"Bratislava II", "Bratislava V"}


@requires_docker
def test_a_named_breakdown_a_month_the_region_and_an_age_pyramid_land_as_the_source_writes_them():
    """T-5676: a budget line's id from its codes and its name from their titles, an optional code
    left out where empty; a street's cp1250 name repaired; a Wi-Fi month with its own last day;
    the region's row beside Slovakia's; the men's side of an age pyramid without its sign."""
    answers = fixture("statistics.json")
    city = {i["id"]: i for i in run("bratislava-pipeline-statistics-mesto-bento.yaml", fixture("services.json"), answers)[0]}
    budget = city[kpi("rozpocet-prijmy-2014.skutocnost.81.111.312001.582000.2014")]
    assert budget["kpiValue"]["value"] == 8106948.78
    assert budget["name"]["value"] == (
        "Skutočné príjmy k 31. 12. 2014 (EUR), Bežné príjmy, Príjmy zo štátneho rozpočtu, 312001, Bratislava, 2014"
    )
    # 2016 tells two lines of one code apart by the source type (R, M) and leaves it empty elsewhere.
    assert kpi("rozpocet-prijmy-2016.schvaleny.81.41.222003.r.582000.2016") in city
    assert kpi("rozpocet-prijmy-2016.schvaleny.81.41.222003.m.582000.2016") in city
    assert kpi("rozpocet-prijmy-2016.schvaleny.81.11e1.453.582000.2016") in city
    assert len([i for i in city if i.startswith(kpi("rozpocet-prijmy-2016.schvaleny."))]) == 95
    street = next(v for k, v in city.items() if k.startswith(kpi("priestupky-ulice.pocet.racianska-ulica.")))
    assert street["name"]["value"].startswith("Počet priestupkov na ulici, Račianska ulica,")
    wifi = city[kpi("wifi-pouzivatelia.pocet.pamv-oblast-pamatnika-vitazstva-a-pred-snd.582000.2019-02")]
    assert wifi["calculationPeriod"]["value"] == {"from": "2019-02-01", "to": "2019-02-28"}
    assert wifi["calculationFrequency"]["value"] == "monthly"
    region = city[kpi("pzi-rast.rast.sk010.2016")]
    assert region["kpiValue"]["value"] == 7.1 and region["area"]["value"] == "Bratislavský kraj"
    assert not [i for i in city if i.startswith(kpi("pzi-")) and ".sk010." not in i]
    social = city[kpi("osv-cerpanie-2019-09-06.cerpanie.bezne-vydavky-spolu.582000.2019")]
    assert social["kpiValue"]["value"] == 646340.35
    assert kpi("osv-cerpanie-2019-09-06.cerpanie.6111.noclaharen-mea-culpa.582000.2019") in city
    districts = {i["id"]: i for i in run("bratislava-pipeline-statistics-bento.yaml", fixture("services.json"), answers)[0]}
    men = districts[kpi("vekove-zlozenie-pocet.pocet.muzi.0.529419.2021")]
    assert men["kpiValue"]["value"] == 46 and men["name"]["value"] == "Počet obyvateľov, muži, 0, Lamač, 2021"
    assert districts[kpi("vekove-zlozenie-podiel.podiel.muzi.0.529419.2021")]["kpiValue"]["value"] == 0.0128
