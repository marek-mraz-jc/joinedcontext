"""The derived dataset of slovensko-participacia (T-6505): proposals per process and state."""
import importlib.util
import json
import re
from pathlib import Path

import pytest
import yaml
from test_bystrica_indicators import bento, mapping_of, requires_docker

ROOT = Path(__file__).resolve().parents[1]
SEED = ROOT / "components/context-gateway/seed/slovensko"
PIPELINE = SEED / "slovensko-pipeline-participacia-statistika.yaml"
ENV = {"JC_SPACE": "slovensko-participacia"}
PROCESS = "urn:ngsi-ld:ParticipatoryProcess:slovensko-participacia:participacia-p"


def seed(name):
    return yaml.safe_load((SEED / name).read_text())


def prop(value):
    return {"type": "Property", "value": value}


def proposal(n, process, state=None, label=None):
    entity = {
        "id": f"urn:ngsi-ld:Proposal:slovensko-participacia:participacia-{n}",
        "type": "Proposal",
        "name": prop(f"Proposal {n}, which a person wrote"),
        "refParticipatoryProcess": {"type": "Relationship", "object": f"{PROCESS}{process}"},
        "source": prop("https://participacia.example"),
        "dataProvider": prop("Platforma joinedcontext"),
    }
    if state:
        entity["proposalState"] = prop(state)
    if label:
        entity["stateLabel"] = prop(label)
    return entity


READ = [
    {"id": f"{PROCESS}1", "type": "ParticipatoryProcess", "name": prop("Rozpočet 2026")},
    proposal(1, 1, "evaluating"),
    proposal(2, 1, "evaluating", "V hodnotení"),
    proposal(3, 1, "to_vote", "Postupuje do hlasovania"),
    proposal(4, 2, "accepted"),
    # A town whose API hides proposals gives the page's words and no token.
    proposal(5, 1, label="Vybavený"),
    proposal(6, 2, "own_token"),
]


@pytest.fixture(scope="module")
def rows():
    return bento(mapping_of(PIPELINE), READ, ENV)


@requires_docker
def test_each_process_and_state_is_one_row_and_the_rows_add_up(rows):
    counted = {(r["processName"]["value"], r["stateLabel"]["value"]): r["proposalCount"]["value"] for r in rows}
    assert counted == {
        ("Rozpočet 2026", "V hodnotení"): 2,
        ("Rozpočet 2026", "Postupuje do hlasovania"): 1,
        ("Rozpočet 2026", "Vybavený"): 1,
        (f"{PROCESS}2", "Prijatý"): 1,
        (f"{PROCESS}2", "own_token"): 1,
    }
    assert sum(counted.values()) == sum(e["type"] == "Proposal" for e in READ)
    assert not [r for r in rows if r.get("proposalState") is None and r["stateLabel"]["value"] != "Vybavený"]


@requires_docker
def test_a_row_keeps_its_id_from_run_to_run_and_validates_against_the_model(rows):
    import jsonschema

    again = bento(mapping_of(PIPELINE), list(reversed(READ)), ENV)
    assert sorted(r["id"] for r in rows) == sorted(r["id"] for r in again)
    assert len({r["id"] for r in rows}) == len(rows)
    assert all(re.fullmatch(r"urn:ngsi-ld:ProposalStatistic:slovensko-participacia:[A-Za-z0-9-]+", r["id"]) for r in rows)
    schema = json.loads((SEED / "slovensko-participacia.v1.schema.json").read_text())
    for row in rows:
        flat = {k: v["value"] if isinstance(v, dict) and "value" in v else v["object"] if isinstance(v, dict) else v for k, v in row.items()}
        flat.pop("expiresAt")
        jsonschema.validate(flat, {**schema["definitions"]["ProposalStatistic"], "definitions": schema["definitions"]})


@requires_docker
def test_no_text_a_person_wrote_reaches_the_rows(rows):
    assert "which a person wrote" not in json.dumps(rows)
    allowed = {"id", "type", "name", "processName", "proposalState", "stateLabel", "proposalCount",
               "refParticipatoryProcess", "source", "dataProvider", "dateModified", "expiresAt"}
    assert {key for row in rows for key in row} <= allowed


def test_the_read_asks_for_no_text_a_person_wrote():
    # `name` of a Proposal is read and dropped: it comes with the process title, by the same attribute.
    attrs = yaml.safe_load(PIPELINE.read_text())["spec"]["source"]["query"]["attrs"]
    assert set(attrs) == {"name", "proposalState", "stateLabel", "refParticipatoryProcess", "source", "dataProvider"}


def test_the_state_words_are_the_sites():
    spec = importlib.util.spec_from_file_location("decidim_generate", ROOT / "seeds/decidim/generate.py")
    generate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(generate)
    words = dict(re.findall(r'"(\w+)": "([^"]+)"', mapping_of(PIPELINE).split("let labels =")[1].split("\n")[0]))
    assert words == generate.OWN_STATE_LABELS


def test_the_dataset_is_public_read_only_and_published_as_links():
    endpoint = seed("slovensko-endpoint-participacia-statistika.yaml")["spec"]
    policy = seed("slovensko-policy-participacia-statistika-read.yaml")["spec"]
    assert endpoint["audience"] == "public"
    assert endpoint["policyRef"].endswith(":slovensko-participacia:participacia-statistika-read")
    assert endpoint["catalog"]["personalData"] == "does-not-contain"
    assert endpoint["publish"]["ckan"] == {"instanceRef": {"kind": "CkanInstance", "name": "slovensko"}, "license": "cc-by"}
    assert {e["type"] for i in policy["information"] for e in i["entities"]} == {"ProposalStatistic"}
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    # A subscriber needs the four subscription operations; nothing here writes an Entity.
    assert not [op for op in policy["operations"] if "Subscription" not in op and op.startswith(("create", "update", "delete", "append", "merge", "replace", "batch"))]


def test_the_harvest_dataset_keeps_its_own_types():
    types = {e["type"] for i in seed("slovensko-policy-participacia-read.yaml")["spec"]["information"] for e in i["entities"]}
    assert "ProposalStatistic" not in types
