#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""The public status page, status.{domain} (T-5102).

Once a minute it reads the synthetic probe's /results.json (T-5103) inside the cluster, keeps the
newest state of this environment's public surfaces, adds the minute to a 90-day history on its
volume and renders the page in Slovak and English with the incidents: the deployer's notes and the
ones the page opens itself when a surface stays down or degraded (T-5184):

    GET /            the page in the visitor's language (Accept-Language, Slovak by default)
    GET /sk/ /en/    the page in one language
    GET /status.css  its stylesheet
    GET /status.json the same states for machines: surface, state, uptime over the measured days, incidents
    GET /healthz     200 while a page was rendered in the last three minutes

A surface is up when every probed Endpoint holds, degraded when some fail, down when all fail and
unknown when the probe reported nothing for it in STALE_SECONDS; current() is that one rule, and the
page and status.json render the same result of it. A surface down or degraded for AUTO_OPEN_MINUTES
opens an automatic incident, AUTO_CLOSE_MINUTES up close it; its text comes from fixed templates.
The page shows states, counts and the Endpoints' public catalogue names only: never a probe reason,
a response body, a host or an address. Read only, no login, GET and HEAD.

STATUS_TARGET    the probe target that is this environment (the first label of its domain)
STATUS_DOMAIN    its public domain, shown in the heading
STATUS_PROBE_URL the probe's in-cluster results URL (http://probe.<ns>.svc.cluster.local:9100/results.json)
STATUS_INCIDENTS the incident notes as JSON (files/incidents.yaml, converted at render)
STATUS_DATA      the volume holding history.json and watch.json (the automatic incidents)
"""

import datetime as dt
import html
import json
import os
import re
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DAYS = 90
STALE_SECONDS = 600
AUTO_OPEN_MINUTES = 3
AUTO_CLOSE_MINUTES = 10
# A daily backup that missed a night, and a monthly restore drill that missed a month (T-5289).
BACKUP_STALE_SECONDS = 26 * 3600
DRILL_STALE_SECONDS = 35 * 86400
DRILL_RESULTS = ("passed", "failed", "skipped", "no-backup")
RESULTS_CAP = 4 * 1024 * 1024
SLUG = re.compile(r"^[a-z0-9]{1,64}$")
INCIDENT_ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
STATES = ("up", "degraded", "down", "unknown")
# Shown in this order; the surface names of probe.py.
SURFACES = {
    "catalogue": {"sk": "Katalóg otvorených dát", "en": "Open data catalogue"},
    "portal": {"sk": "Portál", "en": "Portal"},
    "ngsi-ld": {"sk": "NGSI-LD API", "en": "NGSI-LD API"},
    "odata": {"sk": "OData", "en": "OData"},
    "ogc": {"sk": "OGC API Features", "en": "OGC API Features"},
    "sta": {"sk": "SensorThings API", "en": "SensorThings API"},
    "websocket": {"sk": "WebSocket (živé zmeny)", "en": "WebSocket (live changes)"},
    "mcp": {"sk": "MCP pre asistentov AI", "en": "MCP for AI assistants"},
    "assistant": {"sk": "Asistent", "en": "Assistant"},
    "csv": {"sk": "Súbory CSV", "en": "CSV files"},
    "ical": {"sk": "Kalendáre iCal", "en": "iCal calendars"},
}
TEXT = {
    "sk": {
        "title": "Stav platformy",
        "lede": "Každú minútu skúšame verejné rozhrania zvonku, tak ako ich používa návštevník. Stránka sa obnovuje sama.",
        "up": "V prevádzke", "degraded": "Čiastočne obmedzené", "down": "Mimo prevádzky", "unknown": "Bez údajov",
        "all_up": "Všetko je v prevádzke", "some_degraded": "Niektoré rozhrania sú obmedzené",
        "some_down": "Niektoré rozhrania sú mimo prevádzky", "no_data": "Sonda zatiaľ neposlala výsledky",
        "checked": "Naposledy overené {time} UTC",
        "surfaces": "Rozhrania",
        "uptime": "{pct} % dostupnosť za {days}",
        "uptime_none": "Zatiaľ bez meraní",
        "endpoints": "{ok} z {total} zdrojov odpovedá správne",
        "failing": "Neodpovedajú: {names}",
        "more": "a {n} ďalších",
        "day_none": "{date}: bez meraní", "day": "{date}: {pct} % dostupnosť",
        "days_ago": "pred {days} dňami", "today": "dnes",
        "ongoing": "Prebiehajúce udalosti", "past": "Udalosti za posledných {days} dní", "no_incidents": "Za posledných {days} dní žiadne hlásené udalosti.",
        "since": "od {start} UTC", "between": "{start} – {end} UTC", "affects": "Týka sa: {names}",
        "legend": "Farba dňa: zelená bez výpadku, žltá dostupnosť aspoň 99 %, červená menej ako 99 %, sivá bez meraní.",
        "other": "English", "machine": "Strojovo čitateľné: status.json",
        "backups": "Zálohy databázy",
        "backups_lede": "Databáza sa zálohuje každú noc a raz za mesiac skúšame zálohu obnoviť a porovnať.",
        "backup": "Posledná záloha", "drill": "Posledná skúška obnovy",
        "fresh": "Aktuálna", "stale": "Zastaraná",
        "age_hours": "pred {n} h", "age_days": "pred {n} dňami",
        "never": "Zatiaľ žiadna",
        "result_passed": "obnova a porovnanie riadkov prešli", "result_failed": "obnova zlyhala",
        "result_skipped": "preskočená pre nedostatok pamäte", "result_no-backup": "nebolo čo obnoviť",
    },
    "en": {
        "title": "Platform status",
        "lede": "Every minute we try the public interfaces from outside, the way a visitor uses them. The page refreshes itself.",
        "up": "Operational", "degraded": "Degraded", "down": "Down", "unknown": "No data",
        "all_up": "All systems operational", "some_degraded": "Some interfaces are degraded",
        "some_down": "Some interfaces are down", "no_data": "The probe has not reported yet",
        "checked": "Last checked {time} UTC",
        "surfaces": "Interfaces",
        "uptime": "{pct} % uptime over {days}",
        "uptime_none": "No measurements yet",
        "endpoints": "{ok} of {total} sources answer correctly",
        "failing": "Not answering: {names}",
        "more": "and {n} more",
        "day_none": "{date}: no measurements", "day": "{date}: {pct} % uptime",
        "days_ago": "{days} days ago", "today": "today",
        "ongoing": "Ongoing incidents", "past": "Incidents in the last {days} days", "no_incidents": "No incidents reported in the last {days} days.",
        "since": "since {start} UTC", "between": "{start} – {end} UTC", "affects": "Affects: {names}",
        "legend": "Day colour: green no outage, yellow at least 99 % uptime, red below 99 %, grey no measurements.",
        "other": "Slovensky", "machine": "Machine readable: status.json",
        "backups": "Database backups",
        "backups_lede": "The database is backed up every night, and once a month we restore a backup and compare it.",
        "backup": "Last backup", "drill": "Last restore drill",
        "fresh": "Current", "stale": "Stale",
        "age_hours": "{n} h ago", "age_days": "{n} days ago",
        "never": "None yet",
        "result_passed": "restore and row counts passed", "result_failed": "the restore failed",
        "result_skipped": "skipped, the node was short of memory", "result_no-backup": "there was nothing to restore",
    },
}
# The automatic incidents' only public text (T-5184): no host, address, reason or person.
AUTO_TEXT = {
    "title": {
        "down": {"sk": "Rozhrania mimo prevádzky", "en": "Interfaces down"},
        "degraded": {"sk": "Rozhrania čiastočne obmedzené", "en": "Interfaces degraded"},
    },
    "note": {
        "open": {"sk": "Sonda zistila problém sama a otvorila tento záznam automaticky. Prešetrujeme ho.",
                 "en": "The probe detected the problem and opened this entry automatically. We are looking into it."},
        "closed": {"sk": "Sonda zistila problém sama; po 10 minútach bez chyby záznam automaticky uzavrela.",
                   "en": "The probe detected the problem; after 10 minutes without an error it closed this entry automatically."},
    },
}
CACHE = "public, max-age=30, stale-while-revalidate=30, stale-if-error=86400"
CSP = "default-src 'none'; style-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"


# ---- states


def current(results, target, now):
    """{surface: {state, ok, total, failing}} of `target` from the probe's results.json rows."""
    out = {s: {"state": "unknown", "ok": 0, "total": 0, "failing": []} for s in SURFACES}
    for row in results if isinstance(results, list) else []:
        if not isinstance(row, dict) or row.get("target") != target or row.get("surface") not in SURFACES:
            continue
        at = row.get("at")
        if not isinstance(at, (int, float)) or now - at > STALE_SECONDS:
            continue
        cell = out[row["surface"]]
        cell["total"] += 1
        if row.get("ok") is True:
            cell["ok"] += 1
        elif isinstance(row.get("endpoint"), str) and SLUG.match(row["endpoint"]):
            cell["failing"].append(row["endpoint"])
    for cell in out.values():
        cell["failing"].sort()
        if cell["total"]:
            cell["state"] = "up" if cell["ok"] == cell["total"] else "degraded" if cell["ok"] else "down"
    return out


def backup_cells(backups, now):
    """The backup and drill rows (T-5289), or None when the probe reads no cluster's backups.

    Each is fresh while younger than its limit (26 h, 35 days), and a drill only when it passed."""
    if not isinstance(backups, dict):
        return None
    cells = {}
    for key, limit in (("backup", BACKUP_STALE_SECONDS), ("drill", DRILL_STALE_SECONDS)):
        at = backups.get(f"{key}At")
        at = at if isinstance(at, (int, float)) and not isinstance(at, bool) else None
        fresh = at is not None and now - at <= limit
        result = None
        if key == "drill":
            result = backups.get("drillResult") if backups.get("drillResult") in DRILL_RESULTS else None
            fresh = fresh and result == "passed"
        cells[key] = {"at": at, "state": "fresh" if fresh else "stale", "result": result}
    return cells


def age_text(lang, seconds):
    hours = max(0, int(seconds // 3600))
    return TEXT[lang]["age_hours"].format(n=hours) if hours < 48 else TEXT[lang]["age_days"].format(n=hours // 24)


def overall(states):
    seen = {c["state"] for c in states.values()}
    if seen == {"unknown"}:
        return "no_data"
    return "some_down" if "down" in seen else "some_degraded" if "degraded" in seen else "all_up"


# ---- history: surface -> UTC date -> [up, degraded, down] minutes


def load_history(text):
    """The stored history, keeping only well-formed cells: a damaged file costs history, never the page."""
    try:
        doc = json.loads(text)
    except ValueError:
        return {}
    out = {}
    for surface, days in (doc.items() if isinstance(doc, dict) else []):
        if surface not in SURFACES or not isinstance(days, dict):
            continue
        for day, counts in days.items():
            try:
                dt.date.fromisoformat(day)
            except (TypeError, ValueError):
                continue
            if isinstance(counts, list) and len(counts) == 3 and all(isinstance(n, int) and n >= 0 for n in counts):
                out.setdefault(surface, {})[day] = counts
    return out


def add_minute(history, states, now):
    """Count this minute per surface and drop days older than DAYS; unknown minutes count nowhere."""
    today = dt.datetime.fromtimestamp(now, dt.UTC).date()
    oldest = (today - dt.timedelta(days=DAYS - 1)).isoformat()
    for surface, cell in states.items():
        if cell["state"] in ("up", "degraded", "down"):
            counts = history.setdefault(surface, {}).setdefault(today.isoformat(), [0, 0, 0])
            counts[("up", "degraded", "down").index(cell["state"])] += 1
    for days in history.values():
        for day in [d for d in days if d < oldest]:
            del days[day]
    return history


def uptime(counts):
    """Share of measured minutes that were not down, in percent; None without a measurement."""
    up, degraded, down = counts
    total = up + degraded + down
    return None if total == 0 else 100.0 * (up + degraded) / total


def days_of(history, surface, now):
    today = dt.datetime.fromtimestamp(now, dt.UTC).date()
    days = history.get(surface, {})
    return [(d.isoformat(), days.get(d.isoformat(), [0, 0, 0])) for d in (today - dt.timedelta(days=n) for n in range(DAYS - 1, -1, -1))]


def measured_days(history, surface, now):
    """Days from the surface's first measured minute to today, at most DAYS; 0 before any."""
    measured = [d for d, counts in history.get(surface, {}).items() if sum(counts)]
    if not measured:
        return 0
    today = dt.datetime.fromtimestamp(now, dt.UTC).date()
    return max(1, min(DAYS, (today - dt.date.fromisoformat(min(measured))).days + 1))


def days_text(lang, n):
    if lang == "en":
        return "1 day" if n == 1 else f"{n} days"
    return "1 deň" if n == 1 else f"{n} dni" if 2 <= n <= 4 else f"{n} dní"


def total_uptime(history, surface):
    sums = [sum(c[i] for c in history.get(surface, {}).values()) for i in range(3)]
    return uptime(sums)


def pct(value):
    """99.95 for the page: two decimals, never a rounded-up 100 for a day with an outage."""
    floor = int(value * 100) / 100
    return f"{floor:.2f}".rstrip("0").rstrip(".")


# ---- incidents


def _instant(value, what):
    if not isinstance(value, str):
        raise ValueError(f"{what} must be an ISO 8601 time in UTC such as 2026-10-21T20:15:00Z")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as err:
        raise ValueError(f"{what} {value!r} is no ISO 8601 time such as 2026-10-21T20:15:00Z") from err
    if parsed.tzinfo is None:
        raise ValueError(f"{what} {value!r} needs a zone, write it in UTC with Z")
    return parsed.timestamp()


def _texts(value, what):
    if not isinstance(value, dict) or set(value) != {"sk", "en"}:
        raise ValueError(f"{what} needs exactly an sk and an en text")
    for lang, text in value.items():
        if not isinstance(text, str) or not text.strip() or len(text) > 2000:
            raise ValueError(f"{what}.{lang} must be 1..2000 characters")
    return value


def load_incidents(text, environment):
    """The deployer's notes (files/incidents.yaml as JSON) for this environment, newest first.

    A malformed note stops the pod at start with the field to fix, as a bad probe target does;
    the render test holds the committed file to the same rules.
    """
    doc = json.loads(text) if text else {}
    items = (doc or {}).get("incidents") if isinstance(doc, dict) else None
    if items is None:
        items = []
    if not isinstance(items, list):
        raise ValueError("incidents must be a list")
    out, ids = [], set()
    for i, item in enumerate(items):
        where = f"incidents[{i}]"
        if not isinstance(item, dict) or set(item) - {"id", "environments", "start", "end", "surfaces", "title", "note"}:
            raise ValueError(f"{where}: keys are id, environments, start, end, surfaces, title, note")
        if not INCIDENT_ID.match(str(item.get("id", ""))) or item["id"] in ids:
            raise ValueError(f"{where}.id must be a unique lower-case name such as 2026-10-21-odata-timeouts")
        ids.add(item["id"])
        envs = item.get("environments")
        if not isinstance(envs, list) or not envs or not all(isinstance(e, str) for e in envs):
            raise ValueError(f"{where}.environments must list the environments it concerns, such as [dev]")
        surfaces = item.get("surfaces")
        if not isinstance(surfaces, list) or not surfaces or not set(surfaces) <= set(SURFACES):
            raise ValueError(f"{where}.surfaces must list surfaces of {', '.join(SURFACES)}")
        start = _instant(item.get("start"), f"{where}.start")
        end = _instant(item["end"], f"{where}.end") if item.get("end") is not None else None
        if end is not None and end < start:
            raise ValueError(f"{where}.end lies before its start")
        entry = {"id": item["id"], "start": start, "end": end, "surfaces": [s for s in SURFACES if s in surfaces],
                 "title": _texts(item.get("title"), f"{where}.title"), "note": _texts(item.get("note"), f"{where}.note")}
        if environment in envs:
            out.append(entry)
    return sorted(out, key=lambda e: e["start"], reverse=True)


def split_incidents(incidents, now):
    ongoing = [i for i in incidents if i["start"] <= now and (i["end"] is None or i["end"] > now)]
    past = [i for i in incidents if i["end"] is not None and i["end"] <= now and now - i["end"] <= DAYS * 86400]
    return ongoing, past


# ---- automatic incidents: {runs: surface -> [bad, up] consecutive minutes, open: incident | None, closed: [...]}


def new_watch():
    return {"runs": {}, "open": None, "closed": []}


def _auto_entry(item):
    if not isinstance(item, dict) or not INCIDENT_ID.match(str(item.get("id", ""))) or item.get("worst") not in AUTO_TEXT["title"]:
        raise ValueError("incident")
    surfaces = item.get("surfaces")
    if not isinstance(surfaces, list) or not surfaces or not set(surfaces) <= set(SURFACES):
        raise ValueError("surfaces")
    times = [item.get("start"), item.get("lastBad")] + ([item["end"]] if "end" in item else [])
    if not all(isinstance(t, (int, float)) for t in times):
        raise ValueError("time")
    return {k: item[k] for k in ("id", "start", "lastBad", "worst", "surfaces", "end") if k in item}


def load_watch(text):
    """The stored automatic incidents; a damaged file starts a new watch, never stops the page."""
    try:
        doc = json.loads(text)
        runs = {s: [int(r[0]), int(r[1])] for s, r in doc["runs"].items() if s in SURFACES and len(r) == 2}
        return {"runs": runs, "open": None if doc["open"] is None else _auto_entry(doc["open"]),
                "closed": [_auto_entry(i) for i in doc["closed"] if "end" in i]}
    except (ValueError, TypeError, KeyError, AttributeError, IndexError):
        return new_watch()


def watch_minute(watch, states, now):
    """Advance the automatic incidents by one minute of `states`. An unknown minute changes no run:
    no answer from the probe proves neither an outage nor a recovery."""
    runs = watch["runs"]
    for surface, cell in states.items():
        bad, good = runs.get(surface, [0, 0])
        if cell["state"] in ("down", "degraded"):
            runs[surface] = [bad + 1, 0]
        elif cell["state"] == "up":
            runs[surface] = [0, good + 1]
    failing = [s for s in SURFACES if runs.get(s, [0, 0])[0] >= AUTO_OPEN_MINUTES]
    if failing and watch["open"] is None:
        start = min(now - 60 * (runs[s][0] - 1) for s in failing)
        stamp = dt.datetime.fromtimestamp(start, dt.UTC).strftime("%Y%m%d-%H%M")
        watch["open"] = {"id": f"auto-{stamp}", "start": start, "lastBad": now, "worst": "degraded", "surfaces": []}
    incident = watch["open"]
    if incident is not None:
        incident["surfaces"] = [s for s in SURFACES if s in incident["surfaces"] or s in failing]
        bad_now = [s for s in incident["surfaces"] if states.get(s, {}).get("state") in ("down", "degraded")]
        if bad_now:
            incident["lastBad"] = now
        if any(states[s]["state"] == "down" for s in bad_now):
            incident["worst"] = "down"
        if all(runs.get(s, [0, 0])[1] >= AUTO_CLOSE_MINUTES for s in incident["surfaces"]):
            watch["closed"].insert(0, {**incident, "end": incident["lastBad"] + 60})
            watch["open"] = None
    watch["closed"] = [i for i in watch["closed"] if now - i["end"] <= DAYS * 86400]
    return watch


def incidents_shown(notes, watch):
    """The deployer's notes and the automatic incidents, newest first. A note that covers an
    automatic incident's surfaces at an overlapping time replaces its text."""
    auto = [{"id": i["id"], "start": i["start"], "end": i.get("end"), "surfaces": i["surfaces"],
             "title": AUTO_TEXT["title"][i["worst"]], "note": AUTO_TEXT["note"]["closed" if "end" in i else "open"],
             "automatic": True} for i in ([watch["open"]] if watch["open"] else []) + watch["closed"]]

    def covered(a):
        return any(set(a["surfaces"]) <= set(n["surfaces"]) and n["start"] <= (a["end"] or float("inf"))
                   and a["start"] <= (n["end"] or float("inf")) for n in notes)

    shown = [{**n, "automatic": False} for n in notes] + [a for a in auto if not covered(a)]
    return sorted(shown, key=lambda i: i["start"], reverse=True)


# ---- rendering


def _utc(ts):
    return dt.datetime.fromtimestamp(ts, dt.UTC).strftime("%Y-%m-%d %H:%M")


def _day_class(counts):
    value = uptime(counts)
    if value is None:
        return "none"
    return "up" if counts[2] == 0 else "degraded" if value >= 99 else "down"


def render(lang, domain, states, history, incidents, now, backups=None):
    t, e = TEXT[lang], html.escape
    other = "en" if lang == "sk" else "sk"
    head = overall(states)
    banner = {"all_up": "up", "some_degraded": "degraded", "some_down": "down", "no_data": "unknown"}[head]
    out = [
        "<!doctype html>",
        f'<html lang="{lang}">',
        "<head>",
        '<meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width, initial-scale=1">',
        '<meta http-equiv="refresh" content="60">',
        '<meta name="color-scheme" content="light dark">',
        f"<title>{e(t['title'])} · {e(domain)}</title>",
        '<link rel="stylesheet" href="/status.css">',
        f'<link rel="alternate" hreflang="{other}" href="/{other}/">',
        "</head>",
        "<body>",
        '<main class="wrap">',
        '<header class="top">',
        f"<div><h1>{e(t['title'])}</h1><p class=\"domain\">{e(domain)}</p></div>",
        f'<a class="lang" href="/{other}/" hreflang="{other}" lang="{other}">{e(t["other"])}</a>',
        "</header>",
        f'<p class="lede">{e(t["lede"])}</p>',
        f'<section class="banner {banner}" role="status" aria-live="polite">',
        f'<strong><span class="dot" aria-hidden="true"></span>{e(t[head])}</strong>',
        f'<span class="checked">{e(t["checked"].format(time=_utc(now)))}</span>',
        "</section>",
    ]
    ongoing, past = split_incidents(incidents, now)
    if ongoing:
        out.append(f'<section aria-labelledby="ongoing"><h2 id="ongoing">{e(t["ongoing"])}</h2>')
        out += [_incident(i, lang, ongoing=True) for i in ongoing]
        out.append("</section>")
    out.append(f'<section aria-labelledby="surfaces"><h2 id="surfaces">{e(t["surfaces"])}</h2><ul class="surfaces">')
    for surface, names in SURFACES.items():
        cell = states[surface]
        total = total_uptime(history, surface)
        summary = t["uptime"].format(pct=pct(total), days=days_text(lang, measured_days(history, surface, now))) \
            if total is not None else t["uptime_none"]
        out.append(f'<li class="surface"><div class="row"><h3>{e(names[lang])}</h3>'
                   f'<span class="state {cell["state"]}"><span class="dot" aria-hidden="true"></span>{e(t[cell["state"]])}</span></div>')
        if cell["ok"] < cell["total"]:
            out.append(f'<p class="detail">{e(t["endpoints"].format(ok=cell["ok"], total=cell["total"]))}</p>')
        if cell["failing"]:
            names_shown = ", ".join(cell["failing"][:5])
            if len(cell["failing"]) > 5:
                names_shown += " " + t["more"].format(n=len(cell["failing"]) - 5)
            out.append(f'<p class="detail">{e(t["failing"].format(names=names_shown))}</p>')
        out.append(f'<p class="uptime">{e(summary)}</p><div class="bars" aria-hidden="true">')
        for day, counts in days_of(history, surface, now):
            value = uptime(counts)
            label = t["day_none"].format(date=day) if value is None else t["day"].format(date=day, pct=pct(value))
            out.append(f'<span class="bar {_day_class(counts)}" title="{e(label)}"></span>')
        out.append(f'</div><div class="axis" aria-hidden="true"><span>{e(t["days_ago"].format(days=DAYS))}</span>'
                   f'<span>{e(t["today"])}</span></div></li>')
    out.append(f'</ul><p class="legend">{e(t["legend"])}</p></section>')
    cells = backup_cells(backups, now)
    if cells:
        out.append(f'<section aria-labelledby="backups"><h2 id="backups">{e(t["backups"])}</h2>'
                   f'<p class="lede">{e(t["backups_lede"])}</p><ul class="surfaces">')
        for key, cell in cells.items():
            css = "up" if cell["state"] == "fresh" else "down"
            when = t["never"] if cell["at"] is None else age_text(lang, now - cell["at"])
            stamp = "" if cell["at"] is None else f' datetime="{dt.datetime.fromtimestamp(cell["at"], dt.UTC).isoformat()}"'
            out.append(f'<li class="surface"><div class="row"><h3>{e(t[key])}</h3>'
                       f'<span class="state {css}"><span class="dot" aria-hidden="true"></span>{e(t[cell["state"]])}</span></div>'
                       f'<p class="detail"><time{stamp}>{e(when)}</time></p>')
            if cell["result"]:
                out.append(f'<p class="detail">{e(t["result_" + cell["result"]])}</p>')
            out.append("</li>")
        out.append("</ul></section>")
    out.append(f'<section aria-labelledby="past"><h2 id="past">{e(t["past"].format(days=DAYS))}</h2>')
    if past:
        out += [_incident(i, lang, ongoing=False) for i in past]
    else:
        out.append(f'<p class="empty">{e(t["no_incidents"].format(days=DAYS))}</p>')
    out.append("</section>")
    out.append(f'<footer><a href="/status.json">{e(t["machine"])}</a></footer>')
    out += ["</main>", "</body>", "</html>", ""]
    return "\n".join(out)


def _incident(incident, lang, ongoing):
    t, e = TEXT[lang], html.escape
    when = t["since"].format(start=_utc(incident["start"])) if ongoing else t["between"].format(
        start=_utc(incident["start"]), end=_utc(incident["end"]))
    names = ", ".join(SURFACES[s][lang] for s in incident["surfaces"])
    return (f'<article class="incident{" ongoing" if ongoing else ""}"><h3>{e(incident["title"][lang])}</h3>'
            f'<p class="when"><time datetime="{dt.datetime.fromtimestamp(incident["start"], dt.UTC).isoformat()}">'
            f'{e(when)}</time></p><p>{e(incident["note"][lang])}</p><p class="affects">{e(t["affects"].format(names=names))}</p></article>')


def machine(states, history, incidents, now, backups=None):
    def iso(ts):
        return None if ts is None else dt.datetime.fromtimestamp(ts, dt.UTC).isoformat().replace("+00:00", "Z")

    return {
        "generatedAt": iso(now),
        "status": overall(states),
        "surfaces": [{"id": s, "state": states[s]["state"], "ok": states[s]["ok"], "total": states[s]["total"],
                      "uptime": total_uptime(history, s), "uptimeDays": measured_days(history, s, now)} for s in SURFACES],
        "incidents": [{"id": i["id"], "start": iso(i["start"]), "end": iso(i["end"]), "surfaces": i["surfaces"],
                       "title": i["title"], "note": i["note"], "automatic": i.get("automatic", False)}
                      for i in incidents if now - (i["end"] or now) <= DAYS * 86400],
        "backups": None if (cells := backup_cells(backups, now)) is None else {
            key: {"at": iso(c["at"]), "state": c["state"], **({"result": c["result"]} if key == "drill" else {})}
            for key, c in cells.items()},
    }


CSS = """:root{--bg:#f7f8fa;--card:#fff;--text:#1b1f24;--muted:#57606a;--line:#d8dee4;--up:#1a7f37;--degraded:#9a6700;
--down:#cf222e;--none:#d0d7de;--focus:#0969da;color-scheme:light}
@media (prefers-color-scheme:dark){:root{--bg:#0d1117;--card:#161b22;--text:#e6edf3;--muted:#9da7b3;--line:#30363d;
--up:#3fb950;--degraded:#d29922;--down:#f85149;--none:#30363d;--focus:#58a6ff;color-scheme:dark}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);
font:16px/1.5 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
.wrap{max-width:56rem;margin:0 auto;padding:2rem 1rem 3rem}
.top{display:flex;justify-content:space-between;align-items:flex-start;gap:1rem;flex-wrap:wrap}
h1{margin:0;font-size:1.75rem}h2{font-size:1.15rem;margin:2rem 0 .75rem}h3{margin:0;font-size:1rem}
.domain,.lede,.checked,.detail,.uptime,.legend,.axis,.when,.affects,.empty{color:var(--muted)}
.domain{margin:.25rem 0 0}.lede{margin:1rem 0}
a{color:var(--focus)}a:focus-visible{outline:3px solid var(--focus);outline-offset:2px}
.lang{padding:.4rem .8rem;border:1px solid var(--line);border-radius:.5rem;background:var(--card);text-decoration:none}
.banner{display:flex;align-items:center;flex-wrap:wrap;gap:.5rem 1rem;padding:1rem 1.25rem;border-radius:.75rem;
background:var(--card);border:1px solid var(--line);border-left:6px solid var(--none)}
.banner.up{border-left-color:var(--up)}.banner.degraded{border-left-color:var(--degraded)}.banner.down{border-left-color:var(--down)}
.banner strong{display:inline-flex;align-items:center;gap:.6rem;font-size:1.1rem}.checked{margin-left:auto;font-size:.9rem}
.dot{display:inline-block;width:.7rem;height:.7rem;border-radius:50%;background:var(--none);flex:none}
.up .dot{background:var(--up)}.degraded .dot{background:var(--degraded)}.down .dot{background:var(--down)}
.surfaces{list-style:none;margin:0;padding:0;display:grid;gap:.75rem}
.surface,.incident{background:var(--card);border:1px solid var(--line);border-radius:.75rem;padding:1rem 1.25rem}
.row{display:flex;justify-content:space-between;align-items:center;gap:.5rem;flex-wrap:wrap}
.state{display:inline-flex;align-items:center;gap:.4rem;font-weight:600}
.state.up{color:var(--up)}.state.degraded{color:var(--degraded)}.state.down{color:var(--down)}.state.unknown{color:var(--muted)}
.detail,.uptime{margin:.35rem 0 0;font-size:.9rem;overflow-wrap:anywhere}
.bars{display:grid;grid-template-columns:repeat(90,1fr);gap:1px;height:2rem;margin-top:.5rem}
.bar{border-radius:1px;background:var(--none)}.bar.up{background:var(--up)}.bar.degraded{background:var(--degraded)}
.bar.down{background:var(--down)}
.axis{display:flex;justify-content:space-between;font-size:.8rem;margin-top:.25rem}
.incident{margin-bottom:.75rem}.incident.ongoing{border-left:6px solid var(--degraded)}
.incident p{margin:.35rem 0 0}.legend{font-size:.85rem}footer{margin-top:2rem;font-size:.9rem}
"""


# ---- server


class Page:
    def __init__(self, target, domain, probe_url, incidents, data_dir, clock=time.time, get=None):
        self.target, self.domain, self.probe_url = target, domain, probe_url
        self.incidents, self.data_dir, self.clock = incidents, data_dir, clock
        self.get = get or self._fetch
        self.lock = threading.Lock()
        self.history = load_history(self._read("history.json") or "{}")
        self.watch = load_watch(self._read("watch.json") or "")
        self.files, self.rendered_at = {}, None
        self.tick(count=False)

    def _read(self, name):
        try:
            with open(os.path.join(self.data_dir, name), encoding="utf-8") as f:
                return f.read()
        except FileNotFoundError:
            return None

    def _fetch(self):
        with urllib.request.urlopen(self.probe_url, timeout=10) as answer:
            return json.loads(answer.read(RESULTS_CAP + 1)[:RESULTS_CAP])

    def tick(self, count=True):
        now = self.clock()
        try:
            answer = self.get() or {}
            results, backups = answer.get("results"), answer.get("backups")
        except (OSError, ValueError, AttributeError) as err:
            # The probe is unreachable: the surfaces' state is unknown, never "up".
            print(json.dumps({"error": "probe results unavailable", "kind": type(err).__name__}), file=sys.stderr, flush=True)
            results, backups = [], None
        states = current(results, self.target, now)
        if count:
            add_minute(self.history, states, now)
            watch_minute(self.watch, states, now)
            self._save("history.json", self.history)
            self._save("watch.json", self.watch)
        incidents = incidents_shown(self.incidents, self.watch)
        files = {
            "/sk/": (render("sk", self.domain, states, self.history, incidents, now, backups).encode(), "text/html; charset=utf-8"),
            "/en/": (render("en", self.domain, states, self.history, incidents, now, backups).encode(), "text/html; charset=utf-8"),
            "/status.css": (CSS.encode(), "text/css; charset=utf-8"),
            "/status.json": (json.dumps(machine(states, self.history, incidents, now, backups)).encode(), "application/json"),
        }
        with self.lock:
            self.files, self.rendered_at = files, now

    def _save(self, name, doc):
        path = os.path.join(self.data_dir, name)
        tmp = path + ".tmp"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(doc, f, separators=(",", ":"))
            os.replace(tmp, path)
        except OSError as err:
            print(json.dumps({"error": f"{name} not saved", "kind": type(err).__name__}), file=sys.stderr, flush=True)

    def answer(self, path, accept_language):
        """(status, content type, body, extra headers) for a GET of `path`."""
        path = path.split("?", 1)[0]
        if path == "/healthz":
            ok = self.rendered_at is not None and self.clock() - self.rendered_at < 180
            return (200, "text/plain", b"ok", {}) if ok else (503, "text/plain", b"no page rendered for three minutes", {})
        extra = {"Cache-Control": CACHE}
        if path in ("/", "/index.html"):
            path, extra["Vary"] = f"/{prefer(accept_language)}/", "Accept-Language"
        elif path in ("/sk", "/en"):
            return 308, "text/plain", b"", {"Location": path + "/"}
        with self.lock:
            found = self.files.get(path)
        if found is None:
            return 404, "text/plain; charset=utf-8", b"not found", {}
        return 200, found[1], found[0], extra

    def run(self):
        while True:
            time.sleep(60 - time.time() % 60)
            try:
                self.tick()
            except Exception as err:  # noqa: BLE001 (one bad minute must not stop the page)
                print(json.dumps({"error": "tick failed", "kind": type(err).__name__}), file=sys.stderr, flush=True)


def prefer(accept_language):
    """sk or en from an Accept-Language header; Slovak unless English ranks higher."""
    best, best_q = "sk", -1.0
    for part in (accept_language or "").split(",")[:20]:
        tag, _, params = part.strip().partition(";")
        lang = tag.strip().lower().split("-")[0]
        if lang not in ("sk", "en"):
            continue
        q = 1.0
        if params.strip().startswith("q="):
            try:
                q = float(params.strip()[2:])
            except ValueError:
                continue
        if q > best_q:
            best, best_q = lang, q
    return best


def serve(page, port):
    class Handler(BaseHTTPRequestHandler):
        def _reply(self, head_only):
            status, kind, body, extra = page.answer(self.path, self.headers.get("Accept-Language"))
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Security-Policy", CSP)
            self.send_header("X-Content-Type-Options", "nosniff")
            for name, value in extra.items():
                self.send_header(name, value)
            self.end_headers()
            if not head_only:
                self.wfile.write(body)

        def do_GET(self):
            self._reply(False)

        def do_HEAD(self):
            self._reply(True)

        def log_message(self, *args):
            pass

    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()


def main():
    page = Page(os.environ["STATUS_TARGET"], os.environ["STATUS_DOMAIN"], os.environ["STATUS_PROBE_URL"],
                load_incidents(os.environ.get("STATUS_INCIDENTS", ""), os.environ["STATUS_TARGET"]),
                os.environ.get("STATUS_DATA", "/data"))
    threading.Thread(target=page.run, daemon=True).start()
    serve(page, int(os.environ.get("STATUS_PORT", "8080")))


if __name__ == "__main__":
    main()
