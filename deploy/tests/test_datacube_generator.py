"""ŠÚ SR's DATAcube as 45 public Endpoints of project slovensko (T-4390, T-4402).

`scripts/open-data/datacube.py` writes one space `slovensko/<topic>/` per entry of its TOPICS: one or
two headline tables of a theme, a table over CUBE_LIMIT cells read in slices. Here it runs over a recorded excerpt
of the collection (`fixtures/slovensko/datacube/`, fetched on 2026-10-17: themes as, ps and is), with
the dimension sizes stubbed, and Bento runs the generated decoder over two recorded cubes. The
committed folders are checked against TOPICS, the seed index, the pipelines' account and the catalogue.
"""

import importlib.util
import json
import os
import re
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/slovensko"
FIXTURES = Path(__file__).resolve().parent / "fixtures/slovensko/datacube"
SCHEMA = SEED / "susr/statistical-observation.v1.schema.json"

spec = importlib.util.spec_from_file_location("datacube", ROOT / "scripts/open-data/datacube.py")
datacube = importlib.util.module_from_spec(spec)
spec.loader.exec_module(datacube)

DNS = re.compile(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")
TITLE = {"sk": "Téma", "en": "Topic"}
FIXTURE_TOPICS = [
    {"name": "susr-as", "theme": "as", "title": TITLE, "cubes": ["as1001rs", "as1007rs"]},
    {"name": "susr-ps", "theme": "ps", "title": TITLE, "cubes": ["ps1002rs"]},
]


def items() -> list[dict]:
    return datacube.datasets(json.loads((FIXTURES / "collection-sk.json").read_text()),
                             json.loads((FIXTURES / "collection-en.json").read_text()))


def generate(seed: Path, topics=FIXTURE_TOPICS):
    chosen = datacube.selected(items(), topics)
    parts = {d["code"]: [(datacube.dataset_url(d), 1000)] for _, ds in chosen for d in ds}
    files, rows = datacube.render(chosen, parts, seed)
    for path, text in files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return files, rows


def test_a_topic_takes_its_tables_in_its_order_and_a_missing_or_undated_one_is_an_error():
    chosen = datacube.selected(items(), FIXTURE_TOPICS)
    assert [(t["name"], [d["code"] for d in ds]) for t, ds in chosen] == [
        ("susr-as", ["as1001rs", "as1007rs"]), ("susr-ps", ["ps1002rs"])]
    with pytest.raises(ValueError, match="susr-x: DATAcube has no dated table zz9999rs"):
        datacube.selected(items(), [{"name": "susr-x", "theme": "as", "title": TITLE, "cubes": ["as1001rs", "zz9999rs"]}])
    undated = [d | {"dimensions": {"a_poh": {"note": "Pohlavie"}}} if d["code"] == "as1001rs" else d for d in items()]
    with pytest.raises(ValueError, match="as1001rs"):
        datacube.selected(undated, FIXTURE_TOPICS)


def test_a_dataset_without_an_sk_label_takes_the_en_one():
    by = {d["code"]: d for d in items()}
    # as1007rs is "Default" in sk.
    assert by["as1007rs"]["title"] == {"sk": "The most frequent causes of death of population",
                                       "en": "The most frequent causes of death of population"}
    assert by["as1001rs"]["title"]["sk"] == "Obyvateľstvo a charakteristiky veku"


def test_a_cubes_size_is_the_product_of_its_dimensions():
    ds = [{"code": "as1001rs", "dimensions": {"a": {}, "b": {}}}, {"code": "ps1002rs", "dimensions": {"c": {}}}]
    counts = {"a": 3, "b": 4, "c": 0}
    get = lambda url: {"category": {"index": {str(i): i for i in range(counts[url.split("/")[-1].split("?")[0]])}}}  # noqa: E731
    assert datacube.sizes(ds, get) == {"as1001rs": 12, "ps1002rs": 0}


def test_endpoint_slugs_are_stable_across_two_runs(tmp_path):
    first, rows = generate(tmp_path)
    again, rows_again = generate(tmp_path)
    assert first == again and rows == rows_again
    slugs = [yaml.safe_load(first[tmp_path / f"{t['name']}/endpoint.yaml"])["spec"]["slug"] for t in FIXTURE_TOPICS]
    assert len(set(slugs)) == 2 and all(re.fullmatch(r"[a-z2-7]{32}", s) for s in slugs)


def test_every_generated_manifest_is_a_slovensko_manifest_of_its_own_space(tmp_path):
    files, _ = generate(tmp_path)
    kinds = {}
    for path, text in files.items():
        if path.suffix != ".yaml" or path.name.endswith(".linkml.yaml"):
            continue
        doc = yaml.safe_load(text)
        space = path.parent.name
        assert doc["apiVersion"].startswith("joinedcontext.com/v1alpha") and doc["metadata"]["namespace"] == "slovensko"
        assert DNS.match(doc["metadata"]["name"]) and len(doc["metadata"]["name"]) <= 40, path
        kinds.setdefault(space, []).append(doc["kind"])
        ref = doc["spec"].get("contextSpaceRef")
        if ref is not None:
            assert (ref["name"] if isinstance(ref, dict) else ref) == space, path
        if doc["kind"] == "DataSource":
            url = doc["spec"]["http"]["url"]
            assert url.startswith("https://data.statistics.sk/api/v2/dataset/") and "headers" in doc["spec"]["http"]
            assert set(doc["spec"]["http"]["headers"]) == {"Accept"}, "a DataSource carries no credential"
        if doc["kind"] == "Policy" and doc["spec"]["assignee"]["kind"] == "serviceAccount":
            assert "deleteBatch" not in doc["spec"]["operations"]
        if doc["kind"] == "Pipeline":
            assert doc["spec"]["outputs"][0]["targetEndpoint"] == f"urn:ngsi-ld:Endpoint:joinedcontext.com:{space}:{space}"
    for space, ks in kinds.items():
        assert sorted(set(ks)) == ["ContextSpace", "DataModel", "DataSource", "Endpoint", "Pipeline", "Policy"], space


def test_catalogue_rows_name_the_generated_endpoint(tmp_path):
    files, rows = generate(tmp_path)
    slug = {t["name"]: yaml.safe_load(files[tmp_path / f"{t['name']}/endpoint.yaml"])["spec"]["slug"] for t in FIXTURE_TOPICS}
    owner = {f"susr-{c}": t["name"] for t in FIXTURE_TOPICS for c in t["cubes"]}
    assert set(rows) == set(owner)
    for name, row in rows.items():
        assert row == {"endpoint": slug[owner[name]], "type": "KeyPerformanceIndicator",
                       "dataset": f"https://opendata.dev.joinedcontext.com/dataset/{name}", "location": False, "filtered": True}


def test_the_index_block_and_the_account_roles_are_replaced_not_appended():
    chosen = datacube.selected(items(), FIXTURE_TOPICS)
    entries = datacube.placed(chosen, {d["code"]: [("", 1)] for _, ds in chosen for d in ds})
    assert entries["susr-ps/datasource-ps1002rs.yaml"] == "projects/slovensko/datasources/susr-ps1002rs.yaml"
    assert entries["susr-as/endpoint.yaml"] == "projects/slovensko/spaces/susr-as/endpoints/susr-as.yaml"
    once = datacube.index_text("a.yaml: x/a.yaml\n", entries)
    assert datacube.index_text(once, entries) == once and once.startswith("a.yaml: x/a.yaml\n" + datacube.PLACED)
    account = "spec:\n  roles:\n    - { role: space-writer, scope: { contextSpace: ovzdusie-sk } }\nstatus: {}\n"
    once = datacube.account_text(account, ["susr-as", "susr-ps"])
    assert datacube.account_text(once, ["susr-as", "susr-ps"]) == once
    assert once.splitlines()[-1] == "status: {}" and once.count("contextSpace: susr-") == 2


def test_roles_come_from_the_collection_notes():
    assert datacube.roles({"a_rok": {"note": "Rok"}, "a_obc": {"note": "0 SR + Oblasti + Kraje"}, "a_ukaz": {"note": "Ukazovateľ"},
                           "a_mj": {"note": "Merná jednotka"}, "a_poh": {"note": "Pohlavie"}, "a_x": {}}) == {
        "period": "a_rok", "area": "a_obc", "indicator": "a_ukaz", "unit": "a_mj", "keys": ["a_poh", "a_x"]}
    # A cube nothing dates is not taken.
    assert datacube.roles({"a_poh": {"note": "Pohlavie"}})["period"] is None


def decode(theme: str, fixture: str, tmp_path: Path, body: bytes | None = None) -> list[dict]:
    files, _ = generate(tmp_path)
    steps = yaml.safe_load(files[tmp_path / f"susr-{theme}/pipeline.yaml"])["spec"]["steps"]
    rendered = tmp_path / "steps.yaml"
    # The datacube step as the Portal's jc-core renders it (datacube.mapping, over the checked copy of its template).
    rendered.write_text(yaml.safe_dump({"pipeline": {"processors": [{"mapping": datacube.mapping(s["datacube"])} for s in steps]}}))
    body = body if body is not None else (FIXTURES / fixture).read_bytes()
    return open_data.run(rendered, body, f"susr-{theme}", "joinedcontext.com")


@requires_docker
def test_a_cube_becomes_one_valid_entity_per_cell_as_the_model_example_shows(tmp_path):
    entities = decode("as", "as1001rs.json", tmp_path)
    cube = json.loads((FIXTURES / "as1001rs.json").read_text())
    assert len(entities) == len([v for v in cube["value"] if v is not None])
    assert len({e["id"] for e in entities}) == len(entities)
    example = json.loads((SEED / "susr/statistical-observation.v1.example.jsonld").read_text())
    [cell] = [e for e in entities if e["id"] == example["id"]]
    assert {k: v for k, v in cell.items()} == {k: v for k, v in example.items() if k != "@context"}
    for e in entities:
        assert not open_data.schema_errors(SCHEMA, e), (e["id"], open_data.schema_errors(SCHEMA, e))


@requires_docker
def test_a_unit_dimension_is_the_unit_text_and_every_other_dimension_a_key(tmp_path):
    entities = decode("ps", "ps1002rs.json", tmp_path)
    assert len({e["id"] for e in entities}) == len(entities) == 132
    e = next(e for e in entities if e["refPeriod"]["value"] == "2015" and e["dimensionKey"]["value"] == "TOTAL-SPECU_RATE-TOTAL")
    assert e["unitText"]["value"] == "v %" and e["indicator"]["value"] == "U_PS_0001" and e["kpiValue"]["value"] == 17.3
    # T-4680: what the number is, in the publisher's words, so a reader of the entity alone knows it.
    assert e["name"]["value"] == "Miera rizika chudoby alebo sociálneho vylúčenia"
    assert e["isPartOf"]["value"] == "https://opendata.dev.joinedcontext.com/dataset/susr-ps1002rs"
    for e in entities:
        assert not open_data.schema_errors(SCHEMA, e)


@requires_docker
def test_a_cube_without_labels_has_no_name_rather_than_a_null_one(tmp_path):
    # The broker refuses a Property whose value is null (integrator-2 on T-4680).
    cube = json.loads((FIXTURES / "ps1002rs.json").read_text())
    cube.pop("label", None)
    for dimension in cube["dimension"].values():
        dimension.pop("label", None)
        dimension["category"].pop("label", None)
    entities = decode("ps", "ps1002rs.json", tmp_path, json.dumps(cube).encode())
    assert len(entities) == 132
    assert not [e["id"] for e in entities if "name" in e]
    for e in entities:
        assert not open_data.schema_errors(SCHEMA, e)


def test_the_committed_pipelines_decode_with_the_generators_steps(tmp_path):
    # A change to the decoder reaches every committed topic, not only a fresh generation.
    files, _ = generate(tmp_path)
    for path, text in files.items():
        if path.name != "pipeline.yaml":
            continue
        steps = yaml.safe_load(text)["spec"]["steps"]
        mine = yaml.safe_load((SEED / path.parent.name / "pipeline.yaml").read_text())["spec"]["steps"]
        # The cube table is the live collection's, the fixture's here; the rest is the step (T-5382).
        params = lambda ss: [(s["kind"], {k: v for k, v in s["datacube"].items() if k != "cubes"}) for s in ss]  # noqa: E731
        assert params(mine) == params(steps), path.parent.name


def test_every_committed_pipeline_is_one_datacube_step_jc_core_accepts():
    # jc-core's DatacubeStep::validate, as its regexes read (platform 12d5f50, T-5295): a step it refuses
    # stops the pipeline on dev, so the seed is held to them here.
    url = re.compile(r"^https://[!#$&-\[\]-z|~]{1,2000}$")
    dimension = re.compile(r"^[A-Za-z0-9_]{1,64}$")
    for topic in committed():
        [step] = yaml.safe_load((topic / "pipeline.yaml").read_text())["spec"]["steps"]
        assert step["kind"] == "datacube" and set(step) == {"kind", "datacube"}, topic.name
        cubes = step["datacube"]["cubes"]
        assert 1 <= len(cubes) <= 16 and url.match(step["datacube"]["datasetPage"].replace("{cube}", "cube", 1))
        for code, cube in cubes.items():
            assert re.fullmatch(r"[a-z]{2}[0-9]{4}[a-z]{2}", code) and url.match(cube["source"]), (topic.name, code)
            assert set(cube) <= {*datacube.ROLES, "keys"} and re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", cube["update"])
            for name in [cube.get(r) for r in ("period", "area", "indicator", "unit") if cube.get(r)] + cube.get("keys", []):
                assert dimension.match(name), (topic.name, code, name)


def test_the_decoder_copy_is_jc_cores_template_at_the_portal_pin():
    """scripts/open-data/datacube.blobl renders the Bento cases and the temporal audit: it must be the
    template the pinned Portal's jc-core renders (portal images.yaml `platform`)."""
    pin = yaml.safe_load((ROOT / "components/portal/images.yaml").read_text())["portal"]["portal"]["platform"]
    clone = Path(os.environ.get("JC_PLATFORM_DIR", ROOT.parent / "joinedcontext-platform"))
    shown = subprocess.run(["git", "-C", str(clone), "show", f"{pin}:crates/jc-core/src/datacube.blobl"],
                           capture_output=True, text=True, check=False)
    if shown.returncode != 0:
        pytest.skip(f"no platform clone holding {pin[:7]}")
    assert datacube.TEMPLATE.read_text() == shown.stdout


def committed() -> list[Path]:
    return sorted(SEED.glob("susr-*/"))


def test_the_committed_folders_are_the_45_topics_whole_and_listed_in_the_catalogue():
    topics = {t["name"]: t for t in datacube.TOPICS}
    assert len(topics) == len(datacube.TOPICS) == 45
    assert [f.name for f in committed()] == sorted(topics), "a susr- seed folder no topic names, or a topic not generated"
    assert set(t["theme"] for t in topics.values()) == set(datacube.THEMES)
    od = json.loads((ROOT / "charts/ckan/files/od-datasets.json").read_text())
    named = set()
    for folder in committed():
        assert 1 <= len(topics[folder.name]["cubes"]) <= 2, folder.name
        assert all(c[:2] == topics[folder.name]["theme"] for c in topics[folder.name]["cubes"]), folder.name
        for f in datacube.MODEL_FILES:
            assert (folder / f).read_text() == (SEED / "susr" / f).read_text(), f"{folder.name}/{f} is not the model's copy"
        slug = yaml.safe_load((folder / "endpoint.yaml").read_text())["spec"]["slug"]
        sources = {s["dataSourceRef"]["name"] for s in yaml.safe_load((folder / "pipeline.yaml").read_text())["spec"]["sources"]}
        files = {yaml.safe_load(p.read_text())["metadata"]["name"] for p in folder.glob("datasource-*.yaml")}
        assert sources == files, folder.name
        cubes = {f"susr-{c}" for c in topics[folder.name]["cubes"]}
        assert {re.sub(r"-[0-9]+$", "", n) for n in files} == cubes, folder.name
        for name in cubes:
            assert od[name]["endpoint"] == slug, name
        named |= cubes
    assert named == {k for k in od if k.startswith("susr-")}


def test_every_committed_seed_file_is_placed_in_the_index_and_its_space_has_a_writer():
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    on_disk = {str(p.relative_to(SEED)) for f in committed() for p in f.iterdir()}
    assert {k for k in index if k.startswith("susr-") and "/" in k} == on_disk
    assert len(set(index.values())) == len(index), "two seed files placed at one path"
    account = yaml.safe_load((SEED / "slovensko-serviceaccount-pipelines.yaml").read_text())
    writers = {r["scope"]["contextSpace"] for r in account["spec"]["roles"] if r["role"] == "space-writer"}
    assert {f.name for f in committed()} <= writers


def test_no_cube_over_the_limit_is_read_whole():
    for folder in committed():
        quota = yaml.safe_load((folder / "pipeline.yaml").read_text())["spec"]["quotas"]["maxMemoryMb"]
        assert quota <= datacube.memory(datacube.CUBE_LIMIT), folder.name


@pytest.mark.parametrize("cells,mb", [(0, 128), (1000, 192), (100_000, 384), (10**9, 2048)])
def test_memory_grows_with_the_largest_cube_and_stays_bounded(cells, mb):
    assert datacube.memory(cells) == mb


def test_every_topic_cube_is_in_the_collection_and_read_whole_only_under_the_limit():
    # The TOPICS cubes' collection entries and cell counts (product of the dimension sizes), as
    # `datacube.py --cache` read them on 2026-10-17; a cube over the limit has slices committed.
    recorded = json.loads((FIXTURES / "topics-recorded.json").read_text())
    for topic in datacube.TOPICS:
        for code in topic["cubes"]:
            assert code in recorded, f"{topic['name']}: {code} is not in the collection"
            assert recorded[code]["theme"] == topic["theme"], code
            whole = (SEED / topic["name"] / f"datasource-{code}.yaml").exists()
            assert whole == (recorded[code]["cells"] <= datacube.CUBE_LIMIT), code


def test_every_endpoint_names_its_tables_as_sources_a_role_contact_and_the_od_representations(tmp_path):
    """T-4418 (EP-78, EP-80, EP-104): each Endpoint names its DATAcube tables as `catalog.source`,
    the platform's role address as its contactPoint, and serves what generate.py's od Endpoints serve."""
    spec_od = importlib.util.spec_from_file_location("generate", ROOT / "scripts/open-data/generate.py")
    od = importlib.util.module_from_spec(spec_od)
    spec_od.loader.exec_module(od)
    # od's surfaces in od's order, less the live ones (websocket, T-4441) and the series one
    # (sta, T-4288: only datasets with an observedAt column).
    assert datacube.REPRESENTATIONS == [r for r in od.REPRESENTATIONS if r not in ("websocket", "sta")]
    files, _ = generate(tmp_path)
    by = {d["code"]: d for d in items()}
    endpoints = {p.parent.name: yaml.safe_load(t) for p, t in files.items() if p.name == "endpoint.yaml"}
    assert endpoints.keys() == {t["name"] for t in FIXTURE_TOPICS}
    for topic in FIXTURE_TOPICS:
        catalog = endpoints[topic["name"]]["spec"]["catalog"]
        assert endpoints[topic["name"]]["spec"]["enabledRepresentations"] == datacube.REPRESENTATIONS
        assert catalog["contactPoint"] == {"name": "joinedcontext platform", "email": "hello@joinedcontext.com"}
        assert [s["url"] for s in catalog["source"]] == [datacube.dataset_url(by[c]) for c in topic["cubes"]]
        for s, code in zip(catalog["source"], topic["cubes"]):
            assert s["url"].startswith(f"https://data.statistics.sk/api/v2/dataset/{code}/")
            assert set(s["title"]) == set(s["description"]) == {"sk", "en"}
            assert code in s["description"]["sk"] and code in s["description"]["en"]


def test_the_committed_endpoints_carry_the_sources_contact_and_representations():
    """T-4418: the seed is regenerated, not only the generator changed."""
    topics = {t["name"]: t for t in datacube.TOPICS}
    for folder in committed():
        spec_ = yaml.safe_load((folder / "endpoint.yaml").read_text())["spec"]
        assert spec_["enabledRepresentations"] == datacube.REPRESENTATIONS, folder.name
        assert spec_["catalog"]["contactPoint"]["email"] == datacube.CONTACT["email"], folder.name
        codes = [re.search(r"/dataset/([a-z0-9]+)/", s["url"]).group(1) for s in spec_["catalog"]["source"]]
        assert codes == topics[folder.name]["cubes"], folder.name


def test_a_run_with_a_cube_over_the_limit_stops_before_it_touches_the_seed(tmp_path, monkeypatch):
    seed = tmp_path / "slovensko"
    (seed / "susr-as").mkdir(parents=True)
    (seed / "susr-as" / "endpoint.yaml").write_text("kept\n")
    collection = {"sk": json.loads((FIXTURES / "collection-sk.json").read_text()),
                  "en": json.loads((FIXTURES / "collection-en.json").read_text())}

    def fetch(url, cache):
        if "/collection?" in url:
            return collection["sk" if "lang=sk" in url else "en"]
        # Every dimension of every cube holds 20,000 categories: one alone is over the limit.
        return {"category": {"index": {str(i): i for i in range(20_000)}}}

    monkeypatch.setattr(datacube, "fetch", fetch)
    monkeypatch.setattr(datacube, "SEED", seed)
    monkeypatch.setattr(datacube, "TOPICS", FIXTURE_TOPICS)
    with pytest.raises(SystemExit, match=r"over 10,000 cells in every slice: as1001rs \(.*\), as1007rs"):
        datacube.main([])
    assert [p.name for p in seed.iterdir()] == ["susr-as"]
    assert (seed / "susr-as" / "endpoint.yaml").read_text() == "kept\n"


def test_the_limit_refuses_what_datacube_answers_too_many_results():
    """T-4427: zo0011ms (12,096 cells) came back 400 "Too many results!" and its Endpoint stayed
    empty; sp1838ms (9,828) came back whole. The limit sits between them."""
    assert 9_828 <= datacube.CUBE_LIMIT < 12_096


# The operations a representation needs the space's read Policies to grant (API/07): the WebSocket
# fan-out subscribes on the caller's behalf, and the broker answers 405 without the grant (T-4441).
NEEDS = {"websocket": {"createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"}}


def test_no_susr_endpoint_enables_a_representation_its_policies_cannot_serve():
    """T-4441: an advertised surface the Policy cannot serve is a dead link in CKAN."""
    for folder in committed():
        enabled = yaml.safe_load((folder / "endpoint.yaml").read_text())["spec"]["enabledRepresentations"]
        granted = set()
        for policy in folder.glob("policy-*.yaml"):
            spec_ = yaml.safe_load(policy.read_text())["spec"]
            if spec_["assignee"].get("kind") == "role":
                granted |= set(spec_["operations"])
        for representation in enabled:
            missing = NEEDS.get(representation, set()) - granted
            assert not missing, f"{folder.name}: {representation} needs {sorted(missing)}"


# zo0011ms as DATAcube describes it on 2026-10-17: 2 periods x 12 indicators x import/export x 252 countries.
ZO0011MS = {"code": "zo0011ms", "dimensions": {
    "zo0011ms_rok": {"note": "Rok"}, "zo0011ms_mes": {"note": "Ukazovateľ"},
    "zo0011ms_ukaz": {"note": "Dovoz - vývoz"}, "zo0011ms_ukaz2": {"note": "Krajina"}}}
ZO0011MS_CATEGORIES = {"zo0011ms_rok": ["2026 (p)", "2025 (d)"], "zo0011ms_mes": [f"M{m:02}" for m in range(1, 13)],
                       "zo0011ms_ukaz": ["UKAZ01", "UKAZ02"], "zo0011ms_ukaz2": [f"C{i:03}" for i in range(252)]}


def requested(url: str) -> list[list[str]]:
    """The categories a dataset address asks for, per dimension ("all" kept as is)."""
    from urllib.parse import unquote
    path = url.split("/dataset/zo0011ms/")[1].split("?")[0]
    return [[unquote(c) for c in part.split(",")] for part in path.split("/")]


def test_a_cube_over_the_limit_is_read_in_slices_each_under_it_covering_every_cell_once():
    """T-4433: the slices cut one non-period dimension, every period in each, so a period's status
    turning from (p) to (d), or a new period, still answers. Import/export needs as few slices
    as the 12 indicators and has fewer codes."""
    parts = datacube.slices(ZO0011MS, ZO0011MS_CATEGORIES)
    assert len(parts) == 2
    assert all(n <= datacube.CUBE_LIMIT for _, n in parts)
    assert sum(n for _, n in parts) == 12_096
    cut = [requested(url) for url, _ in parts]
    assert all(c[0] == ["all"] and c[1] == ["all"] for c in cut), "never a period slice"
    assert sorted(code for c in cut for code in c[2]) == ["UKAZ01", "UKAZ02"]
    assert all(c[3] == ["all"] for c in cut)


def test_a_cube_under_the_limit_is_one_whole_request_and_one_that_no_slice_fits_is_none():
    small = {k: v[:1] for k, v in ZO0011MS_CATEGORIES.items()}
    assert datacube.slices(ZO0011MS, small) == [(datacube.dataset_url(ZO0011MS), 1)]
    # Only the periods could be cut: 20,000 dates by one cell each.
    dated = {"code": "zo0011ms", "dimensions": {"zo0011ms_rok": {"note": "Rok"}, "zo0011ms_ukaz": {"note": "Ukazovateľ"}}}
    assert datacube.slices(dated, {"zo0011ms_rok": [str(i) for i in range(20_000)], "zo0011ms_ukaz": ["UKAZ01"]}) is None


def test_slices_split_a_dimension_evenly_and_encode_its_codes():
    indicators = [f"M {i}/x" for i in range(12)]
    cats = dict(ZO0011MS_CATEGORIES, zo0011ms_mes=indicators, zo0011ms_ukaz=["U1", "U2", "U3", "U4", "U5"])
    # 30,240 cells: 5 import/export slices, 4 country slices (63 x 120) or 4 indicator slices
    # (3 x 2,520); the indicators have the fewest codes.
    parts = datacube.slices(ZO0011MS, cats)
    assert [n for _, n in parts] == [7_560] * 4
    cut = [requested(url) for url, _ in parts]
    assert [c[1] for c in cut] == [indicators[i:i + 3] for i in range(0, 12, 3)]
    assert all(c[0] == c[2] == c[3] == ["all"] for c in cut)
    assert "/M%200%2Fx,M%201%2Fx,M%202%2Fx/" in parts[0][0]


def test_the_generated_pipeline_lists_every_slice_and_its_memory_follows_the_largest(tmp_path):
    topic = {"name": "susr-zo-krajiny", "theme": "zo", "title": TITLE, "cubes": ["zo0011ms"]}
    d = ZO0011MS | {"theme": "zo", "update": "2026-10-01", "title": TITLE}
    parts = {"zo0011ms": datacube.slices(d, ZO0011MS_CATEGORIES)}
    files = datacube.topic_files(topic, [d], parts, "slug", 0)
    pipeline = yaml.safe_load(files["pipeline.yaml"])
    assert [s["dataSourceRef"]["name"] for s in pipeline["spec"]["sources"]] == ["susr-zo0011ms-1", "susr-zo0011ms-2"]
    sources = {yaml.safe_load(t)["metadata"]["name"]: yaml.safe_load(t)["spec"]["http"]["url"]
               for n, t in files.items() if n.startswith("datasource-")}
    assert {n for n in files if n.startswith("datasource-")} == {"datasource-zo0011ms-1.yaml", "datasource-zo0011ms-2.yaml"}
    assert sources == {f"susr-zo0011ms-{i}": url for i, (url, _) in enumerate(parts["zo0011ms"], 1)}
    assert pipeline["spec"]["quotas"]["maxMemoryMb"] == datacube.memory(6_048)


def test_population_change_is_read_by_region_and_district_and_says_so():
    """T-4741: the assistant suggested births in Banskobystrický kraj and found nothing, because
    the population cubes were SK0 only. om7104rr carries kraje and okresy in om7104rr_vuc: its
    entities get that refArea, and every topic with an area cube names territories in its
    description, so the router knows a region is there to read."""
    folder = SEED / "susr-om-pohyb"
    def cubes(topic: Path) -> dict:
        return yaml.safe_load((topic / "pipeline.yaml").read_text())["spec"]["steps"][0]["datacube"]["cubes"]
    assert cubes(folder)["om7104rr"]["area"] == "om7104rr_vuc"
    assert "refArea" in datacube.mapping({"wholeArea": "SK0", "datasetPage": "https://x/{cube}", "cubes": cubes(folder)})
    for topic in committed():
        en = yaml.safe_load((topic / "endpoint.yaml").read_text())["metadata"]["description"]["en"]
        regional = any(c.get("area") for c in cubes(topic).values())
        assert ("its territories" in en) == regional, topic.name


def test_a_topic_over_the_download_cap_declares_its_national_chart():
    """T-5089: susr-om-pohyb's file.csv answers 413, so jcctl could neither fill its DataStore nor draw a chart."""
    endpoint = yaml.safe_load((SEED / "susr-om-pohyb/endpoint.yaml").read_text())["spec"]
    assert "datastore" not in endpoint["publish"]["ckan"]
    (chart,) = endpoint["catalog"]["visualizations"]
    assert chart["data"] == {"source": "odata", "entitySet": "KeyPerformanceIndicator",
                             "$filter": "dataSet eq 'om2801ms' and refArea eq 'SK0'", "$select": "refPeriod,kpiValue,name"}
    assert chart["spec"]["encoding"]["y"] == {"aggregate": "sum", "field": "kpiValue", "type": "quantitative", "title": None}
    topic = next(t for t in datacube.TOPICS if t["name"] == "susr-om-pohyb")
    assert [chart] == yaml.safe_load("x:" + datacube.chart(topic))["x"]["visualizations"]
    # No other topic declares a chart, and none asks CKAN for a DataStore table (T-5060, T-5729).
    others = [t for t in datacube.TOPICS if t is not topic]
    assert not any(datacube.chart(t) for t in others)
    for t in others:
        assert "datastore" not in yaml.safe_load((SEED / t["name"] / "endpoint.yaml").read_text())["spec"]["publish"]["ckan"]
