#!/usr/bin/env python3
"""The seed of every mapped Banská Bystrica and BBSK dataset, written from the T-4254 mapping (T-4255).

`bb-bbsk-ngsi-ld-mapping.yaml` beside this script is a copy of joinedcontext-docs
`Research/bb-bbsk-ngsi-ld-mapping.yaml`, which the docs script builds from the CKAN snapshot. For
each project it names, this script writes one public space `{project}-otvorene-data` with its
model, Endpoint and two Policies, and for each mapped dataset one DataSource, one Pipeline and the
Pipeline's Bento mapping, into `components/context-gateway/seed/{project}/` under the prefix
`{project}-od-`, plus the block of `index.yaml` that places them. A dataset a seed already reads
whole (`existing.covers: whole`) is left to that seed; the catalogue lists it through that seed's
public Endpoint (T-4291).

    generate.py               # write the seed; then regenerate the model artifacts (below)
    generate.py --check       # CI: exit 1 when a committed file differs from what this writes
    generate.py --artifacts PYTHON MODEL_TOOLS_SRC
                              # also run gen_json_schema.py, gen_context.py, gen_docs.py and
                              # gen_example.py with that interpreter,
                              # and render each extension's formalisms (below)

A type no Smart Data Model holds as is has an extension (T-4336), declared under `extensions` in
the mapping: `sdm/extensions/<Type>/` holds its `schema.json` (the base model's schema.json at its
pin, under `allOf` with its `type` set to the extension's, plus the attributes the type adds) and
its LinkML, both written here; `--artifacts` renders the LinkML into the @context, SHACL, OWL, RDF,
Markdown and the examples with model-tools.

The JSON Schema and the @context are model-tools' output (joinedcontext-platform
tools/model-tools, linkml); `--check` leaves them out, and tests/test_open_data_seed.py validates
the generated mappings' entities against the committed schema.
"""

from __future__ import annotations

import base64
import hashlib
import json
import math
import os
import re
import secrets
import subprocess
import sys
import urllib.parse
from collections import Counter
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent.parent
MAPPING = Path(__file__).resolve().parent / "bb-bbsk-ngsi-ld-mapping.yaml"
# The Smart Data Models every mapped type is one of, vendored at the commits pins.json names (T-4319).
SDM = Path(__file__).resolve().parent / "sdm"
# A slot's label in each language the widget speaks, where the source column is no heading of its own:
# translated once, reviewed as code, and marked `generated` in the model so the Portal shows it (T-5332).
LABELS = Path(__file__).resolve().parent / "labels.tsv"
LANGS = ("sk", "cs", "en", "de")
EXTENSIONS = SDM / "extensions"
# Where an extension and the source's own columns name their terms (T-4328's jc:).
JC = "https://joinedcontext.com/models/"
EXTENSION_URL = "https://github.com/marek-mraz/joinedcontext-deployment/blob/main/scripts/open-data/sdm/extensions"
# Extensions a city space's own model needs beside the mapping's (T-4338): the class of that
# space's LinkML names the attributes the extension adds.
SPACE_EXTENSIONS = {
    "BudgetLine": {
        "baseModel": "dataModel.CPSV-AP/Cost", "model": "praha/praha.linkml.yaml",
        "description": "One line of a city budget in the national budget classification, with the approved and the adjusted amount.",
        "why": "CPSV-AP Cost is the official model of an amount a public body sets (identifier, value, currency); no subject holds a budget line",
    },
    # Prague's environment layers (T-5429): a register entry or a mapped area, located and named.
    "EmissionSource": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "praha/praha.linkml.yaml",
        "description": "One stationary source of air pollution of a city's emission register: where it is, its fuel and its yearly emissions.",
        "why": "Environment's AirQualityObserved and EnvironmentObserved hold concentrations measured at a station; no model holds a register entry of yearly emissions",
    },
    "RadonRiskArea": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "praha/praha.linkml.yaml",
        "description": "One area of a radon risk map and the risk category of its ground.",
        "why": "Environment has no model of a mapped ground property; EnvironmentObserved is a measurement at a time",
    },
    "FloodZone": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "praha/praha.linkml.yaml",
        "description": "One designated flood area: the ground a flood of a given return period covers, or a zone of it.",
        "why": "Environment's FloodMonitoring holds a water level measured at a gauge, not the area a flood covers",
    },
    "NoiseZone": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "praha/praha.linkml.yaml",
        "description": "One area of a computed noise map: the ground where the day or the night sound level falls into one 5 dB band.",
        "why": "Environment's NoiseLevelObserved holds a level measured at a sensor at a time, not the area a noise band covers",
    },
    "FloodDefence": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "praha/praha.linkml.yaml",
        "description": "One line of flood defences: what it is built of, who keeps it and the flood it is designed for.",
        "why": "no Smart Data Model holds a flood defence structure",
    },
    # Helsinki's cultural heritage maps (T-5670): the WWI fortification strongpoints and the landscape culture areas.
    "HeritageArea": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "helsinki/helsinki.linkml.yaml",
        "description": "One area of a city's cultural heritage map: its outline and its kind, such as a fortification or a historic park.",
        "why": "PointOfInterest's category lists services and amenities, not the kinds of a heritage map, and no Smart Data Model holds a heritage area",
    },
    # Helsinki's signal-controlled junctions (T-5286): the traffic lights and the warning lights of the city.
    "TrafficSignalJunction": {
        "baseModel": "dataModel.PointOfInterest/PointOfInterest", "model": "helsinki/helsinki.linkml.yaml",
        "description": "One junction or crossing with traffic lights or warning lights: where it is, its installation number, its signal kind and whether it is in service.",
        "why": "the pinned Smart Data Models hold no traffic signal or signalised junction, and PointOfInterest's category lists services and amenities",
    },
}
SDM_HEADER = ("$schema", "$schemaVersion", "$id", "modelTags", "title", "description", "derivedFrom", "license")
JSON_TYPE = {"integer": "integer", "float": "number", "date": "string", "datetime": "string", "uri": "string", "string": "string"}
JSON_FORMAT = {"date": "date", "datetime": "date-time"}
SCHEMA_ORG = {"integer": "Integer", "float": "Number", "date": "Date", "datetime": "DateTime", "uri": "URL",
              "string": "Text", "string-list": "Text"}
SEED = ROOT / "components/context-gateway/seed"
BEGIN = "# --- generated by scripts/open-data/generate.py (T-4255): do not edit by hand ---"
END = "# --- end of generated ---"
URL_LIMIT = 2000
PAGE = 1000  # rows per Esri request; every layer of the Hub allows at least this many

PROJECTS = {
    "banskabystrica": {
        "domain": "banskabystrica.sk",
        # Registers and tables, and the city's news as Articles: "tables" alone told an agent the
        # press releases were rows of numbers (T-4569).
        "title": {"sk": "Banská Bystrica — otvorené dáta mesta", "en": "Banská Bystrica, the city's open data"},
        "about": {"sk": "Otvorené dáta mesta Banská Bystrica z katalógu: registre a tabuľky (adresy, zmluvy, objednávky, "
                        "dotácie, zasadnutia, zeleň, parkovanie) a správy mesta (Aktuality) ako články (Article).",
                  "en": "The open data of the city of Banská Bystrica from the catalogue: registers and tables (addresses, "
                        "contracts, orders, grants, council meetings, greenery, parking) and the city's news (Aktuality) "
                        "as Articles."},
        # The city with a margin: the space's place, which the catalogue's Dataset carries (T-4525).
        "area": [18.95, 48.6, 19.35, 48.9],
        "keywords": {"sk": ["Banská Bystrica", "zmluvy", "objednávky", "dotácie", "adresy", "verejná zeleň"],
                     "en": ["Banská Bystrica", "contracts", "orders", "grants", "addresses", "public greenery"]},
        "spatial": ["SK032", "https://data.gov.sk/id/lau2/SK0321508438"],
    },
    "bbsk": {
        "domain": "bbsk.sk",
        "title": {"sk": "Banskobystrický kraj — tabuľky katalógu", "en": "Banská Bystrica Region, catalogue tables"},
        "about": {"sk": "Všetky otvorené dáta Banskobystrického samosprávneho kraja z katalógu (opendata.bbsk.sk).",
                  "en": "Every dataset of the Banská Bystrica Self-Governing Region in the catalogue (opendata.bbsk.sk)."},
        # The region with a margin (its data reaches 18.54 E in the Žarnovica district).
        "area": [18.3, 47.9, 20.5, 49.2],
        "keywords": {"sk": ["Banskobystrický kraj", "školy", "sociálne služby", "cesty", "kultúra"],
                     "en": ["Banská Bystrica Region", "schools", "social services", "roads", "culture"]},
        "spatial": ["SK032"],
    },
}
# Every standard the open data is served in (T-4279). No `sta` (T-5299): a SensorThings Datastream
# needs its unitOfMeasurement (STA 1.1 §9.2.5) and these models declare no `unit`, so the Portal and
# jcctl validate refuse it (EP-125); `series` lists it again once they do. No `mqtt` until 8883
# opens (T-3996); the WebSocket pushes changes.
# One file, `csv`: xlsx, parquet, json, zip and geojson were removed (owner 2026-10-18, T-4597);
# a map reads GeoJSON from NGSI-LD (Accept: application/geo+json) or OGC API Features.
REPRESENTATIONS = ["ngsi-ld", "csv", "websocket", "ogc-features", "odata", "mcp"]


def news_feed(p, types):
    """The base preset `news` of a space with Article (T-6756, API/02 §4a): feeds/news.rss, .atom
    and .json. The WordPress mapping writes its own columns (dateGmt, modifiedGmt), so the roles name them."""
    if "Article" not in types:
        return {}
    title = {lang: f"{text}: aktuality" if lang == "sk" else f"{text}: news" for lang, text in p["title"].items()}
    return {"representations": {"feed": [{
        "name": "news", "title": title, "lang": "sk",
        "sources": [{"type": "Article", "preset": "article",
                     "mapping": {"title": "name", "updated": "modifiedGmt", "published": "dateGmt",
                                 "content": "content", "link": "url"}}]}]}}
# What the catalogue's harvests read to list each converted dataset's standard APIs (T-4279).
DISTRIBUTIONS = ROOT / "charts/ckan/files/od-datasets.json"
# The attribution every entity carries (Open data integration rules §3), declared in the model.
# The catalogue dataset is `isPartOf`, never `dataset`: the NGSI-LD core @context defines `dataset`
# (`ngsi-ld:hasDataset`) and, being protected, wins over the space's @context, so the rows held the
# core IRI while the model said jc:dataset and OGC and OData refused or missed a filter on it (T-4289).
COMMON = {
    "dataProvider": ("string", "Who published the row, with the credit its licence asks for."),
    "source": ("uri", "The service the row was read from."),
    "isPartOf": ("uri", "The catalogue dataset the row belongs to, where its licence and its other rows are."),
}
# A column whose attribute name is one of these keeps its value as `column<Name>`. `name` is the
# model's own (the mapping renames a column to it, T-4319).
TAKEN = {*COMMON, "location", "observedAt"}
# The `name` of a public dataset whose source has no name column, built from its own columns: each
# `{column}` is the row's value, and an empty one drops with the separators beside it (T-4745: the
# catalogue smoke refuses an entity without a name). The row hash never reads it, so ids stay.
NAMES = {
    "bb-dotacie": "{Žiadateľ}, {Akcia}",
    "bb-dtm-bb-register-statickej-dopravy": "{vrstva} {vrstva_id}",
    "bb-objednavky-bb": "Objednávka {Číslo_objednávky}: {Text_objednávky}",
    "bb-zasadnutia-organov-samospravy": "{Orgán_samosprávy}, {Dátum_zasadnutia}",
    "bbsk-clenovia-samospravy-efc2f3": "{Titul_pred_menom} {Meno} {Priezvisko} {Titul_za_menom}",
    "bbsk-nehodovost-v-bbk-0ea72d": "Nehoda {Číslo_nehody}",
}
# Attributes an earlier mapping wrote and the current one no longer does, per project and type ("*": every
# type). The od-* pipelines write in mode replace (PL-79), and a replace through the Endpoint's view counts
# every stored Attribute as written; one the view does not show refuses the whole write (ADR-0023 rule 7),
# so the Entities written before the rename failed every run (T-5236). The model keeps each such name as a
# deprecated slot: the view then lets the next replace remove it, and no form asks for it.
# ponytail: kept until every environment's od-* runs have replaced each stored Entity once; then emptied.
RETIRED = {
    "banskabystrica": {
        "*": ["dataset"],
        "Address": ["cASTOBCE", "castObce", "oBEC", "obec", "pSC", "uLICA", "ulica"],
        "Article": ["link", "sourceId", "title"],
        "Contract": ["centralneCisloZmluvy"],
        "PurchaseOrder": ["cisloObjednavky"],
    },
    # The personal columns impersonal() drops (T-5697).
    "bbsk": {
        "Contract": ["sidloDodavatela"],
        "Grant": ["statutarnyZastupca", "ulicaACislo"],
        "Invoice": ["adresa"],
        "Organization": ["adresa", "riaditel", "riaditelEmail", "riaditelPevnaLinka", "riaditelPracMobil", "statutari", "ulicaACislo"],
        "PointOfInterest": ["riaditel", "riaditelEmail", "riaditelMobilnyTelefon", "riaditelPevnaLinka"],
        "PurchaseOrder": ["adresa"],
    },
}
# The od-* replace pipelines whose stored Entities still hold what an earlier mapping wrote, with the day
# that mapping changed: the mapping carries the day as a variable nothing reads, so its stream text moves
# and a running Portal runs it at once (T-5824) instead of at its weekly or monthly slot (T-5844).
REVISED = {slug: "2026-10-27" for slug in (
    "zmluvy", "organizacna-struktura", "materske-skoly-v-banskobystrickom-kraji",
    "kontakty-na-miestne-samospravy-v-bbk", "stredne-skoly-v-banskobystrickom-kraji", "dodavatelia",
    "stredne-skoly-v-bbk-odbory-a-pocet-stude", "skoly-a-skolske-zariadenia-v-zriadovatel",
    "vysoke-skoly-v-banskobystrickom-kraji", "zakladne-skoly-v-banskobystrickom-kraji")}
# What a visitor reads for the common slots, in each language the widget speaks (T-4682).
COMMON_LABELS = {
    "dataProvider": {"sk": "Poskytovateľ údajov", "cs": "Poskytovatel dat", "en": "Data provider", "de": "Datenanbieter"},
    "source": {"sk": "Zdroj", "cs": "Zdroj", "en": "Source", "de": "Quelle"},
    "isPartOf": {"sk": "Dataset", "cs": "Datová sada", "en": "Dataset", "de": "Datensatz"},
}
# Smart Data Models terms that many classes map, each from a column of its own: the slot is one for the
# whole space, so its text is the model's meaning, never the heading of whichever column came first
# ("Miesto" labelled every Organization's name, F-BNT2-010).
GENERIC = {
    "name": ("The entity's name.", {"sk": "Názov", "cs": "Název", "en": "Name", "de": "Name"}),
    "legalName": ("The organisation's registered name.",
                  {"sk": "Úradný názov", "cs": "Úřední název", "en": "Legal name", "de": "Rechtlicher Name"}),
    "description": ("A description of the entity.", {"sk": "Popis", "cs": "Popis", "en": "Description", "de": "Beschreibung"}),
}
LICENCE = {"cc-by": "CC BY 4.0", "cc-by-sa": "CC BY-SA 4.0"}
# The types whose history matters, a measurement series read over time; every other type keeps its
# current state only (T-4275, DM-82). The space's owner changes either on its Retention tab.
MEASURED = {"AirQualityObserved", "WeatherObserved", "WaterQualityObserved", "KeyPerformanceIndicator",
            "OffStreetParking", "ParkingArea", "BikeHireDockingStation"}
# A measured type's history: every run re-reads the whole table and maps no observedAt, so a row is
# kept when a value changes and once a week, never once per run (T-4844, DM-108, DM-109).
RECORDED = {"history": "P30D", "record": "onChange", "heartbeat": "P7D"}
# The source kinds whose rows are GeoJSON features: the columns sit under `properties`.
FEATURES = {"esri-featureserver", "esri-layers", "zip-geojson"}
# The kinds read from an ArcGIS query, whose geometry the server sends in WGS 84.
ESRI = {"esri-featureserver", "esri-layers"}


def slugs(entries: list[dict]) -> dict[str, str]:
    """dataset name -> a DNS label of at most 40 characters, unique within the mapping. A trailing
    year goes: a name ending in four digits is what the demo janitor sweeps as journey residue."""
    out: dict[str, str] = {}
    for e in entries:
        base = re.sub(r"^(bbsk|bb)-", "", e["name"])
        base = re.sub(r"-[0-9a-f]{6}$", "", base)[:40].rstrip("-")
        base = re.sub(r"(-[0-9]+)+$", "", base)
        name, n = base, 2
        while name in out.values():
            name = f"{base[:37].rstrip('-')}-{n}"
            n += 1
        out[e["name"]] = name
    return out


def attribute(name: str) -> str:
    return "column" + name[0].upper() + name[1:] if name in TAKEN else name


def heading(col: str) -> str | None:
    """The Slovak heading a source column already is (`Dĺžka_premostenia__m_` → "Dĺžka premostenia (m)"),
    the slot's `sk` label at no model cost (T-4682); None for a code (`KRAJ_ID`, `datumzmeny`, `TypSaSZKod`)."""
    unit = re.fullmatch(r"(.+?)__([^_]+)_", col)
    text, unit = (unit.group(1), unit.group(2)) if unit else (col, None)
    text = re.sub(r"_+", " ", text).strip()
    human = not text.isascii() or re.fullmatch(r"[A-Z][a-z]+(?: [A-Za-z]+)*", text)
    if not human or re.search(r"[a-z][A-Z]", text):
        return None
    return text[0].upper() + text[1:] + (f" ({unit})" if unit else "")


def ranges(entries: list[dict]) -> dict[str, str]:
    """One range per attribute name across a model: slots are the model's, not a class's. Integer
    and float meet as float; any other two ranges meet as string."""
    out: dict[str, str] = {}
    for e in entries:
        for a, p in e["properties"].items():
            a, r = attribute(a), p["range"]
            seen = out.get(a, r)
            out[a] = r if seen == r else "float" if {seen, r} <= {"integer", "float"} else "string"
    return out


def q(text: str) -> str:
    """A bloblang string literal."""
    return json.dumps(text, ensure_ascii=False)


def named(path: str, kind: str, template: str) -> str:
    """A `NAMES` template as bloblang: the values joined, blank runs and edge separators trimmed."""
    parts = re.split(r"\{([^}]+)\}", template)
    read = [f"{column(path, kind, x)}.or(\"\").string().trim()" if i % 2 else q(x) for i, x in enumerate(parts) if x]
    return f"[{', '.join(read)}].join(\"\").re_replace_all(\"\\\\s+\", \" \").re_replace_all(\"^[ ,:]+|[ ,:]+$\", \"\").apply(\"string\")"


def column(path: str, kind: str, col: str) -> str:
    if kind == "json-feed" and col in ("title", "content"):
        # WordPress renders both as HTML: "&#8222;" in a title is a quote mark (T-4569).
        return f"{path}.{col}.rendered.strip_html().unescape_html().catch(null)"
    if kind == "json-feed" and col == "slug":
        # A post without a title is named by its slug, "krizovatka-pri-kazacku" as words (T-4569).
        return f"{path}.slug.or(\"\").string().replace_all(\"-\", \" \")"
    return f"{path}.{q(col)}"


MAPS = """\
# Slovak notation: a space (or a no-break one) between thousands, a comma before the decimals, so
# "10 700,01" is 10700.01 (T-4324); a text without a comma keeps its dot, "1.5" is one and a half.
map number {
  let s = this.string().re_replace_all("[ \t\u00a0\u202f]", "")
  let t = if $s.contains(",") { $s.replace_all(".", "").replace_all(",", ".") } else { $s }
  root = $t.number().catch(null)
}
map integer {
  let v = if this == null { null } else { this.apply("number") }
  root = if $v == null { null } else { { "type": "Property", "value": $v.round().int64() } }
}
map float {
  let v = if this == null { null } else { this.apply("number") }
  root = if $v == null { null } else { { "type": "Property", "value": $v } }
}
map string {
  let v = if this == null { "" } else { this.string().trim() }
  root = if $v == "" { null } else { { "type": "Property", "value": $v } }
}
map string_list {
  let v = if this == null { "" } else { this.string().trim() }
  root = if $v == "" { null } else { { "type": "Property", "value": [$v] } }
}
map uri {
  root = this.apply("string")
}
# An Esri date is milliseconds since the epoch; a WordPress one is text.
# `match` rebinds `this` to what it matches, so the value is named first.
map datetime {
  let x = this
  let v = match $x.type() {
    "number" => ($x / 1000).ts_format("2006-01-02T15:04:05Z", "UTC").catch(null),
    "string" => $x.ts_parse("2006-01-02T15:04:05").ts_format("2006-01-02T15:04:05Z", "UTC").catch($x.ts_format("2006-01-02T15:04:05Z", "UTC").catch(null)),
    _ => null,
  }
  root = if $v == null { null } else { { "type": "Property", "value": $v } }
}
# The instant a row was observed, from its `observedAt` column: a bare year is its first instant,
# anything else reads as a datetime (T-4288).
map observed {
  let x = this
  root = if $x == null { null } else if $x.string().re_match("^[0-9]{4}$") { $x.string() + "-01-01T00:00:00Z" } else {
    $x.apply("datetime").value.catch(null)
  }
}
# A measured Property with the instant it holds, so SensorThings charts it (T-4288).
map stamped {
  root = if this.p == null || this.at == null { this.p } else { this.p.merge({ "observedAt": this.at }) }
}
map date {
  let x = this
  let v = match $x.type() {
    "number" => ($x / 1000).ts_format("2006-01-02", "UTC").catch(null),
    # ISO, or the Slovak "29.08.2026" and "1. 3. 2024".
    "string" => if $x.re_match("^[0-9]{4}-[0-9]{2}-[0-9]{2}") { $x.slice(0, 10) } else {
      $x.re_replace_all("[.] +", ".").trim().ts_parse("2.1.2006").ts_format("2006-01-02", "UTC").catch(null)
    },
    _ => null,
  }
  root = if $v == null { null } else { { "type": "Property", "value": $v } }
}"""


def krovak_maps() -> str:
    """Bloblang maps `sjtsk` ([E, N] of EPSG:5514 to [lon, lat] of EPSG:4326, 7 decimals) and
    `sjtsk_coords` (any GeoJSON coordinates array). EPSG guidance note 7-2, Krovak East North
    (method 1041) inverse on Bessel 1841, then the Helmert position-vector shift of EPSG:5239
    (S-JTSK to WGS 84, Slovakia, about 1 m) and Bowring back to latitude. Bloblang has tan, sin,
    cos and pow but no atan or asin: atan is three Newton steps on tan from a 0.002 rad guess."""
    a, f = 6377397.155, 1 / 299.1528128
    e2 = 2 * f - f * f
    e = math.sqrt(e2)
    phi_c, lam0, alpha_c, phi_p, k_p = math.radians(49.5), math.radians(24 + 50 / 60), math.radians(30.28813972222222), math.radians(78.5), 0.9999
    big_a = a * math.sqrt(1 - e2) / (1 - e2 * math.sin(phi_c) ** 2)
    b = math.sqrt(1 + e2 * math.cos(phi_c) ** 4 / (1 - e2))
    g0 = math.asin(math.sin(phi_c) / b)
    t0 = math.tan(math.pi / 4 + g0 / 2) * ((1 + e * math.sin(phi_c)) / (1 - e * math.sin(phi_c))) ** (e * b / 2) / math.tan(math.pi / 4 + phi_c / 2) ** b
    n = math.sin(phi_p)
    r0 = k_p * big_a / math.tan(phi_p)
    sec = math.pi / 180 / 3600
    tx, ty, tz, rx, ry, rz = 485.021, 169.465, 483.839, 7.786342 * sec, 4.397554 * sec, 4.102655 * sec
    aw, fw = 6378137.0, 1 / 298.257223563
    e2w, bw = 2 * fw - fw * fw, aw * (1 - fw)
    ep2 = (aw * aw - bw * bw) / (bw * bw)
    pi, half, quarter = math.pi, math.pi / 2, math.pi / 4
    # Bloblang reads no exponent, so every constant is written out in fixed point.
    c = {k: f"{v:.20f}".rstrip("0").rstrip(".") for k, v in dict(
        pi=pi, half=half, quarter=quarter, e=e, e2=e2, a=a, b=b, ib=1 / b, he=e / 2, n=n, inn=1 / n, r0=r0,
        tp=math.tan(quarter + phi_p / 2), ca=math.cos(alpha_c), sa=math.sin(alpha_c), lam0=lam0,
        k=t0 ** (-1 / b), tx=tx, ty=ty, tz=tz, rx=rx, ry=ry, rz=rz, aw=aw, bw=bw, e2w=e2w, ep2=ep2,
        deg=180 / pi).items()}
    return f"""\
map atan1 {{
  let x = this
  let y0 = $x * {c['quarter']} - $x * ($x.abs() - 1) * (0.2447 + 0.0663 * $x.abs())
  let t0 = $y0.tan()
  let y1 = $y0 - ($t0 - $x) / (1 + $t0 * $t0)
  let t1 = $y1.tan()
  let y2 = $y1 - ($t1 - $x) / (1 + $t1 * $t1)
  let t2 = $y2.tan()
  root = $y2 - ($t2 - $x) / (1 + $t2 * $t2)
}}
map atan {{
  let x = this
  root = if $x > 1 {{ {c['half']} - (1 / $x).apply("atan1") }} else if $x < -1 {{ -{c['half']} - (1 / $x).apply("atan1") }} else {{ $x.apply("atan1") }}
}}
map asin {{
  let x = this
  root = ($x / (1 - $x * $x).pow(0.5)).apply("atan")
}}
map atan2 {{
  let y = this.index(0)
  let x = this.index(1)
  let t = if $x == 0 {{ 0 }} else {{ ($y / $x).apply("atan") }}
  root = if $x > 0 {{ $t }} else if $x < 0 && $y >= 0 {{ $t + {c['pi']} }} else if $x < 0 {{ $t - {c['pi']} }} else if $y > 0 {{ {c['half']} }} else {{ -{c['half']} }}
}}
map sjtsk_lat {{
  let es = {c['e']} * this.phi.sin()
  root = {{"tu": this.tu, "phi": 2 * (({c['k']} * this.tu.pow({c['ib']}) * ((1 + $es) / (1 - $es)).pow({c['he']})).apply("atan") - {c['quarter']})}}
}}
map sjtsk {{
  let xp = 0 - this.index(1)
  let yp = 0 - this.index(0)
  let r = ($xp * $xp + $yp * $yp).pow(0.5)
  let d = ($yp / $xp).apply("atan") * {c['inn']}
  let t = 2 * ((({c['r0']} / $r).pow({c['inn']}) * {c['tp']}).apply("atan") - {c['quarter']})
  let u = ({c['ca']} * $t.sin() - {c['sa']} * $t.cos() * $d.cos()).apply("asin")
  let v = ($t.cos() * $d.sin() / $u.cos()).apply("asin")
  let lam = {c['lam0']} - $v * {c['ib']}
  let tu = ($u / 2 + {c['quarter']}).tan()
  let phi = {{"phi": $u, "tu": $tu}}.apply("sjtsk_lat").apply("sjtsk_lat").apply("sjtsk_lat").apply("sjtsk_lat").phi
  let sp = $phi.sin()
  let nn = {c['a']} / (1 - {c['e2']} * $sp * $sp).pow(0.5)
  let x = $nn * $phi.cos() * $lam.cos()
  let y = $nn * $phi.cos() * $lam.sin()
  let z = $nn * (1 - {c['e2']}) * $sp
  let x2 = {c['tx']} + $x - {c['rz']} * $y + {c['ry']} * $z
  let y2 = {c['ty']} + {c['rz']} * $x + $y - {c['rx']} * $z
  let z2 = {c['tz']} - {c['ry']} * $x + {c['rx']} * $y + $z
  let p = ($x2 * $x2 + $y2 * $y2).pow(0.5)
  let q = [$z2 * {c['aw']}, $p * {c['bw']}].apply("atan2")
  let sq = $q.sin()
  let cq = $q.cos()
  let lat = [$z2 + {c['ep2']} * {c['bw']} * $sq * $sq * $sq, $p - {c['e2w']} * {c['aw']} * $cq * $cq * $cq].apply("atan2")
  let lon = [$y2, $x2].apply("atan2")
  root = [($lon * {c["deg"]} * 10000000).round() / 10000000, ($lat * {c["deg"]} * 10000000).round() / 10000000]
}}
map sjtsk_coords {{
  root = if this.index(0).type() == "number" {{ this.apply("sjtsk") }} else {{ this.map_each(c -> c.apply("sjtsk_coords")) }}
}}"""


def entity(e: dict, slug: str, model_ranges: dict[str, str], source_url: str) -> str:
    kind = e["source"]["kind"]
    path = "this.properties" if kind in FEATURES else "this"
    cols = {a: p["column"] for a, p in e["properties"].items() if "template" not in p}
    keys = e["entityId"].get("keys")
    # The hash keeps reading the columns impersonal() dropped, so an Entity keeps its id and the next
    # replace overwrites the copy that still holds them (T-5697).
    hashed = e.get("hashed", cols)
    if keys:
        local = "[" + ", ".join(column(path, kind, k) for k in keys) + "].map_each(v -> v.string()).join(\"-\")"
    else:
        # No column is unique: the id is the hash of what is mapped, so a changed row is a new
        # entity (Research/bb-bbsk-ngsi-ld-mapping.md §4).
        local = "{" + ", ".join(f"{q(c)}: {column(path, kind, c)}" for c in hashed.values()) + "}" \
                ".format_json(no_indent: true).hash(\"sha256\").encode(\"hex\").slice(0, 24)"
    lines = [f"let mapping_rev = {q(REVISED[slug])}"] if slug in REVISED else []
    lines += [f"let local = {q(slug + '-')} + {local}.re_replace_all(\"[^A-Za-z0-9._~-]\", \"-\").slice(0, 80)"]
    observed = e.get("observedAt")
    if observed:
        lines.append(f"let at = {column(path, kind, observed)}.apply(\"observed\")")
    lines += [
        "root = {",
        # The publisher's domain, never the runner's JC_ORG_DOMAIN, which is the installation's (T-4202).
        f"  \"id\": \"urn:ngsi-ld:%v:%v:%v:%v\".format({q(e['type'])}, {q(PROJECTS[e['org']]['domain'])}, env(\"JC_SPACE\"), $local),",
        f"  \"type\": {q(e['type'])},",
    ]
    for a, p in e["properties"].items():
        # A column empty in a row gives way to the next the mapping names (`otherwise`, T-4319).
        read = [f"{column(path, kind, c)}.apply({q(model_ranges[attribute(a)].replace('-', '_'))})"
                for c in [p["column"], *p.get("otherwise", [])]]
        value = named(path, kind, p["template"]) if "template" in p else f"{'.or('.join(read)}{')' * (len(read) - 1)}"
        # A number measured at the row's instant carries it; the instant's own column does not.
        if observed and p["column"] != observed and model_ranges[attribute(a)] in ("integer", "float"):
            value = f"{{\"p\": {value}, \"at\": $at}}.apply(\"stamped\")"
        lines.append(f"  {q(attribute(a))}: {value},")
    for a, v in (e.get("constants") or {}).items():
        lines.append(f"  {q(a)}: {{ \"type\": \"Property\", \"value\": {q(v)} }},")
    if e.get("identifierFromId"):
        # The base requires an identifier no column holds: the entity's own key or row hash (T-4336).
        lines.append("  \"identifier\": { \"type\": \"Property\", \"value\": $local },")
    if kind in ESRI and e["source"].get("geometry"):
        lines.append("  \"location\": if this.geometry.or(null) == null { null } else { { \"type\": \"GeoProperty\", \"value\": this.geometry } },")
    elif kind == "zip-geojson":
        lines.append("  \"location\": if this.geometry.or(null) == null { null } else { { \"type\": \"GeoProperty\", "
                     "\"value\": { \"type\": this.geometry.type, \"coordinates\": this.geometry.coordinates.apply(\"sjtsk_coords\") } } },")
    credit = f"{e['dataProvider']} ({LICENCE.get(e['license'], e['license'])})"
    lines += [
        f"  \"dataProvider\": {{ \"type\": \"Property\", \"value\": {q(credit)} }},",
        f"  \"source\": {{ \"type\": \"Property\", \"value\": {q(source_url)} }},",
        f"  \"isPartOf\": {{ \"type\": \"Property\", \"value\": {q(e['dataset'])} }}",
        "}.filter(entry -> entry.value != null)",
    ]
    return "\n".join(lines)


def esri_query(e: dict, offset: str, out_fields: str | None = None) -> str:
    """The layer's query for one page. ArcGIS Online answers a URL past about 2 kB with an IIS 404
    (the BBSK religion layers, 50 columns of percent-encoded Slovak); such a layer asks for every
    field instead, which only a layer that drops no column may do."""
    s = e["source"]
    params = {
        "where": (e.get("rowFilter") or {}).get("where", "1=1"),
        # The id hash reads the columns impersonal() keeps out of the Entity: fetch them, or every id moves.
        "outFields": out_fields or ",".join(dict.fromkeys(
            list(e.get("hashed", {}).values()) + [p["column"] for p in e["properties"].values()])),
        "returnGeometry": "true" if s.get("geometry") else "false",
        "outSR": "4326", "geometryPrecision": "6", "f": "geojson",
        "resultOffset": offset, "resultRecordCount": str(PAGE),
    }
    # In object id order, so the pages of one run neither skip nor repeat a row: an unordered
    # resultOffset page is any slice the server likes, and a skipped row is deleted in replace mode.
    # The field is the layer's own (FID, OBJECTID, ObjectId), read from its ?f=json when mapped.
    if s.get("objectIdField"):
        params["orderByFields"] = s["objectIdField"]
    url = f"{s['url']}/query?{urllib.parse.urlencode(params, quote_via=urllib.parse.quote, safe=',*')}"
    url = url.replace(urllib.parse.quote("${! @offset }"), "${! @offset }")
    # Decided on the paging template's length, the longest this query is ever written, so page 0
    # and the further pages ask for the same fields.
    if len(url) - len(offset) + len("${! @offset }") > URL_LIMIT and out_fields is None:
        if e.get("dropped"):
            sys.exit(f"{e['name']}: its query is {len(url)} bytes and it drops columns, so it cannot ask for *")
        return esri_query(e, offset, "*")
    return url


def esri_count(e: dict) -> str:
    """How many rows the layer's filter holds, asked when page 0 says there is more (T-5288)."""
    params = {"where": (e.get("rowFilter") or {}).get("where", "1=1"), "returnCountOnly": "true", "f": "json"}
    return f"{e['source']['url']}/query?{urllib.parse.urlencode(params, quote_via=urllib.parse.quote, safe=',*')}"


def layer_query(layer: dict, offset: str) -> str:
    """One page of one layer of the city's ArcGIS Server (T-4317): its own columns and the object id,
    which the GeoJSON answer then carries as the feature's `id`; in object id order, so the pages
    of one run neither skip nor repeat a row."""
    params = {
        "where": layer.get("where", "1=1"), "outFields": ",".join(["OBJECTID", *layer["fields"]]),
        "orderByFields": "OBJECTID", "returnGeometry": "true",
        "outSR": "4326", "geometryPrecision": "6", "f": "geojson",
        "resultOffset": offset, "resultRecordCount": str(PAGE),
    }
    return f"{layer['url']}/query?{urllib.parse.urlencode(params, quote_via=urllib.parse.quote, safe=',*')}"


# Rows one pipeline of the city's layers reads in a run. Bento holds every page of a run until the
# run's end: the terrain set in one pipeline (36 062 rows, 57 pages) peaked at 897 MB, 8000 of its
# contour lines at 422 MB (2026-10-16). So a dataset of more rows is read by several pipelines, each
# within the 256 MB its quota names (T-4317).
PART_ROWS = 4000
# Who asks: the city's GIS server is a small public one, so its operator can tell our requests apart.
AGENT = "joinedcontext-platform open-data pipeline (https://opendata.dev.joinedcontext.com)"


def parts(e: dict, slug: str) -> list[tuple[dict, str]]:
    """An `esri-layers` entry as pipelines of about PART_ROWS rows each, with their slugs: every page
    of every layer up to half again the rows it held when mapped (ponytail: a static page count, since
    each part's pages are fixed to keep it in its memory quota; past that the newest rows wait until
    the mapping is regenerated, and a page past the end is empty). Any other entry is itself."""
    s = e["source"]
    if s["kind"] != "esri-layers":
        return [(e, slug)]
    groups: list[list[dict]] = [[]]
    held = 0
    for x in s["layers"]:
        for o in range(max(1, -(-int(x["rows"] * 1.5) // PAGE))):
            rows = max(0, min(PAGE, x["rows"] - o * PAGE))
            if groups[-1] and held + rows > PART_ROWS:
                groups.append([])
                held = 0
            groups[-1].append({**x, "offset": o * PAGE})
            held += rows
    return [({**e, "source": {**s, "pages": g}}, slug if i == 0 else f"{slug[:36].rstrip('-')}-p{i + 1}")
            for i, g in enumerate(groups)]


def source_url(e: dict) -> str:
    s = e["source"]
    if s["kind"] == "esri-featureserver":
        return esri_query(e, "0")
    if s["kind"] == "esri-layers":
        first = s["pages"][0]
        return layer_query(first, str(first["offset"]))
    if s["kind"] == "zip-geojson":
        return s["url"]
    fields = ",".join(dict.fromkeys(c for p in e["properties"].values() for c in [p["column"], *p.get("otherwise", [])]))
    return f"{s['url']}?per_page={WP_PAGE}&_fields={fields}"


# Posts per WordPress REST request: the most `per_page` takes.
WP_PAGE = 100


def wp_count(e: dict) -> str:
    """One post's id, asked only for the X-WP-Total header: how many posts the feed holds."""
    return f"{e['source']['url']}?per_page=1&_fields=id"


def bento(e: dict, slug: str, model_ranges: dict[str, str]) -> str:
    s = e["source"]
    read = source_url(e)
    head = [f"# The mapping of the od-{slug} pipeline, generated from the T-4254 mapping: one {e['type']} per row of",
            f"# \"{e['title']}\" ({e['dataset']}). Do not edit: change the mapping and run scripts/open-data/generate.py.",
            "", "pipeline:", "  processors:"]
    steps: list[str] = []
    if s["kind"] == "esri-featureserver":
        steps.append(f"""\
    # A layer that answers an error or no feature list is the service failing, so the run fails
    # where it can be seen rather than writing nothing.
    - mapping: |
        root = if this.error.or(null) != null || this.features.or(null) == null {{ throw("the layer answered no features: %v".format(this.error.message.or(""))) }} else {{ this }}
    # Page 0 as it came. When the layer says there is more, it is asked how many rows it holds and
    # every further page of them is read (T-5288), one page at a time: the runner fetches, maps and
    # writes a page before the next, so a layer of any size runs in a page's memory (jc-ekiben-paging).
    - branch:
        request_map: |
          root = if this.properties.exceededTransferLimit.or(false) {{ "" }} else {{ deleted() }}
        processors:
          - http:
              url: {q(esri_count(e))}
              verb: GET
              headers:
                Accept: application/json
              timeout: 60s
              retries: 2
              rate_limit: pipeline_egress
        result_map: |
          root.rowCount = this.count.or(this.error.message.or("no count"))
    - mapping: |
        let rows = this.rowCount.or(null)
        root = if !this.properties.exceededTransferLimit.or(false) {{ [this] }} else if errored() || $rows.type() != "number" {{ [{{ "problem": "the layer did not say how many rows it holds: %v".format(error().or($rows)) }}] }} else if this.features.length() < [{PAGE}, $rows].min() {{ [{{ "problem": "the layer answered %v rows where {PAGE} were asked, so its pages would skip rows".format(this.features.length()) }}] }} else if $rows <= {PAGE} {{ [this] }} else {{ [this].concat(range(1, (($rows + {PAGE - 1}) / {PAGE}).floor()).map_each(p -> {{ "offset": p * {PAGE}, "rowCount": $rows }})) }}
    - unarchive:
        format: json_array
    # Each page its own batch: fetched, mapped and written before the next is read.
    - split:
        size: 1
    - switch:
        - check: this.offset.or(null) != null
          processors:
            - mapping: |
                meta offset = this.offset
                meta rows = this.rowCount
                root = ""
            - http:
                url: {q(esri_query(e, "${! @offset }"))}
                verb: GET
                headers:
                  Accept: application/geo+json
                timeout: 60s
                retries: 2
                rate_limit: pipeline_egress
            - mapping: |
                root = if errored() {{ throw("the page at %v did not come: %v".format(@offset, error())) }} else if this.features.or(null) == null {{ {{ "problem": "the page at %v answered no features: %v".format(@offset, this.error.message.or("")) }} }} else if this.features.length() < [{PAGE}, @rows - @offset].min() {{ {{ "problem": "the page at %v answered %v rows where %v were due".format(@offset, this.features.length(), [{PAGE}, @rows - @offset].min()) }} }} else {{ this }}
    # A refusal above fails the run and writes none of its page's rows; earlier pages stay written
    # and the failed run deletes nothing (sourceDeletes waits for a finished run).
    - mapping: |
        root = if this.problem.or(null) != null {{ throw(this.problem) }} else {{ this }}
    - mapping: |
        root = this.features.or([])""")
    elif s["kind"] == "esri-layers":
        # The part's further pages, listed when generated (`parts`); its first is what the
        # DataSource fetched. Each row learns its layer and its object id here.
        first, *rest = s["pages"]
        tag = lambda x: {"key": x["key"], "name": x["name"]}  # noqa: E731
        stubs = [{**tag(x), "url": layer_query(x, str(x["offset"]))} for x in rest]
        # PL-07: the step names the server; only the page's path and query come from the message.
        split = urllib.parse.urlsplit(first["url"])
        origin = f"{split.scheme}://{split.netloc}/"
        assert all(x["url"].startswith(origin) for x in stubs), f"every page on {origin}"
        steps.append(f"""\
    - mapping: |
        let rest = {json.dumps(stubs, ensure_ascii=False)}
        root = [{{ "page": this }}.merge({json.dumps(tag(first), ensure_ascii=False)})].concat($rest)
    - unarchive:
        format: json_array
    # Each page its own batch: fetched, mapped and written before the next is read.
    - split:
        size: 1
    - switch:
        - check: this.url.or(null) != null
          processors:
            - mapping: |
                meta url = this.url
                meta path = this.url.trim_prefix({json.dumps(origin)})
                meta key = this.key
                meta name = this.name
                root = ""
            - http:
                url: "{origin}${{! @path }}"
                verb: GET
                headers:
                  Accept: application/geo+json
                  User-Agent: {q(AGENT)}
                timeout: 60s
                retries: 2
                rate_limit: pipeline_egress
            - mapping: |
                root = if errored() {{ throw("the page %v did not come: %v".format(@url, error())) }} else {{ {{ "page": this, "key": @key, "name": @name }} }}
    # A layer that answers an error or no feature list is the service failing, so the run fails
    # where it can be seen rather than writing nothing.
    - mapping: |
        let key = this.key
        let name = this.name
        root = if this.page.error.or(null) != null || this.page.features.or(null) == null {{ throw("the layer %v answered no features: %v".format($key, this.page.error.message.or(""))) }} else {{ this.page.features.map_each(f -> f.merge({{ "properties": {{ "vrstva": $name, "vrstva_id": "%v-%v".format($key, f.id) }} }})) }}""")
    elif s["kind"] == "zip-geojson":
        if s["crs"] != "EPSG:5514":
            sys.exit(f"{e['name']}: no step reprojects {s['crs']}")
        # The measured size of the archive (`bytes` in the mapping) times four caps the download (T-5701).
        cap = 4 * int(s["bytes"])
        steps.append(f"""\
    # The whole ZIP as it came, its size capped before it is unpacked: {s["bytes"]} bytes when measured,
    # four times that at most (T-5701). Bento cannot cap what a member inflates to, so this packed size is
    # the only bound. Its one GeoJSON member, picked by its ending: the archive sets no
    # UTF-8 flag, so a name with diacritics arrives as cp437 bytes. Coordinates are S-JTSK
    # (EPSG:5514), reprojected to WGS 84 in the mapping below (T-4317).
    - try:
        - mapping: |
            root = if content().length() > {cap} {{ throw("the archive holds %v bytes; more than {cap} is not this dataset".format(content().length())) }} else {{ content() }}
        - unarchive:
            format: zip
    # An errored zip skipped the unpack: it goes on as a problem, never as rows.
    - mapping: |
        root = if errored() {{ {{ "problem": error() }} }} else if @archive_filename.or("").lowercase().has_suffix(".geojson") {{ this }} else {{ deleted() }}
    # A refusal fails the run and writes none of its rows.
    - mapping: |
        root = if this.problem.or(null) != null {{ throw(this.problem) }} else if this.features.or(null) == null {{ throw("the archive's GeoJSON holds no features") }} else {{ this.features }}
    - mapping: |
        root = if this.type() == "array" {{ this }} else {{ [] }}""")
    else:
        steps.append(f"""\
    - mapping: |
        root = if this.type() != "array" {{ throw("the site answered no post list") }} else {{ {{ "posts": this }} }}
    # Page 1 as it came. A full page means there may be more: the feed is asked how many posts it
    # holds (X-WP-Total) and every further page of them is read, so the replace-mode run does not
    # delete each post past the newest {WP_PAGE} (F-BNT1-011), one page at a time (jc-ekiben-paging).
    - branch:
        request_map: |
          root = if this.posts.length() < {WP_PAGE} {{ deleted() }} else {{ "" }}
        processors:
          - http:
              url: {q(wp_count(e))}
              verb: GET
              headers:
                Accept: application/json
              extract_headers:
                include_patterns: ["(?i)^x-wp-total$"]
              timeout: 60s
              retries: 2
              rate_limit: pipeline_egress
        result_map: |
          root.total = if errored() {{ error() }} else {{ metadata("x-wp-total").or("no X-WP-Total header").number().catch("no X-WP-Total header") }}
    - mapping: |
        let total = this.total.or(null)
        root = if this.posts.length() < {WP_PAGE} {{ [this] }} else if $total.type() != "number" {{ [{{ "problem": "the feed did not say how many posts it holds: %v".format($total) }}] }} else {{ [this].concat(range(2, (($total + {WP_PAGE - 1}) / {WP_PAGE}).floor() + 1).map_each(p -> {{ "page": p }})) }}
    - unarchive:
        format: json_array
    # Each page its own batch: fetched, mapped and written before the next is read.
    - split:
        size: 1
    - switch:
        - check: this.page.or(null) != null
          processors:
            - mapping: |
                meta page = this.page
                root = ""
            - http:
                url: {q(source_url(e) + "&page=${! @page }")}
                verb: GET
                headers:
                  Accept: application/json
                timeout: 60s
                retries: 2
                rate_limit: pipeline_egress
            - mapping: |
                root = if errored() {{ throw("the page %v did not come: %v".format(@page, error())) }} else if this.type() != "array" {{ {{ "problem": "the page %v answered no post list".format(@page) }} }} else {{ {{ "posts": this }} }}
    # A refusal above fails the run and writes none of its page's rows; earlier pages stay written
    # and the failed run deletes nothing (sourceDeletes waits for a finished run).
    - mapping: |
        root = if this.problem.or(null) != null {{ throw(this.problem) }} else {{ this.posts }}""")
    maps = MAPS + ("\n" + krovak_maps() if s["kind"] == "zip-geojson" else "")
    body = "\n".join("        " + line for line in (maps + "\n" + entity(e, slug, model_ranges, read)).splitlines())
    keep = (e.get("rowFilter") or {}).get("keep")
    if keep:
        path = "this.properties" if s["kind"] in FEATURES else "this"
        keep = f"""
    # A row stays only when its {keep["column"]} looks like one, which no Esri SQL can ask:
    # {e["rowFilter"]["why"]} (T-4255).
    - mapping: |
        root = if {column(path, s["kind"], keep["column"])}.or("").string().trim().re_match({q(keep["pattern"])}) {{ this }} else {{ deleted() }}"""
    steps.append(f"""\
    - unarchive:
        format: json_array{keep or ""}
    - mapping: |
{body}
    # One request per run's batch rather than one per row.
    - split:
        size: 1000
        byte_size: {REQUEST_BYTES}
    - archive:
        format: json_array""")
    return "\n".join(head) + "\n" + "\n".join(steps) + "\n"


# What one upsert carries at most: half of Antares' 4 MiB request body limit (ANTARES_MAX_BODY_BYTES), so
# the gateway's re-serialised body fits too; a 1000-row page of polygons was ~8 MB and refused whole (T-5309).
REQUEST_BYTES = 2 * 1024 * 1024


# On since portal 47b900c5 (on dev from apply 90): an http source runs on its pipeline's cron
# `schedule`, not every 60 s, which OOM-killed the runner on 2026-10-16 (T-4347).
ENABLED = True
# Every pipeline runs: since T-4348 (on dev from apply 95) a scheduled pipeline is loaded only for
# its own run, one batch at a time, so the second half adds queue time, not runner memory (T-4515).
# 2 runs every other pipeline again (T-4347's halves) should the runner need it.
HALVES = 1

def schedule(e: dict, i: int, day: int) -> str:
    """Pipeline i's cron: its own minute, on the day of its dataset's first part (`day`), so the
    parts of a split dataset are written the same night, not on different weekdays (T-4517)."""
    minute, hour = (i * 7) % 60, 1 + (i * 7) // 60 % 5
    return {"P1D": f"{minute} {hour} * * *", "P1W": f"{minute} {hour} * * {day % 7}",
            "P1M": f"{minute} {hour} {1 + day % 28} * *"}[e["refresh"]]


def manifest(comment: str, doc: dict) -> str:
    text = yaml.safe_dump(doc, allow_unicode=True, sort_keys=False, width=110)
    return "".join(f"# {line}\n" if line else "#\n" for line in comment.splitlines()) + text


def sdm_iris(entries: list[dict]) -> dict[str, tuple[str, str]]:
    """Each term of the entries that their Smart Data Model names: the IRI its vendored @context gives
    it, and that @context's URL at the pinned commit (T-4328)."""
    pins = json.loads((SDM / "pins.json").read_text())
    contexts: dict[str, dict] = {}
    iris: dict[str, tuple[str, str]] = {}
    for e in entries:
        model = (e.get("baseModel") or e["smartDataModel"]).split("/")[0]
        if model not in contexts:
            contexts[model] = json.loads((SDM / model.removeprefix("dataModel.") / "context.jsonld").read_text())["@context"]
        source = f"https://raw.githubusercontent.com/smart-data-models/{model}/{pins[model]}/context.jsonld"
        terms = [*COMMON, *map(attribute, e["properties"]), *(e.get("constants") or {}), *(["identifier"] if e.get("identifierFromId") else [])]
        if e.get("baseModel"):
            iris.setdefault(e["type"], (JC + e["type"], f"{EXTENSION_URL}/{e['type']}/context.jsonld"))
        else:
            terms.insert(0, e["type"])
        for term in terms:
            iri = contexts[model].get(term)
            if not isinstance(iri, str):
                continue
            if iris.setdefault(term, (iri, source))[0] != iri:
                sys.exit(f"{e['name']}: {model} gives `{term}` {iri}, another model of the space {iris[term][0]}")
    return iris


def label(texts: dict, generated: list[str] | None = None) -> dict:
    """A slot's label in each language as a LinkML annotation: an explicit tag and value, since LinkML
    reads a bare `{sk: ...}` map as the annotation's own fields and refuses the model (T-5167). The
    languages a machine wrote are named in the label's own `generated` annotation (T-5332)."""
    return {"tag": "label", "value": texts, **({"annotations": {"generated": " ".join(generated)}} if generated else {})}


def labels() -> dict[str, dict[str, str]]:
    """labels.tsv: slot -> its generated text per language; an empty cell keeps the model's own."""
    rows = (line.split("\t") for line in LABELS.read_text(encoding="utf-8").splitlines() if line and not line.startswith("#"))
    return {name: {lang: text for lang, text in zip(LANGS, texts) if text} for name, *texts in rows}


def labelled(slots: dict[str, dict], table: dict[str, dict[str, str]]) -> None:
    """Completes each slot's label from the table: a table cell wins over a column heading (headings are
    cut at 31 characters) and is marked generated; a slot neither names keeps what it has."""
    for name, slot in slots.items():
        more = table.get(name, {})
        if not more:
            continue
        own = (slot["annotations"].get("label") or {}).get("value", {})
        texts = {lang: more.get(lang) or own[lang] for lang in LANGS if more.get(lang) or own.get(lang)}
        slot["annotations"]["label"] = label(texts, [lang for lang in LANGS if lang in more])


def linkml(project: str, space: str, entries: list[dict], model_ranges: dict[str, str]) -> str:
    slots: dict[str, dict] = {}
    for a, (r, d) in COMMON.items():
        slots[a] = {"description": d, **({"range": r} if r != "string" else {}),
                    "annotations": {"ngsi_ld_kind": "Property", "label": label(COMMON_LABELS[a])}}
    columns: dict[str, str] = {}
    for e in entries:
        for a, p in e["properties"].items():
            if "template" not in p:
                columns.setdefault(attribute(a), p["column"])
    for e in entries:
        for a, p in e["properties"].items():
            if "template" in p:
                columns.setdefault(attribute(a), p["template"])
    for a in sorted(columns):
        r = model_ranges[a]
        built = "{" in columns[a]
        slots[a] = {"description": f"Built from the source columns as `{columns[a]}`." if built else f"The source column `{columns[a]}`.",
                    **({"multivalued": True} if r == "string-list" else {"range": r} if r != "string" else {}),
                    "annotations": {"ngsi_ld_kind": "Property",
                                    **({"label": label({"sk": h})} if not built and (h := heading(columns[a])) else {})}}
        if a in GENERIC:
            slots[a] = {**slots[a], "description": GENERIC[a][0],
                        "annotations": {"ngsi_ld_kind": "Property", "label": label(GENERIC[a][1])}}
    for e in entries:
        for a, v in (e.get("constants") or {}).items():
            slots.setdefault(a, {"description": f"`{v}` on every entity: the model requires it and no column holds it.",
                                 "annotations": {"ngsi_ld_kind": "Property"}})
        if e.get("identifierFromId"):
            slots.setdefault("identifier", {"description": "The entity's own key or row hash where the base model requires an "
                                                           "identifier no column holds (T-4336).",
                                            "annotations": {"ngsi_ld_kind": "Property"}})
    # The model's IRI for every term it names, so a client expanding with the SDM @context reads what the
    # broker holds; the source's own columns stay jc: (T-4328).
    iris = sdm_iris(entries)
    for a, slot in slots.items():
        if a in iris:
            slot["slot_uri"], slot["annotations"]["upstream_source"] = iris[a]
    classes: dict[str, dict] = {}
    for e in sorted(entries, key=lambda e: e["type"]):
        c = classes.setdefault(e["type"], {"class_uri": iris[e["type"]][0], "source": iris[e["type"]][1], "titles": [], "is_a": "Entity", "slots": [*COMMON]})
        c["titles"].append(e["title"])
        own = [*map(attribute, e["properties"]), *(e.get("constants") or {}), *(["identifier"] if e.get("identifierFromId") else [])]
        c["slots"] += [a for a in own if a not in c["slots"]]
    retired = RETIRED.get(project, {})
    for t, c in classes.items():
        for a in sorted({*retired.get("*", []), *retired.get(t, [])} - set(c["slots"])):
            slots.setdefault(a, {"description": "An attribute an earlier version of the mapping wrote; the next run removes it.",
                                 "deprecated": "No mapping writes it any more; kept so a replace may remove it.",
                                 "annotations": {"ngsi_ld_kind": "Property"}})
            c["slots"].append(a)
    labelled(slots, labels())
    for c in classes.values():
        titles = c.pop("titles")
        c["description"] = "One row of " + "; ".join(sorted(set(titles)))[:400] + "."
        c["slots"] = c["slots"][:3] + sorted(c["slots"][3:])
        c["class_uri"], c["description"], c["source"] = c.pop("class_uri"), c.pop("description"), c.pop("source")
    doc = {
        "id": f"https://joinedcontext.com/models/{project}/{space}", "name": space,
        "title": PROJECTS[project]["title"]["en"], "description": PROJECTS[project]["about"]["en"],
        "license": "https://joinedcontext.com/licenses/cc-by-4.0",
        "prefixes": {"linkml": "https://w3id.org/linkml/", "jc": "https://joinedcontext.com/models/",
                     "sdm": "https://smartdatamodels.org/", "ngsi-ld": "https://uri.etsi.org/ngsi-ld/"},
        "default_prefix": "jc", "default_range": "string", "imports": ["linkml:types", "ngsi-ld-core"],
        "slots": slots,
        "classes": {k: {"class_uri": v["class_uri"], "description": v["description"], "is_a": "Entity", "slots": v["slots"],
                        "annotations": {"upstream_source": v["source"]}}
                    for k, v in sorted(classes.items())},
    }
    return manifest("Generated by scripts/open-data/generate.py from the T-4254 mapping (T-4255). One class per type the\n"
                    "mapping names, one slot per column it reads; do not edit by hand.", doc)


def slug_for(path: Path) -> str:
    """The Endpoint's slug: kept from the committed file, minted once (EP-02)."""
    if path.exists():
        return yaml.safe_load(path.read_text())["spec"]["slug"]
    return base64.b32encode(secrets.token_bytes(20)).decode().lower().rstrip("=")


def project_files(project: str, entries: list[dict], names: dict[str, str]) -> dict[str, str]:
    p = PROJECTS[project]
    space, pre = f"{project}-otvorene-data", f"{project}-od-"
    types = sorted({e["type"] for e in entries})
    model_ranges = ranges(entries)
    files: dict[str, str] = {}
    meta = lambda name, title: {"name": name, "namespace": project, "title": title}  # noqa: E731
    head = {"apiVersion": "joinedcontext.com/v1alpha1"}
    files[f"{pre}space.yaml"] = manifest(
        "Every dataset of the project's catalogue that reads as a table, one class per kind of row (T-4255).\n"
        "Generated by scripts/open-data/generate.py; the curated spaces keep their own models.",
        {**head, "kind": "ContextSpace", "metadata": {**meta(space, p["title"]), "description": p["about"]},
         "spec": {"urnSegment": space, "isSandbox": False, "dataModelRef": {"kind": "DataModel", "name": space},
                  "defaultLocale": "sk",
                  "retention": [{"entityType": t, **RECORDED} if t in MEASURED else {"entityType": t, "history": "off"} for t in types]}
         | area_rule(p, entries)})
    files[f"{pre}datamodel.yaml"] = manifest(
        "The one model of the space (DM-61), generated with it.",
        {**head, "kind": "DataModel", "metadata": meta(space, p["title"]),
         "spec": {"contextSpaceRef": space, "linkml": f"./{space}.linkml.yaml", "version": "1.0.0", "lifecycle": "published",
                  "classes": types,
                  "artifacts": {"jsonSchema": f"./{space}.v1.schema.json", "context": f"./{space}.v1.context.jsonld",
                                "docs": f"./{space}.v1.md", "example": f"./{space}.v1.example.jsonld"}}})
    files[f"{space}.linkml.yaml"] = linkml(project, space, entries, model_ranges)
    endpoint = SEED / project / f"{pre}endpoint.yaml"
    # The Policies drafted below, the door's whole set (P-18); the Pipelines' derived writes bind it through their targetEndpoint.
    policies = ["otvorene-data-pipelines-write", "otvorene-data-read", *(["otvorene-data-read-history"] if any(t in MEASURED for t in types) else [])]
    files[f"{pre}endpoint.yaml"] = manifest(
        "The space's one door, public: every row is the publisher's open data under the licence its\n"
        "`dataProvider` names. A member of the project's public MCP server, which the public assistant reads.",
        {**head, "kind": "Endpoint", "metadata": {**meta(space, p["title"]), "description": p["about"]},
         "spec": {"contextSpaceRef": space, "policyRefs": [f"urn:ngsi-ld:Policy:{p['domain']}:{space}:{n}" for n in policies],
                  "slug": slug_for(endpoint), "audience": "public",
                  "enabledRepresentations": REPRESENTATIONS + (["feed"] if "Article" in types else []),
                  **news_feed(p, types), "catalog": catalog_block(project, p, entries)}})
    information = [{"entities": [{"type": t} for t in types]}]
    files[f"{pre}policy-read.yaml"] = manifest(
        "Anyone reads every type of the space: each is open data, and no mapping reads a column that names\n"
        "or reaches a person (Research/bb-bbsk-ngsi-ld-mapping.md §5).",
        {**head, "kind": "Policy", "metadata": meta("otvorene-data-read", {"sk": "Verejné čítanie otvorených dát", "en": "Public read: open data"}),
         "spec": {"contextSpaceRef": {"kind": "ContextSpace", "name": space}, "assigner": f"did:web:{p['domain']}",
                  "assignee": {"kind": "role", "id": "public"},
                  "operations": ["retrieveOps", "retrieveEntityTypes", "retrieveEntityTypeDetails",
                                 "retrieveEntityTypeInfo", "retrieveAttrTypes", "retrieveAttrTypeDetails",
                                 "retrieveAttrTypeInfo", "createSubscription", "retrieveSubscription", "updateSubscription", "deleteSubscription"],
                  "information": information}})
    measured = [t for t in types if t in MEASURED]
    if measured:
        files[f"{pre}policy-read-history.yaml"] = manifest(
            "Anyone reads the history of the types that keep one, as far back as the space keeps it (T-4372):\n"
            "the same entities as the read, over time. The other types keep no history.",
            {**head, "kind": "Policy", "metadata": meta("otvorene-data-read-history", {"sk": "Verejné čítanie histórie otvorených dát", "en": "Public read: open data history"}),
             "spec": {"contextSpaceRef": {"kind": "ContextSpace", "name": space}, "assigner": f"did:web:{p['domain']}",
                      "assignee": {"kind": "role", "id": "public"},
                      "operations": ["queryTemporal", "retrieveTemporal"], "temporalQ": "timerel=after;timeAt=P-30D",
                      "information": [{"entities": [{"type": t} for t in measured]}]}})
    files[f"{pre}policy-pipelines-write.yaml"] = manifest(
        "The generated pipelines upsert; nothing deletes, since several pipelines write the space (PL-64).",
        {**head, "kind": "Policy", "metadata": meta("otvorene-data-pipelines-write", {"sk": "Kanály zapisujú otvorené dáta", "en": "The pipelines write the open data"}),
         "spec": {"contextSpaceRef": {"kind": "ContextSpace", "name": space}, "assigner": f"did:web:{p['domain']}",
                  "assignee": {"kind": "serviceAccount", "id": "pipelines"},
                  "operations": ["upsertBatch", "createBatch", "queryBatch"], "information": information}})
    index = [f"{pre}space.yaml: projects/{project}/spaces/{space}/space.yaml",
             f"{pre}datamodel.yaml: projects/{project}/spaces/{space}/datamodels/{space}.yaml",
             *(f"{space}.{x}: projects/{project}/spaces/{space}/datamodels/{space}.{x}"
               for x in ("linkml.yaml", "v1.schema.json", "v1.context.jsonld", "v1.md", "v1.example.jsonld")),
             f"{pre}endpoint.yaml: projects/{project}/spaces/{space}/endpoints/{space}.yaml",
             f"{pre}policy-read.yaml: projects/{project}/spaces/{space}/policies/otvorene-data-read.yaml",
             *([f"{pre}policy-read-history.yaml: projects/{project}/spaces/{space}/policies/otvorene-data-read-history.yaml"]
               if measured else []),
             f"{pre}policy-pipelines-write.yaml: projects/{project}/spaces/{space}/policies/otvorene-data-pipelines-write.yaml"]
    pipelines = [p for e in entries for p in parts(e, names[e["name"]])]
    taken = [slug for _, slug in pipelines]
    if len(set(taken)) != len(taken):
        sys.exit(f"{project}: two pipelines take one name: {sorted({x for x in taken if taken.count(x) > 1})}")
    first: dict[str, int] = {}
    for i, (e, slug) in enumerate(pipelines):
        first.setdefault(e["name"], i)
        if e["source"]["kind"] == "endpoint":
            # The rows live only in the Endpoint: the legacy upload's one copy was the CKAN DataStore,
            # filled back from this Endpoint (T-4742), so no DataSource or Pipeline reads it (T-5060).
            continue
        rows = e["source"]["rows"]
        files[f"{pre}datasource-{slug}.yaml"] = manifest(
            f"\"{e['title']}\", {e['dataset']}: {rows} rows when mapped. {e['dataProvider']},\n"
            f"{LICENCE.get(e['license'], e['license'])}. No key. Generated by scripts/open-data/generate.py (T-4255).",
            {**head, "kind": "DataSource", "metadata": meta(f"od-{slug}", {"sk": e["title"][:120], "en": e["title"][:120]}),
             "spec": {"type": "http", "http": {"url": source_url(e), "verb": "GET",
                                              "headers": {"Accept": "application/geo+json, application/json",
                                                          **({"User-Agent": AGENT} if e["source"]["kind"] == "esri-layers" else {})},
                                              "timeout": "60s"}}})
        files[f"{pre}pipeline-{slug}.yaml"] = manifest(
            f"One {e['type']} per row of \"{e['title']}\" (T-4255); {e.get('note') or 'generated from the T-4254 mapping'}.",
            {**head, "kind": "Pipeline", "metadata": meta(f"od-{slug}", {"sk": e["title"][:120], "en": e["title"][:120]}),
             "spec": {"class": "scheduled", "enabled": ENABLED and i % HALVES == 0, "schedule": schedule(e, i, first[e["name"]]),
                      "source": {"dataSourceRef": {"kind": "DataSource", "name": f"od-{slug}"}},
                      "targetEndpoint": f"urn:ngsi-ld:Endpoint:{p['domain']}:{space}:{space}",
                      # Each od pipeline alone writes its rows (the id carries its slug), so it
                      # replaces them whole: an attribute a mapping drops leaves at the next run
                      # (PL-79, T-4407; T-4289's `dataset`).
                      "output": {"type": e["type"], "mode": "replace"},
                      "quotas": {"maxMemoryMb": 256 if rows > 5000 else 128, "cpuMillicores": 100}}})
        files[f"{pre}pipeline-{slug}-bento.yaml"] = bento(e, slug, model_ranges)
        index += [f"{pre}datasource-{slug}.yaml: projects/{project}/datasources/od-{slug}.yaml",
                  f"{pre}pipeline-{slug}.yaml: projects/{project}/pipelines/od-{slug}/pipeline.yaml",
                  f"{pre}pipeline-{slug}-bento.yaml: projects/{project}/pipelines/od-{slug}/bento.yaml"]
    old = (SEED / project / "index.yaml").read_text()
    block = "\n".join([BEGIN, *index, END])
    if BEGIN in old:
        new = re.sub(re.escape(BEGIN) + r".*?" + re.escape(END), lambda _: block, old, flags=re.S)
    else:
        new = old.rstrip("\n") + "\n" + block + "\n"
    files["index.yaml"] = new
    return files


def sdm_schema(e: dict) -> Path:
    """The vendored schema.json of the entry's Smart Data Model, or of the base its extension extends
    (the extension's own is written by this script): a type without one is not seeded."""
    model = e.get("smartDataModel") or ""
    if model.startswith("extension/"):
        base = SDM / (e.get("baseModel") or "").removeprefix("dataModel.") / "schema.json"
        if model != f"extension/{e['type']}" or not (e.get("baseModel") or "").startswith("dataModel.") or not base.is_file():
            sys.exit(f"{e['name']}: {model} extends no vendored base ({e.get('baseModel') or 'none'}), see {SDM.relative_to(ROOT)}")
        return EXTENSIONS / e["type"] / "schema.json"
    path = SDM / model.removeprefix("dataModel.") / "schema.json"
    if not model.startswith("dataModel.") or not path.is_file():
        sys.exit(f"{e['name']}: mapped without a vendored Smart Data Model ({model or 'none'}), see {SDM.relative_to(ROOT)}")
    return path


# Personal data never reaches a public space (ADR-N-054, T-5697): a director's or statutory
# representative's name, phone and e-mail, and a supplier's street (a sole trader's IČO names a
# person, whose seat is their home); the supplier's name, IČO, town and postcode stay public.
PERSONAL = re.compile(r"^(riaditel|statutar)", re.IGNORECASE)
SUPPLIERS = re.compile(r"^bbsk-(dodavatelia|dotacie|faktury|objednavky|zmluvy)-")
SUPPLIER_STREET = {"adresa", "ulicaACislo", "sidloDodavatela"}


def impersonal(e: dict) -> None:
    """Takes the personal columns out of a dataset's mapping before anything is written from it."""
    street = SUPPLIER_STREET if SUPPLIERS.match(e["name"]) else set()
    # Only a row-hash id reads the dropped columns; a keyed id never does, so its query stops
    # asking for them and its public `source` link names none (F-R2BNT1-007).
    if not e["entityId"].get("keys"):
        e["hashed"] = {a: p["column"] for a, p in e["properties"].items() if "template" not in p}
    e["properties"] = {a: p for a, p in e["properties"].items() if not PERSONAL.match(a) and a not in street}


def render() -> dict[Path, str]:
    mapping = yaml.safe_load(MAPPING.read_text())
    mapped = [e for e in mapping["datasets"] if e["status"] == "mapped"
              and (e.get("existing") or {}).get("covers") != "whole"]
    for e in mapped:
        impersonal(e)
        sdm_schema(e)
        if e["name"] in NAMES and "name" not in e["properties"]:
            template = NAMES[e["name"]]
            e["properties"]["name"] = {"column": re.search(r"\{([^}]+)\}", template).group(1),
                                       "template": template, "range": "string"}
    names = slugs(mapped)
    out: dict[Path, str] = {}
    for project in PROJECTS:
        entries = [e for e in mapped if e["org"] == project]
        for name, text in project_files(project, entries, names).items():
            out[SEED / project / name] = text
    whole = [e for e in mapping["datasets"] if e["status"] == "mapped"
             and (e.get("existing") or {}).get("covers") == "whole"]
    out[DISTRIBUTIONS] = distributions(mapped, whole)
    out.update(extension_files(mapping.get("extensions") or {}, mapped))
    return out


def declared(schema: dict, common: dict) -> set[str]:
    """Every attribute a Smart Data Model schema names, the common-schema blocks it refers to included."""
    names: set[str] = set()

    def walk(x):
        if isinstance(x, dict):
            names.update((x.get("properties") or {}).keys())
            ref = x.get("$ref", "")
            if "common-schema.json#/definitions/" in ref and "/properties/" not in ref:
                walk(common[ref.rsplit("/", 1)[1]])
            for v in x.values():
                walk(v)
        elif isinstance(x, list):
            for v in x:
                walk(v)
    walk(schema)
    return names


def extension_files(extensions: dict[str, dict], mapped: list[dict]) -> dict[Path, str]:
    """Per extension: its schema.json and its LinkML. The base's schema.json at its pin, with only its
    `type` set to the extension's, is the first member of `allOf`, so every attribute and requirement
    of the base holds; the second names what the type's datasets add, typed as the seed writes it."""
    common = json.loads((SDM / "common-schema.json").read_text())["definitions"]
    pins = json.loads((SDM / "pins.json").read_text())
    space_ranges = {project: ranges([e for e in mapped if e["org"] == project]) for project in PROJECTS}
    out: dict[Path, str] = {}
    for t, ext in sorted({**extensions, **SPACE_EXTENSIONS}.items()):
        entries = [e for e in mapped if e["type"] == t]
        if not entries and "model" not in ext:
            continue
        subject = ext["baseModel"].split("/")[0]
        base = json.loads((SDM / ext["baseModel"].removeprefix("dataModel.") / "schema.json").read_text())
        context = json.loads((SDM / subject.removeprefix("dataModel.") / "context.jsonld").read_text())["@context"]
        known = declared(base, common)
        whole = {k: v for k, v in base.items() if k not in SDM_HEADER}
        whole = json.loads(json.dumps(whole).replace(json.dumps({"enum": [base_type(base)]})[1:-1], json.dumps({"enum": [t]})[1:-1]))
        described: dict[str, str] = {}
        emitted: dict[str, set[str]] = {}
        if "model" in ext:
            model = yaml.safe_load((SEED / ext["model"]).read_text())
            for a in model["classes"][t]["slots"]:
                slot = model["slots"][a]
                r = "string-list" if slot.get("multivalued") else slot.get("range", "string")
                if r in model["classes"] and a in known:
                    continue  # an object the base already declares (address): the base's shape holds (T-5429)
                emitted[a] = {r if r in SCHEMA_ORG else "string"}
                described[a] = " ".join(slot.get("description", "").split()) or f"The space model's `{a}`."
        columns: dict[str, list[str]] = {}
        for e in entries:
            for a, p in e["properties"].items():
                a = attribute(a)
                columns.setdefault(a, [])
                if p.get("template", p["column"]) not in columns[a]:
                    columns[a].append(p.get("template", p["column"]))
                emitted.setdefault(a, set()).add(space_ranges[e["org"]][a])
        for a, cols in columns.items():
            described[a] = "The source column " + ", ".join(f"`{c}`" for c in cols) + "."
        added: dict[str, dict] = {"type": {"type": "string", "enum": [t], "description": f"Property. NGSI Entity type. It has to be {t}"}}
        if "isPartOf" not in known and entries:
            added["isPartOf"] = {"type": "string", "format": "uri", "description": f"Property. Model:'https://schema.org/URL'. {COMMON['isPartOf'][1]}"}
        for a in sorted(described):
            if a in known:
                continue
            rs = emitted[a]
            types = sorted({JSON_TYPE.get(r, "string") for r in rs if r != "string-list"})
            prop = {"type": "array", "items": {"type": "string"}} if rs == {"string-list"} else {"type": types[0] if len(types) == 1 else types}
            if len(rs) == 1 and next(iter(rs)) in JSON_FORMAT:
                prop["format"] = JSON_FORMAT[next(iter(rs))]
            r = next(iter(rs)) if len(rs) == 1 else "string"
            prop["description"] = f"Property. Model:'https://schema.org/{SCHEMA_ORG[r]}'. {described[a]}"
            added[a] = prop
        schema = {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$schemaVersion": "0.1.0",
            "$id": f"{JC}sdm/extensions/{t}/schema.json",
            "modelTags": "joinedcontext extension",
            "title": f"Smart Data Models - {t}, an extension of {ext['baseModel']}",
            "description": f"{ext['description']} Extends {ext['baseModel']}: {ext['why']}.",
            "derivedFrom": base.get("$id"),
            "$comment": f"allOf: {ext['baseModel']}'s schema.json at commit {pins[subject]} with its type set to {t}, "
                        "then the attributes the extension adds (T-4336). Generated by scripts/open-data/generate.py.",
            "type": "object",
            "allOf": [whole, {"properties": added}],
        }
        out[EXTENSIONS / t / "schema.json"] = json.dumps(schema, indent=2, ensure_ascii=False) + "\n"
        source = f"https://raw.githubusercontent.com/smart-data-models/{subject}/{pins[subject]}/context.jsonld"
        written = {a: next(iter(rs)) if len(rs) == 1 else "float" if rs <= {"integer", "float"} else "string" for a, rs in emitted.items()}
        out[EXTENSIONS / t / "model.linkml.yaml"] = extension_linkml(t, ext, entries, described, written, (context, source), base.get("$id"))
    return out


def base_type(schema: dict) -> str:
    """The entity type a Smart Data Model schema pins (`type: {enum: [X]}`)."""
    found = re.search(r'"type": \{[^{}]*"enum": \["([^"]+)"\]', json.dumps(schema))
    if not found:
        sys.exit(f"{schema.get('$id')}: no type enum to extend")
    return found.group(1)


def extension_linkml(t: str, ext: dict, entries: list[dict], described: dict[str, str], model_ranges: dict[str, str],
                     base_context: tuple[dict, str], base_id: str) -> str:
    """The extension as LinkML: one class, the slots its datasets write, typed as the seed writes them; a
    term the base's @context names keeps the base's IRI, the source's own columns stay jc: (T-4328)."""
    common = {a: v for a, v in COMMON.items() if entries or a in described}
    slots: dict[str, dict] = {}
    for a, (r, d) in common.items():
        slots[a] = {"description": d, **({"range": r} if r != "string" else {}), "annotations": {"ngsi_ld_kind": "Property"}}
    for a in sorted(set(described) - set(common)):
        r = model_ranges[a]
        slots[a] = {"description": described[a],
                    **({"multivalued": True} if r == "string-list" else {"range": r} if r != "string" else {}),
                    "annotations": {"ngsi_ld_kind": "Property"}}
    if any(e.get("identifierFromId") for e in entries):
        slots.setdefault("identifier", {"description": "The entity's own key or row hash: the base requires an identifier no column holds.",
                                        "annotations": {"ngsi_ld_kind": "Property"}})
    context, source = base_context
    for a, slot in slots.items():
        if isinstance(context.get(a), str):
            slot["slot_uri"], slot["annotations"]["upstream_source"] = context[a], source
    doc = {
        "id": f"{JC}sdm/extensions/{t}", "name": t,
        "title": f"{t}, an extension of {ext['baseModel']}",
        "description": f"{ext['description']} Extends {ext['baseModel']}: {ext['why']}.",
        "license": "https://joinedcontext.com/licenses/cc-by-4.0",
        "prefixes": {"linkml": "https://w3id.org/linkml/", "jc": JC,
                     "sdm": "https://smartdatamodels.org/", "ngsi-ld": "https://uri.etsi.org/ngsi-ld/"},
        "default_prefix": "jc", "default_range": "string", "imports": ["linkml:types", "ngsi-ld-core"],
        "slots": slots,
        "classes": {t: {"class_uri": f"jc:{t}", "description": ext["description"], "is_a": "Entity",
                        "slots": [*common, *sorted(a for a in slots if a not in common)],
                        "annotations": {"upstream_source": base_id}}},
    }
    return manifest(f"The {t} extension of {ext['baseModel']} (T-4336). Generated by scripts/open-data/generate.py\n"
                    "from the mapping; do not edit by hand.", doc)


def area_rule(p: dict, entries: list[dict]) -> dict:
    """The space's place as an `area` rule on its first located type (T-4525): a row outside the
    project's box is flagged, never refused (DM-87). No located type, no rule."""
    kind = next((e["type"] for e in entries if located(e)), None)
    return {"quality": [{"name": "in-area", "entityType": kind, "kind": "area", "attribute": "location",
                         "bbox": p["area"]}]} if kind else {}


def catalog_block(project: str, p: dict, entries: list[dict]) -> dict:
    """What DCAT-AP-SK asks of the door's record (T-4596); the publisher and desk are the Project's.
    CC BY-SA 4.0 covers every mapped dataset of either project, the CC BY ones included; the news
    and the address register are read every night; no mapping reads a column that names a person."""
    licences = {e.get("license") for e in entries}
    assert licences <= {"cc-by", "cc-by-sa"}, f"{project}: a licence CC BY-SA does not cover: {licences}"
    return {"license": "CC_BYSA_4_0", "themes": ["GOVE", "REGI"], "frequency": "DAILY",
            "keywords": p["keywords"], "spatial": p["spatial"], "personalData": "does-not-contain"}


def located(entry: dict) -> bool:
    """Whether the rows of `entry` carry a `location`: a feature source with its geometry."""
    return entry["source"].get("kind") in FEATURES and bool(entry["source"].get("geometry"))


def series(entry: dict) -> bool:
    """Whether the rows of `entry` are measurements at an instant: an `observedAt` column and a number
    beside it, so SensorThings has a Datastream to answer (T-4288)."""
    observed = entry.get("observedAt")
    return bool(observed) and any(p["column"] != observed and p["range"] in ("integer", "float")
                                  for p in entry["properties"].values())


def distributions(mapped: list[dict], whole: list[dict]) -> str:
    """Per CKAN dataset name: its Endpoint's slug, its entity type, whether it has a location or a
    series and whether its addresses keep only its rows. A dataset an older seed reads whole is served by that
    seed's public Endpoint, over its whole type: those rows name no catalogue dataset (T-4291)."""
    rows, gone = {}, Counter()
    for e in mapped:
        endpoint = SEED / e["org"] / f"{e['org']}-od-endpoint.yaml"
        if e["source"]["kind"] == "endpoint":
            # No Pipeline fills it since T-5729: prod holds no row of it, so the catalogue lists no
            # distribution and the harvest withdraws the dataset (T-5788, owner 2026-10-26). Its old
            # rows can still sit on the door, so it still counts against `sole` below.
            gone[(slug_for(endpoint), e["type"])] += 1
            continue
        rows[e["dataset"].rsplit("/", 1)[1]] = {"endpoint": slug_for(endpoint), "type": e["type"],
                                                 "dataset": e["dataset"], "location": located(e), "series": "sta" in REPRESENTATIONS and series(e),
                                                 "filtered": True}
    for e in whole:
        slug, kind = existing_door(e)
        # A seed that shares its door and type with another dataset tells them apart by `isPartOf`
        # (bbsk obce and okresy, both AdministrativeArea: T-4523), so the catalogue filters on it.
        marked = f'"isPartOf": {{ "type": "Property", "value": "{e["dataset"]}" }}'
        filtered = any(marked in b.read_text() for b in (SEED / e["org"]).glob("*-bento.yaml"))
        rows[e["dataset"].rsplit("/", 1)[1]] = {"endpoint": slug, "type": kind, "dataset": e["dataset"],
                                                 "location": located(e), "series": False, "filtered": filtered}
    # A dataset whose type holds only its rows on its door is kept by the type alone (`sole`): a `q`
    # the broker checks row by row can neither be counted past 10 000 stored rows nor read in parallel
    # pages, so bb-objednavky-bb's 26 862 rows timed out in every file and its count was 403 (T-4540).
    doors = Counter((r["endpoint"], r["type"]) for r in rows.values()) + gone
    for r in rows.values():
        r["sole"] = r["filtered"] and doors[(r["endpoint"], r["type"])] == 1
    # The ŠÚ SR rows are scripts/open-data/datacube.py's (T-4390): kept as committed.
    rows.update({k: v for k, v in json.loads(DISTRIBUTIONS.read_text()).items() if k.startswith("susr-")})
    return "{\n" + ",\n".join(f"  {json.dumps(k)}: {json.dumps(v, ensure_ascii=False)}" for k, v in sorted(rows.items())) + "\n}\n"


def existing_door(entry: dict) -> tuple[str, str]:
    """The public Endpoint and entity type of the seed that reads `entry` whole: its DataSource's
    Pipeline, then the public Endpoint of that Pipeline's space whose catalogue names the Pipeline,
    else the one whose Policy grants the Pipeline's type, else the space's own (no Policy narrows it)."""
    source = yaml.safe_load((SEED / entry["existing"]["seed"]).read_text())
    project, name = source["metadata"]["namespace"], source["metadata"]["name"]
    docs = [d for f in sorted((SEED / project).glob("*.yaml")) for d in yaml.safe_load_all(f.read_text())
            if isinstance(d, dict) and (d.get("metadata") or {}).get("namespace") == project]
    pipelines = [d for d in docs if d["kind"] == "Pipeline"
                 and ((d["spec"].get("source") or {}).get("dataSourceRef") or {}).get("name") == name]
    if len(pipelines) != 1:
        sys.exit(f"{entry['name']}: {len(pipelines)} pipelines read {entry['existing']['seed']}, expected one")
    pipeline = pipelines[0]
    kind = pipeline["spec"]["output"]["type"]
    space = pipeline["spec"]["targetEndpoint"].split(":")[-2]
    public = [d for d in docs if d["kind"] == "Endpoint" and d["spec"].get("audience") == "public"
              and d["spec"].get("contextSpaceRef") == space]
    granting = {d["metadata"]["name"] for d in docs if d["kind"] == "Policy"
                and any(x.get("type") == kind for i in d["spec"].get("information") or [] for x in i.get("entities") or [])}
    for door in (
        [d for d in public if ((d["spec"].get("catalog") or {}).get("pipelineRef") or {}).get("name") == pipeline["metadata"]["name"]],
        [d for d in public if (d["spec"].get("policyRef") or "").rsplit(":", 1)[-1] in granting],
        [d for d in public if not d["spec"].get("policyRef")],
    ):
        if door:
            return door[0]["spec"]["slug"], kind
    sys.exit(f"{entry['name']}: no public Endpoint over {space} serves {kind}")


def main() -> None:
    files = render()
    if "--check" in sys.argv:
        stale = [p for p, text in files.items() if not p.exists() or p.read_text() != text]
        orphans = [p for project in PROJECTS for p in (SEED / project).glob(f"{project}-od-*") if p not in files]
        orphans += [d for d in EXTENSIONS.glob("*") if d / "schema.json" not in files]
        for p in stale + orphans:
            print(f"generate.py: {p.relative_to(ROOT)} {'is not what the mapping gives' if p in stale else 'is no longer generated'}; "
                  "run scripts/open-data/generate.py", file=sys.stderr)
        sys.exit(1 if stale or orphans else 0)
    for project in PROJECTS:
        for p in (SEED / project).glob(f"{project}-od-*"):
            if p not in files:
                p.unlink()
    for p, text in files.items():
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
    if "--artifacts" in sys.argv:
        python, tools = sys.argv[sys.argv.index("--artifacts") + 1:][:2]
        # A generator writes some lists (SHACL's ignoredProperties) in set order, which Python's
        # string hashing reshuffles per process; one seed keeps them in one order (T-4437).
        env = {**os.environ, "PYTHONHASHSEED": "0"}
        for project in PROJECTS:
            model = SEED / project / f"{project}-otvorene-data.linkml.yaml"
            stem = str(model).removesuffix(".linkml.yaml")
            # The four artifacts a published model commits (DM-02, T-4425).
            for gen, out in (("gen_json_schema.py", f"{stem}.v1.schema.json"), ("gen_context.py", f"{stem}.v1.context.jsonld"),
                             ("gen_docs.py", f"{stem}.v1.md"), ("gen_example.py", f"{stem}.v1.example.jsonld")):
                subprocess.run([python, f"{tools}/{gen}", str(model), "-o", out], check=True, env=env)
        for model in sorted(EXTENSIONS.glob("*/model.linkml.yaml")):
            d = model.parent
            (d / "examples").mkdir(exist_ok=True)
            for args, out in ((["gen_context.py"], "context.jsonld"), (["gen_rdf_artifacts.py", "--artifact", "shacl"], "model.shacl.ttl"),
                              (["gen_rdf_artifacts.py", "--artifact", "owl"], "model.owl.ttl"), (["gen_docs.py"], "model.md"),
                              (["gen_example.py"], "examples/example.json")):
                subprocess.run([python, f"{tools}/{args[0]}", str(model), *args[1:], "-o", str(d / out)], check=True, env=env)
            # RDF: the model itself as triples, LinkML's own serialization of it with model-tools' imports merged.
            subprocess.run([python, "-c", RDF, tools, str(model), str(d / "model.rdf.ttl")], check=True, env=env)
            subprocess.run([python, "-c", CANONICAL, *(str(d / f"model.{a}.ttl") for a in ("shacl", "owl", "rdf"))], check=True, env=env)
            example = json.loads((d / "examples/example.json").read_text())
            # model-tools fills a text slot with its own name; a uri slot needs a URI, or a validator
            # that checks formats (jsonschema with rfc3987) refuses the example (T-4338).
            for a, slot in yaml.safe_load(model.read_text())["slots"].items():
                if slot.get("range") == "uri" and example.get(a) == a:
                    example[a] = f"https://example.org/{a}"
            (d / "examples/example.json").write_text(json.dumps(example, indent=2, ensure_ascii=False) + "\n")
            (d / "examples/example-normalized.jsonld").write_text(json.dumps(normalized(example, d.name), indent=2, ensure_ascii=False) + "\n")
    print(f"{len(files)} files written")


RDF = """\
import re
import sys
sys.path.insert(0, sys.argv[1])
from common import load
from linkml.generators.rdfgen import RDFGenerator
from linkml_runtime.dumpers import yaml_dumper
view = load(sys.argv[2])
view.merge_imports()
text = RDFGenerator(yaml_dumper.dumps(view.schema)).serialize()
# The generation time is the only line that differs between two runs of the same model.
text = re.sub(r"\\n\\s*linkml:generation_date [^\\n]*", "", text)
open(sys.argv[3], "w", encoding="utf-8").write(text)
"""


# The LinkML generators write the same graph with its blank nodes and restrictions in a new order
# each run (T-4437): one canonical Turtle per graph, so a regeneration changes a file only when
# the model changed. Blank nodes get labels from their content, and rdflib sorts what it writes.
CANONICAL = """\
import sys
from rdflib import Graph
from rdflib.compare import to_canonical_graph
for path in sys.argv[1:]:
    graph = Graph().parse(path, format="turtle")
    out = Graph(bind_namespaces="none")
    for prefix, namespace in sorted(graph.namespaces()):
        out.bind(prefix, namespace, override=True, replace=True)
    for triple in to_canonical_graph(graph):
        out.add(triple)
    open(path, "w", encoding="utf-8").write(out.serialize(format="turtle"))
"""


def normalized(key_values: dict, t: str) -> dict:
    """The key-value example in the normalized form the broker holds, with the extension's @context."""
    out = {"id": key_values["id"], "type": key_values["type"]}
    for a, v in key_values.items():
        if a in ("id", "type", "@context"):
            continue
        out[a] = {"type": "GeoProperty", "value": v} if a == "location" else {"type": "Property", "value": v}
    out["@context"] = [f"{EXTENSION_URL}/{t}/context.jsonld", "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.8.jsonld"]
    return out


if __name__ == "__main__":
    main()
