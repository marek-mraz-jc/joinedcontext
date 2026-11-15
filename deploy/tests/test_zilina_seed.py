"""The Žilina project's seed (T-3137, PF-54, PF-84, DM-61).

The city of Žilina and the University of Žilina publish nothing in the national catalogue and the
city's GIS layers carry no licence, so the project is built on national open data about the city
and on the university's own repository (joinedcontext-docs Research/zilina-open-data-sources.md).
These cases read the seed manifests directly; what the cluster does with them is `dev-smoke`'s.
"""

import json
from pathlib import Path

import pytest
import yaml
from conftest import demo_id

# Reading, plus the subscriptions a WebSocket needs (T-4073); no entity write.
READ_AND_SUBSCRIBE = ["retrieveOps", "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"]
DISCOVERY = [
    "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveEntityTypeInfo",
    "retrieveAttrTypes", "retrieveAttrTypeDetails", "retrieveAttrTypeInfo",
]

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
ZILINA = SEED / "zilina"
REGION = SEED / "bbsk"
SPACES = {
    "zilina-mesto": "statistical-observation",
    "zilina-verejne": "zilina-verejne",
    "zilina-uniza": "zilina-uniza",
    "zilina-kpi": "key-performance-indicator",
    # The city's own open data portal, live (T-5232).
    "zilina-doprava": "zilina-doprava",
}
# The two models the project shares with bbsk, copied rather than generated twice.
SHARED = [
    f"{model}{suffix}"
    for model in ("statistical-observation", "key-performance-indicator")
    for suffix in (".linkml.yaml", ".v1.schema.json", ".v1.context.jsonld", ".v1.md")
]


def manifests(kind: str):
    for path in sorted(ZILINA.glob("*.yaml")):
        if path.name == "index.yaml" or path.name.endswith((".linkml.yaml", "-bento.yaml")):
            continue
        # The city's slice of the catalogue is the Portal's to write, as every body's is
        # (tests/test_catalog_seed.py, T-5493): none of the city's data spaces.
        if "-catalog" in path.name:
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == kind:
                yield doc


def by_name(kind: str) -> dict:
    return {doc["metadata"]["name"]: doc for doc in manifests(kind)}


def test_the_index_lists_every_file_and_names_no_missing_one():
    index = yaml.safe_load((ZILINA / "index.yaml").read_text())
    files = {p.name for p in ZILINA.iterdir() if p.is_file() and p.name not in {"index.yaml", "README.md"}}
    assert set(index) == files
    # The Organization and its bindings live in the organization repository (ADR-N-058).
    own = {"organizations/zilina.yaml", "users/assignments/zilina-admins.yaml", "users/assignments/zilina-viewers.yaml"}
    assert all(path.startswith("projects/zilina/") or path in own for path in index.values())
    assert own <= set(index.values())
    assert len(set(index.values())) == len(index), "two files on one repository path"


def test_the_forge_bootstrap_commits_the_seed():
    values = (ROOT / "components/gitea/values/bootstrap/development-values.yaml.gotmpl").read_text()
    assert 'seed/zilina/index.yaml' in values


def test_one_project_five_spaces_each_pinning_its_segment_and_its_one_model():
    project = by_name("Project")
    assert list(project) == ["zilina"]
    assert project["zilina"]["spec"]["organizationRef"] == "zilina"
    spaces = by_name("ContextSpace")
    assert set(spaces) == set(SPACES)
    models = by_name("DataModel")
    for name, model in SPACES.items():
        spec = spaces[name]["spec"]
        assert spec["urnSegment"] == name, "the segment the ids carry is the space's name (PF-84)"
        assert spec["dataModelRef"] == {"kind": "DataModel", "name": model}
        assert models[model]["spec"]["contextSpaceRef"] == name
        assert models[model]["spec"]["lifecycle"] == "published"
        for artifact in models[model]["spec"]["artifacts"].values():
            assert (ZILINA / artifact.removeprefix("./")).is_file(), artifact


def test_the_shared_models_are_the_regions_files():
    for name in SHARED:
        assert (ZILINA / name).read_bytes() == (REGION / name).read_bytes(), name


def key_values(entity: dict) -> dict:
    out = {}
    for name, attribute in entity.items():
        if not isinstance(attribute, dict) or "type" not in attribute:
            out[name] = attribute
        elif attribute["type"] == "Relationship":
            out[name] = attribute["object"]
        elif attribute["type"] == "LanguageProperty":
            out[name] = attribute.get("languageMap", attribute.get("value"))
        else:
            out[name] = attribute["value"]
    return out


# The indicator model leaves its values' JSON types to the gateway's NGSI-LD reading (a number
# or "not measured", a window object, a DateTime); its example is checked by its attributes below.
@pytest.mark.parametrize("model", ["statistical-observation", "zilina-verejne", "zilina-uniza", "zilina-doprava"])
def test_every_example_validates_against_its_model(model):
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((ZILINA / f"{model}.v1.schema.json").read_text())
    example = json.loads((ZILINA / f"{model}.v1.example.jsonld").read_text())
    example.pop("@context", None)
    # The hand-written example is normalised NGSI-LD; the generated ones are key-values.
    entity = key_values(example) if model == "statistical-observation" else example
    validator = jsonschema.Draft7Validator(schema["definitions"][entity["type"]])
    errors = sorted(validator.iter_errors(entity), key=str)
    assert not errors, [e.message for e in errors]


def test_the_indicator_example_carries_exactly_the_models_attributes():
    linkml = yaml.safe_load((ZILINA / "key-performance-indicator.linkml.yaml").read_text())
    example = json.loads((ZILINA / "key-performance-indicator.v1.example.jsonld").read_text())
    attributes = set(example) - {"@context", "id", "type"}
    assert attributes == set(linkml["classes"]["KeyPerformanceIndicator"]["slots"]) - {"location"}


def test_the_examples_of_the_shared_models_name_this_projects_spaces():
    for model, space in (("statistical-observation", "zilina-mesto"), ("key-performance-indicator", "zilina-kpi")):
        example = json.loads((ZILINA / f"{model}.v1.example.jsonld").read_text())
        assert example["id"].split(":")[3:5] == ["zilina.sk", space]


def test_no_model_carries_a_person():
    """Open data rules §1: authors, owners and parcels name or locate people, so no slot holds them."""
    for model in ("zilina-verejne", "zilina-uniza"):
        linkml = yaml.safe_load((ZILINA / f"{model}.linkml.yaml").read_text())
        slots = set(linkml["slots"])
        assert not slots & {"author", "creator", "owner", "ownerName", "parcel", "parcelNumber", "contributor"}, model
    uniza = yaml.safe_load((ZILINA / "zilina-uniza.linkml.yaml").read_text())
    assert uniza["slots"]["license"]["required"] is True, "only works that carry their own licence enter"


def test_the_pipelines_account_writes_the_projects_five_spaces_and_nothing_else():
    account = by_name("ServiceAccount")["pipelines"]["spec"]
    scopes = {role["scope"]["contextSpace"] for role in account["roles"]}
    assert scopes == set(SPACES)
    assert {role["role"] for role in account["roles"]} == {"space-writer"}


def test_the_catalogue_is_the_projects_own_organisation():
    ckan = by_name("CkanInstance")["zilina"]["spec"]
    assert ckan["organizationDefault"] == "zilina"
    assert "apiTokenRef" in ckan and "token" not in ckan, "the token is a reference, never a value"


CKAN_LICENCE = {"CC_BY_4_0": "cc-by", "CC_BYSA_4_0": "cc-by-sa", "CC0": "cc-zero", "ODC_ODBL": "odc-odbl"}
PUBLIC_SPACES = {"zilina-verejne", "zilina-uniza", "zilina-kpi", "zilina-doprava"}
# The public space's three types are three datasets (T-4344); its own Endpoint stays for the Apps.
PUBLISHED = {"zilina-uniza", "zilina-kpi", "zilina-ovzdusie", "zilina-pamiatky", "zilina-zeleznicne-stanice",
             # The city portal's four kinds of thing, one dataset each (T-5232).
             "zilina-parkoviska", "zilina-scitace", "zilina-svietidla"}
# zilina-mhd stays public for its readers but leaves the catalogue while empty on dev (T-5767).
PUBLIC = PUBLISHED | {"zilina-verejne", "zilina-doprava", "zilina-mhd"}


def test_every_public_space_is_published_to_the_projects_catalogue_with_its_record():
    """T-3139, EP-27, EP-76: the three public spaces reach CKAN with a complete DCAT-AP record."""
    endpoints = {n: e for n, e in by_name("Endpoint").items() if not n.startswith("app-")}
    assert {n for n, e in endpoints.items() if e["spec"]["audience"] == "public"} == PUBLIC
    assert {n for n, e in endpoints.items() if e["spec"].get("publish")} == PUBLISHED
    for name in PUBLISHED:
        endpoint = endpoints[name]
        spec = endpoint["spec"]
        assert set(endpoint["metadata"]["description"]) == {"sk", "en"}, name
        assert spec["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "zilina"}
        assert "datastore" not in spec["publish"]["ckan"], f"{name}: CKAN holds no rows (T-5060)"
        catalog = spec["catalog"]
        # The CKAN licence is the record's licence: ShareAlike data is never offered as plain CC BY.
        assert spec["publish"]["ckan"]["license"] == CKAN_LICENCE[catalog["license"]], name
        for member in ("publisher", "attribution", "themes", "keywords", "spatial", "frequency", "pipelineRef"):
            assert catalog.get(member), f"{name}: no {member}"
        assert "https://data.gov.sk/id/lau2/SK031B517402" in catalog["spatial"]
        assert catalog["pipelineRef"]["name"] in by_name("Pipeline"), name
    # The raw ŠÚ SR mirror is described for the organization, never republished under our URL.
    mesto = endpoints["zilina-mesto"]["spec"]
    assert mesto["audience"] == "organization" and "publish" not in mesto
    assert mesto["catalog"]["license"] == "CC_BYSA_4_0"


def test_the_public_reads_only_and_only_the_types_of_its_space():
    policies = [p for p in manifests("Policy") if p["spec"]["assignee"] == {"kind": "role", "id": "public"}]
    assert {p["spec"]["contextSpaceRef"]["name"] for p in policies} == PUBLIC_SPACES
    models = by_name("DataModel")
    granted: dict[str, set[str]] = {}
    for policy in policies:
        spec = policy["spec"]
        # Reading, discovery (T-4258, EP-116) and the WebSocket's subscriptions; no entity write.
        # The history of the types that keep one is read beside them or in a Policy of its own
        # (T-4372, tests/test_seed_history.py).
        history = ["queryTemporal", "retrieveTemporal"]
        read = READ_AND_SUBSCRIBE[:1] + DISCOVERY + READ_AND_SUBSCRIBE[1:]
        assert spec["operations"] in (read, read + history, history), policy["metadata"]["name"]
        space = spec["contextSpaceRef"]["name"]
        types = {e["type"] for info in spec["information"] for e in info["entities"]}
        granted.setdefault(space, set()).update(types)
    for space, types in granted.items():
        linkml = yaml.safe_load((ZILINA / f"{SPACES[space]}.linkml.yaml").read_text())
        # A value class (CalculationPeriod, Any: the shape of one slot's value) is no type.
        entities = {name for name, body in linkml["classes"].items() if (body or {}).get("is_a") == "Entity"}
        assert types == entities, space
        assert models[SPACES[space]]["spec"]["contextSpaceRef"] == space


APPS = {"zilina-mapa", "zilina-ukazovatele", "zilina-zaznamy", "zilina-vyskum"}


def test_the_four_apps_are_public_read_only_and_granted_exactly_what_they_show():
    """T-3140, AP-04, AP-28: the apps the Portal image ships, each reading public spaces only, its
    own through the Endpoint and Policy the Portal compiles for it and no attribute more."""
    apps = by_name("App")
    assert set(apps) == APPS
    generated = {"joinedcontext.com/generated-by": "portal/app-reconciler"}
    endpoints, policies = by_name("Endpoint"), by_name("Policy")
    for name, app in apps.items():
        spec = app["spec"]
        assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
        assert (spec["visibility"], spec["lifecycle"], spec["source"]) == ("public", "published", {"path": f"./apps/{name}"})
        needs = spec["dataNeeds"]
        assert all(set(need["operations"]) <= {"queryEntity", "retrieveEntity"} for need in needs), name
        assert {need["contextSpaceRef"]["name"] for need in needs} <= PUBLIC, name
        own, *further = needs
        assert len(further) <= 1, f"{name}: its own space and one more at most (AP-04)"
        endpoint = endpoints[f"app-{name}"]
        assert endpoint["metadata"]["annotations"] == generated
        assert endpoint["spec"]["contextSpaceRef"]["name"] == own["contextSpaceRef"]["name"]
        assert endpoint["spec"]["audience"] == "public"
        policy = policies[f"app-{name}-1"]["spec"]
        assert policy["assignee"] == {"kind": "role", "id": f"endpoint:zilina/app-{name}"}
        assert policy["contextSpaceRef"]["name"] == own["contextSpaceRef"]["name"]
        (rule,) = policy["information"]
        assert [entity["type"] for entity in rule["entities"]] == own["types"]
        assert rule["propertyNames"] == own["attrs"]
        assert set(policy["operations"]) == set(own["operations"])
        # A further space is read through its public Endpoint, whose public Policy already grants it.
        for need in further:
            assert need["contextSpaceRef"]["name"] in {e["spec"]["contextSpaceRef"] for e in endpoints.values()
                                                       if e["spec"]["audience"] == "public" and "publish" in e["spec"]}


def test_zilina_is_an_organization_of_its_own_pf121_pf122():
    """ADR-N-058 (T-5242): Mesto Žilina is its own Organization, not the installation's, and its
    project publishes under its domain; its publisher is the city by its registry IRI."""
    org = by_name("Organization")["zilina"]
    assert org["metadata"]["title"]["sk"] == "Mesto Žilina"
    spec = org["spec"]
    assert spec["domain"] == "zilina.sk"
    assert spec["locales"] == ["sk", "en"] and spec["defaultLocale"] == "sk"
    assert spec["dcatProfiles"] == ["dcat-ap-sk"]
    assert not spec.get("installation"), "Helsinki stays the installation's Organization"
    assert spec["catalog"]["publisher"]["uri"] == "https://data.gov.sk/id/legal-subject/00321796"
    project = by_name("Project")["zilina"]["spec"]
    domain = project["domain"]
    assert domain == spec["domain"] or domain.endswith("." + spec["domain"]), "PF-122"


def test_the_zilina_bindings_end_at_its_organization_pf123():
    """Each seeded binding of Žilina is scoped to the Organization `zilina`, and no binding of the
    seed reaches both Organizations (ADR-N-058 2.4)."""
    bindings = by_name("RoleBinding")
    admins, viewers = bindings["zilina-admins"]["spec"], bindings["zilina-viewers"]["spec"]
    assert admins == {"subjects": [{"user": demo_id("demo.zilina")}], "role": "org-admin", "scope": {"organization": "zilina"}}
    assert viewers == {
        "subjects": [{"group": "zilina", "source": "provider"}], "role": "viewer", "scope": {"organization": "zilina"},
    }
    every = [
        doc["spec"]
        for path in sorted(SEED.glob("*/*.yaml"))
        for doc in yaml.safe_load_all(path.read_text())
        if isinstance(doc, dict) and doc.get("kind") == "RoleBinding"
    ]
    zilina_people = {demo_id("demo.zilina"), "zilina"}
    for spec in every:
        names = {s.get("user") or s.get("group") for s in spec["subjects"]}
        if spec.get("scope", {}).get("organization") == "hel":
            assert not names & zilina_people, f"a Helsinki binding names Žilina's people: {spec}"


def test_the_ckan_organisation_is_titled_mesto_zilina():
    """The instance title is what the publisher creates the CKAN organisation with and renames
    an existing one to (jcctl, ADR-N-058 2.6)."""
    instance = by_name("CkanInstance")["zilina"]
    assert instance["metadata"]["title"]["sk"] == "Mesto Žilina"
    assert instance["spec"]["organizationDefault"] == "zilina"
