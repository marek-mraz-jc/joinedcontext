"""Renders any branding block (OPS-47).

The block is read once at start-up from the file the chart mounts. A missing or broken
file is not an error: CKAN then looks like a plain CKAN, which is what an installation
without branding should look like.
"""

import base64
import datetime
import html
import json
import os
import re
import secrets
import shlex
from urllib.parse import parse_qs, quote, urlencode, urlsplit, urlunsplit

import ckan
import ckan.plugins as plugins
import ckan.plugins.toolkit as toolkit

BRANDING_FILE = os.environ.get("JC_BRANDING_FILE", "/etc/jc/branding/branding.json")

NEUTRAL = {
    "instanceName": "joinedcontext",
    "shortName": "joinedcontext",
    "organisation": "",
    "contactEmail": "",
    "logo": "",
    "colours": {
        "primary": "#1d4ed8",
        "secondary": "#0f766e",
        "accent": "#f59e0b",
        "background": "#ffffff",
        "text": "#0f172a",
    },
    "fonts": {"heading": "system-ui, sans-serif", "body": "system-ui, sans-serif"},
    "languages": {"default": "en", "offered": ["en"]},
    "primaryForeground": "#ffffff",
    # The line under the name on the home page and the lines at the foot of every page (a
    # demo disclaimer, an imprint): words only a deployment knows, so they are values too.
    "tagline": "",
    "footerLines": [],
    # A line across the top of every page and a request to search engines to stay out, for an
    # installation that is a demo and must never pass for the publisher's own site (T-4687).
    "ribbon": "",
    "noindex": False,
}

HEX = set("0123456789abcdefABCDEF")


def _is_colour(value):
    """`#rgb` or `#rrggbb`. A custom property is a value the browser evaluates, so
    nothing else may reach a stylesheet."""
    return (
        isinstance(value, str)
        and value.startswith("#")
        and len(value) in (4, 7)
        and all(character in HEX for character in value[1:])
    )


def _load():
    try:
        with open(BRANDING_FILE, encoding="utf-8") as handle:
            block = json.load(handle)
    except (OSError, ValueError):
        return dict(NEUTRAL)
    if not isinstance(block, dict):
        return dict(NEUTRAL)
    branding = dict(NEUTRAL)
    branding.update(block)
    colours = dict(NEUTRAL["colours"])
    for name, value in (branding.get("colours") or {}).items():
        if _is_colour(value):
            colours[name] = value
    branding["colours"] = colours
    if not isinstance(branding.get("tagline"), str):
        branding["tagline"] = ""
    lines = branding.get("footerLines")
    branding["footerLines"] = [line for line in lines if isinstance(line, str) and line.strip()] \
        if isinstance(lines, list) else []
    if not isinstance(branding.get("ribbon"), str):
        branding["ribbon"] = ""
    branding["ribbon"] = branding["ribbon"].strip()
    branding["noindex"] = branding.get("noindex") is True
    red, green, blue = _rgb(colours["primary"])
    branding["primaryRgb"] = "%d, %d, %d" % (red, green, blue)
    # The text on the primary colour, by the YIQ rule the login theme uses too
    # (components/keycloak/charts/theme), so a light brand colour gets dark text.
    branding["primaryForeground"] = "#000000" if (red * 299 + green * 587 + blue * 114) // 1000 >= 128 else "#ffffff"
    return branding


def _rgb(colour):
    """`#rgb` or `#rrggbb` as three integers, for Bootstrap's `--bs-primary-rgb`."""
    digits = colour[1:]
    if len(digits) == 3:
        digits = "".join(character * 2 for character in digits)
    return tuple(int(digits[index:index + 2], 16) for index in (0, 2, 4))


def _available(codes):
    """The subset of `codes` CKAN can actually serve, in the order given.

    A locale CKAN does not ship makes it exit at start-up with "Locale(s) not available".
    The branding block is written by whoever deploys, not by whoever wrote CKAN, so an
    unsupported code narrows the language switcher instead of taking the catalogue down.
    """
    directory = os.path.join(os.path.dirname(ckan.__file__), "i18n")
    try:
        installed = set(os.listdir(directory))
    except OSError:
        installed = set()
    # English is the source language and has no catalogue of its own.
    installed.add("en")
    seen, out = set(), []
    for code in codes:
        if isinstance(code, str) and code in installed and code not in seen:
            seen.add(code)
            out.append(code)
    return out


BRANDING = _load()

# A badge and a description per CKAN organization name, from the chart's `organizations` value,
# for a publisher that set neither in CKAN. Same rule as the branding: a missing or broken file
# leaves CKAN's own placeholder.
ORGANIZATIONS_FILE = os.environ.get(
    "JC_ORGANIZATIONS_FILE", os.path.join(os.path.dirname(BRANDING_FILE), "organizations.json"))


def _organizations():
    try:
        with open(ORGANIZATIONS_FILE, encoding="utf-8") as handle:
            block = json.load(handle)
    except (OSError, ValueError):
        return {}
    return {name: entry for name, entry in block.items() if isinstance(entry, dict)} \
        if isinstance(block, dict) else {}


ORGANIZATIONS = _organizations()

# The packages the publish gate hides (T-4318): `{name: reason}` from the chart's `hidden` value.
# Hidden is not deleted: a sysadmin, and an editor of the package's organization (the harvests'
# token, F-DEP1-002), still finds, reads and updates them, so the next harvest keeps them current
# and a package comes back by leaving the list.
HIDDEN_FILE = os.environ.get("JC_HIDDEN_FILE", os.path.join(os.path.dirname(BRANDING_FILE), "hidden.json"))
PACKAGE_NAME = re.compile(r"^[a-z0-9_-]{2,100}$")
# An organization id, before it goes into a Solr query.
PACKAGE_ID = re.compile(r"^[A-Za-z0-9_-]{1,100}$")


def _hidden():
    try:
        with open(HIDDEN_FILE, encoding="utf-8") as handle:
            block = json.load(handle)
    except (OSError, ValueError):
        return frozenset()
    return frozenset(n for n in block if isinstance(n, str) and PACKAGE_NAME.match(n)) \
        if isinstance(block, dict) else frozenset()


HIDDEN = _hidden()


def jc_hidden_reach():
    """The organization ids whose hidden packages the caller still sees: those it may update
    datasets in (the harvests' editor token, F-DEP1-002). None for a sysadmin, who sees every one;
    nothing for an anonymous caller or outside a request."""
    try:
        user = toolkit.g.userobj
    except (AttributeError, RuntimeError, TypeError):
        return frozenset()
    if not user:
        return frozenset()
    if getattr(user, "sysadmin", False):
        return None
    orgs = toolkit.get_action("organization_list_for_user")(
        {"ignore_auth": True}, {"id": user.id, "permission": "update_dataset"})
    return frozenset(o["id"] for o in orgs if PACKAGE_ID.match(o.get("id") or ""))


def jc_hide(search_params):
    """The search without the hidden packages, on the page, the API and the DCAT feed alike; an
    editor keeps those of its own organizations."""
    reach = jc_hidden_reach() if HIDDEN else None
    if reach is not None:
        names = "-name:(" + " OR ".join(f'"{name}"' for name in sorted(HIDDEN)) + ")"
        if reach:
            orgs = " OR ".join(f'"{org}"' for org in sorted(reach))
            names = f"-(+{names[1:]} -owner_org:({orgs}))"
        search_params["fq"] = f"{search_params.get('fq') or ''} {names}".strip()
    return search_params


def _badge(entry):
    """The publisher's initials on its colour, as an SVG data URI: on this origin, no file to mount."""
    initials = entry.get("initials")
    colour = entry.get("colour")
    if not isinstance(initials, str) or not initials.strip() or not _is_colour(colour):
        return None
    red, green, blue = _rgb(colour)
    ink = "#000000" if (red * 299 + green * 587 + blue * 114) // 1000 >= 128 else "#ffffff"
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="200" viewBox="0 0 400 200">'
        '<rect width="400" height="200" fill="%s"/>'
        '<text x="200" y="100" dy="0.35em" text-anchor="middle" fill="%s" '
        'font-family="system-ui, sans-serif" font-size="72" font-weight="700">%s</text></svg>'
    ) % (colour, ink, html.escape(initials.strip()[:4]))
    return "data:image/svg+xml;base64," + base64.b64encode(svg.encode("utf-8")).decode("ascii")


def jc_org(organization):
    """The organization's image and description: CKAN's own first, then the chart's, in the page's
    language when the chart gives several; None where neither has one."""
    organization = organization or {}
    entry = ORGANIZATIONS.get(organization.get("name") or "", {})
    description = organization.get("description")
    if not description:
        words = entry.get("description")
        if isinstance(words, dict):
            lang = toolkit.h.lang() or "en"
            words = words.get(lang) or words.get("en")
        description = words if isinstance(words, str) and words.strip() else None
    return {"image": organization.get("image_display_url") or _badge(entry), "description": description}

# The words the theme adds to CKAN's own, in the four languages of the Portal (UI-82) and in
# Hungarian for southern BBSK (T-5115). CKAN's catalogues translate everything else; a language
# CKAN names `cs_CZ` reads the `cs` column. The `hu` column is a machine draft until a native
# speaker reviews it (T-5115); the guides and PAGES below have none and fall back to English.
STRINGS = {
    "about": {"en": "About this dataset", "sk": "O tomto datasete", "cs": "O této datové sadě", "de": "Über diesen Datensatz", "hu": "Az adatkészletről"},
    "publisher": {"en": "Publisher", "sk": "Vydavateľ", "cs": "Vydavatel", "de": "Herausgeber", "hu": "Kiadó"},
    "licence": {"en": "Licence", "sk": "Licencia", "cs": "Licence", "de": "Lizenz", "hu": "Licenc"},
    "attribution": {"en": "Attribution", "sk": "Uvedenie zdroja", "cs": "Uvedení zdroje", "de": "Quellenangabe", "hu": "Forrásmegjelölés"},
    "source": {"en": "Original data", "sk": "Pôvodné dáta", "cs": "Původní data", "de": "Ursprüngliche Daten", "hu": "Eredeti adatok"},
    "sources_all": {"en": "Show all ({n})", "sk": "Zobraziť všetky ({n})", "cs": "Zobrazit všechny ({n})", "de": "Alle anzeigen ({n})", "hu": "Mind megjelenítése ({n})"},
    "topic": {"en": "Topic", "sk": "Téma", "cs": "Téma", "de": "Thema", "hu": "Téma"},
    "keywords": {"en": "Keywords", "sk": "Kľúčové slová", "cs": "Klíčová slova", "de": "Schlagwörter", "hu": "Kulcsszavak"},
    "format": {"en": "Format", "sk": "Formát", "cs": "Formát", "de": "Format", "hu": "Formátum"},
    "by_topic": {"en": "Browse by topic", "sk": "Podľa témy", "cs": "Podle tématu", "de": "Nach Thema", "hu": "Témák szerint"},
    "datasets_n": {"en": ("dataset", "datasets", "datasets"), "sk": ("dataset", "datasety", "datasetov"),
                   "cs": ("datová sada", "datové sady", "datových sad"), "de": ("Datensatz", "Datensätze", "Datensätze"),
                   "hu": ("adatkészlet", "adatkészlet", "adatkészlet")},
    "frequency": {"en": "Updated", "sk": "Aktualizácia", "cs": "Aktualizace", "de": "Aktualisierung", "hu": "Frissítés"},
    "last_update": {"en": "Last update", "sk": "Posledná aktualizácia", "cs": "Poslední aktualizace", "de": "Letzte Aktualisierung", "hu": "Utolsó frissítés"},
    "next_update": {"en": "Next update expected", "sk": "Ďalšia aktualizácia", "cs": "Další aktualizace", "de": "Nächste Aktualisierung", "hu": "Következő várható frissítés"},
    "coverage": {"en": "Area covered", "sk": "Pokryté územie", "cs": "Pokryté území", "de": "Abgedecktes Gebiet", "hu": "Lefedett terület"},
    "period": {"en": "Time covered", "sk": "Obdobie", "cs": "Období", "de": "Zeitraum", "hu": "Lefedett időszak"},
    "contact": {"en": "Contact", "sk": "Kontakt", "cs": "Kontakt", "de": "Kontakt", "hu": "Kapcsolat"},
    "identifier": {"en": "Identifier", "sk": "Identifikátor", "cs": "Identifikátor", "de": "Kennung", "hu": "Azonosító"},
    "language": {"en": "Language", "sk": "Jazyk", "cs": "Jazyk", "de": "Sprache", "hu": "Nyelv"},
    "conforms_to": {"en": "Follows the standard", "sk": "Zodpovedá štandardu", "cs": "Odpovídá standardu", "de": "Folgt dem Standard", "hu": "Megfelel a szabványnak"},
    "status": {"en": "Status", "sk": "Stav", "cs": "Stav", "de": "Status", "hu": "Állapot"},
    "live": {"en": "Live data (API)", "sk": "Živé dáta (API)", "cs": "Živá data (API)", "de": "Live-Daten (API)", "hu": "Élő adatok (API)"},
    "live_lead": {
        "en": "The current state of this data, answered by the platform on every request (NGSI-LD).",
        "sk": "Aktuálny stav týchto dát, ktorý platforma vracia pri každej požiadavke (NGSI-LD).",
        "cs": "Aktuální stav těchto dat, který platforma vrací při každém požadavku (NGSI-LD).",
        "de": "Der aktuelle Stand dieser Daten, von der Plattform bei jeder Anfrage beantwortet (NGSI-LD).",
        "hu": "Az adatok aktuális állapota, amelyet a platform minden kérésre megad (NGSI-LD).",
    },
    "example": {"en": "Start here: the kinds of entities it holds", "sk": "Začnite tu: aké druhy entít obsahuje", "cs": "Začněte zde: jaké druhy entit obsahuje", "de": "Hier beginnen: welche Arten von Entitäten es enthält", "hu": "Kezdje itt: milyen entitásokat tartalmaz"},
    "copy": {"en": "Copy", "sk": "Kopírovať", "cs": "Kopírovat", "de": "Kopieren", "hu": "Másolás"},
    "copied": {"en": "Copied", "sk": "Skopírované", "cs": "Zkopírováno", "de": "Kopiert", "hu": "Másolva"},
    # The browser refused the clipboard (T-4762): the text is selected for the person's own copy.
    "copy_select": {"en": "Selected: press Ctrl+C", "sk": "Označené: stlačte Ctrl+C", "cs": "Označeno: stiskněte Ctrl+C", "de": "Markiert: Strg+C drücken", "hu": "Kijelölve: nyomja meg a Ctrl+C billentyűket"},
    "visualizations": {"en": "Charts", "sk": "Grafy", "cs": "Grafy", "de": "Diagramme", "hu": "Grafikonok"},
    "viz_embed": {"en": "Embed on a website", "sk": "Vložiť na web", "cs": "Vložit na web", "de": "Auf einer Website einbetten", "hu": "Beágyazás weboldalba"},
    "viz_embed_lead": {
        "en": "Paste this code into your page; the chart keeps reading the current data.",
        "sk": "Vložte tento kód do svojej stránky; graf bude stále čítať aktuálne dáta.",
        "cs": "Vložte tento kód do své stránky; graf bude stále číst aktuální data.",
        "de": "Fügen Sie diesen Code in Ihre Seite ein; das Diagramm liest weiter die aktuellen Daten.",
        "hu": "Illessze be ezt a kódot az oldalába; a grafikon mindig az aktuális adatokat olvassa.",
    },
    "viz_table": {"en": "The data as a table", "sk": "Údaje ako tabuľka", "cs": "Údaje jako tabulka", "de": "Die Daten als Tabelle", "hu": "Az adatok táblázatként"},
    "resources": {"en": "Data and downloads", "sk": "Dáta na stiahnutie", "cs": "Data ke stažení", "de": "Daten und Downloads", "hu": "Adatok és letöltések"},
    "no_resources": {"en": "This dataset has no downloads yet.", "sk": "Tento dataset zatiaľ nemá nič na stiahnutie.", "cs": "Tato datová sada zatím nemá nic ke stažení.", "de": "Dieser Datensatz hat noch keine Downloads.", "hu": "Ehhez az adatkészlethez még nincs letölthető fájl."},
    "empty": {
        "en": "Nothing matches. Try fewer or other words, or remove a filter on the left.",
        "sk": "Nič nezodpovedá. Skúste menej alebo iné slová, alebo zrušte filter vľavo.",
        "cs": "Nic neodpovídá. Zkuste méně nebo jiná slova, nebo zrušte filtr vlevo.",
        "de": "Nichts gefunden. Versuchen Sie weniger oder andere Wörter, oder entfernen Sie links einen Filter.",
        "hu": "Nincs találat. Próbáljon kevesebb vagy más szót, vagy törölje a bal oldali szűrőt.",
    },
    "portal": {"en": "Portal", "sk": "Portál", "cs": "Portál", "de": "Portal", "hu": "Portál"},
    "about_site": {"en": "About this catalogue", "sk": "O tomto katalógu", "cs": "O tomto katalogu", "de": "Über diesen Katalog", "hu": "A katalógusról"},
    "catalogue_api": {"en": "Catalogue API", "sk": "API katalógu", "cs": "API katalogu", "de": "Katalog-API", "hu": "Katalógus API"},
    # What a failed single sign-on says (T-2888). The extension keeps its reason in the session
    # and shows it nowhere, so the person was sent back to the login page without a word.
    "sso_cookie": {
        "en": "The sign-in could not finish: this browser did not keep the catalogue's cookie from the start of the sign-in. Allow cookies for this site and sign in again.",
        "sk": "Prihlásenie sa nedokončilo: prehliadač si neponechal súbor cookie katalógu zo začiatku prihlásenia. Povoľte súbory cookie pre túto stránku a prihláste sa znova.",
        "cs": "Přihlášení se nedokončilo: prohlížeč si neponechal soubor cookie katalogu ze začátku přihlášení. Povolte soubory cookie pro tento web a přihlaste se znovu.",
        "de": "Die Anmeldung wurde nicht abgeschlossen: Der Browser hat das Cookie des Katalogs vom Beginn der Anmeldung nicht behalten. Erlauben Sie Cookies für diese Seite und melden Sie sich erneut an.",
        "hu": "A bejelentkezés nem fejeződött be: a böngésző nem őrizte meg a katalógus sütijét a bejelentkezés kezdetétől. Engedélyezze a sütiket ezen az oldalon, és jelentkezzen be újra.",
    },
    "sso_state": {
        "en": "The sign-in expired or was started in another tab. Sign in again.",
        "sk": "Prihlásenie vypršalo alebo sa začalo na inej karte. Prihláste sa znova.",
        "cs": "Přihlášení vypršelo nebo bylo zahájeno na jiné kartě. Přihlaste se znovu.",
        "de": "Die Anmeldung ist abgelaufen oder wurde in einem anderen Tab begonnen. Melden Sie sich erneut an.",
        "hu": "A bejelentkezés lejárt, vagy egy másik lapon indult. Jelentkezzen be újra.",
    },
    "sso_refused": {
        "en": "The identity service refused the catalogue's sign-in request. Tell the administrator: the catalogue's log names the reason.",
        "sk": "Služba identity odmietla žiadosť katalógu o prihlásenie. Povedzte to správcovi: dôvod je v logu katalógu.",
        "cs": "Služba identity odmítla žádost katalogu o přihlášení. Řekněte to správci: důvod je v logu katalogu.",
        "de": "Der Identitätsdienst hat die Anmeldeanfrage des Katalogs abgelehnt. Melden Sie es der Administration: Das Protokoll des Katalogs nennt den Grund.",
        "hu": "Az azonosítási szolgáltatás elutasította a katalógus bejelentkezési kérését. Szóljon a rendszergazdának: az okot a katalógus naplója tartalmazza.",
    },
    "sso_account": {
        "en": "More than one catalogue account uses your e-mail address, so the catalogue cannot tell which one is yours. Ask the administrator to merge them.",
        "sk": "Vašu e-mailovú adresu používa viac účtov katalógu, takže katalóg nevie, ktorý je váš. Požiadajte správcu, aby ich zlúčil.",
        "cs": "Vaši e-mailovou adresu používá více účtů katalogu, takže katalog neví, který je váš. Požádejte správce, aby je sloučil.",
        "de": "Mehrere Katalogkonten verwenden Ihre E-Mail-Adresse, daher weiß der Katalog nicht, welches Ihres ist. Bitten Sie die Administration, sie zusammenzuführen.",
        "hu": "Az Ön e-mail-címét több katalógusfiók is használja, így a katalógus nem tudja, melyik az Öné. Kérje meg a rendszergazdát, hogy vonja össze őket.",
    },
    "sso_other": {
        "en": "The sign-in did not finish. Sign in again; if it fails again, tell the administrator when it happened.",
        "sk": "Prihlásenie sa nedokončilo. Prihláste sa znova; ak zlyhá opäť, povedzte správcovi, kedy sa to stalo.",
        "cs": "Přihlášení se nedokončilo. Přihlaste se znovu; pokud selže znovu, řekněte správci, kdy se to stalo.",
        "de": "Die Anmeldung wurde nicht abgeschlossen. Melden Sie sich erneut an; schlägt es wieder fehl, nennen Sie der Administration den Zeitpunkt.",
        "hu": "A bejelentkezés nem fejeződött be. Jelentkezzen be újra; ha ismét nem sikerül, írja meg a rendszergazdának, mikor történt.",
    },
    "assistant": {"en": "Ask the assistant", "sk": "Opýtajte sa asistenta", "cs": "Zeptejte se asistenta", "de": "Fragen Sie den Assistenten", "hu": "Kérdezze az asszisztenst"},
    # The dataset page leads with its rows, then the downloads in three sections (T-3009).
    "preview": {"en": "The data", "sk": "Dáta", "cs": "Data", "de": "Die Daten", "hu": "Az adatok"},
    "explorer": {"en": "Live data", "sk": "Živé dáta", "cs": "Živá data", "de": "Live-Daten", "hu": "Élő adatok"},
    "explorer_lead": {
        "en": "Search, filter and map the entities straight from the published Endpoint; nothing is copied, so you always see the current data.",
        "sk": "Hľadajte, filtrujte a zobrazte entity na mape priamo zo zverejneného Endpointu; nič sa nekopíruje, takže vždy vidíte aktuálne dáta.",
        "cs": "Hledejte, filtrujte a zobrazte entity na mapě přímo ze zveřejněného Endpointu; nic se nekopíruje, takže vždy vidíte aktuální data.",
        "de": "Entitäten direkt am veröffentlichten Endpoint suchen, filtern und auf der Karte zeigen; nichts wird kopiert, Sie sehen immer den aktuellen Stand.",
        "hu": "Keressen, szűrjön és jelenítse meg térképen az entitásokat közvetlenül a közzétett Endpointból; semmi sem másolódik, így mindig az aktuális adatokat látja.",
    },
    "preview_lead": {
        "en": "Browse, search and sort the rows here, or download the whole table.",
        "sk": "Prezerajte, vyhľadávajte a triedte riadky priamo tu, alebo si stiahnite celú tabuľku.",
        "cs": "Procházejte, vyhledávejte a řaďte řádky přímo zde, nebo si stáhněte celou tabulku.",
        "de": "Zeilen hier durchsuchen, filtern und sortieren oder die ganze Tabelle herunterladen.",
        "hu": "Böngéssze, keresse és rendezze itt a sorokat, vagy töltse le a teljes táblázatot.",
    },
    # A count's word in each language's plural forms, one, two to four, five and more (T-4066).
    "rows": {"en": ("row", "rows", "rows"), "sk": ("riadok", "riadky", "riadkov"),
             "cs": ("řádek", "řádky", "řádků"), "de": ("Zeile", "Zeilen", "Zeilen"), "hu": ("sor", "sor", "sor")},
    "table_title": {"en": "Table of the data", "sk": "Tabuľka dát", "cs": "Tabulka dat", "de": "Tabelle der Daten", "hu": "Az adatok táblázata"},
    # The dataset map (T-4639).
    "map": {"en": "Map", "sk": "Mapa", "cs": "Mapa", "de": "Karte", "hu": "Térkép"},
    "map_title": {"en": "Map of the data", "sk": "Mapa dát", "cs": "Mapa dat", "de": "Karte der Daten", "hu": "Az adatok térképe"},
    "map_as_table": {"en": "Show as a table", "sk": "Zobraziť ako tabuľku", "cs": "Zobrazit jako tabulku", "de": "Als Tabelle anzeigen", "hu": "Megjelenítés táblázatként"},
    "map_in_catalogue": {"en": "Show in the catalogue", "sk": "Zobraziť v katalógu", "cs": "Zobrazit v katalogu", "de": "Im Katalog anzeigen", "hu": "Megjelenítés a katalógusban"},
    "map_embed": {"en": "Embed on your site", "sk": "Vložiť na web", "cs": "Vložit na web", "de": "Auf Ihrer Website einbetten", "hu": "Beágyazás a saját oldalába"},
    "map_embed_lead": {
        "en": "Paste this into your page; the map stays live and shows only public data.",
        "sk": "Vložte to do svojej stránky; mapa zostáva živá a ukazuje len verejné údaje.",
        "cs": "Vložte to do své stránky; mapa zůstává živá a ukazuje jen veřejná data.",
        "de": "Fügen Sie dies in Ihre Seite ein; die Karte bleibt aktuell und zeigt nur öffentliche Daten.",
        "hu": "Illessze be ezt az oldalába; a térkép élő marad, és csak nyilvános adatokat mutat.",
    },
    "map_cap": {
        "en": "Showing the first {n}; zoom in to see the rest.",
        "sk": "Zobrazených prvých {n}, priblížte mapu.",
        "cs": "Zobrazeno prvních {n}, přibližte mapu.",
        "de": "Die ersten {n} werden gezeigt; zoomen Sie hinein.",
        "hu": "Az első {n} látható; nagyítson rá a többi megtekintéséhez.",
    },
    "map_failed": {
        "en": "The map could not load its data; the table above still works.",
        "sk": "Mapa nenačítala údaje; tabuľka vyššie funguje.",
        "cs": "Mapa nenačetla data; tabulka výše funguje.",
        "de": "Die Karte konnte ihre Daten nicht laden; die Tabelle oben funktioniert.",
        "hu": "A térkép nem tudta betölteni az adatait; a fenti táblázat továbbra is működik.",
    },
    "map_empty": {
        "en": "This dataset has no places to show on a map.",
        "sk": "Tento dataset nemá miesta, ktoré by sa dali ukázať na mape.",
        "cs": "Tato datová sada nemá místa, která by šla ukázat na mapě.",
        "de": "Dieser Datensatz enthält keine Orte, die eine Karte zeigen könnte.",
        "hu": "Ebben az adatkészletben nincsenek térképen megjeleníthető helyek.",
    },
    "map_close": {"en": "Close", "sk": "Zavrieť", "cs": "Zavřít", "de": "Schließen", "hu": "Bezárás"},
    # The attributes after a feature's key ones (T-4672).
    "map_more": {"en": "More", "sk": "Viac", "cs": "Více", "de": "Mehr", "hu": "Továbbiak"},
    "map_loading": {"en": "Loading the map's data…", "sk": "Načítavam údaje mapy…", "cs": "Načítám data mapy…", "de": "Die Kartendaten werden geladen…", "hu": "A térkép adatai betöltődnek…"},
    "map_panel": {"en": "Selected place", "sk": "Vybrané miesto", "cs": "Vybrané místo", "de": "Ausgewählter Ort", "hu": "Kiválasztott hely"},
    "map_length": {"en": "Length", "sk": "Dĺžka", "cs": "Délka", "de": "Länge", "hu": "Hossz"},
    "map_area": {"en": "Area", "sk": "Plocha", "cs": "Plocha", "de": "Fläche", "hu": "Terület"},
    "map_keys": {
        "en": "Arrow keys move the map, + and - zoom, Enter opens the place in the middle.",
        "sk": "Šípky posúvajú mapu, + a - menia priblíženie, Enter otvorí miesto v strede.",
        "cs": "Šipky posouvají mapu, + a - mění přiblížení, Enter otevře místo uprostřed.",
        "de": "Pfeiltasten verschieben die Karte, + und - zoomen, Enter öffnet den Ort in der Mitte.",
        "hu": "A nyílbillentyűk mozgatják a térképet, a + és a - nagyít és kicsinyít, az Enter megnyitja a középen lévő helyet.",
    },
    "tables": {"en": "tables", "sk": "tabuľky", "cs": "tabulky", "de": "Tabellen", "hu": "táblázat"},
    "table_what": {"en": "The table above, as one file", "sk": "Tabuľka vyššie ako jeden súbor", "cs": "Tabulka výše jako jeden soubor", "de": "Die Tabelle oben als eine Datei", "hu": "A fenti táblázat egy fájlban"},
    "open_table": {"en": "Open the table on its own page", "sk": "Otvoriť tabuľku na samostatnej stránke", "cs": "Otevřít tabulku na samostatné stránce", "de": "Tabelle auf eigener Seite öffnen", "hu": "A táblázat megnyitása külön oldalon"},
    "no_view": {
        "en": "The table has no view yet; download it below.",
        "sk": "Tabuľka zatiaľ nemá zobrazenie; stiahnite si ju nižšie.",
        "cs": "Tabulka zatím nemá zobrazení; stáhněte si ji níže.",
        "de": "Die Tabelle hat noch keine Ansicht; laden Sie sie unten herunter.",
        "hu": "A táblázatnak még nincs nézete; töltse le lent.",
    },
    "download": {"en": "Download", "sk": "Stiahnuť", "cs": "Stáhnout", "de": "Herunterladen", "hu": "Letöltés"},
    "part_of": {"en": "Part of the dataset", "sk": "Súčasť datasetu", "cs": "Součást datové sady", "de": "Teil des Datensatzes", "hu": "Az adatkészlet része"},
    "address": {"en": "Address", "sk": "Adresa", "cs": "Adresa", "de": "Adresse", "hu": "Cím"},
    "file_lead": {
        "en": "This is a file to download; it opens in the program that reads its format.",
        "sk": "Toto je súbor na stiahnutie; otvorí sa v programe, ktorý číta jeho formát.",
        "cs": "Toto je soubor ke stažení; otevře se v programu, který čte jeho formát.",
        "de": "Dies ist eine Datei zum Herunterladen; sie öffnet sich im Programm, das ihr Format liest.",
        "hu": "Ez egy letölthető fájl; a formátumát olvasó programban nyílik meg.",
    },
    "api_lead": {
        "en": "This is an address a program calls; open it to see what it answers.",
        "sk": "Toto je adresa, ktorú volá program; otvorte ju a uvidíte, čo odpovedá.",
        "cs": "Toto je adresa, kterou volá program; otevřete ji a uvidíte, co odpovídá.",
        "de": "Dies ist eine Adresse, die ein Programm aufruft; öffnen Sie sie, um die Antwort zu sehen.",
        "hu": "Ez egy cím, amelyet egy program hív meg; nyissa meg, és meglátja, mit válaszol.",
    },
    "open": {"en": "Open", "sk": "Otvoriť", "cs": "Otevřít", "de": "Öffnen", "hu": "Megnyitás"},
    "details": {"en": "Details", "sk": "Podrobnosti", "cs": "Podrobnosti", "de": "Details", "hu": "Részletek"},
    "section_data": {"en": "Downloads", "sk": "Na stiahnutie", "cs": "Ke stažení", "de": "Downloads", "hu": "Letöltések"},
    "section_data_lead": {"en": "The data as files, updated with the dataset.", "sk": "Dáta ako súbory, aktualizované spolu s datasetom.", "cs": "Data jako soubory, aktualizovaná spolu s datovou sadou.", "de": "Die Daten als Dateien, mit dem Datensatz aktualisiert.", "hu": "Az adatok fájlokként, az adatkészlettel együtt frissítve."},
    "section_api": {"en": "API and live access", "sk": "API a živý prístup", "cs": "API a živý přístup", "de": "API und Live-Zugriff", "hu": "API és élő hozzáférés"},
    "usage": {"en": "How to use it", "sk": "Ako to použiť", "cs": "Jak to použít", "de": "So verwenden Sie es", "hu": "Hogyan használja"},
    "guide": {"en": "The WebSocket guide: messages, limits and examples", "sk": "Príručka k WebSocketu: správy, limity a príklady", "cs": "Průvodce WebSocketem: zprávy, limity a příklady", "de": "Die WebSocket-Anleitung: Nachrichten, Grenzen und Beispiele", "hu": "A WebSocket-útmutató: üzenetek, korlátok és példák"},
    "developer_page": {"en": "Developer page: every surface with code samples, try-it, licence and rate limit", "sk": "Stránka pre vývojárov: každé rozhranie s ukážkami kódu, vyskúšaním, licenciou a limitom požiadaviek", "cs": "Stránka pro vývojáře: každé rozhraní s ukázkami kódu, vyzkoušením, licencí a limitem požadavků", "de": "Entwicklerseite: jede Schnittstelle mit Codebeispielen, Ausprobieren, Lizenz und Anfragelimit", "hu": "Fejlesztői oldal: minden interfész kódpéldákkal, kipróbálással, licenccel és kéréskorláttal"},
    "snippet": {"en": "Command to copy", "sk": "Príkaz na skopírovanie", "cs": "Příkaz ke zkopírování", "de": "Befehl zum Kopieren", "hu": "Másolható parancs"},
    "mcp": {"en": "Connect an AI assistant", "sk": "Pripojte AI asistenta", "cs": "Připojte AI asistenta", "de": "KI-Assistenten verbinden", "hu": "AI-asszisztens csatlakoztatása"},
    "mcp_lead": {
        "en": "One address for the whole catalogue (MCP). Add it once and your assistant finds every dataset, its attributes, data model and access rules by itself. Open data needs no login; signing in adds the data you may read.",
        "sk": "Jedna adresa pre celý katalóg (MCP). Pridajte ju raz a asistent sám nájde každý dataset, jeho atribúty, dátový model a pravidlá prístupu. Otvorené dáta nepotrebujú prihlásenie; po prihlásení pribudnú dáta, ktoré smiete čítať.",
        "cs": "Jedna adresa pro celý katalog (MCP). Přidejte ji jednou a asistent sám najde každou datovou sadu, její atributy, datový model a pravidla přístupu. Otevřená data nepotřebují přihlášení; po přihlášení přibudou data, která smíte číst.",
        "de": "Eine Adresse für den ganzen Katalog (MCP). Einmal hinzugefügt, findet Ihr Assistent jeden Datensatz, seine Attribute, sein Datenmodell und seine Zugriffsregeln selbst. Offene Daten brauchen keine Anmeldung; nach der Anmeldung kommen die Daten hinzu, die Sie lesen dürfen.",
        "hu": "Egyetlen cím az egész katalógushoz (MCP). Adja hozzá egyszer, és az asszisztense magától megtalál minden adatkészletet, azok attribútumait, adatmodelljét és hozzáférési szabályait. A nyílt adatokhoz nem kell bejelentkezni; bejelentkezés után azok az adatok is elérhetők, amelyeket Ön olvashat.",
    },
    "mcp_url": {"en": "MCP server address", "sk": "Adresa MCP servera", "cs": "Adresa MCP serveru", "de": "Adresse des MCP-Servers", "hu": "Az MCP-szerver címe"},
    "mcp_claude_ai": {"en": "Claude (claude.ai and Claude Desktop)", "sk": "Claude (claude.ai a Claude Desktop)", "cs": "Claude (claude.ai a Claude Desktop)", "de": "Claude (claude.ai und Claude Desktop)", "hu": "Claude (claude.ai és Claude Desktop)"},
    "mcp_claude_ai_1": {"en": "Open Customize, then Connectors, choose + Add and Add custom connector.", "sk": "Otvorte Prispôsobiť, potom Konektory, zvoľte + Pridať a Pridať vlastný konektor.", "cs": "Otevřete Přizpůsobit, pak Konektory, zvolte + Přidat a Přidat vlastní konektor.", "de": "Öffnen Sie Anpassen, dann Konnektoren, wählen Sie + Hinzufügen und Benutzerdefinierten Konnektor hinzufügen.", "hu": "Nyissa meg a Testreszabás, majd a Csatlakozók menüt, válassza a + Hozzáadás, majd az Egyéni csatlakozó hozzáadása lehetőséget."},
    "mcp_claude_ai_2": {"en": "Name it joinedcontext and paste this address:", "sk": "Pomenujte ho joinedcontext a vložte túto adresu:", "cs": "Pojmenujte ho joinedcontext a vložte tuto adresu:", "de": "Nennen Sie ihn joinedcontext und fügen Sie diese Adresse ein:", "hu": "Nevezze el joinedcontext néven, és illessze be ezt a címet:"},
    "mcp_claude_ai_3": {"en": "Leave the advanced settings empty and choose Add. Open data needs no login.", "sk": "Rozšírené nastavenia nechajte prázdne a zvoľte Pridať. Otvorené dáta nepotrebujú prihlásenie.", "cs": "Rozšířená nastavení nechte prázdná a zvolte Přidat. Otevřená data nepotřebují přihlášení.", "de": "Lassen Sie die erweiterten Einstellungen leer und wählen Sie Hinzufügen. Offene Daten brauchen keine Anmeldung.", "hu": "A speciális beállításokat hagyja üresen, és válassza a Hozzáadás lehetőséget. A nyílt adatokhoz nem kell bejelentkezni."},
    "mcp_editors": {"en": "Code editors and other assistants", "sk": "Editory kódu a iní asistenti", "cs": "Editory kódu a jiní asistenti", "de": "Code-Editoren und andere Assistenten", "hu": "Kódszerkesztők és más asszisztensek"},
    "mcp_claude": {"en": "Claude Code", "sk": "Claude Code", "cs": "Claude Code", "de": "Claude Code", "hu": "Claude Code"},
    "mcp_json": {"en": "Any other MCP client (JSON)", "sk": "Iný MCP klient (JSON)", "cs": "Jiný MCP klient (JSON)", "de": "Jeder andere MCP-Client (JSON)", "hu": "Bármely más MCP-kliens (JSON)"},
    # "Install", not "Add to" (T-4735): no "Pridať do …" stays on /assistants, whose store links went.
    "mcp_vscode": {"en": "Install in VS Code", "sk": "Nainštalovať vo VS Code", "cs": "Nainstalovat ve VS Code", "de": "In VS Code installieren", "hu": "Telepítés a VS Code-ba"},
    "mcp_cursor": {"en": "Install in Cursor", "sk": "Nainštalovať v Cursore", "cs": "Nainstalovat v Cursoru", "de": "In Cursor installieren", "hu": "Telepítés a Cursorba"},
    # The "Use it in your AI assistant" page and its button (T-4585). The stores are the ones each
    # maker documents, read 2026-10-18: ChatGPT at chatgpt.com/plugins
    # (developers.openai.com/apps-sdk/deploy/connect-chatgpt), Claude at Customize > Connectors
    # (support.claude.com article 11175166). The Grok app takes no connectors (Grok
    # remote MCP only through the xAI API, docs.x.ai), so their cards link nowhere.
    "assistants": {"en": "Use it in your AI assistant", "sk": "Použite v AI asistentovi", "cs": "Použijte v AI asistentovi", "de": "In Ihrem KI-Assistenten nutzen", "hu": "Használja AI-asszisztensében"},
    "assistants_label": {"en": "Use the data in ChatGPT, Claude or any MCP client", "sk": "Použite dáta v ChatGPT, Claude alebo v akomkoľvek klientovi MCP", "cs": "Použijte data v ChatGPT, Claude nebo v libovolném klientovi MCP", "de": "Die Daten in ChatGPT, Claude oder jedem MCP-Client nutzen", "hu": "Használja az adatokat a ChatGPT-ben, a Claude-ban vagy bármely MCP-kliensben"},
    "assistants_lead": {
        "en": "Add {name} to the assistant you already use, and ask it about this catalogue's open data. One address covers every dataset; no account is needed.",
        "sk": "Pridajte {name} do asistenta, ktorého už používate, a pýtajte sa ho na otvorené dáta tohto katalógu. Jedna adresa pokrýva všetky datasety; účet netreba.",
        "cs": "Přidejte {name} do asistenta, kterého už používáte, a ptejte se ho na otevřená data tohoto katalogu. Jedna adresa pokrývá všechny datové sady; účet není potřeba.",
        "de": "Fügen Sie {name} dem Assistenten hinzu, den Sie schon nutzen, und fragen Sie ihn nach den offenen Daten dieses Katalogs. Eine Adresse deckt alle Datensätze ab; kein Konto nötig.",
        "hu": "Adja hozzá a(z) {name} csatlakozót a már használt asszisztenséhez, és kérdezze a katalógus nyílt adatairól. Egy cím minden adatkészletet lefed; fiók nem szükséges.",
    },
    # T-4659: the websites the hub's search_documents reads, so a visitor sees what it covers.
    "assistants_sources": {"en": "What the assistant can search", "sk": "Čo asistent prehľadáva",
                           "cs": "Co asistent prohledává", "de": "Was der Assistent durchsucht", "hu": "Miben kereshet az asszisztens"},
    "assistants_sources_lead": {
        "en": "Beside the datasets, the connector searches the public pages and documents of these websites:",
        "sk": "Okrem datasetov konektor prehľadáva verejné stránky a dokumenty týchto webov:",
        "cs": "Kromě datových sad konektor prohledává veřejné stránky a dokumenty těchto webů:",
        "de": "Neben den Datensätzen durchsucht der Connector die öffentlichen Seiten und Dokumente dieser Websites:",
        "hu": "Az adatkészletek mellett a csatlakozó ezeknek a weboldalaknak a nyilvános oldalain és dokumentumaiban is keres:",
    },
    "assistants_source_languages": {"en": "Languages", "sk": "Jazyky", "cs": "Jazyky", "de": "Sprachen", "hu": "Nyelvek"},
    "coming_soon": {"en": "Coming soon", "sk": "Čoskoro", "cs": "Brzy", "de": "Demnächst", "hu": "Hamarosan"},
    # T-4735 (owner 2026-10-19): no directory lists the connector yet, so /assistants and
    # /connector say so once, above the cards; the line goes when a directory lists us.
    "connector_status": {
        "en": "We are still preparing this connector: it is not yet published in the Claude or ChatGPT directories. Until then, add it as a custom connector by following the guide.",
        "sk": "Konektor pripravujeme: v adresároch Claude a ChatGPT ešte nie je zverejnený. Zatiaľ ho pridáte ako vlastný konektor podľa návodu.",
        "cs": "Konektor připravujeme: v adresářích Claude a ChatGPT zatím není zveřejněný. Do té doby ho přidáte jako vlastní konektor podle návodu.",
        "de": "Wir bereiten diesen Connector noch vor: In den Verzeichnissen von Claude und ChatGPT ist er noch nicht veröffentlicht. Bis dahin fügen Sie ihn als eigenen Connector nach der Anleitung hinzu.",
        "hu": "Ezt a csatlakozót még előkészítjük: a Claude és a ChatGPT könyvtárában még nem jelent meg. Addig adja hozzá egyéni csatlakozóként az útmutató szerint.",
    },
    "store_address": {"en": "Each guide shows where to paste this address:",
                      "sk": "Každý návod ukazuje, kam vložiť túto adresu:",
                      "cs": "Každý návod ukazuje, kam vložit tuto adresu:",
                      "de": "Jede Anleitung zeigt, wo Sie diese Adresse einfügen:", "hu": "Minden útmutató megmutatja, hová illessze be ezt a címet:"},
    # T-4643: one guide per assistant, for someone who has never added anything to one.
    "guide_open": {"en": "Guide", "sk": "Návod", "cs": "Návod", "de": "Anleitung", "hu": "Útmutató"},
    "guide_title": {"en": "{name}, step by step", "sk": "{name} krok za krokom", "cs": "{name} krok za krokem", "de": "{name} Schritt für Schritt", "hu": "{name}, lépésről lépésre"},
    "guide_who": {"en": "Who can use it", "sk": "Kto to môže použiť", "cs": "Kdo to může použít", "de": "Wer es nutzen kann", "hu": "Ki használhatja"},
    "guide_steps": {"en": "Steps", "sk": "Postup", "cs": "Postup", "de": "Schritte", "hu": "Lépések"},
    "guide_try": {"en": "Try it", "sk": "Vyskúšajte", "cs": "Vyzkoušejte", "de": "Probieren Sie es aus", "hu": "Próbálja ki"},
    # T-4735 (owner 11:07Z, 11:08Z): an assistant seldom reaches for a connector unasked, so
    # /assistants and each guide say to name it in the question, by the name the guide gave it.
    "connector_note": {"en": "Important: the assistant usually won't use the connector on its own. Ask for it in your question, for example:",
                       "sk": "Dôležité: asistent konektor väčšinou sám nepoužije. Napíšte mu to v otázke, napríklad:",
                       "cs": "Důležité: asistent konektor většinou sám nepoužije. Napište mu to v otázce, například:",
                       "de": "Wichtig: Der Assistent nutzt den Connector meist nicht von selbst. Bitten Sie in Ihrer Frage darum, zum Beispiel:", "hu": "Fontos: az asszisztens általában magától nem használja a csatlakozót. Kérje a kérdésében, például:"},
    "connector_why": {"en": "The assistant recognises the connector by the name you gave it when you added it.",
                      "sk": "Asistent spozná konektor podľa mena, ktoré ste mu dali pri pridaní.",
                      "cs": "Asistent pozná konektor podle jména, které jste mu dali při přidání.",
                      "de": "Der Assistent erkennt den Connector an dem Namen, den Sie ihm beim Hinzufügen gegeben haben.", "hu": "Az asszisztens arról a névről ismeri fel a csatlakozót, amelyet a hozzáadáskor adott neki."},
    # The question to try, in the note and on each guide, names the connector first (T-4735).
    "guide_use": {"en": "Use {connector}: ", "sk": "Použi {connector}: ", "cs": "Použij {connector}: ", "de": "Nutze {connector}: ", "hu": "Használd a(z) {connector} csatlakozót: "},
    "guide_example": {"en": "Which datasets about air quality do you have?", "sk": "Aké datasety o kvalite ovzdušia máte?",
                      "cs": "Jaké datové sady o kvalitě ovzduší máte?", "de": "Welche Datensätze zur Luftqualität gibt es?", "hu": "Milyen levegőminőségi adatkészleteid vannak?"},
    "guide_good": {"en": "A good first answer names records from this catalogue and says which dataset they come from. If the answer is general or from the web, write: use {name}.",
                   "sk": "Dobrá prvá odpoveď vymenuje záznamy z tohto katalógu a povie, z ktorého datasetu sú. Ak je odpoveď všeobecná alebo z webu, napíšte: použi {name}.",
                   "cs": "Dobrá první odpověď vyjmenuje záznamy z tohoto katalogu a řekne, ze které datové sady jsou. Pokud je odpověď obecná nebo z webu, napište: použij {name}.",
                   "de": "Eine gute erste Antwort nennt Einträge aus diesem Katalog und sagt, aus welchem Datensatz sie stammen. Ist die Antwort allgemein oder aus dem Web, schreiben Sie: nutze {name}.", "hu": "Egy jó első válasz ebből a katalógusból nevez meg rekordokat, és megmondja, melyik adatkészletből származnak. Ha a válasz általános vagy a webről jön, írja be: használd a(z) {name} csatlakozót."},
    "guide_trouble": {"en": "If it does not work", "sk": "Ak to nejde", "cs": "Když to nejde", "de": "Wenn es nicht klappt", "hu": "Ha nem működik"},
    "guide_checked": {"en": "Checked on {date} against {maker}'s own help:", "sk": "Overené dňa {date} podľa pomocníka {maker}:",
                      "cs": "Ověřeno dne {date} podle nápovědy {maker}:", "de": "Geprüft am {date} anhand der Hilfe von {maker}:", "hu": "Ellenőrizve {date} napján a(z) {maker} saját súgója alapján:"},
    "guide_back": {"en": "All assistants", "sk": "Všetci asistenti", "cs": "Všichni asistenti", "de": "Alle Assistenten", "hu": "Minden asszisztens"},
    "for_developers": {"en": "For developers", "sk": "Pre vývojárov", "cs": "Pro vývojáře", "de": "Für Entwickler", "hu": "Fejlesztőknek"},
    "any_client": {"en": "Any MCP client", "sk": "Akýkoľvek MCP klient", "cs": "Jakýkoli MCP klient", "de": "Jeder MCP-Client", "hu": "Bármely MCP-kliens"},
    "section_api_lead": {"en": "For applications and AI assistants that query the data live.", "sk": "Pre aplikácie a AI asistentov, ktorí sa na dáta pýtajú naživo.", "cs": "Pro aplikace a AI asistenty, kteří se na data ptají živě.", "de": "Für Anwendungen und KI-Assistenten, die die Daten live abfragen.", "hu": "Alkalmazásoknak és AI-asszisztenseknek, amelyek élőben kérdezik le az adatokat."},
    "section_schema": {"en": "Data model", "sk": "Dátový model", "cs": "Datový model", "de": "Datenmodell", "hu": "Adatmodell"},
    "section_schema_lead": {"en": "What each field means, in the formats validators and developers read.", "sk": "Čo znamená každé pole, vo formátoch pre validátory a vývojárov.", "cs": "Co znamená každé pole, ve formátech pro validátory a vývojáře.", "de": "Was jedes Feld bedeutet, in den Formaten für Validatoren und Entwickler.", "hu": "Mit jelent az egyes mezők, a validátorok és fejlesztők által olvasott formátumokban."},
    # The home page (T-3009).
    "tagline": {
        "en": "Browse them, download them or ask the assistant about them.",
        "sk": "Prezrite si ich, stiahnite alebo sa na ne opýtajte asistenta.",
        "cs": "Prohlédněte si je, stáhněte nebo se na ně zeptejte asistenta.",
        "de": "Ansehen, herunterladen oder den Assistenten dazu fragen.",
        "hu": "Böngéssze, töltse le őket, vagy kérdezze róluk az asszisztenst.",
    },
    "no_datasets": {"en": "No dataset is published yet.", "sk": "Zatiaľ nie je zverejnený žiadny dataset.", "cs": "Zatím není zveřejněna žádná datová sada.", "de": "Noch ist kein Datensatz veröffentlicht.", "hu": "Még nincs közzétett adatkészlet."},
    "hero_search": {"en": "Search the open data", "sk": "Hľadať v otvorených dátach", "cs": "Hledat v otevřených datech", "de": "Offene Daten durchsuchen", "hu": "Keresés a nyílt adatokban"},
    "hero_placeholder": {"en": "e.g. air quality, bikes, budget", "sk": "napr. kvalita ovzdušia, bicykle, rozpočet", "cs": "např. kvalita ovzduší, kola, rozpočet", "de": "z. B. Luftqualität, Fahrräder, Haushalt", "hu": "pl. levegőminőség, kerékpárok, költségvetés"},
    "search": {"en": "Search", "sk": "Hľadať", "cs": "Hledat", "de": "Suchen", "hu": "Keresés"},
    "datasets": {"en": "datasets", "sk": "datasetov", "cs": "datových sad", "de": "Datensätze", "hu": "adatkészlet"},
    "publishers": {"en": "publishers", "sk": "vydavateľov", "cs": "vydavatelů", "de": "Herausgeber", "hu": "kiadó"},
    "recent": {"en": "Recently updated", "sk": "Nedávno aktualizované", "cs": "Nedávno aktualizované", "de": "Kürzlich aktualisiert", "hu": "Nemrég frissítve"},
    "all_datasets": {"en": "All datasets", "sk": "Všetky datasety", "cs": "Všechny datové sady", "de": "Alle Datensätze", "hu": "Összes adatkészlet"},
    "by_publisher": {"en": "By publisher", "sk": "Podľa vydavateľa", "cs": "Podle vydavatele", "de": "Nach Herausgeber", "hu": "Kiadók szerint"},
    "by_keyword": {"en": "Popular keywords", "sk": "Časté kľúčové slová", "cs": "Častá klíčová slova", "de": "Häufige Schlagwörter", "hu": "Népszerű kulcsszavak"},
    "updated": {"en": "Updated", "sk": "Aktualizované", "cs": "Aktualizováno", "de": "Aktualisiert", "hu": "Frissítve"},
    # The About page, the publisher and topic helpers and the header's words (T-3028).
    "nav_datasets": {"en": "Datasets", "sk": "Datasety", "cs": "Datové sady", "de": "Datensätze", "hu": "Adatkészletek"},
    "nav_publishers": {"en": "Publishers", "sk": "Vydavatelia", "cs": "Vydavatelé", "de": "Herausgeber", "hu": "Kiadók"},
    "nav_topics": {"en": "Topics", "sk": "Témy", "cs": "Témata", "de": "Themen", "hu": "Témák"},
    "subscribe": {"en": "Subscribe", "sk": "Odoberať", "cs": "Odebírat", "de": "Abonnieren", "hu": "Feliratkozás"},
    "subscribe_calendar": {"en": "Calendar", "sk": "Kalendár", "cs": "Kalendář", "de": "Kalender", "hu": "Naptár"},
    "subscribe_news": {"en": "News", "sk": "Novinky", "cs": "Novinky", "de": "Neuigkeiten", "hu": "Hírek"},
    "subscribe_hint": {
        "en": "Copy an address into a feed reader or a calendar, for example Feedly or Google Calendar; new items appear by themselves.",
        "sk": "Skopírujte adresu do čítačky noviniek alebo kalendára, napríklad Feedly alebo Google Kalendár; nové položky sa objavia samy.",
        "cs": "Zkopírujte adresu do čtečky novinek nebo kalendáře, například Feedly nebo Google Kalendář; nové položky se objeví samy.",
        "de": "Kopieren Sie eine Adresse in einen Feedreader oder Kalender, zum Beispiel Feedly oder Google Kalender; neue Einträge erscheinen von selbst.",
        "hu": "Másolja a címet egy hírolvasóba vagy naptárba, például a Feedlybe vagy a Google Naptárba; az új elemek maguktól megjelennek.",
    },
    "search_publishers": {"en": "Search publishers…", "sk": "Hľadať vydavateľov…",
                          "cs": "Hledat vydavatele…", "de": "Herausgeber suchen…", "hu": "Kiadók keresése…"},
    "search_topics": {"en": "Search topics…", "sk": "Hľadať témy…", "cs": "Hledat témata…",
                      "de": "Themen suchen…", "hu": "Témák keresése…"},
    "nav_about": {"en": "About", "sk": "O katalógu", "cs": "O katalogu", "de": "Über den Katalog", "hu": "Névjegy"},
    "try_it": {"en": "Try it", "sk": "Vyskúšajte", "cs": "Vyzkoušejte", "de": "Probieren Sie es aus", "hu": "Próbálja ki"},
    "publishers_help": {
        "en": "The cities, regions and offices whose data this catalogue carries. Open one to see all of its datasets.",
        "sk": "Mestá, kraje a úrady, ktorých dáta tento katalóg obsahuje. Otvorte jedného a uvidíte všetky jeho datasety.",
        "cs": "Města, kraje a úřady, jejichž data tento katalog obsahuje. Otevřete jednoho a uvidíte všechny jeho datové sady.",
        "de": "Die Städte, Regionen und Ämter, deren Daten dieser Katalog enthält. Öffnen Sie einen, um alle seine Datensätze zu sehen.",
        "hu": "Azok a városok, régiók és hivatalok, amelyek adatait ez a katalógus tartalmazza. Nyisson meg egyet, és lássa az összes adatkészletét.",
    },
    "topics_help": {
        "en": "A topic gathers the datasets on one subject from every publisher.",
        "sk": "Téma spája datasety o jednej veci od všetkých vydavateľov.",
        "cs": "Téma spojuje datové sady o jedné věci od všech vydavatelů.",
        "de": "Ein Thema bündelt die Datensätze zu einem Gegenstand von allen Herausgebern.",
        "hu": "Egy téma minden kiadótól összegyűjti az egy tárgykörbe tartozó adatkészleteket.",
    },
    "about_intro_org": {
        "en": "{name} is the open data catalogue of {org}.",
        "sk": "{name} je katalóg otvorených dát: {org}.",
        "cs": "{name} je katalog otevřených dat: {org}.",
        "de": "{name} ist der Katalog offener Daten von {org}.",
        "hu": "A(z) {name} a(z) {org} nyílt adatkatalógusa.",
    },
    "about_intro": {
        "en": "{name} is an open data catalogue.",
        "sk": "{name} je katalóg otvorených dát.",
        "cs": "{name} je katalog otevřených dat.",
        "de": "{name} ist ein Katalog offener Daten.",
        "hu": "A(z) {name} egy nyílt adatkatalógus.",
    },
    "about_data": {
        "en": "Every dataset comes straight from the platform's live data. Open one to browse and search its rows, download it as files, or query it live through its API.",
        "sk": "Každý dataset pochádza priamo zo živých dát platformy. Otvorte ho a prezerajte či vyhľadávajte v jeho riadkoch, stiahnite si ho ako súbory alebo sa naň pýtajte naživo cez jeho API.",
        "cs": "Každá datová sada pochází přímo z živých dat platformy. Otevřete ji a procházejte či vyhledávejte v jejích řádcích, stáhněte si ji jako soubory nebo se na ni ptejte živě přes její API.",
        "de": "Jeder Datensatz stammt direkt aus den Live-Daten der Plattform. Öffnen Sie ihn, um seine Zeilen zu durchsuchen, ihn als Dateien herunterzuladen oder ihn live über seine API abzufragen.",
        "hu": "Minden adatkészlet közvetlenül a platform élő adataiból származik. Nyisson meg egyet, és böngéssze, keresse a sorait, töltse le fájlokként, vagy kérdezze le élőben az API-ján keresztül.",
    },
    "about_machines": {"en": "For developers and other catalogues", "sk": "Pre vývojárov a iné katalógy", "cs": "Pro vývojáře a jiné katalogy", "de": "Für Entwickler und andere Kataloge", "hu": "Fejlesztőknek és más katalógusoknak"},
    "about_api": {
        "en": "Every dataset and its description as JSON, through CKAN's Action API.",
        "sk": "Všetky datasety a ich opis ako JSON, cez Action API CKAN.",
        "cs": "Všechny datové sady a jejich popis jako JSON, přes Action API CKAN.",
        "de": "Alle Datensätze und ihre Beschreibung als JSON, über die Action-API von CKAN.",
        "hu": "Minden adatkészlet és leírása JSON formátumban, a CKAN Action API-ján keresztül.",
    },
    "about_dcat": {"en": "DCAT-AP catalogue", "sk": "Katalóg DCAT-AP", "cs": "Katalog DCAT-AP", "de": "DCAT-AP-Katalog", "hu": "DCAT-AP katalógus"},
    "about_dcat_what": {
        "en": "The whole catalogue as DCAT-AP, for harvesting by national and European data portals.",
        "sk": "Celý katalóg vo formáte DCAT-AP, na zber národnými a európskymi dátovými portálmi.",
        "cs": "Celý katalog ve formátu DCAT-AP, pro sklízení národními a evropskými datovými portály.",
        "de": "Der ganze Katalog als DCAT-AP, zum Einsammeln durch nationale und europäische Datenportale.",
        "hu": "Az egész katalógus DCAT-AP formátumban, a nemzeti és európai adatportálok általi begyűjtéshez.",
    },
    "about_contact": {"en": "Questions about the data", "sk": "Otázky k dátam", "cs": "Dotazy k datům", "de": "Fragen zu den Daten", "hu": "Kérdések az adatokról"},
    "more": {"en": "More details", "sk": "Ďalšie údaje", "cs": "Další údaje", "de": "Weitere Angaben", "hu": "További részletek"},
    # A resource page (T-4333): what it is, its address, its live rows and how to use it.
    "copy_address": {"en": "Copy address", "sk": "Kopírovať adresu", "cs": "Kopírovat adresu", "de": "Adresse kopieren", "hu": "Cím másolása"},
    "full_address": {"en": "The whole address", "sk": "Celá adresa", "cs": "Celá adresa", "de": "Die ganze Adresse", "hu": "A teljes cím"},
    "entity_type": {"en": "Data type", "sk": "Typ dát", "cs": "Typ dat", "de": "Datentyp", "hu": "Adattípus"},
    "scope_dataset": {"en": "Only this dataset's records.", "sk": "Iba záznamy tohto datasetu.", "cs": "Pouze záznamy této datové sady.", "de": "Nur die Einträge dieses Datensatzes.", "hu": "Csak ennek az adatkészletnek a rekordjai."},
    "scope_type": {
        "en": "Every {type} record of the publisher's open data, this dataset's among them.",
        "sk": "Všetky záznamy typu {type} v otvorených dátach vydavateľa, vrátane tohto datasetu.",
        "cs": "Všechny záznamy typu {type} v otevřených datech vydavatele, včetně této datové sady.",
        "de": "Alle {type}-Einträge der offenen Daten des Herausgebers, auch die dieses Datensatzes.",
        "hu": "A kiadó nyílt adatainak minden {type} rekordja, köztük ennek az adatkészletnek a rekordjai.",
    },
    "preview_types": {"en": "The kinds of entities it holds, live", "sk": "Druhy entít, ktoré obsahuje, naživo", "cs": "Druhy entit, které obsahuje, živě", "de": "Die Arten von Entitäten, live", "hu": "Milyen entitásokat tartalmaz, élőben"},
    "preview_api": {"en": "The first records, live", "sk": "Prvé záznamy naživo", "cs": "První záznamy živě", "de": "Die ersten Einträge, live", "hu": "Az első rekordok, élőben"},
    "preview_loading": {"en": "Loading the first records…", "sk": "Načítavajú sa prvé záznamy…", "cs": "Načítají se první záznamy…", "de": "Die ersten Einträge werden geladen…", "hu": "Az első rekordok betöltődnek…"},
    "preview_failed": {
        "en": "The records did not load here; the address still answers a program.",
        "sk": "Záznamy sa tu nenačítali; adresa programu odpovedá aj tak.",
        "cs": "Záznamy se zde nenačetly; adresa programu odpovídá i tak.",
        "de": "Die Einträge wurden hier nicht geladen; die Adresse antwortet einem Programm trotzdem.",
        "hu": "A rekordok itt nem töltődtek be; a cím egy programnak továbbra is válaszol.",
    },
    "preview_empty": {"en": "No records yet.", "sk": "Zatiaľ žiadne záznamy.", "cs": "Zatím žádné záznamy.", "de": "Noch keine Einträge.", "hu": "Még nincsenek rekordok."},
    "preview_shown": {"en": "{shown} of {total} records", "sk": "{shown} z {total} záznamov", "cs": "{shown} z {total} záznamů", "de": "{shown} von {total} Einträgen", "hu": "{shown} / {total} rekord"},
    "records": {"en": "Records", "sk": "Počet záznamov", "cs": "Počet záznamů", "de": "Einträge", "hu": "Rekordok"},
    "ex_first": {"en": "The first 5 records, with curl", "sk": "Prvých 5 záznamov cez curl", "cs": "Prvních 5 záznamů přes curl", "de": "Die ersten 5 Einträge mit curl", "hu": "Az első 5 rekord curl-lel"},
    "ex_file": {"en": "Download it with curl", "sk": "Stiahnutie cez curl", "cs": "Stažení přes curl", "de": "Mit curl herunterladen", "hu": "Letöltés curl-lel"},
    "ex_excel": {
        "en": "In Excel or Power BI: Data, Get Data, From OData Feed, with this address",
        "sk": "V Exceli alebo Power BI: Údaje, Získať údaje, Z informačného kanála OData, s touto adresou",
        "cs": "V Excelu nebo Power BI: Data, Načíst data, Z informačního kanálu OData, s touto adresou",
        "de": "In Excel oder Power BI: Daten, Daten abrufen, Aus OData-Feed, mit dieser Adresse",
        "hu": "Excelben vagy Power BI-ban: Adatok, Adatok beolvasása, OData-hírcsatornából, ezzel a címmel",
    },
    "ex_qgis": {
        "en": "In QGIS: Layer, Add Layer, Add WFS / OGC API - Features Layer, a new connection with this address",
        "sk": "V QGIS: Vrstva, Pridať vrstvu, Pridať vrstvu WFS / OGC API - Features, nové pripojenie s touto adresou",
        "cs": "V QGIS: Vrstva, Přidat vrstvu, Přidat vrstvu WFS / OGC API - Features, nové připojení s touto adresou",
        "de": "In QGIS: Layer, Layer hinzufügen, WFS / OGC API - Features-Layer hinzufügen, eine neue Verbindung mit dieser Adresse",
        "hu": "QGIS-ben: Réteg, Réteg hozzáadása, WFS / OGC API - Features réteg hozzáadása, új kapcsolat ezzel a címmel",
    },
    "ex_ws": {
        "en": "Changes as they happen, with websocat: open the socket, then send the subscription as the first line",
        "sk": "Zmeny naživo cez websocat: otvorte spojenie a ako prvý riadok pošlite odber",
        "cs": "Změny živě přes websocat: otevřete spojení a jako první řádek pošlete odběr",
        "de": "Änderungen live mit websocat: Verbindung öffnen, dann das Abonnement als erste Zeile senden",
        "hu": "Változások valós időben websocattal: nyissa meg a socketet, majd első sorként küldje el a feliratkozást",
    },
    "ex_chatgpt_title": {"en": "ChatGPT", "sk": "ChatGPT", "cs": "ChatGPT", "de": "ChatGPT", "hu": "ChatGPT"},
    "ex_chatgpt": {
        "en": "Open Settings, then Apps and Connectors, and choose Create. Paste this address as the MCP server URL, with no authentication.",
        "sk": "Otvorte Nastavenia, potom Aplikácie a konektory, a zvoľte Vytvoriť. Vložte túto adresu ako URL MCP servera, bez overenia.",
        "cs": "Otevřete Nastavení, pak Aplikace a konektory, a zvolte Vytvořit. Vložte tuto adresu jako URL MCP serveru, bez ověření.",
        "de": "Öffnen Sie Einstellungen, dann Apps und Konnektoren, und wählen Sie Erstellen. Fügen Sie diese Adresse als URL des MCP-Servers ein, ohne Authentifizierung.",
        "hu": "Nyissa meg a Beállítások, majd az Alkalmazások és csatlakozók menüt, és válassza a Létrehozás lehetőséget. Illessze be ezt a címet MCP-szerver URL-ként, hitelesítés nélkül.",
    },
    "created": {"en": "Created", "sk": "Vytvorené", "cs": "Vytvořeno", "de": "Erstellt", "hu": "Létrehozva"},
    "info": {"en": "About this resource", "sk": "O tomto zdroji", "cs": "O tomto zdroji", "de": "Über diese Ressource", "hu": "Az erőforrásról"},
    "original_table": {"en": "The publisher's original table", "sk": "Pôvodná tabuľka vydavateľa", "cs": "Původní tabulka vydavatele", "de": "Die Originaltabelle des Herausgebers", "hu": "A kiadó eredeti táblázata"},
    "records_csv": {"en": "All records as a table", "sk": "Všetky záznamy ako tabuľka", "cs": "Všechny záznamy jako tabulka", "de": "Alle Einträge als Tabelle", "hu": "Minden rekord táblázatként"},
    "records_geojson": {"en": "All records for a map", "sk": "Všetky záznamy na mapu", "cs": "Všechny záznamy na mapu", "de": "Alle Einträge für eine Karte", "hu": "Minden rekord térképhez"},
    "records_json": {"en": "All records for programs", "sk": "Všetky záznamy pre programy", "cs": "Všechny záznamy pro programy", "de": "Alle Einträge für Programme", "hu": "Minden rekord programokhoz"},
    "records_xlsx": {"en": "All records for Excel", "sk": "Všetky záznamy pre Excel", "cs": "Všechny záznamy pro Excel", "de": "Alle Einträge für Excel", "hu": "Minden rekord Excelhez"},
    "records_parquet": {"en": "All records for pandas, Polars or DuckDB", "sk": "Všetky záznamy pre pandas, Polars alebo DuckDB", "cs": "Všechny záznamy pro pandas, Polars nebo DuckDB", "de": "Alle Einträge für pandas, Polars oder DuckDB", "hu": "Minden rekord pandashoz, Polarshoz vagy DuckDB-hez"},
    "live_changes": {"en": "Live changes", "sk": "Zmeny naživo", "cs": "Změny živě", "de": "Änderungen live", "hu": "Élő változások"},
    "ex_types": {"en": "The kinds of entities it holds, with curl", "sk": "Druhy entít, ktoré obsahuje, cez curl", "cs": "Druhy entit, které obsahuje, přes curl", "de": "Die Arten von Entitäten, die sie enthält, mit curl", "hu": "Milyen entitásokat tartalmaz, curl-lel"},
    "section_source": {"en": "At the publisher", "sk": "U vydavateľa", "cs": "U vydavatele", "de": "Beim Herausgeber", "hu": "A kiadónál"},
    "source_lead": {
        "en": "This is the publisher's own page for the data; it opens on their site.",
        "sk": "Toto je stránka vydavateľa s týmito dátami; otvorí sa na jeho webe.",
        "cs": "Toto je stránka vydavatele s těmito daty; otevře se na jeho webu.",
        "de": "Dies ist die Seite des Herausgebers zu diesen Daten; sie öffnet sich auf seiner Website.",
        "hu": "Ez a kiadó saját oldala az adatokhoz; az ő webhelyén nyílik meg.",
    },
    "section_source_lead": {"en": "Where the publisher keeps the original.", "sk": "Kde má vydavateľ originál.", "cs": "Kde má vydavatel originál.", "de": "Wo der Herausgeber das Original hat.", "hu": "Ahol a kiadó az eredetit tárolja."},
    "at_publisher": {"en": "On the publisher's site", "sk": "Na stránke vydavateľa", "cs": "Na stránce vydavatele", "de": "Auf der Seite des Herausgebers", "hu": "A kiadó oldalán"},
}

# What a reader gets from each format, in one line (T-2744). A format not listed gets none.
FORMATS = {
    "NGSI-LD": {"en": "Live entities through the API", "sk": "Živé entity cez API", "cs": "Živé entity přes API", "de": "Live-Entitäten über die API", "hu": "Élő entitások az API-n keresztül"},
    "JSON-LD": {"en": "Linked data with its context", "sk": "Prepojené dáta s kontextom", "cs": "Propojená data s kontextem", "de": "Verknüpfte Daten mit Kontext", "hu": "Kapcsolt adatok a kontextusukkal"},
    "GEOJSON": {"en": "Points and shapes for a map", "sk": "Body a tvary pre mapu", "cs": "Body a tvary pro mapu", "de": "Punkte und Formen für eine Karte", "hu": "Pontok és alakzatok térképhez"},
    "CSV": {"en": "A table for a spreadsheet", "sk": "Tabuľka pre tabuľkový procesor", "cs": "Tabulka pro tabulkový procesor", "de": "Eine Tabelle für die Tabellenkalkulation", "hu": "Táblázat táblázatkezelőhöz"},
    "JSON": {"en": "Machine-readable JSON", "sk": "Strojovo čitateľný JSON", "cs": "Strojově čitelný JSON", "de": "Maschinenlesbares JSON", "hu": "Géppel olvasható JSON"},
    "XLSX": {"en": "A spreadsheet", "sk": "Tabuľkový súbor", "cs": "Tabulkový soubor", "de": "Eine Tabellendatei", "hu": "Táblázatfájl"},
    "PARQUET": {"en": "Typed columns for data analysis (Apache Parquet)", "sk": "Typované stĺpce pre analýzu dát (Apache Parquet)", "cs": "Typované sloupce pro analýzu dat (Apache Parquet)", "de": "Typisierte Spalten für die Datenanalyse (Apache Parquet)", "hu": "Típusos oszlopok adatelemzéshez (Apache Parquet)"},
    "ZIP": {"en": "Everything in one download", "sk": "Všetko v jednom súbore", "cs": "Vše v jednom souboru", "de": "Alles in einem Download", "hu": "Minden egy letöltésben"},
    "WEBSOCKET": {"en": "Live updates as they happen (WebSocket)", "sk": "Zmeny naživo, hneď ako nastanú (WebSocket)", "cs": "Změny živě, hned jak nastanou (WebSocket)", "de": "Änderungen live, sobald sie geschehen (WebSocket)", "hu": "Változások valós időben (WebSocket)"},
    "OGCFEAT": {"en": "Map layers for GIS (OGC API - Features)", "sk": "Mapové vrstvy pre GIS (OGC API - Features)", "cs": "Mapové vrstvy pro GIS (OGC API - Features)", "de": "Kartenebenen für GIS (OGC API - Features)", "hu": "Térképrétegek GIS-hez (OGC API - Features)"},
    "ODATA": {"en": "For Excel and Power BI (OData)", "sk": "Pre Excel a Power BI (OData)", "cs": "Pro Excel a Power BI (OData)", "de": "Für Excel und Power BI (OData)", "hu": "Excelhez és Power BI-hoz (OData)"},
    "STA": {"en": "Sensors and observations (SensorThings API)", "sk": "Senzory a merania (SensorThings API)", "cs": "Senzory a měření (SensorThings API)", "de": "Sensoren und Messungen (SensorThings API)", "hu": "Érzékelők és mérések (SensorThings API)"},
    "MCP": {"en": "For AI assistants (Model Context Protocol)", "sk": "Pre AI asistentov (Model Context Protocol)", "cs": "Pro AI asistenty (Model Context Protocol)", "de": "Für KI-Assistenten (Model Context Protocol)", "hu": "AI-asszisztenseknek (Model Context Protocol)"},
    "LINKML": {"en": "The data model", "sk": "Dátový model", "cs": "Datový model", "de": "Das Datenmodell", "hu": "Az adatmodell"},
    "SHACL": {"en": "Validation rules of the data model", "sk": "Pravidlá kontroly dátového modelu", "cs": "Pravidla kontroly datového modelu", "de": "Prüfregeln des Datenmodells", "hu": "Az adatmodell ellenőrzési szabályai"},
}

# The EU frequency vocabulary, publications.europa.eu/resource/authority/frequency, as a label
# and the days to the next update. Irregular and unknown frequencies promise no date.
FREQUENCIES = {
    "CONT": ({"en": "Continuously", "sk": "Priebežne", "cs": "Průběžně", "de": "Laufend", "hu": "Folyamatosan"}, None),
    "UPDATE_CONT": ({"en": "Continuously", "sk": "Priebežne", "cs": "Průběžně", "de": "Laufend", "hu": "Folyamatosan"}, None),
    "HOURLY": ({"en": "Every hour", "sk": "Každú hodinu", "cs": "Každou hodinu", "de": "Stündlich", "hu": "Óránként"}, 1 / 24),
    "DAILY": ({"en": "Every day", "sk": "Denne", "cs": "Denně", "de": "Täglich", "hu": "Naponta"}, 1),
    "WEEKLY": ({"en": "Every week", "sk": "Týždenne", "cs": "Týdně", "de": "Wöchentlich", "hu": "Hetente"}, 7),
    "MONTHLY": ({"en": "Every month", "sk": "Mesačne", "cs": "Měsíčně", "de": "Monatlich", "hu": "Havonta"}, 30),
    "QUARTERLY": ({"en": "Every quarter", "sk": "Štvrťročne", "cs": "Čtvrtletně", "de": "Vierteljährlich", "hu": "Negyedévente"}, 91),
    "ANNUAL": ({"en": "Every year", "sk": "Ročne", "cs": "Ročně", "de": "Jährlich", "hu": "Évente"}, 365),
    "IRREG": ({"en": "Irregularly", "sk": "Nepravidelne", "cs": "Nepravidelně", "de": "Unregelmäßig", "hu": "Rendszertelenül"}, None),
}

# The extras the About list reads by name, in the order a reader sees them. Everything else
# the publisher wrote stays under "More details"; the internal ones are never shown.
ABOUT_EXTRAS = ("identifier", "language", "contact", "conforms_to", "status")
# `license_url` and `theme` are DCAT metadata for harvesters (T-3025): the EU licence URI beside
# the licence the About list already links, and the themes as a JSON list of vocabulary URIs.
INTERNAL_EXTRAS = {"generated_by", "endpoint", "publisher_name", "publisher_uri", "contact_name",
                   "contact_email", "frequency", "spatial", "spatial_uri", "temporal_start",
                   "temporal_end", "theme", "license_url", "harvest_source", "rights", "source"}
# What a harvested record says about itself, in words (T-4333): the dates the publisher gave and
# where the record came from. Any other extra keeps its own key.
EXTRA_LABELS = {
    "issued": {"en": "Published by the publisher", "sk": "Zverejnené vydavateľom", "cs": "Zveřejněno vydavatelem", "de": "Vom Herausgeber veröffentlicht", "hu": "A kiadó közzétette"},
    "modified": {"en": "Changed by the publisher", "sk": "Zmenené vydavateľom", "cs": "Změněno vydavatelem", "de": "Vom Herausgeber geändert", "hu": "A kiadó módosította"},
    "quality_pass_rate": {"en": "Passed the quality checks", "sk": "Prešlo kontrolami kvality", "cs": "Prošlo kontrolami kvality", "de": "Qualitätsprüfungen bestanden", "hu": "Megfelelt a minőségi ellenőrzéseken"},
    "source_uri": {"en": "Original record", "sk": "Pôvodný záznam", "cs": "Původní záznam", "de": "Ursprünglicher Eintrag", "hu": "Eredeti rekord"},
    "was_generated_by": {"en": "Produced by", "sk": "Vytvoril", "cs": "Vytvořil", "de": "Erzeugt von", "hu": "Előállította"},
}


def _language():
    try:
        code = toolkit.h.lang() or "en"
    except (AttributeError, RuntimeError):
        code = "en"
    return code.split("_")[0].split("-")[0]


def jc_t(key):
    """One of the theme's words in the page's language, English when there is none."""
    words = STRINGS.get(key) or {}
    return words.get(_language()) or words.get("en") or key


def _localized(values, language):
    """`values[language]`, else the catalogue's default language's, else the first; a plain value
    is the same in every language. None when nothing usable is there."""
    if not isinstance(values, dict):
        return values or None
    default = (BRANDING.get("languages") or {}).get("default")
    for code in (language, default):
        if values.get(code):
            return values[code]
    return next((value for value in values.values() if value), None)


def jc_site_title(language=None):
    """The catalogue's name in the page's language (T-4636): its instance's `site.title`, else the
    installation's name."""
    title = _localized(BRANDING.get("siteTitle"), language or _language())
    return title if isinstance(title, str) else BRANDING.get("instanceName") or "joinedcontext"


def jc_site_tagline(language=None):
    """The line under the home page's title (T-4636): the instance's `site.tagline` in the page's
    language, else the installation's tagline, else the theme's."""
    tagline = _localized(BRANDING.get("siteTagline"), language or _language())
    if isinstance(tagline, str):
        return tagline
    return BRANDING.get("tagline") or jc_t("tagline")


def jc_site_logo():
    """The header's image on this origin (T-4636): the instance's `site.logo`, a file in the
    theme's public directory, else the joinedcontext.com lockup (T-4627)."""
    logo = BRANDING.get("siteLogo")
    if isinstance(logo, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.(svg|png|webp)", logo):
        return "/" + logo
    return "/jc-logo.svg"


def _samples(language):
    """The instance's sample questions for the page's language (T-4636): strings only."""
    found = _localized(BRANDING.get("samples"), language)
    return [q for q in found if isinstance(q, str) and q.strip()] if isinstance(found, list) else []


def _extras(pkg):
    return {e.get("key"): e.get("value") for e in (pkg or {}).get("extras") or [] if e.get("key")}


def _is_url(value):
    """An absolute http(s) address both a page link and the DCAT writer can carry: one space,
    quote or angle bracket in an IRI makes rdflib refuse the whole Turtle document, so a single
    record would turn /catalog.ttl into a 500 for every dataset (T-4586, T-6218)."""
    return isinstance(value, str) and value.startswith(("https://", "http://")) \
        and not re.search(r'[\s"<>{}|\\^`]', value)


def _sources(value):
    """(address, title, description) of each http source in a `source` extra jcctl writes: a JSON
    list of `{url, title, description}` (T-4436) or of addresses, or one address. A source without
    a title is titled by its address, one without a description described by its title."""
    try:
        items = json.loads(value) if isinstance(value, str) and value.startswith("[") else [value]
    except ValueError:
        return []
    out = []
    for item in items if isinstance(items, list) else []:
        url = item.get("url") if isinstance(item, dict) else item
        if not _is_url(url):
            continue
        title = item.get("title") if isinstance(item, dict) else None
        title = title if isinstance(title, str) and title else url
        description = item.get("description") if isinstance(item, dict) else None
        out.append((url, title, description if isinstance(description, str) and description else title))
    return out


_DAY = datetime.timedelta(days=1)


def _parse(stamp):
    """CKAN's `metadata_modified`, naive UTC without a zone, as an aware datetime."""
    parsed = datetime.datetime.fromisoformat(str(stamp).replace("Z", "+00:00"))
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=datetime.timezone.utc)


def _when(pkg, extra, own):
    """The extra's date when it reads as one, else CKAN's own field, as an aware datetime."""
    pkg = pkg or {}
    for value in (_extras(pkg).get(extra), pkg.get(own)):
        try:
            if value:
                return _parse(value)
        except ValueError:
            continue
    return None


def jc_created(pkg):
    """When the dataset first appeared (T-4644): the publisher's own `issued` (a harvested record,
    or ours restored after the 15-17 Oct re-creation), else when CKAN created it."""
    return _when(pkg, "issued", "metadata_created")


def jc_updated(pkg):
    """When its data or description last changed (T-4644): the publisher's `modified` for a
    harvested record, else CKAN's own, which a live Endpoint's DataStore mirror moves with every
    data change and a run that changed nothing leaves alone."""
    return _when(pkg, "modified", "metadata_modified")


def jc_frequency(pkg):
    """(label, next update as a datetime or None) of the dataset's frequency; None when unset."""
    value = _extras(pkg).get("frequency")
    if not value:
        return None
    code = str(value).rstrip("/").rsplit("/", 1)[-1].upper()
    known = FREQUENCIES.get(code)
    if not known:
        return (str(value), None)
    words, days = known
    label = words.get(_language()) or words["en"]
    last = jc_updated(pkg)
    if days is None or not last:
        return (label, None)
    return (label, last + datetime.timedelta(days=days))


# The NUTS 3 regions of the bodies on the platform, by code and language (T-4333); any other code
# reads as its code. A data file beside this one, so no country's names sit in the code (T-4636).
# ponytail: a handful of names, the Eurostat NUTS list when more countries publish.
def _nuts_names():
    try:
        with open(os.path.join(os.path.dirname(__file__), "nuts.json"), encoding="utf-8") as handle:
            names = json.load(handle)
    except (OSError, ValueError):
        return {}
    return names if isinstance(names, dict) else {}


NUTS_NAMES = _nuts_names()


def jc_about(pkg):
    """The rows of "About this dataset" as (label, text, link or None), empty ones left out."""
    pkg = pkg or {}
    extras = _extras(pkg)
    rows = []
    organization = pkg.get("organization") or {}
    publisher = extras.get("publisher_name") or organization.get("title") or organization.get("name")
    if publisher:
        link = extras.get("publisher_uri")
        rows.append((jc_t("publisher"), publisher, link if _is_url(link) else None))
    if pkg.get("license_title") or pkg.get("license_id"):
        link = pkg.get("license_url")
        rows.append((jc_t("licence"), pkg.get("license_title") or pkg.get("license_id"),
                     link if _is_url(link) else None))
    # The credit the licence asks for and the data it came from, as the Endpoint names them
    # (T-4417): `source` is the JSON list of IRIs ckanext-dcat reads as dct:source.
    if extras.get("rights"):
        rows.append((jc_t("attribution"), extras["rights"], None))
    for url, title, _ in _sources(extras.get("source")):
        rows.append((jc_t("source"), title, url))
    groups = [g.get("display_name") or g.get("title") or g.get("name") for g in pkg.get("groups") or []]
    if groups:
        rows.append((jc_t("topic"), ", ".join(g for g in groups if g), None))
    tags = [t.get("display_name") or t.get("name") for t in pkg.get("tags") or []]
    if tags:
        rows.append((jc_t("keywords"), ", ".join(t for t in tags if t), None))
    frequency = jc_frequency(pkg)
    if frequency:
        rows.append((jc_t("frequency"), frequency[0], None))
    created, updated = jc_created(pkg), jc_updated(pkg)
    if created:
        rows.append((jc_t("created"), _date(created.date().isoformat()), None))
    if updated:
        rows.append((jc_t("last_update"), _date(updated.date().isoformat()), None))
    if frequency and frequency[1]:
        # A dataset updated more than once a day promises an hour, not only a date.
        hourly = frequency[1] - updated < _DAY
        # CKAN writes a date with hours as "%B %d, %Y", English order with the page's month name;
        # the hour goes after the page's own date instead (T-4333).
        text = _date(frequency[1])
        rows.append((jc_t("next_update"), text + frequency[1].strftime(", %H:%M (UTC)") if hourly else text, None))
    coverage = extras.get("spatial_uri")
    if coverage:
        # An EU NUTS region reads by its name where the theme knows it and by its code, linked to
        # its definition (T-4333).
        nuts = re.search(r"/nuts/code/([A-Z]{2}[0-9A-Z]{0,3})$", coverage)
        text = coverage
        if nuts:
            names = NUTS_NAMES.get(nuts.group(1))
            text = "NUTS " + nuts.group(1)
            if names:
                text = "%s (%s)" % (names.get(_language()) or names["en"], text)
        rows.append((jc_t("coverage"), text, coverage if _is_url(coverage) else None))
    start, end = extras.get("temporal_start"), extras.get("temporal_end")
    if start or end:
        rows.append((jc_t("period"), " – ".join(v for v in (start, end) if v), None))
    contact = extras.get("contact_name") or extras.get("contact_email")
    if contact:
        email = extras.get("contact_email")
        rows.append((jc_t("contact"), contact, "mailto:" + email if email and "@" in email else None))
    for key in ("identifier", "language", "conforms_to", "status"):
        value = extras.get(key)
        if value:
            standard = jc_standard({"conforms_to": value}) if key == "conforms_to" else None
            rows.append((jc_t(key), standard["name"] if standard else value, value if _is_url(value) else None))
    return rows


def _date(value, hours=False):
    """A date (or a datetime) as CKAN writes dates in the page's language (T-4333); the ISO form
    where CKAN's helper is not there to ask."""
    try:
        return toolkit.h.render_datetime(value, with_hours=hours)
    except (AttributeError, TypeError, ValueError):
        if isinstance(value, datetime.datetime):
            return value.strftime("%Y-%m-%d %H:%M UTC" if hours else "%Y-%m-%d")
        return str(value)


def jc_more(pkg):
    """The extras the About list does not show, as (key, value), for "More details"."""
    # `issued` and `modified` are the About list's Created and Last update (T-4644).
    shown = set(ABOUT_EXTRAS) | INTERNAL_EXTRAS | {"identifier", "language", "conforms_to", "status", "issued", "modified"}
    return sorted((k, v) for k, v in _extras(pkg).items() if k not in shown and v)


def jc_extra(key, value):
    """One "More details" row as (label, text, link): a known key in words, an ISO date as the
    page writes dates, an http address as a link; anything else as it is."""
    words = EXTRA_LABELS.get(key)
    label = (words.get(_language()) or words["en"]) if words else str(key).replace("_", " ").capitalize()
    text = str(value)
    if key == "quality_pass_rate" and re.match(r"^(0(\.\d+)?|1(\.0+)?)$", text):
        text = "%d %%" % round(float(text) * 100)
    if re.match(r"^\d{4}-\d{2}-\d{2}(T[\d:.]+Z?)?$", text):
        try:
            text = toolkit.h.render_datetime(text.rstrip("Z"))
        except (AttributeError, TypeError, ValueError):
            pass
    return label, text, text if _is_url(text) else None


def _on_platform(host):
    """Whether `host` is the platform's domain or a subdomain of it; never without a domain."""
    domain = BRANDING.get("domain") or ""
    return bool(domain) and (host == domain or host.endswith("." + domain))


def jc_live(pkg):
    """The dataset's live NGSI-LD address and an example query, or None.

    Only an https address under the platform's own domain is shown: the extra is written by
    the publisher, and a page must never point a reader at an internal service address.
    """
    endpoint = _extras(pkg).get("endpoint")
    if not (isinstance(endpoint, str) and endpoint.startswith("https://")):
        return None
    if not _on_platform(endpoint[len("https://"):].split("/", 1)[0]):
        return None
    base = endpoint if endpoint.endswith("/") else endpoint + "/"
    # `types` answers without a filter; a query for entities needs a type the reader learns here.
    example = "curl -H 'Accept: application/ld+json' '%sngsi-ld/v1/types'" % base
    return {"url": base, "example": example}


# Where a resource belongs on the dataset page (T-3009). The model artifacts are published under
# the endpoint's `/schema/` path; the API formats answer queries rather than being files.
API_FORMATS = ("NGSI-LD", "WEBSOCKET", "MCP", "OGCFEAT", "ODATA", "STA", "DCAT")
DATA_ORDER = ("CSV", "GEOJSON", "JSON", "XLSX", "PARQUET", "ZIP")


# What an address of the platform is, read from its path under `/api/endpoint/{slug}/` (T-4333):
# the format a harvest wrote has been wrong (JSON-LD for NGSI-LD, JSON for MCP and WebSocket),
# the path never is. A file the Endpoint renders is `file.{ext}`; the model is under `schema/`.
_ENDPOINT_PATH = re.compile(r"^/api/endpoint/[^/]+/(.*)$")
_KIND_PREFIXES = (("ngsi-ld/", "NGSI-LD"), ("odata/", "ODATA"), ("ogc/", "OGCFEAT"), ("sta/", "STA"),
                  ("schema/", "MODEL"))
# An Endpoint's door by its kind: product names, the same on a page in any language (T-4333).
DOOR_NAMES = {"DCAT": "DCAT-AP record", "NGSI-LD": "NGSI-LD API", "WEBSOCKET": "WebSocket", "MCP": "Model Context Protocol",
              "OGCFEAT": "OGC API - Features", "ODATA": "OData v4", "STA": "SensorThings API",
              "CSV": "CSV", "JSON": "JSON", "GEOJSON": "GeoJSON", "XLSX": "Excel", "PARQUET": "Apache Parquet"}
# The format as a reader reads it, on a chip and in the facts list.
KIND_LABELS = {"DCAT": "DCAT-AP", "NGSI-LD": "NGSI-LD", "WEBSOCKET": "WebSocket", "MCP": "MCP", "OGCFEAT": "OGC API",
               "ODATA": "OData", "STA": "SensorThings", "CSV": "CSV", "JSON": "JSON",
               "GEOJSON": "GeoJSON", "XLSX": "Excel", "PARQUET": "Parquet"}


def _split(resource):
    """The resource's address in parts and its path below the Endpoint, or None off the platform."""
    url = (resource or {}).get("url") or ""
    try:
        parts = urlsplit(url)
    except ValueError:
        return None
    match = _ENDPOINT_PATH.match(parts.path or "")
    if parts.scheme not in ("http", "https", "ws", "wss") or not parts.netloc or not match:
        return None
    return parts, match.group(1)


def jc_kind(resource):
    """NGSI-LD, MCP, WEBSOCKET, ODATA, OGCFEAT, STA, MODEL, DCAT (the Endpoint's own record, EP-143), or a
    file's CSV, JSON, GEOJSON, XLSX or PARQUET for an Endpoint's address; None for anything else."""
    split = _split(resource)
    if not split:
        return None
    rest = split[1].rstrip("/")
    if not rest:
        return "DCAT"
    for prefix, kind in _KIND_PREFIXES:
        if rest.startswith(prefix) or rest == prefix.rstrip("/"):
            return kind
    if rest in ("mcp", "ws"):
        return "MCP" if rest == "mcp" else "WEBSOCKET"
    match = re.fullmatch(r"file\.(csv|json|geojson|xlsx|parquet)", rest)
    return match.group(1).upper() if match else None


def jc_is_source(resource, pkg):
    """Whether a resource is the harvest's link to the publisher's own page, named after the
    dataset (T-4333): it is opened there, never downloaded."""
    title = ((pkg or {}).get("title") or "").strip()
    return bool(title) and _off_platform(resource) and ((resource or {}).get("name") or "").strip() == title


def _section(resource):
    url = resource.get("url") or ""
    path = url.split("?", 1)[0]
    kind = jc_kind(resource)
    if kind == "MODEL" or "/schema/" in path:
        return "schema"
    if kind:
        return "api" if kind in API_FORMATS else "data"
    if (resource.get("format") or "").strip().upper() in API_FORMATS:
        return "api"
    return "data"


def _order(resource):
    """The DataStore table first (it is the data a reader came for), then files by format."""
    kind = jc_kind(resource)
    name = kind if kind and kind != "MODEL" else (resource.get("format") or "").strip().upper()
    known = DATA_ORDER.index(name) if name in DATA_ORDER else len(DATA_ORDER)
    api = API_FORMATS.index(name) if name in API_FORMATS else len(API_FORMATS)
    return (not resource.get("datastore_active"), known, api, name, resource.get("name") or "")


def jc_is_api(resource):
    """Whether a resource is an address a program calls rather than a file (T-3887)."""
    return _section(resource or {}) == "api"


# An address in a usage text, and a credential where `$TOKEN` belongs: a bearer value or a JWT.
_ADDRESS = re.compile(r"\b(?:https?|wss?)://([^/\s'\"<>`]+)")
_CREDENTIAL = re.compile(
    r"(?i)(?:bearer[ .]+(?!\$TOKEN\b)[A-Za-z0-9._~+/=-]{16,}|\beyJ[A-Za-z0-9_-]{8,}\.)")


def _docs_base():
    """The installation's documentation, from branding `documentationBaseUrl`: https only."""
    base = BRANDING.get("documentationBaseUrl")
    if not (isinstance(base, str) and base.startswith("https://") and _is_url(base)) \
            or re.search(r"[\s\"'<>]", base):
        return None
    return base.rstrip("/")


def _shown(text):
    """Whether a usage block may reach the page: every address it names is on the platform's
    domain or the documentation's host, and it carries no credential (T-3887, EP-110). The text is
    the publisher's, and a page must never show an internal service address or a token."""
    if _CREDENTIAL.search(text):
        return False
    docs = _docs_base()
    docs_host = docs[len("https://"):].split("/", 1)[0] if docs else None
    for host in _ADDRESS.findall(text):
        if not (_on_platform(host) or host == docs_host):
            return False
    return True


def jc_usage(resource):
    """An API resource's usage description as blocks for the page (T-3887, EP-108): paragraphs,
    and the indented code `jcctl` writes as fields with a copy button. A block that names a
    foreign address or holds a credential is left out; anything but an API resource has none."""
    resource = resource or {}
    text = resource.get("description")
    if not jc_is_api(resource) or not isinstance(text, str):
        return []
    key = re.sub(r"[^A-Za-z0-9_-]", "", str(resource.get("id") or "")) or "resource"
    blocks = []
    for number, chunk in enumerate(re.split(r"\n[ \t]*\n", text.strip("\n"))):
        lines = chunk.split("\n")
        code = all(line.startswith("    ") for line in lines if line.strip())
        body = "\n".join(line[4:] for line in lines) if code else " ".join(line.strip() for line in lines)
        if body.strip() and _shown(body):
            blocks.append({"code": code, "text": body, "id": "jc-use-%s-%d" % (key, number)})
    return blocks


def jc_guide(resource):
    """The WebSocket guide (API/07) for a WebSocket resource, when the installation names its
    documentation; None otherwise (T-3887)."""
    base = _docs_base()
    if base and ((resource or {}).get("format") or "").strip().upper() == "WEBSOCKET":
        return base + "/API/07-websocket"
    return None


def jc_resource_sections(pkg):
    """The resources in three sections, data, APIs and the data model, each in reading order
    and each resource with its one-line "what is this". Empty sections are left out."""
    sections = {"data": [], "api": [], "schema": [], "source": []}
    for resource in sorted((pkg or {}).get("resources") or [], key=_order):
        kind = jc_kind(resource)
        name = KIND_LABELS.get(kind) or (resource.get("format") or "").strip()
        words = FORMATS.get(kind or name.upper()) or {}
        if resource.get("datastore_active"):
            # A DataStore table downloads as CSV, whatever format its resource names (T-3012
            # writes its tables with none).
            name = name or "CSV"
            words = STRINGS["table_what"]
        sections["source" if jc_is_source(resource, pkg) else _section(resource)].append({
            "resource": resource,
            "format": name or "—",
            "what": words.get(_language()) or words.get("en") or "",
        })
    return [{"key": key, "title": jc_t("section_" + key), "lead": jc_t("section_" + key + "_lead"),
             "items": items} for key, items in sections.items() if items]


# The data model's files as `jcctl` publishes them (crates/jcctl/src/publish/ckan.rs): the
# dataset's name, a space and the file. A reader sees what each one is instead (T-3048), named
# once the "Data model" group around them has said what they belong to (T-4333).
MODEL_FILES = {
    "context.jsonld": {"en": "JSON-LD context", "sk": "Kontext JSON-LD", "cs": "Kontext JSON-LD", "de": "JSON-LD-Kontext", "hu": "JSON-LD kontextus"},
    "model.linkml.yaml": {"en": "LinkML", "sk": "LinkML", "cs": "LinkML", "de": "LinkML", "hu": "LinkML"},
    "model.md": {"en": "Described in words", "sk": "Popis slovami", "cs": "Popis slovy", "de": "In Worten beschrieben", "hu": "Szöveges leírás"},
    "model.owl.ttl": {"en": "OWL", "sk": "OWL", "cs": "OWL", "de": "OWL", "hu": "OWL"},
    "model.rdf.ttl": {"en": "RDF", "sk": "RDF", "cs": "RDF", "de": "RDF", "hu": "RDF"},
    "model.schema.json": {"en": "JSON Schema", "sk": "JSON Schema", "cs": "JSON Schema", "de": "JSON Schema", "hu": "JSON Schema"},
    "index.json": {"en": "List of its files", "sk": "Zoznam súborov", "cs": "Seznam souborů", "de": "Liste der Dateien", "hu": "A fájlok listája"},
    "model.shacl.ttl": {"en": "Validation rules", "sk": "Pravidlá kontroly", "cs": "Pravidla kontroly", "de": "Prüfregeln", "hu": "Ellenőrzési szabályok"},
}


def jc_resource_label(resource, pkg=None):
    """A resource's name as a reader reads it: a data model file in words, any other name
    without the dataset's own name in front of it, and the name as it is when that leaves nothing."""
    name = ((resource or {}).get("name") or "").strip()
    prefix = ((pkg or {}).get("name") or "").strip()
    rest = name[len(prefix):].strip() if prefix and name.startswith(prefix + " ") else name
    words = MODEL_FILES.get(rest)
    if jc_kind(resource) == "MODEL":
        # The harvest names a model file in English ("Data model: LinkML"); its file name says
        # which one it is in any language (T-4333).
        words = MODEL_FILES.get(((resource or {}).get("url") or "").split("?", 1)[0].rsplit("/", 1)[-1]) or words
    if words:
        return words.get(_language()) or words["en"]
    kind = jc_kind(resource)
    if kind in ("CSV", "JSON", "GEOJSON", "XLSX", "PARQUET") or kind == "WEBSOCKET":
        # The chip already says the format: the name says what is inside and what for, so the
        # platform's files never read as twins of each other (T-4333).
        return jc_t("records_" + kind.lower() if kind != "WEBSOCKET" else "live_changes")
    if kind in DOOR_NAMES:
        # An Endpoint's door is named by what it is, in words every language shares, with its
        # entity type once in the page header instead of on every entry: the harvest wrote
        # "NGSI-LD API (StatisticalObservation)", jcctl "Live updates over WebSocket" (T-4333).
        return DOOR_NAMES[kind]
    elif (resource or {}).get("datastore_active") and pkg and rest.startswith((pkg.get("title") or "\0").strip() + " ("):
        # A harvested table is named "{dataset} (tabuľka)", in Slovak on every page (T-4333).
        return jc_t("original_table")
    elif jc_is_source(resource, pkg):
        # The harvest names the publisher's own page after the dataset, so it read as a twin of
        # the table (T-4333).
        return jc_t("at_publisher")
    return rest or name or (resource or {}).get("format") or (resource or {}).get("id") or ""


def _off_platform(resource):
    """Whether a resource is a link to somewhere other than the platform and this catalogue."""
    resource = resource or {}
    if resource.get("datastore_active") or resource.get("url_type") == "upload":
        return False
    try:
        host = urlsplit(resource.get("url") or "").hostname or ""
    except ValueError:
        return False
    return bool(host) and not _on_platform(host)


def jc_entity_type(resource):
    """The entity type an Endpoint's address reads: its `type` query, or the type in an OData
    or OGC API path. None when it names none."""
    split = _split(resource)
    if not split:
        return None
    parts, rest = split
    found = parse_qs(parts.query).get("type")
    if found and found[0]:
        return found[0]
    match = re.match(r"^(?:odata/v4|ogc/features/collections|sta/v1\.1)/([^/?]+)", rest)
    if match:
        return match.group(1)
    # MCP and WebSocket read every type of the Endpoint; the harvest names the dataset's in
    # parentheses after the door ("WebSocket (StatisticalObservation)").
    named = re.search(r"\(([A-Za-z][A-Za-z0-9_]*)\)$", ((resource or {}).get("name") or "").strip())
    return named.group(1) if named else None


def _filtered(resource):
    """Whether the address keeps only this dataset's rows: the harvest's `q=isPartOf==...`, OGC API
    Features' `isPartOf=...`, OData's `$filter=isPartOf eq ...` (T-4289) or SensorThings'
    `... Thing/properties/isPartOf eq ...` (T-5205)."""
    split = _split(resource)
    if not split:
        return False
    query = parse_qs(split[0].query)
    return ("isPartOf" in query or any("isPartOf==" in q for q in query.get("q", []))
            or any(f.startswith("isPartOf eq ") or " and Thing/properties/isPartOf eq " in f
                   for f in query.get("$filter", [])))


def jc_what(resource):
    """One line, in the page's language, of what this resource is, and for an Endpoint's address
    whose rows it holds (T-4333): the harvest's English sentence never reaches the page."""
    kind = jc_kind(resource)
    words = FORMATS.get(kind or ((resource or {}).get("format") or "").strip().upper()) or {}
    line = words.get(_language()) or words.get("en") or ""
    kind_type = jc_entity_type(resource)
    if kind and kind != "MODEL" and kind_type:
        scope = jc_t("scope_dataset") if _filtered(resource) else jc_t("scope_type").replace("{type}", kind_type)
        line = (line + ". " if line else "") + scope
    return line if not line or line.endswith((".", "…")) else line + "."


def jc_format(resource):
    """The format a reader is shown: the kind of an Endpoint's address, CSV for a DataStore
    table, the format the resource names otherwise."""
    resource = resource or {}
    kind = jc_kind(resource)
    if kind in KIND_LABELS:
        return KIND_LABELS[kind]
    name = (resource.get("format") or "").strip()
    return name or ("CSV" if resource.get("datastore_active") else "")


def jc_address(resource):
    """The address as a person reads it (T-4333): host and path, the Endpoint's 32-character
    slug shortened, without the query a program needs; the whole address stays beside it."""
    split = _split(resource)
    url = (resource or {}).get("url") or ""
    if not split:
        return {"short": url, "full": url}
    parts, rest = split
    slug = parts.path[len("/api/endpoint/"):].split("/", 1)[0]
    short_slug = slug if len(slug) <= 10 else slug[:6] + "\u2026"
    short = "%s/api/endpoint/%s/%s" % (parts.netloc, short_slug, rest)
    # The page breaks it only after a slash, never inside a segment such as "ngsi-ld" (T-4333).
    return {"short": short, "segments": short.split("/"), "full": url}


def jc_pieces(text):
    """A command or address in the pieces a line may break between (T-4333): the spaces as they
    are, every other run cut after a slash, "?" or "&", so a line never breaks inside a word, a
    host, an id or "ngsi-ld". The page keeps each piece whole and scrolls one longer than a line."""
    pieces = []
    for token in re.split(r"(\s+)", str(text or "")):
        if not token:
            continue
        if token.isspace():
            pieces.append({"text": token, "space": True})
        else:
            pieces.extend({"text": part, "space": False} for part in re.findall(r"[^/?&]*(?:/+|[?&])|[^/?&]+", token))
    return pieces


def _with(url, **params):
    """The address with these query parameters set, the others kept in order."""
    parts = urlsplit(url)
    query = [(k, v) for k, values in parse_qs(parts.query, keep_blank_values=True).items()
             if k not in params for v in values]
    query += list(params.items())
    return urlunsplit(parts._replace(query=urlencode(query, quote_via=quote, safe="$")))


def jc_preview(resource):
    """The address the page's script fetches for its live preview (T-4333): five rows, read
    anonymously by the reader's browser from an https address of the platform. None for a kind
    without rows to show (MCP, WebSocket, a file) or an address elsewhere."""
    kind = jc_kind(resource)
    url = (resource or {}).get("url") or ""
    host = urlsplit(url).hostname or "" if url else ""
    if not (url.startswith("https://") and _on_platform(host)):
        return None
    path = urlsplit(url).path
    if kind == "NGSI-LD" and "/entities" in path:
        return {"url": _with(url, limit="5", options="keyValues", count="true"), "kind": kind}
    if kind == "NGSI-LD":
        # The API itself, without a type: what it holds is its list of types.
        return {"url": url.split("?", 1)[0].rstrip("/") + "/types", "kind": "NGSI-LD-TYPES"}
    if kind in ("ODATA", "STA") and jc_entity_type(resource):
        return {"url": _with(url, **{"$top": "5"}), "kind": kind}
    if kind == "OGCFEAT" and path.rstrip("/").endswith("/items"):
        return {"url": _with(url, limit="5"), "kind": kind}
    return None


def _sh(value):
    """`value` as one shell word in single quotes (T-5701): shlex.quote escapes a `'` in a
    resource address, which would otherwise end the quoting and run the rest in the reader's shell."""
    quoted = shlex.quote(value)
    return quoted if quoted.startswith("'") else f"'{quoted}'"


def jc_examples(resource):
    """How to use an Endpoint's address, in the page's language (T-4333): each a title, the
    steps in words and the command to copy. A non-public address carries `$TOKEN` in its
    description, so its commands send the header with it."""
    resource = resource or {}
    kind = jc_kind(resource)
    url = resource.get("url") or ""
    if not kind or kind == "MODEL" or not url:
        return []
    token = "$TOKEN" in (resource.get("description") or "")
    auth = ' -H "Authorization: Bearer $TOKEN"' if token else ""
    key = re.sub(r"[^A-Za-z0-9_-]", "", str(resource.get("id") or "")) or "resource"
    kind_type = jc_entity_type(resource) or ""
    out = []

    def add(title, code=None, steps=()):
        out.append({"title": title, "steps": list(steps), "code": code,
                    "id": "jc-ex-%s-%d" % (key, len(out))})

    if kind == "NGSI-LD" and "/entities" not in urlsplit(url).path:
        add(jc_t("ex_types"), "curl%s -H 'Accept: application/ld+json' %s" % (auth, _sh(url.split("?", 1)[0].rstrip("/") + "/types")))
    elif kind == "NGSI-LD":
        add(jc_t("ex_first"), "curl%s -H 'Accept: application/ld+json' %s" % (auth, _sh(_with(url, limit="5"))))
    elif kind in ("ODATA", "STA"):
        add(jc_t("ex_first"), "curl%s %s" % (auth, _sh(_with(url, **{"$top": "5"}))))
        if kind == "ODATA":
            add(jc_t("ex_excel"), url)
    elif kind == "OGCFEAT":
        add(jc_t("ex_first"), "curl%s %s" % (auth, _sh(_with(url, limit="5"))))
        add(jc_t("ex_qgis"), url.split("/collections/", 1)[0] + "/")
    elif kind == "MCP":
        add(jc_t("mcp_claude_ai"), url, [jc_t("mcp_claude_ai_1"), jc_t("mcp_claude_ai_2")])
        add(jc_t("ex_chatgpt_title"), url, [jc_t("ex_chatgpt")])
        add(jc_t("mcp_claude"), "claude mcp add --transport http joinedcontext %s" % shlex.quote(url))
    elif kind == "WEBSOCKET":
        body = {"type": "Subscription"}
        body["entities" if kind_type else "watchedAttributes"] = [{"type": kind_type}] if kind_type else ["location"]
        message = json.dumps({"metadata": {"x-op": "x-subscribe", "x-id": "s1"}, "body": body},
                             separators=(",", ":"), ensure_ascii=False)
        add(jc_t("ex_ws"), "websocat --protocol ngsi-ld-ws%s %s\n%s" % (auth, _sh(url), message))
    elif kind in ("CSV", "JSON", "GEOJSON", "XLSX", "PARQUET"):
        name = "data." + kind.lower()
        add(jc_t("ex_file"), "curl%s -L -o %s %s" % (auth, name, _sh(url)))
    return out


# The standards a distribution names in `conforms_to`, as a reader knows them.
STANDARDS = (
    ("etsi.org/deliver/etsi_gs/cim", "ETSI NGSI-LD (GS CIM 009)"),
    ("ogcapi-features", "OGC API - Features"),
    ("odata", "OData v4"),
    ("modelcontextprotocol", "Model Context Protocol"),
    ("rfc7946", "GeoJSON (RFC 7946)"),
    ("rfc4180", "CSV (RFC 4180)"),
    ("rfc8259", "JSON (RFC 8259)"),
    ("smart-data-models", "Smart Data Models"),
    ("w3.org/tr/json-ld11", "JSON-LD 1.1"),
    ("w3.org/tr/vocab-dcat", "DCAT"),
    ("smartdatamodels", "Smart Data Models"),
)


# The standard each kind of door follows, when the resource names none (jcctl's do not).
KIND_STANDARDS = {"CSV": "https://www.rfc-editor.org/rfc/rfc4180", "GEOJSON": "https://www.rfc-editor.org/rfc/rfc7946",
                  "JSON": "https://www.rfc-editor.org/rfc/rfc8259",
                  "NGSI-LD": "https://www.etsi.org/deliver/etsi_gs/CIM/001_099/009/",
                  "OGCFEAT": "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
                  "ODATA": "https://docs.oasis-open.org/odata/odata/v4.01/",
                  "MCP": "https://modelcontextprotocol.io/specification"}


def jc_standard(resource):
    """The standard a distribution follows, named, with its address; a file the platform renders
    follows its format's RFC. None when it names none or a value that is not an http address."""
    url = ((resource or {}).get("conforms_to") or "").strip() or KIND_STANDARDS.get(jc_kind(resource), "")
    if not _is_url(url):
        return None
    name = next((label for needle, label in STANDARDS if needle in url.lower()), url)
    return {"url": url, "name": name}


def jc_model(pkg):
    """The dataset's data model for a resource page (T-4333): the Smart Data Model a resource
    names, else the published JSON Schema, else None."""
    resources = (pkg or {}).get("resources") or []
    for resource in resources:
        standard = jc_standard(resource)
        if standard and standard["name"] == "Smart Data Models":
            return {"url": standard["url"], "name": "Smart Data Model"}
    for resource in resources:
        url = (resource.get("url") or "").split("?", 1)[0]
        if _section(resource) == "schema" and url.endswith("schema.json"):
            return {"url": resource["url"], "name": jc_resource_label(resource, pkg)}
    return None


def jc_dates(resource):
    """When the resource was updated, and when it was created only on another day (T-4333):
    three rows of one date said nothing three times."""
    resource = resource or {}
    updated = resource.get("last_modified") or resource.get("metadata_modified") or resource.get("created")
    rows = []
    if updated:
        rows.append((jc_t("updated"), toolkit.h.render_datetime(updated)))
    created = resource.get("created")
    if created and (not updated or str(created)[:10] != str(updated)[:10]):
        rows.append((jc_t("created"), toolkit.h.render_datetime(created)))
    return rows


def jc_previews(pkg):
    """The dataset's DataStore tables for the page itself, one per entity type since T-3012, in
    the order `_order` gives them: each resource with its table view (None when nobody created
    one) and its number of rows (None when the DataStore does not say)."""
    out = []
    for resource in sorted((pkg or {}).get("resources") or [], key=_order):
        if not resource.get("datastore_active"):
            continue
        try:
            views = toolkit.get_action("resource_view_list")({}, {"id": resource["id"]})
        except (toolkit.ObjectNotFound, toolkit.NotAuthorized):
            views = []
        view = next((v for v in views or [] if v.get("view_type") == "datatables_view"), None)
        try:
            total = toolkit.get_action("datastore_search")({}, {"resource_id": resource["id"], "limit": 0}).get("total")
        except (toolkit.ObjectNotFound, toolkit.NotAuthorized, toolkit.ValidationError):
            total = None
        out.append({"resource": resource, "view": view, "total": total})
    return out


VIEW_ID = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
SLUG = re.compile(r"^[a-z0-9]{16,64}$")
# A visualization's name on its Endpoint, a DNS label (EP-130): what the publisher matches its view by.
VIEW_NAME = re.compile(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")
SPEC_BYTES = 200 * 1024


def _json(value):
    """A JSON text from a view's config as the value it holds, None when it is not JSON."""
    try:
        return json.loads(value) if isinstance(value, str) else None
    except ValueError:
        return None


def jc_viz_origin():
    """The renderer's host, viz.{domain} (ADR-N-053 §2.5); None when the branding names no domain."""
    domain = BRANDING.get("domain")
    return "https://viz." + domain if isinstance(domain, str) and re.fullmatch(r"[a-z0-9.-]+", domain) else None


def jc_visualization(view):
    """One published chart (T-4641) for a template: its title and description in the page's
    language, the frame's address on the renderer's host and the snippet that embeds it on another
    site. The frame is named by the view id alone: the renderer reads the spec from CKAN itself."""
    origin = jc_viz_origin()
    if not origin or not view or view.get("view_type") != "jc_visualization" or not VIEW_ID.match(view.get("id") or ""):
        return None
    language = _language()
    title = _localized(_json(view.get("titles")), language) or view.get("title") or ""
    description = _localized(_json(view.get("descriptions")), language) or view.get("description") or ""
    embed = "%s/#%s&lang=%s" % (origin, view["id"], language)
    # The host page reads CKAN and the API under its own origin, so it keeps it
    # (`allow-same-origin` on a frame of another site gives it nothing of the embedding page);
    # the spec itself runs one frame deeper, sandboxed to an opaque origin.
    code = ('<iframe src="%s" title="%s" width="100%%" height="480" style="border:0" loading="lazy" '
            'referrerpolicy="no-referrer" sandbox="allow-scripts allow-same-origin allow-downloads '
            'allow-popups allow-popups-to-escape-sandbox"></iframe>') % (embed, html.escape(title, quote=True))
    return {"id": view["id"], "title": title, "description": description, "origin": origin,
            "src": "%s/#%s&inline&lang=%s" % (origin, view["id"], language), "code": code,
            "package": view.get("package_id")}


_API_ENDPOINT = re.compile(r"^https://api\.([a-z0-9.-]+)/api/endpoint/([a-z0-9]{16,64})/")


def jc_explorer(pkg, resource=None):
    """The live explorer of the dataset's Endpoint on viz.{domain} (T-5059): the frame's address,
    named by the Endpoint's slug (and the resource's entity type, on a resource page), and its
    origin for the frame's height. It reads the Endpoint itself, anonymously, so no row is copied
    and the page asks no DataStore. None without a renderer or an Endpoint on api.{domain}."""
    origin = jc_viz_origin()
    live = jc_live(pkg)
    match = _API_ENDPOINT.match((live or {}).get("url") or "")
    if not origin or not match or match.group(1) != BRANDING.get("domain"):
        return None
    # The catalogue is drawn light only: the widget takes the host's scheme, never the reader's
    # system one, or its rows go dark on a light page (T-5192).
    src = "%s/explore.html#%s&inline&lang=%s&theme=light" % (origin, match.group(2), _language())
    # The dataset's name gives the explorer's Embed code its source and licence line (T-5129).
    if re.fullmatch(r"[a-z0-9_-]{2,100}", (pkg or {}).get("name") or ""):
        src += "&dataset=" + pkg["name"]
    kind_type = jc_entity_type(resource) if resource else None
    if not kind_type and resource and resource.get("datastore_active"):
        kind_type = resource.get("name")
    if kind_type and re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,127}", kind_type):
        src += "&type=" + kind_type
    return {"src": src, "origin": origin, "title": jc_t("explorer")}


def jc_developer_page(pkg):
    """The Portal's developer page of the dataset's public Endpoint (T-5111): `/d/{slug}` on
    portal.{domain}, named by the slug of the Endpoint address on api.{domain}; None otherwise."""
    match = _API_ENDPOINT.match((jc_live(pkg) or {}).get("url") or "")
    portal = jc_portal_url()
    if not portal or not match or match.group(1) != BRANDING.get("domain"):
        return None
    return "%sd/%s?lang=%s" % (portal, match.group(2), _language())


def jc_visualizations(pkg):
    """Every chart published on the dataset's resources, in the resources' order (ADR-N-053)."""
    out = []
    for resource in sorted((pkg or {}).get("resources") or [], key=_order):
        try:
            views = toolkit.get_action("resource_view_list")({}, {"id": resource["id"]})
        except (toolkit.ObjectNotFound, toolkit.NotAuthorized):
            views = []
        out.extend(item for item in map(jc_visualization, views or []) if item)
    return out


def _viz_object(value):
    """A view config member that must be a JSON object: the spec within 200 KB (EP-130)."""
    if value in (None, ""):
        return value
    if not isinstance(value, str) or len(value.encode("utf-8")) > SPEC_BYTES or not isinstance(_json(value), dict):
        raise toolkit.Invalid("a JSON object of at most 200 KB")
    return value


def _viz_name(value):
    if value in (None, "") or (isinstance(value, str) and VIEW_NAME.match(value)):
        return value
    raise toolkit.Invalid("a visualization name: a DNS label")


def _viz_slug(value):
    if value in (None, "") or (isinstance(value, str) and SLUG.match(value)):
        return value
    raise toolkit.Invalid("an Endpoint slug")
# The basemap the map draws under the data (T-4639): a style on this installation's tiles host
# (T-4638), whose vector tiles the page reads with pmtiles. Anything else is no basemap: a map of
# the data alone. OpenStreetMap's own tile servers are not one (T-4661): their policy refuses
# production use, and every visitor's address would reach them.
PMTILES = "/pmtiles-4.5.0.js"
MAPLIBRE = "/maplibre-gl-6.14.0/"
VENDOR = "/srv/app/vendor"


def _basemap():
    try:
        found = json.loads(os.environ.get("JC_MAP_BASEMAP") or "{}")
    except ValueError:
        return {}
    if not isinstance(found, dict):
        return {}
    domain = BRANDING.get("domain") or ""
    style = found.get("style")
    if isinstance(style, str) and domain and re.fullmatch(r"https://tiles\.%s/[A-Za-z0-9/._-]+\.json" % re.escape(domain), style):
        return {"style": style}
    return {}


BASEMAP = _basemap()


def _leaf(column):
    """The innermost named part of a DataStore column: `streetAddress` of
    `address.value.streetAddress`, `name` of `name.languageMap.sk` (jc-core's field_order::leaf)."""
    parts = [part.split("[")[0] for part in column.split(".")]
    found = parts[0]
    skip = False
    for part in parts[1:]:
        if skip:
            skip = False
        elif part == "languageMap":
            skip = True
        elif part not in ("value", "object", ""):
            found = part
    return found


def _display_fields(resource_id):
    """The order, labels and key attributes the publisher wrote into a DataStore table's data
    dictionary (T-4672, DM-98…DM-101): one entry per attribute in the table's column order, `a`
    the attribute, `l` its labels per language, `s` whether it is a key attribute. None when the
    DataStore does not answer or the table carries none."""
    try:
        fields = toolkit.get_action("datastore_search")({}, {"resource_id": resource_id, "limit": 0}).get("fields") or []
    except (toolkit.ObjectNotFound, toolkit.NotAuthorized, toolkit.ValidationError):
        return None
    out = []
    for field in fields:
        column = str(field.get("id") or "")
        attribute = column.split(".")[0].split("[")[0]
        if not attribute or attribute.startswith("_") or attribute == "entity_id":
            continue
        info = field.get("info") if isinstance(field.get("info"), dict) else {}
        entry = next((e for e in out if e["a"] == attribute), None)
        if entry is None:
            entry = {"a": attribute, "l": {}, "s": False}
            out.append(entry)
        labels = info.get("labels")
        if _leaf(column) == attribute and isinstance(labels, dict):
            entry["l"] = {str(k): str(v) for k, v in labels.items() if isinstance(v, str)}
        entry["s"] = entry["s"] or info.get("summary") is True
    return out if any(e["l"] or e["s"] for e in out) else None


def jc_map(pkg, resource_id=None):
    """What the dataset's map reads (T-4639): per DataStore table, the OGC API Features items of
    its entity type on the dataset's own Endpoint, with the collection's schema for units, or
    only `resource_id`'s. The reader's browser asks each for one feature and shows the map only
    when one has a geometry. None without such an address or without the map library."""
    pkg = pkg or {}
    if not os.path.isdir(VENDOR):
        return None
    resources = pkg.get("resources") or []
    bases = []
    for resource in resources:
        url = resource.get("url") or ""
        parts = urlsplit(url)
        host = parts.hostname or ""
        if jc_kind(resource) != "OGCFEAT" or not (url.startswith("https://") and _on_platform(host)):
            continue
        bases.append(parts)
    layers = []
    for resource in resources:
        kind_type = resource.get("name") or ""
        if not resource.get("datastore_active") or (resource_id and resource.get("id") != resource_id):
            continue
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*", kind_type):
            continue
        for parts in bases:
            path = parts.path.rstrip("/")
            if path.endswith("/items"):
                if not path.endswith("/collections/%s/items" % kind_type):
                    continue
                collection = path[:-len("/items")]
            else:
                collection = "%s/collections/%s" % (path, kind_type)
            layer = {
                "type": kind_type,
                "items": urlunsplit(parts._replace(path=collection + "/items")),
                "schema": urlunsplit(parts._replace(path=collection + "/schema", query="")),
            }
            # Past the cap the map draws the collection's vector tiles (T-5189). A tile takes no
            # query, so a layer narrowed to one dataset of a shared Endpoint keeps its items.
            if not parts.query:
                layer["tiles"] = urlunsplit(parts._replace(path=collection + "/tiles/WebMercatorQuad"))
            fields = _display_fields(resource.get("id"))
            if fields:
                layer["fields"] = fields
            layers.append(layer)
            break
    if not layers:
        return None
    name = pkg.get("name") or ""
    return {
        "layers": layers,
        "basemap": BASEMAP,
        "lib": MAPLIBRE + "maplibre-gl.mjs",
        "pmtiles": PMTILES if BASEMAP.get("style") else "",
        "enums": "/jc-enums.json",
        "lang": _language(),
        "title": pkg.get("title") or name,
        "dataset": toolkit.url_for("dataset.read", id=name, _external=True),
        "embed": toolkit.url_for("jc_pages.map", id=name, _external=True),
        "t": {key: jc_t(key) for key in ("map_cap", "map_failed", "map_empty", "map_loading", "map_close", "map_panel",
                                          "map_length", "map_area", "map_keys", "map_title", "map_more")},
    }


def jc_map_page(id, resource_id=None):
    """The map of one dataset as a page of its own, for a site to frame (T-4639): no header,
    no footer, no form. The page names its own policy, the catalogue's with any ancestor, as the
    upstream owns its CSP (OPS-34); the edge strips the cookie."""
    try:
        pkg = toolkit.get_action("package_show")({}, {"id": id})
    except (toolkit.ObjectNotFound, toolkit.NotAuthorized):
        return toolkit.abort(404)
    config = jc_map(pkg, resource_id)
    if not config:
        return toolkit.abort(404)
    page = toolkit.render("jc/map.html", {"pkg": pkg, "jc_map_config": config})
    return page, 200, {"Content-Security-Policy": _csp(jc_csp_nonce(), "*")}


# The catalogue's own Content Security Policy (T-5147, OPS-34): scripts and styles from this host
# and those carrying the request's nonce, nothing inline else; frames, fetches and images from this
# installation's own hosts ({domain} and its subdomains: the Endpoints, tiles, viz and assistant).
# The edge's frame-ancestors-only baseline stays for the answers that carry none (static files).
def _csp(nonce, ancestors="'self'"):
    domain = BRANDING.get("domain")
    own = " https://%s https://*.%s" % (domain, domain) if isinstance(domain, str) and re.fullmatch(r"[a-z0-9.-]+", domain) else ""
    return "; ".join((
        "default-src 'self'",
        "script-src 'self' 'nonce-%s'" % nonce,
        "style-src 'self' 'nonce-%s'" % nonce,
        "img-src 'self' data:" + own,
        "connect-src 'self'" + own,
        "frame-src 'self'" + own,
        "font-src 'self'",
        "worker-src 'self'",
        "manifest-src 'self'",
        "form-action 'self'",
        "frame-ancestors " + ancestors,
        "object-src 'none'",
        "base-uri 'self'",
    ))


def jc_csp_nonce():
    """This request's nonce, for a template's inline <script> or <style> (T-5147)."""
    nonce = getattr(toolkit.g, "jc_csp_nonce", None)
    if not nonce:
        nonce = toolkit.g.jc_csp_nonce = secrets.token_urlsafe(18)
    return nonce


def jc_csp_header(response):
    """Every HTML answer that names no policy of its own gets the catalogue's (T-5147)."""
    if response.mimetype == "text/html" and "Content-Security-Policy" not in response.headers:
        response.headers["Content-Security-Policy"] = _csp(jc_csp_nonce())
    return response


def _site_url():
    return (toolkit.config.get("ckan.site_url") or "").rstrip("/")


def _json_in_script(value):
    """JSON a `<script>` element cannot be closed by: a dataset's words are the publisher's."""
    text = json.dumps(value, ensure_ascii=False, sort_keys=True)
    return text.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")


def jc_schema_org_dict(pkg):
    """The dataset as a schema.org Dataset (T-5123), the record a search engine's dataset index
    reads. Everything comes from the dataset: no value of its own, a field the dataset lacks is
    left out and scripts/check-schema-org.py names it."""
    extras = _extras(pkg)
    site = _site_url()
    url = "%s/dataset/%s" % (site, pkg.get("name"))
    organization = pkg.get("organization") or {}
    creator = {"@type": "Organization", "name": extras.get("publisher_name") or organization.get("title")}
    if _is_url(extras.get("publisher_uri")):
        creator["sameAs"] = extras["publisher_uri"]
    record = {
        "@context": "https://schema.org/",
        "@type": "Dataset",
        "@id": url,
        "url": url,
        "name": pkg.get("title") or pkg.get("name"),
        # Google takes 50 to 5000 characters.
        "description": (pkg.get("notes") or pkg.get("title") or "")[:5000],
        "identifier": extras.get("identifier") or pkg.get("id"),
        "isAccessibleForFree": True,
        "creator": creator,
        "publisher": creator,
        "includedInDataCatalog": {"@type": "DataCatalog", "name": jc_site_title(), "url": site + "/"},
        "keywords": [t.get("display_name") or t.get("name") for t in pkg.get("tags") or []],
        "datePublished": extras.get("issued") or pkg.get("metadata_created"),
        "dateModified": extras.get("modified") or pkg.get("metadata_modified"),
    }
    licence = extras.get("license_url") or pkg.get("license_url")
    if _is_url(licence):
        record["license"] = licence
    coverage = extras.get("spatial_uri")
    if _is_url(coverage):
        nuts = re.search(r"/nuts/code/([A-Z]{2}[0-9A-Z]{0,3})$", coverage)
        names = NUTS_NAMES.get(nuts.group(1)) if nuts else None
        name = (names.get(_language()) or names["en"]) if names else ("NUTS " + nuts.group(1) if nuts else coverage)
        record["spatialCoverage"] = {"@type": "Place", "name": name, "sameAs": coverage}
    # An ISO 8601 interval; a dataset that is still updated runs from its start to "..".
    start = extras.get("temporal_start") or (record["datePublished"] or "")[:10]
    if start:
        record["temporalCoverage"] = "%s/%s" % (start, extras.get("temporal_end") or "..")
    record["distribution"] = [
        {"@type": "DataDownload", "name": r.get("name") or r.get("format"), "contentUrl": r["url"],
         "encodingFormat": r.get("mimetype") or r.get("format")}
        for r in pkg.get("resources") or [] if _is_url(r.get("url")) and (r.get("mimetype") or r.get("format"))
    ]
    return {k: v for k, v in record.items() if v not in (None, "", [])}


def jc_schema_org(pkg):
    """The `application/ld+json` text of a dataset page."""
    return _json_in_script(jc_schema_org_dict(pkg))


# A sitemap file holds at most 50 000 addresses (sitemaps.org).
SITEMAP_MAX = 50000


def jc_sitemap_entries(search=None, organizations=None):
    """(address, last change) of each public dataset and each organization (T-5123). The search
    is anonymous and asks for no private or draft dataset, and the hidden ones (T-4318) are left
    out whoever asks, so the file never names what an anonymous reader cannot open."""
    search = search or toolkit.get_action("package_search")
    organizations = organizations or toolkit.get_action("organization_list")
    site = _site_url()
    entries, start = [], 0
    while len(entries) < SITEMAP_MAX:
        page = search({"user": "", "ignore_auth": False}, {
            "q": "*:*", "fq": "+dataset_type:dataset", "rows": 1000, "start": start, "sort": "name asc",
            "include_private": False, "include_drafts": False, "fl": ["name", "metadata_modified"]})
        results = page.get("results") or []
        for pkg in results:
            if pkg.get("name") in HIDDEN:
                continue
            # Solr's date comes back as text or, through pysolr, a datetime: its first ten characters either way.
            entries.append(("%s/dataset/%s" % (site, pkg["name"]), str(pkg.get("metadata_modified") or "")[:10]))
        start += len(results)
        if not results or start >= page.get("count", 0):
            break
    for name in organizations({"user": "", "ignore_auth": False}, {"all_fields": False}):
        entries.append(("%s/organization/%s" % (site, name), ""))
    return entries[:SITEMAP_MAX]


def jc_sitemap_xml(entries):
    lines = ['<?xml version="1.0" encoding="UTF-8"?>',
             '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">']
    for loc, modified in entries:
        lastmod = "<lastmod>%s</lastmod>" % html.escape(modified) if re.fullmatch(r"\d{4}-\d{2}-\d{2}", modified) else ""
        lines.append("<url><loc>%s</loc>%s</url>" % (html.escape(loc), lastmod))
    lines.append("</urlset>")
    return "\n".join(lines) + "\n"


def jc_robots_txt():
    """CKAN's own rules and the sitemap; a demo that asks search engines to stay out (T-4687)
    closes the whole site instead."""
    if BRANDING.get("noindex"):
        return "User-agent: *\nDisallow: /\n"
    return ("User-agent: *\nDisallow: /dataset/rate/\nDisallow: /revision/\nDisallow: /dataset/*/history\n"
            "Disallow: /api/\nCrawl-Delay: 10\n\nSitemap: %s/sitemap.xml\n" % _site_url())


def jc_sitemap_page():
    return jc_sitemap_xml(jc_sitemap_entries()), 200, {"Content-Type": "application/xml; charset=utf-8"}


def jc_robots_page():
    return jc_robots_txt(), 200, {"Content-Type": "text/plain; charset=utf-8"}


def jc_total(previews):
    """All rows of the tables, or None when any table's count is unknown."""
    totals = [p["total"] for p in previews or []]
    return sum(totals) if totals and all(isinstance(t, int) for t in totals) else None


def jc_formats(pkg):
    """The formats a reader gets, data before APIs, without the data model's artifacts."""
    seen, out = set(), []
    for section in jc_resource_sections(pkg):
        if section["key"] == "schema":
            continue
        for item in section["items"]:
            name = item["format"]
            if name != "—" and name.upper() not in seen:
                seen.add(name.upper())
                out.append(name)
    return out


# The formats a dataset serves its data in (T-5649, owner 2026-10-25): the search's Format filter
# and a result's chips, by their standard names, the same in every language. Only what `jcctl`
# published for an output the Endpoint serves counts; the data model's documents and the DCAT
# record stay on the dataset page. Indexed as `vocab_jc_format` (CKAN's Solr schema keeps any
# `vocab_*` as a multi-valued string), by code, in this order.
DATA_FORMATS = {"NGSI-LD": "NGSI-LD", "WEBSOCKET": "WebSocket", "OGCFEAT": "OGC API Features", "ODATA": "OData",
                "STA": "SensorThings API", "GEOJSON": "GeoJSON", "CSV": "CSV", "JSON": "JSON", "XLSX": "Excel",
                "XLS": "XLS", "XML": "XML", "PARQUET": "Parquet", "ICS": "iCalendar", "RSS": "RSS", "ATOM": "Atom",
                "JSONFEED": "JSON Feed", "MCP": "MCP"}
# A calendar or feed `jcctl` lists per output names its format by media type (a JSON Feed is "JSON").
_MEDIA_FORMATS = {"text/calendar": "ICS", "application/atom+xml": "ATOM", "application/rss+xml": "RSS",
                  "application/feed+json": "JSONFEED"}
# The EU data themes (publications.europa.eu/resource/authority/data-theme) the DCAT profile writes
# into the `theme` extra, as the search's Topic filter and the home page's tiles (T-5294), named the
# way the EU vocabulary names them in each language. Indexed as `vocab_jc_topic`, by code.
THEME_URI = "http://publications.europa.eu/resource/authority/data-theme/"
TOPICS = {
    "AGRI": {"en": "Agriculture, fisheries, forestry and food", "sk": "Poľnohospodárstvo, rybné hospodárstvo, lesníctvo a potraviny", "cs": "Zemědělství, rybolov, lesnictví a výživa", "de": "Landwirtschaft, Fischerei, Forstwirtschaft und Nahrungsmittel", "hu": "Mezőgazdaság, halászat, erdészet és élelmiszer"},
    "ECON": {"en": "Economy and finance", "sk": "Hospodárstvo a financie", "cs": "Hospodářství a finance", "de": "Wirtschaft und Finanzen", "hu": "Gazdaság és pénzügy"},
    "EDUC": {"en": "Education, culture and sport", "sk": "Vzdelávanie, kultúra a šport", "cs": "Vzdělávání, kultura a sport", "de": "Bildung, Kultur und Sport", "hu": "Oktatás, kultúra és sport"},
    "ENER": {"en": "Energy", "sk": "Energetika", "cs": "Energie", "de": "Energie", "hu": "Energia"},
    "ENVI": {"en": "Environment", "sk": "Životné prostredie", "cs": "Životní prostředí", "de": "Umwelt", "hu": "Környezet"},
    "GOVE": {"en": "Government and public sector", "sk": "Vláda a verejný sektor", "cs": "Vláda a veřejný sektor", "de": "Regierung und öffentlicher Sektor", "hu": "Kormányzat és közszféra"},
    "HEAL": {"en": "Health", "sk": "Zdravie", "cs": "Zdraví", "de": "Gesundheit", "hu": "Egészségügy"},
    "INTR": {"en": "International issues", "sk": "Medzinárodné otázky", "cs": "Mezinárodní otázky", "de": "Internationale Themen", "hu": "Nemzetközi ügyek"},
    "JUST": {"en": "Justice, legal system and public safety", "sk": "Spravodlivosť, právny systém a verejná bezpečnosť", "cs": "Spravedlnost, právní systém a veřejná bezpečnost", "de": "Justiz, Rechtssystem und öffentliche Sicherheit", "hu": "Igazságügy, jogrendszer és közbiztonság"},
    "REGI": {"en": "Regions and cities", "sk": "Regióny a mestá", "cs": "Regiony a města", "de": "Regionen und Städte", "hu": "Régiók és városok"},
    "SOCI": {"en": "Population and society", "sk": "Obyvateľstvo a spoločnosť", "cs": "Populace a společnost", "de": "Bevölkerung und Gesellschaft", "hu": "Népesség és társadalom"},
    "TECH": {"en": "Science and technology", "sk": "Veda a technika", "cs": "Věda a technika", "de": "Wissenschaft und Technologie", "hu": "Tudomány és technológia"},
    "TRAN": {"en": "Transport", "sk": "Doprava", "cs": "Doprava", "de": "Verkehr", "hu": "Közlekedés"},
}
FACET_LABELS = {"vocab_jc_format": lambda name: DATA_FORMATS.get(name, name),
                "vocab_jc_topic": lambda name: _words(TOPICS.get(name)) or name}


def _words(words):
    return (words or {}).get(_language()) or (words or {}).get("en") or ""


def jc_data_formats(pkg):
    """The codes of the formats the dataset serves its data in, in DATA_FORMATS order: an
    Endpoint's address by its path, a DataStore table as CSV, a calendar or feed by its media
    type, any other file by its format. The model's documents, the DCAT record and the link to
    the publisher's own page are none."""
    found = set()
    for resource in (pkg or {}).get("resources") or []:
        if _section(resource) == "schema" or jc_is_source(resource, pkg):
            continue
        found.add("CSV" if resource.get("datastore_active") else jc_kind(resource)
                  or _MEDIA_FORMATS.get((resource.get("mimetype") or "").strip().lower())
                  or (resource.get("format") or "").strip().upper())
    return [code for code in DATA_FORMATS if code in found]


def jc_topics(pkg):
    """The dataset's EU data themes by code, from the `theme` extra (a JSON list of their URIs);
    a theme outside the EU vocabulary and a broken extra are left out."""
    extras = (pkg or {}).get("extras") or []
    raw = next((e.get("value") for e in extras if isinstance(e, dict) and e.get("key") == "theme"), None)
    if raw is None:
        raw = (pkg or {}).get("theme")
    try:
        uris = json.loads(raw) if isinstance(raw, str) else raw
    except ValueError:
        return []
    if not isinstance(uris, list):
        return []
    codes = [uri[len(THEME_URI):] for uri in uris if isinstance(uri, str) and uri.startswith(THEME_URI)]
    return [code for code in dict.fromkeys(codes) if code in TOPICS]


def jc_format_chips(pkg):
    """A search result's chips: the formats it serves, by their standard names."""
    return [DATA_FORMATS[code] for code in jc_data_formats(pkg)]


def jc_facet_labels(search_results, search_params):
    """The Format and Topic filters in the page's language. Topic leaves out a value every result
    shares, which narrows nothing (T-5294), unless the search already filters by it, so it can be
    taken off again. Format lists every served format (owner 2026-10-25, T-5649): NGSI-LD on
    every dataset still tells a reader the catalogue serves it."""
    facets = search_results.get("search_facets") or {}
    total = search_results.get("count") or 0
    # CKAN's query turns `fq` into a list of clauses before this hook sees it.
    fq = search_params.get("fq") or ""
    fq = " ".join(map(str, fq)) if isinstance(fq, list) else str(fq)
    fq += " " + " ".join(map(str, search_params.get("fq_list") or []))
    for field, label in FACET_LABELS.items():
        facet = facets.get(field)
        if not isinstance(facet, dict):
            continue
        items = []
        for item in facet.get("items") or []:
            name = item.get("name") or ""
            if field == "vocab_jc_topic" and item.get("count") == total and '%s:"%s"' % (field, name) not in fq:
                continue
            item["display_name"] = label(name)
            items.append(item)
        facet["items"] = items
    return search_results


def jc_topic_tiles():
    """The home page's topics: each EU data theme the public datasets carry, with its count and its
    search, the most used first. Empty when no dataset names a theme."""
    from ckan.lib.search import SearchError

    try:
        found = toolkit.get_action("package_search")(
            {}, {"rows": 0, "fq": 'capacity:"public"', "facet.field": ["vocab_jc_topic"], "facet.limit": len(TOPICS)})
    except SearchError:
        return []
    counts = (found.get("facets") or {}).get("vocab_jc_topic") or {}
    tiles = [{"code": code, "label": _words(TOPICS[code]), "count": count,
              "url": toolkit.url_for("dataset.search", vocab_jc_topic=code)}
             for code, count in counts.items() if code in TOPICS and count]
    return sorted(tiles, key=lambda tile: (-tile["count"], tile["label"]))


def jc_number(value):
    """A count with thin-space thousands, the way every language of the Portal reads it."""
    try:
        return "{:,}".format(int(value)).replace(",", "\u202f")
    except (TypeError, ValueError):
        return ""


def jc_rows(value, key="rows"):
    """A row count with its word in the page's language: "1 row", "3 riadky", "5 řádků"; `key`
    names another counted word, such as "datasets_n"."""
    number = jc_number(value)
    if not number:
        return ""
    count = abs(int(value))
    words = STRINGS[key].get(_language()) or STRINGS[key]["en"]
    form = 0 if count == 1 else 1 if 2 <= count <= 4 else 2
    return "%s %s" % (number, words[form])


def jc_recent(limit=6):
    """The public datasets changed last, for the home page."""
    from ckan.lib.search import SearchError

    try:
        found = toolkit.get_action("package_search")(
            {}, {"rows": limit, "sort": "metadata_modified desc", "fq": 'capacity:"public"'})
    except SearchError:
        return []
    return found.get("results") or []


def jc_has_topics():
    """Whether the catalogue has a topic at all (T-4217): the navigation's Topics and a
    dataset's Topics tab led to "There are currently no groups for this site." until one did."""
    try:
        return bool(toolkit.get_action("group_list")({}, {"limit": 1}))
    except (toolkit.NotAuthorized, toolkit.ObjectNotFound):
        return False


HYPHENATED = re.compile(r"^\w+(?:-\w+)+$")


# The endings Slovak and Czech nouns and adjectives decline through, longest first (T-4239).
SLAVIC_ENDINGS = ("iach", "ách", "ami", "ých", "ými", "ého", "ému", "ia", "ie", "ii", "iu", "ej",
                  "om", "ov", "ou", "ím", "ým", "a", "á", "e", "é", "ě", "i", "í", "o", "u", "ú", "y", "ý")


# The endings whose vowel drops when the word declines (T-5116): "dôchodok" is "dôchodky",
# "lekáreň" "lekárne", "Lučenec" "Lučenca", so the word's own prefix misses every other form.
FLEETING = (("ok", "k"), ("ek", "k"), ("ec", "c"), ("eň", "n"))


def jc_stem(word):
    """`(word OR stem*)` for a word of four letters or more, else the word (T-4239, T-4387). The
    catalogue's Solr stems in English, so "ovzdušie" missed every "ovzdušia". Any word, since a
    visitor types Slovak without its letters ("ovzdusie") and Solr folds them on both sides; a word
    with no ending is its own stem, so the singular finds the plural ("most", "mosty"). The stem
    keeps four letters at least."""
    if not word.isalpha() or len(word) < 4:
        return word
    for ending in SLAVIC_ENDINGS:
        if word.lower().endswith(ending) and len(word) - len(ending) >= 4:
            return "(%s OR %s*)" % (word, word[: -len(ending)])
    for ending, rest in FLEETING:
        if word.lower().endswith(ending) and len(word) - 1 >= 5:
            return "(%s OR %s* OR %s%s*)" % (word, word, word[: -len(ending)], rest)
    return "(%s OR %s*)" % (word, word)


def jc_search_words(q):
    """The visitor's search with each hyphenated word as a phrase (T-4218) and each Slovak or
    Czech word open to its other endings (T-4239). Solr split `bike-sharing` into parts it OR'ed,
    so the word found everything about either part. A query with its own quotes or a field
    (`tags:x`) is the visitor's syntax and goes as typed."""
    if not q or '"' in q or ":" in q:
        return q
    return " ".join('"%s"' % word if HYPHENATED.match(word) else jc_stem(word) for word in q.split())


# CKAN's words for its organizations and groups, where a visitor reads them (T-4220): the
# navigation said Publishers and Topics, the breadcrumb, the title and the search box
# Organizations and Groups. Every other purpose is a staff form and keeps CKAN's word.
HUMANIZE = {
    "breadcrumb": "nav_%s", "page title": "nav_%s", "main nav": "nav_%s",
    "facet label": "nav_%s", "search placeholder": "search_%s",
}
HUMANIZED = {"organization": "publishers", "group": "topics"}


@toolkit.chained_helper
def humanize_entity_type(next_helper, entity_type, object_type, purpose):
    if entity_type == object_type and entity_type in HUMANIZED and purpose in HUMANIZE:
        return jc_t(HUMANIZE[purpose] % HUMANIZED[entity_type])
    return next_helper(entity_type, object_type, purpose)


def jc_about_site():
    """What the About page says when no sysadmin wrote `ckan.site_about` (T-3028): whose
    catalogue this is, how to get at the data, and the machine entrances, all from the branding
    block and the plugins that are loaded. The words are escaped by the template like any text."""
    name = BRANDING.get("instanceName") or "joinedcontext"
    organisation = BRANDING.get("organisation") or ""
    if organisation:
        intro = jc_t("about_intro_org").format(name=name, org=organisation)
    else:
        intro = jc_t("about_intro").format(name=name)
    email = BRANDING.get("contactEmail") or ""
    return {
        "intro": intro,
        "dcat": plugins.plugin_loaded("dcat"),
        "email": email if "@" in email else "",
    }


_HOST = re.compile(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$")
MCP_NAME = "joinedcontext"
# The name a person gives the connector in an assistant and then names in a question: the hub's
# own title (serverInfo.title, T-4732), one name on every page (T-4735). MCP_NAME stays the key.
CONNECTOR_NAME = "Joinedcontext Open Data"
MCP_SERVER = re.compile(r"d/[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?")


def jc_mcp_install():
    """The "Connect an AI assistant" panel (T-4114, EP-113): the installation's MCP hub and one
    ready line per client, or None without a valid domain. Every line names the hub alone, never a
    dataset, a token or a key, so one installation serves the whole catalogue (EP-110)."""
    domain = BRANDING.get("domain")
    if not (isinstance(domain, str) and _HOST.match(domain)):
        return None
    # The data host, api.{domain} (ADR-N-052 §2.1, T-4632): the apex moves an anonymous client there.
    url = "https://api.%s/api/mcp" % domain
    # An installation that offers one assistant's own server instead of the hub, such as a demo
    # (T-4687): branding `mcpServer` `d/{publicId}`, that assistant's Endpoints and sources (T-4691).
    own = BRANDING.get("mcpServer")
    if isinstance(own, str) and MCP_SERVER.fullmatch(own):
        url += "/" + own
    server = {"type": "http", "url": url}
    vscode = json.dumps(dict(name=MCP_NAME, **server), separators=(",", ":"))
    cursor = base64.b64encode(json.dumps({"url": url}, separators=(",", ":")).encode()).decode()
    return {
        "url": url,
        # Where the address may break in a narrow card: before each single slash, never inside
        # a word (T-4284). The template joins the parts with <wbr>; a copy reads them as one.
        "url_parts": re.split(r"(?<=[^/])(?=/(?!/))", url),
        # One argument per line, so a narrow card never breaks the command mid-flag (T-4284).
        "claude": "claude mcp add --transport http \\\n  %s \\\n  %s" % (MCP_NAME, url),
        # Valid JSON laid out for a phone's card: the address alone on its line, so no line is
        # wider than the address itself (T-4284).
        "json": '{"mcpServers": {%s: {\n  "type": "http",\n  "url":\n  %s\n}}}' % (json.dumps(MCP_NAME), json.dumps(url)),
        # What the copy buttons put on the clipboard (T-4762): the command on one line, which runs
        # as pasted in any shell (a `\` continuation fails in cmd and PowerShell), and the JSON
        # in its usual layout, the address on the line of its key.
        "claude_copy": "claude mcp add --transport http %s %s" % (MCP_NAME, url),
        "json_copy": json.dumps({"mcpServers": {MCP_NAME: server}}, indent=2),
        "vscode": "vscode:mcp/install?" + quote(vscode, safe=""),
        "cursor": "cursor://anysphere.cursor-deeplink/mcp/install?name=%s&config=%s" % (MCP_NAME, quote(cursor, safe="")),
    }


def jc_gemini_on():
    """Gemini's card, only once its sign-in through the gateway's own OAuth works on this
    installation (T-4631, T-4630): the chart's `assistants.gemini`, off unless set."""
    return os.environ.get("JC_ASSISTANT_GEMINI") == "true"


def jc_assistant_sources():
    """The websites the hub's `search_documents` reads (T-4659), from the chart's
    `assistants.sources`: each its host, address, publisher and languages. A malformed list or entry,
    or an address that is not https, is left out rather than shown half."""
    try:
        listed = json.loads(os.environ.get("JC_ASSISTANT_SOURCES") or "[]")
    except ValueError:
        return []
    sources = []
    for entry in listed if isinstance(listed, list) else []:
        if not isinstance(entry, dict):
            continue
        url, publisher = entry.get("url"), entry.get("publisher")
        languages = entry.get("languages") or []
        host = urlsplit(url).hostname if isinstance(url, str) and url.startswith("https://") else None
        if not host or not isinstance(publisher, str) or not isinstance(languages, list):
            continue
        sources.append({"site": host, "url": url, "publisher": publisher,
                        "languages": ", ".join(str(code) for code in languages)})
    return sources


def jc_assistants():
    """The "Use it in your AI assistant" page (T-4585, T-4628, T-4634, T-4643): a card per assistant
    whose app takes our connector, ChatGPT, Claude, Grok, then Gemini once its flag is on, each its
    logo, its name and its guide, which the whole card opens; None without the hub. No card links
    to a store: our connector is listed in none yet, so each guide adds it as a custom one (T-4735).
    Gemini's card stays off: the owner removed the option (T-4735), so neither its card nor the
    Gemini CLI command shows. The hub's address stands once under the
    cards, because our connector is listed in no store yet.

    `logo` is the card's mark in its brand colours, a template beside this file."""
    mcp = jc_mcp_install()
    if not mcp:
        return None

    def card(id, title):
        return {"id": id, "name": title, "logo": "jc/card-%s.svg" % id, "guide_label": jc_t("guide_open")}

    cards = [card("chatgpt", "ChatGPT"), card("claude", "Claude"), card("grok", "Grok")]
    if jc_gemini_on():
        cards.append(card("gemini", "Gemini"))

    return {
        # One connector name on every page, the one each guide has the person type (T-4735).
        "lead": jc_t("assistants_lead").replace("{name}", CONNECTOR_NAME),
        "prompt": jc_try(_language()),
        "mcp": mcp,
        "cards": cards,
        "sources": jc_assistant_sources(),
    }


# One guide per assistant at /assistants/{id} (T-4643), for someone who has never added anything to
# an assistant: who may, numbered steps in the assistant's own words, a question to try and what
# goes wrong. `paste` is the step whose address box follows it; `{name}` the catalogue's connector.
# Every step was read against the maker's own help page on `checked`; screenshots come from the
# owner later, so no step leans on one. Each adds our address as a custom MCP
# server, never a plugin or a store listing (T-4735).
GUIDES = {
    "chatgpt": {
        "name": "ChatGPT", "checked": "2026-10-18", "paste": 3,
        "help": "https://help.openai.com/en/articles/12584461",
        "en": {
            "who": ["On chatgpt.com in a computer's browser, with ChatGPT Business, Enterprise or Edu. With Pro, turn on developer mode in the settings first; it then reads only, which is all this catalogue needs.",
                    "OpenAI names no other plan, so Free and Plus may not offer it yet."],
            "steps": ["On your computer, open chatgpt.com, sign in and go to Plugins.",
                      "Click + and choose Add custom MCP server.",
                      "Type {name} as the name.",
                      "Paste this address into the connection field:",
                      "Under sign-in, choose no authentication.",
                      "Read the warning, tick that you understand, and click Create.",
                      "In a new chat type @ and choose {name}."],
            "trouble": [("There is no option to add a custom MCP server.", "Your plan does not offer it: see who can use it above. In a company account, your administrator has to allow it."),
                        ("ChatGPT cannot reach the address.", "Copy the address again with the button and paste it whole, with nothing before or after it."),
                        ("The answer does not use the catalogue.", "Start a new chat, type @, pick {name} and ask again.")],
        },
        "sk": {
            "who": ["Na chatgpt.com v prehliadači v počítači, s plánom ChatGPT Business, Enterprise alebo Edu. S plánom Pro najprv v nastaveniach zapnite režim pre vývojárov; potom iba číta, čo tomuto katalógu stačí.",
                    "Iné plány OpenAI neuvádza, Free a Plus to teda zatiaľ nemusia ponúkať."],
            "steps": ["Na počítači otvorte chatgpt.com, prihláste sa a prejdite na Plugins.",
                      "Kliknite na + a pridajte vlastný MCP server.",
                      "Ako názov napíšte {name}.",
                      "Do poľa pre pripojenie vložte túto adresu:",
                      "Pri prihlasovaní zvoľte možnosť bez overenia.",
                      "Prečítajte si upozornenie, označte, že mu rozumiete, a kliknite na vytvoriť.",
                      "V novom rozhovore napíšte @ a vyberte {name}."],
            "trouble": [("Možnosť pridať vlastný MCP server tam nie je.", "Váš plán ju nemá: pozrite, kto to môže použiť. Vo firemnom účte ju musí povoliť správca."),
                        ("ChatGPT sa na adresu nedostane.", "Skopírujte adresu znova tlačidlom a vložte ju celú, bez ničoho pred ňou a za ňou."),
                        ("Odpoveď katalóg nepoužije.", "Otvorte nový rozhovor, napíšte @, vyberte {name} a opýtajte sa znova.")],
        },
        "cs": {
            "who": ["Na chatgpt.com v prohlížeči v počítači, s plánem ChatGPT Business, Enterprise nebo Edu. S plánem Pro nejprve v nastavení zapněte režim pro vývojáře; pak jen čte, což tomuto katalogu stačí.",
                    "Jiné plány OpenAI neuvádí, Free a Plus to tedy zatím nemusí nabízet."],
            "steps": ["Na počítači otevřete chatgpt.com, přihlaste se a přejděte na Plugins.",
                      "Klikněte na + a přidejte vlastní MCP server.",
                      "Jako název napište {name}.",
                      "Do pole pro připojení vložte tuto adresu:",
                      "U přihlašování zvolte možnost bez ověření.",
                      "Přečtěte si upozornění, označte, že mu rozumíte, a klikněte na vytvořit.",
                      "V novém rozhovoru napište @ a vyberte {name}."],
            "trouble": [("Možnost přidat vlastní MCP server tam není.", "Váš plán ji nemá: podívejte se, kdo to může použít. Ve firemním účtu ji musí povolit správce."),
                        ("ChatGPT se na adresu nedostane.", "Zkopírujte adresu znovu tlačítkem a vložte ji celou, bez ničeho před ní a za ní."),
                        ("Odpověď katalog nepoužije.", "Otevřete nový rozhovor, napište @, vyberte {name} a zeptejte se znovu.")],
        },
        "de": {
            "who": ["Auf chatgpt.com im Browser am Computer, mit ChatGPT Business, Enterprise oder Edu. Mit Pro schalten Sie zuerst in den Einstellungen den Entwicklermodus ein; dann liest er nur, was diesem Katalog genügt.",
                    "Andere Pläne nennt OpenAI nicht, Free und Plus bieten es also womöglich noch nicht an."],
            "steps": ["Öffnen Sie am Computer chatgpt.com, melden Sie sich an und gehen Sie zu Plugins.",
                      "Klicken Sie auf + und fügen Sie einen eigenen MCP-Server hinzu.",
                      "Geben Sie als Namen {name} ein.",
                      "Fügen Sie diese Adresse in das Feld für die Verbindung ein:",
                      "Wählen Sie bei der Anmeldung die Möglichkeit ohne Authentifizierung.",
                      "Lesen Sie die Warnung, bestätigen Sie, dass Sie sie verstehen, und klicken Sie auf Erstellen.",
                      "Tippen Sie in einem neuen Chat @ und wählen Sie {name}."],
            "trouble": [("Es gibt keine Möglichkeit, einen eigenen MCP-Server hinzuzufügen.", "Ihr Plan bietet sie nicht: siehe oben, wer es nutzen kann. In einem Firmenkonto muss Ihre Administration sie erlauben."),
                        ("ChatGPT erreicht die Adresse nicht.", "Kopieren Sie die Adresse erneut mit der Schaltfläche und fügen Sie sie vollständig ein, ohne etwas davor oder danach."),
                        ("Die Antwort nutzt den Katalog nicht.", "Beginnen Sie einen neuen Chat, tippen Sie @, wählen Sie {name} und fragen Sie erneut.")],
        },
    },
    "claude": {
        "name": "Claude", "checked": "2026-10-18", "paste": 3,
        "help": "https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp",
        "en": {
            "who": ["Every Claude plan, the free one too: a free account may add one connector of its own.",
                    "In a Team or Enterprise account, the organization's owner adds it once; you then only connect to it."],
            "steps": ["Open claude.ai/customize/connectors and sign in.",
                      "Click + Add and choose to add your own connector.",
                      "Type {name} as the name.",
                      "Paste this address and continue:",
                      "Keep the settings Claude found and continue.",
                      "Under sign-in, choose no sign-in, then click Add.",
                      "In a new chat, click + at the bottom left, open the connectors and switch {name} on."],
            "trouble": [("Claude says you cannot add another connector.", "A free account keeps one connector of its own: remove the one you no longer use and add this one."),
                        ("There is no button to add one.", "In a Team or Enterprise account, ask the organization's owner to add it."),
                        ("Claude asks you to sign in.", "No sign-in is needed. Remove the connector and add it again, choosing no sign-in: Claude does not let you change it afterwards.")],
        },
        "sk": {
            "who": ["Všetky plány Claude, aj bezplatný: bezplatný účet smie pridať jeden vlastný konektor.",
                    "V účte Team alebo Enterprise ho raz pridá vlastník organizácie; vy sa k nemu potom iba pripojíte."],
            "steps": ["Otvorte claude.ai/customize/connectors a prihláste sa.",
                      "Kliknite na + a vyberte pridanie vlastného konektora.",
                      "Ako názov napíšte {name}.",
                      "Vložte túto adresu a pokračujte:",
                      "Nastavenia, ktoré Claude našiel, nechajte a pokračujte.",
                      "Pri prihlásení zvoľte možnosť bez prihlásenia a kliknite na pridať.",
                      "V novom rozhovore kliknite vľavo dole na +, otvorte konektory a zapnite {name}."],
            "trouble": [("Claude hlási, že ďalší konektor pridať nemôžete.", "Bezplatný účet má jeden vlastný konektor: odstráňte ten, ktorý už nepoužívate, a pridajte tento."),
                        ("Tlačidlo na pridanie tam nie je.", "V účte Team alebo Enterprise požiadajte vlastníka organizácie, aby ho pridal."),
                        ("Claude žiada prihlásenie.", "Prihlásenie netreba. Konektor odstráňte a pridajte znova s možnosťou bez prihlásenia: Claude ju neskôr zmeniť nedovolí.")],
        },
        "cs": {
            "who": ["Všechny plány Claude, i bezplatný: bezplatný účet smí přidat jeden vlastní konektor.",
                    "V účtu Team nebo Enterprise ho jednou přidá vlastník organizace; vy se k němu pak jen připojíte."],
            "steps": ["Otevřete claude.ai/customize/connectors a přihlaste se.",
                      "Klikněte na + a vyberte přidání vlastního konektoru.",
                      "Jako název napište {name}.",
                      "Vložte tuto adresu a pokračujte:",
                      "Nastavení, která Claude našel, ponechte a pokračujte.",
                      "U přihlášení zvolte možnost bez přihlášení a klikněte na přidat.",
                      "V novém rozhovoru klikněte vlevo dole na +, otevřete konektory a zapněte {name}."],
            "trouble": [("Claude hlásí, že další konektor přidat nemůžete.", "Bezplatný účet má jeden vlastní konektor: odstraňte ten, který už nepoužíváte, a přidejte tento."),
                        ("Tlačítko pro přidání tam není.", "V účtu Team nebo Enterprise požádejte vlastníka organizace, aby ho přidal."),
                        ("Claude chce přihlášení.", "Přihlášení není potřeba. Konektor odstraňte a přidejte znovu s možností bez přihlášení: Claude ji později změnit nedovolí.")],
        },
        "de": {
            "who": ["Alle Claude-Pläne, auch der kostenlose: ein kostenloses Konto darf einen eigenen Konnektor hinzufügen.",
                    "In einem Team- oder Enterprise-Konto fügt ihn die Inhaberin oder der Inhaber der Organisation einmal hinzu; Sie verbinden sich dann nur noch."],
            "steps": ["Öffnen Sie claude.ai/customize/connectors und melden Sie sich an.",
                      "Klicken Sie auf + und wählen Sie, einen eigenen Konnektor hinzuzufügen.",
                      "Geben Sie als Namen {name} ein.",
                      "Fügen Sie diese Adresse ein und fahren Sie fort:",
                      "Behalten Sie die Einstellungen, die Claude gefunden hat, und fahren Sie fort.",
                      "Wählen Sie bei der Anmeldung die Möglichkeit ohne Anmeldung und klicken Sie auf Hinzufügen.",
                      "Klicken Sie in einem neuen Chat unten links auf +, öffnen Sie die Konnektoren und schalten Sie {name} ein."],
            "trouble": [("Claude meldet, dass Sie keinen weiteren Konnektor hinzufügen können.", "Ein kostenloses Konto hat einen eigenen Konnektor: Entfernen Sie den, den Sie nicht mehr nutzen, und fügen Sie diesen hinzu."),
                        ("Es gibt keine Schaltfläche zum Hinzufügen.", "Bitten Sie in einem Team- oder Enterprise-Konto die Inhaberin oder den Inhaber der Organisation, ihn hinzuzufügen."),
                        ("Claude verlangt eine Anmeldung.", "Eine Anmeldung ist nicht nötig. Entfernen Sie den Konnektor und fügen Sie ihn ohne Anmeldung erneut hinzu: Claude lässt das später nicht ändern.")],
        },
    },
    "grok": {
        "name": "Grok", "checked": "2026-10-18", "paste": 2,
        "help": "https://docs.x.ai/grok/connectors",
        "en": {
            "who": ["Every Grok user, on grok.com; on a phone the same page is under Settings, Connectors.",
                    "In a Grok Business or Enterprise team, the team's administrator adds it first."],
            "steps": ["Open grok.com/connectors and sign in.",
                      "Click New Connector and choose Custom.",
                      "Paste this address and confirm:",
                      "Start a new chat and ask your question."],
            "trouble": [("Grok refuses the address.", "Copy the address again with the button and paste it whole, with nothing before or after it."),
                        ("There is no button for a new connector.", "In a team account, ask your administrator to add it."),
                        ("The answer does not use the catalogue.", "Start a new chat and write: use {name}.")],
        },
        "sk": {
            "who": ["Každý používateľ Groku, na grok.com; v telefóne je tá istá stránka v Nastaveniach, v časti Konektory.",
                    "V tíme Grok Business alebo Enterprise ho najprv pridá správca tímu."],
            "steps": ["Otvorte grok.com/connectors a prihláste sa.",
                      "Kliknite na nový konektor a vyberte vlastný.",
                      "Vložte túto adresu a potvrďte:",
                      "Otvorte nový rozhovor a položte otázku."],
            "trouble": [("Grok adresu odmietne.", "Skopírujte adresu znova tlačidlom a vložte ju celú, bez ničoho pred ňou a za ňou."),
                        ("Tlačidlo na nový konektor tam nie je.", "V tímovom účte požiadajte správcu, aby ho pridal."),
                        ("Odpoveď katalóg nepoužije.", "Otvorte nový rozhovor a napíšte: použi {name}.")],
        },
        "cs": {
            "who": ["Každý uživatel Groku, na grok.com; v telefonu je tatáž stránka v Nastavení, v části Konektory.",
                    "V týmu Grok Business nebo Enterprise ho nejprve přidá správce týmu."],
            "steps": ["Otevřete grok.com/connectors a přihlaste se.",
                      "Klikněte na nový konektor a vyberte vlastní.",
                      "Vložte tuto adresu a potvrďte:",
                      "Otevřete nový rozhovor a položte otázku."],
            "trouble": [("Grok adresu odmítne.", "Zkopírujte adresu znovu tlačítkem a vložte ji celou, bez ničeho před ní a za ní."),
                        ("Tlačítko pro nový konektor tam není.", "V týmovém účtu požádejte správce, aby ho přidal."),
                        ("Odpověď katalog nepoužije.", "Otevřete nový rozhovor a napište: použij {name}.")],
        },
        "de": {
            "who": ["Alle Grok-Nutzerinnen und -Nutzer, auf grok.com; am Telefon ist dieselbe Seite unter Einstellungen, Konnektoren.",
                    "In einem Team mit Grok Business oder Enterprise fügt ihn zuerst die Team-Administration hinzu."],
            "steps": ["Öffnen Sie grok.com/connectors und melden Sie sich an.",
                      "Klicken Sie auf New Connector und wählen Sie Custom.",
                      "Fügen Sie diese Adresse ein und bestätigen Sie:",
                      "Beginnen Sie einen neuen Chat und stellen Sie Ihre Frage."],
            "trouble": [("Grok lehnt die Adresse ab.", "Kopieren Sie die Adresse erneut mit der Schaltfläche und fügen Sie sie vollständig ein, ohne etwas davor oder danach."),
                        ("Es gibt keine Schaltfläche für einen neuen Konnektor.", "Bitten Sie in einem Team-Konto Ihre Administration, ihn hinzuzufügen."),
                        ("Die Antwort nutzt den Katalog nicht.", "Beginnen Sie einen neuen Chat und schreiben Sie: nutze {name}.")],
        },
    },
    "gemini": {
        "name": "Gemini", "checked": "2026-10-18", "paste": 3,
        "help": "https://support.google.com/gemini/answer/17209137",
        "en": {
            "who": ["Google offers it to people aged 18 or over in the US, with a personal Google account (not a work or school one) and Gemini's activity setting switched on.",
                    "You add it in the Gemini web app, and it then works in the phone app too. It answers in English only."],
            "steps": ["On your computer, open gemini.google.com and sign in with your personal Google account.",
                      "At the bottom, click Settings, then Connected Apps.",
                      "Under custom apps, click to add a custom app.",
                      "Paste this address and click Next:",
                      "A short window opens and closes by itself: there is nothing to sign in to or fill in.",
                      "In a chat, type @, pick {name} and ask in English."],
            "trouble": [("There are no custom apps in the settings.", "Google offers them only to personal accounts, from 18, in the US, with the activity setting on."),
                        ("The window closes and no app is added.", "Try again in a minute. If it keeps failing, write to us from the support page."),
                        ("Gemini does not use the catalogue.", "Type @, pick {name} and ask your question in English.")],
        },
        "sk": {
            "who": ["Google to ponúka ľuďom od 18 rokov v USA, s osobným účtom Google (nie pracovným ani školským) a so zapnutým ukladaním aktivity v Gemini. Na Slovensku to Google zatiaľ neponúka.",
                    "Pridáva sa vo webovej aplikácii Gemini a potom funguje aj v telefóne. Odpovedá iba po anglicky."],
            "steps": ["Na počítači otvorte gemini.google.com a prihláste sa osobným účtom Google.",
                      "Dole kliknite na Nastavenia a potom na prepojené aplikácie.",
                      "V časti vlastných aplikácií kliknite na pridanie vlastnej aplikácie.",
                      "Vložte túto adresu a kliknite na ďalej:",
                      "Otvorí sa krátke okno a samo sa zavrie: netreba sa nikde prihlasovať ani nič vypĺňať.",
                      "V rozhovore napíšte @, vyberte {name} a pýtajte sa po anglicky."],
            "trouble": [("V nastaveniach vlastné aplikácie nie sú.", "Google ich ponúka iba osobným účtom, od 18 rokov, v USA a so zapnutým ukladaním aktivity."),
                        ("Okno sa zavrie a aplikácia sa nepridá.", "Skúste to znova o minútu. Ak to stále nejde, napíšte nám zo stránky podpory."),
                        ("Gemini katalóg nepoužije.", "Napíšte @, vyberte {name} a otázku položte po anglicky.")],
        },
        "cs": {
            "who": ["Google to nabízí lidem od 18 let v USA, s osobním účtem Google (ne pracovním ani školním) a se zapnutým ukládáním aktivity v Gemini. V Česku to Google zatím nenabízí.",
                    "Přidává se ve webové aplikaci Gemini a pak funguje i v telefonu. Odpovídá jen anglicky."],
            "steps": ["Na počítači otevřete gemini.google.com a přihlaste se osobním účtem Google.",
                      "Dole klikněte na Nastavení a pak na propojené aplikace.",
                      "V části vlastních aplikací klikněte na přidání vlastní aplikace.",
                      "Vložte tuto adresu a klikněte na další:",
                      "Otevře se krátké okno a samo se zavře: není třeba se nikde přihlašovat ani nic vyplňovat.",
                      "V rozhovoru napište @, vyberte {name} a ptejte se anglicky."],
            "trouble": [("V nastavení vlastní aplikace nejsou.", "Google je nabízí jen osobním účtům, od 18 let, v USA a se zapnutým ukládáním aktivity."),
                        ("Okno se zavře a aplikace se nepřidá.", "Zkuste to znovu za minutu. Pokud to stále nejde, napište nám ze stránky podpory."),
                        ("Gemini katalog nepoužije.", "Napište @, vyberte {name} a otázku položte anglicky.")],
        },
        "de": {
            "who": ["Google bietet es Personen ab 18 Jahren in den USA an, mit einem privaten Google-Konto (keinem Arbeits- oder Schulkonto) und eingeschalteter Gemini-Aktivität. In Europa bietet Google es noch nicht an.",
                    "Sie fügen es in der Gemini-Web-App hinzu, danach funktioniert es auch in der App am Telefon. Es antwortet nur auf Englisch."],
            "steps": ["Öffnen Sie am Computer gemini.google.com und melden Sie sich mit Ihrem privaten Google-Konto an.",
                      "Klicken Sie unten auf Einstellungen und dann auf verbundene Apps.",
                      "Klicken Sie unter den eigenen Apps darauf, eine eigene App hinzuzufügen.",
                      "Fügen Sie diese Adresse ein und klicken Sie auf Weiter:",
                      "Ein kurzes Fenster öffnet sich und schließt sich von selbst: Sie müssen sich nirgends anmelden und nichts ausfüllen.",
                      "Tippen Sie im Chat @, wählen Sie {name} und fragen Sie auf Englisch."],
            "trouble": [("In den Einstellungen gibt es keine eigenen Apps.", "Google bietet sie nur privaten Konten an, ab 18, in den USA und mit eingeschalteter Aktivität."),
                        ("Das Fenster schließt sich und keine App wird hinzugefügt.", "Versuchen Sie es in einer Minute erneut. Wenn es weiter nicht klappt, schreiben Sie uns über die Support-Seite."),
                        ("Gemini nutzt den Katalog nicht.", "Tippen Sie @, wählen Sie {name} und stellen Sie Ihre Frage auf Englisch.")],
        },
    },
}


def jc_try(language):
    """The question to try in `language`, naming the connector first (T-4735): the instance's first
    sample question, else the theme's own; the theme names no city (T-4636)."""
    own = BRANDING.get("samples")
    samples = _samples(language) if language != "en" or not isinstance(own, dict) or own.get("en") else []
    question = samples[0] if samples else STRINGS["guide_example"].get(language) or STRINGS["guide_example"]["en"]
    return STRINGS["guide_use"].get(language, STRINGS["guide_use"]["en"]).replace("{connector}", CONNECTOR_NAME) + question


def jc_assistant_guide(id):
    """The guide to one assistant in the page's language (T-4643), or None for an assistant without
    a card on this installation: an unknown id, Gemini while its flag is off, or no hub."""
    page = jc_assistants()
    if not page or id not in {card["id"] for card in page["cards"]}:
        return None
    guide = GUIDES[id]
    words = guide.get(_language()) or guide["en"]
    name = CONNECTOR_NAME
    steps = [step.replace("{name}", name) for step in words["steps"]]
    # Gemini's custom apps answer in English only, so its question to try is the English one.
    asked = "en" if id == "gemini" else _language()
    return {
        "id": id, "name": guide["name"], "help": guide["help"],
        "title": jc_t("guide_title").replace("{name}", guide["name"]),
        "who": words["who"], "steps": steps, "paste": guide["paste"], "mcp": page["mcp"],
        "example": jc_try(asked),
        "prompt": jc_try(asked),
        "good": jc_t("guide_good").replace("{name}", name),
        "trouble": [(problem, fix.replace("{name}", name)) for problem, fix in words["trouble"]],
        "checked": jc_t("guide_checked").replace("{date}", guide["checked"]).replace("{maker}", guide["name"]),
    }


def jc_guide_page(id):
    guide = jc_assistant_guide(id)
    if not guide:
        return toolkit.abort(404)
    return toolkit.render("jc/guide.html", {"jc_guide": guide})


# The connector's public pages (T-4315): what a directory reviewer and a visitor read before
# adding the MCP hub to Claude or ChatGPT. Every fact is the deployment's own: no account or
# cookie on the hub, the edge's per-IP limit held in memory, logs on container stdout only and no
# retention period configured, so none is promised. `{url}` is the hub, `{domain}` the
# installation's; a paragraph naming `{url}` is left out on an installation without a domain.
PAGES = {
    "connector": {
        "en": {"title": "MCP connector", "sections": [
            ("", ["Ask an AI assistant about this catalogue's open data: air quality now, hospitals in the "
                  "region, a city's contracts. The connector is an MCP server at {url}. It needs no account "
                  "and only reads."]),
            ("What it can do", ["list_endpoints finds the datasets; list_types, get_type and describe_schema "
                                "read their data models; query_entities and get_entity read the current rows; "
                                "query_temporal and retrieve_temporal read their history; describe_access says "
                                "what a call may read.",
                                "Every tool reads. A tool that would write is refused without a sign-in."])
        ]},
        "sk": {"title": "MCP konektor", "sections": [
            ("", ["Pýtajte sa AI asistenta na otvorené dáta tohto katalógu: aktuálnu kvalitu ovzdušia, "
                  "nemocnice v kraji, zmluvy mesta. Konektor je MCP server na adrese {url}. Netreba účet "
                  "a iba číta."]),
            ("Čo dokáže", ["list_endpoints nájde datasety; list_types, get_type a describe_schema prečítajú "
                           "ich dátové modely; query_entities a get_entity čítajú aktuálne záznamy; "
                           "query_temporal a retrieve_temporal ich históriu; describe_access povie, čo smie "
                           "volanie čítať.",
                           "Každý nástroj iba číta. Nástroj, ktorý by zapisoval, bez prihlásenia odmietne."])
        ]},
    },
    "privacy": {
        "en": {"title": "Privacy", "sections": [
            ("", ["This page covers the MCP connector at {url}."]),
            ("No account, no cookie", ["The connector answers without an account. It sets no cookie and "
                                       "keeps no profile of you."]),
            ("What reaches us", ["Each call's address, the tool, its arguments and your IP address. The edge "
                                 "counts anonymous calls per IP address to limit their rate; the counter lives "
                                 "in memory for one time window and is not stored.",
                                 "The services write each request to their container log on the server, which "
                                 "the server rotates. We do not archive these logs and send them to no "
                                 "analytics or advertising service."]),
            ("Your assistant", ["What you type stays with the assistant you use, such as Claude or ChatGPT; "
                                "the connector receives only the tool calls it makes. That provider's privacy "
                                "policy covers what you type."]),
            ("The data", ["The connector serves only datasets their publishers released as open data."]),
        ]},
        "sk": {"title": "Ochrana súkromia", "sections": [
            ("", ["Táto stránka sa týka MCP konektora na adrese {url}."]),
            ("Bez účtu, bez cookies", ["Konektor odpovedá bez účtu. Nenastavuje žiadne cookie a nevedie "
                                       "o vás žiadny profil."]),
            ("Čo k nám príde", ["Adresa každého volania, nástroj, jeho argumenty a vaša IP adresa. Brána "
                                "počíta anonymné volania podľa IP adresy, aby obmedzila ich frekvenciu; "
                                "počítadlo žije v pamäti počas jedného časového okna a neukladá sa.",
                                "Služby zapisujú každú požiadavku do logu svojho kontajnera na serveri, "
                                "ktorý server rotuje. Tieto logy nearchivujeme a neposielame ich žiadnej "
                                "analytickej ani reklamnej službe."]),
            ("Váš asistent", ["Čo napíšete, zostáva u asistenta, ktorého používate, napríklad Claude alebo "
                              "ChatGPT; konektor dostane iba volania nástrojov, ktoré asistent urobí. Na to, "
                              "čo napíšete, sa vzťahujú zásady ochrany súkromia toho poskytovateľa."]),
            ("Dáta", ["Konektor poskytuje iba datasety, ktoré ich vydavatelia zverejnili ako otvorené dáta."]),
        ]},
    },
    "terms": {
        "en": {"title": "Terms of use", "sections": [
            ("Licences", ["Each dataset names its licence on its catalogue page, and that licence governs its "
                          "reuse. Under Creative Commons Attribution (CC BY) you may copy and reuse the data "
                          "when you name the publisher."]),
            ("Read only", ["The connector never changes data. Tools that would write are refused without a "
                           "sign-in."]),
            ("As is", ["{domain} runs as a development service. It can be slow, change or pause without "
                       "notice, and we give no warranty that the data is complete or current. Check the "
                       "publisher's source before you rely on a number."]),
            ("Fair use", ["Calls are limited per IP address. Do not try to get around the limit or to reach "
                          "data that is not public."]),
        ]},
        "sk": {"title": "Podmienky používania", "sections": [
            ("Licencie", ["Každý dataset uvádza svoju licenciu na svojej stránke v katalógu a tá určuje, ako "
                          "ho smiete použiť. Pri Creative Commons Attribution (CC BY) smiete dáta kopírovať "
                          "a ďalej používať, ak uvediete vydavateľa."]),
            ("Iba čítanie", ["Konektor dáta nikdy nemení. Nástroje, ktoré by zapisovali, bez prihlásenia "
                             "odmietne."]),
            ("Bez záruky", ["{domain} beží ako vývojová služba. Môže byť pomalá, zmeniť sa alebo prerušiť bez "
                            "ohlásenia a neručíme za úplnosť ani aktuálnosť dát. Skôr než sa spoľahnete na "
                            "číslo, overte ho u vydavateľa."]),
            ("Férové používanie", ["Počet volaní je obmedzený podľa IP adresy. Neobchádzajte limit a "
                                   "nesnažte sa dostať k dátam, ktoré nie sú verejné."]),
        ]},
    },
    "support": {
        "en": {"title": "Support", "sections": [
            ("", ["Write to us when the connector fails or a number looks wrong. Name the dataset or the "
                  "tool, the question you asked and the time.",
                  "A wrong number in a dataset: tell its publisher too, named on the dataset page."]),
        ]},
        "sk": {"title": "Podpora", "sections": [
            ("", ["Napíšte nám, keď konektor zlyhá alebo číslo nesedí. Uveďte dataset alebo nástroj, otázku, "
                  "ktorú ste položili, a čas.",
                  "Chybné číslo v datasete: oznámte ho aj jeho vydavateľovi, uvedenému na stránke datasetu."]),
        ]},
    },
    # The accessibility statement (T-5108): Act 95/2019 and Decree 78/2020 ask a public body for
    # one, in the model of EU decision 2018/1523. The Portal serves the same statement at its own
    # /accessibility (portal ui/src/pages/accessibility). The contact is the template's.
    "accessibility": {
        "en": {"title": "Accessibility statement", "sections": [
            ("", ["We want everyone to be able to use this catalogue, including people who work only with a "
                  "keyboard, a screen reader or magnification."]),
            ("Legal basis", ["Accessibility is governed by Act No. 95/2019 Coll. on information technologies in "
                             "public administration and Decree No. 78/2020 Coll. on standards for public "
                             "administration information technologies, which take WCAG 2.1 level AA (EN 301 549) "
                             "as the standard."]),
            ("Compliance status", ["This catalogue is partially compliant with WCAG 2.1 level AA. The content "
                                   "listed below is not yet fully accessible."]),
            ("Content that is not accessible", [
                "Maps show places only as a picture. The same places, with their addresses and coordinates, are "
                "in the dataset's table and in its download.",
                "Charts show numbers only as a picture. The same numbers are in the dataset's table and in its "
                "download.",
                "The test with the NVDA and VoiceOver screen readers has not been done yet; a screen reader may "
                "read some controls unclearly.",
                "Other parties make the AI assistant window and the files publishers upload (PDF, spreadsheets); "
                "we cannot vouch for their accessibility."]),
            ("How we checked", ["By self-assessment. The axe-core tool checks the catalogue's home page, its "
                                "search, a dataset and this page on every automatic run, and the run fails on "
                                "any serious or critical finding. We went through them with a keyboard only."]),
            ("Date", ["This statement was prepared on 21 October 2026 and last reviewed on 21 October 2026."]),
            ("Feedback", ["Did you meet a barrier? Write to the contact below with the page's address and what "
                          "you tried to do."]),
            ("Enforcement", ["If our answer does not satisfy you, turn to the Ministry of Investments, Regional "
                             "Development and Informatization of the Slovak Republic, which oversees the "
                             "accessibility of public administration websites."]),
        ]},
        "sk": {"title": "Vyhlásenie o prístupnosti", "sections": [
            ("", ["Chceme, aby tento katalóg mohol používať každý, aj človek, ktorý pracuje iba s klávesnicou, "
                  "s čítačkou obrazovky alebo so zväčšením."]),
            ("Právny rámec", ["Prístupnosť upravuje zákon č. 95/2019 Z. z. o informačných technológiách vo "
                              "verejnej správe a vyhláška č. 78/2020 Z. z. o štandardoch pre informačné "
                              "technológie verejnej správy, ktorá za štandard berie WCAG 2.1 na úrovni AA "
                              "(EN 301 549)."]),
            ("Stav súladu", ["Katalóg je čiastočne v súlade s WCAG 2.1 na úrovni AA. Obsah uvedený nižšie "
                             "zatiaľ nie je úplne prístupný."]),
            ("Neprístupný obsah", [
                "Mapy ukazujú miesta iba ako obrázok. Tie isté miesta s adresami a súradnicami sú v tabuľke "
                "datasetu a v jeho súbore na stiahnutie.",
                "Grafy ukazujú čísla iba ako obrázok. Tie isté čísla sú v tabuľke datasetu a v jeho súbore na "
                "stiahnutie.",
                "Test s čítačkami obrazovky NVDA a VoiceOver ešte neprebehol; čítačka môže niektoré ovládacie "
                "prvky prečítať nezrozumiteľne.",
                "Okno AI asistenta a súbory, ktoré nahrávajú vydavatelia (PDF, tabuľky), pripravujú iní; ich "
                "prístupnosť nevieme zaručiť."]),
            ("Ako sme prístupnosť overili", ["Vlastným hodnotením. Nástroj axe-core pri každom automatickom "
                                             "behu kontroluje úvodnú stránku katalógu, jeho vyhľadávanie, jeden "
                                             "dataset a túto stránku a beh zlyhá pri každom závažnom alebo "
                                             "kritickom náleze. Prešli sme ich iba klávesnicou."]),
            ("Dátum", ["Vyhlásenie bolo pripravené 21. októbra 2026 a naposledy preskúmané 21. októbra 2026."]),
            ("Spätná väzba", ["Narazili ste na bariéru? Napíšte na kontakt nižšie adresu stránky a čo ste "
                              "chceli urobiť."]),
            ("Vymáhanie", ["Ak vás naša odpoveď neuspokojí, obráťte sa na Ministerstvo investícií, regionálneho "
                           "rozvoja a informatizácie Slovenskej republiky, ktoré dohliada na prístupnosť "
                           "webových sídiel verejnej správy."]),
        ]},
    },
}


def jc_page(name):
    """One of the connector's pages in the page's language, English when there is none; None for
    a name that is not one. The hub, the domain and the contact come from the branding block."""
    words = PAGES.get(name)
    if not words:
        return None
    words = words.get(_language()) or words["en"]
    mcp = jc_mcp_install()
    domain = BRANDING.get("domain") if mcp else None
    email = BRANDING.get("contactEmail") or ""
    sections = []
    for heading, paragraphs in words["sections"]:
        kept = [p.format(url=mcp["url"], domain=domain) if mcp else p
                for p in paragraphs if mcp or ("{url}" not in p and "{domain}" not in p)]
        if kept:
            sections.append({"heading": heading, "paragraphs": kept})
    # The connector's questions are the instance's own, about its own publishers (T-4636).
    samples = _samples(_language()) if name == "connector" else []
    if samples:
        sections.append({"heading": jc_t("try_it"), "paragraphs": samples})
    return {"name": name, "title": words["title"], "sections": sections,
            "email": email if "@" in email else "", "mcp": mcp, "pages": list(PAGES)}


def jc_portal_url():
    """The Portal of this installation, for staff who signed in; None without a domain."""
    domain = BRANDING.get("domain")
    return "https://portal.%s/" % domain if domain else None


ASSISTANT_WIDGET = os.environ.get("JC_ASSISTANT_WIDGET_URL", "")
PUBLIC_ID = r"[a-z0-9](?:[-a-z0-9]{0,61}[a-z0-9])?"


def _widgets():
    """Each CKAN organization's own widget address (T-4281); empty when unset or unreadable."""
    try:
        found = json.loads(os.environ.get("JC_ASSISTANT_WIDGET_URLS") or "{}")
    except ValueError:
        return {}
    return {k: v for k, v in found.items() if isinstance(k, str) and isinstance(v, str)} if isinstance(found, dict) else {}


ASSISTANT_WIDGETS = _widgets()


def jc_assistant_widget(organization=None):
    """The knowledge assistant's widget page on this installation's assistant host (AG-114): the
    publisher's own when `organization` has one (T-4281), else the catalogue's; None when there
    is none, or when the address names any other host or path."""
    domain = BRANDING.get("domain")
    widget = ASSISTANT_WIDGETS.get(organization or "") or ASSISTANT_WIDGET
    if not domain or not widget:
        return None
    pattern = r"https://assistant\.%s/d/%s/widget" % (re.escape(domain), PUBLIC_ID)
    return widget if re.fullmatch(pattern, widget) else None


def jc_branding():
    """The block, for the templates."""
    return BRANDING


# The session key `ckanext-oidc-pkce` writes its reason under, and each reason it writes
# (views.py of 0.4.1, pinned in images/ckan/Dockerfile) mapped to words a person can act on.
SSO_ERROR = "ckanext:oidc-pkce:error"
SSO_REASONS = {
    "Login process was not started properly": "sso_cookie",
    "The app state does not match": "sso_state",
    "Unsupported token type. Should be 'Bearer'.": "sso_refused",
    "No access token returned from the token endpoint.": "sso_refused",
    "Unique user not found": "sso_account",
}


def jc_sso_error(session=None):
    """Why the last single sign-on failed, once, in the page's language; None when it did not.

    The raw reason stays in the extension's log line: it can name the identity service's own
    error, which is for the administrator and not for the page.
    """
    if session is None:
        from ckan.common import session
    reason = session.pop(SSO_ERROR, None)
    if not reason:
        return None
    return jc_t(SSO_REASONS.get(reason, "sso_other"))


# Word's Symbol font writes bullets as private-use code points (U+F0B7 and kin); a page without
# that font draws a box ("□ Druh □" on data.dev, T-4283). Publishers' descriptions arrive with
# them, so the catalogue maps the common ones and drops any other private-use character.
SYMBOL_GLYPHS = {"\uf0b7": "•", "\uf0a7": "▪", "\uf0a8": "◦", "\uf0d8": "›", "\uf0fc": "✓", "\uf076": "❖"}
PRIVATE_USE = re.compile("[\ue000-\uf8ff]")


def jc_plain_glyphs(text):
    """The text with Symbol-font bullets as real ones and no other private-use character."""
    if not isinstance(text, str):
        return text
    return PRIVATE_USE.sub(lambda m: SYMBOL_GLYPHS.get(m.group(), ""), text)


def jc_clean_dataset(pkg):
    """A dataset's words, and its resources', without glyphs no font draws."""
    for key in ("title", "notes"):
        if key in pkg:
            pkg[key] = jc_plain_glyphs(pkg[key])
    for resource in pkg.get("resources") or []:
        for key in ("name", "description"):
            if key in resource:
                resource[key] = jc_plain_glyphs(resource[key])
    return pkg


# The media types of what a person subscribes to (EP-125, EP-150): a calendar, or a feed of news.
CALENDAR_TYPES = {"text/calendar"}
FEED_TYPES = {"application/atom+xml", "application/rss+xml", "application/feed+json"}


def jc_subscriptions(pkg):
    """The dataset's calendar and feed resources, as `jcctl` publishes them one per output and
    format (T-5203): `{"calendar": [...], "news": [...]}`, each item `name`, `url` (https only)
    and `mimetype`. Both empty for a dataset whose items do not appear over time."""
    found = {"calendar": [], "news": []}
    for resource in (pkg or {}).get("resources") or []:
        mimetype = (resource.get("mimetype") or "").split(";")[0].strip().lower()
        url = resource.get("url") or ""
        if urlsplit(url).scheme != "https":
            continue
        group = "calendar" if mimetype in CALENDAR_TYPES else "news" if mimetype in FEED_TYPES else None
        if group:
            found[group].append({"name": resource.get("name") or url, "url": url, "mimetype": mimetype})
    return found


def _feed_href(href, site_url):
    """A link of CKAN's own feed made right (T-5203): absolute on `ckan.site_url`, and CKAN
    2.11's `?query=page&query=2` (views/feed.py `_feed_url`) and float `page=5.0` as `?page=2`."""
    if not href:
        return href
    parts = urlsplit(href)
    pairs = parse_qs(parts.query, keep_blank_values=True)
    if list(pairs) == ["query"] and len(pairs["query"]) % 2 == 0:
        flat = pairs["query"]
        pairs = {flat[i]: [flat[i + 1]] for i in range(0, len(flat), 2)}
    if "page" in pairs:
        try:
            pairs["page"] = [str(int(float(pairs["page"][0])))]
        except ValueError:
            pairs.pop("page")
    query = urlencode([(key, value) for key, values in pairs.items() for value in values])
    base = urlsplit(site_url.rstrip("/") + "/") if site_url else None
    scheme = parts.scheme or (base.scheme if base else "")
    netloc = parts.netloc or (base.netloc if base else "")
    return urlunsplit((scheme, netloc, parts.path, query, parts.fragment))


def jc_feed_class(base, site_url, language, author):
    """CKAN's Atom feed class (`ckan.views.feed.CKANFeed`) with the catalogue's language, the
    catalogue as the feed's author, every link absolute and paging links a reader can follow, and
    each entry's publisher as its author and its page as its link (T-5203)."""

    class JcFeed(base):
        def __init__(self, *args, **kwargs):
            kwargs["language"] = language or kwargs.get("language")
            kwargs["author_name"] = author or kwargs.get("author_name")
            for key in ("feed_link", "feed_url", "previous_page", "next_page", "first_page", "last_page"):
                if kwargs.get(key):
                    kwargs[key] = _feed_href(kwargs[key], site_url)
            if len(args) > 1:
                args = (args[0], _feed_href(args[1], site_url)) + tuple(args[2:])
            super().__init__(*args, **kwargs)

        def add_item(self, **kwargs):
            publisher = kwargs.pop("jc_publisher", None)
            page = kwargs.pop("jc_page", None)
            if page:
                kwargs["link"] = page
            if publisher:
                kwargs["author_name"] = publisher
                kwargs.pop("author_email", None)
            elif not kwargs.get("author_name"):
                kwargs.pop("author_name", None)
            if not kwargs.get("author_email"):
                kwargs.pop("author_email", None)
            super().add_item(**kwargs)

    return JcFeed


class JcThemePlugin(plugins.SingletonPlugin):
    plugins.implements(plugins.IConfigurer)
    plugins.implements(plugins.ITemplateHelpers)
    plugins.implements(plugins.IFacets, inherit=True)
    plugins.implements(plugins.IPackageController, inherit=True)
    plugins.implements(plugins.IBlueprint)
    plugins.implements(plugins.IResourceView, inherit=True)
    plugins.implements(plugins.IFeed, inherit=True)

    # CKAN's catalogue feed /feeds/dataset.atom (T-5203).
    def get_feed_class(self):
        from ckan.views.feed import CKANFeed

        config = toolkit.config
        site_title = jc_site_title((BRANDING.get("languages") or {}).get("default"))
        return jc_feed_class(
            CKANFeed,
            config.get("ckan.site_url") or "",
            config.get("ckan.locale_default") or "en",
            site_title or config.get("ckan.site_title"),
        )

    def get_item_additional_fields(self, pkg):
        fields = {"jc_page": toolkit.url_for("dataset.read", id=pkg.get("name") or pkg["id"], _external=True)}
        publisher = (pkg.get("organization") or {}).get("title")
        if publisher:
            fields["jc_publisher"] = publisher
        return fields

    def update_config(self, config):
        toolkit.add_template_directory(config, "templates")
        # The logo is a file beside this package, so CKAN serves it from its own origin and
        # no page reaches a third party for an asset.
        toolkit.add_public_directory(config, "public")
        # MapLibre, baked into the image beside CKAN (T-4639); without it there is no map.
        if os.path.isdir(VENDOR):
            toolkit.add_public_directory(config, VENDOR)
        # CKAN keeps one title; the pages ask jc_site_title for the reader's language (T-4636).
        title = jc_site_title((BRANDING.get("languages") or {}).get("default"))
        if title:
            config["ckan.site_title"] = title
            config["ckan.site_description"] = BRANDING.get("organisation", "")
        public = os.path.join(os.path.dirname(__file__), "public")
        logo = BRANDING.get("logo")
        if logo and os.path.exists(os.path.join(public, logo)):
            config["ckan.site_logo"] = "/" + logo
        favicon = BRANDING.get("favicon")
        if favicon and os.path.exists(os.path.join(public, favicon)):
            config["ckan.favicon"] = "/" + favicon
        languages = BRANDING.get("languages") or {}
        offered = _available(languages.get("offered") or [])
        default = languages.get("default")
        if default in offered:
            config["ckan.locale_default"] = default
        if offered:
            config["ckan.locales_offered"] = " ".join(offered)

    def get_helpers(self):
        return {
            "jc_branding": jc_branding,
            "jc_org": jc_org,
            "jc_t": jc_t,
            "jc_about": jc_about,
            "jc_created": jc_created,
            "jc_updated": jc_updated,
            "jc_more": jc_more,
            "jc_live": jc_live,
            "jc_resource_sections": jc_resource_sections,
            "jc_resource_label": jc_resource_label,
            "jc_is_api": jc_is_api,
            "jc_extra": jc_extra,
            "jc_is_source": jc_is_source,
            "jc_kind": jc_kind,
            "jc_entity_type": jc_entity_type,
            "jc_what": jc_what,
            "jc_format": jc_format,
            "jc_address": jc_address,
            "jc_pieces": jc_pieces,
            "jc_preview": jc_preview,
            "jc_examples": jc_examples,
            "jc_standard": jc_standard,
            "jc_model": jc_model,
            "jc_dates": jc_dates,
            "jc_usage": jc_usage,
            "jc_guide": jc_guide,
            "jc_developer_page": jc_developer_page,
            "jc_subscriptions": jc_subscriptions,
            "jc_previews": jc_previews,
            "jc_visualizations": jc_visualizations,
            "jc_explorer": jc_explorer,
            "jc_map": jc_map,
            "jc_total": jc_total,
            "jc_number": jc_number,
            "jc_rows": jc_rows,
            "jc_formats": jc_formats,
            "jc_format_chips": jc_format_chips,
            "jc_topic_tiles": jc_topic_tiles,
            "jc_schema_org": jc_schema_org,
            "jc_recent": jc_recent,
            "jc_portal_url": jc_portal_url,
            "jc_mcp_install": jc_mcp_install,
            "jc_assistants": jc_assistants,
            "jc_page": jc_page,
            "jc_site_title": jc_site_title,
            "jc_site_tagline": jc_site_tagline,
            "jc_site_logo": jc_site_logo,
            "jc_assistant_widget": jc_assistant_widget,
            "jc_csp_nonce": jc_csp_nonce,
            "jc_about_site": jc_about_site,
            "humanize_entity_type": humanize_entity_type,
            "jc_has_topics": jc_has_topics,
            "jc_sso_error": jc_sso_error,
        }

    def info(self):
        """`jc_visualization` (ADR-N-053, EP-132): a Vega-Lite or Vega chart the reconciler publishes
        from an Endpoint's `spec.catalog.visualizations`. The config holds the spec, its source
        and the Endpoint; the page shows a frame of the renderer on viz.{domain}."""
        ignore, text = toolkit.get_validator("ignore_missing"), toolkit.get_validator("unicode_safe")
        schema = {name: [ignore, text, _viz_object] for name in ("spec", "source", "titles", "descriptions")}
        schema["endpoint"] = [ignore, text, _viz_slug]
        # The publisher sends `jc_name` on every create and update (EP-144); without it here CKAN
        # refused each view with 409 `__extras` (T-4846).
        schema["jc_name"] = [ignore, text, _viz_name]
        return {"name": "jc_visualization", "title": "Visualization", "icon": "chart-line",
                "iframed": False, "always_available": False, "preview_enabled": False, "schema": schema}

    def can_view(self, data_dict):
        """Never offered in CKAN's "New view" list, never created as a default view: a chart is
        published from its Endpoint's manifest, reviewed there (ADR-N-053 §2.7)."""
        return False

    def view_template(self, context, data_dict):
        return "jc/visualization.html"

    def setup_template_variables(self, context, data_dict):
        return {"viz": jc_visualization(data_dict.get("resource_view"))}

    def get_blueprint(self):
        """/connector, /privacy, /terms, /support (T-4315) and /accessibility (T-5108), one template for all five,
        /assistants (T-4585), a guide per assistant at /assistants/{id} (T-4643) and
        /map/{dataset} (T-4639), /sitemap.xml and /robots.txt (T-5123)."""
        from flask import Blueprint

        blueprint = Blueprint("jc_pages", self.__module__)
        blueprint.after_app_request(jc_csp_header)
        for name in PAGES:
            blueprint.add_url_rule("/" + name, name, lambda name=name: toolkit.render("jc/page.html", {"jc_name": name}))
        blueprint.add_url_rule("/assistants", "assistants", lambda: toolkit.render("jc/assistants.html", {}))
        blueprint.add_url_rule("/assistants/<id>", "guide", jc_guide_page)
        # The framable map of a dataset, or of one of its tables (T-4639).
        blueprint.add_url_rule("/map/<id>", "map", jc_map_page)
        blueprint.add_url_rule("/map/<id>/<resource_id>", "map_resource", jc_map_page)
        # Datasets findable from outside (T-5123).
        blueprint.add_url_rule("/sitemap.xml", "sitemap", jc_sitemap_page)
        blueprint.add_url_rule("/robots.txt", "robots", jc_robots_page)
        return blueprint

    def before_dataset_search(self, search_params):
        """The hidden packages left out (T-4318), and a hyphenated word searched as the phrase it is and a Slovak or Czech word with its other
        endings, on the page and the API alike. A stem is a prefix query, which CKAN's default
        dismax parser does not take; edismax does, with the same fields and minimum match."""
        jc_hide(search_params)
        q = search_params.get("q")
        if q:
            search_params["q"] = jc_search_words(q)
            if "*)" in search_params["q"] and "*)" not in q:
                search_params.setdefault("defType", "edismax")
        return search_params

    def after_dataset_show(self, context, pkg_dict):
        if pkg_dict.get("name") in HIDDEN:
            reach = jc_hidden_reach()
            if reach is not None and pkg_dict.get("owner_org") not in reach:
                raise toolkit.ObjectNotFound("Dataset not found")
        jc_clean_dataset(pkg_dict)

    def after_dataset_search(self, search_results, search_params):
        for pkg in search_results.get("results") or []:
            jc_clean_dataset(pkg)
        return jc_facet_labels(search_results, search_params)

    def before_dataset_index(self, pkg_dict):
        """The served formats (T-5649) and the EU themes (T-5294) as filters of their own. `data_dict` is the
        dataset as CKAN stored it, the index fields beside it are flattened."""
        try:
            stored = json.loads(pkg_dict.get("data_dict") or "{}")
        except ValueError:
            stored = {}
        pkg_dict["vocab_jc_format"] = jc_data_formats(stored)
        pkg_dict["vocab_jc_topic"] = jc_topics(stored)
        return pkg_dict

    def dataset_facets(self, facets_dict, package_type):
        """The search filters in plain words: Publisher, Topic, Keywords, Format, Licence. Topic
        is the EU theme (T-5294) and Format the formats a dataset serves its data in (T-5649):
        CKAN's groups stay empty here and its file formats count the data model's documents."""
        names = {"organization": "publisher", "vocab_jc_topic": "topic", "tags": "keywords",
                 "vocab_jc_format": "format", "license_id": "licence"}
        swap = {"groups": "vocab_jc_topic", "res_format": "vocab_jc_format"}
        renamed = type(facets_dict)()
        for field in facets_dict:
            field = swap.get(field, field)
            renamed[field] = jc_t(names[field]) if field in names else facets_dict.get(field)
        return renamed
