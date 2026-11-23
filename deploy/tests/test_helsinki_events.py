"""Every upcoming Linked Events event becomes an SDM Event, paged and checked (T-5257, PL-03).

Bento runs the committed mapping over a page recorded from the register on 2026-10-22 (five
events with their places and keywords: `fixtures/helsinki/linked-events/upcoming-page.json`)
and five rows the test made from them: a point in Tampere, a swapped longitude and latitude, a
start year typed as 0026, a place with no position and an event listed twice. The calls to the
register are the cut: the page the DataSource read stands in for the first, and the paging test
answers each further page itself. Without a container runtime the Bento cases skip.
"""

import json
import os
import re
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-open-data")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
MAPPING = HELSINKI / "helsinki-pipeline-events-bento.yaml"
FIXTURE = Path(__file__).resolve().parent / "fixtures/helsinki/linked-events/upcoming-page.json"
URN = "urn:ngsi-ld:{}:hel.fi:helsinki:{}"


#: The ask of the space answered as if it held every target a page links.
HOLDS_ALL = ('meta held = this.map_each(e -> [e.superEvent.object.or(null), e.refPointOfInterest.object.or(null)])'
             '.flatten().filter(t -> t != null).join(" ")')


#: The register's answer for the one publisher of the recorded page, read on 2026-11-23.
ORGANIZATION = Path(__file__).resolve().parent / "fixtures/helsinki/linked-events/organization-ahjo-u4804001010.json"
#: The organisation list answered as the register does: the recorded organisation, else a 404.
ORGANIZATIONS = ('root = if content().string() == "ahjo:u4804001010" { %s } else { throw("404 Not Found") }'
                 % json.dumps(json.loads(ORGANIZATION.read_text())))


def first_try(step: dict) -> dict:
    return step.get("branch", {}).get("processors", [{}])[0].get("try", [{}])[0]


def processors(first: str, page: str, held: list[str] | None = None, organizations: str = ORGANIZATIONS) -> list[dict]:
    """The mapping with its mode fixed by `first` (the step that reads the clock), every request
    to the register answered by the mapping `page`, the ask of the space which link targets
    it holds answered by `held` (every target when None; an empty list is also a failed ask), and
    each organisation the runner's cache misses answered by `organizations`, logged as `asked`."""
    steps = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    assert "now()" in steps[1]["mapping"], "the second step chooses the sweep by the clock"
    steps[1] = {"mapping": first}
    asks = [i for i, step in enumerate(steps) if "http" in first_try(step)]
    assert len(asks) == 1, "one ask of the space for the link targets"
    steps[asks[0]] = {"mapping": HOLDS_ALL if held is None else f"meta held = {json.dumps(' '.join(held))}"}
    lookups = [step for step in steps if "cached" in first_try(step)]
    assert len(lookups) == 1, "one lookup of the publisher's name"
    cached = first_try(lookups[0])["cached"]
    http = cached["processors"][0]["http"]
    assert http["url"] == "https://api.hel.fi/linkedevents/v1/organization/${! content() }/", http["url"]
    assert cached["cache"] == "linked_events_orgs" and cached["ttl"] == "24h"
    cached["processors"][0:1] = [{"log": {"level": "INFO", "message": "asked ${! content() }"}}, {"mapping": organizations}]
    for step in steps:
        if "branch" in step and step not in lookups:
            assert step["branch"]["processors"][-1]["http"]["url"].startswith("https://api.hel.fi/linkedevents/v1/event/?")
            step["branch"]["processors"] = [{"mapping": page}]
    return steps


def run(document: bytes, first: str, page: str = "root = {}", held: list[str] | None = None,
        organizations: str = ORGANIZATIONS) -> tuple[list[dict], str]:
    # The runner's cache under the name the mapping writes (the Portal prefixes it with the project).
    caches = [{**cache, "label": cache["label"].split("__", 1)[1]}
              for cache in yaml.safe_load(open_data.RESOURCES.read_text())["cache_resources"]]
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": {"processors": processors(first, page, held, organizations)},
              "cache_resources": caches,
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "info", "format": "logfmt"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=120)
    assert result.returncode == 0, result.stderr.decode()
    pages = [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]
    return [e for page in pages for e in page], result.stderr.decode()


FULL = 'root = { "query": "q", "first": this }'


@requires_docker
def test_the_recorded_page_maps_to_valid_events_and_drops_each_bad_row_by_reason():
    events, log = run(FIXTURE.read_bytes(), FULL)
    by_id = {e["id"]: e for e in events}
    # The register's own defect: its first upcoming event starts in "0026-09-23", so it is dropped
    # as a start year out of range, beside the test's own bad rows.
    assert sorted(by_id) == sorted(URN.format("Event", i) for i in (
        "hkm-3466", "helsinki-agp5dhl25y", "helsinki-afzapzc5sq", "helsinki-agpg4hzhzq"))
    assert 'dropped by reason {\\"duplicate\\":1,\\"no position\\":1,\\"outside Helsinki\\":2,\\"start year out of range\\":2}' in log, log
    for entity in events:
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
        errors = [e for e in open_data.sdm_errors("dataModel.TourismDestinations/Event", entity)
                  if not e.startswith(("eventStatus", "refPointOfInterest"))]
        assert not errors, (entity["id"], errors)
    dance = by_id[URN.format("Event", "helsinki-agpg4hzhzq")]
    assert dance["refPointOfInterest"] == {"type": "Relationship", "object": URN.format("PointOfInterest", "servicemap-48547")}
    assert dance["thematic"]["value"][0] == "dance (performing arts)"
    assert dance["isAccessibleForFree"]["value"] is True
    assert "webSite" not in dance, "an empty info_url is no website"
    assert "dataProvider" not in dance, "a publisher that is no organization id is dropped"
    # A sub-event names its recurring parent by the parent's own URN.
    assert dance["superEvent"]["object"] == URN.format("Event", "helsinki-agpg4hzprq")
    museum = by_id[URN.format("Event", "hkm-3466")]
    assert museum["webSite"]["value"] == "https://www.helsinginkaupunginmuseo.fi/nayttelyt/lasten-kaupunki/"
    assert museum["thematic"]["value"][0] == "everyday"
    # The publisher is the source a per-source count reads back (T-5666).
    assert museum["dataProvider"] == {"type": "Property", "value": "ahjo:u4804001010"}
    # The record names no provider, so its publisher runs it: the id resolved to its name (T-7567).
    assert museum["organizer"] == {"type": "Property", "value": {"name": "Kaupunginkirjasto"}}
    assert "organizer" not in dance, "a publisher that is no organization id names no organizer"


def with_rows(*rows: dict) -> bytes:
    """The recorded page with its rows replaced by the recorded museum event changed by `rows`."""
    page = json.loads(FIXTURE.read_text())
    museum = page["data"][1]
    page["data"] = [{**museum, **row} for row in rows]
    return json.dumps(page).encode()


@requires_docker
def test_the_organizer_is_the_events_provider_else_its_publishers_name_and_never_a_contact():
    """T-7567: Linked Events names a `provider` only when it is not the publisher, so the provider
    wins, in Finnish, else English or Swedish; an e-mail or a phone typed into it is cut; an event
    without one is its publisher's, by the name the register gives that id; an id the register
    does not answer leaves the event without an organizer, and the event is written all the same."""
    document = with_rows(
        {"id": "hkm:p1", "provider": {"fi": "  Tekniikan   museo ", "en": "Museum of Technology"}},
        {"id": "hkm:p2", "provider": {"fi": "", "sv": "Amos Rex"}},
        {"id": "hkm:p3", "provider": {"fi": "Kerho Ry, info@kerho.fi, 040 123 4567"}},
        {"id": "hkm:p4", "provider": None},
        {"id": "hkm:p5", "provider": {"fi": None}, "publisher": "ytj:0586977-6"},
    )
    events, log = run(document, FULL)
    names = {e["id"].rsplit(":", 1)[1]: e.get("organizer", {}).get("value", {}).get("name") for e in events}
    assert names == {"hkm-p1": "Tekniikan museo", "hkm-p2": "Amos Rex", "hkm-p3": "Kerho Ry",
                     "hkm-p4": "Kaupunginkirjasto", "hkm-p5": None}, names
    for entity in events:
        assert not open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity), entity
    # Only the events without a provider ask for their publisher.
    assert sorted(re.findall(r"asked ([^ \"]+)", log)) == ["ahjo:u4804001010", "ytj:0586977-6"], log


@requires_docker
def test_the_runner_asks_the_register_once_a_day_per_publisher_and_again_after_a_failure():
    """T-7567: the name of a publisher id comes from the runner's cache after the first ask, so a
    sweep of thousands of events asks once per publisher; a failed ask is not kept, so the next
    event (and the next run) asks again instead of losing the name for a day."""
    rows = [{"id": f"hkm:c{i}", "provider": None} for i in range(4)]
    events, log = run(with_rows(*rows), FULL)
    assert {e["organizer"]["value"]["name"] for e in events} == {"Kaupunginkirjasto"}
    assert re.findall(r"asked ([^ \"]+)", log) == ["ahjo:u4804001010"], log
    events, log = run(with_rows(*rows), FULL, organizations='root = throw("503 Service Unavailable")')
    assert len(events) == 4 and not any("organizer" in e for e in events)
    assert re.findall(r"asked ([^ \"]+)", log) == ["ahjo:u4804001010"] * 4, log


#: What the mapping writes for the recorded page, organizer included (T-7567). Regenerate after a
#: deliberate mapping change with `UPDATE_GOLDEN=1 pytest tests/test_helsinki_events.py` and read
#: the diff.
GOLDEN = Path(__file__).resolve().parent / "fixtures/helsinki/linked-events/upcoming-page.events.json"


@requires_docker
def test_the_recorded_page_writes_the_golden_events():
    events, _ = run(FIXTURE.read_bytes(), FULL)
    written = json.dumps(sorted(events, key=lambda e: e["id"]), indent=1, ensure_ascii=False, sort_keys=True) + "\n"
    if os.environ.get("UPDATE_GOLDEN") == "1":
        GOLDEN.write_text(written)
    assert written == GOLDEN.read_text(), "the mapping's output changed: rerun with UPDATE_GOLDEN=1 and read the diff"


@requires_docker
def test_an_event_links_only_targets_the_space_holds_and_is_written_either_way():
    """F-R2BNT3-004: the gateway refuses a whole Event whose link names an entity the space does
    not hold (DM-70). A link to an unheld parent or venue is left out and the event still lands."""
    dance_id = URN.format("Event", "helsinki-agpg4hzhzq")
    venue = URN.format("PointOfInterest", "servicemap-48547")
    events, _ = run(FIXTURE.read_bytes(), FULL, held=[venue])
    dance = {e["id"]: e for e in events}[dance_id]
    assert dance["refPointOfInterest"]["object"] == venue and "superEvent" not in dance
    failed, _ = run(FIXTURE.read_bytes(), FULL, held=[])
    assert {e["id"] for e in failed} == {e["id"] for e in events}
    assert not any("superEvent" in e or "refPointOfInterest" in e for e in failed)


@requires_docker
def test_an_empty_register_fails_the_run_instead_of_writing_nothing():
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}},
              "pipeline": {"processors": processors(FULL, "root = {}")[:1]},
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "error"}}
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=b'{"meta": {"count": 0}, "data": []}', capture_output=True, timeout=120)
    # A thrown mapping error leaves the message flagged; the runner's output refuses a flagged
    # message, so the endpoint never sees an empty list.
    assert b"Linked Events lists no upcoming event" in result.stderr + result.stdout


@requires_docker
def test_every_page_of_the_sweep_is_read_once_and_written_on_its_own():
    page = json.loads(FIXTURE.read_text())
    one = page["data"][1]
    first = dict(page, meta={"count": 250}, data=[one])
    # Each further page answers with the event renamed after the query that asked for it.
    answer = ('root.data = [%s.assign({"id": "helsinki:p" + @query.re_find_all_submatch("page=([0-9]+)").index(0).index(1)})]'
              % json.dumps(one))
    events, _ = run(json.dumps(first).encode(), FULL, answer)
    assert [e["id"].rsplit(":", 1)[1] for e in events] == ["hkm-3466", "helsinki-p2", "helsinki-p3"]


@requires_docker
def test_the_incremental_run_reads_its_own_first_page():
    page = json.loads(FIXTURE.read_text())
    one = page["data"][1]
    incremental = 'root = { "query": "q&last_modified_since=2026-10-22T17:00:00Z", "first": null }'
    answer = ('root = if @query.contains("last_modified_since") { {"meta": {"count": 1}, "data": [%s]} } else { throw("unasked") }'
              % json.dumps(one))
    events, _ = run(FIXTURE.read_bytes(), incremental, answer)
    assert [e["id"] for e in events] == [URN.format("Event", "hkm-3466")]


def test_the_datasource_reads_the_upcoming_list_with_places_and_keywords():
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-events.yaml").read_text())
    url = source["spec"]["http"]["url"]
    assert url.startswith("https://api.hel.fi/linkedevents/v1/event/?")
    assert "include=location,keywords" in url and "start=today" in url and "end=" not in url
