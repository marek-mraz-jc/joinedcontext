"""T-4202 (UI-44): dev serves five publishers (Helsinki, Banská Bystrica, BBSK, Žilina, Praha) from
one catalogue and one Portal, so what the installation calls itself names none of them. The
photo audit found Helsinki's name in the footer, the about page and the breadcrumb of the other
cities' pages, and a heading face without š, ž or ř. Each dataset and Endpoint names its own
publisher; the branding block names the demo.
"""
import json

import yaml


def config_map(docs, name):
    (found,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == name]
    return found["data"]


def test_the_catalogue_names_no_city_in_its_title_about_or_fonts(rendered):
    branding = json.loads(config_map(rendered("dev"), "ckan-theme")["branding.json"])
    # The site description and the about page's intro are `organisation`; the title is the
    # instance's name (plugin.py).
    for field in ("instanceName", "shortName", "organisation"):
        assert branding[field] and "Helsinki" not in branding[field], field
    for stack in branding["fonts"].values():
        assert "HelsinkiGrotesk" not in stack


def test_the_catalogue_speaks_slovak_first_and_the_portal_keeps_its_languages(rendered):
    # T-4280: data.dev opens in Slovak; the Portal's English-only choice stays its own.
    docs = rendered("dev")
    catalogue = json.loads(config_map(docs, "ckan-theme")["branding.json"])["languages"]
    assert catalogue == {"default": "sk", "offered": ["sk", "en", "cs", "fi", "hu"]}
    portal = yaml.safe_load(config_map(docs, "portal-branding")["branding.yaml"])["languages"]
    assert portal["default"] == "en"


def test_the_portal_takes_the_same_name_and_fonts(rendered):
    docs = rendered("dev")
    portal = yaml.safe_load(config_map(docs, "portal-branding")["branding.yaml"])
    assert portal["organisation"] == "joinedcontext demo"
    assert all("HelsinkiGrotesk" not in stack for stack in portal["fonts"].values())
