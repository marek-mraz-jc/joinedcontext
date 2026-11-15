"""The two Banská Bystrica projects are two separate publishers (T-2305, T-2310, PF-54, PF-84, EP-15).

The region and the city publish separately and answer for their own numbers separately, so the
seed keeps them apart: two projects, a raw space and an indicator space each, neither writing
into the other's, and what crosses crosses as a named share that grants reads and nothing else.
These cases read the seed manifests directly; what the cluster does with them is `dev-smoke`'s.
"""

import json
from pathlib import Path

import pytest
import yaml

import open_data

# Reading, plus the subscriptions a WebSocket needs (T-4073); no entity write.
READ_AND_SUBSCRIBE = ["retrieveOps", "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"]
# The anonymous grant also lists types and attributes (T-4258, EP-116).
PUBLIC_READ = READ_AND_SUBSCRIBE[:1] + [
    "retrieveEntityTypes", "retrieveEntityTypeDetails", "retrieveEntityTypeInfo",
    "retrieveAttrTypes", "retrieveAttrTypeDetails", "retrieveAttrTypeInfo",
] + READ_AND_SUBSCRIBE[1:]

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
DEMO_USERS = yaml.safe_load(
    (Path(__file__).resolve().parent.parent / "components/keycloak/demo-users.yaml").read_text()
)
REGION = SEED / "bbsk"
CITY = SEED / "banskabystrica"
DOCS = "joinedcontext-docs/Development/10-banska-bystrica-contract.md"

# The artifacts of one LinkML source, committed once per space because a DataModel's artifacts
# sit beside its manifest. The example names its own space and is therefore not in this list.
SHARED_MODEL_FILES = (
    "statistical-observation.linkml.yaml",
    "statistical-observation.v1.schema.json",
    "statistical-observation.v1.context.jsonld",
    "statistical-observation.v1.md",
)


def documents(path: Path):
    return [doc for doc in yaml.safe_load_all(path.read_text()) if isinstance(doc, dict)]


def manifests(folder: Path, kind: str):
    for path in sorted(folder.glob("*.yaml")):
        if path.name.endswith("-bento.yaml") or path.name == "index.yaml":
            continue
        for doc in documents(path):
            if doc.get("kind") == kind:
                yield path, doc


def one(folder: Path, kind: str, name: str):
    for path, doc in manifests(folder, kind):
        if doc["metadata"]["name"] == name:
            return doc
    raise AssertionError(f"{folder.name} holds no {kind} named {name}")


def test_the_region_is_its_own_project_and_the_city_says_it_is_the_city():
    region = one(REGION, "Project", "bbsk")
    city = one(CITY, "Project", "banskabystrica")
    assert region["metadata"]["namespace"] == "org"
    # The region has roughly eight times the city's population; a title that does not say which
    # body it is is the error a dashboard cannot show.
    assert region["metadata"]["title"]["sk"] == "Banskobystrický samosprávny kraj"
    assert city["metadata"]["title"]["sk"] == "Mesto Banská Bystrica"
    assert region["spec"]["organizationRef"] == city["spec"]["organizationRef"] == "hel"


def test_each_projects_quotas_hold_what_it_declares_with_room_for_the_demo():
    """T-2873: the quota leaves room for the demo's work, never exactly what the seed holds."""
    for folder, project, spaces, public in (
        # The region's public endpoints: its raw rows' for the trend lines (T-2934), and the region
        # map's and its grids' (T-2784).
        # The sixth is the space of every other table of the catalogue (T-4255), and the other four
        # the registers' datasets, one per type (T-4344); the events of the region and of Slovakia (T-4895),
        # and the events dashboard's own (T-4897), and the region's own catalogue (T-5493).
        (REGION, "bbsk", ["bbsk-kraj", "bbsk-kpi", "bbsk-registre", "bbsk-otvorene-data", "bbsk-podujatia", "catalog"], 13),
        # The third public endpoint of the city is the air-quality App's own (T-2972), the fourth
        # and fifth the city map's and the data grids' (T-2782), then the public space's events,
        # schools and weather as their own datasets (T-4344), and OpenStreetMap's health places
        # and charging stations (T-4514), and the cadastre's parcels (T-4697), and the city's own
        # catalogue (T-5493). The conformance suite's space (F-BNT1-002) and the MQTT demo
        # sensors' (T-4857) have no public Endpoint.
        (CITY, "banskabystrica", ["ovzdusie", "banskabystrica-mesto", "banskabystrica-kpi", "banskabystrica-verejne",
                                  "banskabystrica-otvorene-data", "catalog", "conformance", "senzory"], 13),
    ):
        quotas = one(folder, "Project", project)["spec"]["quotas"]
        declared = {doc["metadata"]["name"] for _, doc in manifests(folder, "ContextSpace")}
        assert declared == set(spaces), folder.name
        assert quotas["contextSpaces"] >= len(spaces) + 90, folder.name
        endpoints = [doc for _, doc in manifests(folder, "Endpoint")]
        assert sum(e["spec"]["audience"] == "public" for e in endpoints) == public, folder.name
        assert quotas["publicEndpoints"] >= public + 90, folder.name


def test_every_new_space_pins_the_segment_its_ids_carry():
    # PF-84: the `{space}` segment is rendered as `{project}-{name}` unless it is pinned, and
    # every space in this seed pins it, so an id cannot change under anyone.
    for folder in (REGION, CITY):
        for path, space in manifests(folder, "ContextSpace"):
            # An organization's catalogue slice pins `{project}-catalog` (T-5493).
            name = space["metadata"]["name"]
            expected = f"{space['metadata']['namespace']}-catalog" if name == "catalog" else name
            assert space["spec"]["urnSegment"] == expected, path.name


def test_the_raw_spaces_are_not_public_and_the_indicators_of_the_region_are():
    assert one(REGION, "Endpoint", "bbsk-kraj")["spec"]["audience"] == "organization"
    assert one(CITY, "Endpoint", "banskabystrica-mesto")["spec"]["audience"] == "organization"
    assert one(REGION, "Endpoint", "bbsk-kpi")["spec"]["audience"] == "public"


def test_the_citys_indicators_reach_the_region_by_a_named_share_and_nothing_wider():
    endpoint = one(CITY, "Endpoint", "banskabystrica-kpi")
    assert endpoint["spec"]["audience"] == "project-list"
    assert endpoint["spec"]["allowedProjects"] == ["bbsk"]

    share = one(REGION, "SharedSpaceReference", "mesto-kpi")
    assert "endpointSlug" not in share["spec"], "inside one organization a share names its endpoint (EP-77)"
    assert share["spec"]["endpointRef"] == {"project": "banskabystrica", "name": "banskabystrica-kpi"}

    # Read and nothing else: the region cannot write a city number.
    grant = one(CITY, "Policy", "kpi-shared-read")
    assert grant["spec"]["assignee"] == {"kind": "group", "id": "bbsk-readers"}  # the derived group a bound person carries (T-6470)
    assert grant["spec"]["operations"] == READ_AND_SUBSCRIBE
    assert grant["spec"]["information"][0]["entities"] == [{"type": "KeyPerformanceIndicator"}]


def test_no_service_account_can_write_into_the_other_bodys_space():
    for folder, project, own in (
        (REGION, "bbsk", {"bbsk-kraj", "bbsk-kpi", "bbsk-registre", "bbsk-otvorene-data", "bbsk-podujatia"}),
        (CITY, "banskabystrica", {"banskabystrica-mesto", "banskabystrica-kpi", "banskabystrica-verejne", "ovzdusie",
                                  "banskabystrica-otvorene-data", "senzory"}),
    ):
        account = one(folder, "ServiceAccount", "pipelines")
        scopes = {role["scope"]["contextSpace"] for role in account["spec"]["roles"]}
        assert scopes == own, f"{project} writes only its own spaces"
        for path, policy in manifests(folder, "Policy"):
            if policy["spec"].get("assignee", {}).get("kind") != "serviceAccount":
                continue
            # The city's conformance account writes its fixtures in its own space (F-BNT1-002).
            suite = {"conformance"} if project == "banskabystrica" else set()
            assert policy["spec"]["contextSpaceRef"]["name"] in own | suite, path.name


def test_every_endpoint_slug_is_unguessable_and_unique():
    slugs = [
        (path.name, doc["spec"]["slug"])
        for folder in sorted(d for d in SEED.iterdir() if d.is_dir())
        for path, doc in manifests(folder, "Endpoint")
        if "slug" in doc["spec"]
    ]
    for name, slug in slugs:
        # EP-02: at least 128 bits, lowercase RFC 4648 base32 without padding.
        assert len(slug) >= 26, name
        assert set(slug) <= set("abcdefghijklmnopqrstuvwxyz234567"), name
    assert len({slug for _, slug in slugs}) == len(slugs), "two endpoints share a slug"


def test_the_indicator_spaces_grant_only_the_indicator_type():
    for folder, names in ((REGION, ["kpi-pipelines-write", "kpi-read"]),
                          (CITY, ["kpi-pipelines-write", "kpi-shared-read"])):
        for name in names:
            policy = one(folder, "Policy", name)
            assert policy["spec"]["information"][0]["entities"] == [
                {"type": "KeyPerformanceIndicator"}
            ], f"{folder.name}/{name}"


def test_both_copies_of_the_statistical_model_are_the_same_file():
    # One LinkML source, two spaces, so two committed copies of its artifacts. They are copied
    # rather than generated twice, and this is what stops them drifting.
    for name in SHARED_MODEL_FILES:
        assert (REGION / name).read_bytes() == (CITY / name).read_bytes(), name


def test_each_example_entity_names_its_own_space_and_the_contract_url_shape():
    for folder, space, domain in ((REGION, "bbsk-kraj", "bbsk.sk"),
                                  (CITY, "banskabystrica-mesto", "banskabystrica.sk")):
        example = json.loads((folder / "statistical-observation.v1.example.jsonld").read_text())
        assert example["type"] == "KeyPerformanceIndicator"
        segments = example["id"].split(":")
        assert segments[:5] == ["urn", "ngsi-ld", "KeyPerformanceIndicator", domain, space], folder.name
        assert len(segments) == 6, f"{DOCS} fixes four segments after the scheme"
        # The localId is the publisher's own key: cube, territory, period, indicator.
        cube, area, period, indicator = segments[5].split("-")[:4]
        assert example["dataSet"]["value"] == cube
        assert example["refArea"]["value"] == area
        assert example["refPeriod"]["value"] == period
        assert example["indicator"]["value"] == indicator
        # A row that cannot be fetched again is not a row.
        assert example["source"]["value"].startswith("https://")
        assert cube in example["source"]["value"]


def key_values(entity: dict) -> dict:
    """The entity as `keyValues`, which is the shape the generated JSON Schema describes.

    The committed example is normalised NGSI-LD, one object per attribute; the LinkML generator
    renders the simplified projection. Reducing the one to the other is what makes the two
    comparable, and it is the same reduction the gateway serves under `options=keyValues`.
    """
    out = {}
    for name, attribute in entity.items():
        if not isinstance(attribute, dict) or "type" not in attribute:
            out[name] = attribute
        elif attribute["type"] == "Relationship":
            out[name] = attribute["object"]
        else:
            out[name] = attribute["value"]
    return out


def test_every_example_validates_against_the_model_it_was_generated_from():
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((REGION / "statistical-observation.v1.schema.json").read_text())
    validator = jsonschema.Draft7Validator(schema["definitions"]["KeyPerformanceIndicator"])
    for folder in (REGION, CITY):
        example = json.loads((folder / "statistical-observation.v1.example.jsonld").read_text())
        # The wire carries `@context`; the model describes the entity inside it.
        del example["@context"]
        errors = sorted(validator.iter_errors(key_values(example)), key=str)
        assert not errors, f"{folder.name}: {[e.message for e in errors]}"


def test_every_example_is_a_valid_smart_data_model_key_performance_indicator():
    """T-4329: the raw cell is FIWARE's KeyPerformanceIndicator, its number the model's `kpiValue`,
    checked against the schema.json vendored at a pinned commit (scripts/open-data/sdm)."""
    for folder in (REGION, CITY, SEED / "zilina"):
        example = json.loads((folder / "statistical-observation.v1.example.jsonld").read_text())
        del example["@context"]
        assert "kpiValue" in example, folder.name
        assert open_data.sdm_errors("dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator", example) == [], folder.name
        assert open_data.sdm_errors("dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator",
                                    {**example, "calculationPeriod": {"type": "Property", "value": "2023"}}), \
            "a value of the wrong shape for a model attribute fails"


def test_an_attribute_the_model_does_not_declare_is_refused():
    # The model closes the object, which is what keeps an invented column out of a raw space.
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((REGION / "statistical-observation.v1.schema.json").read_text())
    example = json.loads((REGION / "statistical-observation.v1.example.jsonld").read_text())
    del example["@context"]
    invented = key_values(example) | {"territory": "kraj"}
    errors = list(jsonschema.Draft7Validator(schema["definitions"]["KeyPerformanceIndicator"]).iter_errors(invented))
    assert errors and "territory" in errors[0].message


def test_the_seed_index_lists_every_file_beside_it():
    for folder in (REGION, CITY):
        index = yaml.safe_load((folder / "index.yaml").read_text())
        listed = set(index)
        on_disk = {
            path.name
            for path in folder.iterdir()
            if path.is_file() and path.name not in {"index.yaml", "README.md"}
        }
        assert listed == on_disk, f"{folder.name}: {listed ^ on_disk}"
        assert len(set(index.values())) == len(index), f"{folder.name}: two files share a path"


# The steward's note (T-2434): the one attribute of a published row a person may write, and the
# grants around it. Every figure in these spaces belongs to a publisher and is rewritten by the
# next pipeline run, so an application that offered one for editing would lose what a person
# typed without telling them. The note is what makes the two records applications possible.
NOTE = "stewardNote"

# What the models' own artifacts are, per folder: the LinkML source and the three generated or
# hand-written files that a published DataModel commits beside it (DM-02).
MODEL = "statistical-observation"


def model_slots(folder: Path) -> list[str]:
    """The slots the class declares, in the order the LinkML source lists them."""
    source = yaml.safe_load((folder / f"{MODEL}.linkml.yaml").read_text())
    return list(source["classes"]["KeyPerformanceIndicator"]["slots"])


def policies_of(folder: Path, space: str) -> dict[str, dict]:
    return {
        doc["metadata"]["name"]: doc["spec"]
        for _, doc in manifests(folder, "Policy")
        if doc["spec"]["contextSpaceRef"]["name"] == space
    }


RAW = ((REGION, "bbsk-kraj", "kraj"), (CITY, "banskabystrica-mesto", "mesto"))


def test_both_models_declare_the_one_attribute_no_pipeline_writes():
    for folder, _, _ in RAW:
        assert NOTE in model_slots(folder), folder.name
    # A pipeline that wrote it would take the words back on its next run without a word. None
    # does: the mappings produce the publisher's own columns and stop there. The upsert itself
    # is `?options=update` (joinedcontext-portal src/reconciler/streams.rs), so a run that does
    # not mention the note leaves the note where it is.
    for folder in (REGION, CITY):
        for path in sorted(folder.glob("*-bento.yaml")):
            assert NOTE not in path.read_text(), path.name


def test_every_slot_the_model_declares_reaches_the_artifacts_beside_it():
    # The generator's files are committed, not rendered at read time (seed README), so this is
    # what catches a source edited without `gen_json_schema.py` and `gen_context.py` being run.
    for folder, _, _ in RAW:
        schema = json.loads((folder / f"{MODEL}.v1.schema.json").read_text())
        context = json.loads((folder / f"{MODEL}.v1.context.jsonld").read_text())["@context"]
        docs = (folder / f"{MODEL}.v1.md").read_text()
        declared = schema["definitions"]["KeyPerformanceIndicator"]["properties"]
        for slot in model_slots(folder):
            assert slot in declared, f"{folder.name}: {slot} is not in the JSON Schema"
            assert slot in context, f"{folder.name}: {slot} is not in the @context"
            assert f"| `{slot}` |" in docs, f"{folder.name}: {slot} is not in the docs page"


def test_the_note_is_optional_and_bounded_and_the_figures_stay_required():
    for folder, _, _ in RAW:
        entity = json.loads((folder / f"{MODEL}.v1.schema.json").read_text())["definitions"][
            "KeyPerformanceIndicator"
        ]
        assert NOTE not in entity["required"], "a row exists before anybody annotates it"
        assert "kpiValue" in entity["required"], folder.name
        # `value` is an NGSI-LD core term; a slot of that name would collide with it (T-3964).
        assert "value" not in entity["properties"], folder.name
        note = entity["properties"][NOTE]
        # The gateway validates every write against this schema (CC-12), so the bound on a
        # free-text attribute is here and not only in the application that offers the field.
        assert note["pattern"] == "^[^<>]{0,500}$", folder.name
        assert note["type"] == ["string", "null"], folder.name


def test_serving_the_note_is_a_minor_version_of_a_published_model():
    for folder, _, _ in RAW:
        model = one(folder, "DataModel", MODEL)
        assert model["spec"]["version"] == "1.1.0", folder.name
        assert model["spec"]["lifecycle"] == "published", folder.name
        # An added optional attribute is backwards compatible, so the major version and the
        # artifact filenames it names do not move.
        assert model["spec"]["artifacts"]["jsonSchema"] == f"./{MODEL}.v1.schema.json", folder.name
        assert f"`{MODEL}` 1.1.0" in (folder / f"{MODEL}.v1.md").read_text(), folder.name


def test_a_person_may_write_the_note_and_may_not_write_a_figure():
    for folder, space, name in RAW:
        grant = policies_of(folder, space)[f"{name}-steward-note"]
        assert grant["operations"] == ["updateAttrs"], f"{folder.name}: one write and no other"
        assert grant["assignee"]["kind"] == "user", folder.name
        assert grant["information"] == [
            {"entities": [{"type": "KeyPerformanceIndicator"}], "propertyNames": [NOTE]}
        ], f"{folder.name}: the grant names the attribute, not the type (EP-14, GW16)"


def test_no_grant_in_these_projects_lets_a_person_write_a_published_figure():
    # The property that has to hold however many policies are added later: a human write grant
    # over a raw space reaches the note and nothing else, so `kpiValue` cannot be edited by anyone
    # whose typing the next pipeline run would overwrite (AP-62, GW17).
    writes = {"createEntity", "updateEntity", "appendAttrs", "updateAttrs", "deleteAttrs",
              "deleteEntity", "mergeEntity", "replaceEntity", "replaceAttrs", "updateOps",
              "redirectionOps", "createBatch", "upsertBatch", "updateBatch", "deleteBatch"}
    for folder, space, _ in RAW:
        for name, spec in policies_of(folder, space).items():
            if spec["assignee"]["kind"] not in {"user", "group", "role"}:
                continue
            if not writes.intersection(spec["operations"]):
                continue
            granted = [info.get("propertyNames") for info in spec.get("information", [])]
            assert granted == [[NOTE]], f"{folder.name}/{name} grants a person more than the note"


def test_the_steward_reads_the_columns_the_records_application_shows():
    for folder, space, name in RAW:
        grant = policies_of(folder, space)[f"{name}-steward-read"]
        assert grant["operations"] == READ_AND_SUBSCRIBE, f"{folder.name}: reading and nothing else"
        served = grant["information"][0]["propertyNames"]
        # Every column the model declares, because a filter names an attribute and an attribute
        # the grant withholds is a filter the endpoint refuses rather than answers (T-1862).
        assert sorted(served) == sorted(model_slots(folder)), folder.name
        assert grant["information"][0]["entities"] == [{"type": "KeyPerformanceIndicator"}]


def test_the_raw_endpoints_stay_closed_to_the_public_while_the_note_opens():
    # The note is a grant on a space, not a wider door: both raw endpoints keep the audience
    # they had, and the city's public air endpoint is still the only public one it has.
    assert one(REGION, "Endpoint", "bbsk-kraj")["spec"]["audience"] == "organization"
    assert one(CITY, "Endpoint", "banskabystrica-mesto")["spec"]["audience"] == "organization"
    assert one(CITY, "Endpoint", "public-air")["spec"]["audience"] == "public"
def test_a_public_endpoint_serves_a_tabular_representation_and_says_what_it_is():
    """T-2432, EP-62..EP-64: what the open-data catalogue needs from a public endpoint.

    A dataset's preview and its filtered API are the endpoint's own (the CKAN DataStore sheet is
    gone, T-5060). An endpoint that serves no tabular representation can carry links and nothing
    else. The dataset's title
    and notes are the endpoint's `metadata.title` and `metadata.description`, mapped through its
    DCAT-AP record, so an endpoint without them lands in the catalogue named after its manifest.
    """
    public = [
        (folder.name, path, doc)
        for folder in (REGION, CITY)
        for path, doc in manifests(folder, "Endpoint")
        if doc["spec"].get("audience") == "public"
        # An App's own endpoint is its door, not a dataset: it publishes nothing to a catalogue
        # and serves the representations the App reads (T-2972).
        and doc["metadata"].get("annotations", {}).get("joinedcontext.com/generated-by") != "portal/app-reconciler"
    ]
    assert public, "neither project has a public endpoint to publish"
    for project, path, doc in public:
        where = f"{project}/{path.name}"
        representations = doc["spec"].get("enabledRepresentations", [])
        assert "csv" in representations, f"{where}: no tabular representation, so no sheet is possible"
        for field in ("title", "description"):
            text = doc["metadata"].get(field)
            assert isinstance(text, dict) and text.get("sk"), f"{where}: no Slovak {field} for the dataset page"
            assert text.get("en"), f"{where}: no English {field} beside the Slovak one"


def test_every_space_is_named_by_a_model_so_the_portal_can_say_what_is_in_it():
    """A space no model names shows a viewer nothing, however full it is (T-2450).

    The Portal's look-inside view lists a space's entity types from the DataModels that name it:
    `DataModel.spec.contextSpaceRef` is the required half of the link, and a space's own
    `dataModelRef` is an optional pointer at the primary one. Both indicator spaces held their
    indicators for a day with no model naming them, so the view said "nothing here" over a full
    space and the demonstration's third step had nothing to show.
    """
    for folder in (REGION, CITY):
        spaces = {doc["metadata"]["name"] for _, doc in manifests(folder, "ContextSpace")}
        named = {doc["spec"]["contextSpaceRef"] for _, doc in manifests(folder, "DataModel")}
        assert spaces, f"{folder.name} declares no ContextSpace"
        assert spaces <= named, f"{folder.name}: no DataModel names {sorted(spaces - named)}"


def test_the_indicator_model_states_the_contract_the_platform_enforces():
    """The published schema closes the object and requires all seven; the model says the same.

    A model that let an indicator carry `state` or `threshold` would describe a document the
    gateway refuses before it is stored, which is worse than no model at all (Development/10 §4).
    """
    required = {"name", "currentValue", "calculationPeriod", "calculationFormula",
                "derivedFrom", "computedBy", "updatedAt"}
    for folder in (REGION, CITY):
        schema = json.loads((folder / "key-performance-indicator.v1.schema.json").read_text())
        entity = (schema.get("$defs") or schema["definitions"])["KeyPerformanceIndicator"]
        assert entity["additionalProperties"] is False, f"{folder.name}: the object is left open"
        assert required <= set(entity["required"]), f"{folder.name}: {sorted(required - set(entity['required']))} not required"
        assert not ({"state", "threshold"} & set(entity["properties"])), \
            f"{folder.name}: the model declares an attribute the gateway refuses"


def test_every_space_the_portal_shows_is_one_its_own_steward_may_read():
    """T-2456: a body's own person reads the body's own space, indicators included.

    `banskabystrica-kpi` had two grants and neither was a human's — the pipelines write and the
    region reads through the share — so the city's steward opened the space in the Portal and got
    `403` for every row of the numbers the city itself publishes. The demonstration walks into
    each of these four spaces, so each one needs a read a signed-in person actually holds.
    """
    for folder in (REGION, CITY):
        for _, space in manifests(folder, "ContextSpace"):
            name = space["metadata"]["name"]
            readers = [
                (policy_name, spec["assignee"])
                for policy_name, spec in policies_of(folder, name).items()
                if "retrieveOps" in spec["operations"]
            ]
            human = [
                policy_name
                for policy_name, assignee in readers
                if assignee["kind"] == "user"
                or (assignee["kind"] == "role" and assignee["id"] == "public")
            ]
            assert human, f"{name} is readable by no signed-in person: {readers}"

            # And the person the grant names gets past the door in front of it. The gateway
            # resolves a human's project out of the `groups` claim and refuses an endpoint whose
            # audience is a list of projects before it reads any Policy at all
            # (context-gateway app.rs, EP-14, EP-15), so a read granted to somebody in no group
            # of the project is a `403` that looks exactly like a missing grant.
            endpoints = [
                spec
                for _, doc in manifests(folder, "Endpoint")
                for spec in [doc["spec"]]
                if spec["contextSpaceRef"] == name
            ]
            for endpoint in endpoints:
                if endpoint["audience"] != "project-list":
                    continue
                admitted = {folder.name, *endpoint.get("allowedProjects", [])}
                for policy_name, assignee in readers:
                    if assignee["kind"] != "user":
                        continue
                    # The seeds name a demo person by their fixed Keycloak id (PF-126).
                    user = next((n for n, u in DEMO_USERS.items() if u["id"] == assignee["id"]), assignee["id"])
                    groups = set(DEMO_USERS.get(user, {}).get("groups", []))
                    assert groups & admitted, (
                        f"{policy_name} grants {user} a read on {name}, whose endpoint admits"
                        f" {sorted(admitted)} and whose groups are {sorted(groups)}"
                    )


def test_the_application_reading_both_bodies_is_a_published_static_app_on_the_regions_space():
    """T-2457: the Portal serves only an App the configuration repository holds, and it configures
    it with the Endpoints on the spaces its dataNeeds name plus those its project's shares name."""
    app = one(REGION, "App", "bbsk-ukazovatele")
    assert app["metadata"]["namespace"] == "bbsk"
    assert app["spec"]["kind"] == "ui"
    assert app["spec"]["lifecycle"] == "published"
    spaces = [need["contextSpaceRef"]["name"] for need in app["spec"]["dataNeeds"]]
    # Its own space first; each further space is read through that space's public Endpoint of the
    # same project and compiles into no grant (AP-04, T-2933): the district outlines, and the
    # yearly rows the trend lines are drawn from (T-2934).
    assert spaces == ["bbsk-kpi", "bbsk-registre", "bbsk-kraj"]
    assert one(REGION, "Endpoint", "bbsk-kpi")["spec"]["contextSpaceRef"] == "bbsk-kpi"
    register = one(REGION, "Endpoint", "bbsk-registre")["spec"]
    assert register["contextSpaceRef"] == "bbsk-registre" and register["audience"] == "public"
    raw = one(REGION, "Endpoint", "bbsk-kraj-verejne")["spec"]
    assert raw["contextSpaceRef"] == "bbsk-kraj" and raw["audience"] == "public" and "publish" not in raw
    further = [need for need in app["spec"]["dataNeeds"] if need["contextSpaceRef"]["name"] != "bbsk-kpi"]
    assert [need["types"] for need in further] == [["AdministrativeArea"], ["KeyPerformanceIndicator"]]
    assert all("roles" not in need for need in further)
    # The public read of the raw rows names every published column of the statistics office and
    # never the steward's own note, which is the region's annotation and not open data.
    public = one(REGION, "Policy", "kraj-public-read")["spec"]
    # and its history, the 30 days the space keeps (T-4372)
    assert public["assignee"] == {"kind": "role", "id": "public"}
    assert public["operations"] == PUBLIC_READ + ["queryTemporal", "retrieveTemporal"]
    assert public["temporalQ"] == "timerel=after;timeAt=P-30D"
    (rule,) = public["information"]
    assert "stewardNote" not in rule["propertyNames"]
    assert set(further[1]["attrs"]) <= set(rule["propertyNames"])
    # It reads and never writes (AP-07).
    operations = {op for need in app["spec"]["dataNeeds"] for op in need["operations"]}
    assert operations <= {"queryEntity", "retrieveEntity"}, operations


def test_the_citys_records_are_a_published_static_app_the_portal_image_ships():
    """T-2916: the city's one application on dev is the records grid over banskabystrica-mesto.
    The Portal image carries the bundle, so the App names no repository; its one write is the
    steward's note, which a seeded Policy grants and nothing else does."""
    app = one(CITY, "App", "banskabystrica-zaznamy")
    assert app["metadata"]["namespace"] == "banskabystrica"
    assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
    assert app["spec"]["kind"] == "ui"
    assert app["spec"]["lifecycle"] == "published"
    assert app["spec"]["source"] == {"path": "./apps/banskabystrica-zaznamy"}
    read, write = app["spec"]["dataNeeds"]
    assert {need["contextSpaceRef"]["name"] for need in (read, write)} == {"banskabystrica-mesto"}
    assert one(CITY, "ContextSpace", "banskabystrica-mesto")
    assert read["operations"] == ["queryEntity", "retrieveEntity"]
    # The write in a need of its own, on the note alone: the compiled grant names what the need
    # names for every operation it holds (AP-62, T-3596).
    assert (write["operations"], write["attrs"]) == (["updateAttrs"], ["stewardNote"])
    note = one(CITY, "Policy", "mesto-steward-note")["spec"]
    assert note["operations"] == ["updateAttrs"]
    assert note["information"][0]["propertyNames"] == ["stewardNote"]


def test_the_regions_records_are_seeded_with_the_endpoint_and_policy_the_portal_compiles():
    """T-3596: bbsk-zaznamy was published in the Portal repository and never seeded, so dev had no
    App, no certificate and no endpoint for it. The seed carries the App, which the Portal image
    ships, and the Endpoint and Policy the Portal compiles for it, on bbsk-kraj and nowhere else;
    the one write is the steward's note."""
    app = one(REGION, "App", "bbsk-zaznamy")
    assert app["metadata"]["namespace"] == "bbsk"
    assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
    assert app["spec"]["lifecycle"] == "published"
    assert app["spec"]["source"] == {"path": "./apps/bbsk-zaznamy"}
    space = app["spec"]["dataNeeds"][0]["contextSpaceRef"]["name"]
    assert space == "bbsk-kraj" and one(REGION, "ContextSpace", space)

    generated = {"joinedcontext.com/generated-by": "portal/app-reconciler"}
    endpoint = one(REGION, "Endpoint", "app-bbsk-zaznamy")
    assert endpoint["metadata"]["annotations"] == generated
    assert endpoint["spec"]["contextSpaceRef"] == {"kind": "ContextSpace", "name": space}
    # Not public: the App's visibility is the project, so is its endpoint's audience.
    assert endpoint["spec"]["audience"] == "project-list"
    assert endpoint["spec"]["allowedProjects"] == ["bbsk"]

    # One Policy per need, each exactly its need: the columns to read, and the note alone to write.
    # A need with `roles` goes to that role of the endpoint alone (AP-96): the note to the
    # steward, never to every member the endpoint admits (T-5091).
    def name(n, need):
        return "-".join([f"app-bbsk-zaznamy-{n}", *need.get("roles", [])])

    def assignee(need):
        return "/".join(["endpoint:bbsk/app-bbsk-zaznamy", *need.get("roles", [])])

    grants = {doc["metadata"]["name"]: doc for _, doc in manifests(REGION, "Policy")
              if doc["spec"]["assignee"]["id"].startswith("endpoint:bbsk/app-bbsk-zaznamy")}
    needs = app["spec"]["dataNeeds"]
    assert sorted(grants) == sorted(name(n, need) for n, need in enumerate(needs, 1))
    for n, need in enumerate(needs, 1):
        grant = grants[name(n, need)]
        assert grant["spec"]["assignee"] == {"kind": "role", "id": assignee(need)}
        assert grant["metadata"]["annotations"] == generated
        spec = grant["spec"]
        assert spec["contextSpaceRef"]["name"] == need["contextSpaceRef"]["name"] == space
        (rule,) = spec["information"]
        assert [entity["type"] for entity in rule["entities"]] == need["types"]
        assert rule["propertyNames"] == need["attrs"]
        assert spec["operations"] == need["operations"]
    writes = [grant["spec"] for grant in grants.values() if "updateAttrs" in grant["spec"]["operations"]]
    assert [(spec["operations"], spec["information"][0]["propertyNames"]) for spec in writes] == [
        (["updateAttrs"], [NOTE])]

    index = yaml.safe_load((REGION / "index.yaml").read_text())
    assert index["bbsk-app-zaznamy.yaml"] == "projects/bbsk/apps/bbsk-zaznamy/app.yaml"
    assert index["bbsk-app-zaznamy-endpoint.yaml"].startswith(f"projects/bbsk/spaces/{space}/endpoints/")
    assert index["bbsk-app-zaznamy-policy-note.yaml"].endswith("/app-bbsk-zaznamy-2-steward.yaml")
    endpoint_roles = {role["name"]: role["subjects"] for role in endpoint["spec"]["roles"]}
    assert endpoint_roles == {"steward": [{"group": "bbsk-zaznamy-steward"}]}
    for key in ("bbsk-app-zaznamy-policy.yaml", "bbsk-app-zaznamy-policy-note.yaml"):
        assert index[key].startswith(f"projects/bbsk/spaces/{space}/policies/"), key

def test_the_citys_air_quality_is_a_public_static_app_on_its_own_endpoint():
    """T-2916, T-2949, T-2972: the public screen reads the city's public space, where the EEA
    pipelines write station SK0263A, read only, through its own Endpoint and the Policies the
    Portal compiles for it, seeded beside it because the seed commits the App past the Portal's
    door. Without them the static host hands the App the space's public endpoint, whose grant
    has no temporal read, and the day of history answers 404."""
    app = one(CITY, "App", "banskabystrica-ovzdusie")
    assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
    assert app["spec"]["visibility"] == "public"
    assert app["spec"]["lifecycle"] == "published"
    (need,) = app["spec"]["dataNeeds"]
    space = need["contextSpaceRef"]["name"]
    # The space the live readings go to, never the conformance suite's seeded stations.
    assert space == "banskabystrica-verejne", space
    # `urn:ngsi-ld:Endpoint:{org}:{space}:{name}`: both streams land in this space.
    writes = {doc["spec"]["targetEndpoint"].split(":")[4] for _, doc in manifests(CITY, "Pipeline")
              if doc["metadata"]["name"] in {"ovzdusie-pm10", "ovzdusie-pm25"}}
    assert writes == {space}, writes
    # Read only: the entities and one station's history, the panel's `GET /temporal/entities/{id}`.
    assert set(need["operations"]) == {"queryEntity", "retrieveEntity", "retrieveTemporal"}

    generated = {"joinedcontext.com/generated-by": "portal/app-reconciler"}
    endpoint = one(CITY, "Endpoint", "app-banskabystrica-ovzdusie")
    assert endpoint["metadata"]["annotations"] == generated
    assert endpoint["spec"]["contextSpaceRef"] == {"kind": "ContextSpace", "name": space}
    assert endpoint["spec"]["audience"] == "public" and endpoint["spec"]["callerRole"] is True

    role = {"kind": "role", "id": "endpoint:banskabystrica/app-banskabystrica-ovzdusie"}
    grants = [doc for _, doc in manifests(CITY, "Policy") if doc["spec"]["assignee"] == role]
    assert {doc["metadata"]["name"] for doc in grants} == {
        "app-banskabystrica-ovzdusie-1", "app-banskabystrica-ovzdusie-1-history"}
    granted = set()
    for grant in grants:
        spec = grant["spec"]
        assert grant["metadata"]["annotations"] == generated
        assert spec["contextSpaceRef"]["name"] == space
        # No grant wider than the type and the attributes the screen shows.
        (rule,) = spec["information"]
        assert [entity["type"] for entity in rule["entities"]] == need["types"]
        assert rule["propertyNames"] == need["attrs"]
        # The history is the App's window and nothing older.
        temporal = set(spec["operations"]) & {"queryTemporal", "retrieveTemporal"}
        if temporal:
            assert spec["temporalQ"] == "timerel=after;timeAt=P-1D", spec
        granted |= set(spec["operations"])
    assert granted == set(need["operations"]), granted


# What the city and the region cleared for the open-data catalogue (T-2407, user 2026-09-21):
# these two and nothing else, until the other endpoints are reviewed. `public-air` publishes by the
# owner's decision of 2026-09-25 although its space still holds seeded test stations (T-2407).
# T-2783 (owner, 2026-09-24: BBSK's open data "republished to CKAN") adds the region's registers,
# which are the region's own CC BY-SA publication and hold no person.
# T-2781 ("republished to CKAN", by the rule of T-2780) adds the city's public space: events, the
# school map and the EEA's station readings, all CC BY 4.0, none of them about a person.
# T-4344 splits both spaces into one dataset per type; the public space's air-quality reading is
# public-air's, so it is not published twice.
# T-4514 (owner, 2026-10-17: more sources, "the CKAN dataset carries ODbL") adds OpenStreetMap's
# health places and charging stations in the district, ODbL 1.0, none of them about a person.
# T-4697 adds the cadastre's parcels of Banská Bystrica, CC BY 4.0 ÚGKK SR, which carry no owner.
CLEARED = {
    ("banskabystrica", "public-air"), ("banskabystrica", "banskabystrica-podujatia"),
    ("banskabystrica", "banskabystrica-skoly"), ("banskabystrica", "banskabystrica-pocasie"),
    ("banskabystrica", "banskabystrica-kataster"),
    ("bbsk", "bbsk-kpi"), ("bbsk", "bbsk-hranice"), ("bbsk", "bbsk-organizacie"), ("bbsk", "bbsk-zariadenia"),
    ("bbsk", "bbsk-mosty"),
    # The events, cleared by the owner (2026-10-20): the region's and Slovakia's (T-4895).
    ("bbsk", "bbsk-podujatia"),
    # Off the catalogue while empty on dev (T-5767, owner 2026-10-26): banskabystrica-nabijanie
    # (every charging station refused) and banskabystrica catalog (no Dataset record written).
    # Off while empty on prod (T-5788, owner 2026-10-26): banskabystrica-zdravie and bbsk catalog.
}


def test_only_the_cleared_endpoints_publish_each_to_its_own_bodys_catalogue():
    """T-2407: a publication is a step past "reachable at a URL", so it is the city's call."""
    published = set()
    for folder in (CITY, REGION):
        instances = {doc["metadata"]["name"]: doc for _, doc in manifests(folder, "CkanInstance")}
        for _, endpoint in manifests(folder, "Endpoint"):
            ckan = endpoint["spec"].get("publish", {}).get("ckan")
            if ckan is None:
                continue
            project = endpoint["metadata"]["namespace"]
            published.add((project, endpoint["metadata"]["name"]))
            assert endpoint["spec"]["audience"] == "public", endpoint["metadata"]["name"]
            instance = instances[ckan["instanceRef"]["name"]]
            # Each body lands in its own organization, never in hel-fi's.
            assert instance["metadata"]["namespace"] == project
            assert ckan.get("organization", instance["spec"]["organizationDefault"]) == project
            # T-5060: no DataStore sheet is requested of any endpoint.
            assert "datastore" not in ckan, endpoint["metadata"]["name"]
            # The token is a reference, never a value.
            assert set(instance["spec"]["apiTokenRef"]) <= {"name", "key", "envVar"}
    assert published == CLEARED


def test_only_the_sole_writer_of_a_space_expires_and_its_policy_grants_the_sweep():
    """Stale-entity expiry deletes by space and type, never by writer (PL-64, T-2887).

    Nothing on an entity says which pipeline wrote it, so the Portal refuses an expiry while
    another Pipeline of the project writes the same space, a type outside the space's model, or a
    sweep no Policy of the pipelines account grants (queryBatch to read the timestamps, deleteBatch
    to remove). The seed is applied past that door, so the same rules are asserted here: a seed
    the Portal would refuse never reaches dev. The region's indicators are the one demonstration.
    """
    expiring = []
    for folder in sorted(p for p in SEED.iterdir() if p.is_dir()):
        endpoints = {
            doc["metadata"]["name"]: doc["spec"]["contextSpaceRef"]
            for _, doc in manifests(folder, "Endpoint")
        }

        def space_of(pipeline):
            outputs = pipeline["spec"].get("outputs") or [pipeline["spec"]]
            names = [o["targetEndpoint"].rsplit(":", 1)[-1] for o in outputs if "targetEndpoint" in o]
            spaces = [endpoints.get(name) for name in names]
            return [s["name"] if isinstance(s, dict) else s for s in spaces if s]

        pipelines = [doc for _, doc in manifests(folder, "Pipeline")]
        for pipeline in pipelines:
            expiry = pipeline["spec"].get("expiry")
            if expiry is None:
                continue
            name = pipeline["metadata"]["name"]
            expiring.append(f"{folder.name}/{name}")
            (space,) = space_of(pipeline)
            others = [p["metadata"]["name"] for p in pipelines if p is not pipeline and space in space_of(p)]
            assert not others, f"{name} expires {space}, which {others} also write"
            classes = {
                c
                for _, model in manifests(folder, "DataModel")
                if model["spec"]["contextSpaceRef"] == space
                for c in model["spec"]["classes"]
            }
            assert set(expiry["types"]) <= classes, f"{name}: {expiry['types']} not in {classes}"
            for operation in ("queryBatch", "deleteBatch"):
                for entity_type in expiry["types"]:
                    assert any(
                        policy["spec"]["contextSpaceRef"] in (space, {"kind": "ContextSpace", "name": space})
                        and policy["spec"]["assignee"] == {"kind": "serviceAccount", "id": "pipelines"}
                        and policy["spec"].get("effect", "permission") == "permission"
                        and operation in policy["spec"]["operations"]
                        and not any(k in policy["spec"] for k in ("q", "scopeQ", "geoQ", "temporalQ"))
                        and (
                            not policy["spec"].get("information")
                            or any(
                                selector == {"type": entity_type}
                                for info in policy["spec"]["information"]
                                for selector in info["entities"]
                            )
                        )
                        for _, policy in manifests(folder, "Policy")
                    ), f"{name}: no pipelines Policy grants {operation} on {entity_type} in {space}"
    # Off by default: exactly these carry it, every other pipeline deletes nothing. The region's
    # indicators, the city's air readings, whose 13 seeded test stations leave public-air that
    # way (T-3120), and Žilina's two sole writers, its indicators and the university's works
    # (T-3138).
    assert expiring == ["banskabystrica/public-air", "bbsk/ukazovatele", "zilina/drepo", "zilina/ukazovatele"]
    assert one(REGION, "Pipeline", "ukazovatele")["spec"]["expiry"] == {
        "after": "21d",
        "types": ["KeyPerformanceIndicator"],
    }


def test_no_two_published_spaces_or_endpoints_of_the_slovak_bodies_share_a_title():
    """T-4237 (photo audit T-4161 #14): "Mesto — verejné údaje" and "Mesto — verejné dáta" stood
    side by side in the catalogue, told apart only by the small publisher line. Every space and
    Endpoint title names its own body, in both languages, so no two are alike."""
    seen = {}
    for folder in ("banskabystrica", "bbsk", "zilina"):
        for path in sorted((SEED / folder).glob("*.yaml")):
            for doc in documents(path):
                if not isinstance(doc, dict) or doc.get("kind") not in ("ContextSpace", "Endpoint"):
                    continue
                # An App's Endpoint is the Portal's compile and has no catalogue record of its own.
                if (doc["metadata"].get("annotations") or {}).get("joinedcontext.com/generated-by") == "portal/app-reconciler":
                    continue
                title = doc["metadata"].get("title") or {}
                for lang, text in title.items():
                    assert not text.startswith(("Mesto —", "Kraj —", "City,", "Region,")), f"{path.name}: {text}"
                    key = (doc["kind"], lang, text)
                    assert key not in seen, f"{path.name} and {seen.get(key)}: {text}"
                    seen[key] = path.name
