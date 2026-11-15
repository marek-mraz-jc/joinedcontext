#!/usr/bin/env python3
"""Writes the mapping of the `ovzdusie` pipeline of project `slovensko` (T-4340): the hourly air
quality of every Slovak station the EEA download service publishes, one AirQualityObserved per
station.

Inputs, both pinned here and read on 2026-10-16:
- files.txt: what `POST https://eeadmz1-downloads-api-appservice.azurewebsites.net/ParquetFile/urls`
  answered for countries SK, pollutants PM10, PM2.5, NO2 and O3, dataset 1 (up-to-date, E2a):
  one Parquet file per sampling point.
- stations.tsv: each station's coordinates, altitude, type and area from the EEA station metadata
  (https://discomap.eea.europa.eu/map/fme/metadata/PanEuropean_metadata.csv), and the municipality
  its point lies in (OpenStreetMap Nominatim reverse geocoding, zoom 14).

A file whose station has no row in stations.tsv, or whose pollutant is none of the four, is left
out with the reason on stderr: an entity without a location would be no station on a map.

Usage: generate.py [--check]
"""

import csv
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUT = ROOT / "components/context-gateway/seed/slovensko/slovensko-pipeline-ovzdusie-bento.yaml"

# EEA pollutant vocabulary (http://dd.eionet.europa.eu/vocabulary/aq/pollutant/{code}) -> SDM attribute.
POLLUTANTS = {5: "pm10", 6001: "pm25", 8: "no2", 7: "o3"}
FILE = re.compile(r"^https://eeadmz1batchservice02\.blob\.core\.windows\.net/airquality-p/SK/SPO-(SK\d{4}[A-Z])_(\d+)_\d+\.parquet$")
TYPE = {"background": ("pozaďová", "background"), "traffic": ("dopravná", "traffic"), "industrial": ("priemyselná", "industrial")}
SLUG = "wtvuppkop6y3qv7bnvzqwifv4uts4e2k"  # the ovzdusie-sk Endpoint
BACKFILL_DAYS = 7
AREA = {"urban": ("mestská", "urban"), "suburban": ("predmestská", "suburban")}


def stations(path: Path) -> dict:
    out = {}
    with path.open(encoding="utf-8") as f:
        for row in csv.DictReader(f, delimiter="\t"):
            kind_sk, kind_en = TYPE[row["type"]]
            area_sk, area_en = AREA.get(row["area"], ("vidiecka", "rural"))
            town = row["municipality"]
            out[row["station"]] = {
                "sk": f"{town}, stanica {row['station']} ({area_sk} {kind_sk})",
                "en": f"{town}, station {row['station']} ({area_en} {kind_en})",
                "town": town,
                "point": [round(float(row["longitude"]), 6), round(float(row["latitude"]), 6)],
            }
    return out


def files(lines: list[str], known: dict) -> list[dict]:
    out = []
    for url in (line.strip() for line in lines):
        if not url:
            continue
        m = FILE.match(url)
        if not m:
            print(f"generate.py: left out {url}: not an EEA SK sampling-point file", file=sys.stderr)
            continue
        station, code = m.group(1), int(m.group(2))
        if code not in POLLUTANTS:
            print(f"generate.py: left out {url}: pollutant {code} is none of PM10, PM2.5, NO2, O3", file=sys.stderr)
        elif station not in known:
            print(f"generate.py: left out {url}: station {station} has no location in stations.tsv", file=sys.stderr)
        else:
            out.append({"url": url, "station": station, "attribute": POLLUTANTS[code]})
    return out


# Parquet INT96: [nanoseconds of the day, low and high 32 bits, Julian day]. Start and End carry no
# zone; they are UTC+1 (CET, no summer time) for every country, the zone the EEA download service
# writes (checked 2026-10-16, T-4340: SK0001A's file, modified 11:58:45Z, held a row with ResultTime
# 12:25:42, and FI00273's, modified 11:51:56Z, one with 12:30:02, though the metadata gives SK as UTC
# and FI as UTC+2; the service's former CSV printed "+01:00"). Validity 1, 2 and 3 are valid readings
# and 4 is ozone's "valid, CCQM.O3.2019"; -1 and -99 are not (EEA vocabulary "observationvalidity").
ROWS = """\
map unix {
  root = (this.index(2) - 2440588) * 86400 + ((this.index(1) * 4294967296 + this.index(0)) / 1000000000).floor() - 3600
}
let rows = content().parse_json().or([]).filter(r -> r.End.type() == "array" && r.End.length() == 3).map_each(r -> r.merge({ "unix": r.End.apply("unix") })).filter(r -> r.unix > @since)
let valid = $rows.filter(r -> [1, 2, 3, 4].contains(r.Validity) && r.Value.number().catch(-1) >= 0)
meta skipped = $rows.length() - $valid.length()
root = { "ok": true, "records": $valid.sort_by(r -> r.unix).map_each(r -> { "station": @station, "attribute": @attribute, "value": r.Value.number(), "unix": r.unix }) }"""


def indent(text: str, by: int) -> str:
    return "\n".join((" " * by + line) if line else line for line in text.splitlines())


def render(table: dict, todo: list[dict], slug: str = SLUG) -> str:
    used = {f["station"] for f in todo}
    table = {k: {"sk": v["sk"], "point": v["point"]} for k, v in sorted(table.items()) if k in used}
    since = """\
let floor = @jc_now.or(now()).ts_parse("2006-01-02T15:04:05Z07:00").ts_unix() - %d
let stored = this.or([]).fold({}, item -> item.tally.merge({ (item.value.id.split(":").index(-1)): %s.fold({}, a -> a.tally.merge(if item.value.get(a.value).observedAt.or(null) == null { {} } else { { (a.value): item.value.get(a.value).observedAt.ts_parse("2006-01-02T15:04:05Z07:00").ts_unix() } })) }))
root = %s.map_each(f -> f.merge({ "since": [$floor, $stored.get(f.station).get(f.attribute).or(0)].max() }))""" % (
        BACKFILL_DAYS * 86400, json.dumps(sorted(POLLUTANTS.values())), json.dumps(todo, ensure_ascii=False))
    # The publisher's domain (the platform, the Endpoint's publisher), never the runner's
    # JC_ORG_DOMAIN, which is the installation's (T-4202, T-4373).
    entity = """\
let stations = %s
let files = this.filter(m -> m.ok.or(false))
root = if $files.length() == 0 { throw("no station file came") } else {
  $files.map_each(m -> m.records).flatten().fold({}, item -> item.tally.merge({ (item.value.station): { ("%%v".format(item.value.unix)): { (item.value.attribute): item.value } } })).key_values().sort_by(s -> s.key).map_each(s -> s.value.key_values().sort_by(h -> h.key.number()).map_each(h -> {
    "id": "urn:ngsi-ld:AirQualityObserved:%%v:%%v:%%v".format("joinedcontext.com", env("JC_SPACE"), s.key),
    "type": "AirQualityObserved",
    "name": { "type": "Property", "value": $stations.get(s.key).sk },
    "dateObserved": { "type": "Property", "value": h.key.number().ts_format("2006-01-02T15:04:05Z07:00", "UTC") },
    "location": { "type": "GeoProperty", "value": { "type": "Point", "coordinates": $stations.get(s.key).point } }
  }.merge(h.value.map_each(a -> { "type": "Property", "value": a.value.value, "unitCode": "GQ", "observedAt": a.value.unix.ts_format("2006-01-02T15:04:05Z07:00", "UTC") })))).flatten().sort_by(e -> e.dateObserved.value + " " + e.id)
}""" % json.dumps(table, ensure_ascii=False, sort_keys=True)
    return f"""\
# The mapping of the ovzdusie pipeline of project slovensko (T-4340): the hourly means of PM10,
# PM2.5, NO2 and O3 at every Slovak station the EEA download service publishes, one AirQualityObserved
# per station. Do not edit: change scripts/air-sk/ and run scripts/air-sk/generate.py.
#
# Each run first reads, from the space's own public Endpoint, the latest hour it holds of each
# station and pollutant, and then writes every valid hour after it, oldest first, back to at most
# {BACKFILL_DAYS} days: the first run fills the week's history, every later one adds the new hour, and a
# restart writes nothing twice. The hours go out hour by hour, every station's in each (T-4410): the
# broker applies the repeats of one id in a batch as sequential rounds, and station by station a
# 1000-entity batch was ~168 rounds, which the 30 s write timeout cut 2-3 days into the week. A failed read of the Endpoint fails the run before anything is
# written. The files are read one at a time (for_each), so the runner holds one decoded file, not
# {len(todo)}. A file that does not come is left out with a log line, so one retired sampling point
# does not stop the other stations; a run in which no file comes fails where it can be seen. The name is
# Smart Data Models' plain text, in Slovak.

pipeline:
  processors:
    - mapping: root = ""
    - http:
        url: "${{JC_GATEWAY_URL}}/api/endpoint/{slug}/ngsi-ld/v1/entities?type=AirQualityObserved&attrs={','.join(sorted(POLLUTANTS.values()))}&limit=1000"
        verb: GET
        headers:
          Accept: application/json
        timeout: 30s
        retries: 2
    - mapping: |
        root = if errored() {{ throw("the stored stations could not be read: %v".format(error())) }} else {{ this }}
    # Nothing is fetched or written after a failed read: the errored message alone goes on.
    - switch:
        - check: '!errored()'
          processors:
          - mapping: |
{indent(since, 14)}
          - unarchive:
              format: json_array
          - for_each:
              - mapping: |
                  meta url = this.url
                  meta path = this.url.trim_prefix("https://eeadmz1batchservice02.blob.core.windows.net/")
                  meta station = this.station
                  meta attribute = this.attribute
                  meta since = this.since
                  root = ""
              - http:
                  # PL-07: the host is fixed in the step, only the file's path comes from the message.
                  url: "https://eeadmz1batchservice02.blob.core.windows.net/${{! @path }}"
                  verb: GET
                  headers:
                    Accept: application/octet-stream
                    User-Agent: "joinedcontext-platform open-data pipeline (https://opendata.dev.joinedcontext.com)"
                  timeout: 60s
                  retries: 2
                  rate_limit: pipeline_egress
              - catch:
                  - log:
                      level: WARN
                      message: 'left out ${{! @url }}: ${{! error() }}'
                  - mapping: |
                      meta failed = true
                      root = {{ "ok": false }}
              - switch:
                  - check: '@failed.or(false) == false'
                    processors:
                      - parquet_decode: {{}}
                      - archive:
                          format: json_array
                      - mapping: |
{indent(ROWS, 26)}
                      - switch:
                          - check: '@skipped > 0'
                            processors:
                              - log:
                                  level: INFO
                                  message: '${{! @station }} ${{! @attribute }}: ${{! @skipped }} rows skipped (invalid, negative or missing value)'
          - archive:
              format: json_array
          - mapping: |
{indent(entity, 14)}
          - unarchive:
              format: json_array
"""


def main() -> int:
    known = stations(HERE / "stations.tsv")
    text = render(known, files((HERE / "files.txt").read_text(encoding="utf-8").splitlines(), known))
    if "--check" in sys.argv[1:]:
        if not OUT.exists() or OUT.read_text(encoding="utf-8") != text:
            print(f"generate.py: {OUT.relative_to(ROOT)} is not what scripts/air-sk gives; run scripts/air-sk/generate.py", file=sys.stderr)
            return 1
        return 0
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
