"""Every mapped Banská Bystrica and BBSK dataset, seeded by one generator (T-4255).

`scripts/open-data/generate.py` writes the `{project}-od-*` files of the two project folders from
the T-4254 mapping. The seed has to equal what it writes, so a hand edit of a generated file or a
mapping change without a regeneration fails here. Bento runs the generated mappings over recorded
answers of the sources their DataSources name (`fixtures/open-data/`, fetched on 2026-10-16 and
trimmed to a few rows), and every entity is checked against the generated space's JSON Schema,
which the gateway enforces, and against its FIWARE Smart Data Model's schema.json, vendored in
`scripts/open-data/sdm/` (T-4319). Without a container runtime the Bento cases skip; the others do not.
"""

import http.server
import importlib.util
import json
import re
import subprocess
import sys
import threading
import urllib.parse
from collections import Counter
from collections.abc import Callable
from datetime import datetime, timezone
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-open-data-seed")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
FIXTURES = Path(__file__).resolve().parent / "fixtures/open-data"
SCRIPT = ROOT / "scripts/open-data/generate.py"

spec = importlib.util.spec_from_file_location("generate", SCRIPT)
generate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generate)

MAPPING = yaml.safe_load(generate.MAPPING.read_text())
SEEDED = [e for e in MAPPING["datasets"] if e["status"] == "mapped" and (e.get("existing") or {}).get("covers") != "whole"]
# As the generator writes them: without the columns that name or reach a person (T-5697).
for _e in SEEDED:
    generate.impersonal(_e)
SLUGS = generate.slugs(SEEDED)


def files(project: str, kind: str) -> list[Path]:
    return sorted(p for p in (SEED / project).glob(f"{project}-od-{kind}-*.yaml") if not p.name.endswith("-bento.yaml"))


def entry(name: str) -> dict:
    return next(e for e in SEEDED if e["name"] == name)


def run(project: str, slug: str, fixture: str) -> list[dict]:
    mapping = SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml"
    return open_data.run_all(mapping, (FIXTURES / f"{fixture}.json").read_bytes(), f"{project}-otvorene-data",
                             generate.PROJECTS[project]["domain"])


def run_bytes(project: str, slug: str, document: bytes) -> list[dict]:
    mapping = SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml"
    return open_data.run_all(mapping, document, f"{project}-otvorene-data", generate.PROJECTS[project]["domain"])


def schema_errors(project: str, entity: dict) -> list[str]:
    return open_data.schema_errors(SEED / project / f"{project}-otvorene-data.v1.schema.json", entity)


MAPPED = [e for e in MAPPING["datasets"] if e["status"] == "mapped"]
MODEL = {e["type"]: e["smartDataModel"] for e in MAPPED}
BASE = {e["type"]: e.get("baseModel") for e in MAPPED}
PINS = json.loads((generate.SDM / "pins.json").read_text())


def sdm_errors(entity: dict, model_of: str | None = None) -> list[str]:
    """The entity against its type's vendored Smart Data Model, or the extension the type has (T-4336)."""
    t = model_of or entity["type"]
    if t in generate.SPACE_EXTENSIONS:
        return open_data.sdm_errors(f"extensions/{t}", entity)
    schema = generate.sdm_schema({"name": t, "type": t, "smartDataModel": MODEL[t], "baseModel": BASE[t]})
    return open_data.sdm_errors(str(schema.parent.relative_to(generate.SDM)), entity)


# --- the seed is the generator's output ---

def test_the_committed_seed_is_what_the_mapping_generates():
    result = subprocess.run([sys.executable, str(SCRIPT), "--check"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr


def test_no_generated_pipeline_or_model_publishes_a_persons_contact_or_a_suppliers_street():
    """ADR-N-054, T-5697: a director's or statutory representative's name, phone and e-mail, and a
    supplier's street (a sole trader's home), never reach the public od spaces. A pipeline may still
    read such a column into its id hash; the model keeps the name only as a deprecated slot."""
    personal = re.compile(r'^\s*"(riaditel\w*|statutar\w*)"\s*:', re.M | re.I)
    street = re.compile(r'^\s*"(adresa|ulicaACislo|sidloDodavatela)"\s*:', re.M)
    found = []
    for path in sorted(SEED.glob("*/*-od-pipeline-*-bento.yaml")):
        text = path.read_text()
        found += [f"{path.name}: {m.group(0).strip()}" for m in personal.finditer(text)]
        if re.search(r"-od-pipeline-(dodavatelia|dotacie-2|faktury|objednavky|zmluvy)-bento", path.name):
            found += [f"{path.name}: {m.group(0).strip()}" for m in street.finditer(text)]
    for path in sorted(SEED.glob("*/*-otvorene-data.linkml.yaml")):
        slots = yaml.safe_load(path.read_text())["slots"]
        found += [f"{path.name}: {a}" for a, s in slots.items()
                  if re.match(r"(riaditel|statutar)", a, re.I) and "deprecated" not in (s or {})]
    assert not found, found


def test_a_keyed_dataset_never_asks_its_source_for_a_column_it_dropped_as_personal():
    """F-R2BNT1-007 (T-6215): only a row-hash id reads the columns impersonal() drops. A keyed id
    does not, so neither its query nor the public `source` link that repeats it names them."""
    raw = [e for e in yaml.safe_load(generate.MAPPING.read_text())["datasets"]
           if e["status"] == "mapped" and (e.get("existing") or {}).get("covers") != "whole"]
    asked, keyed = [], 0
    for e in raw:
        kept = entry(e["name"])
        gone = {p["column"] for a, p in e["properties"].items() if a not in kept["properties"]}
        if not gone or not e["entityId"].get("keys"):
            continue
        keyed += 1
        slug = SLUGS[e["name"]]
        for path in (SEED / e["org"]).glob(f"{e['org']}-od-*-{slug}*.yaml"):
            fields = {f for line in path.read_text().splitlines() if "outFields=" in line
                      for f in urllib.parse.unquote(line.split("outFields=")[1].split("&")[0]).split(",")}
            asked += [f"{path.name}: {c}" for c in sorted(gone & fields)]
    assert keyed, "no keyed dataset drops a column: this test checks nothing"
    assert not asked, asked


def test_dropping_a_personal_column_keeps_each_id_and_lets_the_next_replace_remove_it():
    """T-5697: the Entities on dev already hold the personal columns. A replace rewrites only the
    Entities its batch names and refuses one holding an attribute the view does not show (T-5236), so
    a hashed id still covers the dropped columns and each dropped attribute is a retired slot."""
    raw = [e for e in yaml.safe_load(generate.MAPPING.read_text())["datasets"]
           if e["status"] == "mapped" and (e.get("existing") or {}).get("covers") != "whole"]
    missing, hashed = [], 0
    for e in raw:
        kept = entry(e["name"])
        gone = set(e["properties"]) - set(kept["properties"])
        retired = generate.RETIRED.get(e["org"], {})
        missing += [f"{e['org']}/{e['type']}: {generate.attribute(a)}" for a in sorted(gone)
                    if generate.attribute(a) not in {*retired.get("*", []), *retired.get(e["type"], [])}]
        if gone and not e["entityId"].get("keys"):
            hashed += 1
            pipeline = (SEED / e["org"] / f"{e['org']}-od-pipeline-{SLUGS[e['name']]}-bento.yaml").read_text()
            local = next(line for line in pipeline.splitlines() if "let local = " in line)
            missing += [f"{e['name']}: id hash lacks {p['column']}" for a, p in e["properties"].items()
                        if a in gone and f'"{p["column"]}": ' not in local]
            # An ArcGIS query that stops asking for a hashed column hashes null for it: every id moves
            # and the old Entities keep the dropped values (dev 2026-10-26: zmluvy, odbory).
            asked = {f for line in pipeline.splitlines() if "outFields=" in line
                     for f in urllib.parse.unquote(line.split("outFields=")[1].split("&")[0]).split(",")}
            missing += [f"{e['name']}: query lacks hashed {p['column']}" for a, p in e["properties"].items()
                        if a in gone and asked and "*" not in asked and p["column"] not in asked]
    assert not missing, missing
    assert hashed, "no hashed dataset drops a column: the id half of this test checks nothing"


def test_every_mapped_dataset_has_its_pipeline_unless_a_seed_reads_it_whole():
    mapped = [e for e in MAPPING["datasets"] if e["status"] == "mapped"]
    whole = [e for e in mapped if (e.get("existing") or {}).get("covers") == "whole"]
    pipelines = {p.name for project in generate.PROJECTS for p in files(project, "pipeline")}
    assert len(mapped) - len(whole) == len(SEEDED)
    # A dataset of the city's layers may take several pipelines (generate.parts, T-4317).
    want = {f"{e['org']}-od-pipeline-{slug}.yaml" for e in SEEDED if e["source"]["kind"] != "endpoint"
            for _, slug in generate.parts(e, SLUGS[e["name"]])}
    assert pipelines == want
    # Each pipeline alone writes its rows, so it replaces them whole and a dropped attribute
    # leaves at the next run (PL-79, T-4407).
    modes = {yaml.safe_load(p.read_text())["spec"]["output"]["mode"] for project in generate.PROJECTS
             for p in files(project, "pipeline")}
    assert modes == {"replace"}
    # Every pipeline runs: T-4348's queue loads one batch at a time, so no dataset waits on a half (T-4515).
    off = [p.name for project in generate.PROJECTS for p in files(project, "pipeline")
           if not yaml.safe_load(p.read_text())["spec"]["enabled"]]
    assert off == []


def test_the_model_classes_and_both_policies_name_every_type_the_pipelines_write():
    for project in generate.PROJECTS:
        # A dataset its Endpoint alone holds keeps its type in the model and both Policies (T-5060).
        written = {yaml.safe_load(p.read_text())["spec"]["output"]["type"] for p in files(project, "pipeline")} | {
            e["type"] for e in SEEDED if e["org"] == project and e["source"]["kind"] == "endpoint"}
        model = yaml.safe_load((SEED / project / f"{project}-od-datamodel.yaml").read_text())
        schema = json.loads((SEED / project / f"{project}-otvorene-data.v1.schema.json").read_text())
        assert set(model["spec"]["classes"]) == written
        assert written <= set(schema["definitions"]), "regenerate the artifacts: generate.py --artifacts"
        for policy in ("read", "pipelines-write"):
            doc = yaml.safe_load((SEED / project / f"{project}-od-policy-{policy}.yaml").read_text())
            assert {e["type"] for e in doc["spec"]["information"][0]["entities"]} == written


def written_attributes(bento: Path) -> set[str]:
    """The Attributes the mapping's entity names: the top-level keys of its last `root = {...}`."""
    text = bento.read_text()
    block = text[text.rindex("root = {\n"):]
    block = block[:block.index("}.filter(")]
    indent = re.match(r'root = \{\n(\s+)"', block).group(1)
    return set(re.findall(rf'^{indent}"(\w+)":', block, re.MULTILINE)) - {"id", "type"}


def class_slots(classes: dict, name: str) -> set[str]:
    """What the Portal's view lists for a type shown whole (reconciler/views.rs class_slots): the
    class's slots and attributes through is_a and mixins, and the core Entity's `location`."""
    cls = classes.get(name) or {}
    slots = set(cls.get("slots") or []) | set(cls.get("attributes") or {}) | {"location"}
    for parent in [cls.get("is_a"), *(cls.get("mixins") or [])]:
        if parent:
            slots |= class_slots(classes, parent)
    return slots


def test_every_mapping_writes_only_attributes_its_view_shows():
    # Antares refuses a whole write naming one Attribute the Endpoint's view does not show (ADR-0023
    # rule 7), so 516 rows of a census table failed at once (T-5718). The view lists the space model's
    # class slots for every type the pipelines Policy grants whole.
    for project in generate.PROJECTS:
        classes = yaml.safe_load((SEED / project / f"{project}-otvorene-data.linkml.yaml").read_text())["classes"]
        policy = yaml.safe_load((SEED / project / f"{project}-od-policy-pipelines-write.yaml").read_text())
        for info in policy["spec"]["information"]:
            assert not info.get("propertyNames") and not info.get("relationshipNames"), "a narrowed grant: check it here"
        for pipeline in files(project, "pipeline"):
            kind = yaml.safe_load(pipeline.read_text())["spec"]["output"]["type"]
            bento = pipeline.with_name(pipeline.stem + "-bento.yaml")
            hidden = written_attributes(bento) - class_slots(classes, kind)
            assert not hidden, f"{bento.name} writes {sorted(hidden)}, which the {kind} view does not show"


def test_an_attribute_outside_the_class_is_one_the_view_does_not_show(tmp_path):
    bento = tmp_path / "x-bento.yaml"
    bento.write_text('root = {\n  "id": "x",\n  "type": "T",\n  "a": 1,\n  "b": { "c": 2 },\n}.filter(e -> e)\n')
    assert written_attributes(bento) == {"a", "b"}
    classes = {"T": {"is_a": "Base", "slots": ["a"]}, "Base": {"mixins": ["M"]}, "M": {"attributes": {"m": {}}}}
    assert class_slots(classes, "T") == {"a", "m", "location"}
    assert written_attributes(bento) - class_slots(classes, "T") == {"b"}


def test_the_index_places_every_generated_file_once():
    for project in generate.PROJECTS:
        index = yaml.safe_load((SEED / project / "index.yaml").read_text())
        generated = {p.name for p in (SEED / project).glob(f"{project}-od-*")} | {
            p.name for p in (SEED / project).glob(f"{project}-otvorene-data.*") if not p.name.endswith((".md", ".example.jsonld"))}
        assert generated <= set(index), sorted(generated - set(index))
        targets = list(index.values())
        assert len(targets) == len(set(targets))


def test_the_new_spaces_are_writable_by_the_pipelines_and_reachable_by_the_public_mcp_server():
    for project, sa, mcp in (("bbsk", "bbsk-serviceaccount-pipelines.yaml", "bbsk-mcp-otvorene-data.yaml"),
                             ("banskabystrica", "serviceaccount-pipelines.yaml", "banskabystrica-mcp-otvorene-data.yaml")):
        space = f"{project}-otvorene-data"
        roles = yaml.safe_load((SEED / project / sa).read_text())["spec"]["roles"]
        assert {"role": "space-writer", "scope": {"contextSpace": space}} in roles
        # T-5367: the connector selects the publisher's public Endpoints instead of naming them.
        select = yaml.safe_load((SEED / project / mcp).read_text())["spec"]["select"]
        assert project in select.get("projects", []) or f"{project}/{space}" in select.get("spaces", [])
        endpoint = yaml.safe_load((SEED / project / f"{project}-od-endpoint.yaml").read_text())["spec"]
        assert endpoint["audience"] == "public" and "mcp" in endpoint["enabledRepresentations"]
        assert re.fullmatch(r"[a-z2-7]{26,}", endpoint["slug"])


def test_every_open_data_endpoint_serves_each_standard_and_the_catalogue_lists_each_converted_dataset():
    """T-4279: one list of representations for both spaces, and the harvests' mapping names every
    converted dataset once, with the Endpoint of its own project."""
    # No `sta` (T-5299): the models declare no unit, so a Datastream would have no unitOfMeasurement.
    assert not {"sta", "xlsx", "parquet", "json", "zip", "geojson"} & set(generate.REPRESENTATIONS)
    slugs = {}
    for project in ("bbsk", "banskabystrica"):
        endpoint = yaml.safe_load((SEED / project / f"{project}-od-endpoint.yaml").read_text())["spec"]
        # A space with Article adds `feed` and its base preset `news` (T-6756).
        news = endpoint.get("representations", {}).get("feed", [])
        assert endpoint["enabledRepresentations"] == generate.REPRESENTATIONS + (["feed"] if news else [])
        assert [o["name"] for o in news] == (["news"] if project == "banskabystrica" else [])
        slugs[project] = endpoint["slug"]
    rows = json.loads(generate.DISTRIBUTIONS.read_text())
    # A dataset no Pipeline fills since T-5729 (source.kind endpoint) lists no distribution (T-5788).
    mapped = [e for e in MAPPING["datasets"] if e["status"] == "mapped" and e["source"]["kind"] != "endpoint"]
    # The susr- rows are scripts/open-data/datacube.py's (T-4390, test_datacube_generator.py).
    assert len([k for k in rows if not k.startswith("susr-")]) == len({e["dataset"] for e in mapped})
    for e in SEEDED:
        if e["source"]["kind"] == "endpoint":
            assert e["dataset"].rsplit("/", 1)[1] not in rows
            continue
        row = rows[e["dataset"].rsplit("/", 1)[1]]
        assert (row["endpoint"], row["type"], row["dataset"], row["filtered"]) == (slugs[e["org"]], e["type"], e["dataset"], True)
    # T-4288: SensorThings would be listed for the datasets whose numbers carry an instant; with no
    # `sta` served (T-5299) the catalogue lists it for none, so no dataset page links a refused door.
    series = {name for name, row in rows.items() if row.get("series")}
    assert series == set(), sorted(series)
    assert any(generate.series(e) for e in SEEDED), "the series rule still finds the dated datasets"
    # T-4291: a dataset another seed reads whole is listed through that seed's public Endpoint, over
    # its whole type, since those rows name no catalogue dataset; unless two datasets share that
    # door and type, which the seed then tells apart by `isPartOf` (bbsk obce and okresy, T-4523).
    shared = {"bbsk-obce-bbk-hranice-ec7ac6", "bbsk-okresy-bbk-hranice-17725f"}
    endpoints = {d["spec"]["slug"]: (f.parent.name, d["spec"]) for f in SEED.glob("*/*.yaml")
                 for d in yaml.safe_load_all(f.read_text()) if isinstance(d, dict) and d.get("kind") == "Endpoint"}
    for e in mapped:
        if (e.get("existing") or {}).get("covers") != "whole":
            continue
        row = rows[e["dataset"].rsplit("/", 1)[1]]
        project, endpoint = endpoints[row["endpoint"]]
        filtered = e["dataset"].rsplit("/", 1)[1] in shared
        assert (project, endpoint["audience"], row["filtered"]) == (e["org"], "public", filtered), e["name"]
        assert {"ngsi-ld", "csv", "ogc-features", "odata"} <= set(endpoint["enabledRepresentations"]), e["name"]
    # T-4540: a dataset whose type holds its rows alone on its door is kept by the type, which the
    # broker counts and the gateway pages in parallel; a `q` timed out on bb-objednavky-bb's 26 862.
    # A withdrawn dataset's old rows can still sit on its door, so it still shares the door (T-5788).
    doors = Counter((r["endpoint"], r["type"]) for r in rows.values())
    doors += Counter((slugs[e["org"]], e["type"]) for e in SEEDED if e["source"]["kind"] == "endpoint")
    for name, row in rows.items():
        if not name.startswith("susr-"):
            assert row["sole"] == (row["filtered"] and doors[(row["endpoint"], row["type"])] == 1), name
    assert rows["bb-aktuality"]["sole"]
    # bb-adresy's Address rows stay on the door its register shares: the register keeps its isPartOf filter.
    assert not rows["bb-dtm-bb-register-adries"]["sole"]
    assert not any(rows[name]["sole"] for name in shared), "obce and okresy share a door and a type"
    mosty = rows["bbsk-mosty-v-banskobystrickom-kraji-public-view-3b4675"]
    assert (mosty["type"], endpoints[mosty["endpoint"]][1]["policyRef"]) == ("Bridge", "urn:ngsi-ld:Policy:bbsk.sk:bbsk-registre:mosty-read")


# --- the generator's own rules ---

def test_names_are_dns_labels_unique_and_short_enough_for_a_principal():
    assert len(set(SLUGS.values())) == len(SLUGS)
    for slug in SLUGS.values():
        # `od-` and the reconciler's `pl-` prefix stay inside a 63-character label.
        assert re.fullmatch(r"[a-z0-9]([a-z0-9-]*[a-z0-9])?", slug) and len(slug) <= 40, slug
    assert generate.slugs([{"name": "bbsk-x-abcdef"}, {"name": "bb-x"}, {"name": "bb-y-1996-2024"}]) == {
        "bbsk-x-abcdef": "x", "bb-x": "x-2", "bb-y-1996-2024": "y"}
    assert not any(re.search(r"-[0-9]{4}$", s) for s in SLUGS.values()), "the janitor would sweep it"


def test_one_range_per_attribute_across_the_model():
    entries = [{"properties": {"rok": {"column": "Rok", "range": "integer"}, "kod": {"column": "Kod", "range": "integer"}}},
               {"properties": {"rok": {"column": "Rok", "range": "float"}, "kod": {"column": "Kod", "range": "string"}}}]
    assert generate.ranges(entries) == {"rok": "float", "kod": "string"}


def test_a_column_named_like_the_attribution_keeps_its_value_under_another_name():
    assert generate.attribute("source") == "columnSource"
    assert generate.attribute("location") == "columnLocation"
    assert generate.attribute("rok") == "rok"


def test_schedules_are_valid_cron_and_follow_the_refresh():
    for project in generate.PROJECTS:
        for p in files(project, "pipeline"):
            spec = yaml.safe_load(p.read_text())["spec"]
            fields = spec["schedule"].split()
            assert len(fields) == 5 and 0 <= int(fields[0]) < 60 and 1 <= int(fields[1]) <= 5, p.name
            # Bento's cron reads a weekday 0-6; a 7 refused the whole stream on dev (T-4347).
            assert fields[4] == "*" or 0 <= int(fields[4]) <= 6, p.name
    daily = next(e for e in SEEDED if e["refresh"] == "P1D")
    assert generate.schedule(daily, 3, 3).endswith("* * *")
    assert generate.schedule({"refresh": "P1M"}, 30, 30).split()[2] == "3"


def test_the_parts_of_a_split_dataset_run_the_same_day_at_their_own_minute():
    # The city's green layers ran as p1 on Sunday, p3 on Friday, p2 on Saturday: a day's gate
    # counted 4000 of 8546 rows (T-4517).
    for project in generate.PROJECTS:
        days: dict[str, set[str]] = {}
        minutes: dict[str, set[str]] = {}
        for p in files(project, "pipeline"):
            if p.name.endswith("-bento.yaml"):
                continue
            doc = yaml.safe_load(p.read_text())
            fields = doc["spec"]["schedule"].split()
            dataset = re.sub(r"-p\d+$", "", doc["metadata"]["name"])
            days.setdefault(dataset, set()).add(" ".join(fields[2:]))
            minutes.setdefault(dataset, set()).add(" ".join(fields[:2]))
        split = [d for d, m in minutes.items() if len(m) > 1]
        assert all(len(days[d]) == 1 for d in split), {d: days[d] for d in split if len(days[d]) > 1}
    assert generate.schedule({"refresh": "P1W"}, 9, 2).split()[2:] == ["*", "*", "2"]


# --- Security: public sources only, no personal column, the row filters at the source ---

def test_every_source_is_public_https_without_a_credential():
    for project in generate.PROJECTS:
        for p in files(project, "datasource"):
            spec = yaml.safe_load(p.read_text())["spec"]
            url = urllib.parse.urlsplit(spec["http"]["url"])
            # The T-MAPY host is where the city's DTM_BB archives are published (T-4317).
            assert url.scheme == "https" and url.hostname in {"services-eu1.arcgis.com", "opendata.dev.joinedcontext.com",
                                                               "www.banskabystrica.sk", "dtm-banskabystrica.tmapserver.sk"}, p.name
            assert set(spec["http"]) == {"url", "verb", "headers", "timeout"} and "Authorization" not in spec["http"]["headers"]


def test_no_dropped_column_is_asked_for_or_read():
    dropped = [e for e in SEEDED if e.get("dropped")]
    assert dropped, "the mapping drops personal columns; this test must see them"
    for e in dropped:
        slug, project = SLUGS[e["name"]], e["org"]
        url = yaml.safe_load((SEED / project / f"{project}-od-datasource-{slug}.yaml").read_text())["spec"]["http"]["url"]
        # An archive is read whole, so only the mapping keeps a dropped column out (T-4317).
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)
        out_fields = query["outFields"][0].split(",") if e["source"]["kind"] == "esri-featureserver" else []
        bento = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        for column in e["dropped"]["columns"]:
            assert column not in out_fields, (e["name"], column)
            assert json.dumps(column, ensure_ascii=False) not in bento, (e["name"], column)


def test_every_request_fits_the_url_length_arcgis_answers():
    """ArcGIS Online answers a query URL past about 2 kB with an IIS 404: the BBSK religion layers
    asked for 50 columns by name and failed live (T-4255). Such a layer asks for `*`."""
    starred = 0
    for e in SEEDED:
        if e["source"]["kind"] == "endpoint":
            continue
        slug, project = SLUGS[e["name"]], e["org"]
        url = yaml.safe_load((SEED / project / f"{project}-od-datasource-{slug}.yaml").read_text())["spec"]["http"]["url"]
        bento = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        for u in [url, *re.findall(r'url: "([^"]+)"', bento)]:
            assert len(u) <= 2000, (e["name"], len(u))
            if urllib.parse.parse_qs(urllib.parse.urlsplit(u).query).get("outFields") == ["*"]:
                assert not e.get("dropped"), e["name"]
                starred += 1
    assert starred, "no layer asks for * any more: drop the fallback in generate.py and this test"


def test_a_row_filter_travels_with_every_request_of_its_layer():
    filtered = [e for e in SEEDED if (e.get("rowFilter") or {}).get("where")]
    assert filtered, "the mapping filters rows; this test must see them"
    for e in filtered:
        slug, project = SLUGS[e["name"]], e["org"]
        url = yaml.safe_load((SEED / project / f"{project}-od-datasource-{slug}.yaml").read_text())["spec"]["http"]["url"]
        assert urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)["where"] == [e["rowFilter"]["where"]]
        bento = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        for page_url in re.findall(r'url: "([^"]+)"', bento):
            assert urllib.parse.parse_qs(urllib.parse.urlsplit(page_url).query)["where"] == [e["rowFilter"]["where"]]


# --- Smart Data Models (T-4319): every type is a published model, every entity valid in it ---

def test_every_mapped_type_is_one_vendored_smart_data_model_at_a_pinned_commit():
    assert len(MODEL) == len({(e["type"], e["smartDataModel"]) for e in MAPPED}), "one model per type"
    for e in MAPPED:
        assert generate.sdm_schema(e).is_file()
        official = e.get("baseModel") or e["smartDataModel"]
        assert re.fullmatch(r"[0-9a-f]{40}", PINS[official.split("/")[0]]), official
    assert re.fullmatch(r"[0-9a-f]{40}", PINS["data-models"])
    vendored = {str(p.parent.relative_to(generate.SDM)) for p in generate.SDM.glob("*/*/schema.json")
                if p.parent.parent != generate.EXTENSIONS}
    in_use = {(e.get("baseModel") or e["smartDataModel"]).removeprefix("dataModel.") for e in MAPPED}
    # The folder also serves the city seeds' models (T-4329: Helsinki, Praha), so it holds at
    # least the models this seed uses, the bases of the extensions included, each subject pinned.
    assert vendored >= in_use, "vendor every model in use, the bases of the extensions included"
    assert {f"dataModel.{v.split('/')[0]}" for v in vendored} | {"about", "data-models"} == set(PINS), "a pin per vendored subject"


def test_a_mapped_dataset_without_a_model_or_with_one_not_vendored_is_refused():
    for model in (None, "", "dataModel.Finance/Invoice", "Organization/Organization"):
        with pytest.raises(SystemExit, match="mapped without a vendored Smart Data Model"):
            generate.sdm_schema({"name": "bbsk-x", "smartDataModel": model})
    # An extension names a declared, vendored base and its own type (T-4336).
    for bad in ({"baseModel": None}, {"baseModel": "dataModel.Finance/Ledger"}, {"type": "Other"}):
        with pytest.raises(SystemExit, match="extends no vendored base"):
            generate.sdm_schema({"name": "bbsk-x", "type": "Invoice", "smartDataModel": "extension/Invoice",
                                 "baseModel": "dataModel.CPSV-AP/Cost", **bad})
    assert not [e for e in MAPPING["datasets"] if e["status"] == "removed" and SLUGS.get(e["name"])]
    removed = {e["name"] for e in MAPPING["datasets"] if e["status"] == "removed"}
    for project in generate.PROJECTS:
        for p in files(project, "datasource"):
            assert yaml.safe_load(p.read_text())["metadata"]["title"]["en"] not in {
                e["title"][:120] for e in MAPPING["datasets"] if e["name"] in removed and e["org"] == project}, p.name


def expanded(context: dict, term: str) -> str:
    """The IRI the generated @context gives `term`, its CURIE resolved by the context's own prefixes."""
    iri = context[term]["@id"] if isinstance(context[term], dict) else context[term]
    prefix, _, rest = iri.partition(":")
    return context[prefix] + rest if isinstance(context.get(prefix), str) and not rest.startswith("//") else iri


def test_every_term_a_model_names_expands_to_its_vendored_context_and_a_source_column_stays_jc():
    """T-4328: a client expanding with the Smart Data Model's @context reads the IRIs the broker holds."""
    for project in generate.PROJECTS:
        context = json.loads((SEED / project / f"{project}-otvorene-data.v1.context.jsonld").read_text())["@context"]
        named = 0
        for e in [e for e in SEEDED if e["org"] == project]:
            model = (e.get("baseModel") or e["smartDataModel"]).split("/")[0]
            sdm = json.loads((generate.SDM / model.removeprefix("dataModel.") / "context.jsonld").read_text())["@context"]
            # An extension's type is ours (T-4336); every other term reads as its base names it.
            want = f"https://joinedcontext.com/models/{e['type']}" if e.get("baseModel") else sdm[e["type"]]
            assert expanded(context, e["type"]) == want, e["name"]
            for a in [*generate.COMMON, *map(generate.attribute, e["properties"]), *(e.get("constants") or {})]:
                if isinstance(sdm.get(a), str):
                    assert expanded(context, a) == sdm[a], (e["name"], a)
                    named += 1
                else:
                    assert expanded(context, a) == f"https://joinedcontext.com/models/{a}", (e["name"], a)
        assert named, project
    # A source's own column (BBSK's school statistics) is the organisation's term.
    assert expanded(json.loads((SEED / "bbsk/bbsk-otvorene-data.v1.context.jsonld").read_text())["@context"],
                    "absolventiZaMinulyRokSpol") == "https://joinedcontext.com/models/absolventiZaMinulyRokSpol"


def test_every_context_is_vendored_beside_its_schema_at_the_same_pin():
    for model in {(BASE[t] or m).split("/")[0] for t, m in MODEL.items()}:
        assert (generate.SDM / model.removeprefix("dataModel.") / "context.jsonld").is_file(), model
        assert model in PINS


def test_two_models_naming_one_term_differently_are_refused():
    a = {"name": "x", "type": "PointOfInterest", "smartDataModel": "dataModel.PointOfInterest/PointOfInterest",
         "properties": {"category": {}}}
    b = {**a, "name": "y", "type": "KeyPerformanceIndicator",
         "smartDataModel": "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator"}
    with pytest.raises(SystemExit, match="`category`"):
        generate.sdm_iris([a, b])


SAMPLE = {"string": "x", "integer": 1, "float": 1.5, "datetime": "2026-01-01T00:00:00Z", "date": "2026-01-01",
          "uri": "https://example.org/a", "string-list": ["x"]}


def test_every_mapping_writes_an_entity_its_model_accepts():
    """Without Bento: the entity each mapping writes when every column holds a value of its range."""
    # The range the Bento writes is the space's: integer and text columns of one name meet as text.
    space = {project: generate.ranges([e for e in SEEDED if e["org"] == project]) for project in generate.PROJECTS}
    for e in MAPPED:
        written = space[e["org"]] if e in SEEDED else {generate.attribute(a): p["range"] for a, p in e["properties"].items()}
        entity = {"id": f"urn:ngsi-ld:{e['type']}:bbsk.sk:s:x-1", "type": e["type"],
                  **{generate.attribute(a): {"type": "Property", "value": SAMPLE[written[generate.attribute(a)]]} for a in e["properties"]},
                  **{a: {"type": "Property", "value": v} for a, v in (e.get("constants") or {}).items()},
                  **{a: {"type": "Property", "value": "https://example.org/s"} for a in ("source", "isPartOf")},
                  "dataProvider": {"type": "Property", "value": "x (CC BY 4.0)"},
                  # The entity's own key where the base requires an identifier no column holds (T-4336).
                  **({"identifier": {"type": "Property", "value": "x-1"}} if e.get("identifierFromId") else {})}
        if e["source"].get("geometry"):
            entity["location"] = {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [19.1, 48.7]}}
        assert sdm_errors(entity) == [], e["name"]
    # A list attribute that came through as text is what the model refuses.
    assert sdm_errors({"id": "urn:ngsi-ld:Address:bb:x", "type": "Address", "thoroughfare": {"type": "Property", "value": "x"}})


# --- the mappings over recorded answers ---

def valid(project: str, entities: list[dict]) -> bool:
    for e in entities:
        assert schema_errors(project, e) == [], e["id"]
        assert sdm_errors(e) == [], e["id"]
    return True


@requires_docker
def test_a_layer_with_a_key_writes_one_point_per_row_under_its_key():
    entities = run("bbsk", "zoznam-lekarni-v-bbk", "lekarne")
    rows = json.loads((FIXTURES / "lekarne.json").read_text())["features"]
    assert [e["id"] for e in entities] == [
        f"urn:ngsi-ld:PointOfInterest:bbsk.sk:bbsk-otvorene-data:zoznam-lekarni-v-bbk-{r['properties']['IdZZ']}" for r in rows]
    for e, r in zip(entities, rows):
        assert e["location"] == {"type": "GeoProperty", "value": r["geometry"]}
        assert e["name"]["value"] == r["properties"]["Názov"].strip()
        assert e["isPartOf"]["value"] == entry("bbsk-zoznam-lekarni-v-bbk-c564de")["dataset"]
        assert "dataset" not in e, "the core @context owns `dataset` (T-4289)"
        assert "CC BY-SA 4.0" in e["dataProvider"]["value"]
    assert valid("bbsk", entities)


@requires_docker
def test_a_key_with_its_year_and_a_polygon():
    entities = run("bbsk", "pohyb-obyvatelstva-v-okresoch", "pohyb-okresy")
    rows = json.loads((FIXTURES / "pohyb-okresy.json").read_text())["features"]
    assert len(entities) == len(rows) == 2
    for e, r in zip(entities, rows):
        assert e["id"].startswith("urn:ngsi-ld:KeyPerformanceIndicator:")
        assert e["id"].endswith(f"-{r['properties']['IDN3']}-{r['properties']['Rok']}")
        assert e["location"]["value"]["type"] == "Polygon"
        # Every number is stamped with its year, so SensorThings charts it; the year itself is not (T-4288).
        numbers = {a: p for a, p in e.items() if isinstance(p, dict) and isinstance(p.get("value"), (int, float))}
        stamped = {a for a, p in numbers.items() if p.get("observedAt") == f"{r['properties']['Rok']}-01-01T00:00:00Z"}
        assert numbers and stamped == set(numbers) - {"rok"}, sorted(set(numbers) - stamped)
        assert "observedAt" not in e.get("rok", {})
    assert valid("bbsk", entities)


@requires_docker
def test_a_page_of_large_rows_reaches_the_broker_in_requests_under_its_body_limit():
    # Dev and prod, 2026-10-19: od-vekova-struktura-v-okresoch, od-pohyb-obyvatelstva-v-obciach and
    # od-vyvoj-emisii-v-bbk wrote nothing, "the upsert answers an unexpected status" (T-5309): a
    # 1000-row page of ~8 KB polygons went as one ~8 MB request, past Antares' 4 MiB body limit
    # (ANTARES_MAX_BODY_BYTES). The mapping splits a page into requests under generate.REQUEST_BYTES.
    page = json.loads((FIXTURES / "pohyb-okresy.json").read_text())
    row = page["features"][0]
    page["features"] = [{**row, "properties": {**row["properties"], "IDN3": i}} for i in range(1000)]
    mapping = SEED / "bbsk" / "bbsk-od-pipeline-pohyb-obyvatelstva-v-okresoch-bento.yaml"
    out = open_data._bento(mapping, json.dumps(page).encode(), "bbsk-otvorene-data", "bbsk.sk", "lines")
    requests = [line for line in out.decode().splitlines() if line.strip()]
    assert len(requests) > 1
    assert all(len(r.encode()) <= generate.REQUEST_BYTES for r in requests), [len(r) for r in requests]
    assert generate.REQUEST_BYTES * 2 <= 4 * 1024 * 1024, "a request needs room under the broker's limit"
    assert sorted(e["id"] for r in requests for e in json.loads(r)) == sorted(
        f"urn:ngsi-ld:KeyPerformanceIndicator:bbsk.sk:bbsk-otvorene-data:pohyb-obyvatelstva-v-okresoch-{i}-{row['properties']['Rok']}"
        for i in range(1000))


@requires_docker
def test_a_corrected_accident_keeps_its_id_and_the_constant_its_model_requires():
    first = run("bbsk", "nehodovost-v-bbk", "nehodovost-v-bbk")
    rows = json.loads((FIXTURES / "nehodovost-v-bbk.json").read_text())["features"]
    assert [e["id"] for e in first] == [
        f"urn:ngsi-ld:RoadAccident:bbsk.sk:bbsk-otvorene-data:nehodovost-v-bbk-{r['properties']['Číslo_nehody']}"
        for r in rows]
    assert all(e["status"]["value"] == "archived" for e in first)
    assert [e["name"]["value"] for e in first] == [f"Nehoda {r['properties']['Číslo_nehody']}" for r in rows]
    # An Esri date is the instant every number of the accident was observed at (T-4288).
    for e, r in zip(first, rows):
        at = datetime.fromtimestamp(r["properties"]["Dátum_a_čas"] / 1000, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        assert {p.get("observedAt") for p in e.values() if isinstance(p, dict) and isinstance(p.get("value"), (int, float))} == {at}
    assert valid("bbsk", first)
    # A corrected value updates the accident instead of adding a second one (T-4332).
    doc = json.loads((FIXTURES / "nehodovost-v-bbk.json").read_text())
    doc["features"][0]["properties"]["Škoda_v_EUR"] = 1.5
    changed = run_bytes("bbsk", "nehodovost-v-bbk", json.dumps(doc).encode())
    assert [e["id"] for e in changed] == [e["id"] for e in first]


@requires_docker
def test_a_table_without_a_key_takes_a_stable_row_hash():
    first = run("bbsk", "dotacie-2", "dotacie-2")
    again = run("bbsk", "dotacie-2", "dotacie-2")
    assert [e["id"] for e in first] == [e["id"] for e in again]
    assert len({e["id"] for e in first}) == 3
    # One changed value is another row, so another id.
    doc = json.loads((FIXTURES / "dotacie-2.json").read_text())
    doc["features"][0]["properties"]["Žiadateľ"] = "Iný žiadateľ"
    changed = run_bytes("bbsk", "dotacie-2", json.dumps(doc).encode())
    assert changed[0]["id"] != first[0]["id"] and changed[1]["id"] == first[1]["id"]
    assert all(e["type"] == "Grant" for e in first) and valid("bbsk", first)


@requires_docker
def test_a_list_attribute_is_a_one_item_list_and_an_empty_name_reads_the_next_column():
    doc = json.loads((FIXTURES / "nabijacky-a-zastavky.json").read_text())
    doc["features"][0]["properties"]["PRIM_NAME"] = None
    entities = run_bytes("bbsk", "nabijacky-a-zastavky", json.dumps(doc).encode())
    rows = [f["properties"] for f in doc["features"]]
    assert len(entities) == len(rows) == 3
    assert entities[0]["name"]["value"] == rows[0]["Categ"]
    assert [e["name"]["value"] for e in entities[1:]] == [r["PRIM_NAME"] for r in rows[1:]]
    assert entities[0]["category"]["value"] == [str(rows[0]["category"])]
    assert valid("bbsk", entities)
    spatial = run("bbsk", "prispevkove-rozpoctove-a-ine-organizacie", "prispevkove-rozpoctove-a-ine-organizacie")
    assert [e["spatial"]["value"] for e in spatial] == [[r["properties"]["Obec"]] for r in json.loads(
        (FIXTURES / "prispevkove-rozpoctove-a-ine-organizacie.json").read_text())["features"]]
    assert valid("bbsk", spatial)


@requires_docker
def test_a_name_where_the_ico_belongs_keeps_the_row_out():
    """The BBSK IČO columns also hold surnames padded to eight characters (`0000BIBA`), which name
    natural persons and pass the layer's `IS NOT NULL` filter (T-4255): the mapping drops them."""
    doc = json.loads((FIXTURES / "dodavatelia.json").read_text())
    kept = [f["properties"]["ICO"] for f in doc["features"][1:]]
    doc["features"][0]["properties"]["ICO"] = "0000BIBA"
    entities = run_bytes("bbsk", "dodavatelia", json.dumps(doc).encode())
    assert sorted(e["iCO"]["value"] for e in entities) == sorted(v.strip() for v in kept)
    assert valid("bbsk", entities)
    doc["features"][1]["properties"]["ICO"] = "00177431 0006"
    assert len(run_bytes("bbsk", "dodavatelia", json.dumps(doc).encode())) == 2


@requires_docker
def test_every_article_has_a_name_from_its_title_or_else_its_slug():
    """WordPress renders a title as HTML, so "&#8222;" reads as the quote mark; a post with no title
    is named by its slug rather than left without a name (T-4569)."""
    posts = json.loads((FIXTURES / "aktuality.json").read_text())
    entities = run("banskabystrica", "aktuality", "aktuality")
    assert [e["identifier"]["value"] for e in entities] == [str(p["id"]) for p in posts]
    titled, untitled = entities
    assert titled["name"]["value"] == "Tretí ročník Svetla v meste poodhalí „druhý plán“ historického centra"
    assert untitled["name"]["value"] == posts[1]["slug"].replace("-", " ")
    assert [e["url"]["value"] for e in entities] == [p["link"] for p in posts]
    assert all("<" not in e["content"]["value"] and "&#" not in e["content"]["value"] for e in entities)
    assert valid("banskabystrica", entities)



@requires_docker
def test_a_feed_of_more_posts_than_a_page_is_read_page_by_page():
    """WordPress answers at most 100 posts a request, and the run replaces the whole set: one page
    a night deleted every article past the newest 100 of 1558 (F-BNT1-011)."""
    post = json.loads((FIXTURES / "aktuality.json").read_text())[0]
    posts = [{**post, "id": 1000 + i, "slug": f"post-{i}"} for i in range(250)]
    asked: list[dict] = []

    def answer(query: dict) -> tuple[bytes | None, dict]:
        asked.append(query)
        if query["per_page"] == ["1"]:
            return json.dumps([{"id": 1}]).encode(), {"X-WP-Total": "250", "X-WP-TotalPages": "250"}
        page = int(query["page"][0])
        return json.dumps(posts[(page - 1) * 100:page * 100]).encode(), {}

    first = json.dumps(posts[:100]).encode()
    out = written(stubbed("banskabystrica", "aktuality", first, "https://www.banskabystrica.sk", answer))
    assert sorted(e["identifier"]["value"] for e in out) == sorted(str(p["id"]) for p in posts)
    assert sorted(q.get("page", ["count"])[0] for q in asked) == ["2", "3", "count"]

    def no_total(query: dict) -> tuple[bytes | None, dict]:
        return json.dumps([{"id": 1}]).encode(), {}
    failed = stubbed("banskabystrica", "aktuality", first, "https://www.banskabystrica.sk", no_total)
    assert b"the feed did not say how many posts it holds" in failed.stdout + failed.stderr

def test_every_built_name_reads_columns_its_dataset_has():
    """A NAMES template names a mapped dataset without a name column, and each `{column}` is one its rows
    carry (T-4745); a typo would name every row by the template's text alone."""
    by_name = {e["name"]: e for e in SEEDED}
    for dataset, template in generate.NAMES.items():
        e = by_name[dataset]
        assert "name" not in {a for a, p in e["properties"].items() if "template" not in p}, dataset
        known = {c for p in e["properties"].values() for c in [p["column"], *p.get("otherwise", [])]}
        known |= set(e["entityId"].get("keys") or []) | {"vrstva", "vrstva_id"}
        assert set(re.findall(r"\{([^}]+)\}", template)) <= known, dataset


@requires_docker

def offline(project: str, slug: str, document: bytes, pages: dict[int, bytes | None],
            count: int | None = None) -> subprocess.CompletedProcess:
    """The mapping over `document`, its further pages answered on this machine by `resultOffset`:
    `pages[offset]` is the answer, None a 500, and an offset it does not name an empty page. The
    layer's row count answers `count`, a 500 when None."""
    def answer(query: dict) -> tuple[bytes | None, dict]:
        if query.get("returnCountOnly") == ["true"]:
            return (None if count is None else json.dumps({"count": count}).encode()), {}
        return pages.get(int(query["resultOffset"][0]), b'{"type": "FeatureCollection", "features": []}'), {}
    return stubbed(project, slug, document, "https://services-eu1.arcgis.com", answer)


def stubbed(project: str, slug: str, document: bytes, origin: str,
            answer: Callable[[dict], tuple[bytes | None, dict]]) -> subprocess.CompletedProcess:
    """The mapping over `document`, every request it makes to `origin` answered on this machine by
    `answer(query)`: a body (None is a 500) and its headers."""
    class Source(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body, headers = answer(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query))
            self.send_response(500 if body is None else 200)
            for k, v in headers.items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body or b"")

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Source)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        text = text.replace(origin, f"http://127.0.0.1:{server.server_port}")
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": yaml.safe_load(text)["pipeline"],
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "error"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=bbsk.sk",
             "-e", f"JC_SPACE={project}-otvorene-data", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             open_data.BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=document, capture_output=True, timeout=180,
        )
    finally:
        server.shutdown()


def written(result: subprocess.CompletedProcess) -> list[dict]:
    assert result.returncode == 0, result.stderr.decode()
    out = []
    for line in result.stdout.decode().splitlines():
        if line.strip():
            message = json.loads(line)
            out.extend(message if isinstance(message, list) else [message])
    return out


def supplier_pages(rows: int) -> list[dict]:
    """`rows` suppliers of the dodavatelia layer, each its own IČO, as the pages ArcGIS answers:
    every page but the last says the layer holds more."""
    page0 = json.loads((FIXTURES / "dodavatelia.json").read_text())
    row = page0["features"][0]
    features = [{**row, "properties": {**row["properties"], "ICO": f"{50000000 + i}"}} for i in range(rows)]
    chunks = [features[o:o + generate.PAGE] for o in range(0, rows, generate.PAGE)]
    return [{**page0, "features": c, **({"properties": {"exceededTransferLimit": True}} if i < len(chunks) - 1 else {})}
            for i, c in enumerate(chunks)]


def as_pages(pages: list[dict]) -> dict[int, bytes]:
    return {i * generate.PAGE: json.dumps(p).encode() for i, p in enumerate(pages) if i}


@requires_docker
def test_a_layer_over_one_page_is_read_page_by_page_until_its_count_with_its_filter():
    pages = supplier_pages(2 * generate.PAGE + 500)
    entities = written(offline("bbsk", "dodavatelia", json.dumps(pages[0]).encode(), as_pages(pages), count=2500))
    assert len({e["id"] for e in entities}) == 2500, "page 0, page 1000 and the 500 rows of page 2000"
    assert valid("bbsk", entities[:3])
    # Without the flag the layer said it was whole: neither its count nor a further page is asked for.
    page0 = json.loads((FIXTURES / "dodavatelia.json").read_text())
    assert len(written(offline("bbsk", "dodavatelia", json.dumps(page0).encode(), {1000: None}, count=None))) == 3


@requires_docker
def test_a_layer_of_any_size_is_read_whole_and_one_short_of_its_pages_fails_the_run():
    # The runner holds one page at a time (jc-ekiben-paging), so no row count caps a run: a layer
    # of ten times the rows it held when mapped is read to its last row, page by page.
    e = next(x for x in SEEDED if x["name"] == "bbsk-dodavatelia-825cd0")
    rows = max(10 * e["source"]["rows"], 12 * generate.PAGE) + 7
    big = supplier_pages(rows)
    entities = written(offline("bbsk", "dodavatelia", json.dumps(big[0]).encode(), as_pages(big), count=rows))
    assert len({x["id"] for x in entities}) == rows
    pages = supplier_pages(2 * generate.PAGE + 500)
    first = json.dumps(pages[0]).encode()
    # A server answering fewer rows a page than asked would skip the rest of each page.
    short = offline("bbsk", "dodavatelia", first, {**as_pages(pages), 1000: json.dumps(pages[2]).encode()}, count=2500)
    assert b"the page at 1000 answered 500 rows where 1000 were due" in short.stdout + short.stderr
    small = offline("bbsk", "dodavatelia", json.dumps({**pages[2], "properties": {"exceededTransferLimit": True}}).encode(), {}, count=2500)
    assert b"the layer answered 500 rows where 1000 were asked" in small.stdout + small.stderr
    uncounted = offline("bbsk", "dodavatelia", first, as_pages(pages), count=None)
    assert b"the layer did not say how many rows it holds" in uncounted.stdout + uncounted.stderr


def test_every_esri_layer_counts_its_rows_with_its_filter_and_reads_them_a_page_at_a_time():
    layers = [x for x in SEEDED if x["source"]["kind"] == "esri-featureserver"]
    assert layers
    for e in layers:
        slug, project = SLUGS[e["name"]], e["org"]
        bento = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        assert "range(1, 5)" not in bento and "returnCountOnly=true" in bento, e["name"]
        # No row cap (jc-ekiben-paging): each listed page is its own batch, fetched, mapped and
        # written before the next, so the runner holds a page whatever the layer's size.
        assert "one run reads" not in bento, e["name"]
        steps = yaml.safe_load(bento)["pipeline"]["processors"]
        at = next(i for i, p in enumerate(steps) if "switch" in p and "offset" in json.dumps(p))
        assert steps[at - 2] == {"unarchive": {"format": "json_array"}}, e["name"]
        assert steps[at - 1] == {"split": {"size": 1}}, e["name"]



def test_every_esri_page_is_asked_in_object_id_order():
    """resultOffset pages without an order are any slice the server likes: a row can come twice or
    never, and a row never read is deleted by the replace-mode run (F-BNT1-012)."""
    layers = [x for x in SEEDED if x["source"]["kind"] == "esri-featureserver" and x["source"].get("objectIdField")]
    assert len(layers) > 80
    for e in layers:
        slug, project = SLUGS[e["name"]], e["org"]
        bento = (SEED / project / f"{project}-od-pipeline-{slug}-bento.yaml").read_text()
        page = yaml.safe_load(bento)["pipeline"]["processors"]
        urls = [generate.source_url(e)] + [p["http"]["url"] for step in page if "switch" in step
                                           for case in step["switch"] for p in case["processors"] if "http" in p]
        assert len(urls) == 2, e["name"]
        for u in urls:
            q = urllib.parse.parse_qs(urllib.parse.urlsplit(u).query)
            assert q["orderByFields"] == [e["source"]["objectIdField"]], (e["name"], u)

@requires_docker
def test_an_empty_layer_writes_nothing_and_a_failure_fails_the_run_where_it_shows():
    assert written(offline("bbsk", "dodavatelia", b'{"type": "FeatureCollection", "features": []}', {})) == []
    error = offline("bbsk", "dodavatelia", b'{"error": {"code": 400, "message": "Invalid query"}}', {})
    assert b"the layer answered no features: Invalid query" in error.stdout + error.stderr
    pages = supplier_pages(2 * generate.PAGE + 500)
    missing = offline("bbsk", "dodavatelia", json.dumps(pages[0]).encode(), {**as_pages(pages), 2000: None}, count=2500)
    assert b"the page at 2000 did not come" in missing.stdout + missing.stderr
    refused = offline("bbsk", "dodavatelia", json.dumps(pages[0]).encode(),
                      {**as_pages(pages), 1000: b'{"error": {"code": 498, "message": "Invalid token"}}'}, count=2500)
    assert b"the page at 1000 answered no features: Invalid token" in refused.stdout + refused.stderr


# --- S-JTSK archives (T-4317): the ZIP read whole, its GeoJSON member reprojected to WGS 84 ---

# Control points in S-JTSK (EPSG:5514) and what PROJ 9.5 (GDAL 3.10.0) answers for them with the same
# Helmert shift: `cs2cs +proj=krovak +lat_0=49.5 +lon_0=24.83333333333333 +alpha=30.28813972222222
# +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +towgs84=485.021,169.465,483.839,7.786342,4.397554,4.102655,0
# +units=m +to +proj=longlat +datum=WGS84`. The first three are vertices of the city's own archives.
CONTROL = [
    ((-417349.9072, -1227405.1559), (19.150454056, 48.745789330)),
    ((-420281.0068, -1222336.9984), (19.105554825, 48.789279679)),
    ((-415867.542, -1226405.1929), (19.169552258, 48.755744569)),
    ((-460000.0, -1250000.0), (18.597394267, 48.513365548)),
    ((-380000.0, -1200000.0), (19.631909576, 49.015474718)),
]


def sjtsk_archive(features: list[dict], extra: dict[str, bytes] | None = None) -> bytes:
    import io
    import zipfile
    doc = {"type": "FeatureCollection", "crs": {"type": "name", "properties": {"name": "urn:ogc:def:crs:EPSG::5514"}},
           "features": features}
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in (extra or {}).items():
            z.writestr(name, data)
        z.writestr("OE_01_register_adries.geojson", json.dumps(doc, ensure_ascii=False))
    return buf.getvalue()


def near(got: list, want: tuple) -> bool:
    # 2e-7 degrees is under 2 cm: the mapping keeps 7 decimals.
    return abs(got[0] - want[0]) < 2e-7 and abs(got[1] - want[1]) < 2e-7


@requires_docker
def test_an_sjtsk_address_lands_where_proj_puts_it_and_keeps_no_sjtsk_column():
    feats = [{"type": "Feature", "geometry": {"type": "Point", "coordinates": list(src)},
              "properties": {"ID": 10170905.0 + i, "ULICA": "Mikuláša Kováča", "OBEC": "Banská Bystrica",
                             "POINT_X": src[0], "POINT_Y": src[1]}} for i, (src, _) in enumerate(CONTROL)]
    entities = run_bytes("banskabystrica", "dtm-bb-register-adries",
                         sjtsk_archive(feats, {"legenda.pdf": b"%PDF-1.4 not data"}))
    assert len(entities) == len(CONTROL), "one address per feature, the PDF beside it read as nothing"
    for e, (_, want) in zip(entities, CONTROL):
        assert e["location"]["value"]["type"] == "Point"
        assert near(e["location"]["value"]["coordinates"], want), e["location"]
        assert e["thoroughfare"]["value"] == ["Mikuláša Kováča"], "CPSV-AP Address (T-4319)"
        assert "pOINTX" not in e and "pOINTY" not in e
        assert schema_errors("banskabystrica", e) == []
    assert len({e["id"] for e in entities}) == len(CONTROL)


@requires_docker
def test_a_line_and_a_polygon_reproject_every_vertex():
    # The street and building registers have no model (T-4319); the address pipeline reprojects
    # whatever geometry an archive's feature has, so it carries the case.
    (a, pa), (b, pb), (c, pc) = CONTROL[:3]
    street = {"type": "Feature", "geometry": {"type": "MultiLineString", "coordinates": [[list(a), list(b)]]},
              "properties": {"ID": 1, "ULICA": "Uľanská cesta"}}
    ring = [list(a), list(b), list(c), list(a)]
    house = {"type": "Feature", "geometry": {"type": "MultiPolygon", "coordinates": [[ring]]}, "properties": {"ID": 2}}
    out = run_bytes("banskabystrica", "dtm-bb-register-adries", sjtsk_archive([street, house]))
    line, polygon = sorted(out, key=lambda e: e["location"]["value"]["type"])
    assert [[near(p, w) for p, w in zip(x, (pa, pb))] for x in line["location"]["value"]["coordinates"]] == [[True, True]]
    got = polygon["location"]["value"]["coordinates"][0][0]
    assert [near(p, w) for p, w in zip(got, (pa, pb, pc, pa))] == [True] * 4
    assert {e["id"].rsplit("-", 1)[1] for e in out} == {"1", "2"}, [e["id"] for e in out]


@requires_docker
def test_an_archive_without_geojson_writes_nothing():
    import io
    import zipfile
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("mapa.pdf", b"%PDF-1.4")
    assert run_bytes("banskabystrica", "dtm-bb-register-adries", buf.getvalue()) == []


@requires_docker
def test_an_archive_past_four_times_its_measured_size_writes_nothing():
    """T-5701: the size gate refuses the zip before it is unpacked; Bento cannot cap the inflated size."""
    import os
    feats = [{"type": "Feature", "geometry": {"type": "Point", "coordinates": list(CONTROL[0][0])},
              "properties": {"ID": 10170905.0, "ULICA": "Mikuláša Kováča", "OBEC": "Banská Bystrica"}}]
    cap = 4 * entry("bb-dtm-bb-register-adries")["source"]["bytes"]
    assert len(run_bytes("banskabystrica", "dtm-bb-register-adries", sjtsk_archive(feats))) == 1
    padded = sjtsk_archive(feats, {"padding.bin": os.urandom(cap)})
    assert len(padded) > cap
    assert run_bytes("banskabystrica", "dtm-bb-register-adries", padded) == []


def test_an_archive_in_a_crs_no_step_reprojects_stops_the_generator():
    e = {**entry("bb-dtm-bb-register-adries")}
    e["source"] = {**e["source"], "crs": "EPSG:3035"}
    with pytest.raises(SystemExit):
        generate.bento(e, "x", generate.ranges([e]))


# --- the city's ArcGIS layers (T-4317): many layers per dataset, the server answers WGS 84 ---

LAYERED = [e for e in SEEDED if e["source"]["kind"] == "esri-layers"]
PARTS = {e["name"]: generate.parts(e, SLUGS[e["name"]]) for e in LAYERED}
DTM = "https://dtm-banskabystrica.tmapserver.sk/arcgis/rest/services/dtm/DTM_v_1_4_2_3_verejny/MapServer"


def three_layers(rows: tuple[int, int, int]) -> dict:
    """The green areas' entry read from three layers, as a sheet of several layers is."""
    e = entry("bb-dtm-bb-register-verejnej-zelene-1")
    layers = [{"url": f"{DTM}/{n}", "key": f"dtm-{n}", "name": name, "rows": r, "fields": ["typudrzovanezelene"]}
              for n, name, r in zip((57, 23, 26), ("udržiavaná plocha zelene - db", "vodná nádrž - db", "koryto - db"), rows)]
    return {**e, "source": {**e["source"], "layers": layers, "rows": sum(rows)}}


asked: list[tuple[int, int, str]] = []


def layered(text: str, document: bytes, pages: dict[tuple[int, int], bytes | None]) -> subprocess.CompletedProcess:
    """A Bento mapping of an `esri-layers` pipeline over `document`, its further pages answered
    here: `pages[(layer, offset)]` answers, None is a 500, any other page is empty; every request
    is recorded in `asked`."""
    asked.clear()

    class Service(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            url = urllib.parse.urlsplit(self.path)
            layer = int(url.path.rsplit("/", 2)[-2])
            offset = int(urllib.parse.parse_qs(url.query)["resultOffset"][0])
            asked.append((layer, offset, self.headers.get("User-Agent", "")))
            body = pages.get((layer, offset), b'{"type": "FeatureCollection", "features": []}')
            self.send_response(500 if body is None else 200)
            self.end_headers()
            self.wfile.write(body or b"")

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Service)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        text = text.replace("https://dtm-banskabystrica.tmapserver.sk", f"http://127.0.0.1:{server.server_port}")
        config = {
            "input": {"stdin": {"scanner": {"to_the_end": {}}}},
            "pipeline": yaml.safe_load(text)["pipeline"],
            "rate_limit_resources": [{"label": "pipeline_egress", "local": {"count": 100, "interval": "1s"}}],
            "output": {"stdout": {"codec": "lines"}},
            "logger": {"level": "error"},
        }
        return subprocess.run(
            ["docker", "run", "--rm", "-i", "--network", "host", "-e", "JC_ORG_DOMAIN=banskabystrica.sk",
             "-e", "JC_SPACE=banskabystrica-otvorene-data", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh",
             open_data.BENTO, "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
            input=document, capture_output=True, timeout=300,
        )
    finally:
        server.shutdown()


def generated(e: dict, part: int = 0) -> str:
    one, slug = generate.parts(e, "dtm-bb-register-verejnej-zelene")[part]
    return generate.bento(one, slug, generate.ranges(SEEDED))


def page(features: list[dict], more: bool = False) -> bytes:
    return json.dumps({"type": "FeatureCollection", "features": features,
                       **({"exceededTransferLimit": True, "properties": {"exceededTransferLimit": True}} if more else {})}).encode()


def test_every_layer_page_asks_wgs84_geojson_of_the_city_host_in_pages_the_server_allows():
    assert sorted(e["name"] for e in LAYERED) == ["bb-dtm-bb-register-statickej-dopravy", "bb-dtm-bb-register-verejnej-zelene-1"]
    for e in LAYERED:
        urls = []
        for part, slug in PARTS[e["name"]]:
            bento = (SEED / e["org"] / f"{e['org']}-od-pipeline-{slug}-bento.yaml").read_text()
            source = yaml.safe_load((SEED / e["org"] / f"{e['org']}-od-datasource-{slug}.yaml").read_text())
            assert source["spec"]["http"]["url"] == generate.source_url(part)
            assert source["spec"]["http"]["headers"]["User-Agent"] == generate.AGENT
            calls = [p["http"] for step in yaml.safe_load(bento)["pipeline"]["processors"] if "switch" in step
                     for case in step["switch"] for p in case["processors"] if "http" in p]
            assert all(c["headers"]["User-Agent"] == generate.AGENT for c in calls)
            urls += [generate.source_url(part), *re.findall(r'"url": "([^"]+)"', bento)]
        assert {u.split("/query?")[0] for u in urls} == {x["url"] for x in e["source"]["layers"]}, e["name"]
        assert len(urls) == len(set(urls)), "no page is read by two pipelines"
        for u in urls:
            q = urllib.parse.parse_qs(urllib.parse.urlsplit(u).query)
            assert u.startswith("https://dtm-banskabystrica.tmapserver.sk/arcgis/rest/services/") and len(u) <= generate.URL_LIMIT
            assert q["outSR"] == ["4326"] and q["f"] == ["geojson"] and int(q["resultRecordCount"][0]) <= 2000
            assert q["outFields"][0].split(",")[0] == "OBJECTID" and q["orderByFields"] == ["OBJECTID"]
        assert not any(c in u.lower() for u in urls for c in ("vkladosoba", "zmenaosoba", "ideditora", "created_user", "last_edited_user"))
    # A sheet with no model names the layers it would read, so it can come back (T-4319).
    removed = next(e for e in MAPPING["datasets"] if e["name"] == "bb-dtm-bb-vyskopis-teran")
    assert removed["status"] == "removed" and f"{DTM}/136" in removed["layers"]


def test_a_large_dataset_is_read_by_several_pipelines_each_under_the_row_bound():
    green = PARTS["bb-dtm-bb-register-verejnej-zelene-1"]  # 8546 rows
    assert [slug for _, slug in green] == ["dtm-bb-register-verejnej-zelene", "dtm-bb-register-verejnej-zelene-p2",
                                           "dtm-bb-register-verejnej-zelene-p3"]
    for e in [*LAYERED, three_layers((22417, 2257, 11388))]:
        parts = generate.parts(e, "x")
        for part, _ in parts:
            pages = part["source"]["pages"]
            held = sum(max(0, min(generate.PAGE, x["rows"] - x["offset"])) for x in pages)
            assert held <= generate.PART_ROWS or len(pages) == 1, (e["name"], held)
        every = [(x["key"], x["offset"]) for part, _ in parts for x in part["source"]["pages"]]
        assert len(every) == len(set(every)) and len(every) == sum(max(1, -(-int(x["rows"] * 1.5) // generate.PAGE))
                                                                   for x in e["source"]["layers"])
    first, second = green[0][0]["source"]["pages"], green[1][0]["source"]["pages"]
    assert (first[0]["offset"], len(first)) == (0, 4) and second[0]["offset"] == 4000
    assert generate.parts(entry("bb-dotacie"), "dotacie") == [(entry("bb-dotacie"), "dotacie")]


@requires_docker
def test_a_dataset_of_three_layers_reads_each_layer_and_tags_each_row_with_its_layer():
    fixture = json.loads((FIXTURES / "dtm-vrstevnice.json").read_text())  # 2 rows as the server sent them, more to come
    line = fixture["features"][1]
    reservoir = {"type": "Feature", "id": 1, "geometry": {"type": "Point", "coordinates": [19.15, 48.73]},
                 "properties": {"objectid": 1, "typudrzovanezelene": 2}}
    channel = {**line, "id": 2, "properties": {"objectid": 2}}
    e = three_layers((97, 14, 16))
    entities = written(layered(generated(e), page(fixture["features"], more=True), {(23, 0): page([reservoir]), (26, 0): page([channel])}))
    ids = sorted(x["id"].rsplit(":", 1)[1] for x in entities)
    prefix = "dtm-bb-register-verejnej-zelene-"
    assert ids == [prefix + k for k in ("dtm-23-1", "dtm-26-2", "dtm-57-1", "dtm-57-2")], "object id 1 in two layers, two entities"
    by = {x["id"].rsplit(":", 1)[1]: x for x in entities}
    assert by[prefix + "dtm-23-1"]["typudrzovanezelene"]["value"] == 2
    assert by[prefix + "dtm-23-1"]["vrstva"]["value"] == "vodná nádrž - db"
    assert by[prefix + "dtm-57-2"]["location"]["value"] == line["geometry"], "WGS 84 as the server sent it"
    assert "location" not in by[prefix + "dtm-57-1"], "a row without geometry has no location"
    assert all(schema_errors("banskabystrica", x) == [] for x in entities)
    # The DataSource's page is never asked again; each other layer once, from its first page.
    assert sorted((layer, o) for layer, o, _ in asked) == [(23, 0), (26, 0)]
    assert {agent for _, _, agent in asked} == {generate.AGENT}


@requires_docker
def test_a_part_reads_exactly_its_pages_and_an_empty_one_past_the_end_writes_nothing():
    _, slug = PARTS["bb-dtm-bb-register-verejnej-zelene-1"][1]
    fixture = json.loads((FIXTURES / "dtm-vrstevnice.json").read_text())
    text = (SEED / "banskabystrica" / f"banskabystrica-od-pipeline-{slug}-bento.yaml").read_text()
    entities = written(layered(text, page(fixture["features"][1:]), {}))
    assert len(entities) == 1
    assert sorted(o for layer, o, _ in asked if layer == 57) == [5000, 6000, 7000]


@requires_docker
def test_a_layer_that_fails_or_answers_an_error_fails_the_run():
    fixture = (FIXTURES / "dtm-vrstevnice.json").read_bytes()
    text = generated(three_layers((97, 14, 16)))
    down = layered(text, fixture, {(23, 0): None})
    assert b"did not come" in down.stdout + down.stderr
    error = layered(text, fixture, {(26, 0): b'{"error": {"code": 400, "message": "Invalid query"}}'})
    assert b"the layer dtm-26 answered no features: Invalid query" in error.stdout + error.stderr


@requires_docker
def test_slovak_numbers_and_dates_become_numbers_and_iso_dates(tmp_path):
    """T-4324: the city's tables write "10 700,01" and "29.08.2026". The generated maps read
    them as 10700.01 and 2026-08-29, keep a dotted decimal, and leave text that is no number null."""
    mapping = tmp_path / "maps.yaml"
    probe = "\n".join([
        generate.MAPS,
        'root = this.map_each(kv -> {"float": kv.value.apply("float"), "integer": kv.value.apply("integer"),',
        '                            "date": kv.value.apply("date")})',
    ])
    mapping.write_text(yaml.safe_dump({"pipeline": {"processors": [{"mapping": probe}]}}))
    values = {"price": "10 700,01", "nbsp": "1 234,5", "thousands": "1.234,56", "negative": "-12,5",
              "dotted": "1.5", "number": 12, "text": "abc", "empty": None,
              "signed": "29.08.2026", "spaced": "1. 3. 2024", "iso": "2024-03-01"}
    out = open_data.run(mapping, json.dumps(values).encode(), "s")
    value = lambda key, kind: (out[key][kind] or {}).get("value")
    assert [value(k, "float") for k in ("price", "nbsp", "thousands", "negative", "dotted", "number")] == [
        10700.01, 1234.5, 1234.56, -12.5, 1.5, 12]
    assert value("price", "integer") == 10700
    assert value("text", "float") is None and value("empty", "float") is None and value("signed", "float") is None
    assert [value(k, "date") for k in ("signed", "spaced", "iso")] == ["2026-08-29", "2024-03-01", "2024-03-01"]
    assert value("text", "date") is None


# --- extensions (T-4336): the base whole under allOf, the formalisms beside, the examples valid ---

EXTENDED = {t: e for t, e in (MAPPING.get("extensions") or {}).items() if any(x["type"] == t for x in SEEDED)}
EXTENDED.update(generate.SPACE_EXTENSIONS)  # a city space's own (T-4338), checked the same way


def test_the_mapping_extends_where_no_model_holds_and_refuses_nothing_for_want_of_one():
    # Only the city's DTM sheets (T-4317) still wait for a model; every type T-4319 removed is extended.
    removed = [e["name"] for e in MAPPING["datasets"] if e["status"] == "removed"]
    assert all(n.startswith("bb-dtm-") for n in removed), removed
    assert not [e for e in MAPPING["datasets"] if e["status"] == "removed" and e.get("type") in MAPPING["extensions"]]
    assert {e["type"] for e in MAPPED if e.get("baseModel")} == set(MAPPING["extensions"])
    assert {d.name for d in generate.EXTENSIONS.iterdir()} == set(EXTENDED)


def test_every_extension_keeps_its_base_whole_and_only_renames_the_type():
    for t, ext in EXTENDED.items():
        schema = json.loads((generate.EXTENSIONS / t / "schema.json").read_text())
        base = json.loads((generate.SDM / ext["baseModel"].removeprefix("dataModel.") / "schema.json").read_text())
        whole, added = schema["allOf"]
        kept = {k: v for k, v in base.items() if k not in generate.SDM_HEADER}
        assert json.dumps(whole).replace(f'"enum": ["{t}"]', f'"enum": ["{generate.base_type(base)}"]') == json.dumps(kept), t
        assert set(base.get("required", [])) <= set(whole["required"]), t
        assert added["properties"]["type"]["enum"] == [t]
        assert not set(added["properties"]) - {"type"} & generate.declared(base, json.loads((generate.SDM / "common-schema.json").read_text())["definitions"])


FORMALISMS = ("schema.json", "context.jsonld", "model.linkml.yaml", "model.md", "model.owl.ttl", "model.rdf.ttl",
              "model.shacl.ttl", "examples/example.json", "examples/example-normalized.jsonld")


def test_every_extension_ships_the_seven_formalisms_and_both_examples():
    for t in EXTENDED:
        for f in FORMALISMS:
            path = generate.EXTENSIONS / t / f
            assert path.is_file() and path.stat().st_size > 100, path
        assert f"jc:{t}" in (generate.EXTENSIONS / t / "model.owl.ttl").read_text() or t in (generate.EXTENSIONS / t / "model.owl.ttl").read_text()
        assert "generation_date" not in (generate.EXTENSIONS / t / "model.rdf.ttl").read_text()


def test_every_extension_validates_its_own_examples_and_refuses_a_missing_base_requirement():
    for t, ext in EXTENDED.items():
        example = json.loads((generate.EXTENSIONS / t / "examples/example.json").read_text())
        normalized = {k: v for k, v in json.loads((generate.EXTENSIONS / t / "examples/example-normalized.jsonld").read_text()).items()
                      if k != "@context"}
        assert example["type"] == t and open_data.key_values(normalized) == example, t
        assert sdm_errors(normalized) == [], t
        required = set(json.loads((generate.EXTENSIONS / t / "schema.json").read_text())["allOf"][0]["required"]) - {"id", "type"}
        for a in required:
            assert sdm_errors({k: v for k, v in normalized.items() if k != a}), (t, a)
        base = json.loads((generate.SDM / ext["baseModel"].removeprefix("dataModel.") / "schema.json").read_text())
        assert sdm_errors(dict(normalized, type=generate.base_type(base)), model_of=t), t


def test_a_dataset_held_by_its_endpoint_alone_reads_no_catalogue_table():
    """The eight legacy uploads whose one copy was the CKAN DataStore, filled back from their own
    Endpoint (T-4742), keep their entities and model there: no DataSource reads the catalogue (T-5060)."""
    held = [e for e in SEEDED if e["source"]["kind"] == "endpoint"]
    assert len(held) == 8
    slugs = {SLUGS[e["name"]] for e in held}
    for project in generate.PROJECTS:
        assert not [p.name for p in files(project, "datasource") if p.name.removeprefix(f"{project}-od-datasource-")[:-5] in slugs]
        written = (SEED / project / f"{project}-otvorene-data.linkml.yaml").read_text()
        assert all(e["type"] in written for e in held if e["org"] == project)
    ours = re.compile(r"opendata\.dev\.joinedcontext\.com/(datastore/|api/3/action/datastore)")
    seeds = [p.name for p in SEED.rglob("*.yaml") if ours.search(p.read_text())]
    assert seeds == [], "no seed reads the platform's own CKAN DataStore"


# T-4385: what each publisher's catalogue holds once its harvest ran whole. A harvest lists a
# dataset only when od-datasets.json gives its rows an Endpoint (EP-118), one entry per dataset;
# beside them sit the Endpoints the platform publishes itself (`spec.publish.ckan`), into the
# organization of their CkanInstance. dev counted 33 on 2026-10-17 with both harvests dead.
def mapped_rows() -> dict:
    """od-datasets.json less the ŠÚ SR rows, which scripts/open-data/datacube.py writes, not the
    bb-bbsk mapping, and which CKAN lists only once T-4399 publishes them (T-4390)."""
    od = json.loads((ROOT / "charts/ckan/files/od-datasets.json").read_text())
    return {name: row for name, row in od.items() if not name.startswith("susr-")}


def expected_catalogue() -> dict[str, int]:
    od = mapped_rows()
    org_of = {e["name"]: e["org"] for e in MAPPING["datasets"]}
    counts: dict[str, int] = {}
    for name in od:
        counts[org_of[name]] = counts.get(org_of[name], 0) + 1
    # The 45 ŠÚ SR Endpoints publish themselves into slovensko (T-4402, T-4433).
    for path in sorted([*SEED.glob("*/*.yaml"), *SEED.glob("slovensko/susr-*/endpoint.yaml")]):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Endpoint":
                ckan = ((doc.get("spec") or {}).get("publish") or {}).get("ckan")
                if ckan:
                    org = ckan["instanceRef"]["name"]
                    counts[org] = counts.get(org, 0) + 1
    return counts


def test_every_endpoint_text_is_one_string_per_language():
    """A comma in a flow-mapped text, expanded by a formatter, split barcelona-endpoint-traffic's counter
    description into keys such as `el tipus i el lloc de cada aforament.: null`, which DCAT then published."""
    def texts(node, where):
        if isinstance(node, dict):
            for key, value in node.items():
                if key in ("title", "description") and isinstance(value, dict):
                    yield where + "." + key, value
                else:
                    yield from texts(value, f"{where}.{key}")
        elif isinstance(node, list):
            for i, item in enumerate(node):
                yield from texts(item, f"{where}[{i}]")

    broken = []
    for path in sorted([*SEED.glob("*/*.yaml"), *SEED.glob("slovensko/susr-*/endpoint.yaml")]):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Endpoint":
                for where, text in texts(doc.get("spec"), path.name):
                    if not all(re.fullmatch(r"[a-z]{2,3}", str(k)) and isinstance(v, str) and v.strip()
                               for k, v in text.items()):
                        broken.append((where, text))
    assert broken == []


def test_each_publisher_expects_its_converted_datasets_and_its_endpoints():
    od = mapped_rows()
    mapped = {e["name"] for e in MAPPING["datasets"] if e["status"] == "mapped"}
    assert set(od) <= mapped, "a converted dataset the mapping does not map"
    assert len({entry["dataset"] for entry in od.values()}) == len(od), "one entry per dataset"
    assert expected_catalogue() == {
        # T-5047 moved "Podujatia na Slovensku" from bbsk to slovensko; T-5065 added "Participácia občanov na Slovensku"; T-5232 added Žilina's four live datasets; T-5259 Helsinki air quality; T-5253 Helsinki's cycle and traffic counts; T-5255 the Helsinki street cleaning air pilot; T-5258 the eight Helsinki statistics themes; T-5278 the heat vulnerability index and the service property energy use; T-5268 PID vehicles and T-5276 Golemio city services and T-5269 IPR city amenities, T-5423 the PID lines, stops, metro entrances and fare zones and T-5424 on-street parking, no-parking stretches and parking machines and T-5425 its parks and green areas and, as a second dataset, its trees and T-5429 its air pollution sources, natural hazards and weather and T-5426 its cemeteries, nature trails, water areas, dog zones, planning balance units and cycle routes and T-5682 its memorials and war graves, post offices and green space elements and T-5428 its police reports, budget execution, district transfers and road works in praha; T-5257 Helsinki's feedback reports; T-5273 Bratislava's counters, bike stations and public transport stops and lines.
        # T-5680 Mr.Parkit garages and shared cars
        # T-5683 Prague's noise map.
        # T-5263 added Barcelona's air quality, weather and groundwater; T-5265 its activity agenda and its
        # facilities; T-5262 its traffic, parking and EV charging; T-5264 its districts and barris, its
        # works on the public space and its crossings; T-5431 its street furniture; T-5417 its tourist flats;
        # T-5413 its dog areas, trees and weather forecast; T-5374 its bike lanes and road accidents; T-5266 its three
        # statistics themes. T-5274 Bratislava's districts, four tables of places and 327 statistics tables (districts, okresy, city);
        # T-5673 12 more (city, daily);
        # T-5674 its 14 tables of basic settlement units and of the region's municipalities;
        # T-5675 13 more: residents of the basic settlement units, municipalities and the sports classes;
        # T-5676 17 more;
        # T-5273 its counters, pedestrians, BAjk and MHD stops and lines.
        # T-5444 eight more tables of places.
        # T-5671 19 tables over 2,000 rows.
        # T-5672 12 statistics tables with a period from a month column or the item description.
        # T-5681 added Prague's cycle counters.
        # T-5767 took the datasets empty on dev off the catalogue (owner 2026-10-26): banskabystrica
        # nabijanie and catalog (-2), helsinki-alerts (-1), praha garaze and sdilena-auta (-2), zilina-mhd (-1).
        # T-5493 added each body's own catalogue (+1 in banskabystrica, bbsk, hel-fi, praha and zilina).
        # T-5669 Helsinki's autumn traffic volumes and machine vision counts.
        # T-5666 Prague's event store praha-akce and Bratislava's events over the national store (slovensko).
        # T-5691 Prague's health care and social services (+1 in praha).
        # T-5677 Prague's TSK street sections and street cleaning dates (+2 in praha).
        # T-5670 Helsinki's heritage maps (+1 in hel-fi).
        # T-5286 Helsinki's traffic signal junctions (+1 in hel-fi).
        # T-5788 took the datasets empty on prod off the catalogue (owner 2026-10-26): banskabystrica zdravie
        # and the 8 bb- rows no Pipeline fills (-9), barcelona city-works, events, dog-areas (-3), bbsk catalog (-1),
        # bratislava MHD stops and lines (-2), helsinki heat vulnerability (-1), praha vozidla, scitace, sluzby (-3); T-5378 helsinki noise survey and tree register (+2); T-5823 helsinki nature information system (+1).
        # T-6071 took the zilina, praha and helsinki catalogues off it while dev and prod hold no record (-1 each).
        # T-6505 the proposals counted per process and state (+1 in slovensko).
        # T-6570 Barcelona's architectural heritage from Wikidata (+1 in barcelona).
        "banskabystrica": 13 + 7 + 1 - 2 - 9, "barcelona": 3 + 2 + 3 + 3 + 1 + 1 + 3 + 2 + 3 - 3 + 1, "bbsk": 88 + 6 + 1 - 1, "bratislava": 1 + 4 + 327 + 4 + 2 + 8 + 1 + 1 + 17 + 19 + 12 + 12 + 14 + 13 + 17 - 2,
        "hel-fi": 11 + 1 + 2 + 1 + 1 + 5 + 3 + 2 + 1 + 2 - 1 + 1 - 1 + 1 + 2 + 1 - 1, "praha": 8 + 1 + 1 + 4 + 3 + 1 + 5 + 1 + 1 + 6 + 3 + 5 + 2 + 1 + 1 + 1 + 1 + 2 - 2 - 3 - 1, "slovensko": 3 + 45 + 1 + 1, "zilina": 5 + 4 + 1 - 1 - 1}


def test_the_rdf_artifacts_are_canonical_whatever_order_a_generator_wrote(tmp_path):
    """T-4437: the same graph, its triples and blank nodes written in two orders as the LinkML
    generators do from run to run, comes out byte-identical, and still the same graph."""
    pytest.importorskip("rdflib")
    from rdflib import Graph
    from rdflib.compare import isomorphic

    one = """@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://example.org/> .
ex:Shape a sh:NodeShape ;
    sh:property [ sh:path ex:b ; sh:maxCount 1 ] , [ sh:path ex:a ; sh:minCount 1 ] .
"""
    two = """@prefix ex: <https://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
_:x sh:minCount 1 ; sh:path ex:a .
ex:Shape sh:property _:y , _:x ; a sh:NodeShape .
_:y sh:maxCount 1 ; sh:path ex:b .
"""
    a, b = tmp_path / "a.ttl", tmp_path / "b.ttl"
    a.write_text(one)
    b.write_text(two)
    subprocess.run([sys.executable, "-c", generate.CANONICAL, str(a), str(b)], check=True)
    assert a.read_text() == b.read_text()
    assert isomorphic(Graph().parse(a), Graph().parse(data=one, format="turtle"))
    once = a.read_text()
    subprocess.run([sys.executable, "-c", generate.CANONICAL, str(a)], check=True)
    assert a.read_text() == once


def test_every_seed_model_writes_an_annotation_map_as_a_tag_and_a_value():
    """T-5167: LinkML reads a bare `label: {sk: ...}` as the annotation's own fields and refuses the
    whole model, so Model Tools compiled no broker schema for the bb and bbsk spaces after T-4682 and
    dev kept serving the models of before it (no `name` on six types). A map goes under `value`."""
    allowed = {"tag", "value", "annotations", "extensions"}

    def bare(node, where):
        if isinstance(node, dict):
            for key, value in (node.get("annotations") or {}).items() if isinstance(node.get("annotations"), dict) else ():
                if isinstance(value, dict) and ("value" not in value or set(value) - allowed):
                    yield f"{where}.annotations.{key}"
            for key, value in node.items():
                yield from bare(value, f"{where}.{key}")
        elif isinstance(node, list):
            for i, value in enumerate(node):
                yield from bare(value, f"{where}[{i}]")

    models = sorted(SEED.glob("**/*.linkml.yaml"))
    assert models
    found = [w for m in models for w in bare(yaml.safe_load(m.read_text()), m.relative_to(SEED).as_posix())]
    assert not found, found[:5]


def test_a_slot_many_classes_map_reads_as_the_model_term_not_one_column():
    """`name` is one slot for every class of a space, each mapping it from a column of its own: the
    text of the first one ("Miesto") labelled every Organization's name (F-BNT2-010)."""
    for project in generate.PROJECTS:
        slots = yaml.safe_load((SEED / project / f"{project}-otvorene-data.linkml.yaml").read_text())["slots"]
        columns = {p["column"] for e in SEEDED if e["org"] == project for p in e["properties"].values()}
        for a, (description, texts) in generate.GENERIC.items():
            if a not in slots:
                continue
            assert slots[a]["description"] == description, (project, a)
            assert slots[a]["annotations"]["label"]["value"] == texts, (project, a)


def test_no_seed_or_generator_asks_ckan_for_a_datastore_table():
    """jc-core refuses publish.ckan.datastore since platform 752949c (T-5087): one such block makes the
    Portal refuse that seed's Endpoint (T-5060, T-5729)."""
    asked = []
    for path in sorted(SEED.rglob("*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            publish = ((doc or {}).get("spec") or {}).get("publish") if isinstance(doc, dict) else None
            if isinstance(publish, dict) and "datastore" in (publish.get("ckan") or {}):
                asked.append(str(path.relative_to(SEED)))
    assert asked == []
    writers = [str(p.relative_to(ROOT)) for d in ("scripts", "seeds") for p in (ROOT / d).rglob("*.py")
               if re.search(r"datastore:\s*\{", p.read_text())]
    assert writers == [], "a generator writes publish.ckan.datastore"
