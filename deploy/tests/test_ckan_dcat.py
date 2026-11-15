"""DCAT-AP on the catalogue (T-3010, EP-91): what a harvester reads has to pass the DCAT-AP 3 shapes.

The serializer is ckanext-dcat's own, run inside the catalogue image the chart pins, with the
theme mounted where the chart mounts it; the output is validated by scripts/check-dcat-ap.py
against the DCAT-AP 3.0.1 shapes. The dataset is praha-mesto as `package_show` answered on
dev (2026-09-26), so the record is the one the publisher really writes. The image pinned
before T-3010 does not carry ckanext-dcat yet, so the version images/ckan/Dockerfile pins is
installed beside it and put on the path: the test holds whichever image is pinned.
"""

import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
import yaml
from rdflib import Graph, Namespace, URIRef
from rdflib.namespace import DCAT, DCTERMS as DCT, FOAF, RDF, RDFS

from test_ckan import CHART_BASE_VALUES, by_name, ckan_container, env_of, render_chart

# Starts containers from module-scoped fixtures; one worker runs the whole module.
pytestmark = pytest.mark.xdist_group("docker-ckan-dcat")

PROJECT_ROOT = Path(__file__).resolve().parent.parent
THEME = PROJECT_ROOT / "charts/ckan/files/jc_theme"
FIXTURES = PROJECT_ROOT / "tests/fixtures/ckan-dcat"
PINNED = yaml.safe_load((PROJECT_ROOT / "components/ckan/images.yaml").read_text())["ckan"]["catalogue"]
IMAGE = f"{PINNED['repository']}:{PINNED['tag']}@{PINNED['digest']}"
PROFILES = "euro_dcat_ap_3 jc_dcat_ap"
BRANDING = {
    "instanceName": "joinedcontext",
    "organisation": "City of Helsinki (demo instance, not affiliated)",
    "contactEmail": "hello@joinedcontext.com",
    "languages": {"default": "en", "offered": ["en", "fi"]},
    "siteIssued": "2026-09-28",
}

requires_docker = pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")

spec = importlib.util.spec_from_file_location("check_dcat_ap", PROJECT_ROOT / "scripts/check-dcat-ap.py")
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


def with_dcat(enabled: bool) -> dict:
    return {"ckan": dict(CHART_BASE_VALUES["ckan"], dcat={"enabled": enabled})}


@requires_helm
def test_the_flag_lists_the_plugin_and_both_profiles_and_nothing_while_off(tmp_path):
    """CKAN exits on a plugin it cannot import, so `dcat` is listed only with the flag, and the
    theme's profile runs after the EU one it corrects."""
    off = env_of(ckan_container(render_chart(tmp_path / "off", with_dcat(False))))
    assert "dcat" not in off["CKAN__PLUGINS"].split()
    assert "CKANEXT__DCAT__RDF__PROFILES" not in off

    on = env_of(ckan_container(render_chart(tmp_path / "on", with_dcat(True))))
    assert on["CKAN__PLUGINS"].split()[-1] == "dcat"
    assert on["CKANEXT__DCAT__RDF__PROFILES"] == PROFILES


@requires_helm
def test_the_dataset_page_keeps_the_keys_the_theme_reads(tmp_path):
    """T-3025: ckanext-dcat renames every extra of a page's dataset to an English label
    (`conforms_to` -> `Conforms to`) unless told not to, and the theme's About list, which labels
    those keys itself in four languages, then found none of them."""
    on = env_of(ckan_container(render_chart(tmp_path / "on", with_dcat(True))))
    assert on["CKANEXT__DCAT__TRANSLATE_KEYS"] == "false"
    off = env_of(ckan_container(render_chart(tmp_path / "off", with_dcat(False))))
    assert "CKANEXT__DCAT__TRANSLATE_KEYS" not in off


@requires_helm
def test_the_profile_is_mounted_where_its_entry_point_imports_it(tmp_path):
    """An entry point naming a module that is not mounted is a /catalog.ttl that answers 500."""
    docs = render_chart(tmp_path / "on", with_dcat(True))
    mounts = {m["subPath"]: m["mountPath"] for m in ckan_container(docs)["volumeMounts"] if m["name"] == "theme"}
    assert mounts["dcat_profile.py"] == "/srv/theme/ckanext_jc_theme/dcat_profile.py"
    entry = re.search(r"^jc_dcat_ap = ckanext_jc_theme\.dcat_profile:(\w+)$", (THEME / "entry_points.txt").read_text(), re.M)
    assert entry and f"class {entry.group(1)}(" in (THEME / "dcat_profile.py").read_text()
    assert "dcat_profile.py" in by_name(docs, "ConfigMap", "ckan-theme")["data"]


@pytest.fixture(scope="module")
def dcat_libs(tmp_path_factory):
    """ckanext-dcat at the Dockerfile's pin and the libraries the base image lacks, for CPython 3.10."""
    version = re.search(r"'ckanext-dcat==([\d.]+)'", (PROJECT_ROOT / "images/ckan/Dockerfile").read_text()).group(1)
    target = tmp_path_factory.mktemp("dcat-libs")
    subprocess.run(
        [sys.executable, "-m", "pip", "install", "--quiet", "--no-deps", "--target", str(target),
         "--python-version", "3.10", "--only-binary=:all:", f"ckanext-dcat=={version}",
         "rdflib", "pyld", "geomet", "isodate", "cachetools", "frozendict"],
        check=True,
    )
    return target


@pytest.fixture(scope="module")
def serialize(tmp_path_factory, dcat_libs):
    """`serialize(mode, profiles)`: the praha-mesto record as Turtle, from the pinned image."""
    workdir = tmp_path_factory.mktemp("dcat-run")
    branding = workdir / "branding.json"
    branding.write_text(json.dumps(BRANDING))
    mounts = []
    # The chart's own mounts, so a file the chart forgets to mount is missing here as well.
    docs = render_chart(workdir / "chart", with_dcat(True))
    for mount in ckan_container(docs)["volumeMounts"]:
        source = THEME / mount.get("subPath", "")
        if mount["name"] == "theme" and source.is_file():
            mounts += ["-v", f"{source}:{mount['mountPath']}:ro"]

    def run(mode: str, profiles: str, drop: tuple[str, ...] = (), extras: dict | None = None,
            resources: list[dict] = ()) -> Graph:
        dataset = json.loads((FIXTURES / "praha-mesto.json").read_text())
        for field in drop:
            dataset.pop(field)
        dataset["extras"] += [{"key": k, "value": v} for k, v in (extras or {}).items()]
        dataset["resources"] += list(resources)
        result = subprocess.run(
            # As the caller, so pytest's private temporary directories are readable inside.
            ["docker", "run", "--rm", "-i", "--network", "none", "--user", f"{os.getuid()}:{os.getgid()}",
             "-e", "PYTHONPATH=/srv/theme", "-e", "JC_BRANDING_FILE=/etc/jc/branding.json",
             "-e", "JC_DCAT_LIBS=/srv/dcat", "-v", f"{dcat_libs}:/srv/dcat:ro",
             "-v", f"{branding}:/etc/jc/branding.json:ro",
             "-v", f"{FIXTURES / 'serialize.py'}:/srv/serialize.py:ro", *mounts,
             IMAGE, "python3", "/srv/serialize.py", mode, profiles],
            input=json.dumps(dataset), capture_output=True, text=True, timeout=300,
        )
        assert result.returncode == 0, result.stderr[-2000:]
        return Graph().parse(data=result.stdout, format="turtle")

    return run


@pytest.fixture(scope="module")
def shapes():
    return check.shapes()


@requires_docker
@requires_helm
def test_the_eu_profile_alone_is_refused_by_the_shapes(serialize, shapes):
    """Why the theme carries a profile at all: CKAN's strings where the shapes want IRIs."""
    problems = "\n".join(check.violations(serialize("dataset", "euro_dcat_ap_3"), shapes))
    assert "dcat#mediaType" in problems
    assert "terms/format" in problems
    assert "dcat#theme" in problems


@requires_docker
@requires_helm
def test_a_dataset_is_valid_dcat_ap_3(serialize, shapes):
    """EP-91: one dataset, as /dataset/{id}.ttl answers it."""
    graph = serialize("dataset", PROFILES)
    assert check.violations(graph, shapes) == []
    dataset = graph.value(None, RDF.type, DCAT.Dataset, any=False)
    # What a harvester keys a re-harvest on, and what the record already said.
    assert str(graph.value(dataset, DCT.identifier)) == "jw7tffshzip7igmzcszgffkkkuetnc6v"
    assert str(graph.value(dataset, DCT.spatial)) == "http://data.europa.eu/nuts/code/CZ010"
    services = {str(graph.value(s, DCT.conformsTo)) for s in graph.subjects(RDF.type, DCAT.DataService)}
    assert services == {
        "https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/",
        "https://modelcontextprotocol.io/specification",
    }
    licences = {str(graph.value(d, DCT.license)) for d in graph.objects(dataset, DCAT.distribution)}
    assert licences == {"http://publications.europa.eu/resource/authority/licence/CC_BY_4_0"}


@requires_docker
@requires_helm
def test_a_record_without_a_description_still_passes_under_its_title(serialize, shapes):
    """EP-91: dct:description is mandatory; an Endpoint that wrote none is not refused by harvesters."""
    graph = serialize("dataset", PROFILES, drop=("notes",))
    assert check.violations(graph, shapes) == []
    dataset = graph.value(None, RDF.type, DCAT.Dataset, any=False)
    assert graph.value(dataset, DCT.description) == graph.value(dataset, DCT.title)


# What jcctl writes of an Endpoint's catalogue block beside the rest (T-4417, EP-78).
CREDITED = {
    "rights": "Zdroj: Hlavní město Praha, CC BY 4.0",
    "source": '["https://opendata.praha.eu/datasets/rozpocet"]',
    "was_generated_by": "pipeline od-rozpocet",
}


@requires_docker
@requires_helm
def test_the_attribution_rides_every_distribution_and_the_source_and_pipeline_the_dataset(serialize, shapes):
    """EP-78, T-4417: dct:rights on each distribution, dct:source and prov:wasGeneratedBy on the
    dataset, and the shapes still pass."""
    graph = serialize("dataset", PROFILES, extras=CREDITED)
    assert check.violations(graph, shapes) == []
    (dataset,) = set(graph.subjects(DCT.source))
    distributions = list(graph.objects(dataset, DCAT.distribution))
    assert distributions
    for distribution in distributions:
        statement = graph.value(distribution, DCT.rights)
        assert (statement, RDF.type, DCT.RightsStatement) in graph
        assert str(graph.value(statement, RDFS.label)) == CREDITED["rights"]
    assert [str(s) for s in graph.objects(dataset, DCT.source)] == ["https://opendata.praha.eu/datasets/rozpocet"]
    prov = Namespace("http://www.w3.org/ns/prov#")
    activity = graph.value(dataset, prov.wasGeneratedBy)
    assert str(graph.value(activity, RDFS.label)) == "pipeline od-rozpocet"
    # Without the extras nothing of them is invented.
    plain = serialize("dataset", PROFILES)
    assert not list(plain.objects(None, DCT.rights)) and not list(plain.objects(None, DCT.source))
    # T-4436: a source jcctl names with its title is titled so in the record, and still passes.
    titled = serialize("dataset", PROFILES, extras=dict(CREDITED, source=json.dumps([
        {"url": "https://opendata.praha.eu/datasets/rozpocet", "title": "Rozpočet", "description": "Rozpočet města"}])))
    assert check.violations(titled, shapes) == []
    source = URIRef("https://opendata.praha.eu/datasets/rozpocet")
    assert (source, RDF.type, DCAT.Dataset) in titled
    assert [str(t) for t in titled.objects(source, DCT.title)] == ["Rozpočet"]
    assert [str(t) for t in titled.objects(source, DCT.description)] == ["Rozpočet města"]
    # A bare IRI still names itself.
    assert [str(t) for t in graph.objects(source, DCT.title)] == ["https://opendata.praha.eu/datasets/rozpocet"]


@requires_docker
@requires_helm
def test_the_catalogue_page_is_valid_dcat_ap_3_and_names_its_publisher(serialize, shapes):
    """EP-91: /catalog.ttl, with its publisher and languages from the branding block."""
    graph = serialize("catalog", PROFILES)
    assert check.violations(graph, shapes, DCAT.Catalog) == []
    catalogue = graph.value(None, RDF.type, DCAT.Catalog, any=False)
    assert len(list(graph.objects(catalogue, DCAT.dataset))) == 1
    publisher = graph.value(catalogue, DCT.publisher)
    assert str(graph.value(publisher, FOAF.name)) == BRANDING["organisation"]
    assert {str(language) for language in graph.objects(catalogue, DCT.language)} == {
        "http://publications.europa.eu/resource/authority/language/ENG",
        "http://publications.europa.eu/resource/authority/language/FIN",
    }
    # The day the catalogue went public, from the instance's `site.issued` (T-4733).
    assert [str(d) for d in graph.objects(catalogue, DCT.issued)] == ["2026-09-28"]


# A resource a republish re-created today (T-4733): CKAN dates it by `created`.
RECREATED = {
    "id": "c3f0a3e2-0000-4000-8000-000000000002",
    "package_id": "bc2d847b-00a9-4605-8d12-cf3f6c302492",
    "name": "praha-mesto data.csv",
    "url": "https://api.dev.joinedcontext.com/api/endpoint/jw7tffshzip7igmzcszgffkkkuetnc6v/data.csv",
    "format": "CSV",
    "mimetype": "text/csv",
    "created": "2026-10-19T08:00:00.000000",
}


@requires_docker
@requires_helm
def test_no_distribution_is_dated_after_its_dataset(serialize, shapes):
    """T-4733: a republish re-creates the resources, so `created` is no publication date. A later
    one is the dataset's dct:issued, an earlier one stays; the shapes still pass."""
    graph = serialize("dataset", PROFILES, extras={"issued": "2026-09-28T00:00:00Z"}, resources=[RECREATED])
    assert check.violations(graph, shapes) == []
    dataset = graph.value(None, RDF.type, DCAT.Dataset, any=False)
    dates = {str(d).rsplit("/", 1)[-1]: [str(i) for i in graph.objects(d, DCT.issued)]
             for d in graph.objects(dataset, DCAT.distribution)}
    (recreated,) = dates.pop(RECREATED["id"])
    assert recreated.startswith("2026-09-28T00:00:00"), recreated
    # Every other resource of the fixture was created 2026-09-25 and keeps it.
    assert dates and all(len(v) == 1 and v[0].startswith("2026-09-25") for v in dates.values()), dates


# What CKAN held on dev for every schema OWL artifact (T-4586): a media type with a parameter.
OWL_ARTIFACT = {
    "id": "c3f0a3e2-0000-4000-8000-000000000001",
    "package_id": "bc2d847b-00a9-4605-8d12-cf3f6c302492",
    "name": "praha-mesto model.owl.ttl",
    "url": "https://api.dev.joinedcontext.com/api/endpoint/jw7tffshzip7igmzcszgffkkkuetnc6v/schema/v2/model.owl.ttl",
    "format": "OWL",
    "mimetype": 'text/turtle; profile="owl"',
}


@requires_docker
@requires_helm
def test_an_address_the_turtle_writer_cannot_write_is_left_out_and_the_catalogue_still_writes(serialize, shapes):
    """T-6218 (R2DEP1 F-008): a spatial, source, licence or service address with a space or a
    quote is an IRI rdflib refuses to write, and one such record made /catalog.ttl a 500 for every
    dataset, as the media type did in T-4586. The profile leaves such an address out."""
    bad = "https://opendata.praha.eu/a b\"c"
    service = dict(OWL_ARTIFACT, id="c3f0a3e2-0000-4000-8000-000000000003", format="NGSI-LD",
                   url=bad, mimetype="application/ld+json")
    graph = serialize("catalog", PROFILES, extras={"spatial_uri": bad, "license_url": bad,
                                                    "source": json.dumps([bad])}, resources=[service])
    assert check.violations(graph, shapes, DCAT.Catalog) == []
    assert not [o for o in graph.objects() if " " in str(o) and isinstance(o, URIRef)]


@requires_docker
@requires_helm
def test_a_media_type_with_a_parameter_is_its_bare_iana_type_and_the_catalogue_still_writes(serialize, shapes):
    """T-4586: /catalog.ttl answered 500 because `text/turtle; profile="owl"` became an IRI with a
    space and quotes. The register names the bare type; a profile that is no IRI is dropped, one
    that is an IRI is the distribution's dct:conformsTo."""
    graph = serialize("catalog", PROFILES, resources=[OWL_ARTIFACT])
    assert check.violations(graph, shapes, DCAT.Catalog) == []
    (owl,) = set(graph.subjects(DCAT.accessURL, URIRef(OWL_ARTIFACT["url"])))
    assert [str(m) for m in graph.objects(owl, DCAT.mediaType)] == ["http://www.iana.org/assignments/media-types/text/turtle"]
    assert graph.value(owl, DCT.conformsTo) is None

    shacl = dict(OWL_ARTIFACT, id="c3f0a3e2-0000-4000-8000-000000000002", url=OWL_ARTIFACT["url"].replace("owl", "shacl"),
                 mimetype='text/turtle; profile="http://www.w3.org/ns/shacl#"')
    graph = serialize("dataset", PROFILES, resources=[shacl])
    assert check.violations(graph, shapes) == []
    (distribution,) = set(graph.subjects(DCAT.accessURL, URIRef(shacl["url"])))
    assert str(graph.value(distribution, DCT.conformsTo)) == "http://www.w3.org/ns/shacl#"


def test_an_empty_document_is_not_a_pass(shapes):
    """An empty graph conforms to every shape; the checker must not read that as valid."""
    assert check.violations(Graph(), shapes) == ["no dcat:Dataset in the document"]


@requires_docker
def test_every_plugin_dev_lists_is_in_the_image_dev_pins(rendered):
    """`global.ckan.dcat` goes on with the pin of an image built from images/ckan/Dockerfile.
    Turned on against an older pin, CKAN exits on `dcat` at start-up; this is where that is
    refused instead. The theme is mounted, not installed, so it is the one plugin left out."""
    container = ckan_container(rendered("dev"))
    listed = [p for p in env_of(container)["CKAN__PLUGINS"].split() if p != "jc_theme"]
    result = subprocess.run(
        ["docker", "run", "--rm", "--network", "none", container["image"], "python3", "-c",
         "import sys; from importlib.metadata import entry_points; "
         "known = {e.name for e in entry_points(group='ckan.plugins')}; "
         "print(' '.join(p for p in sys.argv[1:] if p not in known))", *listed],
        capture_output=True, text=True, timeout=300,
    )
    assert result.returncode == 0, result.stderr[-2000:]
    assert result.stdout.split() == [], f"{container['image']} carries no {result.stdout.split()}"
