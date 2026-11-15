"""The seeded site of images/decidim (T-6453): enough content, and nobody's personal data."""

import re
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1] / "images/decidim"
CONTENT = yaml.safe_load((ROOT / "site/content.yml").read_text(encoding="utf-8"))
PROCESSES = CONTENT["processes"]
STATES = {"evaluating", "to_vote", "accepted", "rejected"}
# The shapes a contact line takes in the public texts: e-mail, telephone, academic title.
PERSONAL = re.compile(
    r"[\w.+-]+@[\w-]+\.\w+|\+?\d{3,4}[ /]?\d{3}[ /]?\d{3}\b|\b(?:Mgr|Ing|JUDr|PaedDr|PhDr|Bc|RNDr)\."
)


def strings(value):
    if isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, str):
        yield value


def test_the_three_processes_are_open_in_evaluation_and_finished():
    assert [p["slug"] for p in PROCESSES] == [
        "participatorybudget20262027", "participatorybudget2026", "participatorybudget2025",
    ]
    assert CONTENT["home"]["cta_process"] == PROCESSES[0]["slug"]


@pytest.mark.parametrize("process", PROCESSES, ids=lambda p: p["slug"])
def test_a_process_reads_like_a_real_one(process):
    assert sum(1 for step in process["steps"] if step.get("active")) == 1
    assert len(process["steps"]) >= 5
    assert len(process["proposals"]) >= 5
    assert {p["state"] for p in process["proposals"]} <= STATES
    assert len(process["meetings"]) >= 2
    assert all(m["location"] and m["address"] for m in process["meetings"])
    assert process["projects"] and process["results"]
    assert sum(p["amount"] for p in process["projects"] if p.get("selected")) <= process["budget_total"]
    assert all(0 <= r["progress"] <= 100 for r in process["results"])
    assert (ROOT / "site" / process["image"]).is_file()


def test_no_contact_detail_or_titled_person_is_in_the_content():
    found = [text for text in strings(CONTENT) if PERSONAL.search(text)]
    assert not found


@pytest.mark.parametrize("name", ["logo.png", "favicon.png", "hero.jpg"])
def test_the_assets_are_in_the_image(name):
    assert (ROOT / "site" / name).stat().st_size > 0
    assert "COPY site db/joinedcontext_site" in (ROOT / "Dockerfile").read_text(encoding="utf-8")


def test_every_site_file_reaches_its_place_in_the_image():
    dockerfile = (ROOT / "Dockerfile").read_text(encoding="utf-8")
    for line in (
        "COPY site.rb db/joinedcontext_site.rb",
        "COPY locale.yml config/locales/joinedcontext.yml",
        "COPY theme.css public/joinedcontext-theme.css",
        "COPY head-extra.html.erb app/views/layouts/decidim/_head_extra.html.erb",
        "COPY footer-legal.html.erb app/views/layouts/decidim/footer/_main_legal.html.erb",
    ):
        assert line in dockerfile
    assert "/joinedcontext-theme.css" in (ROOT / "head-extra.html.erb").read_text(encoding="utf-8")


def test_each_phase_title_has_its_english_title():
    titles = {step["title"] for process in PROCESSES for step in process["steps"]}
    assert titles == set(CONTENT["step_titles_en"])


def test_a_rebuild_keeps_every_migration_number():
    """T-6449: a build-clock number made db:migrate rerun every migration on the next image."""
    assert "printf '%014d' $((20000101000000 + i))" in (ROOT / "Dockerfile").read_text(encoding="utf-8")
