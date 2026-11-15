"""Every attribute a visitor sees has a heading in their language, never its camelCase name (T-4682, T-5332).

Every slot of every seed model carries sk, cs, en and de labels. A generated catalogue model takes the
source column's own Slovak heading where the column is one; the rest comes from
scripts/open-data/labels.tsv. A language a machine wrote is named in the label's `generated`
annotation, so a reviewer sees which words nobody has checked yet.
"""

import importlib.util
import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
# An attribute name as a model writes it (availableBikeNumber); YouTube or OvZP are words a source uses.
CAMEL = re.compile(r"\b[a-z]+[A-Z]")
LANGS = ("sk", "cs", "en", "de")

spec = importlib.util.spec_from_file_location("generate_od", ROOT / "scripts/open-data/generate.py")
generate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(generate)


def label(node: dict | None) -> dict | None:
    found = ((node or {}).get("annotations") or {}).get("label")
    return found.get("value", found) if isinstance(found, dict) else None


@pytest.mark.parametrize("model", ["bbsk/bbsk-registre", "zilina/zilina-verejne", "zilina/zilina-doprava"])
def test_every_slot_and_enum_value_of_a_hand_model_reads_in_sk_and_en(model):
    schema = yaml.safe_load((SEED / f"{model}.linkml.yaml").read_text())
    for name, slot in schema["slots"].items():
        words = label(slot)
        assert words, f"{model}: {name} has no label"
        for lang in ("sk", "en"):
            assert words.get(lang) and words[lang] != name and not CAMEL.search(words[lang]), (model, name, lang)
    for enum, about in (schema.get("enums") or {}).items():
        for value, said in about["permissible_values"].items():
            words = label(said)
            assert words and words.get("sk") and words.get("en") and words["sk"] != value, (enum, value)


def test_the_bike_stations_the_widget_lists_read_in_sk_and_en():
    schema = yaml.safe_load((SEED / "helsinki/helsinki.linkml.yaml").read_text())
    station = schema["classes"]["BikeHireDockingStation"]
    for name in station["slots"]:
        words = label((station.get("slot_usage") or {}).get(name)) or label(schema["slots"].get(name))
        assert words and words.get("sk") and words.get("en"), name


@pytest.mark.parametrize(("column", "heading"), [
    ("Dĺžka_premostenia__m_", "Dĺžka premostenia (m)"),
    ("Rok_výstavby", "Rok výstavby"),
    ("Cena_celkom", "Cena celkom"),
    ("Č._zasadnutia", "Č. zasadnutia"),
    ("IČO", "IČO"),
    ("KRAJ_ID", None), ("datumzmeny", None), ("TypSaSZKod", None), ("content", None), ("dlzka_VU_1", None),
])
def test_a_source_column_that_is_a_slovak_heading_becomes_the_sk_label_and_a_code_does_not(column, heading):
    assert generate.heading(column) == heading


def test_the_generated_catalogue_models_carry_those_headings():
    schema = yaml.safe_load((SEED / "bbsk/bbsk-otvorene-data.linkml.yaml").read_text())
    assert label(schema["slots"]["dlzkaPremosteniaM"])["sk"] == "Dĺžka premostenia (m)"
    assert label(schema["slots"]["source"])["en"] == "Source"
    labelled = [s for s in schema["slots"].values() if label(s)]
    assert all(not CAMEL.search(label(s)["sk"]) for s in labelled)
    assert len(labelled) > len(schema["slots"]) / 2


def generated(node: dict | None) -> list[str]:
    found = ((node or {}).get("annotations") or {}).get("label") or {}
    said = (found.get("annotations") or {}).get("generated") if isinstance(found, dict) else None
    return str(said.get("value", said) if isinstance(said, dict) else said or "").split()


@pytest.mark.parametrize("model", sorted(p.relative_to(SEED).as_posix() for p in SEED.rglob("*.linkml.yaml")))
def test_every_slot_of_every_seed_model_reads_in_four_languages_and_no_label_is_camel_case(model):
    schema = yaml.safe_load((SEED / model).read_text())
    slots = schema.get("slots") or {}
    for cls, about in (schema.get("classes") or {}).items():
        usage = about.get("slot_usage") or {}
        for name in about.get("slots") or []:
            node = usage.get(name) if label(usage.get(name)) else slots.get(name)
            words = label(node)
            assert words, f"{model}: {cls}.{name} has no label"
            for lang in LANGS:
                text = words.get(lang)
                assert text and text != name and not CAMEL.search(text), (model, cls, name, lang, text)
            assert set(generated(node)) <= set(LANGS), (model, name, generated(node))


def test_a_generated_language_is_marked_and_a_heading_the_source_wrote_is_not():
    schema = yaml.safe_load((SEED / "bbsk/bbsk-otvorene-data.linkml.yaml").read_text())
    assert generated(schema["slots"]["dlzkaPremosteniaM"]) == ["cs", "en", "de"]
    assert generated(schema["slots"]["aUT"]) == list(LANGS)
    assert generated(schema["slots"]["source"]) == []
    registre = yaml.safe_load((SEED / "bbsk/bbsk-registre.linkml.yaml").read_text())
    assert generated(registre["slots"]["name"]) == []
    helsinki = yaml.safe_load((SEED / "helsinki/helsinki.linkml.yaml").read_text())
    assert generated(helsinki["slots"]["streetAddress"]) == list(LANGS)
