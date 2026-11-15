"""The events pipelines, one per kept source and store: the Banská Bystrica Region's (T-4896, PL-43) and the
national store slovensko-podujatia (T-5047).

Each template runs in Bento over a feed recorded from its publisher on 2026-10-20
(fixtures/bbsk-events/), with the detail pages a template reads served from the recording and the
clock pinned to that day, so the cases do not age. What reaches the Endpoint is checked against
the Event model: an event without a name, a start or a link is refused there and lands in the run's
rejected list (PL-62), never in the space; one without a GPS point or an organizer is kept (owner 2026-10-22).

Without a container runtime the Bento cases skip; the seed and inventory cases do not.
"""

import copy
import csv
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
from datetime import UTC, datetime, timedelta
from pathlib import Path
from urllib.parse import urlsplit

import jsonschema
import pytest
import yaml

pytestmark = pytest.mark.xdist_group("docker-bbsk-events-pipelines")

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/bbsk"
NATIONAL = ROOT / "components/context-gateway/seed/slovensko"
EVENTS = ROOT / "seeds/bbsk-events"
FIXTURES = Path(__file__).resolve().parent / "fixtures/bbsk-events"
#: The runner's shared resources, events_tail among them (T-5555), loaded with -r as the runner does.
RESOURCES = ROOT / "components/pipeline-runner/streams/resources.yaml"
BENTO = "ghcr.io/warpstreamlabs/bento:1.22.0@sha256:ef674bebae45793c982c350d8f37c2c7661133c1dc65a5517fb977b36dc09841"
#: 2026-10-20T00:00:00Z, the day the fixtures were recorded.
NOW = 1792454400
SCHEMA = json.loads((SEED / "bbsk-podujatia.v1.schema.json").read_text())
VALID = jsonschema.Draft7Validator(
    {**copy.deepcopy(SCHEMA), "$ref": "#/definitions/Event"}
)

requires_docker = pytest.mark.skipif(
    shutil.which("docker") is None, reason="no container runtime for Bento"
)

sys.path.insert(0, str(EVENTS))
import generate

#: CIM 009 4.18: [/] ScopeLevel *(/ScopeLevel), ScopeLevel = unicodeLetter *(letter/digit/_). The broker
#: refuses any other scope, and a refused upsert member answers 403 AccessDenied (T-5051).
SCOPE = re.compile(r"(/[^\W\d_]\w*)+")


def processors(name: str, ns: str = "bbsk") -> list:
    return yaml.safe_load(
        (ROOT / "components/context-gateway/seed" / ns / f"{ns}-pipeline-podujatia-{name}-bento.yaml").read_text()
    )["pipeline"]["processors"]


def template(kind: str, host: str, store: str = "slovensko", region: str = "BA", town: str = "Častá", path: str = "/",
             org_type: str = "municipality") -> list:
    """A kind's generated steps for a made-up row, so a template is tested apart from the inventory."""
    row = {"feed_kind": kind, "host": f"https://{host}", "town": town, "town_slug": generate.slug(town),
           "org_type": org_type, "site": f"https://{host}",
           "region": region, "events_url": f"https://{host}/", "check_url": f"https://{host}{path}", "licence_terms": "no licence stated",
           "scope": generate.REGIONS[region][0], "region_name": generate.REGIONS[region][1],
           "country": generate.COUNTRIES.get(region, "SK")}
    return generate.processors(row, next(s for s in generate.STORES if s["ns"] == store))


def offline(steps: list, pages: dict) -> list:
    """The steps with the clock pinned and each detail request answered from the recording."""
    text = json.dumps(steps).replace("let now = timestamp_unix()", f"let now = {NOW}")
    steps = json.loads(text)
    table = json.dumps(
        [{"path": k, "page": v} for k, v in pages.items()], ensure_ascii=False
    )
    for step in steps:
        if "branch" in step:
            assert [next(iter(p)) for p in step["branch"]["processors"]] == [
                "sleep",
                "http",
            ]
            assert step["branch"]["processors"][1]["http"]["url"].startswith("https://")
            step["branch"]["processors"] = [
                {
                    "mapping": (
                        f'let p = {table}.filter(x -> x.path == "/akcia/" + @event_path || x.path == "/events/" + @event_path || x.path == "/" + @event_path || x.path == @event_path).index(0).catch(null)\n'
                        'root = if $p == null { throw("no recorded page for %v".format(@event_path)) } else { $p.page }'
                    )
                }
            ]
    return steps


def run(steps: list, payload: str, space: str = "bbsk-podujatia", refused: bool = False, now: int = NOW) -> list:
    """The records the steps write with the runner's resources; `refused` expects the run to fail with an error and
    write nothing."""
    with tempfile.TemporaryDirectory(dir="/tmp") as directory:
        work = Path(directory)
        work.chmod(0o777)
        (work / "in.txt").write_text(payload)
        (work / "resources.yaml").write_text(
            RESOURCES.read_text(encoding="utf-8").replace("let now = timestamp_unix()", f"let now = {now}"),
            encoding="utf-8",
        )
        (work / "cfg.yaml").write_text(
            yaml.safe_dump(
                {
                    "input": {
                        "file": {"paths": ["/w/in.txt"], "scanner": {"to_the_end": {}}}
                    },
                    "pipeline": {"processors": steps},
                    "output": {"file": {"path": "/w/out.json", "codec": "lines"}},
                },
                allow_unicode=True,
            )
        )
        for item in work.iterdir():
            item.chmod(0o666)
        result = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--user",
                f"{os.getuid()}:{os.getgid()}",
                "-v",
                f"{work}:/w",
                "-w",
                "/w",
                "-e",
                "JC_ORG_DOMAIN=bbsk.sk",
                "-e",
                f"JC_SPACE={space}",
                BENTO,
                "-r",
                "/w/resources.yaml",
                "-c",
                "/w/cfg.yaml",
            ],
            capture_output=True,
            text=True,
            timeout=240,
            check=False,
        )
        # A fixture records some of a feed's pages; the stub's refusal for the others is no error of the template.
        log = "\n".join(line for line in (result.stdout + result.stderr).splitlines() if "no recorded page" not in line)
        assert ("level=error" in log) == refused, log[-2000:]
        if refused:
            return []
        produced = work / "out.json"
        lines = produced.read_text().splitlines() if produced.is_file() else []
        entities = [e for line in lines if line.strip() for e in json.loads(line)]
        for e in entities:
            assert SCOPE.fullmatch(e["scope"]), e["scope"]
        return entities


def pipeline(name: str, fixture: str, pages: str | None = None) -> list:
    recorded = json.loads((FIXTURES / "pages" / pages).read_text()) if pages else {}
    return run(offline(processors(name), recorded), (FIXTURES / fixture).read_text())


def pipeline_ns(name: str, fixture: str) -> list:
    """A national-store source's generated steps over its recorded feed."""
    return run(offline(processors(name, "slovensko"), {}), (FIXTURES / fixture).read_text(), "slovensko-podujatia")


def home_point(events_url: str) -> list[float]:
    """[lon, lat] of the venues.csv row homes.csv names for a source."""
    events = ROOT / "seeds/bbsk-events"
    (home,) = [h for h in csv.DictReader((events / "homes.csv").open(encoding="utf-8")) if h["events_url"] == events_url]
    (row,) = [v for v in csv.DictReader((events / "venues.csv").open(encoding="utf-8"))
              if (v["town"], v["match"]) == (home["town"], home["match"])]
    return [float(row["lon"]), float(row["lat"])]


def key_values(entity: dict) -> dict:
    """The entity as the model's key-values shape, without the NGSI-LD system members."""
    out = {}
    for k, v in entity.items():
        if k in ("scope", "expiresAt"):
            continue
        out[k] = (
            v.get("value", v.get("object"))
            if isinstance(v, dict) and "type" in v and k not in ("id", "type")
            else v
        )
    return out


def wp_event(i: int, day: datetime, title: str | None = None) -> dict:
    return {
        "id": i,
        "link": f"https://www.banskabystrica.sk/podujatia/e-{i}/",
        "title": {"rendered": title or f"Podujatie {i}"},
        "content": {"rendered": "<p>Text</p>"},
        "modified_gmt": "2026-10-01T10:00:00",
        "meta_box": {
            "date_start": day.strftime("%Y-%m-%d"),
            "time_start": "18:00",
            "date_end": None,
            "time_end": None,
            "address_street": "Námestie SNP",
            "address_number": "1",
            "address_locality": "Banská Bystrica",
            "coordinates": {"latitude": "48.7363", "longitude": "19.1462"},
        },
    }


# --- seed and inventory -----------------------------------------------------------------------------------------


def test_the_seed_is_what_the_templates_and_the_inventory_generate():
    assert generate.main(check=True) == 0, "run python3 seeds/bbsk-events/generate.py"


def test_every_kept_source_with_a_template_has_its_pipeline_and_the_rest_are_named():
    rows = [
        r
        for f in generate.INVENTORIES
        for r in csv.DictReader((EVENTS / f).open(encoding="utf-8"))
        if r["decision"] == "pipeline"
    ]
    built = {r["check_url"] for r in generate.kept_sources()}
    without = sorted({r["feed_kind"] for r in rows if r["check_url"] not in built})
    # Free-text dates (wp-rss, joomla-rss, most wp-rest event pages) need one parser per site, and the runner
    # reads https only (Betliar's iCal is http); T-5047 says which rows wait and why.
    assert without == ["ical", "joomla-rss", "wp-rest", "wp-rss"]
    towns = {r["town"] for r in generate.kept_sources()} - {generate.REGION}
    assert len(towns) >= 60


def test_the_national_store_reads_at_least_100_sources_from_all_eight_regions():
    rows = generate.store_rows(generate.STORES[1])
    assert len(rows) >= 100
    by_region = {code: sum(r["region"] == code for r in rows) for code in generate.SLOVAK}
    assert not any(r["region"] in generate.COUNTRIES for r in rows)
    assert all(n >= 6 for n in by_region.values()), by_region
    assert len(list(p for p in NATIONAL.glob("slovensko-pipeline-podujatia-*.yaml") if not p.name.endswith("-bento.yaml"))) == len(rows)


def test_every_kept_source_names_its_licence_and_robots_reading():
    for row in generate.kept_sources():
        assert row["licence_terms"].strip(), row["name"]
        assert row["robots_allows"].startswith("y"), row["name"]
        assert row["read_on"] >= "2026-10-20", row["name"]


def test_every_events_pipeline_ends_with_its_settings_and_the_one_shared_tail():
    """T-5555: the tail lives once in the runner's resources; a pipeline carries only its own settings."""
    tails = [r for r in yaml.safe_load(RESOURCES.read_text())["processor_resources"] if r["label"] == generate.TAIL]
    assert len(tails) == 1
    tail = tails[0]["processors"]
    assert tail[-1] == {"mutation": "meta events = deleted()\n"}
    paths = sorted([*SEED.glob("bbsk-pipeline-podujatia-*-bento.yaml"), *NATIONAL.glob("slovensko-pipeline-podujatia-*-bento.yaml")])
    assert len(paths) > 300
    read = set(re.findall(r"@events\.([a-z_]+)\b(?!\()", json.dumps(tail)))
    for path in paths:
        steps = yaml.safe_load(path.read_text())["pipeline"]["processors"]
        assert steps[-1] == {"resource": generate.TAIL}, path.name
        settings = steps[-2]["mutation"]
        assert settings.startswith("meta events = {") and 'env("JC_SPACE")' in settings, path.name
        # Every setting the tail reads is set, and the tail is nowhere else in the file.
        assert {k for k in read if f'"{k}": ' not in settings} == set(), path.name
        assert "generate.py ABBREVIATIONS" not in path.read_text(), path.name


@requires_docker
def test_a_pipeline_writes_through_the_shared_tail_what_it_wrote_with_the_tail_inlined():
    """T-5555: `- resource: events_tail` gives the records the copied tail gave, settings and space included."""
    steps = offline(processors("banskabystrica"), {})
    (tail,) = [r["processors"] for r in yaml.safe_load(RESOURCES.read_text())["processor_resources"] if r["label"] == generate.TAIL]
    inlined = json.loads(json.dumps([*steps[:-1], *tail]).replace("let now = timestamp_unix()", f"let now = {NOW}"))
    day = datetime.fromtimestamp(NOW, UTC) + timedelta(days=3)
    feed = json.dumps([wp_event(i, day + timedelta(hours=i)) for i in range(5)])
    shared = run(steps, feed)
    assert len(shared) == 5 and shared == run(inlined, feed)
    assert all(e["id"].startswith("urn:ngsi-ld:Event:bbsk-podujatia:") for e in shared)


@requires_docker
def test_the_shared_tail_without_its_settings_fails_the_run_and_writes_nothing():
    assert run([{"resource": generate.TAIL}], json.dumps([{"name": "Koncert", "start": "2026-10-23"}]), refused=True) == []


def test_every_http_input_is_https_to_its_inventory_host_with_a_timeout():
    hosts = {
        generate.urlsplit(r["check_url"]).hostname for r in generate.kept_sources()
    }
    for path in [*SEED.glob("bbsk-datasource-podujatia-*.yaml"), *NATIONAL.glob("slovensko-datasource-podujatia-*.yaml")]:
        http = yaml.safe_load(path.read_text())["spec"]["http"]
        assert (
            http["url"].startswith("https://")
            and generate.urlsplit(http["url"]).hostname in hosts
        ), path.name
        assert http["timeout"] == "30s" and "authorization" not in http, path.name
    for path in [*SEED.glob("bbsk-pipeline-podujatia-*-bento.yaml"), *NATIONAL.glob("slovensko-pipeline-podujatia-*-bento.yaml")]:
        for step in yaml.safe_load(path.read_text())["pipeline"]["processors"]:
            if "branch" in step:
                http = step["branch"]["processors"][1]["http"]
                # The source's own host, fixed: the item's path is checked by the request_map before the request.
                fixed = http["url"].split("${!")[0]
                assert generate.urlsplit(fixed).hostname in hosts and fixed.count("/") >= 3, path.name
                # One request a second, three or ten where the CMS's robots.txt asks Crawl-Delay 3 (Lego) or 10 (TUZVO).
                assert http["timeout"] == "20s" and step["branch"]["processors"][0] in (
                    {"sleep": {"duration": "1s"}}, {"sleep": {"duration": "3s"}}, {"sleep": {"duration": "10s"}}
                )


def test_each_source_runs_once_a_day_in_its_own_night_minute_into_the_events_space():
    minutes = []
    for path in (p for p in SEED.glob("bbsk-pipeline-podujatia-*.yaml") if not p.name.endswith("-bento.yaml")):
        spec = yaml.safe_load(path.read_text())["spec"]
        assert spec["targetEndpoint"] == generate.TARGET and spec["output"] == {
            "type": "Event",
            "mode": "upsert",
        }
        minute, hour, *rest = spec["schedule"].split()
        assert rest == ["*", "*", "*"] and spec["period"] == "24h"
        minutes.append(int(hour) * 60 + int(minute))
    # 00:00-02:59 UTC is 01:00-05:00 in Bratislava in winter and summer alike.
    assert len(set(minutes)) == len(minutes) and max(minutes) < 180


def test_each_national_feed_runs_every_3_hours_and_each_village_calendar_daily_in_its_own_minute():
    target = generate.STORES[1]["target"]
    assert target == "urn:ngsi-ld:Endpoint:joinedcontext.com:slovensko-podujatia:slovensko-podujatia-zapis"
    starts = {"3h": [], "24h": []}
    for path in (p for p in NATIONAL.glob("slovensko-pipeline-podujatia-*.yaml") if not p.name.endswith("-bento.yaml")):
        spec = yaml.safe_load(path.read_text())["spec"]
        kind = yaml.safe_load(NATIONAL.joinpath(path.name.replace("-pipeline-", "-datasource-")).read_text())["metadata"]["title"]["en"]
        daily = kind.endswith(tuple(f"({k})" for k in generate.STORES[1]["cadences"][0]["kinds"]))
        assert spec["targetEndpoint"] == target and spec["period"] == ("24h" if daily else "3h"), path.name
        minute, hour, *rest = spec["schedule"].split()
        assert rest == ["*", "*", "*"], path.name
        if daily:
            assert hour.isdigit(), path.name
        else:
            assert hour.endswith("-23/3"), path.name
        starts[spec["period"]].append(int(hour.split("-")[0]) * 60 + int(minute))
    assert len(set(starts["3h"])) == len(starts["3h"]) and max(starts["3h"]) < 180
    assert len(set(starts["24h"])) == len(starts["24h"]) and max(starts["24h"]) < 1440


# --- the templates in Bento ---------------------------------------------------------------------------------


@requires_docker
def test_bento_lints_every_generated_stream():
    """One Bento lint over every stream: a container per stream took minutes for the ~290 generated pipelines."""
    paths = sorted([*SEED.glob("bbsk-pipeline-podujatia-*-bento.yaml"), *NATIONAL.glob("slovensko-pipeline-podujatia-*-bento.yaml")])
    assert paths
    with tempfile.TemporaryDirectory(dir="/tmp") as directory:
        work = Path(directory)
        work.chmod(0o777)
        for path in paths:
            cfg = {
                "input": {"stdin": {}},
                "pipeline": yaml.safe_load(path.read_text())["pipeline"],
                "output": {"stdout": {}},
            }
            (work / path.name).write_text(yaml.safe_dump(cfg, allow_unicode=True))
            (work / path.name).chmod(0o666)
        shutil.copy(RESOURCES, work / "resources.yaml")
        (work / "resources.yaml").chmod(0o666)
        result = subprocess.run(
            ["docker", "run", "--rm", "-v", f"{work}:/w", BENTO, "lint", "/w/resources.yaml", *(f"/w/{p.name}" for p in paths)],
            capture_output=True,
            text=True,
            timeout=240,
            check=False,
        )
    assert result.returncode == 0, f"{result.stdout}{result.stderr}"


@requires_docker
def test_banska_bystrica_wordpress_events_carry_their_own_gps():
    entities = pipeline("banskabystrica", "banskabystrica-wp-rest.json")
    assert entities and all(
        e["type"] == "Event" and e["scope"] == "/SK/BB/banska_bystrica"
        for e in entities
    )
    for e in entities:
        assert VALID.is_valid(key_values(e)), list(VALID.iter_errors(key_values(e)))
        assert e["expiresAt"] >= "2026-10-20T00:00:00Z"
    assert all("description" in e for e in entities), (
        "CC BY 4.0: the text may be republished"
    )
    text = " ".join(e["description"]["value"] for e in entities)
    assert "vkmk.sk" in (FIXTURES / "banskabystrica-wp-rest.json").read_text()
    assert "@" not in text and "417 37 24" not in text and "413 48 68" not in text


@requires_docker
def test_the_events_calendar_and_ical_sources_give_facts_only():
    for name, fixture in (
        ("visitbanskabystrica", "visitbanskabystrica-tribe.json"),
        ("centralslovakia", "centralslovakia.ics"),
    ):
        entities = pipeline(name, fixture)
        assert entities, name
        for e in entities:
            assert e["startDate"]["value"] and "description" not in e, (name, e)
    ical = {
        e["name"]["value"]: e
        for e in pipeline("centralslovakia", "centralslovakia.ics")
    }
    # "SNG Zvolen" is a venue of the table: the castle's point, scoped to Zvolen.
    assert ical["Moháč 500SK"]["location"]["value"]["coordinates"] == [
        19.1276837,
        48.5729001,
    ]
    assert ical["Moháč 500SK"]["scope"] == "/SK/BB/zvolen"
    # A whole-day DTEND is exclusive: 20261026 is the 25th, the last day.
    assert ical["Moháč 500SK"]["endDate"]["value"] == "2026-10-25"
    assert (
        ical["Coffee Tour Banská Bystrica"]["startDate"]["value"]
        == "2026-11-14T08:30:00+01:00"
    )


@requires_docker
def test_weby_feeds_read_each_event_page_and_place_it_by_the_venue_table():
    lucenec = {
        e["name"]["value"]: e
        for e in pipeline("lucenec", "lucenec-atom.xml", "weby.json")
    }
    apollo = lucenec["SILNÉ REČI"]
    assert apollo["startDate"]["value"] == "2026-11-12T19:00:00+01:00"
    assert apollo["location"]["value"]["coordinates"] == [19.6642043, 48.3319139]
    assert (
        apollo["venue"]["value"] == "Kino Apollo"
        and apollo["scope"] == "/SK/BB/lucenec"
    )
    # A venue nobody placed takes the town's office, labelled approximate (owner 2026-10-21, T-5240).
    arena = lucenec["Futsalová Liga majstrov"]
    assert arena["location"]["value"]["coordinates"] == town_point("lucenec") and VALID.is_valid(key_values(arena))
    assert arena["locationPrecision"]["value"] == "office" and apollo["locationPrecision"]["value"] == "building"
    zvolen = {
        e["name"]["value"]: e
        for e in pipeline("zvolen", "zvolen-rss1.xml", "weby.json")
    }
    assert zvolen["Pravda v háji"]["startDate"]["value"] == "2026-12-19"


@requires_docker
def test_a_weby_feed_that_links_its_pages_over_http_still_reads_each_page():
    # Kysucké Nové Mesto's Atom feed links every event as http://: the run read no page, so no event had a date and
    # every run sent 0 (T-5721). The page itself is still fetched from the fixed https host.
    recorded = json.loads((FIXTURES / "pages" / "weby.json").read_text())
    events = run(
        offline(processors("kysuckenovemesto", "slovensko"), recorded),
        (FIXTURES / "kysuckenovemesto-atom.xml").read_text(),
        "slovensko-podujatia",
    )
    assert [(e["name"]["value"], e["startDate"]["value"]) for e in events] == [("Patranie po tekvičkách", "2026-10-30")]
    assert events[0]["venue"]["value"] == "Mestská knižnica Kysucké Nové Mesto"


@requires_docker
def test_mec_events_take_their_dates_from_the_pages_json_ld():
    by_name = {
        e["name"]["value"]: e
        for e in pipeline("banskastiavnica", "banskastiavnica-mec.json", "mec.json")
    }
    vedatour = by_name["Vedatour / Vedátor v Art Cafe"]
    assert (
        vedatour["startDate"]["value"] == "2026-10-24T20:30:00+02:00"
        and "endDate" not in vedatour
    )
    assert vedatour["location"]["value"]["coordinates"] == [18.8960644, 48.4585939]
    assert VALID.is_valid(key_values(vedatour))


@requires_docker
def test_an_item_off_the_sources_host_is_never_fetched():
    feed = (
        (FIXTURES / "lucenec-atom.xml")
        .read_text()
        .replace(
            "https://www.lucenec.sk/akcia/silne-reci-1.html",
            "https://www.lucenec.sk@attacker.example/akcia/silne-reci-1.html",
        )
    )
    steps = offline(
        processors("lucenec"), json.loads((FIXTURES / "pages/weby.json").read_text())
    )
    # The forged item's page is never read, so it has no date and is refused (T-5215); the others are written.
    out = run(steps, feed)
    assert out and all(e["name"]["value"] != "SILNÉ REČI" for e in out)


@requires_docker
def test_of_150_events_the_100_soonest_upcoming_are_kept():
    day = datetime.fromtimestamp(NOW, UTC)
    past = [wp_event(i, day - timedelta(days=i + 2)) for i in range(30)]
    future = [wp_event(100 + i, day + timedelta(days=i + 1)) for i in range(120)]
    steps = offline(processors("banskabystrica"), {})
    entities = run(steps, json.dumps(list(reversed(past + future))))
    starts = sorted(e["startDate"]["value"] for e in entities)
    assert len(entities) == 100
    assert starts[0].startswith((day + timedelta(days=1)).strftime("%Y-%m-%d"))
    assert starts[-1].startswith((day + timedelta(days=100)).strftime("%Y-%m-%d"))
    assert run(steps, json.dumps(past)) == []


@requires_docker
def test_one_event_announced_by_two_sources_is_one_entity():
    wp = run(
        offline(processors("banskabystrica"), {}),
        json.dumps(
            [
                {
                    **wp_event(
                        1,
                        datetime(2026, 11, 14, tzinfo=UTC),
                        "Richard Autner &#038; band",
                    ),
                    "meta_box": {
                        **wp_event(1, datetime(2026, 11, 14))["meta_box"],
                        "time_start": "19:00",
                    },
                }
            ]
        ),
    )
    tribe = [
        e
        for e in pipeline("visitbanskabystrica", "visitbanskabystrica-tribe.json")
        if e["name"]["value"] == "Richard Autner & band"
    ]
    assert len(wp) == 1 and len(tribe) == 1
    assert wp[0]["id"] == tribe[0]["id"] and wp[0]["id"].startswith(
        "urn:ngsi-ld:Event:bbsk-podujatia:"
    )


@requires_docker
def test_an_event_without_name_or_start_is_refused_and_one_without_gps_kept():
    day = datetime.fromtimestamp(NOW, UTC) + timedelta(days=3)
    no_gps = wp_event(1, day)
    no_gps["meta_box"]["coordinates"] = None
    no_name = {**wp_event(2, day), "title": {"rendered": " "}}
    no_start = wp_event(3, day)
    no_start["meta_box"]["date_start"] = None
    out = {
        e["sourceId"]["value"]: e
        for e in run(
            offline(processors("banskabystrica"), {}),
            json.dumps([no_gps, no_name, no_start, wp_event(4, day)]),
        )
    }
    assert VALID.is_valid(key_values(out["4"])) and VALID.is_valid(key_values(out["1"]))
    assert not VALID.is_valid(key_values(out["2"])) and "name" not in out["2"]
    # A record without a start is never stored without a date (owner 2026-10-22, T-5215); one without GPS that
    # names no place takes the town office's point (homes.csv).
    assert "3" not in out
    assert out["1"]["location"]["value"]["coordinates"] == home_point("https://www.banskabystrica.sk/podujatia/")


# --- the national space (T-4898) ----------------------------------------------------------------------------


def test_every_source_writes_its_own_region_and_only_bbsk_sources_reach_the_regions_store():
    """A source elsewhere never writes under /SK/BB, and the scope comes from the inventory row, not from the
    feed; the region's own store holds its own sources alone (T-5047)."""
    regions = {r["region"] for r in generate.kept_sources()}
    assert regions == set(generate.REGIONS)
    bbsk = {p.name for p in SEED.glob("bbsk-pipeline-podujatia-*-bento.yaml")}
    assert bbsk == {f"bbsk-pipeline-{r['name']}-bento.yaml" for r in generate.kept_sources() if r["region"] == "BBSK"}
    for row in generate.kept_sources():
        code, name = generate.REGIONS[row["region"]]
        if row["region"] in generate.COUNTRIES:
            # A source outside Slovakia never reaches the national store (T-5666).
            assert not (NATIONAL / f"slovensko-pipeline-{row['name']}-bento.yaml").exists(), row["name"]
            continue
        text = (NATIONAL / f"slovensko-pipeline-{row['name']}-bento.yaml").read_text()
        # The shared tail writes "scope" from @events.scope and "addressRegion" from @events.region (T-5555).
        assert f'"scope": "/SK/{code}"' in text, row["name"]
        assert f'"region": "{name}"' in text, row["name"]
        assert ('"/SK/BB"' in text) == (row["region"] == "BBSK"), row["name"]


def test_every_scope_a_pipeline_can_write_is_a_valid_ngsi_ld_scope():
    """The town comes from the inventory row or the venue table, both slugs with hyphens; the
    template writes them with "_", so the broker takes the scope (T-5051: six sources got 403)."""
    towns = {r["town_slug"] for r in generate.kept_sources()} - {None}
    with (EVENTS / "venues.csv").open(encoding="utf-8") as f:
        towns |= {v["town"] for v in csv.DictReader(f)}
    assert "banska-bystrica" in towns and "velky-krtis" in towns
    for row in generate.kept_sources():
        code, _ = generate.REGIONS[row["region"]]
        assert SCOPE.fullmatch(f"/SK/{code}")
    for town in towns:
        assert SCOPE.fullmatch("/SK/BB/" + town.replace("-", "_")), town


@requires_docker
def test_an_events_calendar_time_without_seconds_is_read():
    """visithumenne.sk writes "2026-11-03 19:00": before T-5051 the template failed on it and the
    unchanged object reached the tail, which wants an array ("expected array value, got object")."""
    feed = json.loads((FIXTURES / "visitbanskabystrica-tribe.json").read_text())
    feed["events"][0].update(
        start_date="2026-11-03 19:00", end_date="2026-11-03 20:40:00"
    )
    entities = run(offline(processors("visithumenne", "slovensko"), {}), json.dumps(feed), "slovensko-podujatia")
    assert [e["startDate"]["value"] for e in entities] == ["2026-11-03T19:00:00+01:00"]
    assert entities[0]["endDate"]["value"] == "2026-11-03T20:40:00+01:00"
    assert entities[0]["scope"] == "/SK/PO/humenne"


def test_each_dataset_lists_its_sources_with_their_licence():
    def listed(file: str) -> dict:
        spec = yaml.safe_load((SEED / file).read_text())["spec"]
        return {s["url"]: s for s in spec["catalog"]["source"]}

    kept = [r for r in generate.kept_sources() if r["region"] in generate.SLOVAK]
    prague = {s["url"] for s in yaml.safe_load((PRAHA / "praha-endpoint-akce.yaml").read_text())["spec"]["catalog"]["source"]}
    assert prague == {r["events_url"] for r in generate.kept_sources() if r["region"] == "PHA"}
    national = {s["url"]: s for s in yaml.safe_load((NATIONAL / "slovensko-endpoint-podujatia.yaml").read_text())["spec"]["catalog"]["source"]}
    region = listed("bbsk-endpoint-podujatia.yaml")
    assert set(national) == {r["events_url"] for r in kept}
    assert set(region) == {r["events_url"] for r in kept if r["region"] == "BBSK"}
    for row in kept:
        text = national[row["events_url"]]["description"]["en"]
        if row["licence_terms"].startswith("CC-BY"):
            assert "licence CC-BY" in text, row["name"]
        else:
            assert "no licence stated, facts only" in text, row["name"]


@requires_docker
def test_a_source_outside_the_region_lands_in_its_own_region_at_its_venue():
    # Recorded 2026-10-21 (start_date=now): four events at Dom umenia, one with no venue.
    out = {
        e["sourceId"]["value"]: e
        for e in run(offline(processors("domumenia-piestany", "slovensko"), {}),
                     (FIXTURES / "piestany-tribe.json").read_text(), "slovensko-podujatia")
    }
    assert len(out) == 5
    placed = [e for e in out.values() if e["locationPrecision"]["value"] == "building"]
    assert len(placed) == 4
    # The one with no venue takes Piešťany's office (T-5240).
    (other,) = [e for e in out.values() if e not in placed]
    assert other["location"]["value"]["coordinates"] == town_point("piestany") and other["locationPrecision"]["value"] == "office"
    for e in placed:
        assert e["scope"] == "/SK/TT/piestany"
        assert e["location"]["value"]["coordinates"] == [17.8400391, 48.5923954]
        assert e["address"]["value"]["addressRegion"] == "Trnavský kraj"
        assert "description" not in e  # no licence stated: facts only
        assert VALID.is_valid(key_values(e)), e["name"]
    assert VALID.is_valid(key_values(other))


# --- the national store's adapters (T-5047) -----------------------------------------------------------------


def national(kind: str, host: str, payload: str, pages: str | None = None, refused: bool = False, path: str = "/",
             org_type: str = "municipality") -> list:
    recorded = json.loads((FIXTURES / "pages" / pages).read_text()) if pages else {}
    return run(offline(template(kind, host, path=path, org_type=org_type), recorded), payload, "slovensko-podujatia", refused)


@requires_docker
def test_lsvr_events_take_their_dates_from_the_pages_json_ld_written_with_a_space():
    # Recorded 2026-10-21 from casta.sk: LSVR writes "2026-12-31 23:59:00", Bratislava's wall time.
    out = {e["name"]["value"]: e for e in national("lsvr-rest", "www.casta.sk", (FIXTURES / "casta-lsvr.json").read_text(), "lsvr.json")}
    assert len(out) == 3
    toast = out["Novoročný prípitok v obci"]
    assert toast["startDate"]["value"] == "2026-12-31T23:59:00+01:00"
    assert toast["endDate"]["value"] == "2027-01-01T03:00:00+01:00"
    assert toast["id"].startswith("urn:ngsi-ld:Event:slovensko-podujatia:") and toast["scope"] == "/SK/BA/casta"
    # Častá's events name no place of their own, so they take the village office's point (homes.csv, T-5215),
    # never the village centre (owner 2026-10-20).
    office = home_point("https://www.casta.sk/")
    assert all(e["location"]["value"]["coordinates"] == office for e in out.values())


@requires_docker
def test_a_printed_date_with_a_leading_zero_day_or_month_is_read():
    # int64() reads "08" and "09" as octal: before T-5287 each such event failed its branch and was never written.
    printed = {"a": "09. 08. 2026", "b": "08. 09. 2026", "c": "01. 10. 2026"}
    items = [{"id": i, "link": f"https://www.casta.sk/events/{k}/", "title": {"rendered": f"Podujatie {k}"}, "modified_gmt": None}
             for i, k in enumerate(printed)]
    pages = {f"/events/{k}/": '<ul class="post__info"><li class="post__info-item post__info-item--date">'
             f'<p class="post__info-item-text">{v}</p></li></ul>' for k, v in printed.items()}
    # The clock sits before every date, so none of them is past and dropped.
    early = int(datetime(2026, 8, 1, tzinfo=UTC).timestamp())
    steps = json.loads(json.dumps(offline(template("lsvr-rest", "www.casta.sk"), pages)).replace(f"let now = {NOW}", f"let now = {early}"))
    out = {e["name"]["value"]: e["startDate"]["value"] for e in run(steps, json.dumps(items), "slovensko-podujatia", now=early)}
    assert out == {"Podujatie a": "2026-08-09", "Podujatie b": "2026-09-08", "Podujatie c": "2026-10-01"}


@requires_docker
def test_a_national_event_leaves_a_day_after_its_end_or_8_hours_after_the_last_run_that_saw_it():
    out = {e["name"]["value"]: e for e in national("lsvr-rest", "www.casta.sk", (FIXTURES / "casta-lsvr.json").read_text(), "lsvr.json")}
    lease = datetime.fromtimestamp(NOW + 8 * 3600, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    # Every event here ends later than the lease, so the lease decides: a source that drops it ends it in 8 h.
    assert {e["expiresAt"] for e in out.values()} == {lease}
    day = datetime.fromtimestamp(NOW, UTC)

    def ended(hours_ago: int) -> list:
        item = [{"id": 9, "link": "https://www.casta.sk/events/vcera/", "title": {"rendered": "Včera"}, "modified_gmt": None}]
        at = lambda h: (day - timedelta(hours=h)).strftime("%Y-%m-%dT%H:%M:%S+00:00")
        page = '<script type="application/ld+json">{"@type":"Event","startDate":"%s","endDate":"%s"}</script>' % (
            at(hours_ago + 2), at(hours_ago))
        return run(offline(template("lsvr-rest", "www.casta.sk"), {"/events/vcera/": page}), json.dumps(item), "slovensko-podujatia")

    # An event that ended 20 hours ago stays until a day after its end, earlier than the lease; one that ended
    # 25 hours ago is past its day and never loaded.
    (yesterday,) = ended(20)
    assert yesterday["expiresAt"] == (day + timedelta(hours=4)).strftime("%Y-%m-%dT%H:%M:%SZ")
    assert ended(25) == []


@requires_docker
def test_a_region_event_leaves_52_hours_after_the_last_run_that_saw_it():
    # T-5081: without a lease a region event the model now refuses (or its source dropped) stayed half until its end.
    steps = template("lsvr-rest", "www.casta.sk", store="bbsk", region="BBSK")
    pages = json.loads((FIXTURES / "pages" / "lsvr.json").read_text())
    out = run(offline(steps, pages), (FIXTURES / "casta-lsvr.json").read_text())
    lease = datetime.fromtimestamp(NOW + 52 * 3600, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    assert out and {e["expiresAt"] for e in out} == {lease}


@requires_docker
def test_a_wordpress_list_that_is_no_list_fails_the_run_and_an_empty_one_writes_nothing():
    national("lsvr-rest", "www.casta.sk", '{"code":"rest_no_route"}', refused=True)
    assert national("lsvr-rest", "www.casta.sk", "[]") == []


@requires_docker
def test_an_item_off_the_sources_host_or_with_dot_segments_is_never_fetched():
    items = [{"id": 1, "link": "https://www.casta.sk@attacker.example/events/x/", "title": {"rendered": "A"}},
             {"id": 2, "link": "https://www.casta.sk/events/../../etc/", "title": {"rendered": "B"}}]
    # Neither page is read, so neither has a date: both are refused (T-5215).
    assert national("lsvr-rest", "www.casta.sk", json.dumps(items)) == []


@requires_docker
def test_an_rss_feed_of_event_posts_reads_each_posts_json_ld():
    rss = """<?xml version="1.0"?><rss version="2.0"><channel><title>Podujatia</title>
<item><title>Novoročný prípitok v obci</title><link>https://www.casta.sk/events/novorocny-pripitok-v-obci-2/</link><guid>p-1</guid></item>
</channel></rss>"""
    (e,) = national("wp-rss-jsonld", "www.casta.sk", rss, "lsvr.json")
    assert e["startDate"]["value"] == "2026-12-31T23:59:00+01:00" and e["sourceId"]["value"] == "p-1"
    national("wp-rss-jsonld", "www.casta.sk", "<html>not a feed</html>", refused=True)


@requires_docker
def test_lego_events_read_each_pages_machine_readable_time_and_venue():
    # Recorded 2026-10-21 from jaslovske-bohunice.sk (Lego CMS); the Christmas market page is that page with
    # its title and time changed, so one event is past and one upcoming on the fixtures' day.
    html = (FIXTURES / "jaslovske-bohunice-lego.html").read_text()
    out = national("lego-html", "www.jaslovske-bohunice.sk", html, "lego.json", path="/zivot-v-obci/najblizsie-podujatia/")
    # The off-host and the news link are never read; the seniors' afternoon of 18 October stays its day of grace.
    by_name = {e["name"]["value"]: e for e in out}
    assert set(by_name) == {"Vianočné trhy", "Posedenie dôchodcov"}
    market = by_name["Vianočné trhy"]
    assert market["name"]["value"] == "Vianočné trhy"
    assert market["startDate"]["value"] == "2026-12-12T10:00:00+01:00"
    assert market["endDate"]["value"] == "2026-12-12T18:00:00+01:00"
    assert market["venue"]["value"] == "Športový areál ŠK Blava - športová hala č.2"
    assert market["eventCategory"]["value"] == "ostatné (akcie pre seniorov)"
    assert "description" not in market  # no licence stated: facts only
    assert national("lego-html", "www.jaslovske-bohunice.sk", "<html><body>Žiadne podujatia</body></html>") == []


@requires_docker
def test_a_page_with_its_events_as_json_ld_gives_each_event_and_a_page_without_json_ld_fails():
    page = """<html><script type="application/ld+json">{"@context":"https://schema.org","@graph":[{"@type":"WebPage"},
{"@type":"MusicEvent","name":"Lúčnica: Vianoce","startDate":"2026-12-20T19:00","url":"https://www.lucnica.sk/vianoce",
 "location":{"@type":"Place","name":"Istropolis","address":{"@type":"PostalAddress","streetAddress":"Trnavské mýto 1",
 "addressLocality":"Bratislava"},"geo":{"latitude":48.1601,"longitude":17.1290}}}]}</script>
<script type="application/ld+json">not json</script></html>"""
    (e,) = national("jsonld", "www.lucnica.sk", page)
    assert e["startDate"]["value"] == "2026-12-20T19:00:00+01:00"
    assert e["location"]["value"]["coordinates"] == [17.129, 48.1601]
    assert e["address"]["value"]["streetAddress"] == "Trnavské mýto 1" and VALID.is_valid(key_values(e))
    national("jsonld", "www.lucnica.sk", "<html><body>Warning: require_once()</body></html>", refused=True)


# --- organizer and link (T-5081) -----------------------------------------------------------------------------


def tribe_event(i: int, organizer: list | dict, url: str | None = "default") -> dict:
    day = (datetime.fromtimestamp(NOW, UTC) + timedelta(days=5)).strftime("%Y-%m-%d")
    return {
        "id": i, "title": f"Koncert {i}", "start_date": f"{day} 19:00:00", "end_date": f"{day} 21:00:00", "all_day": False,
        "url": f"https://www.casta.sk/udalost/koncert-{i}/" if url == "default" else url,
        "venue": {"venue": "Kultúrny dom", "address": "Hlavná 1", "city": "Častá", "geo_lat": "48.39", "geo_lng": "17.35"},
        "organizer": organizer,
    }


@requires_docker
def test_the_events_calendar_organizer_gives_its_name_and_web_site_and_never_a_contact():
    feed = {"events": [
        tribe_event(1, [{"organizer": "Stredoslovenské múzeum", "website": "https://www.stredoslovenskemuzeum.sk",
                         "email": "info@muzeum.sk", "phone": "+421 48 412 58 97"}]),
        tribe_event(2, [{"organizer": "Ján Novák, tel. 0905 123 456, jan.novak@example.sk", "website": "mailto:jan@example.sk"}]),
        tribe_event(3, []),
    ]}
    out = {e["sourceId"]["value"]: e for e in national("tribe-rest", "www.casta.sk", json.dumps(feed))}
    assert out["1"]["organizer"]["value"] == {"name": "Stredoslovenské múzeum", "url": "https://www.stredoslovenskemuzeum.sk"}
    # A contact typed into the name is cut out; a mailto: is no web site.
    assert out["2"]["organizer"]["value"] == {"name": "Ján Novák"}
    # The feed names no organizer: the municipality whose calendar it is runs the event.
    assert out["3"]["organizer"]["value"] == {"name": "Obec Častá", "url": "https://www.casta.sk"}
    for e in out.values():
        assert VALID.is_valid(key_values(e)), list(VALID.iter_errors(key_values(e)))
        assert "@" not in json.dumps(e["organizer"]) and "0905" not in json.dumps(e["organizer"])


@requires_docker
def test_an_aggregators_event_keeps_no_organizer_and_is_kept():
    feed = {"events": [tribe_event(1, []), tribe_event(2, {"organizer": "Divadlo J. G. Tajovského", "website": ""})]}
    out = {e["sourceId"]["value"]: e for e in national("tribe-rest", "www.casta.sk", json.dumps(feed),
                                                       org_type="tourism organisation (OOCR Test)")}
    assert "organizer" not in out["1"] and VALID.is_valid(key_values(out["1"]))
    assert out["2"]["organizer"]["value"] == {"name": "Divadlo J. G. Tajovského"} and VALID.is_valid(key_values(out["2"]))


@requires_docker
def test_an_event_without_its_own_page_links_to_the_organizers_listing_page():
    feed = {"events": [tribe_event(1, [], url=None), tribe_event(2, [], url="javascript:alert(1)")]}
    for e in national("tribe-rest", "www.casta.sk", json.dumps(feed)):
        assert e["url"]["value"] == "https://www.casta.sk/" and VALID.is_valid(key_values(e))


@requires_docker
def test_json_ld_organizer_and_weby_usporiadatel_and_ical_organizer_are_read():
    page = """<script type="application/ld+json">{"@type":"Event","name":"Prednáška","startDate":"2026-11-20T17:00",
 "url":"https://www.lucnica.sk/p","organizer":[{"@type":"Organization","name":"Univerzita Mateja Bela","url":"https://www.umb.sk",
 "email":"x@umb.sk"}],"location":{"name":"Aula","geo":{"latitude":48.73,"longitude":19.15}}}</script>"""
    (e,) = national("jsonld", "www.lucnica.sk", page)
    assert e["organizer"]["value"] == {"name": "Univerzita Mateja Bela", "url": "https://www.umb.sk"}
    # MEC writes the café that runs the evening as its organizer.
    mec = {x["name"]["value"]: x for x in pipeline("banskastiavnica", "banskastiavnica-mec.json", "mec.json")}
    assert mec["Vedatour / Vedátor v Art Cafe"]["organizer"]["value"]["name"] == "Art Café"
    lucenec = {x["name"]["value"]: x for x in pipeline("lucenec", "lucenec-atom.xml", "weby.json")}
    assert lucenec["SILNÉ REČI"]["organizer"]["value"] == {"name": "Mesto Lučenec"}
    ics = "\r\n".join(["BEGIN:VCALENDAR", "BEGIN:VEVENT", "UID:u-1", "SUMMARY:Jarmok", "DTSTART:20261125T090000Z",
                        'ORGANIZER;CN="Mesto Zvolen":mailto:podatelna@zvolen.sk', "GEO:48.57;19.12",
                        "URL:https://www.zvolen.sk/jarmok", "END:VEVENT", "END:VCALENDAR"])
    (ev,) = national("ical", "www.zvolen.sk", ics)
    assert ev["organizer"]["value"] == {"name": "Mesto Zvolen"} and "podatelna" not in json.dumps(ev)
    assert VALID.is_valid(key_values(ev))


def test_every_source_names_an_organizer_unless_it_is_an_aggregator():
    for row in generate.kept_sources():
        organizer = generate.organizer(row)
        if generate.AGGREGATORS.match(row["org_type"]):
            assert organizer is None, row["name"]
        else:
            assert organizer and organizer["name"] and organizer["url"].startswith("https://"), row["name"]


# --- HTML kinds of the universities and geocoded venues (T-5083) --------------------------------------------


def bbsk_html(kind: str, host: str, town: str, path: str, org_type: str, payload: str, pages: str) -> dict:
    steps = template(kind, host, store="bbsk", region="BBSK", town=town, path=path, org_type=org_type)
    recorded = json.loads((FIXTURES / "pages" / pages).read_text())
    return {e["name"]["value"]: e for e in run(offline(steps, recorded), payload)}


@requires_docker
def test_umb_events_read_each_pages_event_table_and_place_it_by_the_geocoded_venue():
    # Recorded 2026-10-21 from umb.sk; the third page has a made-up contact row added, which is never read.
    out = bbsk_html("umb-cms", "www.umb.sk", "Banská Bystrica", "/kalendar-udalosti/",
                    "university (Univerzita Mateja Bela v Banskej Bystrici)",
                    (FIXTURES / "umb-kalendar.html").read_text(), "umb.json")
    # The menu link without an h3 title, the off-host and the off-path links are never read.
    assert set(out) == {"GAUDEAMUS BRNO", "Školenie: Od konfliktu k porozumeniu: Ako vystúpiť z dramatického trojuholníka",
                        "Občianske fórum 2026"}
    school = out["Školenie: Od konfliktu k porozumeniu: Ako vystúpiť z dramatického trojuholníka"]
    assert school["startDate"]["value"] == "2026-10-27T13:30:00+01:00"
    assert school["endDate"]["value"] == "2026-10-27T15:30:00+01:00"
    assert school["organizer"]["value"] == {"name": "Centrum podpory a ďalšieho vzdelávania UMB"}
    # The room text matches the venue geocoded from its address, never the town's centre.
    assert school["location"]["value"]["coordinates"] == [19.1376321, 48.7395768]
    assert school["url"]["value"].startswith("https://www.umb.sk/kalendar-udalosti/skolenie-")
    forum = out["Občianske fórum 2026"]
    assert VALID.is_valid(key_values(forum)), list(VALID.iter_errors(key_values(forum)))
    assert "nováková" not in json.dumps(forum, ensure_ascii=False).lower() and "@" not in json.dumps(forum)
    # A fair in Brno: no venue of Banská Bystrica's table, so no point; the event is kept. "Iné" is no
    # organizer: the university whose calendar it is stands in.
    brno = out["GAUDEAMUS BRNO"]
    assert brno["address"]["value"]["addressLocality"] == "Brno" and "location" not in brno
    assert VALID.is_valid(key_values(brno))
    assert brno["organizer"]["value"] == {"name": "Univerzita Mateja Bela v Banskej Bystrici", "url": "https://www.umb.sk"}


@requires_docker
def test_tuzvo_events_come_from_this_and_the_next_months_list_without_the_persons_named():
    # Recorded 2026-10-21 from tuzvo.sk, the persons' names replaced; November repeats one October event.
    out = bbsk_html("drupal-kalendar", "www.tuzvo.sk", "Zvolen", "/sk/kalendar-podujati",
                    "university (Technická univerzita vo Zvolene)",
                    (FIXTURES / "tuzvo-kalendar.html").read_text(), "tuzvo.json")
    assert "Krajina - človek - kultúra" in out and "Výstava plodov starých odrôd ovocných drevín" in out
    assert len(out) == len({e["id"] for e in out.values()})
    fair = out["Výstava plodov starých odrôd ovocných drevín"]
    assert fair["startDate"]["value"] == "2026-11-04T09:00:00+01:00"
    assert fair["venue"]["value"] == "FEE E214" and fair["eventCategory"]["value"]
    assert fair["organizer"]["value"]["name"] and fair["url"]["value"].startswith("https://www.tuzvo.sk/sk/")
    assert fair["location"]["value"]["coordinates"] == [19.1165987, 48.5729663]
    assert "Test Osoba" not in json.dumps(out, ensure_ascii=False)
    # A venue nobody geocoded (ÚTVŠ) takes Zvolen's office, labelled approximate (T-5240).
    sport = out["Súčasné trendy v športe, pohybových aktivitách a životnom štýle"]
    assert sport["location"]["value"]["coordinates"] == town_point("zvolen") and VALID.is_valid(key_values(sport))
    assert sport["locationPrecision"]["value"] == "office"


def test_the_next_month_is_read_from_the_same_list_page_10_seconds_later():
    steps = template("drupal-kalendar", "www.tuzvo.sk", store="bbsk", region="BBSK", town="Zvolen",
                     path="/sk/kalendar-podujati", org_type="university (Technická univerzita vo Zvolene)")
    (branch,) = [s["branch"] for s in steps if "branch" in s]
    assert branch["processors"][0] == {"sleep": {"duration": "10s"}}
    assert branch["processors"][1]["http"]["url"] == "https://www.tuzvo.sk/${! @event_path }"
    assert '"/sk/kalendar-podujati".trim_prefix("/") + "?mini="' in branch["request_map"]


@requires_docker
def test_an_event_naming_no_place_or_only_the_town_takes_its_organizers_address():
    # T-5135, owner 2026-10-22: GPS from the source, the venue table, or the institution's address; never a town centre.
    def event(i: int, venue) -> dict:
        e = tribe_event(i, [], url=f"https://svkbb.eu/udalosti/e-{i}/")
        e["venue"] = venue
        return e

    feed = {"events": [
        event(1, []),
        event(2, {"venue": "Banská Bystrica", "city": "Banská Bystrica"}),
        event(3, {"venue": "Neznáma sála", "city": "Banská Bystrica"}),
        event(4, {"venue": "Záhrada", "city": "Banská Bystrica", "geo_lat": "48.7336", "geo_lng": "19.1423"}),
    ]}
    steps = offline(processors("svkbb"), {})
    out = {e["name"]["value"]: e for e in run(steps, json.dumps(feed))}
    library = [19.1436671, 48.7373616]
    assert out["Koncert 1"]["location"]["value"]["coordinates"] == library and "venue" not in out["Koncert 1"]
    assert out["Koncert 1"]["address"]["value"]["streetAddress"] == "Lazovná 240/9"
    assert out["Koncert 2"]["location"]["value"]["coordinates"] == library
    assert out["Koncert 2"]["venue"]["value"] == "Banská Bystrica", "the venue stays as the source wrote it"
    # A place nobody placed takes the town's point, never the organizer's (T-5240).
    assert out["Koncert 3"]["location"]["value"]["coordinates"] == town_point("banska-bystrica")
    assert out["Koncert 3"]["locationPrecision"]["value"] == "centre" and out["Koncert 1"]["locationPrecision"]["value"] == "office"
    assert out["Koncert 4"]["location"]["value"]["coordinates"] == [19.1423, 48.7336]
    assert all(VALID.is_valid(key_values(e)) for e in out.values())


def test_every_home_names_one_venue_of_a_kept_source_and_no_aggregator():
    homes = list(csv.DictReader((EVENTS / "homes.csv").open(encoding="utf-8")))
    kept = {r["events_url"]: r for r in generate.kept_sources()}
    assert homes and len({h["events_url"] for h in homes}) == len(homes)
    for h in homes:
        assert h["events_url"] in kept, h
        assert generate.organizer(kept[h["events_url"]]) is not None, f"an aggregator has no address of its own: {h}"
        assert generate.home(kept[h["events_url"]])["match"] == h["match"]


# --- T-5215: dates from the page, notices out, one entity per event ---------------------------------------------


def lsvr_page(info: str, ld_start: str = "") -> str:
    """An LSVR (TownPress) event page as recorded 2026-10-22 from obecpredajna.sk: empty JSON-LD dates."""
    return (
        '<script type="application/ld+json">{"@context":"http://schema.org","@type":"Event","name":"X",'
        f'"startDate":"{ld_start}","endDate":""}}</script><main>{info}</main>'
    )


MULTIDAY = """<ul class="post__info post__info--multiday"> <li class="post__info-item post__info-item--date">
<h3 class="post__info-item-title">Začiatok</h3> <p class="post__info-item-text"> 28. augusta 2027 <br>Celodenná udalosť </p> </li>
<li class="post__info-item post__info-item--date"> <h3 class="post__info-item-title">Koniec</h3>
<p class="post__info-item-text"> 30. augusta 2027 <br>Celodenná udalosť </p> </li></ul>"""
SINGLEDAY = """<ul class="post__info post__info--singleday"> <li class="post__info-item post__info-item--date">
<h3 class="post__info-item-title">Dátum</h3> <p class="post__info-item-text"> 7. 12. 2026 </p> </li>
<li class="post__info-item post__info-item--time"> <h3 class="post__info-item-title">Čas</h3>
<p class="post__info-item-text"> 8:00 - 12:30 </p> </li></ul>"""


def wp_items(*names: str) -> str:
    return json.dumps([{"id": i, "link": f"https://www.casta.sk/events/e{i}/", "title": {"rendered": n}}
                       for i, n in enumerate(names)])


@requires_docker
def test_an_lsvr_page_without_json_ld_dates_gives_the_dates_it_prints():
    pages = {"/events/e0/": lsvr_page(MULTIDAY), "/events/e1/": lsvr_page(SINGLEDAY),
             "/events/e2/": lsvr_page("<p>Bez dátumu</p>")}
    steps = offline(template("lsvr-rest", "www.casta.sk"), pages)
    out = {e["name"]["value"]: e for e in run(steps, wp_items("Biatlon", "Jarmok", "Nič"), "slovensko-podujatia")}
    assert out["Biatlon"]["startDate"]["value"] == "2027-08-28" and out["Biatlon"]["endDate"]["value"] == "2027-08-30"
    assert out["Jarmok"]["startDate"]["value"] == "2026-12-07T08:00:00+01:00"
    assert out["Jarmok"]["endDate"]["value"] == "2026-12-07T12:30:00+01:00"
    assert "Nič" not in out, "a page that prints no date is refused, never stored dateless"


@requires_docker
def test_an_eventon_offset_with_a_one_digit_hour_is_read():
    # Recorded 2026-10-22 from k13.sk (EventON): "2026-11-14T10:00+2:00".
    page = ('<script type="application/ld+json">{"@context":"http://schema.org","@type":"Event","name":"SteelPark",'
            '"startDate":"2027-11-14T10:00+2:00","endDate":"2027-11-15T18:00+1:00"}</script>')
    (e,) = run(offline(template("eventon-rest", "www.casta.sk"), {"/events/e0/": page}), wp_items("SteelPark"),
               "slovensko-podujatia")
    assert e["startDate"]["value"] == "2027-11-14T10:00:00+02:00" and e["endDate"]["value"] == "2027-11-15T18:00:00+01:00"


@requires_docker
def test_notices_are_never_loaded_for_any_source_kind():
    notices = ["Vývoz triedeného zberu plastov", "Odvoz komunálneho odpadu", "Zber papiera", "Papier",
               "Zasadnutie OZ – 30.09.2026", "Oznámenie o prerušení distribúcie elektriny", "VZN č. 3/2026",
               "Zámer predaja pozemku", "Komunálne voľby", "Svadba", "Rodinná oslava",
               # A hall's booking calendar (Ďurkov on dev 2026-10-24, T-5393): its slots, booked or free, are no events.
               "Oslava", "KD obsadený - súkromná akcia", "Rezervácia jedáleň ocu - J.M.", "Rezervované", "Voľné",
               "14.11.2026 Prenájom sály KD- súkromná oslava",
               # Recorded on dev 2026-10-22 under "akcie pre celú rodinu" (T-5363).
               "Vývoz smetných nádob", "ZBERNÝ DVOR"]
    events = ["Mikuláš v obci", "Koncert skupiny FIDEM", "Zberateľská burza", "Katarínska zábava", "Oslava 800. výročia obce"]
    day = datetime.fromtimestamp(NOW, UTC) + timedelta(days=3)
    # wp-rest-sc (feed with dates), lsvr-rest (page JSON-LD), lego-html and every other kind share common.yaml.
    sc = run(offline(processors("banskabystrica"), {}),
             json.dumps([wp_event(i, day, title=t) for i, t in enumerate(notices + events)]))
    assert sorted(e["name"]["value"] for e in sc) == sorted(events)
    page = '<script type="application/ld+json">{"@type":"Event","startDate":"' + day.strftime("%Y-%m-%dT%H:%M:%S+00:00") + '"}</script>'
    pages = {f"/events/e{i}/": page for i in range(len(notices + events))}
    lsvr = run(offline(template("lsvr-rest", "www.casta.sk"), pages), wp_items(*notices, *events), "slovensko-podujatia")
    assert sorted(e["name"]["value"] for e in lsvr) == sorted(events)


@requires_docker
def test_one_event_written_by_two_sources_with_other_punctuation_or_a_minute_apart_is_one_entity():
    # Recorded 2026-10-22 on dev: banskabystrica.sk and vkmk.sk wrote "Kintsugi – japonské umenie" and
    # "Kintsugi: japonské umenie", and "Čarovný kľúčik" at 16:35 and 16:36.
    def one(title: str, at: str) -> str:
        page = '<script type="application/ld+json">{"@type":"Event","startDate":"' + at + '"}</script>'
        (e,) = run(offline(template("lsvr-rest", "www.casta.sk"), {"/events/e0/": page}), wp_items(title), "slovensko-podujatia")
        return e["id"]

    assert one("Kintsugi – japonské umenie", "2027-11-19T16:30:00+01:00") == one("Kintsugi: japonské umenie", "2027-11-19T16:30:00+01:00")
    assert one("Čarovný kľúčik", "2027-11-18T16:35:00+01:00") == one("Čarovný kľúčik", "2027-11-18T16:36:00+01:00")
    assert one("Korčuľovanie", "2027-10-29T10:30:00+01:00") != one("Korčuľovanie", "2027-10-29T15:00:00+01:00")


# --- the main category (T-5363) -----------------------------------------------------------------------------

#: Source words and names recorded on dev 2026-10-22 (slovensko-podujatia) with the category the rule table gives
#: them: the source's own word first, without its audience and catch-all parts, then the name, then the whole word.
CATEGORISED = [
    ("hudba (tanečná zábava)", "Katarínska zábava", "music"),
    ("ostatné (akcie pre celú rodinu)", "Svadobná veselica", "music"),
    ("ostatné (akcie pre celú rodinu)", "Mikuláš", "festival-fair"),
    ("trhy, výstavy, veľtrhy (výstava fotografií)", "Svet okolo nás", "exhibition-museum"),
    ("trhy, výstavy, veľtrhy (ostatné)", "Jesenné stretnutie", "food-market"),
    ("trhy, výstavy, veľtrhy (jarmok, trhy)", "Vianočné trhy 2026", "festival-fair"),
    ("šport, aktívne trávenie voľného času (turistika)", "Štefanský výstup na Jahodnisko", "sport"),
    ("cirkevný (ostatné)", "Poklona", "faith-tradition"),
    ("kino - film (filmový festival), ostatné (prezentácia)", "SVET A MY 2026", "film"),
    ("Kino, Pre deti", "Ovečka Shaun vo filme", "film"),
    ("ostatné (prednáška)", "Beseda s Józsefom Demmelom", "education-talk"),
    ("ostatné (pietna akcia)", "Pietna spomienka ukončenia 1. sv. vojny", "history-heritage"),
    ("gastronómia (ochutnávka jedál)", "Októberfest - Kúria Beckov", "food-market"),
    ("divadlo (ochotnícke divadlo)", "Spiknutie", "theatre-dance"),
    ("Šport", "Korčuľovanie pre verejnosť Zimný štadión", "sport"),
    (None, "Lampiónový sprievod Skalica", "festival-fair"),
    (None, "Mobilná kvapka krvi v Hriňovej", "health-wellness"),
    (None, "Jesenné modrotlačenie", "hobby-workshop"),
    (None, "Finisáž s kurátorskou komentovanou prehliadkou – Kozmická krajina", "exhibition-museum"),
    (None, "Turnaj v šachu o majstra mesta Turany", "sport"),
    (None, "Knižný klub Živena", "education-talk"),
    (None, "Music Argentum: Andrej Farkaš Trio", "music"),
    (None, "Tradičné zabíjačkové špeciality z Hontu", "food-market"),
    (None, "Stretnutie seniorov", "community-civic"),
    (None, "Strašidelná noc", "festival-fair"),
    (None, "Exkurzia po náučnom chodníku v prírode", "outdoor-nature"),
    ("Pre deti", "Santova čarovná dielňa", "children-family"),
    ("ostatné (akcie pre celú rodinu)", "ZDRUFF", "children-family"),
]


def categorised(cases: list, org_type: str = "municipality") -> list:
    """The entities the shared tail writes for (source word, name) records, in the cases' order."""
    tail = template("wp-rest-sc", "www.casta.sk", org_type=org_type)[-2:]
    day = (datetime.fromtimestamp(NOW, UTC) + timedelta(days=3)).strftime("%Y-%m-%dT18:00:00+02:00")
    records = [{"name": name, "start": day, "category": word, "url": f"https://www.casta.sk/e{i}", "sourceId": str(i)}
               for i, (word, name, *_) in enumerate(cases)]
    out = {e["sourceId"]["value"]: e for e in run(offline(tail, {}), json.dumps(records), "slovensko-podujatia")}
    return [out[str(i)] for i in range(len(cases))]


@requires_docker
def test_each_recorded_source_word_and_name_gets_its_main_category():
    for (word, name, code), e in zip(CATEGORISED, categorised(CATEGORISED)):
        assert (e.get("category", {}).get("value"), e.get("categorySource", {}).get("value")) == ([code], "rule"), (word, name)
        assert VALID.is_valid(key_values(e)), list(VALID.iter_errors(key_values(e)))
        # The source's own word stays for traceability.
        assert e.get("eventCategory", {}).get("value") == word
    audience = [e.get("audience", {}).get("value") for e in categorised(CATEGORISED[-2:])]
    assert audience == [["children"], ["family"]]


@requires_docker
def test_an_event_no_rule_names_has_no_category_unless_its_body_holds_one_kind():
    cases = [("ostatné (nešpecifikované/ostatné)", "ZDRUFF"), (None, "Hriešnice")]
    for e in categorised(cases):
        assert {"category", "categorySource", "audience"}.isdisjoint(e), e
    # A theatre's calendar holds plays: its title alone ("Hriešnice") is enough.
    assert [e["category"]["value"] for e in categorised(cases, "theatre (Divadlo GUnaGU)")] == [["theatre-dance"]] * 2


@requires_docker
def test_an_event_no_rule_names_takes_the_decision_models_cached_answer(monkeypatch):
    """scripts/events-classify.py caches a code per source and hash of name and venue; the pipeline reads it (T-5490)."""
    cached = hashlib.sha256("zdruff|".encode()).hexdigest()[:16]
    monkeypatch.setitem(generate.DECIDED, "https://www.casta.sk/", {cached: "children-family"})
    cases = [("ostatné (nešpecifikované/ostatné)", "ZDRUFF"), (None, "Hriešnice"), (None, "Koncert skupiny FIDEM")]
    zdruff, unknown, ruled = categorised(cases)
    assert (zdruff["category"]["value"], zdruff["categorySource"]["value"]) == (["children-family"], "model")
    assert zdruff["audience"]["value"] == ["family"]
    assert VALID.is_valid(key_values(zdruff)), list(VALID.iter_errors(key_values(zdruff)))
    # Not cached: no category until the next offline run; a rule's answer beats the cache.
    assert {"category", "categorySource"}.isdisjoint(unknown)
    assert (ruled["category"]["value"], ruled["categorySource"]["value"]) == (["music"], "rule")


def test_every_category_has_four_labels_a_valid_rule_and_is_the_models_enum():
    cats = generate.CATEGORIES
    assert len(cats) == 16 and len({c["code"] for c in cats}) == 16
    # `other` has no rule: only the decision model gives it (T-5490).
    assert [c["code"] for c in cats if c["rule"] is None] == ["other"]
    for c in cats:
        assert set(c["label"]) == {"sk", "cs", "en", "de"} and all(c["label"].values()), c["code"]
        if c["rule"] is None:
            continue
        re.compile(c["rule"])
        # RE2 (Bento) has no look-around or back-references.
        assert not re.search(r"\(\?[=!<]|\\[1-9]", c["rule"]), c["code"]
    for model in (SEED / "bbsk-podujatia.linkml.yaml", NATIONAL / "slovensko-podujatia.linkml.yaml"):
        enum = yaml.safe_load(model.read_text())["enums"]["EventCategory"]["permissible_values"]
        assert {k: v["annotations"]["label"]["value"] for k, v in enum.items()} == {c["code"]: c["label"] for c in cats}
    for schema in (SEED / "bbsk-podujatia.v1.schema.json", NATIONAL / "slovensko-podujatia.v1.schema.json"):
        definitions = json.loads(schema.read_text())["definitions"]
        assert definitions["Event"]["properties"]["category"]["items"] == {"$ref": "#/definitions/EventCategory"}
        assert definitions["EventCategory"]["enum"] == [c["code"] for c in cats]


# --- where an event's point comes from, and how precise it is (T-5240) ---------------------------------------


def records(monkeypatch, items: list[dict], town: str = "Cífer", org_type: str = "municipality") -> dict:
    """The common tail alone over plain records, for a made-up source of `town` (no homes.csv row)."""
    monkeypatch.setitem(generate.TEMPLATES, "records", ())
    day = datetime.fromtimestamp(NOW, UTC) + timedelta(days=3)
    payload = [{"name": f"Akcia {i}", "start": day.strftime("%Y-%m-%dT18:00:00+02:00"), "url": f"https://obec.example.sk/{i}",
                "sourceId": str(i), **item} for i, item in enumerate(items)]
    steps = offline(template("records", "obec.example.sk", region="TT", town=town, org_type=org_type), {})
    return {e["sourceId"]["value"]: e for e in run(steps, json.dumps(payload, ensure_ascii=False), "slovensko-podujatia")}


#: Who placed Cífer's culture house in the venue table.
CIFER_KD = next(v["checked_by"] for v in csv.DictReader((EVENTS / "venues.csv").open(encoding="utf-8"))
                if (v["town"], v["match"]) == ("cifer", "kultúrny dom"))


def town_point(town: str) -> list[float]:
    (row,) = [t for t in csv.DictReader((EVENTS / "towns.csv").open(encoding="utf-8")) if t["town"] == town]
    return [round(float(row["lon"]), 5), round(float(row["lat"]), 5)]


@requires_docker
def test_each_point_says_how_precise_it_is_and_an_unplaced_venue_takes_its_towns_office(monkeypatch):
    """DM-141: the fill order source > venue table > office > centre, the venue link and the municipality."""
    out = records(monkeypatch, [
        {"venue": "Futbalové ihrisko"},                        # nobody placed it: the town's office
        {"venue": "Futbalové ihrisko", "lat": 48.31, "lon": 17.49},  # the publisher's own point
        {"venue": "KD"},                                       # the calendar's short name for the culture house
        {"venue": "Ihrisko", "locality": "Brno"},              # a town the table does not know: no point at all
    ])
    assert out["0"]["location"]["value"]["coordinates"] == town_point("cifer")
    assert out["0"]["locationPrecision"]["value"] == "office" and out["0"]["venue"]["value"] == "Futbalové ihrisko"
    assert (out["1"]["location"]["value"]["coordinates"], out["1"]["locationPrecision"]["value"]) == ([17.49, 48.31], "source")
    assert out["2"]["locationPrecision"]["value"] == "building" and out["2"]["venue"]["value"] == "Kultúrny dom"
    assert out["2"]["location"]["value"]["coordinates"] == [17.5184876, 48.3025638]
    assert "location" not in out["3"] and "locationPrecision" not in out["3"]
    # Who placed each point (T-5490): the town table, the feed, the venue table's checker; nobody, no source.
    assert [out[k].get("locationSource", {}).get("value") for k in "0123"] == ["towns", "feed", CIFER_KD, None]
    # Only the venue table's place links its PointOfInterest; every event names its municipality (DM-141).
    kd = [v for v in generate.venues_for("cifer") if v["venue"] == "Kultúrny dom"][0]
    assert out["2"]["refPointOfInterest"] == {"type": "Relationship",
                                              "object": f"urn:ngsi-ld:PointOfInterest:slovensko-podujatia:{kd['poi']}"}
    assert [k for k, e in out.items() if "refPointOfInterest" in e] == ["2"]
    assert [out[k]["areaServed"]["value"] for k in "0123"] == ["Cífer", "Cífer", "Cífer", "Brno"]
    for e in out.values():
        assert VALID.is_valid(key_values(e)), e


@requires_docker
def test_a_district_or_region_as_the_town_and_a_glued_postcode_are_read_as_meant(monkeypatch):
    out = records(monkeypatch, [
        {"venue": "Hostinec pod Bradlom", "locality": "okres Myjava"},
        {"venue": "Sála", "locality": "Trnavský kraj", "streetAddress": "Bečov 96/6976 63"},
        {"streetAddress": "Bíňovce 134, 919 07, Slovensko", "locality": "Cífer"},
        {"streetAddress": "976 63 Predajná", "locality": "Cífer", "postalCode": "976 99"},
    ])
    # The geocode pilot of 2026-10-22: "okres Myjava" as the town, "Bečov 96/6976 63" for 96/6 and 976 63.
    assert out["0"]["address"]["value"]["addressLocality"] == "Cífer"
    assert out["0"]["locationPrecision"]["value"] == "office"
    assert out["1"]["address"]["value"] == {"streetAddress": "Bečov 96/6", "postalCode": "976 63", "addressLocality": "Cífer",
                                           "addressRegion": "Trnavský kraj", "addressCountry": "SK"}
    assert (out["2"]["address"]["value"]["streetAddress"], out["2"]["address"]["value"]["postalCode"]) == ("Bíňovce 134", "919 07")
    # A postcode alone is no street; the feed's own postcode field wins.
    assert "streetAddress" not in out["3"]["address"]["value"] and out["3"]["address"]["value"]["postalCode"] == "976 99"


@requires_docker
def test_a_tourism_body_places_an_event_in_the_town_it_names_within_its_region(monkeypatch):
    out = records(monkeypatch, [{"venue": "Námestie", "locality": "Bíňovce"}, {"venue": "Námestie"}],
                  org_type="tourism organisation (OOCR Test)")
    assert out["0"]["location"]["value"]["coordinates"] == town_point("binovce") and out["0"]["scope"] == "/SK/TT/binovce"
    assert out["1"]["location"]["value"]["coordinates"] == town_point("cifer")
    # A municipality's calendar keeps to its own town: another village it names gets no point from the table.
    out = records(monkeypatch, [{"venue": "Námestie", "locality": "Bíňovce"}])
    assert "location" not in out["0"]


POI = jsonschema.Draft7Validator({**copy.deepcopy(SCHEMA), "$ref": "#/definitions/PointOfInterest"})


@requires_docker
def test_the_venue_pipeline_writes_each_venue_once_as_a_valid_point_of_interest():
    """DM-141: one PointOfInterest per venue, valid against the store's model."""
    for ns, space in (("bbsk", "bbsk-podujatia"), ("slovensko", "slovensko-podujatia")):
        steps = yaml.safe_load((ROOT / "components/context-gateway/seed" / ns / f"{ns}-pipeline-miesta-podujati-bento.yaml").read_text())
        out = run(steps["pipeline"]["processors"], "tick", space)
        ids = [e["id"] for e in out]
        assert len(ids) == len(set(ids)) == len(generate.store_venues(generate.store_rows(next(s for s in generate.STORES if s["ns"] == ns))))
        for e in out:
            assert e["id"].startswith(f"urn:ngsi-ld:PointOfInterest:{space}:") and SCOPE.fullmatch(e["scope"]), e
            assert POI.is_valid(key_values(e)), (e, [x.message for x in POI.iter_errors(key_values(e))])
        (detva,) = [e for e in out if e["id"].endswith(":osm-way-292648504")]
        assert key_values(detva) == {
            "id": f"urn:ngsi-ld:PointOfInterest:{space}:osm-way-292648504", "type": "PointOfInterest",
            "name": "Dom kultúry A. Sládkoviča", "location": {"type": "Point", "coordinates": [19.4074269, 48.5419222]},
            "address": {"streetAddress": "Obrancov mieru 871/1", "addressLocality": "Detva", "addressCountry": "SK"},
            "areaServed": "Detva", "source": "https://www.openstreetmap.org/way/292648504",
            "dataProvider": "© OpenStreetMap contributors, ODbL 1.0"}


def test_every_wp_rest_fields_source_names_where_its_start_date_is():
    kinds = {urlsplit(r["check_url"]).hostname for r in generate.kept_sources() if r["feed_kind"] == "wp-rest-fields"}
    assert kinds == set(generate.WP_FIELDS), "a wp-rest-fields row without its WP_FIELDS entry writes nothing"
    assert all(f.get("start") for f in generate.WP_FIELDS.values())


@requires_docker
def test_wordpress_events_with_their_dates_in_fields_of_their_own_are_read_per_site():
    # Fixtures recorded 2026-10-23 from each site's REST list (T-5365), contacts scrubbed.
    out = {}
    for name in ("zilina", "regiontatry", "podujatiaksk", "upjs", "senec", "ruzomberok"):
        entities = pipeline_ns(name, f"{name}-wp-rest.json")
        assert entities, name
        for e in entities:
            assert VALID.is_valid(key_values(e)), (name, list(VALID.iter_errors(key_values(e))))
        out[name] = {e["name"]["value"]: e for e in entities}
    # Žilina: meta.iq_dateStart; a post titled only by its day ("19.&nbsp;10.&nbsp;2026 (PONDELOK)") is no event.
    assert out["zilina"]["MindLab"]["startDate"]["value"] == "2026-11-02"
    assert not any(n[0].isdigit() for n in out["zilina"])
    # regiontatry.sk: YYYYMMDD plus a time; 00:00:00 is the empty time field, so the event keeps its day.
    tatry = out["regiontatry"]
    assert tatry["To ťa musí baviť – Pavol Jackovič"]["startDate"]["value"] == "2026-10-29T17:30:00+01:00"
    assert tatry["Halloweenska večera pod hviezdami"]["startDate"]["value"] == "2026-10-31"
    # KSK calendar: one ACF group with a local datetime.
    assert out["podujatiaksk"]["Čo nás učia knihy: Mengeleho dievča"]["startDate"]["value"] == "2026-10-26T13:00:00+01:00"

    # UPJŠ: YYYYMMDD start and end.
    assert out["upjs"]["Univerzitné dni športu 2026"]["startDate"]["value"] == "2026-11-09"
    assert out["upjs"]["Univerzitné dni športu 2026"]["endDate"]["value"] == "2026-11-11"
    # Senec and Ružomberok run the smart-city theme of Banská Bystrica: Meta Box date_start and time_start.
    assert out["senec"]["Koncert úcty k starším v Senci"]["startDate"]["value"] == "2026-10-22T18:00:00+02:00"
    assert out["ruzomberok"]


@requires_docker
def test_visitbratislava_events_take_their_days_from_the_rest_field_event_date():
    # Recorded 2026-10-25 (T-5435): the event pages lost their JSON-LD Event; event_date holds each
    # day's 00:00 UTC in Unix seconds, which is the day the page prints ("From 23. Oct 2026 12:00").
    entities = pipeline_ns("visitbratislava", "visitbratislava-wp-rest.json")
    for e in entities:
        assert VALID.is_valid(key_values(e)), list(VALID.iter_errors(key_values(e)))
    by_name = {e["name"]["value"]: e for e in entities}
    assert len(by_name) == 4
    jazz = by_name["Bratislavské jazzové dni 2026"]
    assert (jazz["startDate"]["value"], jazz["endDate"]["value"]) == ("2026-10-23", "2026-10-25")
    assert by_name["Mauglí"]["startDate"]["value"] == "2026-10-25"


@requires_docker
def test_events_list_pages_that_print_each_date_as_text_are_read_card_by_card():
    # Recorded 2026-10-23 from each site's events list page (T-5393): no REST date field, no JSON-LD Event.
    want = {"prievidza": 8, "uluv": 7, "vsmu": 4, "agrokomplex": 12, "novacvernovka": 20, "bkis": 6, "kosnr": 9,
            "kultura-mestocadca": 12}
    out = {}
    for name, n in want.items():
        entities = pipeline_ns(name, f"{name}-cards.html")
        assert len(entities) == n, name
        for e in entities:
            assert VALID.is_valid(key_values(e)), (name, list(VALID.iter_errors(key_values(e))))
            assert e["url"]["value"].startswith("https://"), (name, e["url"])
        out[name] = {e["name"]["value"]: e for e in entities}
    # "28. 10. 2026" with a time of its own; "17. novembra 2026"; YYYYMMDD with data-end; day, month word and year apart.
    assert out["vsmu"]["Jubilejný Galla koncert"]["startDate"]["value"] == "2026-10-28T18:00:00+01:00"
    assert out["novacvernovka"]["Stupavar Brewing Punk vol. 2"]["endDate"]["value"] == "2026-11-28"
    assert out["kosnr"]["Klub grafológie (pokročilí)"]["startDate"]["value"] == "2026-11-04T17:00:00+01:00"
    dogs = out["agrokomplex"]["Medzinárodná výstava psov"]
    assert (dogs["startDate"]["value"], dogs["endDate"]["value"]) == ("2026-12-17", "2026-12-20")
    # A fair's own site is its link; a card's venue is the event's venue.
    assert dogs["url"]["value"] == "https://www.nitradog.sk/"
    assert out["bkis"]["DIVADLO TANDEM – Ukáž, či ma poznáš"]["venue"]["value"] == "Dom kultúry Lúky"
    # A branch in another town is that town's event: no point of the source's own town (Bratislava).
    kosice = out["uluv"]["Čo drôt spojil, čas nerozdelí…"]
    assert kosice["address"]["value"]["addressLocality"] == "Košice" and "location" not in kosice
    # Every other card without a venue point takes its town's office (towns.csv), labelled so.
    assert out["prievidza"]["Tvorivá dielňa Na Šech svatých"]["locationPrecision"]["value"] == "office"


# --- the HTML sources of T-5354 ----------------------------------------------------------------------------------


@requires_docker
def test_sliac_events_take_their_times_from_the_pages_microdata_and_never_the_towns_default_point():
    # Recorded 2026-10-23 from sliac.sk: every page names the town alone as its place, beside one static-map point.
    pages = json.loads((FIXTURES / "pages/sliac.json").read_text())
    out = {e["name"]["value"]: e for e in pipeline("sliac", "sliac-rss.xml", "sliac.json")}
    assert len(out) == 6
    india = out["Cestovateľský večer – India"]
    assert india["startDate"]["value"] == "2026-10-23T18:00:00+02:00" and india["endDate"]["value"] == "2026-10-23T19:00:00+02:00"
    assert out["PONOŽKOVÝ OKTÓBER"]["startDate"]["value"] == "2026-10-01" and out["PONOŽKOVÝ OKTÓBER"]["endDate"]["value"] == "2026-10-30"
    # The town's own name as the place: the town office, labelled so, never the theme's point as the event's.
    office = home_point("https://www.sliac.sk/podujatia/")
    assert all(e["location"]["value"]["coordinates"] == office and e["locationPrecision"]["value"] == "office" for e in out.values())
    assert all(VALID.is_valid(key_values(e)) for e in out.values())
    # A page naming a place of its own gives its point (the lantern parade page with its place changed).
    path = "/podujatia/lampionovy-sprievo/"
    pages[path] = pages[path].replace('<span itemprop="name">Sliač</span>', '<span itemprop="name">Parkovisko pri stanici</span>')
    (parade,) = [e for e in run(offline(processors("sliac"), pages), (FIXTURES / "sliac-rss.xml").read_text())
                 if e["name"]["value"] == "LAMPIÓNOVÝ SPRIEVOD"]
    assert parade["location"]["value"]["coordinates"] == [19.133676, 48.616528]
    assert parade["locationPrecision"]["value"] == "source" and parade["venue"]["value"] == "Parkovisko pri stanici"


@requires_docker
def test_ticketware_gives_each_show_with_its_year_and_hall_and_an_event_without_shows_nothing():
    # Recorded 2026-10-23 from kultura.banskastiavnica.sk; the talk of 837515 lists no show and writes nothing.
    out = pipeline("kultura-banskastiavnica", "ticketware-banskastiavnica.html", "ticketware-banskastiavnica.json")
    by_name = {e["name"]["value"]: e for e in out}
    assert len(out) == 3 and "Večer s Katarínou Nádaskou" not in by_name
    trio = by_name["Music Argentum: Alan Bartus New York Trio - Deep Roots"]
    assert trio["startDate"]["value"] == "2026-10-24T19:00:00+02:00"
    assert trio["venue"]["value"] == "Kultúrne centrum" and trio["locationPrecision"]["value"] == "building"
    assert trio["address"]["value"] == {"streetAddress": "Kammerhofská 1", "postalCode": "969 01", "addressLocality": "Banská Štiavnica",
                                        "addressRegion": "Banskobystrický kraj", "addressCountry": "SK"}
    assert by_name["David Kollar: Between Strings & Signals"]["venue"]["value"] == "Kino Akademik"
    assert trio["url"]["value"] == "https://www.kultura.banskastiavnica.sk/event/839978/music-argentum-alan-bartus-new-york-trio-deep-roots"
    assert all(VALID.is_valid(key_values(e)) for e in out)
    tornala = pipeline("kultura-tornala", "ticketware-tornala.html", "ticketware-tornala.json")
    assert {e["venue"]["value"] for e in tornala} == {"Mestské kultúrne stredisko"} and len(tornala) == 2


@requires_docker
def test_a_ticketware_date_without_its_year_is_the_next_one_and_an_off_host_link_is_never_read():
    pages = {
        "/event/1/silvester": '<h2 class="left">Silvester</h2><span class="date">Štvrtok,&nbsp;31. 12.&nbsp;o&nbsp;20:00</span>'
        '<td class="miesto"> <span class="miesto-btn off">Kino<span></span></span> <div class="miesto-content"> <span> Námestie sv. Trojice 1<br> 969 01&#32;Banská Štiavnica<br>',
        "/event/2/novy-rok": '<h2 class="left">Nový rok</h2><span class="date">Piatok,&nbsp;8. 1.&nbsp;o&nbsp;18:00</span>'
        '<td class="miesto"> <span class="miesto-btn off">Kino<span></span></span> <div class="miesto-content"> <span> Námestie sv. Trojice 1<br> 969 01&#32;Banská Štiavnica<br>',
        "/event/3/cudzi": '<h2 class="left">Cudzí</h2><span class="date">Piatok,&nbsp;6. 11.&nbsp;o&nbsp;18:00</span>'
        '<td class="miesto"> <span class="miesto-btn off">Kino<span></span></span> <div class="miesto-content"> <span> Námestie sv. Trojice 1<br> 969 01&#32;Banská Štiavnica<br>',
    }
    home = ('<a href="/event/1/silvester">a</a><a href="https://www.kultura.banskastiavnica.sk/event/2/novy-rok">b</a>'
            '<a href="/event/1/silvester-2">the same event again</a><a href="https://evil.example/event/3/cudzi">c</a>')
    out = {e["name"]["value"]: e["startDate"]["value"] for e in run(offline(processors("kultura-banskastiavnica"), pages), home)}
    assert out == {"Silvester": "2026-12-31T20:00:00+01:00", "Nový rok": "2027-01-08T18:00:00+01:00"}


@requires_docker
def test_brezno_events_come_from_the_calendars_map_list_one_per_date_and_placed_by_the_venue_table():
    # Recorded 2026-10-23 from brezno.sk: 26 event pages, 36 dates.
    out = pipeline("brezno", "brezno-eventphp.html", "brezno.json")
    assert len(out) == 36 and all(VALID.is_valid(key_values(e)) for e in out)
    dragon = sorted((e for e in out if e["name"]["value"] == "Drakovláčikom za kúzlami"), key=lambda e: e["startDate"]["value"])
    assert [e["startDate"]["value"] for e in dragon] == ["2026-10-29T16:00:00+01:00", "2026-10-30T16:00:00+01:00", "2026-10-31T16:00:00+01:00"]
    assert len({e["id"] for e in dragon}) == 3 and {e["venue"]["value"] for e in dragon} == {"Mestská veža"}
    (verity,) = [e for e in out if e["name"]["value"] == "Verity"]
    assert verity["startDate"]["value"] == "2026-10-23T19:00:00+02:00" and verity["venue"]["value"] == "Kino Mostár"
    assert verity["location"]["value"]["coordinates"] == [19.63921, 48.802746]
    assert verity["address"]["value"]["postalCode"] == "977 01" and verity["address"]["value"]["addressLocality"] == "Brezno"
    assert verity["url"]["value"] == "https://www.brezno.sk/verity/"
    placed = sum("location" in e for e in out)
    assert placed >= 30, placed
    # The map array's points are the CMS's default and never read; a list link outside the array is never read.
    assert all(e["url"]["value"].startswith("https://www.brezno.sk/") and "?f=" not in e["url"]["value"] for e in out)


@requires_docker
def test_krupina_events_read_the_summary_list_with_a_date_range_and_the_town_last_in_the_place():
    # Recorded 2026-10-23 from krupina.sk.
    out = {e["name"]["value"] + " " + e["startDate"]["value"]: e for e in pipeline("krupina", "krupina-podujatia.html", "krupina.json")}
    assert len(out) == 12 and all(VALID.is_valid(key_values(e)) for e in out.values())
    market = out["Vianočné trhy v Kúpeľoch Dudince 2026-12-03"]
    assert market["endDate"]["value"] == "2026-12-05" and market["address"]["value"]["addressLocality"] == "Dudince"
    # A Dudince event is never placed at Krupina's office; one at Krupina's own name is.
    assert "location" not in market
    talk = out["Katarína Nádaská: Dejiny čarodejníctva 2026-11-04"]
    assert talk["location"]["value"]["coordinates"] == home_point("https://krupina.sk/podujatia")
    harp = out["Harfový koncert 2026-11-01"]
    assert harp["venue"]["value"] == "Lobby bar v Liečebnom dome Rubín" and harp["organizer"]["value"] == {"name": "Kúpele Dudince"}
    blood = out["Mobilná kvapka krvi 2026-11-02"]
    assert blood["location"]["value"]["coordinates"] == [19.065536, 48.352837]


# --- Prague (T-5666) -------------------------------------------------------------------------------------------------

PRAHA = ROOT / "components/context-gateway/seed/praha"


def prague(kind: str, fixture: str, host: str, path: str = "/", org_type: str = "cinema (Kino Aero)") -> list:
    """A Prague source's steps for a made-up row over a recorded feed, written into praha-akce."""
    steps = template(kind, host, store="praha", region="PHA", town="Hlavní město Praha", path=path, org_type=org_type)
    return run(offline(steps, {}), (FIXTURES / fixture).read_text(), "praha-akce")


def test_every_prague_source_writes_praha_akce_under_cz_pha_and_runs_every_6_hours():
    rows = [r for r in generate.kept_sources() if r["region"] == "PHA"]
    assert len(rows) >= 10
    for row in rows:
        bento = (PRAHA / f"praha-pipeline-{row['name']}-bento.yaml").read_text()
        assert '"scope": "/CZ/PHA"' in bento and '"town_scope": "/CZ/PHA/hlavni_mesto_praha"' in bento, row["name"]
        spec = yaml.safe_load((PRAHA / f"praha-pipeline-{row['name']}.yaml").read_text())["spec"]
        assert spec["targetEndpoint"] == "urn:ngsi-ld:Endpoint:praha.eu:praha-akce:praha-akce-zapis"
        assert spec["period"] == "6h" and spec["schedule"].split()[1].endswith("-23/6")
    index = (PRAHA / "index.yaml").read_text()
    assert all(f"praha-pipeline-{r['name']}.yaml:" in index for r in rows)


@requires_docker
def test_a_prague_cinema_page_gives_events_in_czechia_at_the_city_office_point():
    events = prague("jsonld", "kinoaero-jsonld.html", "www.kinoaero.cz")
    assert [e["name"]["value"] for e in events] == ["Macbeth | Giuseppe Verdi", "Fjord"]
    for e in events:
        assert e["scope"] == "/CZ/PHA/hlavni_mesto_praha"
        assert e["address"]["value"]["addressCountry"] == "CZ"
        assert e["address"]["value"]["streetAddress"] == "Biskupcova 31"
        # "Prague, Czechia" is Praha; the venue nobody placed takes the city office, labelled approximate.
        assert e["location"]["value"]["coordinates"] == [14.41783, 50.08711]
        assert e["locationPrecision"]["value"] == "office" and VALID.is_valid(key_values(e))


@requires_docker
def test_a_prague_borough_ical_and_a_conservatory_events_calendar_are_read():
    ical = prague("ical", "pocernice.ics", "www.pocernice.cz", "/?ical=1", "city district (Praha 20)")
    assert {e["name"]["value"] for e in ical} == {"RADKA FOLPRECHTOVÁ: TANEC KVĚTIN", "Dýňobraní"}
    assert {e["scope"] for e in ical} == {"/CZ/PHA/hlavni_mesto_praha"}
    tribe = prague("tribe-rest", "prazskakonzervator-tribe.json", "prazskakonzervator.cz",
                   "/wp-json/tribe/events/v1/events?per_page=50&start_date=now", "theatre (Divadlo Na Rejdišti)")
    first = next(e for e in tribe if e["name"]["value"] == "Koncert pěveckého oddělení")
    assert first["startDate"]["value"] == "2026-10-26T18:30:00+01:00"
    # "Praha 1" is a borough of Praha, so the event takes the city's point and never a Slovak one.
    assert first["address"]["value"]["addressCountry"] == "CZ" and first["location"]["value"]["coordinates"][1] > 50


@requires_docker
def test_a_point_outside_the_country_of_the_store_is_dropped():
    page = (FIXTURES / "kinoaero-jsonld.html").read_text().replace(
        '"streetAddress": "Biskupcova 31"', '"streetAddress": "Biskupcova 31"}, "geo": {"latitude": 48.15, "longitude": 17.11')
    steps = template("jsonld", "www.kinoaero.cz", store="praha", region="PHA", town="Hlavní město Praha", org_type="cinema (Kino Aero)")
    events = run(offline(steps, {}), page, "praha-akce")
    # Bratislava's point is outside Czechia: the event is kept without a point, never with the wrong one.
    assert len(events) == 2 and all("location" not in e for e in events)


@requires_docker
def test_the_shared_tail_strips_slovak_and_czech_phone_numbers_and_keeps_dates_and_prices():
    """F-R2BNT3-002: praha-akce runs the same tail, and the Slovak-only pattern published Czech
    numbers and every international number written with a space after the country code."""
    block = re.search(r"^( *)map uncontactable \{\n.*?^\1\}\n", RESOURCES.read_text(encoding="utf-8"), re.M | re.S)
    assert block, "the tail defines the one contact scrub its organizer and description share"
    texts = ["Volejte +420 222 333 444 nebo 777123456.", "Tel. 0905 123 456, +421 905 123 456, +421905123456.",
             "Pište na info@akce.cz.", "Vstupné 150 Kč, 19. 10. 2026 od 18.00, sál 2, kapacita 1200."]
    mapping = textwrap.dedent(block.group(0)) + "root = {\"t\": this.apply(\"uncontactable\")}"
    result = subprocess.run(["docker", "run", "--rm", "-i", BENTO, "blobl", mapping],
                            input="\n".join(json.dumps(t) for t in texts), capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, result.stderr
    out = [json.loads(line)["t"] for line in result.stdout.splitlines() if line.strip()]
    assert len(out) == 4, result.stdout + result.stderr
    assert not any(ch.isdigit() for ch in out[0] + out[1]) and "@" not in out[2], out
    assert out[3] == texts[3]
